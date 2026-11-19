//! Leitura de uma aba (`xl/worksheets/sheetN.xml`) com parse paralelo em pedaços.
//!
//! ```text
//!                             ┌─▶ parse (thread 1) ─▶ SheetPart 0 ─┐
//! inflate ──pedaços de 8 MB──┼─▶ parse (thread 2) ─▶ SheetPart 1 ─┼─▶ merge em ordem ─▶ Sheet
//! (cortes após </row>)       └─▶ parse (thread N) ─▶ SheetPart k ─┘
//! ```
//!
//! Cada pedaço começa numa fronteira de linha, e as linhas trazem o próprio
//! número (`<row r="12345">`), então cada pedaço é montado sem depender dos
//! outros. Se uma aba não tiver os números (raro), ela é relida em sequência.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use duck_core::{CellError, Sheet, SheetPart, kind};
use duck_par::{InFlight, cut_after_last};
use memchr::{memchr, memmem};

use crate::Error;
use crate::package::{Entry, Package};
use crate::xml::{attr, is_name_end, parse_u32, rich_text_into, tag_end, unescape_into};

/// Estatísticas da leitura de uma aba.
#[derive(Clone, Debug, Default)]
pub struct SheetReport {
    pub name: String,
    pub xml_bytes: u64,
    pub chunks: usize,
    /// Tempo de parede desde o início da aba até ela estar montada.
    pub wall: Duration,
    /// Tempo gasto descompactando (uma thread).
    pub inflate: Duration,
    /// Soma do tempo de parse de todos os pedaços (várias threads).
    pub parse_cpu: Duration,
    pub merge: Duration,
    /// A aba precisou ser lida em sequência (linhas sem número).
    pub sequential: bool,
}

/// O pedaço não pode ser lido isoladamente (linha/célula sem número).
#[derive(Debug)]
pub(crate) struct NeedsSequential;

/// Estado de leitura que atravessa pedaços no modo sequencial.
#[derive(Clone, Copy, Default)]
pub(crate) struct RowState {
    /// Linha atual (0-based); `None` se ainda desconhecida.
    row: Option<u32>,
    next_col: u32,
}

/// Lê as linhas de `buf` para `part`.
pub(crate) fn parse_rows(
    buf: &[u8],
    part: &mut SheetPart,
    state: &mut RowState,
    scratch: &mut String,
) -> Result<(), NeedsSequential> {
    let close_c = memmem::Finder::new(b"</c>");
    let mut pos = 0;
    while let Some(off) = memchr(b'<', &buf[pos..]) {
        let i = pos + off;
        let rest = &buf[i + 1..];
        // <row ...>
        if rest.len() > 3 && &rest[..3] == b"row" && is_name_end(rest[3]) {
            let Some(end) = tag_end(buf, i) else { break };
            let attrs = &buf[i + 4..end - 1];
            state.row = match attr(attrs, b"r").and_then(parse_u32) {
                Some(r) if r > 0 => Some(r - 1),
                _ => state.row.map(|r| r + 1),
            };
            state.next_col = 0;
            pos = end;
            continue;
        }
        // <c ...>
        if rest.len() > 1 && rest[0] == b'c' && is_name_end(rest[1]) {
            let Some(end) = tag_end(buf, i) else { break };
            let self_closing = buf[end - 2] == b'/';
            let attrs = &buf[i + 2..if self_closing { end - 2 } else { end - 1 }];
            let (mut r_attr, mut s_attr, mut t_attr) = (None, None, None);
            scan_cell_attrs(attrs, &mut r_attr, &mut s_attr, &mut t_attr);

            let (col, row) = match r_attr.and_then(parse_ref) {
                Some((c, Some(r))) => (c, r),
                Some((c, None)) => (c, state.row.ok_or(NeedsSequential)?),
                None => (state.next_col, state.row.ok_or(NeedsSequential)?),
            };
            state.row = Some(row);
            state.next_col = col + 1;
            let style = s_attr.and_then(parse_u32).unwrap_or(0).min(u16::MAX as u32) as u16;

            if self_closing {
                if style != 0 {
                    part.set(row, col, kind::EMPTY, style, 0);
                }
                pos = end;
                continue;
            }
            let close = match close_c.find(&buf[end..]) {
                Some(k) => end + k,
                None => buf.len(),
            };
            let content = &buf[end..close];
            store_cell(part, row, col, style, t_attr, content, scratch);
            pos = (close + 4).min(buf.len());
            continue;
        }
        pos = i + 1;
    }
    Ok(())
}

/// Lê `r`, `s` e `t` numa passada.
#[inline]
fn scan_cell_attrs<'a>(
    attrs: &'a [u8],
    r: &mut Option<&'a [u8]>,
    s: &mut Option<&'a [u8]>,
    t: &mut Option<&'a [u8]>,
) {
    let mut i = 0;
    let n = attrs.len();
    while i < n {
        while i < n && attrs[i].is_ascii_whitespace() {
            i += 1;
        }
        let k0 = i;
        while i < n && attrs[i] != b'=' {
            i += 1;
        }
        let key = &attrs[k0..i];
        i += 1;
        while i < n && attrs[i] != b'"' && attrs[i] != b'\'' {
            i += 1;
        }
        if i >= n {
            return;
        }
        let q = attrs[i];
        let v0 = i + 1;
        let Some(len) = memchr(q, &attrs[v0..]) else { return };
        let v = &attrs[v0..v0 + len];
        match key.trim_ascii() {
            b"r" => *r = Some(v),
            b"s" => *s = Some(v),
            b"t" => *t = Some(v),
            _ => {}
        }
        i = v0 + len + 1;
    }
}

/// "AB12" → (coluna 27, Some(linha 11)); "AB" → (27, None). Base 0.
#[inline]
pub(crate) fn parse_ref(r: &[u8]) -> Option<(u32, Option<u32>)> {
    let mut i = 0;
    let mut col: u32 = 0;
    while i < r.len() && r[i].is_ascii_alphabetic() {
        col = col.checked_mul(26)? + (r[i].to_ascii_uppercase() - b'A') as u32 + 1;
        i += 1;
    }
    if col == 0 {
        return None;
    }
    let row = if i < r.len() { Some(parse_u32(&r[i..])?.checked_sub(1)?) } else { None };
    Some((col - 1, row))
}

#[inline]
fn find_child(content: &[u8], name: &[u8]) -> Option<(usize, usize)> {
    // Retorna (início do conteúdo, fim do conteúdo) de <name ...>...</name>.
    let mut pos = 0;
    while let Some(off) = memchr(b'<', &content[pos..]) {
        let i = pos + off;
        let rest = &content[i + 1..];
        if rest.len() > name.len() && &rest[..name.len()] == name && is_name_end(rest[name.len()]) {
            let end = tag_end(content, i)?;
            if content[end - 2] == b'/' {
                return Some((end, end));
            }
            let mut close = Vec::with_capacity(name.len() + 3);
            close.extend_from_slice(b"</");
            close.extend_from_slice(name);
            close.push(b'>');
            let k = memmem::find(&content[end..], &close).unwrap_or(content.len() - end);
            return Some((end, end + k));
        }
        pos = i + 1;
    }
    None
}

#[inline]
fn store_cell(
    part: &mut SheetPart,
    row: u32,
    col: u32,
    style: u16,
    t: Option<&[u8]>,
    content: &[u8],
    scratch: &mut String,
) {
    let formula = if find_child(content, b"f").is_some() { kind::FORMULA } else { 0 };
    let t = t.unwrap_or(b"n");
    if t == b"inlineStr" {
        scratch.clear();
        if let Some((a, b)) = find_child(content, b"is") {
            rich_text_into(&content[a..b], scratch);
        }
        let id = part.strings.push(scratch);
        part.set(row, col, kind::LOCAL_STR | formula, style, id as u64);
        return;
    }
    let v = match find_child(content, b"v") {
        Some((a, b)) => &content[a..b],
        None => {
            if style != 0 || formula != 0 {
                part.set(row, col, kind::EMPTY | formula, style, 0);
            }
            return;
        }
    };
    match t {
        b"s" => match parse_u32(v.trim_ascii()) {
            Some(i) => part.set(row, col, kind::SHARED_STR | formula, style, i as u64),
            None => part.set(row, col, kind::EMPTY | formula, style, 0),
        },
        b"b" => part.set(row, col, kind::BOOL | formula, style, (v.trim_ascii() == b"1") as u64),
        b"e" => part.set(row, col, kind::ERROR | formula, style, CellError::parse(v.trim_ascii()) as u64),
        b"str" | b"d" => {
            scratch.clear();
            unescape_into(v, scratch);
            let id = part.strings.push(scratch);
            part.set(row, col, kind::LOCAL_STR | formula, style, id as u64);
        }
        _ => match fast_float2::parse::<f64, _>(v.trim_ascii()) {
            Ok(x) => part.set(row, col, kind::NUMBER | formula, style, x.to_bits()),
            Err(_) => {
                // Número malformado: guarda o texto para não perder o conteúdo.
                scratch.clear();
                unescape_into(v, scratch);
                let id = part.strings.push(scratch);
                part.set(row, col, kind::LOCAL_STR | formula, style, id as u64);
            }
        },
    }
}

/// Lê uma aba inteira. Chamado de dentro de um pool rayon.
pub(crate) fn load_sheet(
    pkg: &Package,
    entry: &Entry,
    name: &str,
    chunk_bytes: usize,
    limiter: &InFlight,
) -> Result<(Sheet, SheetReport), Error> {
    let t0 = Instant::now();
    let mut report = SheetReport { name: name.to_string(), xml_bytes: entry.size, ..Default::default() };

    let parts: Mutex<Vec<(usize, SheetPart, Duration)>> = Mutex::new(Vec::new());
    let needs_seq = std::sync::atomic::AtomicBool::new(false);
    let mut inflate_time = Duration::ZERO;
    let mut io_error = None;

    rayon::scope(|s| {
        let mut inf = pkg.inflater(entry);
        let mut buf: Vec<u8> = Vec::with_capacity(chunk_bytes + 64 * 1024);
        let mut idx = 0usize;
        loop {
            let ti = Instant::now();
            let want = buf.len() + chunk_bytes;
            let eof = match inf.read_into(&mut buf, want) {
                Ok(e) => e,
                Err(e) => {
                    io_error = Some(e);
                    break;
                }
            };
            inflate_time += ti.elapsed();
            let cut = if eof {
                buf.len()
            } else {
                match cut_after_last(&buf, b"</row>") {
                    Some(c) => c,
                    // Linha maior que o pedaço: lê mais antes de cortar.
                    None => continue,
                }
            };
            let tail = buf.split_off(cut);
            let mut chunk = std::mem::replace(&mut buf, tail);
            buf.reserve(chunk_bytes);

            limiter.acquire();
            let (parts, needs_seq) = (&parts, &needs_seq);
            let my = idx;
            s.spawn(move |_| {
                let tp = Instant::now();
                let mut part = SheetPart::new();
                let mut state = RowState::default();
                let mut scratch = String::new();
                if parse_rows(&chunk, &mut part, &mut state, &mut scratch).is_err() {
                    needs_seq.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                chunk.clear();
                chunk.shrink_to(0);
                limiter.release();
                parts.lock().unwrap().push((my, part, tp.elapsed()));
            });
            idx += 1;
            if eof {
                break;
            }
        }
        report.chunks = idx;
    });
    if let Some(e) = io_error {
        return Err(e);
    }
    report.inflate = inflate_time;

    let tm = Instant::now();
    let mut sheet = Sheet::new(name);
    if needs_seq.into_inner() {
        drop(parts);
        report.sequential = true;
        sequential(pkg, entry, chunk_bytes, &mut sheet)?;
    } else {
        let mut parts = parts.into_inner().unwrap();
        parts.sort_by_key(|p| p.0);
        for (_, part, cpu) in parts {
            report.parse_cpu += cpu;
            sheet.merge_part(part);
        }
    }
    report.merge = tm.elapsed();
    report.wall = t0.elapsed();
    Ok((sheet, report))
}

/// Leitura em sequência, carregando o número da linha entre pedaços.
fn sequential(pkg: &Package, entry: &Entry, chunk_bytes: usize, sheet: &mut Sheet) -> Result<(), Error> {
    let mut inf = pkg.inflater(entry);
    let mut buf = Vec::with_capacity(chunk_bytes);
    let mut state = RowState::default();
    let mut scratch = String::new();
    loop {
        let want = buf.len() + chunk_bytes;
        let eof = inf.read_into(&mut buf, want)?;
        let cut = if eof {
            buf.len()
        } else {
            match cut_after_last(&buf, b"</row>") {
                Some(c) => c,
                None => continue,
            }
        };
        let tail = buf.split_off(cut);
        let mut part = SheetPart::new();
        // Linhas/células sem número continuam a contagem do pedaço anterior.
        let _ = parse_rows(&buf, &mut part, &mut state, &mut scratch);
        sheet.merge_part(part);
        buf = tail;
        if eof {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{StringTable, Value};

    fn parse_all(xml: &[u8]) -> Sheet {
        let mut part = SheetPart::new();
        let mut st = RowState::default();
        let mut s = String::new();
        parse_rows(xml, &mut part, &mut st, &mut s).unwrap();
        let mut sheet = Sheet::new("t");
        sheet.merge_part(part);
        sheet
    }

    #[test]
    fn refs() {
        assert_eq!(parse_ref(b"A1"), Some((0, Some(0))));
        assert_eq!(parse_ref(b"AB12"), Some((27, Some(11))));
        assert_eq!(parse_ref(b"XFD1048576"), Some((16383, Some(1048575))));
        assert_eq!(parse_ref(b"C"), Some((2, None)));
        assert_eq!(parse_ref(b"12"), None);
    }

    #[test]
    fn cell_types() {
        let xml = br#"<sheetData>
<row r="1"><c r="A1" t="s"><v>1</v></c><c r="B1"><v>3.25</v></c><c r="C1" t="b"><v>1</v></c>
<c r="D1" t="e"><v>#N/A</v></c><c r="E1" t="str"><f>A1&amp;"x"</f><v>a &amp; b</v></c>
<c r="F1" t="inlineStr"><is><t>inline</t></is></c><c r="G1" s="7"/><c r="H1" s="2"><f>1/0</f><v>1E-3</v></c></row>
<row r="5"><c><v>10</v></c><c><v>11</v></c></row>
</sheetData><cols><col min="1" max="2"/></cols><conditionalFormatting><cfRule><formula>1</formula></cfRule></conditionalFormatting>"#;
        let sheet = parse_all(xml);
        let mut shared = StringTable::new();
        shared.push("zero");
        shared.push("um");
        let c = |r, c| sheet.cell(r, c, &shared);
        assert_eq!(c(0, 0).value, Value::Text("um"));
        assert_eq!(c(0, 1).value, Value::Number(3.25));
        assert_eq!(c(0, 2).value, Value::Bool(true));
        assert_eq!(c(0, 3).value, Value::Error(CellError::NA));
        assert_eq!(c(0, 4).value, Value::Text("a & b"));
        assert!(c(0, 4).formula);
        assert_eq!(c(0, 5).value, Value::Text("inline"));
        assert_eq!(c(0, 6).value, Value::Empty);
        assert_eq!(c(0, 6).style, 7);
        assert_eq!(c(0, 7).value, Value::Number(0.001));
        assert!(c(0, 7).formula);
        // Células sem `r` seguem a ordem da linha.
        assert_eq!(c(4, 0).value, Value::Number(10.0));
        assert_eq!(c(4, 1).value, Value::Number(11.0));
        assert_eq!((sheet.rows(), sheet.cols()), (5, 8));
    }

    #[test]
    fn chunk_without_row_numbers_asks_for_sequential() {
        let mut part = SheetPart::new();
        let mut st = RowState::default();
        let mut s = String::new();
        let r = parse_rows(b"<row><c><v>1</v></c></row>", &mut part, &mut st, &mut s);
        assert!(r.is_err());
    }
}
