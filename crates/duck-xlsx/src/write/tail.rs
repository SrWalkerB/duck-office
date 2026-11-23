//! Trocas no fim do XML de uma aba (depois de `</sheetData>`): validação de
//! dados, proteção e o `<legacyDrawing>` das notas, cada um no lugar que o
//! esquema pede (o Excel recusa a aba se a ordem estiver errada).

use duck_core::Sheet;
use memchr::memmem;

use crate::xml::{find_tag, is_name_end, tag_end};

/// Filhos de `<worksheet>` depois de `<sheetData>`, na ordem do esquema.
const ORDER: [&[u8]; 33] = [
    b"sheetCalcPr",
    b"sheetProtection",
    b"protectedRanges",
    b"scenarios",
    b"autoFilter",
    b"sortState",
    b"dataConsolidate",
    b"customSheetViews",
    b"mergeCells",
    b"phoneticPr",
    b"conditionalFormatting",
    b"dataValidations",
    b"hyperlinks",
    b"printOptions",
    b"pageMargins",
    b"pageSetup",
    b"headerFooter",
    b"rowBreaks",
    b"colBreaks",
    b"customProperties",
    b"cellWatches",
    b"ignoredErrors",
    b"smartTags",
    b"drawing",
    b"legacyDrawing",
    b"legacyDrawingHF",
    b"drawingHF",
    b"picture",
    b"oleObjects",
    b"controls",
    b"webPublishItems",
    b"tableParts",
    b"extLst",
];

fn order(name: &[u8]) -> Option<usize> {
    // `<controls>` e `<oleObjects>` costumam vir embrulhados.
    let name = if name == b"mc:AlternateContent" { b"oleObjects" } else { name };
    ORDER.iter().position(|n| *n == name)
}

/// Elementos do nível de cima: (nome, início, fim).
fn top_level(x: &[u8]) -> Vec<(&[u8], usize, usize)> {
    let mut out: Vec<(&[u8], usize, usize)> = Vec::new();
    let mut depth = 0u32;
    let mut open = (&b""[..], 0);
    let mut pos = 0;
    while let Some(i) = memchr::memchr(b'<', &x[pos..]).map(|k| pos + k) {
        if x[i..].starts_with(b"<!--") {
            pos = memmem::find(&x[i..], b"-->").map_or(x.len(), |k| i + k + 3);
            continue;
        }
        let Some(end) = tag_end(x, i) else { break };
        pos = end;
        match x.get(i + 1) {
            Some(b'/') => {
                if depth == 0 {
                    break; // `</worksheet>`
                }
                depth -= 1;
                if depth == 0 {
                    out.push((open.0, open.1, end));
                }
            }
            Some(b'?' | b'!') => {}
            _ => {
                let len = x[i + 1..].iter().position(|&b| is_name_end(b)).unwrap_or(0);
                let name = &x[i + 1..i + 1 + len];
                if x[end - 2] == b'/' {
                    if depth == 0 {
                        out.push((name, i, end));
                    }
                } else {
                    if depth == 0 {
                        open = (name, i);
                    }
                    depth += 1;
                }
            }
        }
    }
    out
}

/// O elemento `name` do nível de cima, como está.
pub fn element<'a>(tail: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    top_level(tail).into_iter().find(|e| e.0 == name).map(|e| &tail[e.1..e.2])
}

/// Troca o elemento `name` por `new` (vazio = tira), no lugar do esquema.
pub fn replace(tail: &[u8], name: &[u8], new: &[u8]) -> Vec<u8> {
    let list = top_level(tail);
    let mine = order(name).unwrap_or(ORDER.len());
    let at = match list.iter().find(|e| e.0 == name) {
        Some(e) => e.1,
        None => match list.iter().find(|e| order(e.0).is_some_and(|o| o > mine)) {
            Some(e) => e.1,
            None => list.last().map_or_else(|| memmem::find(tail, b"</").unwrap_or(tail.len()), |e| e.2),
        },
    };
    let mut out = Vec::with_capacity(tail.len() + new.len());
    let mut pos = 0;
    let mut done = false;
    let mut insert = |out: &mut Vec<u8>, upto: usize, pos: &mut usize| {
        if !done && at >= *pos && at <= upto {
            out.extend_from_slice(&tail[*pos..at]);
            out.extend_from_slice(new);
            *pos = at;
            done = true;
        }
    };
    for (_, a, b) in list.iter().filter(|e| e.0 == name) {
        insert(&mut out, *a, &mut pos);
        out.extend_from_slice(&tail[pos..*a]);
        pos = *b;
    }
    insert(&mut out, tail.len(), &mut pos);
    out.extend_from_slice(&tail[pos..]);
    out
}

const EXT_VALIDATION: &str = "{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}";
/// A formatação condicional vem antes da validação na lista de extensões.
const EXT_COND: &[u8] = b"{78C0D931-6437-407d-A8EE-F0AAD7539E65}";

/// Troca o `<x14:dataValidations>` da extensão (vazio = tira o `<ext>`, e a
/// lista se ficar vazia).
fn replace_ext_validations(tail: &[u8], new: &[u8]) -> Vec<u8> {
    let Some((_, a, b)) = top_level(tail).into_iter().rfind(|e| e.0 == b"extLst") else {
        if new.is_empty() {
            return tail.to_vec();
        }
        let list = [b"<extLst>", &ext(new)[..], b"</extLst>"].concat();
        return replace(tail, b"extLst", &list);
    };
    let el = &tail[a..b];
    let gt = tag_end(el, 0).unwrap_or(el.len());
    if el[gt - 2] == b'/' {
        let list =
            if new.is_empty() { el.to_vec() } else { [b"<extLst>", &ext(new)[..], b"</extLst>"].concat() };
        return [&tail[..a], &list, &tail[b..]].concat();
    }
    let body = &el[gt..el.len() - b"</extLst>".len()];
    let items = top_level(body);
    let mut out: Vec<u8> = Vec::with_capacity(body.len() + new.len());
    let mut done = new.is_empty();
    for (_, s, e) in items {
        let item = &body[s..e];
        if let Some(t) = find_tag(item, 0, b"x14:dataValidations") {
            if !done {
                const CLOSE: &[u8] = b"</x14:dataValidations>";
                let end = if t.self_closing {
                    t.end
                } else {
                    memmem::find(&item[t.end..], CLOSE).map_or(item.len(), |k| t.end + k + CLOSE.len())
                };
                out.extend_from_slice(&item[..t.start]);
                out.extend_from_slice(new);
                out.extend_from_slice(&item[end..]);
                done = true;
            }
            continue;
        }
        let cond = item.windows(EXT_COND.len()).any(|w| w.eq_ignore_ascii_case(EXT_COND));
        let later = !cond && !body[e..].windows(21).any(|w| w == b"<x14:dataValidations ");
        if !done && later {
            out.extend_from_slice(&ext(new));
            done = true;
        }
        out.extend_from_slice(item);
    }
    if !done {
        out.extend_from_slice(&ext(new));
    }
    if out.is_empty() {
        return [&tail[..a], &tail[b..]].concat();
    }
    [&tail[..a], &el[..gt], &out, b"</extLst>", &tail[b..]].concat()
}

fn ext(new: &[u8]) -> Vec<u8> {
    let mut v = format!(
        "<ext uri=\"{EXT_VALIDATION}\" xmlns:x14=\"http://schemas.microsoft.com/office/spreadsheetml/2009/9/main\">"
    )
    .into_bytes();
    v.extend_from_slice(new);
    v.extend_from_slice(b"</ext>");
    v
}

/// O fim da aba com a validação, a proteção e o `<legacyDrawing>` das notas do
/// modelo, se algum mudou (`all`: regrava tudo). `None` = fica como está.
pub fn apply(tail: &[u8], sheet: &Sheet, all: bool) -> Option<Vec<u8>> {
    let (e, l) = (sheet.edits(), &sheet.layout);
    let notes = (e.notes || all) && !l.notes.is_empty();
    if tail.is_empty() || !(e.validations || e.protection || notes || all) {
        return None;
    }
    let mut t = tail.to_vec();
    if e.protection || all {
        t = replace(&t, b"sheetProtection", &crate::protect::sheet_xml(l.protection.as_ref()));
        let orig = element(&t, b"protectedRanges").unwrap_or_default();
        let new = crate::protect::ranges_xml(&l.protected_ranges, orig);
        t = replace(&t, b"protectedRanges", &new);
    }
    if e.validations || all {
        t = replace(&t, b"dataValidations", &crate::valid::block_xml(&l.validations));
        t = replace_ext_validations(&t, &crate::valid::ext_block_xml(&l.validations));
    }
    if notes && element(&t, b"legacyDrawing").is_none() {
        let rid = super::notes::vml_rid(sheet);
        let x = format!(
            "<legacyDrawing xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"{rid}\"/>"
        );
        t = replace(&t, b"legacyDrawing", x.as_bytes());
    }
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn elements_go_to_their_place() {
        // Um `extLst` dentro da regra não é o da aba.
        let t = br#"<mergeCells/><conditionalFormatting><cfRule><extLst><ext/></extLst></cfRule></conditionalFormatting><pageMargins/><drawing r:id="a"/><mc:AlternateContent><mc:Choice><controls/></mc:Choice></mc:AlternateContent><tableParts/></worksheet>"#;
        let x = s(replace(t, b"dataValidations", b"<DV/>"));
        assert!(x.contains("</conditionalFormatting><DV/><pageMargins/>"), "{x}");
        let x = s(replace(x.as_bytes(), b"sheetProtection", b"<SP/>"));
        assert!(x.starts_with("<SP/><mergeCells/>"), "{x}");
        let x = s(replace(x.as_bytes(), b"legacyDrawing", b"<LD/>"));
        assert!(x.contains(r#"<drawing r:id="a"/><LD/><mc:AlternateContent>"#), "{x}");
        assert_eq!(s(replace(b"</worksheet>", b"dataValidations", b"<DV/>")), "<DV/></worksheet>");
        assert_eq!(
            s(replace(b"<pageMargins/></worksheet>", b"extLst", b"<E/>")),
            "<pageMargins/><E/></worksheet>"
        );
    }

    #[test]
    fn replaces_in_place() {
        let t = b"<mergeCells/><dataValidations count=\"1\"><dataValidation/></dataValidations><hyperlinks/></worksheet>";
        assert_eq!(
            s(replace(t, b"dataValidations", b"<DV/>")),
            "<mergeCells/><DV/><hyperlinks/></worksheet>"
        );
        assert_eq!(s(replace(t, b"dataValidations", b"")), "<mergeCells/><hyperlinks/></worksheet>");
    }

    #[test]
    fn extension_list() {
        let new = b"<x14:dataValidations N/>";
        let x = s(replace_ext_validations(b"<pageMargins/></worksheet>", new));
        assert!(
            x.contains("<pageMargins/><extLst><ext uri=\"{CCE6A557")
                && x.ends_with("</ext></extLst></worksheet>"),
            "{x}"
        );
        assert_eq!(s(replace_ext_validations(x.as_bytes(), b"")), "<pageMargins/></worksheet>");
        // Depois da formatação condicional, antes do resto.
        let t = br#"<extLst><ext uri="{78C0D931-6437-407d-A8EE-F0AAD7539E65}"><a/></ext><ext uri="{05C60535}"><b/></ext></extLst></worksheet>"#;
        let x = s(replace_ext_validations(t, new));
        let (c, v, o) = (x.find("<a/>").unwrap(), x.find(" N/>").unwrap(), x.find("<b/>").unwrap());
        assert!(c < v && v < o, "{x}");
        assert_eq!(s(replace_ext_validations(x.as_bytes(), b"")).as_bytes(), t);
        // Um bloco que já existe é trocado onde está.
        let t = br#"<extLst><ext uri="{1}"><b/></ext><ext uri="{2}" xmlns:x14="u"><x14:dataValidations count="1"><x14:dataValidation/></x14:dataValidations></ext></extLst></worksheet>"#;
        let x = s(replace_ext_validations(t, new));
        assert_eq!(
            x,
            r#"<extLst><ext uri="{1}"><b/></ext><ext uri="{2}" xmlns:x14="u"><x14:dataValidations N/></ext></extLst></worksheet>"#
        );
    }
}
