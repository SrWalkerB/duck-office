//! Estilos criados na edição acrescentados ao `styles.xml` (F3).
//!
//! Cada estilo novo é uma cópia do `<xf>` do arquivo de que derivou
//! (`Styles::xf_base`) com só o que mudou trocado: formato de número,
//! fonte, preenchimento, borda e alinhamento. Fontes, preenchimentos e
//! bordas novos entram no fim das listas deles. O resto do arquivo fica
//! como está.

use duck_core::{Alignment, Border, BorderStyle, CellXf, Color, Fill, Font, HAlign, Styles, VAlign};

use super::cellxml::{push_number, push_u32};
use crate::xml::{Tag, find_tag, for_each_attr};

/// `styles.xml` com os estilos novos de `st`. Sem estilos novos, devolve igual.
pub fn patch_styles(xml: &[u8], st: &Styles) -> Vec<u8> {
    let fc = st.file_counts;
    let mut inserts: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut counts: Vec<(Tag, usize)> = Vec::new();

    let mut list = |name: &[u8], new: Vec<Vec<u8>>, total: usize| {
        if new.is_empty() {
            return;
        }
        if let Some(tag) = find_tag(xml, 0, name) {
            let close = close_of(xml, &tag, name);
            inserts.push((close, new.concat()));
            counts.push((tag, total));
        }
    };
    list(b"fonts", st.fonts[fc.fonts.min(st.fonts.len())..].iter().map(font_xml).collect(), st.fonts.len());
    list(b"fills", st.fills[fc.fills.min(st.fills.len())..].iter().map(fill_xml).collect(), st.fills.len());
    list(
        b"borders",
        st.borders[fc.borders.min(st.borders.len())..].iter().map(border_xml).collect(),
        st.borders.len(),
    );
    if st.xfs.len() > fc.xfs
        && let Some(tag) = find_tag(xml, 0, b"cellXfs")
    {
        let close = close_of(xml, &tag, b"cellXfs");
        let originals = xf_elements(&xml[tag.end..close], tag.end);
        let mut new = Vec::new();
        for (k, xf) in st.xfs[fc.xfs..].iter().enumerate() {
            let base = st.xf_base.get(k).copied().unwrap_or(0) as usize;
            let base_xf = st.xfs.get(base).copied().unwrap_or_default();
            let element = originals.get(base).map(|&(a, b)| &xml[a..b]);
            new.push(xf_xml(element, &base_xf, xf));
        }
        inserts.push((close, new.concat()));
        counts.push((tag, st.xfs.len()));
    }
    // Formatos de número novos: no fim de `<numFmts>` (ou numa lista nova,
    // que é o primeiro filho de `<styleSheet>`).
    if !st.new_num_fmts.is_empty() {
        let mut items = Vec::new();
        for id in &st.new_num_fmts {
            if let Some(code) = st.num_fmts.get(id) {
                items.extend_from_slice(b"<numFmt numFmtId=\"");
                push_u32(&mut items, *id as u32);
                items.extend_from_slice(b"\" formatCode=\"");
                super::cellxml::escape_attr(code, &mut items);
                items.extend_from_slice(b"\"/>");
            }
        }
        match find_tag(xml, 0, b"numFmts") {
            Some(tag) if !tag.self_closing => {
                let close = close_of(xml, &tag, b"numFmts");
                let before = find_all(&xml[tag.end..close], b"numFmt");
                inserts.push((close, items));
                counts.push((tag, before + st.new_num_fmts.len()));
            }
            _ => {
                if let Some(root) = find_tag(xml, 0, b"styleSheet") {
                    let mut list = b"<numFmts count=\"".to_vec();
                    push_u32(&mut list, st.new_num_fmts.len() as u32);
                    list.extend_from_slice(b"\">");
                    list.extend_from_slice(&items);
                    list.extend_from_slice(b"</numFmts>");
                    inserts.push((root.end, list));
                }
            }
        }
    }
    if inserts.is_empty() {
        return xml.to_vec();
    }

    // Aplica as inserções e os `count=` em ordem de posição.
    let mut edits: Vec<(usize, usize, Vec<u8>)> = inserts.into_iter().map(|(at, b)| (at, at, b)).collect();
    for (tag, total) in counts {
        let mut t = Vec::new();
        t.push(b'<');
        let name_end = tag.start
            + 1
            + xml[tag.start + 1..].iter().position(|&c| crate::xml::is_name_end(c)).unwrap_or(0);
        t.extend_from_slice(&xml[tag.start + 1..name_end]);
        let mut seen = false;
        for_each_attr(tag.attrs, |k, whole| {
            t.push(b' ');
            if k == b"count" {
                seen = true;
                t.extend_from_slice(b"count=\"");
                push_u32(&mut t, total as u32);
                t.push(b'"');
            } else {
                t.extend_from_slice(whole);
            }
        });
        if !seen {
            t.extend_from_slice(b" count=\"");
            push_u32(&mut t, total as u32);
            t.push(b'"');
        }
        t.push(b'>');
        edits.push((tag.start, tag.end, t));
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = Vec::with_capacity(xml.len() + 4096);
    let mut pos = 0;
    for (a, b, bytes) in edits {
        out.extend_from_slice(&xml[pos..a]);
        out.extend_from_slice(&bytes);
        pos = b;
    }
    out.extend_from_slice(&xml[pos..]);
    out
}

/// Quantos elementos `<name>` há em `buf`.
fn find_all(buf: &[u8], name: &[u8]) -> usize {
    let mut n = 0;
    let mut pos = 0;
    while let Some(t) = find_tag(buf, pos, name) {
        n += 1;
        pos = t.end;
    }
    n
}

/// Posição do `</name>` que fecha a lista aberta em `tag`.
fn close_of(xml: &[u8], tag: &Tag, name: &[u8]) -> usize {
    tag.end + crate::xml::inner(xml, tag, name).len()
}

/// Posições `[início, fim)` de cada `<xf>` de `cellXfs`.
fn xf_elements(inner: &[u8], base: usize) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut pos = 0;
    while let Some(t) = find_tag(inner, pos, b"xf") {
        let end = if t.self_closing {
            t.end
        } else {
            memchr::memmem::find(&inner[t.end..], b"</xf>").map_or(inner.len(), |x| t.end + x + 5)
        };
        v.push((base + t.start, base + end));
        pos = end;
    }
    v
}

/// O `<xf>` de `xf`, a partir do `<xf>` original (`element`) do estilo base.
fn xf_xml(element: Option<&[u8]>, base: &CellXf, xf: &CellXf) -> Vec<u8> {
    let mut out = b"<xf".to_vec();
    let ids: [(&[u8], u16, &[u8], bool); 4] = [
        (b"numFmtId", xf.num_fmt, b"applyNumberFormat", xf.num_fmt != base.num_fmt),
        (b"fontId", xf.font, b"applyFont", xf.font != base.font),
        (b"fillId", xf.fill, b"applyFill", xf.fill != base.fill),
        (b"borderId", xf.border, b"applyBorder", xf.border != base.border),
    ];
    let align_changed = xf.alignment != base.alignment;
    let (attrs, children) = match element {
        Some(e) => {
            let gt = crate::xml::tag_end(e, 0).unwrap_or(e.len());
            let self_closing = e[gt - 2] == b'/';
            let attrs = &e[3..if self_closing { gt - 2 } else { gt - 1 }];
            let children = if self_closing { &e[gt..gt] } else { &e[gt..e.len() - 5] };
            (attrs, children)
        }
        None => (&b""[..], &b""[..]),
    };
    let skip: Vec<&[u8]> = ids
        .iter()
        .flat_map(|(k, _, apply, changed)| if *changed { vec![*k, *apply] } else { vec![*k] })
        .chain(align_changed.then_some(&b"applyAlignment"[..]))
        .collect();
    for (k, v, _, _) in ids {
        out.push(b' ');
        out.extend_from_slice(k);
        out.extend_from_slice(b"=\"");
        push_u32(&mut out, v as u32);
        out.push(b'"');
    }
    for_each_attr(attrs, |k, whole| {
        if !skip.contains(&k) {
            out.push(b' ');
            out.extend_from_slice(whole);
        }
    });
    for (_, _, apply, changed) in ids {
        if changed {
            out.push(b' ');
            out.extend_from_slice(apply);
            out.extend_from_slice(b"=\"1\"");
        }
    }
    if align_changed {
        out.extend_from_slice(b" applyAlignment=\"1\"");
    }
    // Filhos: `<alignment>` trocado se mudou; `<protection>` e o resto como estão.
    let mut kids = Vec::new();
    if align_changed {
        alignment_xml(&xf.alignment, &mut kids);
        let mut rest = children.to_vec();
        if let Some(t) = find_tag(children, 0, b"alignment") {
            let end = if t.self_closing {
                t.end
            } else {
                memchr::memmem::find(&children[t.end..], b"</alignment>").map_or(t.end, |x| t.end + x + 12)
            };
            rest = [&children[..t.start], &children[end..]].concat();
        }
        kids.extend_from_slice(&rest);
    } else {
        kids.extend_from_slice(children);
    }
    if kids.is_empty() {
        out.extend_from_slice(b"/>");
    } else {
        out.push(b'>');
        out.extend_from_slice(&kids);
        out.extend_from_slice(b"</xf>");
    }
    out
}

fn alignment_xml(a: &Alignment, out: &mut Vec<u8>) {
    if *a == Alignment::default() {
        return;
    }
    out.extend_from_slice(b"<alignment");
    let h: Option<&[u8]> = match a.horizontal {
        HAlign::General => None,
        HAlign::Left => Some(b"left"),
        HAlign::Center => Some(b"center"),
        HAlign::Right => Some(b"right"),
        HAlign::Fill => Some(b"fill"),
        HAlign::Justify => Some(b"justify"),
        HAlign::CenterContinuous => Some(b"centerContinuous"),
        HAlign::Distributed => Some(b"distributed"),
    };
    if let Some(h) = h {
        out.extend_from_slice(b" horizontal=\"");
        out.extend_from_slice(h);
        out.push(b'"');
    }
    let v: Option<&[u8]> = match a.vertical {
        VAlign::Bottom => None,
        VAlign::Top => Some(b"top"),
        VAlign::Center => Some(b"center"),
        VAlign::Justify => Some(b"justify"),
        VAlign::Distributed => Some(b"distributed"),
    };
    if let Some(v) = v {
        out.extend_from_slice(b" vertical=\"");
        out.extend_from_slice(v);
        out.push(b'"');
    }
    if a.rotation != 0 {
        out.extend_from_slice(b" textRotation=\"");
        push_u32(out, a.rotation as u32);
        out.push(b'"');
    }
    if a.wrap {
        out.extend_from_slice(b" wrapText=\"1\"");
    }
    if a.indent != 0 {
        out.extend_from_slice(b" indent=\"");
        push_u32(out, a.indent as u32);
        out.push(b'"');
    }
    out.extend_from_slice(b"/>");
}

fn color_xml(tag: &[u8], c: Color, out: &mut Vec<u8>) {
    let start = out.len();
    out.push(b'<');
    out.extend_from_slice(tag);
    match c {
        Color::Auto => {
            out.truncate(start);
            return;
        }
        Color::Rgb(argb) => {
            out.extend_from_slice(format!(" rgb=\"{argb:08X}\"").as_bytes());
        }
        Color::Theme { index, tint } => {
            out.extend_from_slice(b" theme=\"");
            push_u32(out, index as u32);
            out.push(b'"');
            if tint != 0.0 {
                out.extend_from_slice(b" tint=\"");
                push_number(out, tint as f64);
                out.push(b'"');
            }
        }
        Color::Indexed(i) => {
            out.extend_from_slice(b" indexed=\"");
            push_u32(out, i as u32);
            out.push(b'"');
        }
    }
    out.extend_from_slice(b"/>");
}

fn font_xml(f: &Font) -> Vec<u8> {
    let mut out = b"<font>".to_vec();
    if f.bold {
        out.extend_from_slice(b"<b/>");
    }
    if f.italic {
        out.extend_from_slice(b"<i/>");
    }
    if f.strike {
        out.extend_from_slice(b"<strike/>");
    }
    if f.underline {
        out.extend_from_slice(b"<u/>");
    }
    out.extend_from_slice(b"<sz val=\"");
    push_number(&mut out, f.size as f64);
    out.extend_from_slice(b"\"/>");
    color_xml(b"color", f.color, &mut out);
    out.extend_from_slice(b"<name val=\"");
    super::cellxml::escape_attr(&f.name, &mut out);
    out.extend_from_slice(b"\"/></font>");
    out
}

fn fill_xml(f: &Fill) -> Vec<u8> {
    let mut out = b"<fill><patternFill".to_vec();
    match &f.pattern {
        None => out.extend_from_slice(b" patternType=\"none\"/></fill>"),
        Some(p) => {
            out.extend_from_slice(b" patternType=\"");
            super::cellxml::escape_attr(p, &mut out);
            out.extend_from_slice(b"\">");
            color_xml(b"fgColor", f.fg, &mut out);
            color_xml(b"bgColor", f.bg, &mut out);
            out.extend_from_slice(b"</patternFill></fill>");
        }
    }
    out
}

fn border_xml(b: &Border) -> Vec<u8> {
    let mut out = b"<border>".to_vec();
    for (name, side) in
        [(&b"left"[..], &b.left), (b"right", &b.right), (b"top", &b.top), (b"bottom", &b.bottom)]
    {
        out.push(b'<');
        out.extend_from_slice(name);
        match Some(side) {
            Some(s) if s.style != BorderStyle::None => {
                out.extend_from_slice(b" style=\"");
                out.extend_from_slice(s.style.name().as_bytes());
                out.extend_from_slice(b"\">");
                color_xml(b"color", s.color, &mut out);
                out.extend_from_slice(b"</");
                out.extend_from_slice(name);
                out.push(b'>');
            }
            _ => out.extend_from_slice(b"/>"),
        }
    }
    out.extend_from_slice(b"<diagonal/></border>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = concat!(
        r#"<styleSheet><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>"#,
        r#"<cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>"#,
        r#"<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0" applyAlignment="1"><alignment horizontal="center"/><protection locked="0"/></xf></cellXfs>"#,
        r#"<cellStyles count="1"/></styleSheet>"#,
    );

    #[test]
    fn new_xfs_copy_their_base() {
        let mut st = crate::parts::parse_styles(XML.as_bytes());
        assert_eq!(st.file_counts.xfs, 2);
        let a = st.derive_xf(1, |x| x.num_fmt = 14);
        let b = st.derive_xf(0, |x| x.alignment.wrap = true);
        assert_eq!((a, b), (2, 3));
        assert_eq!(st.derive_xf(1, |x| x.num_fmt = 14), 2, "igual a um existente");
        let mut f = st.fonts[0].clone();
        f.bold = true;
        st.fonts.push(f);
        let out = String::from_utf8(patch_styles(XML.as_bytes(), &st)).unwrap();
        assert!(out.contains(r#"<fonts count="2">"#), "{out}");
        assert!(out.contains(r#"<cellXfs count="4">"#), "{out}");
        assert!(out.contains(
            r#"<xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyAlignment="1" applyNumberFormat="1"><alignment horizontal="center"/><protection locked="0"/></xf>"#
        ), "{out}");
        assert!(out.contains(r#"<alignment wrapText="1"/></xf></cellXfs>"#), "{out}");
        let back = crate::parts::parse_styles(out.as_bytes());
        assert_eq!(back.xfs, st.xfs);
        assert_eq!(back.fonts, st.fonts);
        // Formato de número novo: lista criada no começo do styleSheet.
        let id = st.intern_num_fmt("0.000");
        assert_eq!(id, 164);
        assert_eq!(st.intern_num_fmt("0.00"), 2, "interno");
        let out = patch_styles(XML.as_bytes(), &st);
        let back = crate::parts::parse_styles(&out);
        assert_eq!(back.num_fmts.get(&164).map(String::as_str), Some("0.000"));
    }
}
