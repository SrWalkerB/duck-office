//! Remendo dos desenhos ao inserir/apagar linhas e colunas (F5): as âncoras
//! dos objetos, as referências das séries dos gráficos e a fonte dos caches
//! de tabela dinâmica. O resto das partes fica como está.

use duck_core::CellRange;
use duck_core::drawing::{Anchor, Chart, Marker};
use memchr::memmem;

use super::cellxml::{cell_ref, escape_text};
use crate::xml::{find_tag, unescape_into};

fn marker_xml(m: &Marker) -> String {
    format!(
        "<xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff>",
        m.col, m.col_off, m.row, m.row_off
    )
}

/// Troca o conteúdo do primeiro `<name>…</name>` de `el`.
fn replace_inner(el: &[u8], name: &[u8], new: &str) -> Vec<u8> {
    let Some(t) = find_tag(el, 0, name) else { return el.to_vec() };
    if t.self_closing {
        return el.to_vec();
    }
    let mut close = b"</".to_vec();
    close.extend_from_slice(name);
    close.push(b'>');
    let Some(end) = memmem::find(&el[t.end..], &close).map(|i| t.end + i) else { return el.to_vec() };
    [&el[..t.end], new.as_bytes(), &el[end..]].concat()
}

/// Reescreve os cantos das âncoras de um desenho, na ordem do arquivo. Se o
/// número de âncoras não bater com o modelo, a parte fica como está.
pub fn patch_anchors(xml: &[u8], anchors: &[Anchor]) -> Vec<u8> {
    const NAMES: [&[u8]; 3] = [b"xdr:twoCellAnchor", b"xdr:oneCellAnchor", b"xdr:absoluteAnchor"];
    // Elementos das âncoras: (início, fim).
    let mut spans = Vec::new();
    let mut pos = 0;
    loop {
        let next =
            NAMES.iter().filter_map(|n| find_tag(xml, pos, n).map(|t| (t, *n))).min_by_key(|(t, _)| t.start);
        let Some((t, name)) = next else { break };
        let end = if t.self_closing {
            t.end
        } else {
            let mut close = b"</".to_vec();
            close.extend_from_slice(name);
            close.push(b'>');
            match memmem::find(&xml[t.end..], &close) {
                Some(i) => t.end + i + close.len(),
                None => return xml.to_vec(),
            }
        };
        spans.push((t.start, end));
        pos = end;
    }
    if spans.len() != anchors.len() {
        return xml.to_vec();
    }
    let mut out = Vec::with_capacity(xml.len());
    let mut last = 0;
    for ((a, b), anchor) in spans.into_iter().zip(anchors) {
        out.extend_from_slice(&xml[last..a]);
        let el = &xml[a..b];
        let el = match anchor {
            Anchor::Two { from, to } => {
                let e = replace_inner(el, b"xdr:from", &marker_xml(from));
                replace_inner(&e, b"xdr:to", &marker_xml(to))
            }
            Anchor::One { from, .. } => replace_inner(el, b"xdr:from", &marker_xml(from)),
            Anchor::Abs { .. } => el.to_vec(),
        };
        out.extend_from_slice(&el);
        last = b;
    }
    out.extend_from_slice(&xml[last..]);
    out
}

/// Referências das séries de um gráfico (`<c:f>`) mudadas no modelo.
/// `None` = nada mudou.
pub fn patch_chart_refs(xml: &[u8], model: &Chart) -> Option<Vec<u8>> {
    let mut orig = crate::drawing::parse_chart(xml)?;
    let mut model = model.clone();
    let old: Vec<String> = orig.refs_mut().map(|s| s.clone()).collect();
    let new: Vec<String> = model.refs_mut().map(|s| s.clone()).collect();
    if old.len() != new.len() || old == new {
        return None;
    }
    let map: std::collections::HashMap<&str, &str> =
        old.iter().zip(&new).filter(|(a, b)| a != b).map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let mut out = Vec::with_capacity(xml.len());
    let mut pos = 0;
    while let Some(t) = find_tag(xml, pos, b"c:f") {
        if t.self_closing {
            out.extend_from_slice(&xml[pos..t.end]);
            pos = t.end;
            continue;
        }
        let Some(end) = memmem::find(&xml[t.end..], b"</c:f>").map(|i| t.end + i) else { break };
        let mut text = String::new();
        unescape_into(&xml[t.end..end], &mut text);
        out.extend_from_slice(&xml[pos..t.end]);
        match map.get(text.as_str()) {
            Some(n) => escape_text(n, &mut out),
            None => out.extend_from_slice(&xml[t.end..end]),
        }
        pos = end;
    }
    out.extend_from_slice(&xml[pos..]);
    Some(out)
}

/// Intervalo da fonte de um cache de tabela dinâmica (`worksheetSource ref`).
pub fn patch_cache_source(xml: &[u8], r: CellRange) -> Vec<u8> {
    let Some(t) = find_tag(xml, 0, b"worksheetSource") else { return xml.to_vec() };
    let tag = &xml[t.start..t.end];
    let Some(i) = memmem::find(tag, b" ref=\"") else { return xml.to_vec() };
    let v0 = i + 6;
    let Some(v1) = memchr::memchr(b'"', &tag[v0..]).map(|k| v0 + k) else { return xml.to_vec() };
    let mut val = Vec::new();
    cell_ref(r.first_row, r.first_col, &mut val);
    if (r.first_row, r.first_col) != (r.last_row, r.last_col) {
        val.push(b':');
        cell_ref(r.last_row, r.last_col, &mut val);
    }
    [&xml[..t.start + v0], &val[..], &xml[t.start + v1..]].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors() {
        let xml = br#"<xdr:wsDr><xdr:twoCellAnchor editAs="oneCell"><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>4</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:sp/></xdr:twoCellAnchor><xdr:absoluteAnchor><xdr:pos x="0" y="0"/></xdr:absoluteAnchor></xdr:wsDr>"#;
        let m = |row| Marker { col: 1, col_off: 0, row, row_off: 0 };
        let a = [
            Anchor::Two { from: m(5), to: Marker { col: 3, col_off: 0, row: 7, row_off: 0 } },
            Anchor::Abs { x: 0, y: 0, cx: 0, cy: 0 },
        ];
        let out = String::from_utf8(patch_anchors(xml, &a)).unwrap();
        assert!(out.contains("<xdr:row>5</xdr:row>") && out.contains("<xdr:row>7</xdr:row>"), "{out}");
        assert!(out.contains("<xdr:sp/></xdr:twoCellAnchor><xdr:absoluteAnchor>"), "{out}");
        // Número diferente de âncoras: fica como está.
        assert_eq!(patch_anchors(xml, &a[..1]), xml.to_vec());
    }

    #[test]
    fn cache_source() {
        let xml =
            br#"<cacheSource type="worksheet"><worksheetSource ref="A1:F100" sheet="Dados"/></cacheSource>"#;
        let r = CellRange { first_row: 0, first_col: 0, last_row: 101, last_col: 5 };
        let out = String::from_utf8(patch_cache_source(xml, r)).unwrap();
        assert!(out.contains(r#"<worksheetSource ref="A1:F102" sheet="Dados"/>"#), "{out}");
    }
}
