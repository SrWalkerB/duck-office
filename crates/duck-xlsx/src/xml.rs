//! Utilitários mínimos de XML para as partes do OOXML.
//!
//! Não é um parser XML geral: acha elementos pelo nome, lê atributos e
//! decodifica texto. Tudo opera sobre `&[u8]` sem alocar, com buscas via
//! `memchr` (SIMD).

use memchr::{memchr, memmem};

/// Um elemento de abertura encontrado no buffer.
#[derive(Clone, Copy, Debug)]
pub struct Tag<'a> {
    /// Atributos (entre o nome e o `>` / `/>`).
    pub attrs: &'a [u8],
    pub self_closing: bool,
    /// Posição do `<`.
    pub start: usize,
    /// Posição logo depois do `>`.
    pub end: usize,
}

#[inline]
pub fn is_name_end(b: u8) -> bool {
    matches!(b, b' ' | b'>' | b'/' | b'\t' | b'\n' | b'\r')
}

/// Posição logo depois do `>` que fecha a tag iniciada em `lt`, respeitando aspas.
#[inline]
pub fn tag_end(buf: &[u8], lt: usize) -> Option<usize> {
    let mut i = lt + 1;
    let mut quote = 0u8;
    while i < buf.len() {
        let b = buf[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// Próximo elemento `<name ...>` a partir de `from`.
pub fn find_tag<'a>(buf: &'a [u8], from: usize, name: &[u8]) -> Option<Tag<'a>> {
    let mut pat = Vec::with_capacity(name.len() + 1);
    pat.push(b'<');
    pat.extend_from_slice(name);
    let finder = memmem::Finder::new(&pat);
    let mut pos = from;
    while pos < buf.len() {
        let i = pos + finder.find(&buf[pos..])?;
        let after = i + pat.len();
        if after < buf.len() && is_name_end(buf[after]) {
            let end = tag_end(buf, i)?;
            let self_closing = buf[end - 2] == b'/';
            let attrs_end = if self_closing { end - 2 } else { end - 1 };
            return Some(Tag { attrs: &buf[after..attrs_end.max(after)], self_closing, start: i, end });
        }
        pos = after;
    }
    None
}

/// Conteúdo entre a tag de abertura e `</name>`. Vazio se a tag é `<name/>`.
pub fn inner<'a>(buf: &'a [u8], tag: &Tag, name: &[u8]) -> &'a [u8] {
    if tag.self_closing {
        return &[];
    }
    let mut close = Vec::with_capacity(name.len() + 3);
    close.extend_from_slice(b"</");
    close.extend_from_slice(name);
    close.push(b'>');
    match memmem::find(&buf[tag.end..], &close) {
        Some(i) => &buf[tag.end..tag.end + i],
        None => &buf[tag.end..],
    }
}

/// Itera sobre todos os elementos `<name>` de `buf`, com atributos e conteúdo.
pub fn for_each<'a>(buf: &'a [u8], name: &[u8], mut f: impl FnMut(&'a [u8], &'a [u8])) {
    let mut pos = 0;
    while let Some(tag) = find_tag(buf, pos, name) {
        let content = inner(buf, &tag, name);
        f(tag.attrs, content);
        pos = if tag.self_closing { tag.end } else { tag.end + content.len() };
    }
}

/// Valor bruto (sem decodificar entidades) do atributo `key`.
pub fn attr<'a>(attrs: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    let n = attrs.len();
    while i < n {
        while i < n && attrs[i].is_ascii_whitespace() {
            i += 1;
        }
        let k0 = i;
        while i < n && attrs[i] != b'=' && !attrs[i].is_ascii_whitespace() {
            i += 1;
        }
        let k = &attrs[k0..i];
        while i < n && (attrs[i] == b'=' || attrs[i].is_ascii_whitespace()) {
            i += 1;
        }
        if i >= n {
            return None;
        }
        let q = attrs[i];
        if q != b'"' && q != b'\'' {
            return None;
        }
        let v0 = i + 1;
        let v1 = v0 + memchr(q, &attrs[v0..])?;
        if k == key {
            return Some(&attrs[v0..v1]);
        }
        i = v1 + 1;
    }
    None
}

/// Visita cada atributo: `f(nome, atributo inteiro)` (ex.: `cm`, `cm="1"`).
pub fn for_each_attr<'a>(attrs: &'a [u8], mut f: impl FnMut(&'a [u8], &'a [u8])) {
    scan_attrs(attrs, |k0, k1, _, v1| f(&attrs[k0..k1], &attrs[k0..v1 + 1]));
}

/// Cada atributo: nome e valor.
#[inline]
pub fn for_each_value<'a>(attrs: &'a [u8], mut f: impl FnMut(&'a [u8], &'a [u8])) {
    scan_attrs(attrs, |k0, k1, v0, v1| f(&attrs[k0..k1], &attrs[v0..v1]));
}

/// Posições de cada atributo: nome (início, fim) e valor (início, fim).
#[inline]
fn scan_attrs(attrs: &[u8], mut f: impl FnMut(usize, usize, usize, usize)) {
    let mut i = 0;
    let n = attrs.len();
    while i < n {
        while i < n && attrs[i].is_ascii_whitespace() {
            i += 1;
        }
        let k0 = i;
        while i < n && attrs[i] != b'=' && !attrs[i].is_ascii_whitespace() {
            i += 1;
        }
        let k1 = i;
        while i < n && (attrs[i] == b'=' || attrs[i].is_ascii_whitespace()) {
            i += 1;
        }
        if i >= n || k0 == k1 {
            return;
        }
        let q = attrs[i];
        if q != b'"' && q != b'\'' {
            return;
        }
        let Some(len) = memchr(q, &attrs[i + 1..]) else { return };
        let v1 = i + 1 + len;
        f(k0, k1, i + 1, v1);
        i = v1 + 1;
    }
}

/// Atributo decodificado como texto.
pub fn attr_str(attrs: &[u8], key: &[u8]) -> Option<String> {
    attr(attrs, key).map(|v| {
        let mut s = String::new();
        unescape_into(v, &mut s);
        s
    })
}

#[inline]
pub fn parse_u32(v: &[u8]) -> Option<u32> {
    if v.is_empty() || v.len() > 10 {
        return None;
    }
    let mut n: u64 = 0;
    for &b in v {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n * 10 + (b - b'0') as u64;
    }
    u32::try_from(n).ok()
}

/// Decodifica entidades XML (`&amp;`, `&#10;`...) e escapes do OOXML
/// (`_x000D_`) e anexa em `out`. Bytes UTF-8 inválidos viram U+FFFD.
///
/// Fim de linha segue a especificação XML (seção 2.11): `\r\n` e `\r`
/// **literais** viram `\n`. Um CR de verdade só sobrevive escapado
/// (`&#13;` ou `_x000D_`), que é como o Excel grava.
pub fn unescape_into(raw: &[u8], out: &mut String) {
    let needs_entities = memchr(b'&', raw).is_some();
    let needs_ooxml = memmem::find(raw, b"_x").is_some();
    let needs_eol = memchr(b'\r', raw).is_some();
    if !needs_entities && !needs_ooxml && !needs_eol {
        push_utf8(raw, out);
        return;
    }
    let mut tmp = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        if b == b'\r' {
            tmp.push(b'\n');
            i += if raw.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
            continue;
        }
        if b == b'&'
            && let Some(semi) = memchr(b';', &raw[i..(i + 12).min(raw.len())])
        {
            let ent = &raw[i + 1..i + semi];
            let ch = match ent {
                b"amp" => Some('&'),
                b"lt" => Some('<'),
                b"gt" => Some('>'),
                b"quot" => Some('"'),
                b"apos" => Some('\''),
                _ if ent.first() == Some(&b'#') => {
                    let (digits, radix) = match ent.get(1) {
                        Some(b'x') | Some(b'X') => (&ent[2..], 16),
                        _ => (&ent[1..], 10),
                    };
                    std::str::from_utf8(digits)
                        .ok()
                        .and_then(|d| u32::from_str_radix(d, radix).ok())
                        .and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(c) = ch {
                let mut buf = [0u8; 4];
                tmp.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                i += semi + 1;
                continue;
            }
        }
        if b == b'_'
            && raw.len() >= i + 7
            && raw[i + 1] == b'x'
            && raw[i + 6] == b'_'
            && raw[i + 2..i + 6].iter().all(u8::is_ascii_hexdigit)
        {
            let hex = std::str::from_utf8(&raw[i + 2..i + 6]).unwrap_or("0");
            if let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                let mut buf = [0u8; 4];
                tmp.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                i += 7;
                continue;
            }
        }
        tmp.push(b);
        i += 1;
    }
    push_utf8(&tmp, out);
}

#[inline]
fn push_utf8(bytes: &[u8], out: &mut String) {
    match std::str::from_utf8(bytes) {
        Ok(s) => out.push_str(s),
        Err(_) => out.push_str(&String::from_utf8_lossy(bytes)),
    }
}

/// Concatena o texto de todos os `<t>` de um `<si>`/`<is>`, ignorando a
/// leitura fonética (`<rPh>`), que não aparece na célula.
pub fn rich_text_into(content: &[u8], out: &mut String) {
    let mut pos = 0;
    loop {
        let t = find_tag(content, pos, b"t");
        let ph = find_tag(content, pos, b"rPh");
        match (t, ph) {
            (Some(t), Some(ph)) if ph.start < t.start => {
                let skip = inner(content, &ph, b"rPh");
                pos = ph.end + skip.len();
            }
            (Some(t), _) => {
                let text = inner(content, &t, b"t");
                unescape_into(text, out);
                pos = t.end + text.len();
            }
            (None, _) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes() {
        let a = br#" r="A1" s='3' t="s" xml:space="preserve""#;
        assert_eq!(attr(a, b"r"), Some(&b"A1"[..]));
        assert_eq!(attr(a, b"s"), Some(&b"3"[..]));
        assert_eq!(attr(a, b"t"), Some(&b"s"[..]));
        assert_eq!(attr(a, b"xml:space"), Some(&b"preserve"[..]));
        assert_eq!(attr(a, b"x"), None);
    }

    /// Uma passada vê o mesmo que as buscas por nome, e para num atributo quebrado.
    #[test]
    fn values_in_one_pass() {
        let pairs = |a: &[u8]| {
            let mut v = Vec::new();
            for_each_value(a, |k, x| {
                v.push((String::from_utf8(k.to_vec()).unwrap(), String::from_utf8(x.to_vec()).unwrap()));
            });
            v
        };
        let a = br#" r="12" ht='15.5'  hidden = "1" x14ac:dyDescent="0.25""#;
        let got = pairs(a);
        assert_eq!(got.len(), 4);
        for (k, v) in &got {
            assert_eq!(attr(a, k.as_bytes()), Some(v.as_bytes()));
        }
        assert_eq!(pairs(br#"r="1" s="2"#), [("r".to_string(), "1".to_string())]);
        assert!(pairs(b"  ").is_empty());
    }

    #[test]
    fn tags_respect_name_boundary_and_quotes() {
        let b = br#"<cols><col min="1"/></cols><c r="A1" f="a>b"><v>1</v></c>"#;
        let t = find_tag(b, 0, b"c").unwrap();
        assert_eq!(attr(t.attrs, b"f"), Some(&b"a>b"[..]));
        assert_eq!(inner(b, &t, b"c"), b"<v>1</v>");
        let col = find_tag(b, 0, b"col").unwrap();
        assert!(col.self_closing);
    }

    #[test]
    fn unescape_entities_and_ooxml() {
        let mut s = String::new();
        unescape_into(b"a &amp; b &lt;c&gt; &#233;&#x41; _x000D__x000A_fim", &mut s);
        assert_eq!(s, "a & b <c> éA \r\nfim");
        // CR literal é normalizado; CR escapado sobrevive.
        s.clear();
        unescape_into(b"l1\r\nl2\rl3&#13;\n_x000D_", &mut s);
        assert_eq!(s, "l1\nl2\nl3\r\n\r");
        s.clear();
        unescape_into("sem escape ção".as_bytes(), &mut s);
        assert_eq!(s, "sem escape ção");
    }

    #[test]
    fn rich_text_skips_phonetic() {
        let si = r#"<r><rPr><b/></rPr><t>Fil</t></r><r><t xml:space="preserve">é </t></r><rPh sb="0"><t>FIRE</t></rPh><t>X</t>"#.as_bytes();
        let mut s = String::new();
        rich_text_into(si, &mut s);
        assert_eq!(s, "Filé X");
    }
}
