//! Referências por clique: com uma fórmula aberta, clicar, arrastar ou andar
//! com as setas na grade escreve a referência no lugar do cursor.
//!
//! - Só onde uma referência cabe (depois de `=`, operador, `(` ou `;`, ver
//!   [`crate::complete::Analysis::accepts`]); em outro lugar o clique confirma
//!   a edição.
//! - A referência recém-apontada fica em [`Point`]: o próximo clique ou seta a
//!   troca, em vez de somar outra. Qualquer tecla que mexe no texto a fixa.
//! - Dá para apontar em outra aba: a edição continua na célula de origem
//!   ([`Editor::sheet`]) e confirmar ou cancelar volta para ela.
//! - Cada referência da fórmula tem uma cor, no texto e na grade.

use std::fmt::Write as _;
use std::ops::Range;

use duck_core::CellRange;
use duck_platform::Modifiers;
use duck_ui::{Painter, Rect, Rgba};

use crate::app::{Drag, Geom, MAX_COLS, MAX_ROWS, Merges, Quad, SheetsApp, View, cell_name, col_name};
use crate::complete::analyze;
use crate::edit::Editor;
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cells,
    /// Colunas inteiras (`B:C`).
    Cols,
    /// Linhas inteiras (`3:5`).
    Rows,
}

/// Referência sendo apontada.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// Onde ela está no texto (com o nome da aba).
    pub range: Range<usize>,
    /// Aba apontada.
    pub sheet: usize,
    pub kind: Kind,
    pub anchor: (u32, u32),
    pub cur: (u32, u32),
}

// ---------------------------------------------------------------------------
// Escrever
// ---------------------------------------------------------------------------

/// O retângulo de `a` até `b`, sem cortar células mescladas. Uma célula
/// mesclada sozinha é só o canto dela.
pub fn area(merges: &Merges, kind: Kind, a: (u32, u32), b: (u32, u32)) -> CellRange {
    let mut r = CellRange {
        first_row: a.0.min(b.0),
        first_col: a.1.min(b.1),
        last_row: a.0.max(b.0),
        last_col: a.1.max(b.1),
    };
    match kind {
        Kind::Cols => (r.first_row, r.last_row) = (0, MAX_ROWS - 1),
        Kind::Rows => (r.first_col, r.last_col) = (0, MAX_COLS - 1),
        Kind::Cells => {
            if let Some(m) = merges.at(a.0, a.1)
                && m.contains(b.0, b.1)
            {
                return CellRange { last_row: m.first_row, last_col: m.first_col, ..m };
            }
            loop {
                let mut g = r;
                for m in merges.in_rows(r.first_row, r.last_row) {
                    if m.first_col <= r.last_col && m.last_col >= r.first_col {
                        g.first_row = g.first_row.min(m.first_row);
                        g.first_col = g.first_col.min(m.first_col);
                        g.last_row = g.last_row.max(m.last_row);
                        g.last_col = g.last_col.max(m.last_col);
                    }
                }
                if g == r {
                    break;
                }
                r = g;
            }
        }
    }
    r
}

/// `B2`, `B2:C5`, `B:C` ou `3:5`.
pub fn area_text(kind: Kind, a: &CellRange) -> String {
    let mut s = String::new();
    match kind {
        Kind::Cells => {
            cell_name(a.first_row, a.first_col, &mut s);
            if (a.first_row, a.first_col) != (a.last_row, a.last_col) {
                s.push(':');
                cell_name(a.last_row, a.last_col, &mut s);
            }
        }
        Kind::Cols => {
            col_name(a.first_col, &mut s);
            s.push(':');
            col_name(a.last_col, &mut s);
        }
        Kind::Rows => {
            let _ = write!(s, "{}:{}", a.first_row + 1, a.last_row + 1);
        }
    }
    s
}

/// Nome da aba com o `!`, entre apóstrofos quando precisa.
pub fn sheet_prefix(name: &str) -> String {
    let plain = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        // `A1`, `AB`, `L1C1` e `VERDADEIRO` pareceriam outra coisa.
        && part(name).is_none()
        && !looks_r1c1(name)
        && !matches!(name.to_uppercase().as_str(), "VERDADEIRO" | "FALSO" | "TRUE" | "FALSE");
    if plain { format!("{name}!") } else { format!("'{}'!", name.replace('\'', "''")) }
}

/// `L1C1`, `R2C3`, `L`, `C`...
fn looks_r1c1(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    let rest = up.strip_prefix(['L', 'R']).unwrap_or(&up).trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = rest.strip_prefix('C').unwrap_or(rest);
    up.starts_with(['L', 'R', 'C']) && rest.chars().all(|c| c.is_ascii_digit())
}

/// Fecha os parênteses que faltam no fim da fórmula, se só há um jeito: a
/// fórmula termina num valor e, com mais de um aberto, cada função fica com
/// uma quantidade de argumentos que ela aceita.
pub fn close_parens(text: &str) -> Option<String> {
    let a = analyze(text, text.len());
    if !a.sound || a.open.is_empty() {
        return None;
    }
    let last = text.trim_end().chars().next_back()?;
    let def = |f: Option<duck_calc::funcs::Func>| f.map(duck_calc::funcs::def).filter(|d| !d.opaque);
    let empty = last == '(';
    if empty {
        // `=HOJE(` fecha; `=SOMA(` não (falta o argumento).
        if def(a.open.last()?.0).is_none_or(|d| d.min > 0) {
            return None;
        }
    } else if !(last.is_alphanumeric() || matches!(last, ')' | ']' | '"' | '%' | '}' | '#' | '_' | '!' | '?'))
    {
        return None;
    }
    if a.open.len() > 1 {
        let n = a.open.len();
        let fits = a.open.iter().enumerate().all(|(i, &(f, k))| {
            let args = if i + 1 == n && empty { 0 } else { k + 1 };
            def(f).is_none_or(|d| (d.min as usize..=d.max as usize).contains(&args))
        });
        if !fits {
            return None;
        }
    }
    Some(format!("{}{}", text.trim_end(), ")".repeat(a.open.len())))
}

// ---------------------------------------------------------------------------
// Ler
// ---------------------------------------------------------------------------

/// Uma ponta de referência: coluna e linha (faltando numa inteira), cada uma
/// com o `$`.
type Part = (Option<(bool, u32)>, Option<(bool, u32)>);

fn part(w: &str) -> Option<Part> {
    let (ca, rest) = w.strip_prefix('$').map_or((false, w), |r| (true, r));
    let n = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
    if n == 0 {
        // Só a linha: `3` ou `$3`.
        let row: u32 = digits(rest)?;
        return Some((None, Some((ca, row))));
    }
    if n > 3 {
        return None;
    }
    let col = rest[..n].bytes().fold(0u32, |c, b| c * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32);
    if col > MAX_COLS {
        return None;
    }
    let col = Some((ca, col - 1));
    let rest = &rest[n..];
    if rest.is_empty() {
        return Some((col, None));
    }
    let (ra, rest) = rest.strip_prefix('$').map_or((false, rest), |r| (true, r));
    Some((col, Some((ra, digits(rest)?))))
}

/// Número de linha (base zero).
fn digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|r| (1..=MAX_ROWS).contains(r)).map(|r| r - 1)
}

fn part_text(p: Part) -> String {
    let mut s = String::new();
    if let Some((abs, c)) = p.0 {
        s.push_str(if abs { "$" } else { "" });
        col_name(c, &mut s);
    }
    if let Some((abs, r)) = p.1 {
        let _ = write!(s, "{}{}", if abs { "$" } else { "" }, r + 1);
    }
    s
}

/// Uma referência no texto da fórmula.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    /// Com o nome da aba.
    pub range: Range<usize>,
    /// Sem ele.
    pub body: Range<usize>,
    pub sheet: Option<String>,
    pub area: CellRange,
    pub kind: Kind,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '\\' | '?' | '$')
}

/// Uma referência (`A1`, `$A1:B$2`, `A:B`, `1:3`) no começo de `s`: (tamanho, área, tipo).
fn ref_at(s: &str) -> Option<(usize, CellRange, Kind)> {
    let word = |s: &str| s.find(|c| !is_word(c)).unwrap_or(s.len());
    let n = word(s);
    let a = part(&s[..n])?;
    let second = s[n..].strip_prefix(':').and_then(|r| {
        let m = word(r);
        Some((n + 1 + m, part(&r[..m])?))
    });
    let span = |a: u32, b: u32| (a.min(b), a.max(b));
    let (len, area, kind) = match (a, second) {
        ((Some(c), Some(r)), Some((len, (Some(d), Some(t))))) => {
            let ((r0, r1), (c0, c1)) = (span(r.1, t.1), span(c.1, d.1));
            (len, CellRange { first_row: r0, first_col: c0, last_row: r1, last_col: c1 }, Kind::Cells)
        }
        ((Some(c), None), Some((len, (Some(d), None)))) => {
            let (c0, c1) = span(c.1, d.1);
            (len, CellRange { first_row: 0, first_col: c0, last_row: MAX_ROWS - 1, last_col: c1 }, Kind::Cols)
        }
        ((None, Some(r)), Some((len, (None, Some(t))))) => {
            let (r0, r1) = span(r.1, t.1);
            (len, CellRange { first_row: r0, first_col: 0, last_row: r1, last_col: MAX_COLS - 1 }, Kind::Rows)
        }
        ((Some(c), Some(r)), _) => {
            (n, CellRange { first_row: r.1, first_col: c.1, last_row: r.1, last_col: c.1 }, Kind::Cells)
        }
        _ => return None,
    };
    // `LOG10(`, `A1!`, `Tab1[`: outra coisa.
    let next = s[len..].trim_start_matches(' ').chars().next();
    (!matches!(next, Some('(')) && !s[len..].starts_with(['!', '['])).then_some((len, area, kind))
}

/// As referências de uma fórmula digitada, na ordem em que aparecem. Textos,
/// nomes, tabelas e outros arquivos (`[1]Aba!A1`) ficam de fora.
pub fn refs(text: &str) -> Vec<Ref> {
    let mut out = Vec::new();
    if !text.starts_with('=') {
        return out;
    }
    let mut i = 1;
    // Começo e nome da aba, quando a referência vem depois de `Aba!`.
    let mut prefix: Option<(usize, Option<String>)> = None;
    while let Some(c) = text[i..].chars().next() {
        match c {
            '"' | '\'' => {
                let st = i;
                i += 1;
                let mut name = String::new();
                while let Some(d) = text[i..].chars().next() {
                    i += d.len_utf8();
                    if d == c {
                        if !text[i..].starts_with(c) {
                            break;
                        }
                        i += 1;
                    }
                    name.push(d);
                }
                if c == '\'' && text[i..].starts_with('!') {
                    i += 1;
                    let local = !name.contains('[');
                    prefix = Some((st, local.then_some(name)));
                }
            }
            '[' => {
                let mut depth = 0;
                while let Some(d) = text[i..].chars().next() {
                    i += d.len_utf8();
                    match d {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        '\'' => i += text[i..].chars().next().map_or(0, char::len_utf8),
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                }
                // `[1]Aba!A1`: outro arquivo.
                let n = text[i..].find(|c| !is_word(c)).unwrap_or(0);
                if n > 0 && text[i + n..].starts_with('!') {
                    i += n + 1;
                    prefix = Some((i, None));
                }
            }
            _ if is_word(c) => {
                let n = text[i..].find(|c| !is_word(c)).unwrap_or(text.len() - i);
                if text[i + n..].starts_with('!') && prefix.is_none() {
                    prefix = Some((i, Some(text[i..i + n].to_string())));
                    i += n + 1;
                    continue;
                }
                let sheet = prefix.take();
                match ref_at(&text[i..]) {
                    Some((len, area, kind)) => {
                        if !matches!(sheet, Some((_, None))) {
                            let start = sheet.as_ref().map_or(i, |s| s.0);
                            let sheet = sheet.and_then(|s| s.1);
                            out.push(Ref { range: start..i + len, body: i..i + len, sheet, area, kind });
                        }
                        i += len;
                    }
                    None => i += n,
                }
            }
            _ => {
                prefix = None;
                i += c.len_utf8();
            }
        }
    }
    out
}

/// F4: `A1` → `$A$1` → `A$1` → `$A1` → `A1`; em linhas e colunas inteiras
/// liga e desliga o `$`. Num intervalo, as duas pontas ficam iguais.
pub fn cycle(body: &str) -> String {
    let ends: Vec<Part> = body.split(':').filter_map(part).collect();
    let Some(&first) = ends.first() else { return body.to_string() };
    let next = match (first.0.map(|c| c.0), first.1.map(|r| r.0)) {
        (Some(c), Some(r)) => match (c, r) {
            (false, false) => (true, true),
            (true, true) => (false, true),
            (false, true) => (true, false),
            (true, false) => (false, false),
        },
        (c, r) => (c == Some(false), r == Some(false)),
    };
    let set = |p: &Part| (p.0.map(|c| (next.0, c.1)), p.1.map(|r| (next.1, r.1)));
    ends.iter().map(|p| part_text(set(p))).collect::<Vec<_>>().join(":")
}

/// A cor (índice) de cada referência: a mesma área tem a mesma cor. `home` =
/// nome da aba da célula editada.
pub fn colors(refs: &[Ref], home: &str) -> Vec<usize> {
    let key = |r: &Ref| (r.sheet.as_deref().unwrap_or(home).to_lowercase(), r.area);
    let mut seen = Vec::new();
    refs.iter()
        .map(|r| {
            let k = key(r);
            seen.iter().position(|s| *s == k).unwrap_or_else(|| {
                seen.push(k);
                seen.len() - 1
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// No editor
// ---------------------------------------------------------------------------

/// Onde a próxima referência entra: no lugar da recém-apontada, da seleção do
/// texto ou no cursor, se ali cabe uma. `None` = não cabe.
pub fn target(ed: &Editor) -> Option<Range<usize>> {
    if let Some(p) = &ed.point {
        return Some(p.range.clone());
    }
    let text = &ed.input.text;
    let (a, b) = ed.input.selection().unwrap_or((ed.input.cursor(), ed.input.cursor()));
    if a == b {
        return analyze(text, a).accepts.then_some(a..a);
    }
    let without = format!("{}{}", &text[..a], &text[b..]);
    analyze(&without, a).accepts.then_some(a..b)
}

/// Escreve a referência apontada no editor. `prefix` = nome da aba, se não é
/// a da célula editada.
pub fn write(ed: &mut Editor, mut p: Point, merges: &Merges, prefix: &str) {
    let Some(at) = target(ed) else { return };
    let text = format!("{prefix}{}", area_text(p.kind, &area(merges, p.kind, p.anchor, p.cur)));
    p.range = at.start..at.start + text.len();
    ed.input.replace(at, &text);
    ed.point = Some(p);
}

/// F4 na referência sob o cursor (ou encostada nele). `false` = não há.
pub fn cycle_in(ed: &mut Editor) -> bool {
    let cur = ed.input.cursor();
    let list = refs(&ed.input.text);
    // Entre duas encostadas, a da esquerda.
    let Some(r) = list.iter().find(|r| r.range.start <= cur && cur <= r.range.end) else { return false };
    let new = cycle(&ed.input.text[r.body.clone()]);
    let end = r.body.start + new.len();
    ed.input.replace(r.body.clone(), &new);
    match &mut ed.point {
        Some(p) if p.range.start == r.range.start => p.range.end = end,
        p => *p = None,
    }
    true
}

/// Cores do texto do editor: (trecho, cor).
pub fn spans(list: &[Ref], color: &[usize], th: &Theme) -> Vec<(Range<usize>, Rgba)> {
    list.iter().zip(color).map(|(r, &c)| (r.range.clone(), th.refs[c % th.refs.len()])).collect()
}

/// O retângulo da área em cada painel, com o recorte dele.
fn on_panes(g: &Geom, v: &View, a: &CellRange, px: f32) -> Vec<(Rect, Rect)> {
    let (nr, nc) = (v.rows.len(), v.cols.len());
    if a.first_row >= nr || a.first_col >= nc {
        return Vec::new();
    }
    // Uma célula mesclada: o contorno da mesclagem.
    let single = (a.first_row, a.first_col) == (a.last_row, a.last_col);
    let a = v.merges.at(a.first_row, a.first_col).filter(|_| single).unwrap_or(*a);
    let pane = |q: &Quad| {
        let x = |c: u32| crate::draw::pos(&v.cols, q.ox, c.min(nc), q.rect.x0, g.g);
        let y = |r: u32| crate::draw::pos(&v.rows, q.oy, r.min(nr), q.rect.y0, g.g);
        let r = Rect::new(
            x(a.first_col) - px,
            y(a.first_row) - px,
            x(a.last_col.saturating_add(1)),
            y(a.last_row.saturating_add(1)),
        );
        (!r.intersect(&q.rect).is_empty()).then_some((r, q.rect))
    };
    g.quads.iter().filter(|q| !q.rect.is_empty()).filter_map(pane).collect()
}

fn sides(r: &Rect, w: f32) -> [Rect; 4] {
    [
        Rect::new(r.x0, r.y0, r.x1, r.y0 + w),
        Rect::new(r.x0, r.y1 - w, r.x1, r.y1),
        Rect::new(r.x0, r.y0, r.x0 + w, r.y1),
        Rect::new(r.x1 - w, r.y0, r.x1, r.y1),
    ]
}

/// Contornos na grade das referências que ficam na aba à vista.
/// `shown` = (área, cor, é a apontada).
pub fn draw_outlines(p: &mut Painter, g: &Geom, v: &View, shown: &[(CellRange, Rgba, bool)], px: f32) {
    for (a, color, dashed) in shown {
        for (r, clip) in on_panes(g, v, a, px) {
            p.fill(r, &clip, [color[0], color[1], color[2], 28]);
            let w = 2.0 * px;
            if *dashed {
                for side in sides(&r, w) {
                    p.dashed(side, &clip, *color, 8.0 * px, 5.0 * px);
                }
            } else {
                p.stroke(r, &clip, w, *color);
            }
        }
    }
}

/// Borda tracejada do que está copiado ou recortado. O fundo do traço tampa o
/// contorno da seleção, que costuma estar no mesmo lugar.
pub fn draw_marked(p: &mut Painter, g: &Geom, v: &View, areas: &[CellRange], px: f32, th: &Theme) {
    let w = 2.0 * px;
    for a in areas {
        for (r, clip) in on_panes(g, v, a, px) {
            // Como o contorno da seleção: um pixel para cada lado da linha.
            let r = Rect::new(r.x0, r.y0, r.x1 + px, r.y1 + px);
            for side in sides(&r, w) {
                p.fill(side, &clip, th.paper);
                p.dashed(side, &clip, th.selection, 8.0 * px, 5.0 * px);
            }
        }
    }
}

impl SheetsApp {
    /// Um clique na grade agora escreve uma referência?
    pub fn can_point(&self) -> bool {
        self.editor.as_ref().is_some_and(|ed| target(ed).is_some())
    }

    /// Referências da fórmula em edição e a cor de cada uma.
    pub fn editor_refs(&self) -> (Vec<Ref>, Vec<usize>) {
        let (Some(ed), Some(doc)) = (&self.editor, &self.doc) else { return Default::default() };
        let list = refs(&ed.input.text);
        let home = doc.sheets.get(ed.sheet).map_or("", |s| s.info.name.as_str());
        let color = colors(&list, home);
        (list, color)
    }

    /// As que aparecem na aba à vista: (área, cor, é a apontada).
    pub fn outlines(&self, list: &[Ref], color: &[usize], th: &Theme) -> Vec<(CellRange, Rgba, bool)> {
        let (Some(ed), Some(doc)) = (&self.editor, &self.doc) else { return Vec::new() };
        let here = |r: &Ref| match &r.sheet {
            None => ed.sheet == doc.active,
            Some(n) => doc.slot().info.name.to_lowercase() == n.to_lowercase(),
        };
        let pointed = ed.point.as_ref().map(|p| p.range.start);
        let mut out: Vec<(CellRange, Rgba, bool)> = Vec::new();
        for (r, &c) in list.iter().zip(color).filter(|(r, _)| here(r)) {
            let dashed = pointed == Some(r.range.start);
            // A mesma área duas vezes: um contorno só.
            match out.iter_mut().find(|o| o.0 == r.area) {
                Some(o) => o.2 |= dashed,
                None => out.push((r.area, th.refs[c % th.refs.len()], dashed)),
            }
        }
        out
    }

    /// Aponta de `anchor` a `cur` na aba à vista.
    fn point(&mut self, kind: Kind, anchor: (u32, u32), cur: (u32, u32)) {
        let Some(doc) = &self.doc else { return };
        let Some((_, v)) = doc.current() else { return };
        let Some(ed) = &mut self.editor else { return };
        let prefix = if ed.sheet == doc.active { String::new() } else { sheet_prefix(&doc.slot().info.name) };
        let p = Point { range: 0..0, sheet: doc.active, kind, anchor, cur };
        write(ed, p, &v.merges, &prefix);
        self.refresh_complete(false);
    }

    /// Clique com a fórmula aberta: aponta células, colunas ou linhas, troca
    /// de aba ou usa a barra de rolagem. `false` = não é com ele (o clique
    /// confirma a edição, como sempre).
    pub fn editor_click(&mut self, g: &Geom, x: f32, y: f32, mods: Modifiers) -> bool {
        if !self.can_point() {
            return false;
        }
        if g.tabs.contains(x, y) {
            let i = self.tabbar.hit(x - g.tabs.x0);
            if let Some(i) = i.and_then(|i| self.visible_sheet_indices().get(i).copied()) {
                self.switch_sheet(i);
            }
            return true;
        }
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return false };
        let (kind, at) = if g.colhdr.contains(x, y) {
            let q = if x < g.grid.x0 + g.fw { &g.quads[0] } else { &g.quads[1] };
            (Kind::Cols, (0, g.cell_in(v, q, x, q.rect.y0).1))
        } else if g.rowhdr.contains(x, y) {
            let q = if y < g.grid.y0 + g.fh { &g.quads[0] } else { &g.quads[2] };
            (Kind::Rows, (g.cell_in(v, q, q.rect.x0, y).0, 0))
        } else if let Some(at) = g.hit(v, x, y) {
            (Kind::Cells, at)
        } else {
            return false;
        };
        let active = self.doc.as_ref().map_or(0, |d| d.active);
        let old = self.editor.as_ref().and_then(|e| e.point.as_ref());
        let anchor = match old {
            Some(p) if mods.shift && p.sheet == active && p.kind == kind => p.anchor,
            _ => at,
        };
        self.point(kind, anchor, at);
        self.drag = Some(Drag::Point);
        true
    }

    /// Arrastar depois do clique: estende até a célula.
    pub fn point_drag(&mut self, at: (u32, u32)) {
        let Some(p) = self.editor.as_ref().and_then(|e| e.point.clone()) else { return };
        let cur = match p.kind {
            Kind::Cells => at,
            Kind::Cols => (0, at.1),
            Kind::Rows => (at.0, 0),
        };
        if cur != p.cur {
            self.point(p.kind, p.anchor, cur);
        }
    }

    /// Seta com a fórmula aberta: anda com a referência apontada (Shift
    /// estende, Ctrl vai à borda dos dados). `false` = a seta não aponta agora.
    pub fn point_key(&mut self, dr: i32, dc: i32, extend: bool, jump: bool) -> bool {
        let Some(ed) = &self.editor else { return false };
        if !(ed.enter_mode || ed.point.is_some()) || !self.can_point() {
            return false;
        }
        let Some(doc) = &self.doc else { return false };
        let Some((_, v)) = doc.current() else { return false };
        // Sem nada apontado nesta aba, parte da célula editada (ou da ativa).
        let old = ed.point.clone().filter(|p| p.sheet == doc.active);
        let home = if ed.away { v.cur } else { (ed.row, ed.col) };
        let (kind, anchor, cur) = old.map_or((Kind::Cells, home, home), |p| (p.kind, p.anchor, p.cur));
        let (dr, dc) = match kind {
            Kind::Cells => (dr, dc),
            Kind::Cols => (0, dc),
            Kind::Rows => (dr, 0),
        };
        self.grow_grid(cur.0 + dr.max(0) as u32, cur.1 + dc.max(0) as u32);
        let Some(doc) = &self.doc else { return false };
        let Some((sheet, v)) = doc.current() else { return false };
        let to = crate::app::step(sheet, v, cur, dr, dc, jump);
        self.point(kind, if extend { anchor } else { to }, to);
        if kind == Kind::Cells {
            self.reveal(to.0, to.1);
        }
        true
    }

    /// F2: alterna entre apontar com as setas e andar no texto. Ao voltar a
    /// apontar, a referência antes do cursor é a apontada.
    pub fn toggle_pointing(&mut self) {
        let active = self.doc.as_ref().map_or(0, |d| d.active);
        let name = self.doc.as_ref().map(|d| d.slot().info.name.to_lowercase()).unwrap_or_default();
        let Some(ed) = &mut self.editor else { return };
        if ed.enter_mode || ed.point.is_some() {
            ed.enter_mode = false;
            ed.point = None;
            return;
        }
        ed.enter_mode = true;
        if ed.input.selection().is_some() {
            return;
        }
        let at = ed.input.cursor();
        let here = |r: &Ref| match &r.sheet {
            None => ed.sheet == active,
            Some(n) => n.to_lowercase() == name,
        };
        if let Some(r) = refs(&ed.input.text).into_iter().find(|r| r.range.end == at && here(r)) {
            ed.point = Some(Point {
                range: r.range,
                sheet: active,
                kind: r.kind,
                anchor: (r.area.first_row, r.area.first_col),
                cur: (r.area.last_row, r.area.last_col),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_ui::TextInput;

    fn rng(r0: u32, c0: u32, r1: u32, c1: u32) -> CellRange {
        CellRange { first_row: r0, first_col: c0, last_row: r1, last_col: c1 }
    }

    fn editor(text: &str) -> Editor {
        let mut input = TextInput::new();
        input.set_text(text);
        Editor {
            row: 0,
            col: 0,
            input,
            enter_mode: true,
            in_bar: false,
            wrap: false,
            sheet: 0,
            away: false,
            point: None,
            complete: Default::default(),
        }
    }

    fn cell(r: u32, c: u32) -> Point {
        Point { range: 0..0, sheet: 0, kind: Kind::Cells, anchor: (r, c), cur: (r, c) }
    }

    fn range(a: (u32, u32), b: (u32, u32)) -> Point {
        Point { anchor: a, cur: b, ..cell(0, 0) }
    }

    #[test]
    fn where_a_reference_fits() {
        for t in [
            "=",
            "= ",
            "=A1+",
            "=A1 * ",
            "=SOMA(",
            "=SOMA(A1;",
            "=-",
            "=A1&",
            "=A1>=",
            "=A1<>",
            "=(",
            "=A1:",
            "=@",
        ] {
            assert_eq!(target(&editor(t)), Some(t.len()..t.len()), "{t}");
        }
        // Depois de um valor completo, no meio de um nome, em texto ou fora de fórmula.
        for t in [
            "",
            "a",
            "12+",
            "=A1",
            "=1",
            "=1,5",
            "=so",
            "=\"a+",
            "=\"a\"",
            "=SOMA(A1)",
            "=T[",
            "=T[Col]",
            "={1;",
            "='Aba",
            "=10%",
        ] {
            assert_eq!(target(&editor(t)), None, "{t}");
        }
        // No meio do texto, só onde não encosta em outro valor.
        let mut ed = editor("=SOMA(;B2)");
        ed.input.set_cursor(6);
        assert_eq!(target(&ed), Some(6..6));
        let mut ed = editor("=+B2");
        ed.input.set_cursor(2);
        assert_eq!(target(&ed), None);
    }

    #[test]
    fn inserts_and_replaces() {
        let m = Merges::default();
        let mut ed = editor("=SOMA(");
        write(&mut ed, cell(1, 1), &m, "");
        assert_eq!((ed.input.text.as_str(), ed.input.cursor()), ("=SOMA(B2", 8));
        // Outro clique troca; arrastar estende.
        write(&mut ed, cell(4, 2), &m, "");
        assert_eq!(ed.input.text, "=SOMA(C5");
        write(&mut ed, range((1, 1), (9, 1)), &m, "");
        assert_eq!(ed.input.text, "=SOMA(B2:B10");
        // De baixo para cima dá no mesmo.
        write(&mut ed, range((9, 1), (1, 1)), &m, "");
        assert_eq!(ed.input.text, "=SOMA(B2:B10");
        // Digitar fixa; depois de um `;` começa outra.
        ed.point = None;
        assert_eq!(target(&ed), None);
        ed.input.insert(";");
        write(&mut ed, cell(0, 3), &m, "");
        assert_eq!(ed.input.text, "=SOMA(B2:B10;D1");
        // No meio do texto, o resto fica.
        let mut ed = editor("=SOMA(;B2)");
        ed.input.set_cursor(6);
        write(&mut ed, cell(0, 0), &m, "");
        write(&mut ed, cell(0, 27), &m, "");
        assert_eq!((ed.input.text.as_str(), ed.input.cursor()), ("=SOMA(AB1;B2)", 9));
        // Onde não cabe, nada muda.
        let mut ed = editor("=A1");
        write(&mut ed, cell(0, 0), &m, "");
        assert_eq!((ed.input.text.as_str(), &ed.point), ("=A1", &None));
        // Com uma parte do texto selecionada, entra no lugar dela.
        let mut ed = editor("=A1");
        ed.input.select(1, 3);
        write(&mut ed, cell(8, 1), &m, "");
        assert_eq!(ed.input.text, "=B9");
    }

    #[test]
    fn whole_columns_and_rows() {
        let m = Merges::default();
        let mut ed = editor("=SOMA(");
        write(&mut ed, Point { kind: Kind::Cols, ..cell(0, 1) }, &m, "");
        assert_eq!(ed.input.text, "=SOMA(B:B");
        write(&mut ed, Point { kind: Kind::Cols, ..range((0, 3), (0, 1)) }, &m, "");
        assert_eq!(ed.input.text, "=SOMA(B:D");
        write(&mut ed, Point { kind: Kind::Rows, ..cell(2, 0) }, &m, "");
        assert_eq!(ed.input.text, "=SOMA(3:3");
    }

    #[test]
    fn merged_cells() {
        let m = Merges::new(vec![rng(1, 1, 3, 2)]);
        // Uma mesclada sozinha: o canto.
        assert_eq!(area_text(Kind::Cells, &area(&m, Kind::Cells, (2, 2), (2, 2))), "B2");
        assert_eq!(area_text(Kind::Cells, &area(&m, Kind::Cells, (1, 1), (3, 2))), "B2");
        // Um arraste que a toca leva ela inteira.
        assert_eq!(area_text(Kind::Cells, &area(&m, Kind::Cells, (0, 0), (1, 1))), "A1:C4");
        assert_eq!(area_text(Kind::Cells, &area(&m, Kind::Cells, (5, 5), (6, 6))), "F6:G7");
    }

    #[test]
    fn other_sheets() {
        assert_eq!(sheet_prefix("Dados"), "Dados!");
        assert_eq!(sheet_prefix("Março_2026"), "Março_2026!");
        assert_eq!(sheet_prefix("Vendas 2026"), "'Vendas 2026'!");
        assert_eq!(sheet_prefix("D'água"), "'D''água'!");
        // Pareceriam células, colunas, linhas ou constantes.
        for n in ["A1", "ABC", "2026", "L1C1", "C", "Verdadeiro", "Jan-26"] {
            assert_eq!(sheet_prefix(n), format!("'{n}'!"));
        }
        let m = Merges::default();
        let mut ed = editor("=A1+");
        write(&mut ed, Point { sheet: 1, ..cell(2, 0) }, &m, &sheet_prefix("Vendas 2026"));
        assert_eq!(ed.input.text, "=A1+'Vendas 2026'!A3");
        // Voltar para a aba da célula troca por uma sem o nome.
        write(&mut ed, cell(0, 1), &m, "");
        assert_eq!(ed.input.text, "=A1+B1");
    }

    #[test]
    fn finds_references() {
        let list =
            |t: &str| -> Vec<String> { refs(t).iter().map(|r| t[r.range.clone()].to_string()).collect() };
        assert_eq!(list("=SOMA(A1;$B$2:C3)*d4"), ["A1", "$B$2:C3", "d4"]);
        assert_eq!(list("=SOMA(B:C;3:5;$A:$A)"), ["B:C", "3:5", "$A:$A"]);
        assert_eq!(list("=Dados!A1+'Vendas 2026'!B2:B3"), ["Dados!A1", "'Vendas 2026'!B2:B3"]);
        // Funções com cara de célula, textos, nomes, tabelas, erros e números.
        assert!(list("=LOG10(1,5)&\"A1\"&Taxa1x&Tab1[Col A1]&#REF!+1E5").is_empty());
        assert!(list("=[1]Aba!A1+'[x.xlsx]Aba'!B2").is_empty());
        assert!(list("=ABCD1+XFE1+A0+A1048577").is_empty());
        assert!(list("A1").is_empty());
        let r = &refs("='D''água'!C5:B2")[0];
        assert_eq!((r.sheet.as_deref(), r.area), (Some("D'água"), rng(1, 1, 4, 2)));
        assert_eq!(refs("=B:A")[0].area, rng(0, 0, MAX_ROWS - 1, 1));
        // Um intervalo pela metade ainda é a primeira célula.
        assert_eq!(list("=SOMA(A1:"), ["A1"]);
    }

    #[test]
    fn f4_cycles() {
        let mut t = "A1".to_string();
        for want in ["$A$1", "A$1", "$A1", "A1"] {
            t = cycle(&t);
            assert_eq!(t, want);
        }
        assert_eq!(cycle("a1:B3"), "$A$1:$B$3");
        assert_eq!(cycle("$A$1:B3"), "A$1:B$3");
        assert_eq!((cycle("B:C"), cycle("$B:$C")), ("$B:$C".into(), "B:C".into()));
        assert_eq!((cycle("3:3"), cycle("$3:$3")), ("$3:$3".into(), "3:3".into()));

        // No editor: a referência sob o cursor, sem mexer no nome da aba.
        let mut ed = editor("=SOMA('Vendas 2026'!A1;B2)");
        ed.input.set_cursor(22);
        assert!(cycle_in(&mut ed));
        assert_eq!((ed.input.text.as_str(), ed.input.cursor()), ("=SOMA('Vendas 2026'!$A$1;B2)", 24));
        let mut ed = editor("=1+2");
        assert!(!cycle_in(&mut ed));
        // Na recém-apontada: o próximo clique ainda troca ela.
        let m = Merges::default();
        let mut ed = editor("=");
        write(&mut ed, cell(0, 0), &m, "");
        assert!(cycle_in(&mut ed));
        assert_eq!(ed.input.text, "=$A$1");
        write(&mut ed, cell(4, 4), &m, "");
        assert_eq!(ed.input.text, "=E5");
    }

    #[test]
    fn same_area_same_color() {
        let list = refs("=A1+B2+Dados!A1+$A$1+resumo!B2+A1:A2");
        // Dados!A1 é outra célula; Resumo é a aba da própria fórmula.
        assert_eq!(colors(&list, "Resumo"), [0, 1, 2, 0, 1, 3]);
        let th = &crate::theme::DARK;
        let s = spans(&list, &colors(&list, "Resumo"), th);
        assert_eq!(s.len(), 6);
        assert_eq!(s[0].1, s[3].1);
        assert_ne!(s[0].1, s[1].1);
        assert_eq!(s[2].0, 7..15);
    }

    #[test]
    fn closes_parentheses() {
        let c = |t: &str| close_parens(t);
        assert_eq!(c("=SOMA(B2:B10").as_deref(), Some("=SOMA(B2:B10)"));
        assert_eq!(c("=SOMA(B2:B10 ").as_deref(), Some("=SOMA(B2:B10)"));
        assert_eq!(c("=(A1+B1").as_deref(), Some("=(A1+B1)"));
        assert_eq!(c("=HOJE(").as_deref(), Some("=HOJE()"));
        assert_eq!(c("=ESQUERDA(\"a(b\"").as_deref(), Some("=ESQUERDA(\"a(b\")"));
        assert_eq!(c("=SE(A1>0;SOMA(B2:B10").as_deref(), Some("=SE(A1>0;SOMA(B2:B10))"));
        // Nada a fechar, incompleta ou com mais de um jeito.
        for t in [
            "=SOMA(A1)",
            "=SOMA(",
            "=SOMA(A1;",
            "=SOMA(A1+",
            "=ESQUERDA(\"abc",
            "=A1)",
            "=SOMA(T[Col",
            "texto(",
        ] {
            assert_eq!(c(t), None, "{t}");
        }
        assert_eq!(c("=PROCV(SOMA(A1;B1;2;0"), None);
        assert_eq!(c("=SEERRO(ABS(A1;0"), None);
    }
}
