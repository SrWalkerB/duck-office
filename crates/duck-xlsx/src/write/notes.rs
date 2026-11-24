//! Gravação das notas de uma aba: a lista (`comments`), as caixas (VML) e as
//! conversas. As notas que não mudaram voltam com o XML de origem; formas do
//! VML que não são notas (controles) ficam como estão. Uma aba sem notas
//! ganha as partes, os relacionamentos e os tipos de conteúdo.

use duck_core::{Note, Sheet};
use memchr::memmem;

use super::cellxml::{cell_ref, escape_text};
use crate::Error;
use crate::notes::{authors, note_parts, note_shape_spans, vml_find};
use crate::xml::{find_tag, for_each_attr, is_name_end, tag_end};

/// Relacionamentos criados pelo duck-sheets.
const RID_VML: &str = "rIdDuckNotes";
const RID_COMMENTS: &str = "rIdDuckComments";

const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// O que gravar para as notas de uma aba.
#[derive(Default)]
pub struct Output {
    /// Partes novas ou regravadas.
    pub parts: Vec<(String, Vec<u8>)>,
    /// Os relacionamentos da aba, se ganharam algum.
    pub rels: Option<Vec<u8>>,
    /// Elementos novos do `[Content_Types].xml`.
    pub types: Vec<(String, String)>,
}

/// O `r:id` do `<legacyDrawing>` da aba (o que ela tem, ou o que ganha).
pub fn vml_rid(sheet: &Sheet) -> &str {
    sheet.layout.legacy_drawing.as_deref().unwrap_or(RID_VML)
}

/// `read` dá a parte como está sendo gravada (outro remendo pode tê-la
/// mudado); `taken` diz se um caminho já existe ou foi criado agora.
pub fn build(
    sheet_part: &str,
    sheet: &Sheet,
    read: &dyn Fn(&str) -> Result<Vec<u8>, Error>,
    taken: &dyn Fn(&str) -> bool,
) -> Result<Output, Error> {
    let mut out = Output::default();
    // A aba pode não ter relacionamentos.
    let old_rels = read(&super::rels_path(sheet_part)).ok();
    let p = note_parts(old_rels.as_deref().unwrap_or_default(), sheet_part);
    let notes = &sheet.layout.notes;
    if notes.is_empty() && p.comments.is_none() && p.threads.is_none() {
        return Ok(out);
    }
    let mut rels: Vec<String> = Vec::new();
    let free = |pattern: &dyn Fn(u32) -> String| (1..).map(pattern).find(|n| !taken(n)).unwrap_or_default();

    // A lista de notas.
    match &p.comments {
        Some(part) => {
            let xml = plain(&read(part)?);
            out.parts.push((part.clone(), comments(Some(&xml), notes)?));
        }
        None => {
            let part = free(&|n| format!("xl/comments{n}.xml"));
            out.parts.push((part.clone(), comments(None, notes)?));
            rels.push(format!(
                r#"<Relationship Id="{RID_COMMENTS}" Type="{REL}/comments" Target="/{part}"/>"#
            ));
            out.types.push((
                format!("PartName=\"/{part}\""),
                format!(
                    r#"<Override PartName="/{part}" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml"/>"#
                ),
            ));
        }
    }

    // As caixas. O `r:id` do `<legacyDrawing>` diz qual dos VML é o da aba (o
    // outro, se houver, é do cabeçalho da página).
    let legacy = sheet.layout.legacy_drawing.as_deref();
    let vml = p.vml.iter().find(|v| Some(v.0.as_str()) == legacy || v.0 == RID_VML);
    match vml {
        Some((_, part)) => {
            let xml = read(part)?;
            out.parts.push((part.clone(), shapes(Some(&xml), notes, sheet.tab_id)?));
        }
        // Sem notas, sem caixas: nada a criar.
        None if notes.is_empty() => {}
        None => {
            let part = free(&|n| format!("xl/drawings/vmlDrawing{n}.vml"));
            let rid = vml_rid(sheet);
            out.parts.push((part.clone(), shapes(None, notes, sheet.tab_id)?));
            rels.push(format!(r#"<Relationship Id="{rid}" Type="{REL}/vmlDrawing" Target="/{part}"/>"#));
            out.types.push((
                "Extension=\"vml\"".into(),
                r#"<Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/>"#
                    .into(),
            ));
        }
    }

    // As conversas (só mudam de célula ou somem com ela).
    if let Some(part) = &p.threads {
        let xml = plain(&read(part)?);
        out.parts.push((part.clone(), threads(&xml, notes)?));
    }
    if !rels.is_empty() {
        out.rels = Some(add_rels(old_rels.as_deref(), &rels));
    }
    Ok(out)
}

/// O XML com o espaço de nomes da raiz como padrão (`<x:comments>` →
/// `<comments>`), como o leitor espera.
pub(crate) fn plain(xml: &[u8]) -> Vec<u8> {
    let mut pos = 0;
    let root = loop {
        let Some(i) = memchr::memchr(b'<', &xml[pos..]).map(|k| pos + k) else { return xml.to_vec() };
        if !matches!(xml.get(i + 1), Some(b'?' | b'!')) {
            break i + 1;
        }
        pos = i + 1;
    };
    let end = xml[root..].iter().position(|&b| crate::xml::is_name_end(b)).map_or(root, |k| root + k);
    let Some(colon) = xml[root..end].iter().position(|&b| b == b':') else { return xml.to_vec() };
    let prefix = &xml[root..root + colon];
    let mut out = Vec::with_capacity(xml.len());
    let mut i = 0;
    while i < xml.len() {
        let rest = &xml[i..];
        let cut = |lead: &[u8]| {
            rest.starts_with(lead)
                && rest[lead.len()..].starts_with(prefix)
                && rest.get(lead.len() + prefix.len()) == Some(&b':')
        };
        if cut(b"<") || cut(b"</") {
            let lead = if rest[1] == b'/' { 2 } else { 1 };
            out.extend_from_slice(&rest[..lead]);
            i += lead + prefix.len() + 1;
            continue;
        }
        if rest.starts_with(b"xmlns:")
            && rest[6..].starts_with(prefix)
            && rest.get(6 + prefix.len()) == Some(&b'=')
        {
            out.extend_from_slice(b"xmlns");
            i += 6 + prefix.len();
            continue;
        }
        out.push(xml[i]);
        i += 1;
    }
    out
}

fn a1(n: &Note) -> String {
    let mut v = Vec::new();
    cell_ref(n.row, n.col, &mut v);
    String::from_utf8(v).unwrap_or_default()
}

/// O conteúdo de `<name>` trocado por `new`.
fn swap_body(xml: &[u8], name: &str, new: &[u8]) -> Result<Vec<u8>, Error> {
    let t = find_tag(xml, 0, name.as_bytes())
        .ok_or_else(|| Error::Format(format!("parte de notas sem <{name}>")))?;
    let open = &xml[t.start..t.end];
    let (open, end) = if t.self_closing {
        ([&open[..open.len() - 2], b">"].concat(), t.end)
    } else {
        let close = format!("</{name}>");
        let k = memmem::rfind(xml, close.as_bytes())
            .ok_or_else(|| Error::Format(format!("parte de notas sem </{name}>")))?;
        (open.to_vec(), k + close.len())
    };
    Ok([&xml[..t.start], &open, new, format!("</{name}>").as_bytes(), &xml[end..]].concat())
}

fn comments(orig: Option<&[u8]>, notes: &[Note]) -> Result<Vec<u8>, Error> {
    const EMPTY: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors/><commentList/></comments>"#;
    let orig = orig.unwrap_or(EMPTY);
    let mut names = authors(orig);
    let mut list = Vec::new();
    for n in notes {
        if let Some(x) = &n.xml {
            list.extend_from_slice(&set_ref(x, &a1(n)));
            continue;
        }
        let k = names.iter().position(|a| *a == n.author).unwrap_or_else(|| {
            names.push(n.author.clone());
            names.len() - 1
        });
        list.extend_from_slice(format!(r#"<comment ref="{}" authorId="{k}" shapeId="0"><text><r><rPr><sz val="9"/><color indexed="81"/><rFont val="Tahoma"/><family val="2"/></rPr><t xml:space="preserve">"#, a1(n)).as_bytes());
        escape_text(&n.text, &mut list);
        list.extend_from_slice(b"</t></r></text></comment>");
    }
    let mut who = Vec::new();
    for a in &names {
        who.extend_from_slice(b"<author>");
        escape_text(a, &mut who);
        who.extend_from_slice(b"</author>");
    }
    swap_body(&swap_body(orig, "authors", &who)?, "commentList", &list)
}

/// O elemento `x` com o `ref` novo.
fn set_ref(x: &str, cell: &str) -> Vec<u8> {
    let b = x.as_bytes();
    let end = tag_end(b, 0).unwrap_or(b.len());
    let name = b.iter().position(|&c| is_name_end(c)).unwrap_or(end);
    let close = if b[end - 2] == b'/' { end - 2 } else { end - 1 };
    let mut out = b[..name].to_vec();
    for_each_attr(&b[name..close], |k, whole| {
        out.push(b' ');
        if k == b"ref" {
            out.extend_from_slice(b"ref=\"");
            out.extend_from_slice(cell.as_bytes());
            out.push(b'"');
        } else {
            out.extend_from_slice(whole);
        }
    });
    out.extend_from_slice(&b[close..]);
    out
}

fn threads(orig: &[u8], notes: &[Note]) -> Result<Vec<u8>, Error> {
    let mut list = Vec::new();
    for n in notes {
        for m in n.thread.iter().flat_map(|t| &t.messages) {
            list.extend_from_slice(&set_ref(&m.xml, &a1(n)));
        }
    }
    swap_body(orig, "ThreadedComments", &list)
}

// ---------------------------------------------------------------------------
// VML
// ---------------------------------------------------------------------------

const VML_NS: &str = r#"xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel""#;

/// Forma "caixa de texto", de que as notas derivam.
fn shape_type() -> String {
    format!(
        r##"<v:shapetype {VML_NS} id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype>"##
    )
}

fn shapes(orig: Option<&[u8]>, notes: &[Note], tab: u32) -> Result<Vec<u8>, Error> {
    let block = tab.clamp(1, 1000);
    let mut xml = match orig {
        Some(x) => {
            // As caixas de nota saem; o resto do desenho fica.
            let mut out = Vec::with_capacity(x.len());
            let mut pos = 0;
            for (a, b, _) in note_shape_spans(x) {
                out.extend_from_slice(&x[pos..a]);
                pos = b;
            }
            out.extend_from_slice(&x[pos..]);
            out
        }
        None => format!(
            r#"<xml {VML_NS}><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="{block}"/></o:shapelayout></xml>"#
        )
        .into_bytes(),
    };
    let mut next = orig.into_iter().flat_map(shape_ids).max().unwrap_or(block * 1024);
    for n in notes {
        next = n.shape.iter().flat_map(|s| shape_ids(s.as_bytes())).fold(next, u32::max);
    }
    let mut new = Vec::new();
    if notes.iter().any(|n| n.shape.is_none()) && memmem::find(&xml, b"\"_x0000_t202\"").is_none() {
        new.extend_from_slice(shape_type().as_bytes());
    }
    for (i, n) in notes.iter().enumerate() {
        match &n.shape {
            Some(s) => new.extend_from_slice(&placed(s.as_bytes(), n)),
            None => {
                next += 1;
                new.extend_from_slice(new_shape(n, next, i + 1).as_bytes());
            }
        }
    }
    let at = memmem::rfind(&xml, b"</xml>").ok_or_else(|| Error::Format("desenho VML sem </xml>".into()))?;
    xml.splice(at..at, new);
    Ok(xml)
}

/// Os números de `_x0000_s1025`.
fn shape_ids(x: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = memmem::find(&x[pos..], b"_x0000_s").map(|k| pos + k + 8) {
        let digits = x[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        if let Some(n) = std::str::from_utf8(&x[i..i + digits]).ok().and_then(|s| s.parse::<u32>().ok()) {
            // Só os da numeração do Excel (1024 por bloco).
            if n < 16_000_000 {
                out.push(n);
            }
        }
        pos = i;
    }
    out
}

fn anchor_text(a: &[i32; 8]) -> String {
    a.iter().map(i32::to_string).collect::<Vec<_>>().join(", ")
}

fn new_shape(n: &Note, id: u32, z: usize) -> String {
    let vis = if n.visible { "visible" } else { "hidden" };
    let mark = if n.visible { "<x:Visible/>" } else { "" };
    format!(
        r##"<v:shape {VML_NS} id="_x0000_s{id}" type="#_x0000_t202" style="position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;z-index:{z};visibility:{vis}" fillcolor="#ffffe1" o:insetmode="auto"><v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/><v:path o:connecttype="none"/><v:textbox style="mso-direction-alt:auto"><div style="text-align:left"></div></v:textbox><x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>{}</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>{}</x:Row><x:Column>{}</x:Column>{mark}</x:ClientData></v:shape>"##,
        anchor_text(&n.anchor),
        n.row,
        n.col,
    )
}

/// A caixa original no lugar (e com a visibilidade) que a nota tem agora.
fn placed(shape: &[u8], n: &Note) -> Vec<u8> {
    let mut x = shape.to_vec();
    for (name, v) in [
        (&b"Anchor"[..], anchor_text(&n.anchor)),
        (b"Row", n.row.to_string()),
        (b"Column", n.col.to_string()),
    ] {
        let Some(e) = vml_find(&x, 0, name) else { continue };
        let body = e.body.as_ptr() as usize - x.as_ptr() as usize;
        if e.body.is_empty() || trimmed(e.body) == v.as_bytes() || same_anchor(e.body, &v) {
            continue;
        }
        let len = e.body.len();
        x.splice(body..body + len, v.into_bytes());
    }
    // Visível: `<x:Visible/>` e o estilo da forma.
    let has = vml_find(&x, 0, b"Visible").map(|e| (e.start, e.end));
    match (n.visible, has) {
        (true, None) => {
            if let Some(cd) = vml_find(&x, 0, b"ClientData") {
                // O mesmo prefixo do `<x:ClientData>`.
                let name = &x[cd.start + 1..];
                let len = name.iter().position(|&b| crate::xml::is_name_end(b)).unwrap_or(0);
                let full = &name[..len];
                let prefix = full.iter().position(|&b| b == b':').map_or(&b""[..], |k| &full[..k + 1]);
                let mark = [b"<", prefix, b"Visible/>"].concat();
                let at = cd.end - (len + 3);
                x.splice(at..at, mark);
            }
        }
        (false, Some((a, b))) => {
            x.drain(a..b);
        }
        _ => {}
    }
    let gt = tag_end(&x, 0).unwrap_or(x.len());
    let (from, to): (&[u8], &[u8]) = if n.visible {
        (b"visibility:hidden", b"visibility:visible")
    } else {
        (b"visibility:visible", b"visibility:hidden")
    };
    if let Some(i) = memmem::find(&x[..gt], from) {
        x.splice(i..i + from.len(), to.iter().copied());
    } else if !n.visible
        && memmem::find(&x[..gt], to).is_none()
        && let Some(i) = memmem::find(&x[..gt], b"style=")
    {
        let at = i + 7;
        x.splice(at..at, b"visibility:hidden;".iter().copied());
    }
    x
}

fn trimmed(b: &[u8]) -> &[u8] {
    b.trim_ascii()
}

/// O Excel quebra linha e muda os espaços da âncora: só importa se os números
/// mudaram.
fn same_anchor(body: &[u8], v: &str) -> bool {
    let nums = |s: &[u8]| -> Vec<String> {
        String::from_utf8_lossy(s).split(',').map(|t| t.trim().to_string()).collect()
    };
    nums(body) == nums(v.as_bytes())
}

/// Acrescenta os tipos que ainda não estão no `[Content_Types].xml`.
pub fn add_types(ct: &[u8], types: &[(String, String)]) -> Vec<u8> {
    let mut out = ct.to_vec();
    for (key, el) in types {
        if memmem::find(&out, key.as_bytes()).is_none()
            && let Some(at) = memmem::rfind(&out, b"</Types>")
        {
            out.splice(at..at, el.bytes());
        }
    }
    out
}

/// Acrescenta relacionamentos (cria a parte se `rels` é `None`).
pub fn add_rels(rels: Option<&[u8]>, new: &[String]) -> Vec<u8> {
    let mut out = rels.map_or_else(
        || br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#.to_vec(),
        <[u8]>::to_vec,
    );
    match memmem::rfind(&out, b"</Relationships>") {
        Some(at) => {
            out.splice(at..at, new.concat().into_bytes());
        }
        // `<Relationships …/>`, sem nenhum.
        None => {
            if let Some(t) = find_tag(&out, 0, b"Relationships").filter(|t| t.self_closing) {
                let (a, b) = (t.end - 2, t.end);
                let mut el = b">".to_vec();
                el.extend_from_slice(new.concat().as_bytes());
                el.extend_from_slice(b"</Relationships>");
                out.splice(a..b, el);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn list_keeps_what_did_not_change() {
        let xml = br#"<comments xmlns="m"><authors><author>Ana</author></authors><commentList><comment ref="B3" authorId="0" xr:uid="{1}"><text><r><rPr><b/></rPr><t>oi</t></r></text></comment></commentList><extLst/></comments>"#;
        let mut notes = crate::notes::parse_comments(xml);
        assert_eq!(text(comments(Some(xml), &notes).unwrap()).as_bytes(), xml);
        notes[0].move_to(5, 2);
        notes.push(Note::new(9, 0, "Beto & Cia", "linha 1\n<2>"));
        let out = comments(Some(xml), &notes).unwrap();
        let s = text(out.clone());
        assert!(s.contains(r#"<comment ref="C6" authorId="0" xr:uid="{1}"><text><r><rPr><b/></rPr>"#), "{s}");
        assert!(s.contains("<author>Ana</author><author>Beto &amp; Cia</author>"), "{s}");
        assert!(s.ends_with("</commentList><extLst/></comments>"), "{s}");
        let back = crate::notes::parse_comments(&out);
        assert_eq!((back[1].author.as_str(), back[1].text.as_str()), ("Beto & Cia", "linha 1\n<2>"));
        // Sem parte de origem, e sem nenhuma nota.
        let out = comments(None, &notes[1..]).unwrap();
        assert_eq!(crate::notes::parse_comments(&out).len(), 1);
        assert!(text(comments(Some(xml), &[]).unwrap()).contains("<commentList></commentList>"));
    }

    #[test]
    fn prefixed_parts() {
        let xml = br#"<?xml version="1.0"?><x:comments xmlns:x="m"><x:authors><x:author>A</x:author></x:authors><x:commentList/></x:comments>"#;
        assert_eq!(
            text(plain(xml)),
            r#"<?xml version="1.0"?><comments xmlns="m"><authors><author>A</author></authors><commentList/></comments>"#
        );
    }

    #[test]
    fn boxes_follow_the_notes() {
        let vml = br##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:x="urn:schemas-microsoft-com:office:excel"><v:shapetype id="_x0000_t202"/><v:shape id="_x0000_s1030" type="#_x0000_t202" style='position:absolute;visibility:hidden' fillcolor="#abcdef"><x:ClientData ObjectType="Note"><x:MoveWithCells/><x:Anchor>
    2, 15, 1, 10, 4, 31, 5, 9</x:Anchor><x:Row>2</x:Row><x:Column>1</x:Column></x:ClientData></v:shape><v:shape id="_x0000_s1031"><x:ClientData ObjectType="Button"/></v:shape></xml>"##;
        let span = note_shape_spans(vml);
        let mut n = Note::new(2, 1, "Ana", "oi");
        n.anchor = span[0].2.anchor.unwrap();
        n.shape = Some(span[0].2.xml.clone());
        // Sem mudança, a forma volta igual.
        assert_eq!(placed(n.shape.as_deref().unwrap().as_bytes(), &n), span[0].2.xml.as_bytes());
        n.move_to(4, 1);
        n.visible = true;
        let new = Note::new(8, 3, "Ana", "nova");
        let out = shapes(Some(vml), &[n, new], 1).unwrap();
        let s = text(out.clone());
        assert!(s.contains("ObjectType=\"Button\""), "o controle fica: {s}");
        assert!(s.contains("<x:Anchor>2, 15, 3, 10, 4, 31, 7, 9</x:Anchor><x:Row>4</x:Row>"), "{s}");
        assert!(s.contains("visibility:visible' fillcolor=\"#abcdef\""), "{s}");
        assert!(s.contains("<x:Column>1</x:Column><x:Visible/></x:ClientData>"), "{s}");
        assert!(s.contains("id=\"_x0000_s1032\""), "id livre: {s}");
        assert_eq!(s.matches("\"_x0000_t202\"").count(), 1);
        let back = note_shape_spans(&out);
        assert_eq!(back.len(), 2);
        assert!(back[0].2.visible && !back[1].2.visible);
        assert_eq!((back[1].2.row, back[1].2.col), (8, 3));
        // Esconder de novo.
        let mut n = Note::new(4, 1, "Ana", "oi");
        n.anchor = back[0].2.anchor.unwrap();
        n.shape = Some(back[0].2.xml.clone());
        let s = text(shapes(None, &[n], 3).unwrap());
        assert!(s.contains("visibility:hidden") && !s.contains("Visible/>"), "{s}");
        assert!(s.contains("data=\"3\""), "{s}");
    }

    #[test]
    fn types_and_rels() {
        let t = vec![("Extension=\"vml\"".to_string(), "<Default Extension=\"vml\"/>".to_string())];
        let ct = add_types(b"<Types></Types>", &t);
        assert_eq!(add_types(&ct, &t), ct, "não repete");
        let r = vec!["<Relationship Id=\"a\"/>".to_string()];
        assert!(text(add_rels(None, &r)).ends_with("<Relationship Id=\"a\"/></Relationships>"));
        assert_eq!(
            text(add_rels(Some(b"<Relationships xmlns=\"p\"/>"), &r)),
            "<Relationships xmlns=\"p\"><Relationship Id=\"a\"/></Relationships>"
        );
    }
}
