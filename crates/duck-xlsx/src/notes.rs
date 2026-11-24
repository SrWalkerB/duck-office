//! Notas e conversas: leitura das partes ligadas à aba (`comments`, o VML das
//! caixas e `threadedComments`). A gravação fica em `write/notes.rs`.

use std::collections::HashMap;

use duck_core::Note;
use duck_core::notes::{Message, Thread};

use crate::package::Package;
use crate::parts;
use crate::sheet::parse_range;
use crate::xml::{
    attr, attr_str, find_tag, inner, is_name_end, parse_u32, rich_text_into, tag_end, unescape_into,
};

/// Nomes das pessoas (`xl/persons/person.xml`), por id.
pub type Persons = HashMap<String, String>;

pub fn read_persons(pkg: &Package, workbook: &str) -> Persons {
    let mut out = Persons::new();
    for (_, kind, part) in parts::rel_targets_typed(pkg, workbook) {
        if !kind.ends_with("/person") {
            continue;
        }
        let Ok(xml) = pkg.read(&part) else { continue };
        crate::xml::for_each(&xml, b"person", |a, _| {
            if let (Some(id), Some(name)) = (attr_str(a, b"id"), attr_str(a, b"displayName")) {
                out.insert(id.to_ascii_uppercase(), name);
            }
        });
    }
    out
}

/// As partes de notas ligadas a uma aba (caminhos no pacote).
#[derive(Default)]
pub(crate) struct NoteParts {
    pub comments: Option<String>,
    pub threads: Option<String>,
    /// (`r:id`, caminho) dos VML: o das notas e o do cabeçalho da página.
    pub vml: Vec<(String, String)>,
}

/// As partes de notas nos relacionamentos (`rels`) da aba `sheet_part`.
pub(crate) fn note_parts(rels: &[u8], sheet_part: &str) -> NoteParts {
    let dir = sheet_part.rsplit_once('/').map_or("", |p| p.0);
    let mut p = NoteParts::default();
    crate::xml::for_each(rels, b"Relationship", |a, _| {
        let (Some(id), Some(kind), Some(target)) =
            (attr_str(a, b"Id"), attr_str(a, b"Type"), attr_str(a, b"Target"))
        else {
            return;
        };
        if attr(a, b"TargetMode") == Some(b"External") {
            return;
        }
        let part = parts::resolve(dir, &target);
        if kind.ends_with("/comments") {
            p.comments = Some(part);
        } else if kind.ends_with("/threadedComment") {
            p.threads = Some(part);
        } else if kind.ends_with("/vmlDrawing") {
            p.vml.push((id, part));
        }
    });
    p
}

/// As notas da aba. `legacy` é o `r:id` do `<legacyDrawing>`.
pub fn read_notes(
    pkg: &Package,
    sheet_part: &str,
    legacy: Option<&str>,
    persons: &dyn Fn() -> Persons,
) -> Vec<Note> {
    let Ok(rels) = pkg.read(&crate::write::rels_path(sheet_part)) else { return Vec::new() };
    let p = note_parts(&rels, sheet_part);
    if p.comments.is_none() && p.threads.is_none() {
        return Vec::new();
    }
    let plain = crate::write::notes::plain;
    let mut list = match p.comments.as_deref().and_then(|c| pkg.read(c).ok()) {
        Some(xml) => parse_comments(&plain(&xml)),
        None => Vec::new(),
    };
    let vml = p.vml.iter().find(|v| Some(v.0.as_str()) == legacy).or_else(|| p.vml.first());
    if let Some(xml) = vml.and_then(|v| pkg.read(&v.1).ok()) {
        let mut shapes: HashMap<(u32, u32), VmlNote> =
            note_shapes(&xml).into_iter().map(|s| ((s.row, s.col), s)).collect();
        for n in &mut list {
            if let Some(s) = shapes.remove(&(n.row, n.col)) {
                n.anchor = s.anchor.unwrap_or(n.anchor);
                n.visible = s.visible;
                n.shape = Some(s.xml);
            }
        }
    }
    if let Some(xml) = p.threads.as_deref().and_then(|t| pkg.read(t).ok()) {
        attach_threads(&plain(&xml), &persons(), &mut list);
    }
    duck_core::notes::sort(&mut list);
    list.dedup_by_key(|n| (n.row, n.col));
    list
}

/// O `xl/commentsN.xml`.
pub fn parse_comments(xml: &[u8]) -> Vec<Note> {
    let authors = authors(xml);
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(t) = find_tag(xml, pos, b"comment") {
        let body = inner(xml, &t, b"comment");
        let end = if t.self_closing { t.end } else { (t.end + body.len() + 10).min(xml.len()) };
        pos = end;
        let Some(cell) = attr(t.attrs, b"ref").and_then(parse_range) else { continue };
        let mut n = Note { row: cell.first_row, col: cell.first_col, ..Default::default() };
        n.author = attr(t.attrs, b"authorId")
            .and_then(parse_u32)
            .and_then(|i| authors.get(i as usize).cloned())
            .unwrap_or_default();
        if let Some(tt) = find_tag(body, 0, b"text") {
            let c = inner(body, &tt, b"text");
            rich_text_into(c, &mut n.text);
            n.runs = parts::rich_runs(c, n.text.len()).unwrap_or_default();
        }
        n.anchor = duck_core::notes::default_anchor(n.row, n.col);
        n.xml = Some(String::from_utf8_lossy(&xml[t.start..end]).into());
        out.push(n);
    }
    out
}

pub(crate) fn authors(xml: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(t) = find_tag(xml, 0, b"authors") {
        crate::xml::for_each(inner(xml, &t, b"authors"), b"author", |_, c| {
            let mut s = String::new();
            unescape_into(c, &mut s);
            out.push(s);
        });
    }
    out
}

/// Caixa de uma nota no VML.
pub(crate) struct VmlNote {
    pub row: u32,
    pub col: u32,
    pub anchor: Option<[i32; 8]>,
    pub visible: bool,
    /// O `<v:shape>` inteiro.
    pub xml: Box<str>,
}

/// Um elemento do VML pelo nome local (o prefixo depende de quem gravou:
/// `v:shape`, `ns1:shape`).
#[derive(Clone, Copy)]
pub(crate) struct VmlEl<'a> {
    pub start: usize,
    pub end: usize,
    pub attrs: &'a [u8],
    pub body: &'a [u8],
}

/// Próximo elemento com o nome local `name`, com o conteúdo até o fechamento.
pub(crate) fn vml_find<'a>(x: &'a [u8], from: usize, name: &[u8]) -> Option<VmlEl<'a>> {
    let mut pos = from;
    while let Some(i) = memchr::memchr(b'<', x.get(pos..)?).map(|k| pos + k) {
        pos = i + 1;
        let len = x[i + 1..].iter().position(|&b| is_name_end(b))?;
        let full = &x[i + 1..i + 1 + len];
        let local = full.iter().position(|&b| b == b':').map_or(full, |k| &full[k + 1..]);
        if local != name || full.first() == Some(&b'/') {
            continue;
        }
        let gt = tag_end(x, i)?;
        if x[gt - 2] == b'/' {
            return Some(VmlEl { start: i, end: gt, attrs: &x[i + 1 + len..gt - 2], body: &[] });
        }
        let close = [b"</", full, b">"].concat();
        let k = memchr::memmem::find(&x[gt..], &close)?;
        return Some(VmlEl {
            start: i,
            end: gt + k + close.len(),
            attrs: &x[i + 1 + len..gt - 1],
            body: &x[gt..gt + k],
        });
    }
    None
}

fn vml_text(body: &[u8], name: &[u8]) -> Option<String> {
    let e = vml_find(body, 0, name)?;
    let mut s = String::new();
    unescape_into(e.body, &mut s);
    Some(s.trim().to_string())
}

/// As formas do VML que são caixas de nota: (início, fim, nota).
pub(crate) fn note_shape_spans(xml: &[u8]) -> Vec<(usize, usize, VmlNote)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(s) = vml_find(xml, pos, b"shape") {
        pos = s.end;
        let Some(cd) = vml_find(s.body, 0, b"ClientData") else { continue };
        if attr(cd.attrs, b"ObjectType") != Some(b"Note") {
            continue;
        }
        let num = |n: &[u8]| vml_text(cd.body, n).and_then(|t| t.parse::<u32>().ok());
        let (Some(row), Some(col)) = (num(b"Row"), num(b"Column")) else { continue };
        let anchor = vml_text(cd.body, b"Anchor").and_then(|t| {
            let v: Vec<i32> = t.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            <[i32; 8]>::try_from(v).ok()
        });
        // `<x:Visible/>` ou `<x:Visible>True</x:Visible>`.
        let visible = vml_text(cd.body, b"Visible")
            .is_some_and(|t| t.is_empty() || t.eq_ignore_ascii_case("t") || t.eq_ignore_ascii_case("true"));
        let xml = String::from_utf8_lossy(&xml[s.start..s.end]).into();
        out.push((s.start, s.end, VmlNote { row, col, anchor, visible, xml }));
    }
    out
}

fn note_shapes(xml: &[u8]) -> Vec<VmlNote> {
    note_shape_spans(xml).into_iter().map(|s| s.2).collect()
}

/// Liga as conversas às notas das mesmas células.
pub(crate) fn attach_threads(xml: &[u8], persons: &Persons, list: &mut Vec<Note>) {
    // Raiz por id → índice em `threads`.
    let mut threads: Vec<((u32, u32), Thread)> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut pos = 0;
    while let Some(t) = find_tag(xml, pos, b"threadedComment") {
        let body = inner(xml, &t, b"threadedComment");
        let end = if t.self_closing { t.end } else { (t.end + body.len() + 18).min(xml.len()) };
        pos = end;
        let id = attr_str(t.attrs, b"id").unwrap_or_default().to_ascii_uppercase();
        let mut m = Message {
            author: attr_str(t.attrs, b"personId")
                .and_then(|p| persons.get(&p.to_ascii_uppercase()).cloned())
                .unwrap_or_default(),
            date: attr_str(t.attrs, b"dT").unwrap_or_default(),
            xml: String::from_utf8_lossy(&xml[t.start..end]).into(),
            ..Default::default()
        };
        if let Some(x) = find_tag(body, 0, b"text") {
            unescape_into(inner(body, &x, b"text"), &mut m.text);
        }
        let parent = attr_str(t.attrs, b"parentId").map(|p| p.to_ascii_uppercase());
        match parent.and_then(|p| by_id.get(&p).copied()) {
            Some(k) => threads[k].1.messages.push(m),
            None => {
                let Some(cell) = attr(t.attrs, b"ref").and_then(parse_range) else { continue };
                by_id.insert(id, threads.len());
                let done = matches!(attr(t.attrs, b"done"), Some(b"1") | Some(b"true"));
                threads.push(((cell.first_row, cell.first_col), Thread { messages: vec![m], done }));
            }
        }
    }
    for ((row, col), th) in threads {
        match list.iter_mut().find(|n| (n.row, n.col) == (row, col)) {
            // Duas conversas na mesma célula não existem no Excel; se vierem,
            // as mensagens ficam juntas (e nenhuma se perde ao gravar).
            Some(n) => match &mut n.thread {
                Some(old) => old.messages.extend(th.messages),
                None => n.thread = Some(Box::new(th)),
            },
            None => {
                let text: Vec<&str> = th.messages.iter().map(|m| m.text.as_str()).collect();
                let author = th.messages.first().map(|m| m.author.clone()).unwrap_or_default();
                let mut n = Note::new(row, col, author, text.join("\n"));
                n.thread = Some(Box::new(th));
                list.push(n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_shapes() {
        let xml = r#"<comments xmlns="m"><authors><author>Ana &amp; Cia</author><author>tc={1}</author></authors><commentList><comment ref="B3" authorId="0" shapeId="0"><text><r><rPr><b/><sz val="9"/></rPr><t>Ana:</t></r><r><t xml:space="preserve">
olá</t></r></text></comment><comment ref="D1" authorId="1"><text><t>simples</t></text></comment></commentList></comments>"#;
        let n = parse_comments(xml.as_bytes());
        assert_eq!(n.len(), 2);
        assert_eq!((n[0].row, n[0].col, n[0].author.as_str()), (2, 1, "Ana & Cia"));
        assert_eq!(n[0].text, "Ana:\nolá");
        assert_eq!(n[0].runs.len(), 2);
        assert!(n[1].xml.as_deref().unwrap().ends_with("</comment>"));
        for (v, x) in [("v", "x"), ("ns1", "ns2")] {
            let vml = format!(
                r#"<xml><{v}:shapetype id="_x0000_t202"/><{v}:shape id="_x0000_s1025" type="t"><{v}:fill/><{x}:ClientData ObjectType="Note"><{x}:MoveWithCells/><{x}:Anchor>
    2, 15, 1, 10, 4, 31, 5, 9</{x}:Anchor><{x}:Row>2</{x}:Row><{x}:Column>1</{x}:Column><{x}:Visible/></{x}:ClientData></{v}:shape><{v}:shape id="b"><{x}:ClientData ObjectType="Button"/></{v}:shape></xml>"#
            );
            let s = note_shape_spans(vml.as_bytes());
            assert_eq!(s.len(), 1);
            assert_eq!((s[0].2.row, s[0].2.col, s[0].2.visible), (2, 1, true));
            assert_eq!(s[0].2.anchor, Some([2, 15, 1, 10, 4, 31, 5, 9]));
            assert!(s[0].2.xml.ends_with(&format!("</{v}:shape>")));
        }
    }

    #[test]
    fn threads() {
        let xml = r#"<ThreadedComments xmlns="t"><threadedComment ref="B3" dT="2026-11-23T10:30:00.00" personId="{P1}" id="{A}" done="1"><text>Pergunta</text></threadedComment><threadedComment ref="B3" dT="2026-11-23T11:00:00.00" personId="{p2}" id="{B}" parentId="{a}"><text>Resposta &lt;</text></threadedComment></ThreadedComments>"#;
        let persons: Persons =
            [("{P1}".to_string(), "Ana".to_string()), ("{P2}".to_string(), "Beto".to_string())].into();
        let mut list = vec![Note::new(2, 1, "tc={A}", "[Threaded comment]")];
        attach_threads(xml.as_bytes(), &persons, &mut list);
        let t = list[0].thread.as_ref().unwrap();
        assert!(t.done);
        assert_eq!(t.messages.len(), 2);
        assert_eq!((t.messages[1].author.as_str(), t.messages[1].text.as_str()), ("Beto", "Resposta <"));
    }
}
