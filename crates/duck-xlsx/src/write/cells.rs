//! Células movidas, inseridas ou excluídas (só células): o que está no XML da
//! aba depois de `</sheetData>` (formatação condicional, validação, links,
//! minigráficos) acompanha. As células saem do modelo, como qualquer edição.

use duck_core::CellRange;
use duck_core::cells::CellOp;
use memchr::memmem;

use super::cellxml::{cell_ref, escape_text};
use crate::xml::{for_each_attr, is_name_end, tag_end, unescape_into};

fn range_text(r: &CellRange) -> String {
    let mut v = Vec::new();
    cell_ref(r.first_row, r.first_col, &mut v);
    if (r.first_row, r.first_col) != (r.last_row, r.last_col) {
        v.push(b':');
        cell_ref(r.last_row, r.last_col, &mut v);
    }
    String::from_utf8(v).unwrap_or_default()
}

/// Uma lista de intervalos (`A1:B2 C5`) depois da operação. `None` = nada mudou.
fn sqref(v: &str, op: &CellOp) -> Option<String> {
    let mut out = Vec::new();
    let mut changed = false;
    for part in v.split_whitespace() {
        match crate::sheet::parse_range(part.replace('$', "").as_bytes()) {
            Some(r) => {
                let new = op.split(r);
                changed |= new != [r];
                out.extend(new.iter().map(range_text));
            }
            None => out.push(part.to_string()),
        }
    }
    changed.then(|| out.join(" "))
}

/// O fim da aba ajustado a uma operação de células.
pub(super) fn tail(xml: &[u8], op: &CellOp, sheet: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len() + 256);
    let mut pos = 0;
    while let Some(lt) = memchr::memchr(b'<', &xml[pos..]).map(|k| pos + k) {
        let Some(end) = tag_end(xml, lt) else { break };
        out.extend_from_slice(&xml[pos..lt]);
        pos = end;
        let n = xml[lt + 1..end].iter().position(|&c| is_name_end(c)).unwrap_or(0);
        let name = &xml[lt + 1..lt + 1 + n];
        let container = matches!(
            name,
            b"conditionalFormatting"
                | b"x14:conditionalFormatting"
                | b"dataValidation"
                | b"x14:dataValidation"
                | b"hyperlink"
                | b"ignoredError"
                | b"protectedRange"
        );
        if !container {
            out.extend_from_slice(&xml[lt..end]);
            continue;
        }
        let closed = xml[end - 2] == b'/';
        let close = [b"</", name, b">"].concat();
        let stop = if closed {
            end
        } else {
            memmem::find(&xml[end..], &close).map_or(xml.len(), |k| end + k + close.len())
        };
        let body = texts(&xml[end..stop], op, sheet);
        pos = stop;
        // Sem intervalo: a regra perdeu todas as células.
        let Some(tags) = opening(&xml[lt..end], name, op) else { continue };
        if name.starts_with(b"x14:")
            && memmem::find(&xml[end..stop], b"<xm:sqref>").is_some()
            && memmem::find(&body, b"<xm:sqref>").is_none()
        {
            continue;
        }
        for t in tags {
            out.extend_from_slice(&t);
            out.extend_from_slice(&body);
        }
    }
    out.extend_from_slice(&xml[pos..]);
    out
}

/// A tag de abertura com `sqref`/`ref` ajustados (um link cortado vira vários).
fn opening(tag: &[u8], name: &[u8], op: &CellOp) -> Option<Vec<Vec<u8>>> {
    let closed = tag.ends_with(b"/>");
    let attrs = &tag[1 + name.len()..tag.len() - if closed { 2 } else { 1 }];
    let mut value: Option<(Vec<u8>, String)> = None;
    for_each_attr(attrs, |k, whole| {
        if matches!(k, b"sqref" | b"ref") {
            let mut v = String::new();
            unescape_into(&whole[k.len() + 2..whole.len() - 1], &mut v);
            value = Some((k.to_vec(), v));
        }
    });
    let Some((key, v)) = value else { return Some(vec![tag.to_vec()]) };
    let Some(new) = sqref(&v, op) else { return Some(vec![tag.to_vec()]) };
    if new.is_empty() {
        return None;
    }
    // `ref` guarda um intervalo só.
    let values: Vec<&str> = if key == b"ref" { new.split(' ').collect() } else { vec![new.as_str()] };
    Some(
        values
            .into_iter()
            .map(|v| {
                let mut out = tag[..1 + name.len()].to_vec();
                for_each_attr(attrs, |k, whole| {
                    out.push(b' ');
                    if k == key.as_slice() {
                        out.extend_from_slice(k);
                        out.extend_from_slice(format!("=\"{v}\"").as_bytes());
                    } else {
                        out.extend_from_slice(whole);
                    }
                });
                out.extend_from_slice(if closed { b"/>" } else { b">" });
                out
            })
            .collect(),
    )
}

/// Fórmulas e `<xm:sqref>` dentro de um elemento.
fn texts(body: &[u8], op: &CellOp, sheet: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut pos = 0;
    while let Some(lt) = memchr::memchr(b'<', &body[pos..]).map(|k| pos + k) {
        let Some(end) = tag_end(body, lt) else { break };
        let n = body[lt + 1..end].iter().position(|&c| is_name_end(c)).unwrap_or(0);
        let name = &body[lt + 1..lt + 1 + n];
        out.extend_from_slice(&body[pos..lt]);
        let formula = matches!(name, b"formula" | b"formula1" | b"formula2" | b"xm:f");
        let list = name == b"xm:sqref";
        pos = end;
        if !(formula || list) || body[end - 2] == b'/' {
            out.extend_from_slice(&body[lt..end]);
            continue;
        }
        let close = [b"</", name, b">"].concat();
        let Some(stop) = memmem::find(&body[end..], &close).map(|k| end + k) else { break };
        let mut text = String::new();
        unescape_into(&body[end..stop], &mut text);
        let new = if formula { duck_calc::cellops::adjust(&text, op, true, sheet) } else { sqref(&text, op) };
        pos = stop + close.len();
        match new {
            Some(t) if list && t.is_empty() => {}
            Some(t) => {
                out.extend_from_slice(&body[lt..end]);
                escape_text(&t, &mut out);
                out.extend_from_slice(&close);
            }
            None => out.extend_from_slice(&body[lt..pos]),
        }
    }
    out.extend_from_slice(&body[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{Axis, Shift};

    #[test]
    fn rules_and_links_follow() {
        let xml = concat!(
            r#"<conditionalFormatting sqref="A1:A4"><cfRule type="expression"><formula>A1&gt;$B$2</formula></cfRule></conditionalFormatting>"#,
            r#"<dataValidations count="1"><dataValidation sqref="B2"><formula1>$B$2:$B$3</formula1></dataValidation></dataValidations>"#,
            r#"<hyperlinks><hyperlink ref="A2" r:id="rId1"/></hyperlinks></worksheet>"#,
        );
        // A2:B3 vai para D6:E7.
        let src = CellRange { first_row: 1, first_col: 0, last_row: 2, last_col: 1 };
        let op = CellOp::Move { src, dr: 4, dc: 3 };
        let out = String::from_utf8(tail(xml.as_bytes(), &op, "Plan")).unwrap();
        assert_eq!(
            out,
            concat!(
                r#"<conditionalFormatting sqref="A1 A4 D6:D7"><cfRule type="expression"><formula>A1&gt;$E$6</formula></cfRule></conditionalFormatting>"#,
                r#"<dataValidations count="1"><dataValidation sqref="E6"><formula1>$E$6:$E$7</formula1></dataValidation></dataValidations>"#,
                r#"<hyperlinks><hyperlink ref="D6" r:id="rId1"/></hyperlinks></worksheet>"#,
            )
        );
        // Exclui A1:A4: a regra some.
        let s = Shift { axis: Axis::Rows, at: 0, n: 4, insert: false };
        let op = CellOp::Shift { shift: s, lo: 0, hi: 0 };
        let out = String::from_utf8(tail(xml.as_bytes(), &op, "Plan")).unwrap();
        assert!(!out.contains("conditionalFormatting"), "{out}");
        assert!(out.contains(r#"sqref="B2""#), "{out}");
        assert!(!out.contains("<hyperlink "), "{out}");
    }
}
