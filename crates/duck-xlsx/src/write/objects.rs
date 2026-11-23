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

/// Propriedades de um controle com a marca nova. `None` = já estava assim.
pub fn patch_control(xml: &[u8], checked: bool) -> Option<Vec<u8>> {
    let t = find_tag(xml, 0, b"formControlPr")?;
    let had = crate::xml::attr(t.attrs, b"checked") == Some(b"Checked");
    if had == checked {
        return None;
    }
    let tag = &xml[t.start..t.end];
    let new = match memmem::find(tag, b" checked=\"") {
        Some(i) => {
            let end = i + 10 + memchr::memchr(b'"', &tag[i + 10..])? + 1;
            let mark: &[u8] = if checked { b" checked=\"Checked\"" } else { b"" };
            [&tag[..i], mark, &tag[end..]].concat()
        }
        None => {
            let at = memmem::find(tag, b"objectType=")? - 1;
            [&tag[..at], b" checked=\"Checked\"", &tag[at..]].concat()
        }
    };
    Some([&xml[..t.start], &new[..], &xml[t.end..]].concat())
}

/// Marcas dos controles no desenho VML (id da forma → marcado). `None` =
/// nada mudou.
pub fn patch_vml_checked(xml: &[u8], marks: &std::collections::HashMap<u32, bool>) -> Option<Vec<u8>> {
    const MARK: &[u8] = b"<x:Checked>1</x:Checked>";
    let mut out = Vec::with_capacity(xml.len() + 64);
    let mut pos = 0;
    let mut changed = false;
    while let Some(i) = memmem::find(&xml[pos..], b"_x0000_s").map(|i| pos + i) {
        let digits = xml[i + 8..].iter().take_while(|b| b.is_ascii_digit()).count();
        let id: u32 = std::str::from_utf8(&xml[i + 8..i + 8 + digits]).ok()?.parse().unwrap_or(0);
        out.extend_from_slice(&xml[pos..i + 8 + digits]);
        pos = i + 8 + digits;
        let Some(&want) = marks.get(&id) else { continue };
        // O `<x:ClientData>` desta forma (antes da próxima).
        let Some(end) = memmem::find(&xml[pos..], b"</x:ClientData>").map(|k| pos + k) else { break };
        if memmem::find(&xml[pos..end], b"</v:shape>").is_some() {
            continue;
        }
        let body = &xml[pos..end];
        let a = memmem::find(body, b"<x:Checked>");
        let had = a.is_some_and(|a| body[a + 11..].first() == Some(&b'1'));
        if had == want {
            continue;
        }
        changed = true;
        match a {
            Some(a) => {
                let z = a + memmem::find(&body[a..], b"</x:Checked>")? + 12;
                out.extend_from_slice(&body[..a]);
                if want {
                    out.extend_from_slice(MARK);
                }
                out.extend_from_slice(&body[z..]);
            }
            None => {
                // Na ordem do Excel: antes do que vem depois de `Checked`.
                let at = [&b"<x:FmlaLink>"[..], b"<x:NoThreeD", b"<x:FirstButton"]
                    .iter()
                    .filter_map(|t| memmem::find(body, t))
                    .min()
                    .unwrap_or(body.len());
                out.extend_from_slice(&body[..at]);
                out.extend_from_slice(MARK);
                out.extend_from_slice(&body[at..]);
            }
        }
        pos = end;
    }
    out.extend_from_slice(&xml[pos..]);
    changed.then_some(out)
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
    fn control_marks() {
        let on = br#"<?xml version="1.0"?><formControlPr xmlns="m" objectType="Radio" checked="Checked" firstButton="1"/>"#;
        let off = patch_control(on, false).unwrap();
        assert!(String::from_utf8_lossy(&off).contains(r#"objectType="Radio" firstButton="1"/>"#));
        assert_eq!(patch_control(on, true), None);
        let back = patch_control(&off, true).unwrap();
        assert!(String::from_utf8_lossy(&back).contains(r#"xmlns="m" checked="Checked" objectType"#));

        let vml = b"<v:shape id=\"_x0000_s10\" o:spid=\"x\"><x:ClientData ObjectType=\"Radio\"><x:TextVAlign>Center</x:TextVAlign><x:Checked>1</x:Checked><x:FmlaLink>$A$1</x:FmlaLink></x:ClientData></v:shape><v:shape id=\"_x0000_s11\"><x:ClientData ObjectType=\"Radio\"><x:NoThreeD/></x:ClientData></v:shape>";
        let marks = [(10, false), (11, true)].into_iter().collect();
        let out = String::from_utf8(patch_vml_checked(vml, &marks).unwrap()).unwrap();
        assert!(out.contains("</x:TextVAlign><x:FmlaLink>"), "{out}");
        assert!(out.contains("<x:Checked>1</x:Checked><x:NoThreeD/>"), "{out}");
        assert_eq!(patch_vml_checked(out.as_bytes(), &marks), None);
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
