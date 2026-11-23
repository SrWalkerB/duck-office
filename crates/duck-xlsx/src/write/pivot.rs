//! Remendo das tabelas dinâmicas e das segmentações de dados ao salvar (F5):
//! itens escondidos, área e as linhas/colunas do resultado
//! (`rowItems`/`colItems`, no formato que o Excel grava), e a seleção de
//! cada segmentação. O resto das partes fica como está.

use std::collections::HashMap;

use duck_core::pivot::{AxisItem, PivotTable};
use memchr::memmem;

use super::cellxml::cell_ref;
use crate::xml::{find_tag, for_each_attr, tag_end};

/// Reescreve uma tag de abertura mudando/tirando atributos.
fn retag(tag_src: &[u8], set: &[(&[u8], Option<Vec<u8>>)]) -> Vec<u8> {
    // tag_src = "<nome ...>" ou "<nome .../>"
    let self_closing = tag_src.ends_with(b"/>");
    let name_end = tag_src.iter().position(|&b| b == b' ' || b == b'/' || b == b'>').unwrap_or(tag_src.len());
    let attrs_end = if self_closing { tag_src.len() - 2 } else { tag_src.len() - 1 };
    let attrs = &tag_src[name_end..attrs_end.max(name_end)];
    let mut out = tag_src[..name_end].to_vec();
    let mut done = vec![false; set.len()];
    for_each_attr(attrs, |k, whole| {
        if let Some(i) = set.iter().position(|(n, _)| *n == k) {
            done[i] = true;
            if let Some(v) = &set[i].1 {
                out.push(b' ');
                out.extend_from_slice(k);
                out.extend_from_slice(b"=\"");
                out.extend_from_slice(v);
                out.push(b'"');
            }
        } else {
            out.push(b' ');
            out.extend_from_slice(whole);
        }
    });
    for (i, (k, v)) in set.iter().enumerate() {
        if !done[i]
            && let Some(v) = v
        {
            out.push(b' ');
            out.extend_from_slice(k);
            out.extend_from_slice(b"=\"");
            out.extend_from_slice(v);
            out.push(b'"');
        }
    }
    out.extend_from_slice(if self_closing { b"/>" } else { b">" });
    out
}

fn axis_xml(tag: &str, items: &[AxisItem]) -> Vec<u8> {
    let mut s = format!("<{tag} count=\"{}\">", items.len());
    for it in items {
        s.push_str("<i");
        if let Some(t) = &it.t {
            s.push_str(&format!(" t=\"{t}\""));
        }
        if it.r > 0 {
            s.push_str(&format!(" r=\"{}\"", it.r));
        }
        if it.i > 0 {
            s.push_str(&format!(" i=\"{}\"", it.i));
        }
        if it.x.is_empty() {
            s.push_str("/>");
            continue;
        }
        s.push('>');
        for &x in &it.x {
            if x == 0 {
                s.push_str("<x/>");
            } else {
                s.push_str(&format!("<x v=\"{x}\"/>"));
            }
        }
        s.push_str("</i>");
    }
    s.push_str(&format!("</{tag}>"));
    s.into_bytes()
}

/// Troca o elemento `<name>…</name>` (ou `<name/>`) inteiro; se não existir,
/// insere antes de `before`.
fn replace_or_insert(xml: &[u8], name: &[u8], new: &[u8], before: &[&[u8]]) -> Vec<u8> {
    if let Some(t) = find_tag(xml, 0, name) {
        let end = if t.self_closing {
            t.end
        } else {
            let mut close = b"</".to_vec();
            close.extend_from_slice(name);
            close.push(b'>');
            memmem::find(&xml[t.end..], &close).map_or(xml.len(), |i| t.end + i + close.len())
        };
        return [&xml[..t.start], new, &xml[end..]].concat();
    }
    for b in before {
        if let Some(t) = find_tag(xml, 0, b) {
            return [&xml[..t.start], new, &xml[t.start..]].concat();
        }
    }
    xml.to_vec()
}

pub fn patch_pivot_table(xml: &[u8], t: &PivotTable) -> Vec<u8> {
    let mut out = xml.to_vec();
    // Área.
    if let Some(loc) = find_tag(&out, 0, b"location") {
        let mut r = Vec::new();
        cell_ref(t.range.first_row, t.range.first_col, &mut r);
        r.push(b':');
        cell_ref(t.range.last_row, t.range.last_col, &mut r);
        let new = retag(&out[loc.start..loc.end], &[(b"ref", Some(r))]);
        out = [&out[..loc.start], &new[..], &out[loc.end..]].concat();
    }
    // Itens escondidos, campo a campo.
    if let Some(pfs) = find_tag(&out, 0, b"pivotFields") {
        let mut pos = pfs.end;
        let mut fi = 0usize;
        let mut rebuilt: Vec<u8> = out[..pos].to_vec();
        while let Some(pf) = find_tag(&out, pos, b"pivotField") {
            rebuilt.extend_from_slice(&out[pos..pf.end]);
            pos = pf.end;
            if pf.self_closing {
                fi += 1;
                continue;
            }
            let close = memmem::find(&out[pf.end..], b"</pivotField>").map_or(out.len(), |i| pf.end + i);
            // Itens deste campo.
            let mut k = 0usize;
            let mut q = pos;
            while let Some(it) = find_tag(&out[..close], q, b"item") {
                rebuilt.extend_from_slice(&out[q..it.start]);
                let model = t.fields.get(fi).and_then(|f| f.items.get(k));
                let tag = &out[it.start..it.end];
                match model {
                    Some(m) if m.t.is_none() => {
                        let h: Option<Vec<u8>> = m.hidden.then(|| b"1".to_vec());
                        rebuilt.extend_from_slice(&retag(tag, &[(b"h", h)]));
                    }
                    _ => rebuilt.extend_from_slice(tag),
                }
                q = it.end;
                k += 1;
            }
            rebuilt.extend_from_slice(&out[q..close]);
            pos = close;
            fi += 1;
            if find_tag(&out, pos, b"pivotField").is_none_or(|n| {
                memmem::find(&out[pos..], b"</pivotFields>").is_some_and(|e| pos + e < n.start)
            }) {
                break;
            }
        }
        rebuilt.extend_from_slice(&out[pos..]);
        out = rebuilt;
    }
    // Linhas e colunas do resultado.
    out = replace_or_insert(
        &out,
        b"rowItems",
        &axis_xml("rowItems", &t.row_items),
        &[b"colFields", b"colItems", b"pageFields", b"dataFields"],
    );
    out = replace_or_insert(
        &out,
        b"colItems",
        &axis_xml("colItems", &t.col_items),
        &[b"pageFields", b"dataFields", b"formats"],
    );
    out
}

/// Seleção de uma segmentação: `s="1"` nos itens escolhidos (por índice no cache).
pub fn patch_slicer_cache(xml: &[u8], selected: &HashMap<u32, bool>) -> Vec<u8> {
    let Some(tab) = find_tag(xml, 0, b"tabular") else { return xml.to_vec() };
    let mut out = xml[..tab.end].to_vec();
    let mut pos = tab.end;
    while let Some(lt) = memchr::memmem::find(&xml[pos..], b"<i ").map(|i| pos + i) {
        let Some(end) = tag_end(xml, lt) else { break };
        out.extend_from_slice(&xml[pos..lt]);
        let tag = &xml[lt..end];
        let x = crate::xml::attr(&tag[2..], b"x").and_then(crate::xml::parse_u32).unwrap_or(0);
        let s = selected.get(&x).copied().unwrap_or(true);
        out.extend_from_slice(&retag(tag, &[(b"s", s.then(|| b"1".to_vec()))]));
        pos = end;
        if memmem::find(&xml[pos..], b"</items>")
            .is_some_and(|e| memmem::find(&xml[pos..], b"<i ").is_none_or(|n| e < n))
        {
            break;
        }
    }
    out.extend_from_slice(&xml[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicer_selection() {
        let xml = br#"<slicerCacheDefinition><data><tabular pivotCacheId="7"><items count="2"><i x="0" s="1"/><i x="1" s="1" nd="1"/></items></tabular></data></slicerCacheDefinition>"#;
        let sel: HashMap<u32, bool> = [(0, false), (1, true)].into_iter().collect();
        let out = String::from_utf8(patch_slicer_cache(xml, &sel)).unwrap();
        assert!(out.contains(r#"<i x="0"/>"#), "{out}");
        assert!(out.contains(r#"<i x="1" s="1" nd="1"/>"#), "{out}");
    }

    #[test]
    fn axis_items() {
        let items = vec![
            AxisItem { t: None, r: 0, i: 0, x: vec![0] },
            AxisItem { t: None, r: 1, i: 0, x: vec![3] },
            AxisItem { t: Some("grand".into()), r: 0, i: 0, x: vec![0] },
        ];
        assert_eq!(
            String::from_utf8(axis_xml("rowItems", &items)).unwrap(),
            r#"<rowItems count="3"><i><x/></i><i r="1"><x v="3"/></i><i t="grand"><x/></i></rowItems>"#
        );
    }
}
