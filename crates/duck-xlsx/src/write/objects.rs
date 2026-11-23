//! Remendo dos desenhos ao inserir/apagar linhas e colunas (F5): as âncoras
//! dos objetos, as referências das séries dos gráficos e a fonte dos caches
//! de tabela dinâmica. O resto das partes fica como está.

use duck_core::CellRange;
use duck_core::drawing::{Anchor, Chart, Marker};
use memchr::memmem;

use super::cellxml::{cell_ref, escape_text};
use crate::ns::{Kind, Scan};
use crate::xml::{find_tag, unescape_into};

/// Conteúdo de um canto, com o prefixo que o arquivo usa.
fn marker_xml(prefix: &[u8], m: &Marker) -> String {
    let p = match prefix {
        b"" => String::new(),
        p => format!("{}:", String::from_utf8_lossy(p)),
    };
    format!(
        "<{p}col>{}</{p}col><{p}colOff>{}</{p}colOff><{p}row>{}</{p}row><{p}rowOff>{}</{p}rowOff>",
        m.col, m.col_off, m.row, m.row_off
    )
}

/// Cantos de uma âncora do arquivo: conteúdo de `from` e `to` (início, fim) e
/// o prefixo do elemento.
#[derive(Default)]
struct Corners<'a> {
    from: Option<(usize, usize, &'a [u8])>,
    to: Option<(usize, usize, &'a [u8])>,
}

/// As âncoras de um desenho, na ordem do arquivo. `None` = XML quebrado.
fn anchor_corners(xml: &[u8]) -> Option<Vec<Corners<'_>>> {
    let mut out: Vec<Corners> = Vec::new();
    // Elementos abertos a partir da âncora (0 = fora de uma).
    let mut depth = 0;
    for e in Scan::new(xml) {
        if depth == 0 {
            let anchor = e.ns == b"xdr"
                && matches!(e.local, b"twoCellAnchor" | b"oneCellAnchor" | b"absoluteAnchor")
                && e.kind != Kind::Close;
            if anchor {
                out.push(Corners::default());
                depth = (e.kind == Kind::Open) as u32;
            }
            continue;
        }
        let cur = out.last_mut()?;
        // `from` e `to` são filhos diretos da âncora.
        let corner = match e.local {
            b"from" if e.ns == b"xdr" => Some(&mut cur.from),
            b"to" if e.ns == b"xdr" => Some(&mut cur.to),
            _ => None,
        };
        match e.kind {
            Kind::Open => {
                if depth == 1
                    && let Some(c) = corner
                {
                    *c = Some((e.end, e.end, e.prefix));
                }
                depth += 1;
            }
            Kind::Close => {
                depth -= 1;
                if depth == 1
                    && let Some(Some(c)) = corner
                {
                    c.1 = e.start;
                }
            }
            Kind::Empty => {}
        }
    }
    (depth == 0).then_some(out)
}

/// Reescreve os cantos das âncoras de um desenho, na ordem do arquivo. Se o
/// número de âncoras não bater com o modelo, a parte fica como está.
pub fn patch_anchors(xml: &[u8], anchors: &[Anchor]) -> Vec<u8> {
    let Some(corners) = anchor_corners(xml).filter(|c| c.len() == anchors.len()) else {
        return xml.to_vec();
    };
    // (início, fim, novo conteúdo).
    let mut edits = Vec::new();
    for (c, anchor) in corners.iter().zip(anchors) {
        let (from, to) = match anchor {
            Anchor::Two { from, to } => (Some(from), Some(to)),
            Anchor::One { from, .. } => (Some(from), None),
            Anchor::Abs { .. } => (None, None),
        };
        for (at, m) in [(c.from, from), (c.to, to)] {
            if let (Some((a, b, prefix)), Some(m)) = (at, m) {
                edits.push((a, b, marker_xml(prefix, m)));
            }
        }
    }
    edits.sort_unstable_by_key(|e| e.0);
    let mut out = Vec::with_capacity(xml.len() + 64);
    let mut last = 0;
    for (a, b, new) in edits {
        out.extend_from_slice(&xml[last..a]);
        out.extend_from_slice(new.as_bytes());
        last = b;
    }
    out.extend_from_slice(&xml[last..]);
    out
}

/// Referências das séries de um gráfico (`c:f`) mudadas no modelo.
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
    let mut open = None;
    for e in Scan::new(xml) {
        // `c:f` só tem texto: a tag seguinte é a que o fecha.
        if e.kind == Kind::Open && e.is(b"c", b"f") {
            open = Some(e.end);
            continue;
        }
        let Some(start) = open.take() else { continue };
        let mut text = String::new();
        unescape_into(&xml[start..e.start], &mut text);
        if let Some(n) = map.get(text.as_str()) {
            out.extend_from_slice(&xml[pos..start]);
            escape_text(n, &mut out);
            pos = e.start;
        }
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

    /// O prefixo do arquivo fica: padrão (openpyxl) ou inventado.
    #[test]
    fn anchors_keep_the_prefix() {
        const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
        let m = |row| Marker { col: 4, col_off: 0, row, row_off: 0 };
        for (decl, p) in [("xmlns", ""), ("xmlns:q", "q:")] {
            let xml = format!(
                r#"<{p}wsDr {decl}="{XDR}"><{p}oneCellAnchor><{p}from><{p}col>4</{p}col><{p}colOff>0</{p}colOff><{p}row>7</{p}row><{p}rowOff>0</{p}rowOff></{p}from><{p}ext cx="9" cy="9" /><{p}sp><from xmlns="urn:x"/></{p}sp></{p}oneCellAnchor><{p}twoCellAnchor><{p}from /><{p}to><{p}row>9</{p}row></{p}to></{p}twoCellAnchor></{p}wsDr>"#
            );
            let a = [Anchor::One { from: m(10), cx: 9, cy: 9 }, Anchor::Two { from: m(1), to: m(12) }];
            let out = String::from_utf8(patch_anchors(xml.as_bytes(), &a)).unwrap();
            let to = format!(
                "<{p}to><{p}col>4</{p}col><{p}colOff>0</{p}colOff><{p}row>12</{p}row><{p}rowOff>0</{p}rowOff></{p}to>"
            );
            let want = xml
                .replace(&format!("<{p}row>7<"), &format!("<{p}row>10<"))
                .replace(&format!("<{p}to><{p}row>9</{p}row></{p}to>"), &to);
            assert_eq!(out, want);
            // XML cortado: fica como está.
            let cut = &xml.as_bytes()[..xml.len() - 40];
            assert_eq!(patch_anchors(cut, &a), cut);
        }
    }

    #[test]
    fn chart_refs_keep_the_prefix() {
        const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
        for (decl, p) in [("xmlns", ""), ("xmlns:c", "c:"), ("xmlns:q", "q:")] {
            let xml = format!(
                r#"<{p}chartSpace {decl}="{C}" xmlns:c15="urn:c15"><{p}chart><{p}plotArea><{p}barChart><{p}ser><{p}tx><{p}strRef><{p}f>'D &amp; E'!$B$1</{p}f></{p}strRef></{p}tx><{p}val><{p}numRef><{p}f>'D &amp; E'!$B$2:$B$5</{p}f></{p}numRef></{p}val><c15:f>'D &amp; E'!$B$2:$B$5</c15:f></{p}ser></{p}barChart></{p}plotArea></{p}chart></{p}chartSpace>"#
            );
            let mut chart = crate::drawing::parse_chart(xml.as_bytes()).unwrap();
            assert_eq!(patch_chart_refs(xml.as_bytes(), &chart), None);
            for r in chart.refs_mut() {
                *r = r.replace("$B$5", "$B$8");
            }
            let out = String::from_utf8(patch_chart_refs(xml.as_bytes(), &chart).unwrap()).unwrap();
            assert_eq!(out, xml.replace(&format!("$B$5</{p}f>"), &format!("$B$8</{p}f>")));
        }
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
