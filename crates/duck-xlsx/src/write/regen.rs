//! Aba com linhas/colunas inseridas ou apagadas (F3.5): o `<sheetData>` é
//! escrito inteiro a partir do modelo, em paralelo (pedaços de linhas), e o
//! resto do XML original anda junto com as mudanças:
//!
//! - `<dimension>` e `<cols>` saem do modelo; `<mergeCells>` também;
//! - atributos `ref`/`sqref`/`topLeftCell`/`activeCell` e os textos
//!   `<xm:sqref>` (intervalos) e `<formula>`, `<xm:f>` (fórmulas) são
//!   ajustados por cada mudança, em ordem;
//! - formatação condicional e links que ficaram sem intervalo saem;
//! - validação e intervalos protegidos saem do modelo (`tail`).
//!
//! O que o modelo não guarda nas células (`cm`, `vm`) e nas linhas
//! (`outlineLevel`, `spans`) não volta nesta aba.

use duck_core::{CellRange, Sheet, Shift, StringTable};
use memchr::memmem;
use rayon::prelude::*;

use super::cellxml::{cell_ref, escape_text, push_number, push_u32, write_cell};
use crate::xml::{find_tag, for_each_attr, is_name_end, tag_end, unescape_into};

/// Linhas por pedaço gerado em paralelo.
const ROWS_PER_CHUNK: u32 = 2048;

/// O `<sheetData>` inteiro, entregue em pedaços em ordem a `sink`.
pub fn sheet_data(
    sheet: &Sheet,
    shared: &StringTable,
    sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<u64> {
    sink(b"<sheetData>")?;
    let rows = sheet.rows();
    let chunks: Vec<u32> = (0..rows.div_ceil(ROWS_PER_CHUNK)).collect();
    let batch = rayon::current_num_threads().max(1) * 2;
    let mut cells = 0u64;
    for group in chunks.chunks(batch) {
        let parts: Vec<(Vec<u8>, u64)> = group
            .par_iter()
            .map(|&k| {
                let r0 = k * ROWS_PER_CHUNK;
                let r1 = (r0 + ROWS_PER_CHUNK).min(rows);
                let mut out = Vec::with_capacity(256 * 1024);
                let mut n = 0;
                for r in r0..r1 {
                    n += write_row(sheet, shared, r, &mut out);
                }
                (out, n)
            })
            .collect();
        for (p, n) in parts {
            cells += n;
            sink(&p)?;
        }
    }
    // Linhas só com altura/estilo depois da última célula.
    let mut tail = Vec::new();
    for spec in sheet.layout.rows.iter().filter(|s| s.row >= rows) {
        write_row(sheet, shared, spec.row, &mut tail);
    }
    sink(&tail)?;
    sink(b"</sheetData>")?;
    Ok(cells)
}

/// Uma linha (atributos do layout + células). Devolve quantas células escreveu.
fn write_row(sheet: &Sheet, shared: &StringTable, r: u32, out: &mut Vec<u8>) -> u64 {
    let spec = sheet.layout.row(r);
    let outline = &sheet.layout.outline;
    let (level, closed) = (outline.rows.level(r), outline.closed_rows.contains(&r));
    let start = out.len();
    out.extend_from_slice(b"<row r=\"");
    push_u32(out, r + 1);
    out.push(b'"');
    if let Some(s) = spec {
        if let Some(st) = s.style {
            out.extend_from_slice(b" s=\"");
            push_u32(out, st as u32);
            out.extend_from_slice(b"\" customFormat=\"1\"");
        }
        if let Some(h) = s.height {
            out.extend_from_slice(b" ht=\"");
            push_number(out, h as f64);
            out.extend_from_slice(b"\" customHeight=\"1\"");
        }
        if s.hidden {
            out.extend_from_slice(b" hidden=\"1\"");
        }
    }
    if level > 0 {
        out.extend_from_slice(b" outlineLevel=\"");
        push_u32(out, level as u32);
        out.push(b'"');
    }
    if closed {
        out.extend_from_slice(b" collapsed=\"1\"");
    }
    out.push(b'>');
    let body = out.len();
    let mut n = 0;
    for c in 0..sheet.cols() {
        let before = out.len();
        write_cell(out, sheet, shared, r, c, b"");
        n += (out.len() > before) as u64;
    }
    if out.len() == body && spec.is_none() && level == 0 && !closed {
        out.truncate(start);
        return 0;
    }
    out.extend_from_slice(b"</row>");
    n
}

/// O que vem antes de `<sheetData>`: `<dimension>` e `<cols>` do modelo;
/// referências da visão ajustadas.
pub fn head(orig: &[u8], sheet: &Sheet, shifts: &[Shift]) -> Vec<u8> {
    let mut out = Vec::with_capacity(orig.len() + 1024);
    let mut pos = 0;
    let mut cols_done = false;
    while let Some(k) = memchr::memchr(b'<', &orig[pos..]) {
        let i = pos + k;
        out.extend_from_slice(&orig[pos..i]);
        if matches!(orig.get(i + 1), Some(b'/' | b'!' | b'?')) {
            let end = tag_end(orig, i).unwrap_or(orig.len());
            out.extend_from_slice(&orig[i..end]);
            pos = end;
            continue;
        }
        let name_len = orig[i + 1..].iter().position(|&c| is_name_end(c)).unwrap_or(0);
        let name = &orig[i + 1..i + 1 + name_len];
        let Some(end) = tag_end(orig, i) else {
            out.extend_from_slice(&orig[i..]);
            return out;
        };
        match name {
            b"dimension" => {
                out.extend_from_slice(b"<dimension ref=\"A1");
                if sheet.rows() > 0 && sheet.cols() > 0 {
                    out.push(b':');
                    cell_ref(sheet.rows() - 1, sheet.cols() - 1, &mut out);
                }
                out.extend_from_slice(b"\"/>");
                pos = end;
            }
            b"cols" => {
                let close = if orig[end - 2] == b'/' {
                    end
                } else {
                    memmem::find(&orig[end..], b"</cols>").map_or(end, |x| end + x + 7)
                };
                cols(sheet, &mut out);
                cols_done = true;
                pos = close;
            }
            _ => {
                out.extend_from_slice(&adjust_tag(&orig[i..end], shifts));
                pos = end;
            }
        }
    }
    out.extend_from_slice(&orig[pos..]);
    if !cols_done && !sheet.layout.cols.is_empty() {
        // `<cols>` fica logo antes de `<sheetData>`, que vem depois daqui.
        cols(sheet, &mut out);
    }
    out
}

fn cols(sheet: &Sheet, out: &mut Vec<u8>) {
    if sheet.layout.cols.is_empty() {
        return;
    }
    out.extend_from_slice(b"<cols>");
    for c in &sheet.layout.cols {
        out.extend_from_slice(b"<col min=\"");
        push_u32(out, c.first + 1);
        out.extend_from_slice(b"\" max=\"");
        push_u32(out, c.last + 1);
        out.push(b'"');
        if let Some(w) = c.width {
            out.extend_from_slice(b" width=\"");
            push_number(out, w as f64);
            out.extend_from_slice(b"\" customWidth=\"1\"");
        }
        if c.style != 0 {
            out.extend_from_slice(b" style=\"");
            push_u32(out, c.style as u32);
            out.push(b'"');
        }
        if c.hidden {
            out.extend_from_slice(b" hidden=\"1\"");
        }
        let level = sheet.layout.outline.cols.level(c.first);
        if level > 0 {
            out.extend_from_slice(b" outlineLevel=\"");
            push_u32(out, level as u32);
            out.push(b'"');
        }
        if sheet.layout.outline.closed_cols.contains(&c.first) {
            out.extend_from_slice(b" collapsed=\"1\"");
        }
        out.extend_from_slice(b"/>");
    }
    out.extend_from_slice(b"</cols>");
}

/// Ajusta uma lista de intervalos (`A1:B2 C5`). `None` = nada sobrou.
fn adjust_sqref(v: &str, shifts: &[Shift]) -> Option<String> {
    let mut out = Vec::new();
    for part in v.split_whitespace() {
        match crate::sheet::parse_range(part.replace('$', "").as_bytes()) {
            Some(mut r) => {
                let mut alive = true;
                for s in shifts {
                    match s.range(r) {
                        Some(x) => r = x,
                        None => {
                            alive = false;
                            break;
                        }
                    }
                }
                if alive {
                    out.push(range_text(r));
                }
            }
            // Algo que não entendemos: fica como está.
            None => out.push(part.to_string()),
        }
    }
    (!out.is_empty()).then(|| out.join(" "))
}

fn range_text(r: CellRange) -> String {
    let mut v = Vec::new();
    cell_ref(r.first_row, r.first_col, &mut v);
    if (r.first_row, r.first_col) != (r.last_row, r.last_col) {
        v.push(b':');
        cell_ref(r.last_row, r.last_col, &mut v);
    }
    String::from_utf8(v).unwrap_or_default()
}

/// Uma tag de abertura com `ref`, `sqref`, `topLeftCell` e `activeCell` ajustados.
/// `None` se um `ref`/`sqref` ficou sem intervalo.
fn adjust_tag_opt(tag: &[u8], shifts: &[Shift]) -> Option<Vec<u8>> {
    let name_len = tag[1..].iter().position(|&c| is_name_end(c)).unwrap_or(tag.len() - 1);
    let self_closing = tag.ends_with(b"/>");
    let attrs = &tag[1 + name_len..tag.len() - if self_closing { 2 } else { 1 }];
    let mut out = tag[..1 + name_len].to_vec();
    let mut gone = false;
    for_each_attr(attrs, |k, whole| {
        if matches!(k, b"ref" | b"sqref" | b"topLeftCell" | b"activeCell") {
            let raw = &whole[k.len() + 2..whole.len() - 1];
            let mut v = String::new();
            unescape_into(raw, &mut v);
            match adjust_sqref(&v, shifts) {
                Some(n) => {
                    out.push(b' ');
                    out.extend_from_slice(k);
                    out.extend_from_slice(b"=\"");
                    out.extend_from_slice(n.as_bytes());
                    out.push(b'"');
                }
                None if matches!(k, b"ref" | b"sqref") => gone = true,
                None => {}
            }
        } else {
            out.push(b' ');
            out.extend_from_slice(whole);
        }
    });
    out.extend_from_slice(if self_closing { b"/>" } else { b">" });
    (!gone).then_some(out)
}

fn adjust_tag(tag: &[u8], shifts: &[Shift]) -> Vec<u8> {
    adjust_tag_opt(tag, shifts).unwrap_or_else(|| tag.to_vec())
}

/// O que vem depois de `</sheetData>`, com as referências ajustadas.
pub fn tail(orig: &[u8], sheet: &Sheet, sheet_name: &str, shifts: &[Shift]) -> Vec<u8> {
    let mut out = Vec::with_capacity(orig.len() + 1024);
    let mut pos = 0;
    let mut merges_done = false;
    while let Some(k) = memchr::memchr(b'<', &orig[pos..]) {
        let i = pos + k;
        out.extend_from_slice(&orig[pos..i]);
        if orig.get(i + 1) == Some(&b'/') || orig.get(i + 1) == Some(&b'?') || orig.get(i + 1) == Some(&b'!')
        {
            let end = tag_end(orig, i).unwrap_or(orig.len());
            out.extend_from_slice(&orig[i..end]);
            pos = end;
            continue;
        }
        let name_len = orig[i + 1..].iter().position(|&c| is_name_end(c)).unwrap_or(0);
        let name = &orig[i + 1..i + 1 + name_len];
        let Some(end) = tag_end(orig, i) else {
            out.extend_from_slice(&orig[i..]);
            return out;
        };
        let self_closing = orig[end - 2] == b'/';
        let close_of = |n: &[u8]| -> usize {
            if self_closing {
                return end;
            }
            let mut c = b"</".to_vec();
            c.extend_from_slice(n);
            c.push(b'>');
            memmem::find(&orig[end..], &c).map_or(orig.len(), |x| end + x + c.len())
        };
        match name {
            b"mergeCells" => {
                pos = close_of(name);
                out.extend_from_slice(&super::patch::merges_xml(sheet));
                merges_done = true;
            }
            // Elementos que somem se o intervalo sumiu.
            b"conditionalFormatting" | b"x14:conditionalFormatting" | b"hyperlink" | b"ignoredError" => {
                let close = close_of(name);
                let whole = &orig[i..close];
                // `x14`: o intervalo está no texto de `<xm:sqref>`.
                let inner = adjust_texts(&whole[end - i..], sheet_name, shifts);
                let x14_gone = name.starts_with(b"x14:")
                    && find_tag(&whole[end - i..], 0, b"xm:sqref").is_some()
                    && !inner.windows(9).any(|w| w == b"<xm:sqref");
                match adjust_tag_opt(&orig[i..end], shifts) {
                    Some(t) if !x14_gone => {
                        out.extend_from_slice(&t);
                        out.extend_from_slice(&inner);
                    }
                    _ => {}
                }
                pos = close;
            }
            _ => {
                out.extend_from_slice(&adjust_tag(&orig[i..end], shifts));
                pos = end;
            }
        }
    }
    out.extend_from_slice(&orig[pos..]);
    if !merges_done && !sheet.layout.merges.is_empty() {
        // Sem `<mergeCells>` no original: entra no lugar do esquema.
        out = super::patch::replace_merges(&out, &super::patch::merges_xml(sheet));
    }
    if sheet.edits().filter {
        out = super::patch::replace_autofilter(
            &out,
            &super::patch::autofilter_xml(sheet.layout.autofilter.as_ref()),
        );
    }
    out
}

/// Dentro de um elemento: fórmulas (`<formula>`, `<xm:f>`) e listas de
/// intervalo (`<xm:sqref>`) ajustadas; `<xm:sqref>` vazio some.
fn adjust_texts(body: &[u8], sheet_name: &str, shifts: &[Shift]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut pos = 0;
    while let Some(k) = memchr::memchr(b'<', &body[pos..]) {
        let i = pos + k;
        out.extend_from_slice(&body[pos..i]);
        let Some(end) = tag_end(body, i) else {
            out.extend_from_slice(&body[i..]);
            return out;
        };
        if matches!(body.get(i + 1), Some(b'/' | b'!' | b'?')) {
            out.extend_from_slice(&body[i..end]);
            pos = end;
            continue;
        }
        let name_len = body[i + 1..].iter().position(|&c| is_name_end(c)).unwrap_or(0);
        let name = &body[i + 1..i + 1 + name_len];
        let is_formula = matches!(name, b"formula" | b"xm:f");
        let is_sqref = name == b"xm:sqref";
        if (is_formula || is_sqref) && body[end - 2] != b'/' {
            let mut close = b"</".to_vec();
            close.extend_from_slice(name);
            close.push(b'>');
            let Some(x) = memmem::find(&body[end..], &close) else {
                out.extend_from_slice(&body[i..]);
                return out;
            };
            let mut text = String::new();
            unescape_into(&body[end..end + x], &mut text);
            let new = if is_formula {
                let mut t = text.clone();
                for s in shifts {
                    if let Some((n, _)) = duck_calc::adjust_refs(&t, *s, true, sheet_name) {
                        t = n;
                    }
                }
                Some(t)
            } else {
                adjust_sqref(&text, shifts)
            };
            if let Some(n) = new {
                out.extend_from_slice(&body[i..end]);
                escape_text(&n, &mut out);
                out.extend_from_slice(&close);
            }
            pos = end + x + close.len();
            continue;
        }
        out.extend_from_slice(&adjust_tag(&body[i..end], shifts));
        pos = end;
    }
    out.extend_from_slice(&body[pos..]);
    out
}

/// `<definedName>` do workbook com as referências às abas mudadas ajustadas.
pub fn defined_names(xml: &[u8], changes: &[(&str, &[Shift])]) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut pos = 0;
    while let Some(t) = find_tag(xml, pos, b"definedName") {
        if t.self_closing {
            out.extend_from_slice(&xml[pos..t.end]);
            pos = t.end;
            continue;
        }
        let Some(x) = memmem::find(&xml[t.end..], b"</definedName>") else { break };
        out.extend_from_slice(&xml[pos..t.end]);
        let mut text = String::new();
        unescape_into(&xml[t.end..t.end + x], &mut text);
        for (name, shifts) in changes {
            for s in *shifts {
                if let Some((n, _)) = duck_calc::adjust_refs(&text, *s, false, name) {
                    text = n;
                }
            }
        }
        escape_text(&text, &mut out);
        pos = t.end + x;
    }
    out.extend_from_slice(&xml[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::Axis;

    #[test]
    fn tail_refs_follow_the_shift() {
        let s = [Shift { axis: Axis::Rows, at: 1, n: 2, insert: false }]; // apaga as linhas 2 e 3
        let mut sheet = Sheet::new("Dados");
        sheet.layout.merges.push(CellRange { first_row: 4, first_col: 0, last_row: 5, last_col: 1 });
        let orig = concat!(
            r#"<mergeCells count="1"><mergeCell ref="A7:B8"/></mergeCells>"#,
            r#"<conditionalFormatting sqref="A2:A3"><cfRule type="expression"><formula>A2&gt;0</formula></cfRule></conditionalFormatting>"#,
            r#"<conditionalFormatting sqref="B5:B9"><cfRule type="expression"><formula>Dados!$A$9&gt;0</formula></cfRule></conditionalFormatting>"#,
            r#"<hyperlinks><hyperlink ref="C10" r:id="rId1"/></hyperlinks></worksheet>"#,
        );
        let out = String::from_utf8(tail(orig.as_bytes(), &sheet, "Dados", &s)).unwrap();
        assert_eq!(
            out,
            concat!(
                r#"<mergeCells count="1"><mergeCell ref="A5:B6"/></mergeCells>"#,
                r#"<conditionalFormatting sqref="B3:B7"><cfRule type="expression"><formula>Dados!$A$7&gt;0</formula></cfRule></conditionalFormatting>"#,
                r#"<hyperlinks><hyperlink ref="C8" r:id="rId1"/></hyperlinks></worksheet>"#,
            )
        );
        let names = br#"<definedNames><definedName name="_xlnm.Print_Area" localSheetId="0">Dados!$A$1:$C$10</definedName></definedNames>"#;
        let out = String::from_utf8(defined_names(names, &[("Dados", &s)])).unwrap();
        assert!(out.contains("Dados!$A$1:$C$8"), "{out}");
    }
}
