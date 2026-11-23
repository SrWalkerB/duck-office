//! Namespaces dos desenhos e gráficos.
//!
//! O Excel grava sempre os mesmos prefixos (`xdr:`, `c:`, `a:`...), e os
//! leitores procuram os elementos por esses nomes. Mas o prefixo é livre: o
//! openpyxl usa o namespace padrão (`<wsDr xmlns="…">`) e outros geradores
//! inventam os seus (`<ns0:wsDr>`). [`canonical`] troca os prefixos pelos de
//! costume; quem já os usa passa direto, sem cópia. Para remendar a parte sem
//! mudar os prefixos dela, [`Scan`] percorre as tags do original.

use std::borrow::Cow;

use memchr::{memchr, memmem};

use crate::xml::{for_each_attr, for_each_value, is_name_end, tag_end};

/// Namespace (transicional e estrito) → prefixo usado pelos leitores.
const KNOWN: [(&[u8], &[u8]); 12] = [
    (b"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing", b"xdr"),
    (b"http://schemas.openxmlformats.org/drawingml/2006/chart", b"c"),
    (b"http://schemas.openxmlformats.org/drawingml/2006/main", b"a"),
    (b"http://schemas.openxmlformats.org/officeDocument/2006/relationships", b"r"),
    (b"http://schemas.openxmlformats.org/markup-compatibility/2006", b"mc"),
    (b"http://schemas.microsoft.com/office/drawing/2010/main", b"a14"),
    (b"http://schemas.microsoft.com/office/drawing/2010/slicer", b"sle"),
    (b"http://schemas.microsoft.com/office/drawing/2012/timeslicer", b"tsle"),
    (b"http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing", b"xdr"),
    (b"http://purl.oclc.org/ooxml/drawingml/chart", b"c"),
    (b"http://purl.oclc.org/ooxml/drawingml/main", b"a"),
    (b"http://purl.oclc.org/ooxml/officeDocument/relationships", b"r"),
];

/// Prefixo de costume de um namespace (vazio = os leitores não usam).
fn canon_of(uri: &[u8]) -> &'static [u8] {
    KNOWN.iter().find(|k| k.0 == uri).map_or(b"", |k| k.1)
}

/// O prefixo, se for um dos de costume.
fn usual(prefix: &[u8]) -> Option<&'static [u8]> {
    KNOWN.iter().find(|k| k.1 == prefix).map(|k| k.1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Open,
    Close,
    /// `<x/>`.
    Empty,
}

/// Uma tag: `xml[start..end]`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Event<'a> {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    /// Prefixo como está no arquivo (vazio = namespace padrão).
    pub prefix: &'a [u8],
    pub local: &'a [u8],
    /// Prefixo de costume do namespace (vazio = outro namespace).
    pub ns: &'static [u8],
    /// Fim do nome (onde começam os atributos).
    name_end: usize,
}

impl Event<'_> {
    pub fn is(&self, ns: &[u8], local: &[u8]) -> bool {
        self.ns == ns && self.local == local
    }
}

/// Percorre as tags de `xml`, com as declarações `xmlns` em vigor.
pub(crate) struct Scan<'a> {
    xml: &'a [u8],
    pos: usize,
    /// Declarações em vigor, da mais de fora à mais de dentro.
    bound: Vec<(&'a [u8], &'static [u8])>,
    /// Tamanho de `bound` antes de cada elemento aberto.
    marks: Vec<usize>,
    /// A última tag fechou um elemento: as declarações dele saem na próxima.
    pop: bool,
}

impl<'a> Scan<'a> {
    pub fn new(xml: &'a [u8]) -> Self {
        Scan { xml, pos: 0, bound: Vec::new(), marks: Vec::new(), pop: false }
    }

    /// Prefixo de costume do namespace de `prefix`. Sem declaração, um prefixo
    /// de costume vale por ele mesmo.
    fn resolve(&self, prefix: &[u8]) -> &'static [u8] {
        match self.bound.iter().rev().find(|b| b.0 == prefix) {
            Some(b) => b.1,
            None => usual(prefix).unwrap_or(b""),
        }
    }
}

impl<'a> Iterator for Scan<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        if std::mem::take(&mut self.pop)
            && let Some(n) = self.marks.pop()
        {
            self.bound.truncate(n);
        }
        let xml = self.xml;
        loop {
            let start = self.pos + memchr(b'<', xml.get(self.pos..)?)?;
            let rest = &xml[start..];
            // Comentários, CDATA e instruções: o `>` de dentro não conta.
            let skip: Option<(usize, &[u8])> = match rest {
                [b'<', b'!', b'-', b'-', ..] => Some((4, b"-->")),
                [b'<', b'!', b'[', ..] => Some((3, b"]]>")),
                [b'<', b'?', ..] => Some((2, b"?>")),
                _ => None,
            };
            if let Some((open, close)) = skip {
                self.pos = start + open + memmem::find(&rest[open..], close)? + close.len();
                continue;
            }
            let end = tag_end(xml, start)?;
            self.pos = end;
            if rest.get(1) == Some(&b'!') {
                continue;
            }
            let closing = rest.get(1) == Some(&b'/');
            let name_start = start + 1 + closing as usize;
            let name_end = (name_start..end).find(|&k| is_name_end(xml[k])).unwrap_or(end - 1);
            let name = &xml[name_start..name_end];
            let (prefix, local) = match memchr(b':', name) {
                Some(i) => (&name[..i], &name[i + 1..]),
                None => (&name[..0], name),
            };
            let kind = match (closing, xml[end - 2]) {
                (true, _) => Kind::Close,
                (false, b'/') => Kind::Empty,
                _ => Kind::Open,
            };
            if kind != Kind::Close {
                self.marks.push(self.bound.len());
                let bound = &mut self.bound;
                for_each_value(&xml[name_end..end - 1], |k, v| {
                    if let Some(p) = k.strip_prefix(b"xmlns") {
                        match p {
                            [] => bound.push((p, canon_of(v))),
                            [b':', p @ ..] => bound.push((p, canon_of(v))),
                            _ => {}
                        }
                    }
                });
            }
            self.pop = kind != Kind::Open;
            return Some(Event { kind, start, end, prefix, local, ns: self.resolve(prefix), name_end });
        }
    }
}

/// Alguma declaração `xmlns` foge dos prefixos de costume?
fn unusual(xml: &[u8]) -> bool {
    memmem::find_iter(xml, b"xmlns").any(|i| {
        let rest = &xml[i + 5..];
        let Some(eq) = memchr(b'=', rest) else { return false };
        let prefix = rest[..eq].trim_ascii();
        let prefix = match prefix {
            [] => prefix,
            [b':', p @ ..] => p,
            // "xmlns" no meio de um texto.
            _ => return false,
        };
        let value = rest[eq + 1..].trim_ascii_start();
        let Some((&q, value)) = value.split_first() else { return false };
        let Some(n) = memchr(q, value) else { return false };
        match canon_of(&value[..n]) {
            b"" => usual(prefix).is_some(),
            canon => canon != prefix,
        }
    })
}

/// `xml` com os prefixos de costume (`xdr:`, `c:`, `a:`, `r:`...), seja qual
/// for o jeito de declarar os namespaces. As declarações ficam como estão.
pub(crate) fn canonical(xml: &[u8]) -> Cow<'_, [u8]> {
    if !unusual(xml) {
        return Cow::Borrowed(xml);
    }
    // Um prefixo de costume ligado a outro namespace não pode ser confundido.
    let put = |out: &mut Vec<u8>, prefix: &[u8], ns: &[u8], local: &[u8]| {
        match ns {
            b"" if usual(prefix).is_some() => {
                out.push(b'_');
                out.extend_from_slice(prefix);
            }
            b"" => out.extend_from_slice(prefix),
            ns => out.extend_from_slice(ns),
        }
        if !(prefix.is_empty() && ns.is_empty()) {
            out.push(b':');
        }
        out.extend_from_slice(local);
    };
    let mut out = Vec::with_capacity(xml.len() + xml.len() / 4);
    let mut last = 0;
    let mut scan = Scan::new(xml);
    while let Some(e) = scan.next() {
        out.extend_from_slice(&xml[last..e.start]);
        out.extend_from_slice(if e.kind == Kind::Close { b"</" } else { b"<" });
        put(&mut out, e.prefix, e.ns, e.local);
        last = e.name_end;
        // Atributos com prefixo (`r:id`); sem prefixo, não têm namespace.
        let tail = if e.kind == Kind::Empty { 2 } else { 1 };
        let attrs = &xml[e.name_end..(e.end - tail).max(e.name_end)];
        if e.kind != Kind::Close {
            let mut renamed = false;
            for_each_value(attrs, |k, _| {
                if let Some(p) = attr_prefix(k) {
                    let ns = scan.resolve(p);
                    renamed |= if ns.is_empty() { usual(p).is_some() } else { ns != p };
                }
            });
            if renamed {
                for_each_attr(attrs, |k, whole| {
                    out.push(b' ');
                    match attr_prefix(k) {
                        Some(p) => {
                            put(&mut out, p, scan.resolve(p), &k[p.len() + 1..]);
                            out.extend_from_slice(&whole[k.len()..]);
                        }
                        None => out.extend_from_slice(whole),
                    }
                });
                last = e.name_end + attrs.len();
            }
        }
    }
    out.extend_from_slice(&xml[last..]);
    Cow::Owned(out)
}

/// Prefixo de um atributo (menos `xmlns:` e `xml:`, que não mudam).
fn attr_prefix(k: &[u8]) -> Option<&[u8]> {
    memchr(b':', k).map(|i| &k[..i]).filter(|p| !matches!(*p, b"xmlns" | b"xml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
    const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

    fn canon(xml: &str) -> String {
        String::from_utf8(canonical(xml.as_bytes()).into_owned()).unwrap()
    }

    #[test]
    fn usual_prefixes_are_not_copied() {
        for xml in [
            format!(
                r#"<xdr:wsDr xmlns:xdr="{XDR}" xmlns:a="{A}"><xdr:x a16="1" xmlns:a16="urn:x"/></xdr:wsDr>"#
            ),
            r#"<c:chartSpace xmlns:c="http://purl.oclc.org/ooxml/drawingml/chart"/>"#.to_string(),
            "<xdr:sp><a:t>xmlns = nada</a:t></xdr:sp>".to_string(),
        ] {
            assert!(matches!(canonical(xml.as_bytes()), Cow::Borrowed(_)), "{xml}");
        }
    }

    #[test]
    fn default_namespace() {
        let got = canon(&format!(
            r#"<?xml version="1.0"?><!-- <from> --><wsDr xmlns:a="{A}" xmlns="{XDR}"><ext cx="1" /><a:off/><t>a &lt; b > c</t></wsDr >"#
        ));
        let want = format!(
            r#"<?xml version="1.0"?><!-- <from> --><xdr:wsDr xmlns:a="{A}" xmlns="{XDR}"><xdr:ext cx="1" /><a:off/><xdr:t>a &lt; b > c</xdr:t></xdr:wsDr >"#
        );
        assert_eq!(got, want);
    }

    /// A declaração vale só dentro do elemento; aspas simples também contam.
    #[test]
    fn scopes() {
        let got = canon(&format!(
            r#"<d:wsDr xmlns:d='{XDR}'><chart xmlns='{C}' xmlns:q='{R}' q:id='rId1' id="2"/><f/><q xmlns='{A}'><t/></q><t/></d:wsDr>"#
        ));
        let want = format!(
            r#"<xdr:wsDr xmlns:d='{XDR}'><c:chart xmlns='{C}' xmlns:q='{R}' r:id='rId1' id="2"/><f/><a:q xmlns='{A}'><a:t/></a:q><t/></xdr:wsDr>"#
        );
        assert_eq!(got, want);
    }

    /// Um prefixo de costume ligado a outra coisa não vira o elemento errado.
    #[test]
    fn taken_prefix() {
        let got = canon(&format!(r#"<c:f xmlns:c="urn:outro" c:k="1"/><f xmlns="{C}"><xdr:t/></f>"#));
        let want = format!(r#"<_c:f xmlns:c="urn:outro" _c:k="1"/><c:f xmlns="{C}"><xdr:t/></c:f>"#);
        assert_eq!(got, want);
    }

    #[test]
    fn broken_xml_does_not_panic() {
        let full =
            format!(r#"<wsDr xmlns="{XDR}"><from a:b="1"><![CDATA[<x>]]><col>1</col></from></wsDr></x>"#);
        for n in 0..full.len() {
            canonical(&full.as_bytes()[..n]);
            canonical(&full.as_bytes()[n..]);
        }
        for s in ["<>", "</>", "<", "</", "<!", "<!-->", "<?>", "< xmlns=", "<a xmlns='", "<a xmlns:=''/>"] {
            canonical(format!("<q xmlns='{C}'>{s}").as_bytes());
        }
    }
}
