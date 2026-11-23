//! `duck-calc`: fórmulas (ADR-003).
//!
//! A parte de **referências** das fórmulas:
//!
//! - [`shift`]: desloca as referências relativas (usado nas fórmulas
//!   compartilhadas, cujo texto o Excel grava só na célula-mestre).
//! - [`to_template`] / [`from_template`]: converte uma fórmula A1 num
//!   **modelo relativo** (cada referência vira um deslocamento a partir da
//!   célula, como no R1C1) e de volta. Fórmulas copiadas para baixo, como
//!   `A2*2`, `A3*2`, `A4*2`, viram o mesmo modelo e são guardadas uma vez só.
//!
//! E o **motor de cálculo** (F4):
//!
//! - [`parse`]: texto → fórmula compilada, independente da célula.
//! - [`eval`]: avaliação ([`Book`], [`Engine`], [`Eval`]).
//! - [`funcs`]: as funções de planilha.
//! - [`index`]: índices de busca (PROCV, SOMASES...) e critérios.
//! - [`recalc`]: grafo de dependências e recálculo em paralelo.
//! - [`valid`]: validação de dados (listas e valores aceitos).

pub mod valid;

pub mod cellops;
pub mod chartdata;
pub mod dynarr;
pub mod editing;
pub mod eval;
pub mod fns;
pub mod funcs;
pub mod index;
pub mod locale;
pub mod parse;
pub mod pivot;
pub mod recalc;
pub mod sheetrefs;
pub mod tabs;
pub mod value;

pub use eval::{Book, Engine, Eval, Overlay};
pub use recalc::{Dirty, Graph, Outcome};

use std::borrow::Cow;
use std::sync::Arc;

use duck_core::{Axis, FormulaKind, FormulaOps, Sheet, Shift};

/// Maior coluna (XFD) e linha do Excel, base 1.
const MAX_COL: i64 = 16_384;
const MAX_ROW: i64 = 1_048_576;

/// Texto da fórmula da célula (sem o `=`), já em notação A1 para a posição
/// da célula: deslocado se for de um grupo compartilhado, remontado se
/// estiver guardado como modelo relativo.
pub fn cell_formula(sheet: &Sheet, row: u32, col: u32) -> Option<Cow<'_, str>> {
    let f = sheet.formula(row, col)?;
    if f.template {
        return Some(Cow::Owned(from_template(f.text, row, col)));
    }
    if matches!(f.kind, FormulaKind::Shared { .. }) && (row, col) != (f.anchor_row, f.anchor_col) {
        let dr = row as i64 - f.anchor_row as i64;
        let dc = col as i64 - f.anchor_col as i64;
        return Some(Cow::Owned(shift(f.text, dr, dc)));
    }
    Some(Cow::Borrowed(f.text))
}

// ---------------------------------------------------------------------------
// Tokenizador de referências
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellRef {
    /// Base 1.
    col: i64,
    row: i64,
    abs_col: bool,
    abs_row: bool,
}

/// Pedaço de uma fórmula: texto que fica como está, ou uma referência.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Cell(CellRef, &'a str),
    /// Coluna inteira num intervalo (`A` em `A:C`).
    Col {
        col: i64,
        abs: bool,
        raw: &'a str,
    },
    /// Linha inteira num intervalo (`2` em `2:5`).
    Row {
        row: i64,
        abs: bool,
        raw: &'a str,
    },
}

/// Percorre a fórmula entregando texto e referências na ordem.
fn walk<'a>(formula: &'a str, mut f: impl FnMut(Piece<'a>)) {
    let b = formula.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    let mut pending_col_range = false;
    let mut pending_row_range = false;
    let flush = |f: &mut dyn FnMut(Piece<'a>), from: usize, to: usize| {
        if to > from {
            f(Piece::Text(&formula[from..to]));
        }
    };

    while i < b.len() {
        let c = b[i];
        match c {
            b'"' => i = skip_quoted(b, i, b'"'),
            b'\'' => i = skip_quoted(b, i, b'\''),
            b'[' => i = skip_brackets(b, i),
            b'#' => {
                i += 1;
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'/' | b'!' | b'?' | b'_'))
                {
                    i += 1;
                }
            }
            _ if is_ident_start(c) => {
                let mut j = i;
                while j < b.len() && is_ident_char(b[j]) {
                    j += 1;
                }
                let tok = &formula[i..j];
                let next = b.get(j).copied();
                let prev_colon = i > 0 && b[i - 1] == b':';
                let token_after_colon = || {
                    let mut k = j + 1;
                    while k < b.len() && is_ident_char(b[k]) {
                        k += 1;
                    }
                    (j + 1, k)
                };
                let sheet_range = next == Some(b':') && {
                    let (s, e) = token_after_colon();
                    e > s && b.get(e) == Some(&b'!')
                };
                let col_only_next = next == Some(b':') && {
                    let (s, e) = token_after_colon();
                    is_col_only(&b[s..e])
                };
                let row_only_next = next == Some(b':') && {
                    let (s, e) = token_after_colon();
                    is_row_only(&b[s..e])
                };

                let mut piece = None;
                let mut keep_col = false;
                let mut keep_row = false;
                if next == Some(b'(') || next == Some(b'!') || sheet_range {
                    // Função ou nome de aba: texto.
                } else if let Some(r) = parse_cell(tok.as_bytes()) {
                    piece = Some(Piece::Cell(r, tok));
                } else if is_col_only(tok.as_bytes()) && ((prev_colon && pending_col_range) || col_only_next)
                {
                    let abs = tok.starts_with('$');
                    let col = col_number(tok.trim_start_matches('$'));
                    piece = Some(Piece::Col { col, abs, raw: tok });
                    keep_col = next == Some(b':');
                } else if is_row_only(tok.as_bytes()) && ((prev_colon && pending_row_range) || row_only_next)
                {
                    let abs = tok.starts_with('$');
                    let row = tok.trim_start_matches('$').parse().unwrap_or(0);
                    piece = Some(Piece::Row { row, abs, raw: tok });
                    keep_row = next == Some(b':');
                }
                if let Some(p) = piece {
                    flush(&mut f, text_start, i);
                    f(p);
                    text_start = j;
                }
                pending_col_range = keep_col;
                pending_row_range = keep_row;
                i = j;
            }
            _ => {
                i += 1;
            }
        }
    }
    flush(&mut f, text_start, b.len());
}

fn skip_quoted(b: &[u8], start: usize, q: u8) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        if b[i] == q {
            if b.get(i + 1) == Some(&q) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    b.len()
}

fn skip_brackets(b: &[u8], start: usize) -> usize {
    let mut depth = 0;
    let mut i = start;
    while i < b.len() {
        match b[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            // Dentro de referência estruturada, ' escapa o caractere seguinte.
            b'\'' => i += 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Caracteres de identificador (nomes, funções, referências). Bytes não-ASCII
/// fazem parte de nomes (ex.: abas e nomes definidos com acento).
fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'$' | b'\\' | b'?') || c >= 0x80
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b'\\') || c >= 0x80
}

/// `$A$1`, `A1`, `$AB12`... (coluna de 1 a 3 letras até XFD, linha até 1048576).
fn parse_cell(t: &[u8]) -> Option<CellRef> {
    let mut i = 0;
    let abs_col = t.first() == Some(&b'$');
    if abs_col {
        i += 1;
    }
    let c0 = i;
    let mut col: i64 = 0;
    while i < t.len() && t[i].is_ascii_alphabetic() {
        col = col * 26 + (t[i].to_ascii_uppercase() - b'A') as i64 + 1;
        i += 1;
    }
    if i == c0 || i - c0 > 3 || col > MAX_COL {
        return None;
    }
    let abs_row = t.get(i) == Some(&b'$');
    if abs_row {
        i += 1;
    }
    let r0 = i;
    let mut row: i64 = 0;
    while i < t.len() && t[i].is_ascii_digit() {
        row = row * 10 + (t[i] - b'0') as i64;
        if row > MAX_ROW {
            return None;
        }
        i += 1;
    }
    if i == r0 || i != t.len() || row == 0 {
        return None;
    }
    Some(CellRef { col, row, abs_col, abs_row })
}

fn is_col_only(t: &[u8]) -> bool {
    let t = t.strip_prefix(b"$").unwrap_or(t);
    !t.is_empty() && t.len() <= 3 && t.iter().all(u8::is_ascii_alphabetic)
}

fn is_row_only(t: &[u8]) -> bool {
    let t = t.strip_prefix(b"$").unwrap_or(t);
    !t.is_empty() && t.iter().all(u8::is_ascii_digit)
}

fn col_number(letters: &str) -> i64 {
    letters.bytes().fold(0i64, |a, c| a * 26 + (c.to_ascii_uppercase() - b'A') as i64 + 1)
}

fn push_col(s: &mut String, col: i64) {
    let mut c = col;
    let mut buf = [0u8; 4];
    let mut n = 0;
    while c > 0 {
        let r = (c - 1) % 26;
        buf[n] = b'A' + r as u8;
        n += 1;
        c = (c - 1) / 26;
    }
    for i in (0..n).rev() {
        s.push(buf[i] as char);
    }
}

/// Escreve uma referência A1; fora da planilha vira `#REF!`.
fn push_cell(out: &mut String, col: i64, row: i64, abs_col: bool, abs_row: bool) {
    if !(1..=MAX_COL).contains(&col) || !(1..=MAX_ROW).contains(&row) {
        out.push_str("#REF!");
        return;
    }
    if abs_col {
        out.push('$');
    }
    push_col(out, col);
    if abs_row {
        out.push('$');
    }
    out.push_str(itoa(row).as_str());
}

fn itoa(n: i64) -> String {
    n.to_string()
}

// ---------------------------------------------------------------------------
// Deslocamento
// ---------------------------------------------------------------------------

/// Desloca as referências relativas (sem `$`) de uma fórmula em notação A1.
/// Referências que sairiam da planilha viram `#REF!`, como no Excel.
pub fn shift(formula: &str, drow: i64, dcol: i64) -> String {
    if drow == 0 && dcol == 0 {
        return formula.to_string();
    }
    let mut out = String::with_capacity(formula.len() + 8);
    walk(formula, |p| match p {
        Piece::Text(t) => out.push_str(t),
        Piece::Cell(r, _) => {
            let col = if r.abs_col { r.col } else { r.col + dcol };
            let row = if r.abs_row { r.row } else { r.row + drow };
            push_cell(&mut out, col, row, r.abs_col, r.abs_row);
        }
        Piece::Col { col, abs, raw } => {
            if abs {
                out.push_str(raw);
            } else if (1..=MAX_COL).contains(&(col + dcol)) {
                push_col(&mut out, col + dcol);
            } else {
                out.push_str("#REF!");
            }
        }
        Piece::Row { row, abs, raw } => {
            if abs {
                out.push_str(raw);
            } else if (1..=MAX_ROW).contains(&(row + drow)) {
                out.push_str(&itoa(row + drow));
            } else {
                out.push_str("#REF!");
            }
        }
    });
    out
}

// ---------------------------------------------------------------------------
// Inserir e apagar linhas e colunas (F3.5)
// ---------------------------------------------------------------------------

/// Nome da aba que qualifica a referência, pelo texto logo antes dela
/// (`Plan1!`, `'Minha Aba'!`). `Some(None)` = sem nome (a aba da fórmula);
/// `None` = referência que não se ajusta (outro arquivo, várias abas).
fn qualifier(before: &str) -> Option<Option<String>> {
    let Some(head) = before.strip_suffix('!') else { return Some(None) };
    if let Some(q) = head.strip_suffix('\'') {
        // Nome entre aspas, com aspas dobradas dentro.
        let b = q.as_bytes();
        let mut i = b.len();
        while i > 0 {
            i -= 1;
            if b[i] == b'\'' {
                if i > 0 && b[i - 1] == b'\'' {
                    i -= 1;
                    continue;
                }
                let name = q[i + 1..].replace("''", "'");
                return (!name.contains([']', ':'])).then_some(Some(name));
            }
        }
        return None;
    }
    let start =
        head.rfind(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '\\'))).map_or(0, |i| i + 1);
    let name = &head[start..];
    if name.is_empty() || head[..start].ends_with([']', ':']) {
        return None;
    }
    Some(Some(name.to_string()))
}

/// Ajusta as referências de uma fórmula (A1) a uma mudança de estrutura na
/// aba `sheet` (`own` = a fórmula é dessa aba, então referências sem nome de
/// aba também contam). Devolve o texto novo, se mudar, e se algo se perdeu
/// (`#REF!` ou intervalo encolhido).
pub fn adjust_refs(formula: &str, s: Shift, own: bool, sheet: &str) -> Option<(String, bool)> {
    let mut pieces = Vec::new();
    walk(formula, |p| pieces.push(p));
    if !pieces.iter().any(|p| !matches!(p, Piece::Text(_))) {
        return None;
    }
    let mut out = String::with_capacity(formula.len() + 8);
    let mut lossy = false;
    let mut changed = false;
    let mut i = 0;
    let rows = s.axis == Axis::Rows;
    while i < pieces.len() {
        let p = pieces[i];
        let prev_text = match i.checked_sub(1).map(|k| pieces[k]) {
            Some(Piece::Text(t)) => t,
            _ => "",
        };
        let applies = |prev: &str| match qualifier(prev) {
            Some(Some(name)) => name.eq_ignore_ascii_case(sheet),
            Some(None) => own,
            None => false,
        };
        // Intervalo `X:Y` (duas referências separadas só por `:`).
        let range_end = match (pieces.get(i + 1), pieces.get(i + 2)) {
            (Some(Piece::Text(":")), Some(q)) if !matches!(q, Piece::Text(_)) => Some(*q),
            _ => None,
        };
        match p {
            Piece::Text(t) => {
                out.push_str(t);
                i += 1;
                continue;
            }
            _ if !applies(prev_text) => {
                // Copia como está (com o fim do intervalo, se houver).
                push_raw(&mut out, p);
                if let Some(q) = range_end {
                    out.push(':');
                    push_raw(&mut out, q);
                    i += 3;
                } else {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        // Coordenadas no eixo (base 0) do começo e do fim.
        let axis_of = |p: Piece| -> Option<i64> {
            match p {
                Piece::Cell(r, _) => Some(if rows { r.row } else { r.col } - 1),
                Piece::Col { col, .. } if !rows => Some(col - 1),
                Piece::Row { row, .. } if rows => Some(row - 1),
                _ => None,
            }
        };
        let end = range_end.unwrap_or(p);
        match (axis_of(p), axis_of(end)) {
            (Some(a), Some(b)) => {
                let (lo, hi) = (a.min(b) as u32, a.max(b) as u32);
                let single = range_end.is_none();
                let mapped = if single { s.index(lo).map(|x| (x, x)) } else { s.span(lo, hi) };
                match mapped {
                    None => {
                        out.push_str("#REF!");
                        lossy = true;
                        changed = true;
                    }
                    Some((na, nb)) => {
                        if !single && !s.insert && (nb - na) != (hi - lo) {
                            lossy = true;
                        }
                        let (na, nb) = if a <= b { (na, nb) } else { (nb, na) };
                        changed |= na as i64 != a || nb as i64 != b;
                        push_with(&mut out, p, na as i64 + 1, rows);
                        if let Some(q) = range_end {
                            out.push(':');
                            push_with(&mut out, q, nb as i64 + 1, rows);
                        }
                    }
                }
            }
            // Peça que o eixo não move (ex.: `A:C` ao inserir linhas).
            _ => {
                push_raw(&mut out, p);
                if let Some(q) = range_end {
                    out.push(':');
                    push_raw(&mut out, q);
                }
            }
        }
        i += if range_end.is_some() { 3 } else { 1 };
    }
    changed.then_some((out, lossy))
}

fn push_raw(out: &mut String, p: Piece) {
    match p {
        Piece::Text(t) => out.push_str(t),
        Piece::Cell(_, raw) => out.push_str(raw),
        Piece::Col { raw, .. } | Piece::Row { raw, .. } => out.push_str(raw),
    }
}

/// Escreve a peça com a coordenada do eixo trocada por `v` (base 1).
fn push_with(out: &mut String, p: Piece, v: i64, rows: bool) {
    match p {
        Piece::Cell(r, _) => {
            let (row, col) = if rows { (v, r.col) } else { (r.row, v) };
            push_cell(out, col, row, r.abs_col, r.abs_row);
        }
        Piece::Col { abs, .. } => {
            if abs {
                out.push('$');
            }
            push_col(out, v);
        }
        Piece::Row { abs, .. } => {
            if abs {
                out.push('$');
            }
            out.push_str(&itoa(v));
        }
        Piece::Text(t) => out.push_str(t),
    }
}

/// As operações de fórmula para uma mudança de estrutura na aba `sheet`
/// (ver `duck_core::FormulaOps`).
#[derive(Clone, Debug)]
pub struct ShiftOps {
    pub shift: Shift,
    pub sheet: String,
    /// As fórmulas ajustadas são da própria aba.
    pub own: bool,
}

impl FormulaOps for ShiftOps {
    fn adjust(&self, a1: &str) -> Option<(String, bool)> {
        adjust_refs(a1, self.shift, self.own, &self.sheet)
    }

    fn make_template(&self, a1: &str, row: u32, col: u32) -> Option<String> {
        to_template(a1, row, col)
    }

    fn expand_template(&self, template: &str, row: u32, col: u32) -> String {
        from_template(template, row, col)
    }

    fn shift_a1(&self, a1: &str, drow: i64, dcol: i64) -> String {
        shift(a1, drow, dcol)
    }

    fn with_shift(&self, s: Shift) -> Arc<dyn FormulaOps> {
        Arc::new(ShiftOps { shift: s, ..self.clone() })
    }

    fn axis_refs(&self, template: &str, rows: bool) -> Vec<(bool, i64)> {
        axis_refs(template, rows)
    }
}

/// As referências de um modelo relativo num eixo: (absoluta, valor).
pub fn axis_refs(template: &str, rows: bool) -> Vec<(bool, i64)> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find([M_CELL, M_COL, M_ROW]) {
        let marker = rest[i..].chars().next().unwrap();
        let body_start = i + marker.len_utf8();
        let Some(end) = rest[body_start..].find(M_END) else { break };
        let body = &rest[body_start..body_start + end];
        let part = match marker {
            M_CELL => {
                let (c, r) = body.split_once(M_SEP).unwrap_or((body, "0"));
                Some(if rows { r } else { c })
            }
            M_COL if !rows => Some(body),
            M_ROW if rows => Some(body),
            _ => None,
        };
        if let Some(p) = part {
            let (abs, v) = read_part(p, 0);
            out.push((abs, v));
        }
        rest = &rest[body_start + end + M_END.len_utf8()..];
    }
    out
}

// ---------------------------------------------------------------------------
// Modelos relativos
// ---------------------------------------------------------------------------

// Marcadores de uso privado do Unicode: não aparecem em fórmulas do Excel.
const M_CELL: char = '\u{F000}';
const M_COL: char = '\u{F001}';
const M_ROW: char = '\u{F002}';
const M_SEP: char = '\u{F003}';
const M_END: char = '\u{F004}';

/// Uma parte (coluna ou linha) do modelo: absoluta guarda o valor, relativa o deslocamento.
fn push_part(out: &mut String, abs: bool, value: i64, origin: i64) {
    if abs {
        out.push('$');
        out.push_str(&itoa(value));
    } else {
        out.push_str(&itoa(value - origin));
    }
}

fn read_part(s: &str, origin: i64) -> (bool, i64) {
    match s.strip_prefix('$') {
        Some(v) => (true, v.parse().unwrap_or(0)),
        None => (false, origin + s.parse::<i64>().unwrap_or(0)),
    }
}

/// Converte a fórmula da célula (`row`, `col`, base 0) num modelo relativo.
/// Fórmulas sem referência (ex.: colunas calculadas de tabela, com
/// `[#This Row]`) viram o próprio texto, e as cópias idênticas se juntam.
/// Retorna `None` se a ida e volta não for exata (ex.: referência em
/// minúsculas ou com zeros à esquerda): aí a fórmula é guardada como texto.
pub fn to_template(formula: &str, row: u32, col: u32) -> Option<String> {
    if formula.contains([M_CELL, M_COL, M_ROW, M_SEP, M_END]) {
        return None;
    }
    let (r0, c0) = (row as i64 + 1, col as i64 + 1);
    let mut out = String::with_capacity(formula.len() + 8);
    let mut exact = true;
    walk(formula, |p| match p {
        Piece::Text(t) => out.push_str(t),
        Piece::Cell(r, raw) => {
            let mut canon = String::with_capacity(raw.len());
            push_cell(&mut canon, r.col, r.row, r.abs_col, r.abs_row);
            exact &= canon == raw;
            out.push(M_CELL);
            push_part(&mut out, r.abs_col, r.col, c0);
            out.push(M_SEP);
            push_part(&mut out, r.abs_row, r.row, r0);
            out.push(M_END);
        }
        Piece::Col { col, abs, raw } => {
            let mut canon = String::new();
            if abs {
                canon.push('$');
            }
            push_col(&mut canon, col);
            exact &= canon == raw;
            out.push(M_COL);
            push_part(&mut out, abs, col, c0);
            out.push(M_END);
        }
        Piece::Row { row: r, abs, raw } => {
            exact &= (if abs { format!("${r}") } else { itoa(r) }) == raw;
            out.push(M_ROW);
            push_part(&mut out, abs, r, r0);
            out.push(M_END);
        }
    });
    exact.then_some(out)
}

/// O modelo vale igual em qualquer célula (não tem referência relativa)?
/// Ex.: `$AJ$1*2` ou `Tabela1[[#This Row],[Valor]]`.
pub fn is_position_independent(template: &str) -> bool {
    let mut rest = template;
    while let Some(i) = rest.find([M_CELL, M_COL, M_ROW]) {
        let marker = rest[i..].chars().next().unwrap();
        let body_start = i + marker.len_utf8();
        let Some(end) = rest[body_start..].find(M_END) else { return false };
        let body = &rest[body_start..body_start + end];
        if !body.split(M_SEP).all(|p| p.starts_with('$')) {
            return false;
        }
        rest = &rest[body_start + end + M_END.len_utf8()..];
    }
    true
}

/// Remonta a fórmula A1 de um modelo relativo para a célula (`row`, `col`, base 0).
pub fn from_template(template: &str, row: u32, col: u32) -> String {
    let (r0, c0) = (row as i64 + 1, col as i64 + 1);
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find([M_CELL, M_COL, M_ROW]) {
        out.push_str(&rest[..i]);
        let marker = rest[i..].chars().next().unwrap();
        let body_start = i + marker.len_utf8();
        let Some(end) = rest[body_start..].find(M_END) else {
            rest = "";
            break;
        };
        let body = &rest[body_start..body_start + end];
        match marker {
            M_CELL => {
                let (c, r) = body.split_once(M_SEP).unwrap_or((body, "0"));
                let (abs_col, col) = read_part(c, c0);
                let (abs_row, row) = read_part(r, r0);
                push_cell(&mut out, col, row, abs_col, abs_row);
            }
            M_COL => {
                let (abs, col) = read_part(body, c0);
                if (1..=MAX_COL).contains(&col) {
                    if abs {
                        out.push('$');
                    }
                    push_col(&mut out, col);
                } else {
                    out.push_str("#REF!");
                }
            }
            _ => {
                let (abs, row) = read_part(body, r0);
                if (1..=MAX_ROW).contains(&row) {
                    if abs {
                        out.push('$');
                    }
                    out.push_str(&itoa(row));
                } else {
                    out.push_str("#REF!");
                }
            }
        }
        rest = &rest[body_start + end + M_END.len_utf8()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_relative_refs_only() {
        assert_eq!(shift("A1+B2", 1, 0), "A2+B3");
        assert_eq!(shift("$A1+A$1+$A$1", 2, 3), "$A3+D$1+$A$1");
        assert_eq!(shift("SUM(A1:B10)*2", 5, 1), "SUM(B6:C15)*2");
        assert_eq!(shift("A1", -1, 0), "#REF!");
    }

    #[test]
    fn leaves_functions_strings_and_names() {
        assert_eq!(shift("LOG10(A1)", 1, 0), "LOG10(A2)");
        assert_eq!(shift(r#"IF(A1="B2","C3",D4)"#, 1, 0), r#"IF(A2="B2","C3",D5)"#);
        assert_eq!(shift("Taxa*A1", 1, 0), "Taxa*A2");
        assert_eq!(shift("TRUE+A1", 0, 1), "TRUE+B1");
        assert_eq!(shift("A1&#REF!", 1, 0), "A2&#REF!");
    }

    #[test]
    fn sheet_prefixes() {
        assert_eq!(shift("Plan1!A1+'Minha Aba'!B2", 1, 1), "Plan1!B2+'Minha Aba'!C3");
        assert_eq!(shift("'It''s'!A1", 1, 0), "'It''s'!A2");
        assert_eq!(shift("Aba1:Aba3!A1", 0, 1), "Aba1:Aba3!B1");
        assert_eq!(shift("Ações!A1", 1, 0), "Ações!A2");
    }

    #[test]
    fn whole_columns_and_rows() {
        assert_eq!(shift("SUM(A:C)", 3, 1), "SUM(B:D)");
        assert_eq!(shift("SUM($A:C)", 3, 1), "SUM($A:D)");
        assert_eq!(shift("SUM(2:5)", 3, 1), "SUM(5:8)");
        assert_eq!(shift("COUNTIF(Plan1!B:B,A2)", 1, 0), "COUNTIF(Plan1!B:B,A3)");
    }

    #[test]
    fn structured_and_numbers() {
        assert_eq!(shift("Tabela1[[#This Row],[Valor]]*A1", 1, 0), "Tabela1[[#This Row],[Valor]]*A2");
        assert_eq!(shift("A1*1.5E+3+2", 1, 0), "A2*1.5E+3+2");
        assert_eq!(shift("[1]Plan1!A1", 1, 0), "[1]Plan1!A2");
    }

    #[test]
    fn not_cell_refs() {
        // 4 letras, coluna além de XFD, R1C1: não são referências.
        assert_eq!(shift("ABCD1+XFE1+R1C1", 1, 1), "ABCD1+XFE1+R1C1");
        assert_eq!(shift("XFD1", 0, 1), "#REF!");
    }

    #[test]
    fn structural_adjustments() {
        let ins = Shift { axis: Axis::Rows, at: 4, n: 2, insert: true }; // antes da linha 5
        let a = |f: &str, own: bool| adjust_refs(f, ins, own, "Dados").map(|x| x.0);
        assert_eq!(a("A1+A5+$B$10", true).as_deref(), Some("A1+A7+$B$12"));
        assert_eq!(a("SUM(A1:A10)", true).as_deref(), Some("SUM(A1:A12)"));
        assert_eq!(a("SUM(A1:A4)", true), None);
        assert_eq!(a("SUM(A:A)+SUM(5:6)", true).as_deref(), Some("SUM(A:A)+SUM(7:8)"));
        assert_eq!(a("A10", false), None, "outra aba, sem nome: não muda");
        assert_eq!(a("Dados!A10+'Outra'!A10", false).as_deref(), Some("Dados!A12+'Outra'!A10"));
        assert_eq!(a("\"A10\"&A10", true).as_deref(), Some("\"A10\"&A12"));

        let del = ins.inverse(); // apaga as linhas 5 e 6
        let d = |f: &str| adjust_refs(f, del, true, "Dados");
        assert_eq!(d("A5+A7"), Some(("#REF!+A5".into(), true)));
        assert_eq!(d("SUM(A1:A10)"), Some(("SUM(A1:A8)".into(), true)));
        assert_eq!(d("SUM(A5:A6)"), Some(("SUM(#REF!)".into(), true)));
        assert_eq!(d("SUM(A7:A9)"), Some(("SUM(A5:A7)".into(), false)));

        let cols = Shift { axis: Axis::Cols, at: 1, n: 1, insert: true }; // antes de B
        let c = |f: &str| adjust_refs(f, cols, true, "Dados").map(|x| x.0);
        assert_eq!(c("A1+B1+$C$2").as_deref(), Some("A1+C1+$D$2"));
        assert_eq!(c("SUM(A:C)").as_deref(), Some("SUM(A:D)"));
        assert_eq!(c("SUM(1:3)"), None);
    }

    #[test]
    fn copied_down_formulas_share_a_template() {
        let a = to_template("B2*2+$C$1+SUM(D:D)", 1, 0).unwrap();
        let b = to_template("B3*2+$C$1+SUM(D:D)", 2, 0).unwrap();
        let c = to_template("B4*2+$C$1+SUM(D:D)", 3, 0).unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert_eq!(from_template(&a, 9, 0), "B10*2+$C$1+SUM(D:D)");
        assert!(!is_position_independent(&a));
        assert!(is_position_independent(&to_template("$C$1*2+TODAY()", 4, 4).unwrap()));
        assert!(is_position_independent(&to_template("Tabela1[[#This Row],[Valor]]", 4, 4).unwrap()));
        assert!(!is_position_independent(&to_template("$C1", 4, 4).unwrap()));
    }

    #[test]
    fn template_roundtrip_is_exact() {
        let cases = [
            "A1",
            "$A$1+Plan1!B$2-'Minha Aba'!$C3",
            r#"IF(ISERROR(VLOOKUP(A2,Base!A:Z,3,0)),"",VLOOKUP(A2,Base!A:Z,3,0))"#,
            "SUM(2:5)+SUM($1:$1)+COUNTIF(B:B,\"x\")",
            "Tabela1[[#This Row],[Valor]]*A1",
            "XFD1048576+A1",
            "Aba1:Aba3!A1",
        ];
        for f in cases {
            for (r, c) in [(0u32, 0u32), (5, 7), (1_000_000, 100)] {
                if let Some(t) = to_template(f, r, c) {
                    assert_eq!(from_template(&t, r, c), f, "{f} em ({r},{c})");
                }
            }
        }
        // Sem referências: o modelo é o próprio texto.
        assert_eq!(
            to_template("Tabela1[[#This Row],[Valor]]*2", 7, 3).as_deref(),
            Some("Tabela1[[#This Row],[Valor]]*2")
        );
        // Fora do formato canônico: fica como texto.
        assert_eq!(to_template("a1+B2", 0, 0), None);
        assert_eq!(to_template("A01", 0, 0), None);
    }

    /// Aba de teste: A1..A3000 = números; B = A*2 (modelo); C2 = SUM(A1:A3000);
    /// D1..D3 = grupo compartilhado A1+1.
    fn shift_sheet() -> duck_core::Sheet {
        use duck_core::{FormulaKind, SheetPart, kind};
        let mut p = SheetPart::new();
        for r in 0..3000u32 {
            p.set(r, 0, kind::NUMBER, 0, (r as f64).to_bits());
            p.set(r, 1, kind::NUMBER | kind::FORMULA, 0, (2.0 * r as f64).to_bits());
            let t = to_template(&format!("A{}*2", r + 1), r, 1).unwrap();
            p.set_formula_template(r, 1, &t);
        }
        p.set(1, 2, kind::NUMBER | kind::FORMULA, 0, 1.0f64.to_bits());
        p.set_formula(1, 2, "SUM(A1:A3000)", FormulaKind::Normal);
        p.set(0, 3, kind::NUMBER | kind::FORMULA, 0, 1.0f64.to_bits());
        p.set_formula(0, 3, "A1+1", FormulaKind::Shared { si: 0 });
        for r in 1..3 {
            p.set(r, 3, kind::NUMBER | kind::FORMULA, 0, 1.0f64.to_bits());
            p.set_shared_formula(r, 3, 0);
        }
        let mut s = duck_core::Sheet::new("Dados");
        s.merge_part(p);
        s.layout.merges.push(duck_core::CellRange { first_row: 9, first_col: 4, last_row: 11, last_col: 5 });
        s.finish();
        s
    }

    fn snapshot(s: &duck_core::Sheet) -> Vec<String> {
        let sh = duck_core::StringTable::new();
        (0..3010)
            .flat_map(|r| (0..6).map(move |c| (r, c)))
            .map(|(r, c)| format!("{:?} {:?}", s.cell(r, c, &sh), cell_formula(s, r, c)))
            .collect()
    }

    #[test]
    fn insert_and_delete_rows_in_the_model() {
        use duck_core::{Command, OpsHandle};
        let mut s = shift_sheet();
        let before = snapshot(&s);
        let sh = duck_core::StringTable::new();
        let ops = |shift: Shift| OpsHandle(Arc::new(ShiftOps { shift, sheet: "Dados".into(), own: true }));

        // Inserir 2 linhas antes da linha 5 (base 0: 4).
        let ins = Shift { axis: Axis::Rows, at: 4, n: 2, insert: true };
        let undo = Command::Shift(ins, ops(ins)).apply(&mut s);
        assert_eq!(s.cell(6, 0, &sh).value, duck_core::Value::Number(4.0));
        assert_eq!(cell_formula(&s, 6, 1).as_deref(), Some("A7*2"));
        assert_eq!(cell_formula(&s, 3, 1).as_deref(), Some("A4*2"));
        assert_eq!(cell_formula(&s, 1, 2).as_deref(), Some("SUM(A1:A3002)"));
        assert_eq!(cell_formula(&s, 2, 3).as_deref(), Some("A3+1"));
        assert_eq!(s.layout.merges[0].first_row, 11);
        assert_eq!(s.rows(), 3002);
        assert!(s.edits().structure);
        undo.apply(&mut s);
        assert_eq!(snapshot(&s), before, "desfazer volta ao mesmo");

        // Apagar as linhas 2 e 3 (base 0: 1..3): C2 (a soma) some; D2/D3 também.
        let del = Shift { axis: Axis::Rows, at: 1, n: 2, insert: false };
        let undo = Command::Shift(del, ops(del)).apply(&mut s);
        assert_eq!(s.cell(1, 0, &sh).value, duck_core::Value::Number(3.0));
        assert_eq!(cell_formula(&s, 1, 1).as_deref(), Some("A2*2"));
        assert_eq!(cell_formula(&s, 1, 2), None);
        assert_eq!(cell_formula(&s, 0, 3).as_deref(), Some("A1+1"));
        undo.apply(&mut s);
        assert_eq!(snapshot(&s), before, "desfazer devolve as linhas apagadas");

        // Colunas: inserir antes de B.
        let ic = Shift { axis: Axis::Cols, at: 1, n: 1, insert: true };
        let undo = Command::Shift(ic, ops(ic)).apply(&mut s);
        assert_eq!(cell_formula(&s, 5, 2).as_deref(), Some("A6*2"));
        assert_eq!(cell_formula(&s, 1, 3).as_deref(), Some("SUM(A1:A3000)"));
        assert_eq!(s.layout.merges[0].first_col, 5);
        undo.apply(&mut s);
        assert_eq!(snapshot(&s), before);

        // Apagar a coluna A: as fórmulas que usavam A viram #REF!.
        let dc = Shift { axis: Axis::Cols, at: 0, n: 1, insert: false };
        let undo = Command::Shift(dc, ops(dc)).apply(&mut s);
        assert_eq!(cell_formula(&s, 5, 0).as_deref(), Some("#REF!*2"));
        undo.apply(&mut s);
        assert_eq!(snapshot(&s), before);
    }
}
