//! Fórmulas do BIFF12: tokens `Ptg` em notação polonesa reversa (MS-XLSB
//! 2.2.2) → texto A1 em inglês, como o `.xlsx` grava.
//!
//! Em duas etapas, porque milhões de células repetem a mesma fórmula:
//!
//! 1. [`normalize`]: uma passada pelos tokens que troca cada referência
//!    relativa pelo deslocamento a partir da célula e tira o que não aparece
//!    no texto (atributos de salto, classe dos operandos). Células com a mesma
//!    fórmula ficam com os mesmos bytes, que servem de chave.
//! 2. [`decode`]: só para chaves novas, monta o **modelo relativo** do
//!    `duck-calc` (o mesmo que a leitura do `.xlsx` produz a partir do texto).

use duck_calc::RefPart;

use super::ftab;
use super::rec::{self, Cur, LAST_COL, LAST_ROW, push_utf16};

/// O que os tokens citam por número: abas, nomes, tabelas.
#[derive(Default)]
pub struct Names {
    /// `BrtName`, na ordem do arquivo (`PtgName` conta a partir de 1).
    pub names: Vec<String>,
    /// Prefixos de `BrtExternSheet`, pelo `ixti`.
    pub xti: Vec<Xti>,
    /// Tabelas pelo `idList`: nome e colunas.
    pub tables: std::collections::HashMap<u32, (String, Vec<String>)>,
}

/// Para onde aponta um `ixti`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Xti {
    /// `Aba!`, `'Minha aba'!`, `[1]Aba!`, `Jan:Mar!`. `None` = a aba não existe mais.
    pub prefix: Option<String>,
    /// Prefixo dos nomes sem aba (`[1]!`; vazio neste arquivo).
    pub book: String,
    /// Nomes do arquivo externo. `None` = nomes deste arquivo.
    pub names: Option<std::sync::Arc<Vec<String>>>,
}

/// Referência de uma célula para outra, na forma normalizada.
const REL_ROW: u8 = 1;
const REL_COL: u8 = 2;
/// Só áreas: colunas ou linhas inteiras.
const WHOLE_COLS: u8 = 4;
const WHOLE_ROWS: u8 = 8;

fn put32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Linha como está no token → (valor normalizado, é relativa).
/// `offset` = o token já traz o deslocamento (fórmula compartilhada, formatação condicional).
fn norm_row(v: u32, rel: bool, offset: bool, origin: u32) -> i32 {
    norm(v & LAST_ROW, rel, offset, origin, LAST_ROW + 1)
}

fn norm_col(v: u16, rel: bool, offset: bool, origin: u32) -> i32 {
    norm(v as u32 & LAST_COL, rel, offset, origin, LAST_COL + 1)
}

fn norm(v: u32, rel: bool, offset: bool, origin: u32, size: u32) -> i32 {
    match (rel, offset) {
        (false, _) => v as i32,
        // O deslocamento dá a volta na aba: o alvo fica sempre dentro dela.
        (true, true) => ((origin + v) % size) as i32 - origin as i32,
        (true, false) => v as i32 - origin as i32,
    }
}

const F_COL: u16 = 0x4000;
const F_ROW: u16 = 0x8000;

/// Reescreve `rgce` em `out` com as referências relativas à célula
/// (`row`, `col`). `None` = token desconhecido ou fórmula truncada.
pub fn normalize(rgce: &[u8], rgcb: &[u8], (row, col): (u32, u32), out: &mut Vec<u8>) -> Option<()> {
    let mut c = Cur::new(rgce);
    let mut extra = Cur::new(rgcb);
    while let Some(ptg) = c.u8() {
        // Operandos têm a classe (referência, valor, matriz) nos bits 5 e 6.
        let base = if ptg < 0x20 { ptg } else { 0x20 | (ptg & 0x1F) };
        match base {
            // Operadores, parênteses e argumento vazio.
            0x03..=0x16 => out.push(base),
            0x17 => {
                out.push(base);
                let n = c.u16()?;
                out.extend_from_slice(&n.to_le_bytes());
                out.extend_from_slice(c.take(n as usize * 2)?);
            }
            0x18 => {
                // Só `PtgList` (referência estruturada) aparece em células.
                if c.u8()? != 0x19 {
                    return None;
                }
                out.push(base);
                out.extend_from_slice(c.take(12)?);
            }
            0x19 => match c.u8()? {
                // Saltos e marcas que não aparecem no texto.
                0x01 | 0x02 | 0x08 | 0x20 | 0x80 => c.skip(2)?,
                0x04 => {
                    let n = c.u16()? as usize;
                    c.skip((n + 1) * 2)?;
                }
                0x10 => {
                    c.skip(2)?;
                    out.extend_from_slice(&[0x19, 0x10]);
                }
                0x40 | 0x41 => {
                    let (kind, n) = (c.u8()?, c.u8()?);
                    out.extend_from_slice(&[0x19, 0x40, kind, n]);
                }
                _ => return None,
            },
            0x1C | 0x1D => out.extend_from_slice(&[base, c.u8()?]),
            0x1E => {
                out.push(base);
                out.extend_from_slice(c.take(2)?);
            }
            0x1F => {
                out.push(base);
                out.extend_from_slice(c.take(8)?);
            }
            0x20 => {
                // Constante matricial: os valores estão nos dados extras.
                c.skip(14)?;
                out.push(base);
                copy_array(&mut extra, out)?;
            }
            0x21 => {
                out.push(base);
                out.extend_from_slice(c.take(2)?);
            }
            0x22 => {
                out.push(base);
                out.extend_from_slice(c.take(3)?);
            }
            0x23 => {
                out.push(base);
                out.extend_from_slice(c.take(4)?);
            }
            0x24 | 0x2C => {
                out.push(0x24);
                cell(&mut c, base == 0x2C, row, col, out)?;
            }
            0x25 | 0x2D => {
                out.push(0x25);
                area(&mut c, base == 0x2D, row, col, out)?;
            }
            // Subexpressões de referência: o conteúdo vem em seguida, como tokens.
            0x26 => {
                c.skip(6)?;
                let n = extra.u32()? as usize;
                extra.skip(n.checked_mul(16)?)?;
            }
            0x27 | 0x28 => c.skip(6)?,
            0x29 | 0x2E | 0x2F => c.skip(2)?,
            0x2A => {
                c.skip(6)?;
                out.push(0x2A);
            }
            0x2B => {
                c.skip(12)?;
                out.push(0x2A);
            }
            0x39 => {
                out.push(base);
                out.extend_from_slice(c.take(6)?);
            }
            0x3A => {
                out.push(base);
                out.extend_from_slice(c.take(2)?);
                cell(&mut c, false, row, col, out)?;
            }
            0x3B => {
                out.push(base);
                out.extend_from_slice(c.take(2)?);
                area(&mut c, false, row, col, out)?;
            }
            0x3C | 0x3D => {
                out.push(0x3C);
                out.extend_from_slice(c.take(2)?);
                c.skip(if base == 0x3C { 6 } else { 12 })?;
            }
            _ => return None,
        }
    }
    Some(())
}

fn cell(c: &mut Cur, offset: bool, row: u32, col: u32, out: &mut Vec<u8>) -> Option<()> {
    let (r, k) = (c.u32()?, c.u16()?);
    let (rr, rc) = (k & F_ROW != 0, k & F_COL != 0);
    out.push((rr as u8 * REL_ROW) | (rc as u8 * REL_COL));
    put32(out, norm_row(r, rr, offset, row));
    put32(out, norm_col(k, rc, offset, col));
    Some(())
}

fn area(c: &mut Cur, offset: bool, row: u32, col: u32, out: &mut Vec<u8>) -> Option<()> {
    let (r0, r1, k0, k1) = (c.u32()?, c.u32()?, c.u16()?, c.u16()?);
    let mut flags = [0u8; 2];
    for (f, k) in flags.iter_mut().zip([k0, k1]) {
        *f = ((k & F_ROW != 0) as u8 * REL_ROW) | ((k & F_COL != 0) as u8 * REL_COL);
    }
    let whole = if k0 as u32 & LAST_COL == 0 && k1 as u32 & LAST_COL == LAST_COL && (k0 | k1) & F_COL == 0 {
        WHOLE_ROWS
    } else if r0 & LAST_ROW == 0 && r1 & LAST_ROW == LAST_ROW {
        WHOLE_COLS
    } else {
        0
    };
    out.push(flags[0] | whole);
    out.push(flags[1]);
    for (r, k) in [(r0, k0), (r1, k1)] {
        // Nas colunas inteiras a linha não conta (senão cada linha teria uma chave).
        let r = if whole == WHOLE_COLS { 0 } else { norm_row(r, k & F_ROW != 0, offset, row) };
        let k = if whole == WHOLE_ROWS { 0 } else { norm_col(k, k & F_COL != 0, offset, col) };
        put32(out, r);
        put32(out, k);
    }
    Some(())
}

/// `PtgExtraArray`: linhas, colunas e os valores.
fn copy_array(extra: &mut Cur, out: &mut Vec<u8>) -> Option<()> {
    let (rows, cols) = (extra.u32()?, extra.u32()?);
    out.extend_from_slice(&rows.to_le_bytes());
    out.extend_from_slice(&cols.to_le_bytes());
    for _ in 0..rows.checked_mul(cols)? {
        let kind = extra.u8()?;
        out.push(kind);
        let n = match kind {
            0 => 8,
            1 => {
                let n = extra.u16()?;
                out.extend_from_slice(&n.to_le_bytes());
                n as usize * 2
            }
            2 => 1,
            4 => 4,
            _ => return None,
        };
        out.extend_from_slice(extra.take(n)?);
    }
    Some(())
}

/// Número como o Excel escreve numa fórmula (`0.5`, `1E+20`).
fn push_number(x: f64, out: &mut String) {
    use std::fmt::Write;
    let a = x.abs();
    if x == 0.0 || (1e-9..1e15).contains(&a) {
        let _ = write!(out, "{x}");
        return;
    }
    let s = format!("{x:E}");
    match s.split_once('E') {
        Some((m, e)) if !e.starts_with('-') => {
            let _ = write!(out, "{m}E+{e}");
        }
        _ => out.push_str(&s),
    }
}

fn push_quoted(s: &str, out: &mut String) {
    out.push('"');
    out.push_str(&s.replace('"', "\"\""));
    out.push('"');
}

/// Operando na pilha: texto de `start` até o próximo.
struct Stack {
    out: String,
    starts: Vec<usize>,
    /// Espaços pendentes: antes do próximo token, do `(` e do `)`.
    before: String,
    open: String,
    close: String,
}

impl Stack {
    /// Começa um operando (com os espaços pendentes).
    fn begin(&mut self) {
        self.starts.push(self.out.len());
        self.out.push_str(&std::mem::take(&mut self.before));
    }

    /// Insere um operador entre os dois últimos operandos.
    fn binary(&mut self, op: &str) -> Option<()> {
        let at = self.starts.pop()?;
        self.starts.last()?;
        let op = format!("{}{op}", std::mem::take(&mut self.before));
        self.out.insert_str(at, &op);
        Some(())
    }

    fn unary(&mut self, op: char) -> Option<()> {
        let at = *self.starts.last()?;
        let op = format!("{}{op}", std::mem::take(&mut self.before));
        self.out.insert_str(at, &op);
        Some(())
    }

    /// Junta os últimos `n` operandos numa chamada de `name`.
    fn call(&mut self, name: &str, n: usize) -> Option<()> {
        let k = self.starts.len().checked_sub(n)?;
        let head = format!("{}{name}{}(", std::mem::take(&mut self.before), std::mem::take(&mut self.open));
        // De trás para a frente: as posições anteriores não mudam.
        for &at in self.starts[k + usize::from(n > 0)..].iter().rev() {
            self.out.insert(at, ',');
        }
        let start = self.starts.get(k).copied().unwrap_or(self.out.len());
        self.out.insert_str(start, &head);
        self.out.push_str(&std::mem::take(&mut self.close));
        self.out.push(')');
        self.starts.truncate(k);
        self.starts.push(start);
        Some(())
    }
}

fn part(rel: bool, v: i32) -> RefPart {
    if rel { RefPart::Rel(v) } else { RefPart::Abs(v as u32) }
}

/// Modelo relativo da fórmula já normalizada.
pub fn decode(norm: &[u8], names: &Names) -> Option<String> {
    let mut c = Cur::new(norm);
    let mut st = Stack {
        out: String::with_capacity(norm.len()),
        starts: Vec::new(),
        before: String::new(),
        open: String::new(),
        close: String::new(),
    };
    let xti = |i: u16| names.xti.get(i as usize);
    while let Some(ptg) = c.u8() {
        match ptg {
            0x03..=0x11 => {
                const OPS: [&str; 15] =
                    ["+", "-", "*", "/", "^", "&", "<", "<=", "=", ">=", ">", "<>", " ", ",", ":"];
                st.binary(OPS[ptg as usize - 3])?;
            }
            0x12 => st.unary('+')?,
            0x13 => st.unary('-')?,
            0x14 => {
                st.starts.last()?;
                let sp = std::mem::take(&mut st.before);
                st.out.push_str(&sp);
                st.out.push('%');
            }
            0x15 => {
                let at = *st.starts.last()?;
                let head = format!("{}(", std::mem::take(&mut st.open));
                st.out.insert_str(at, &head);
                let close = std::mem::take(&mut st.close);
                st.out.push_str(&close);
                st.out.push(')');
            }
            0x16 => st.begin(),
            0x17 => {
                st.begin();
                let n = c.u16()? as usize;
                push_quoted(&rec::utf16(c.take(n * 2)?), &mut st.out);
            }
            0x18 => {
                st.begin();
                list(&mut c, names, &mut st.out)?;
            }
            0x19 => match c.u8()? {
                0x10 => st.call("SUM", 1)?,
                _ => {
                    let (kind, n) = (c.u8()?, c.u8()?);
                    let ch = if kind & 1 == 1 { '\n' } else { ' ' };
                    let to = match kind {
                        0 | 1 => &mut st.before,
                        2 | 3 => &mut st.open,
                        4 | 5 => &mut st.close,
                        _ => continue,
                    };
                    to.extend(std::iter::repeat_n(ch, n as usize));
                }
            },
            0x1C => {
                st.begin();
                st.out.push_str(rec::error(c.u8()?).as_str());
            }
            0x1D => {
                st.begin();
                st.out.push_str(if c.u8()? == 0 { "FALSE" } else { "TRUE" });
            }
            0x1E => {
                st.begin();
                push_number(c.u16()? as f64, &mut st.out);
            }
            0x1F => {
                st.begin();
                push_number(c.f64()?, &mut st.out);
            }
            0x20 => {
                st.begin();
                array(&mut c, &mut st.out)?;
            }
            0x21 => {
                let (name, n) = ftab::function(c.u16()?)?;
                st.call(name, n? as usize)?;
            }
            0x22 => {
                let n = (c.u8()? & 0x7F) as usize;
                match c.u16()? {
                    // Função pelo nome: o primeiro operando é o nome dela.
                    255 => {
                        let k = st.starts.len().checked_sub(n.max(1))?;
                        let end = st.starts.get(k + 1).copied().unwrap_or(st.out.len());
                        let start = st.starts[k];
                        let name = st.out[start..end].to_string();
                        st.out.replace_range(start..end, "");
                        st.starts.remove(k);
                        for s in &mut st.starts[k..] {
                            *s -= end - start;
                        }
                        st.call(&name, n.max(1) - 1)?;
                    }
                    id => st.call(ftab::function(id)?.0, n)?,
                }
            }
            0x23 => {
                st.begin();
                st.out.push_str(names.names.get((c.u32()? as usize).checked_sub(1)?)?);
            }
            0x24 => {
                st.begin();
                ref_cell(&mut c, &mut st.out)?;
            }
            0x25 => {
                st.begin();
                ref_area(&mut c, &mut st.out)?;
            }
            0x2A => {
                st.begin();
                st.out.push_str("#REF!");
            }
            0x39 => {
                st.begin();
                let x = xti(c.u16()?)?;
                let i = (c.u32()? as usize).checked_sub(1)?;
                match &x.names {
                    Some(list) => {
                        st.out.push_str(&x.book);
                        st.out.push_str(list.get(i)?);
                    }
                    None => {
                        st.out.push_str(x.prefix.as_deref().unwrap_or_default());
                        st.out.push_str(names.names.get(i)?);
                    }
                }
            }
            0x3A | 0x3B => {
                st.begin();
                let prefix = xti(c.u16()?)?.prefix.as_deref();
                let at = st.out.len();
                st.out.push_str(prefix.unwrap_or_default());
                if ptg == 0x3A {
                    ref_cell(&mut c, &mut st.out)?;
                } else {
                    ref_area(&mut c, &mut st.out)?;
                }
                if prefix.is_none() {
                    st.out.truncate(at);
                    st.out.push_str("#REF!");
                }
            }
            0x3C => {
                st.begin();
                st.out.push_str(xti(c.u16()?)?.prefix.as_deref().unwrap_or_default());
                st.out.push_str("#REF!");
            }
            _ => return None,
        }
    }
    (st.starts.len() == 1).then_some(st.out)
}

fn ref_cell(c: &mut Cur, out: &mut String) -> Option<()> {
    let f = c.u8()?;
    let (r, k) = (c.i32()?, c.i32()?);
    duck_calc::template_cell(out, part(f & REL_COL != 0, k), part(f & REL_ROW != 0, r));
    Some(())
}

fn ref_area(c: &mut Cur, out: &mut String) -> Option<()> {
    let (f0, f1) = (c.u8()?, c.u8()?);
    let (r0, k0, r1, k1) = (c.i32()?, c.i32()?, c.i32()?, c.i32()?);
    let (rows, cols) = (
        [part(f0 & REL_ROW != 0, r0), part(f1 & REL_ROW != 0, r1)],
        [part(f0 & REL_COL != 0, k0), part(f1 & REL_COL != 0, k1)],
    );
    if f0 & WHOLE_ROWS != 0 {
        duck_calc::template_span(out, true, rows[0], rows[1]);
    } else if f0 & WHOLE_COLS != 0 {
        duck_calc::template_span(out, false, cols[0], cols[1]);
    } else {
        duck_calc::template_cell(out, cols[0], rows[0]);
        out.push(':');
        duck_calc::template_cell(out, cols[1], rows[1]);
    }
    Some(())
}

/// Constante matricial: `{1,2;3,"a"}`.
fn array(c: &mut Cur, out: &mut String) -> Option<()> {
    let (rows, cols) = (c.u32()?, c.u32()?);
    out.push('{');
    for r in 0..rows {
        for k in 0..cols {
            if k > 0 {
                out.push(',');
            } else if r > 0 {
                out.push(';');
            }
            match c.u8()? {
                0 => push_number(c.f64()?, out),
                1 => {
                    let n = c.u16()? as usize;
                    let mut s = String::new();
                    push_utf16(c.take(n * 2)?, &mut s);
                    push_quoted(&s, out);
                }
                2 => out.push_str(if c.u8()? == 0 { "FALSE" } else { "TRUE" }),
                4 => out.push_str(rec::error(c.take(4)?[0]).as_str()),
                _ => return None,
            }
        }
    }
    out.push('}');
    Some(())
}

/// `PtgList`: referência estruturada (`Tabela1[[#This Row],[Valor]]`).
fn list(c: &mut Cur, names: &Names, out: &mut String) -> Option<()> {
    let (_ixti, flags, id) = (c.u16()?, c.u16()?, c.u32()?);
    let (c0, c1) = (c.u16()? as usize, c.u16()? as usize);
    const INVALID: u16 = 1 << 12;
    let Some((table, cols)) = names.tables.get(&id).filter(|_| flags & INVALID == 0) else {
        out.push_str("#REF!");
        return Some(());
    };
    let mut items: Vec<String> = Vec::new();
    for (bit, name) in [(2, "#All"), (3, "#Headers"), (4, "#Data"), (5, "#Totals"), (6, "#This Row")] {
        if flags & (1 << bit) != 0 {
            items.push(name.to_string());
        }
    }
    let column = |k: usize| cols.get(k).map(|n| escape_column(n));
    let columns = match flags & 3 {
        0 => None,
        1 => Some(column(c0)?),
        _ => Some(format!("[{}]:[{}]", column(c0)?, column(c1)?)),
    };
    out.push_str(table);
    out.push('[');
    match (items.len(), &columns) {
        (0, None) => {}
        (1, None) => out.push_str(&items[0]),
        // `T[Coluna]`; com caracteres especiais, `T[[Col.una]]` vale sempre.
        (0, Some(name)) if flags & 3 == 1 && is_plain(name) => out.push_str(name),
        _ => {
            for (k, i) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                out.push_str(&format!("[{i}]"));
            }
            if let Some(name) = columns {
                if !items.is_empty() {
                    out.push(',');
                }
                if flags & 3 == 1 {
                    out.push_str(&format!("[{name}]"));
                } else {
                    out.push_str(&name);
                }
            }
        }
    }
    out.push(']');
    Some(())
}

/// Nome de coluna que pode aparecer com um colchete só.
fn is_plain(name: &str) -> bool {
    name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_'))
        && !name.starts_with(' ')
        && !name.ends_with(' ')
}

fn escape_column(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for ch in name.chars() {
        if matches!(ch, '[' | ']' | '#' | '\'') {
            s.push('\'');
        }
        s.push(ch);
    }
    s
}

/// Fórmula inteira em texto A1, para a célula (`row`, `col`).
pub fn text(rgce: &[u8], rgcb: &[u8], at: (u32, u32), names: &Names) -> Option<String> {
    let mut norm = Vec::with_capacity(rgce.len() + 16);
    normalize(rgce, rgcb, at, &mut norm)?;
    Some(duck_calc::from_template(&decode(&norm, names)?, at.0, at.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        let x = |p: Option<&str>| Xti { prefix: p.map(str::to_string), ..Default::default() };
        Names {
            names: vec!["Taxa".into(), "_xlfn.XLOOKUP".into(), "_xlfn.ANCHORARRAY".into()],
            xti: vec![
                x(Some("'Aba 2'!")),
                x(None),
                Xti {
                    prefix: Some("[1]Base!".into()),
                    book: "[1]!".into(),
                    names: Some(std::sync::Arc::new(vec!["Fora".into()])),
                },
            ],
            tables: [(7, ("Vendas".to_string(), vec!["Loja".into(), "Valor R$".into(), "A#1".into()]))]
                .into(),
        }
    }

    /// Tokens montados à mão.
    #[derive(Default)]
    struct T(Vec<u8>, Vec<u8>);

    impl T {
        fn b(mut self, b: &[u8]) -> Self {
            self.0.extend_from_slice(b);
            self
        }
        fn int(self, v: u16) -> Self {
            self.b(&[0x1E]).b(&v.to_le_bytes())
        }
        fn str(self, s: &str) -> Self {
            let u: Vec<u16> = s.encode_utf16().collect();
            let b: Vec<u8> = u.iter().flat_map(|c| c.to_le_bytes()).collect();
            self.b(&[0x17]).b(&(u.len() as u16).to_le_bytes()).b(&b)
        }
        fn cell(self, ptg: u8, row: u32, col: u16) -> Self {
            self.b(&[ptg]).b(&row.to_le_bytes()).b(&col.to_le_bytes())
        }
        fn area(self, ptg: u8, r0: u32, r1: u32, c0: u16, c1: u16) -> Self {
            self.b(&[ptg]).b(&r0.to_le_bytes()).b(&r1.to_le_bytes()).b(&c0.to_le_bytes()).b(&c1.to_le_bytes())
        }
        fn var(self, n: u8, id: u16) -> Self {
            self.b(&[0x42, n]).b(&id.to_le_bytes())
        }
        fn at(&self, row: u32, col: u32) -> Option<String> {
            text(&self.0, &self.1, (row, col), &names())
        }
    }

    const REL: u16 = F_ROW | F_COL;

    #[test]
    fn operators_and_references() {
        // (A1+$B$2)*-3%
        let t =
            T::default().cell(0x44, 0, REL).cell(0x44, 1, 1).b(&[0x03, 0x15]).int(3).b(&[0x13, 0x14, 0x05]);
        assert_eq!(t.at(4, 4).unwrap(), "(A1+$B$2)*-3%");
        // Mistas: $A1 e A$1.
        let t = T::default().cell(0x44, 0, F_ROW).cell(0x44, 0, F_COL).b(&[0x08]);
        assert_eq!(t.at(0, 0).unwrap(), "$A1&A$1");
        // Áreas, colunas e linhas inteiras.
        let t = T::default()
            .area(0x25, 0, 9, REL, 1 | REL)
            .area(0x25, 0, LAST_ROW, 2, 3)
            .area(0x25, 4, 5, 0, LAST_COL as u16)
            .var(3, 4);
        assert_eq!(t.at(20, 5).unwrap(), "SUM(A1:B10,$C:$D,$5:$6)");
        // Texto, booleano, erro, número.
        let t = T::default().str("a\"b").b(&[0x1D, 1]).b(&[0x1C, 0x2A]).b(&[0x1F]).b(&1e20f64.to_le_bytes());
        let t = t.b(&[0x16]).var(5, 1);
        assert_eq!(t.at(0, 0).unwrap(), "IF(\"a\"\"b\",TRUE,#N/A,1E+20,)");
    }

    #[test]
    fn functions_and_attributes() {
        // SUM por atributo, com IF, saltos e espaços.
        let t = T::default()
            .b(&[0x19, 0x01, 0, 0])
            .area(0x25, 0, 9, REL, REL)
            .b(&[0x19, 0x10, 0, 0])
            .b(&[0x19, 0x02, 9, 0])
            .int(1)
            .b(&[0x19, 0x40, 0, 2])
            .int(2)
            .b(&[0x19, 0x08, 3, 0])
            .var(3, 1);
        assert_eq!(t.at(0, 3).unwrap(), "IF(SUM(A1:A10),1,  2)");
        // Sem argumentos e com número fixo de argumentos.
        let t = T::default().b(&[0x41, 221, 0]).int(2).b(&[0x41, 27, 0]);
        assert_eq!(t.at(0, 0).unwrap(), "ROUND(TODAY(),2)");
        // CHOOSE com tabela de saltos.
        let t = T::default().int(1).b(&[0x19, 0x04, 2, 0, 1, 0, 2, 0, 3, 0]).int(5).int(6).var(3, 100);
        assert_eq!(t.at(0, 0).unwrap(), "CHOOSE(1,5,6)");
        // Função nova pelo nome (`PtgName` + 255), com outra aninhada.
        let t = T::default()
            .b(&[0x23, 2, 0, 0, 0])
            .cell(0x44, 0, REL)
            .b(&[0x23, 3, 0, 0, 0])
            .cell(0x24, 1, REL)
            .var(2, 255)
            .b(&[0x23, 1, 0, 0, 0])
            .var(4, 255);
        assert_eq!(t.at(0, 0).unwrap(), "_xlfn.XLOOKUP(A1,_xlfn.ANCHORARRAY(A2),Taxa)");
        // Função pelo nome sem argumentos.
        let t = T::default().b(&[0x23, 1, 0, 0, 0]).var(1, 255);
        assert_eq!(t.at(0, 0).unwrap(), "Taxa()");
    }

    #[test]
    fn sheets_names_and_errors() {
        let three_d =
            |ptg: u8, i: u8| T::default().b(&[ptg, i, 0]).b(&3u32.to_le_bytes()).b(&REL.to_le_bytes());
        assert_eq!(three_d(0x5A, 0).at(0, 0).unwrap(), "'Aba 2'!A4");
        assert_eq!(three_d(0x5A, 1).at(0, 0).unwrap(), "#REF!");
        assert_eq!(three_d(0x5A, 2).at(9, 9).unwrap(), "[1]Base!A4");
        let nx = |i: u8| T::default().b(&[0x39, i, 0, 1, 0, 0, 0]);
        assert_eq!(nx(0).at(0, 0).unwrap(), "'Aba 2'!Taxa");
        assert_eq!(nx(2).at(0, 0).unwrap(), "[1]!Fora");
        assert_eq!(T::default().b(&[0x2A, 0, 0, 0, 0, 0, 0]).at(0, 0).unwrap(), "#REF!");
        // Subexpressão de referência (`PtgMemFunc`): só o conteúdo aparece.
        let t = T::default().b(&[0x29, 9, 0]).cell(0x24, 0, 0).cell(0x24, 1, 0).b(&[0x10, 0x15]);
        assert_eq!(t.at(0, 0).unwrap(), "($A$1,$A$2)");
    }

    #[test]
    fn arrays_and_structured_references() {
        let mut t = T::default().b(&[0x60]).b(&[0; 14]);
        t.1.extend_from_slice(&2u32.to_le_bytes());
        t.1.extend_from_slice(&2u32.to_le_bytes());
        t.1.push(0);
        t.1.extend_from_slice(&1.5f64.to_le_bytes());
        t.1.extend_from_slice(&[1, 1, 0, b'x', 0]);
        t.1.extend_from_slice(&[2, 1]);
        t.1.extend_from_slice(&[4, 0x2A, 0, 0, 0]);
        assert_eq!(t.at(0, 0).unwrap(), "{1.5,\"x\";TRUE,#N/A}");

        let list = |flags: u16, id: u32, c0: u16, c1: u16| {
            T::default()
                .b(&[0x18, 0x19, 0, 0])
                .b(&flags.to_le_bytes())
                .b(&id.to_le_bytes())
                .b(&c0.to_le_bytes())
                .b(&c1.to_le_bytes())
                .at(0, 0)
                .unwrap()
        };
        assert_eq!(list(0, 7, 0, 0), "Vendas[]");
        assert_eq!(list(1, 7, 0, 0), "Vendas[Loja]");
        assert_eq!(list(1, 7, 1, 1), "Vendas[[Valor R$]]");
        assert_eq!(list(0x41, 7, 1, 1), "Vendas[[#This Row],[Valor R$]]");
        assert_eq!(list(0x42, 7, 0, 2), "Vendas[[#This Row],[Loja]:[A'#1]]");
        assert_eq!(list(0x04, 7, 0, 0), "Vendas[#All]");
        assert_eq!(list(0x18, 7, 0, 0), "Vendas[[#Headers],[#Data]]");
        assert_eq!(list(0x09, 7, 0, 0), "Vendas[[#Headers],[Loja]]");
        assert_eq!(list(0, 99, 0, 0), "#REF!");
    }

    /// Cópias para baixo têm a mesma chave, e a compartilhada (com
    /// deslocamentos) a mesma que a comum.
    #[test]
    fn same_key_for_copies_and_shared() {
        let key = |t: &T, at| {
            let mut v = Vec::new();
            normalize(&t.0, &t.1, at, &mut v).unwrap();
            v
        };
        let plain = |row: u32| {
            T::default()
                .cell(0x44, row, REL)
                .area(0x25, 0, LAST_ROW, 1, 1)
                .area(0x25, 0, row, 2, 2 | F_ROW)
                .var(3, 4)
        };
        assert_eq!(key(&plain(5), (5, 3)), key(&plain(900), (900, 3)));
        // A1 a partir de D6: -5 linhas, -3 colunas (com volta).
        let shared = T::default().cell(0x4C, (1 << 20) - 5, REL | ((1 << 14) - 3));
        let common = T::default().cell(0x44, 0, REL);
        assert_eq!(key(&shared, (5, 3)), key(&common, (5, 3)));
        assert_eq!(shared.at(5, 3).unwrap(), "A1");
        // Membro do grupo mais abaixo.
        let mut norm = Vec::new();
        normalize(&shared.0, &[], (5, 3), &mut norm).unwrap();
        let t = decode(&norm, &names()).unwrap();
        assert_eq!(duck_calc::from_template(&t, 105, 4), "B101");
        // Deslocamento grande para baixo não vira negativo.
        let far = T::default().cell(0x4C, 700_000, REL);
        assert_eq!(far.at(0, 0).unwrap(), "A700001");
    }

    #[test]
    fn unknown_or_truncated_is_none() {
        assert!(T::default().b(&[0x24, 1, 0]).at(0, 0).is_none());
        assert!(T::default().b(&[0x1A]).at(0, 0).is_none());
        assert!(T::default().b(&[0x03]).at(0, 0).is_none());
        assert!(T::default().int(1).int(2).at(0, 0).is_none());
        assert!(T::default().int(1).var(2, 4).at(0, 0).is_none());
        assert!(T::default().b(&[0x18, 0x1D, 0, 0, 0, 0]).at(0, 0).is_none());
        assert!(T::default().b(&[0x23, 99, 0, 0, 0]).at(0, 0).is_none());
        assert!(T::default().b(&[0x42, 2, 0xFF, 0]).at(0, 0).is_none());
    }
}
