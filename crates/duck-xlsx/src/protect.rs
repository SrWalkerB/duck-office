//! Proteção: leitura e gravação de `<sheetProtection>`, `<protectedRange>` e
//! `<workbookProtection>`, e a conferência das senhas pelos algoritmos do
//! Excel (o resumo de 16 bits antigo e SHA com sal e voltas).

use duck_core::protect::deny;
use duck_core::{BookProtection, Password, ProtectedRange, SheetProtection};
use sha2::Digest;

use crate::write::escape_attr;
use crate::xml::{Tag, attr, attr_str, find_tag, for_each_attr, parse_u32};

// ---------------------------------------------------------------------------
// Senhas
// ---------------------------------------------------------------------------

/// Resultado de conferir uma senha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Ok,
    Wrong,
    /// O arquivo usa um algoritmo que o duck-sheets não conhece: não dá para
    /// conferir, e sem conferir não se desprotege.
    Unknown,
}

/// Voltas que o Excel usa ao proteger.
const SPIN: u32 = 100_000;
/// Mais que isto é arquivo malfeito (travaria a janela por minutos).
const MAX_SPIN: u32 = 10_000_000;

/// Confere `typed` com a senha guardada.
pub fn check(p: &Password, typed: &str) -> Check {
    if !p.is_set() {
        return Check::Ok;
    }
    if !p.algorithm.is_empty() || !p.hash.is_empty() {
        if p.spin > MAX_SPIN || p.hash.is_empty() {
            return Check::Unknown;
        }
        return match digest(&p.algorithm, typed, &p.salt, p.spin) {
            Some(h) if h == p.hash => Check::Ok,
            Some(_) => Check::Wrong,
            None => Check::Unknown,
        };
    }
    match p.legacy {
        Some(h) if legacy_hash(typed) == h => Check::Ok,
        _ => Check::Wrong,
    }
}

/// Senha nova, com SHA-512 e 100 mil voltas, como o Excel. Vazia = sem senha.
pub fn new_password(typed: &str) -> Password {
    if typed.is_empty() {
        return Password::default();
    }
    let salt = random_salt();
    let hash = digest("SHA-512", typed, &salt, SPIN).unwrap_or_default();
    Password { legacy: None, algorithm: "SHA-512".into(), hash, salt: salt.to_vec(), spin: SPIN }
}

/// 16 bytes do gerador que o `HashMap` usa (semeado pelo sistema). O sal não
/// precisa ser secreto, só diferente a cada proteção.
fn random_salt() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; 16];
    for (i, half) in out.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        half.copy_from_slice(&h.finish().to_le_bytes());
    }
    out
}

/// H(sal + senha em UTF-16), e depois H(anterior + volta) `spin` vezes.
fn digest(algorithm: &str, typed: &str, salt: &[u8], spin: u32) -> Option<Vec<u8>> {
    fn run<D: Digest>(typed: &str, salt: &[u8], spin: u32) -> Vec<u8> {
        let mut first = salt.to_vec();
        first.extend(typed.encode_utf16().flat_map(u16::to_le_bytes));
        let mut h = D::digest(&first).to_vec();
        for i in 0..spin {
            let mut d = D::new();
            d.update(&h);
            d.update(i.to_le_bytes());
            h.copy_from_slice(&d.finalize());
        }
        h
    }
    Some(match algorithm.to_ascii_uppercase().as_str() {
        "SHA-512" | "SHA512" => run::<sha2::Sha512>(typed, salt, spin),
        "SHA-384" | "SHA384" => run::<sha2::Sha384>(typed, salt, spin),
        "SHA-256" | "SHA256" => run::<sha2::Sha256>(typed, salt, spin),
        "SHA-1" | "SHA1" => run::<sha1::Sha1>(typed, salt, spin),
        _ => return None,
    })
}

/// Resumo de 16 bits do Excel antigo (`password="83AF"`).
fn legacy_hash(typed: &str) -> u16 {
    // Um byte por caractere; fora do Latin-1, o Excel usava a página do
    // sistema: fica o byte de baixo.
    let bytes: Vec<u8> = typed.chars().map(|c| c as u32 as u8).collect();
    if bytes.is_empty() {
        return 0;
    }
    let rot = |h: u16| ((h >> 14) & 1) | ((h << 1) & 0x7fff);
    let mut h = 0u16;
    for &b in bytes.iter().rev() {
        h = rot(h) ^ b as u16;
    }
    rot(h) ^ bytes.len() as u16 ^ 0xCE4B
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let v = (c[0] as u32) << 16
            | (c.get(1).copied().unwrap_or(0) as u32) << 8
            | c.get(2).copied().unwrap_or(0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(T[(v >> (18 - 6 * k)) as usize & 63] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn from_base64(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in s {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// Nomes dos atributos de senha: os da aba e os da pasta.
struct Names {
    legacy: &'static str,
    algorithm: &'static str,
    hash: &'static str,
    salt: &'static str,
    spin: &'static str,
}

const SHEET: Names = Names {
    legacy: "password",
    algorithm: "algorithmName",
    hash: "hashValue",
    salt: "saltValue",
    spin: "spinCount",
};

const BOOK: Names = Names {
    legacy: "workbookPassword",
    algorithm: "workbookAlgorithmName",
    hash: "workbookHashValue",
    salt: "workbookSaltValue",
    spin: "workbookSpinCount",
};

fn read_password(a: &[u8], n: &Names) -> Password {
    let bytes = |k: &str| attr(a, k.as_bytes()).map(from_base64).unwrap_or_default();
    Password {
        legacy: attr(a, n.legacy.as_bytes())
            .filter(|v| !v.is_empty())
            // Um resumo que não é hexadecimal ainda é uma senha (que não confere).
            .map(|v| {
                std::str::from_utf8(v).ok().and_then(|s| u16::from_str_radix(s.trim(), 16).ok()).unwrap_or(1)
            }),
        algorithm: attr_str(a, n.algorithm.as_bytes()).unwrap_or_default(),
        hash: bytes(n.hash),
        salt: bytes(n.salt),
        spin: attr(a, n.spin.as_bytes()).and_then(parse_u32).unwrap_or(0),
    }
}

fn push(out: &mut Vec<u8>, k: &str, v: &str) {
    out.push(b' ');
    out.extend_from_slice(k.as_bytes());
    out.extend_from_slice(b"=\"");
    escape_attr(v, out);
    out.push(b'"');
}

fn write_password(p: &Password, n: &Names, out: &mut Vec<u8>) {
    if let Some(h) = p.legacy {
        push(out, n.legacy, &format!("{h:04X}"));
    }
    if !p.algorithm.is_empty() {
        push(out, n.algorithm, &p.algorithm);
    }
    if !p.hash.is_empty() {
        push(out, n.hash, &base64(&p.hash));
    }
    if !p.salt.is_empty() {
        push(out, n.salt, &base64(&p.salt));
    }
    if p.spin > 0 {
        push(out, n.spin, &p.spin.to_string());
    }
}

// ---------------------------------------------------------------------------
// Aba
// ---------------------------------------------------------------------------

/// Atributos do `<sheetProtection>`.
pub(crate) fn parse_sheet(a: &[u8]) -> SheetProtection {
    let on = |k: &str| match attr(a, k.as_bytes()) {
        Some(b"1") | Some(b"true") => Some(true),
        Some(b"0") | Some(b"false") => Some(false),
        _ => None,
    };
    let mut d = 0;
    for (bit, name) in deny::ATTRS {
        if on(name).unwrap_or(deny::DEFAULT & bit != 0) {
            d |= bit;
        }
    }
    SheetProtection { enabled: on("sheet").unwrap_or(false), deny: d, password: read_password(a, &SHEET) }
}

pub(crate) fn sheet_xml(p: Option<&SheetProtection>) -> Vec<u8> {
    let Some(p) = p else { return Vec::new() };
    let mut out = b"<sheetProtection".to_vec();
    write_password(&p.password, &SHEET, &mut out);
    if p.enabled {
        push(&mut out, "sheet", "1");
    }
    for (bit, name) in deny::ATTRS {
        let on = p.deny & bit != 0;
        if on != (deny::DEFAULT & bit != 0) {
            push(&mut out, name, if on { "1" } else { "0" });
        }
    }
    out.extend_from_slice(b"/>");
    out
}

/// Atributos de um `<protectedRange>`.
pub(crate) fn parse_range(a: &[u8]) -> Option<ProtectedRange> {
    let ranges = crate::valid::ranges(attr(a, b"sqref")?);
    (!ranges.is_empty()).then(|| ProtectedRange {
        name: attr_str(a, b"name").unwrap_or_default(),
        ranges,
        password: read_password(a, &SHEET),
    })
}

/// `<protectedRanges>` com os intervalos do modelo; o que o modelo não guarda
/// (`securityDescriptor`) vem do XML original, pelo nome.
pub(crate) fn ranges_xml(list: &[ProtectedRange], original: &[u8]) -> Vec<u8> {
    if list.is_empty() {
        return Vec::new();
    }
    let mut out = b"<protectedRanges>".to_vec();
    for p in list {
        out.extend_from_slice(b"<protectedRange");
        write_password(&p.password, &SHEET, &mut out);
        out.extend_from_slice(b" sqref=\"");
        crate::valid::sqref(&p.ranges, &mut out);
        out.push(b'"');
        push(&mut out, "name", &p.name);
        let mut pos = 0;
        let mut body: &[u8] = &[];
        while let Some(t) = find_tag(original, pos, b"protectedRange") {
            pos = t.end;
            if attr_str(t.attrs, b"name").as_deref() == Some(p.name.as_str()) {
                for_each_attr(t.attrs, |k, whole| {
                    if k == b"securityDescriptor" {
                        out.push(b' ');
                        out.extend_from_slice(whole);
                    }
                });
                body = crate::xml::inner(original, &t, b"protectedRange");
                break;
            }
        }
        if body.is_empty() {
            out.extend_from_slice(b"/>");
        } else {
            out.push(b'>');
            out.extend_from_slice(body);
            out.extend_from_slice(b"</protectedRange>");
        }
    }
    out.extend_from_slice(b"</protectedRanges>");
    out
}

// ---------------------------------------------------------------------------
// Pasta
// ---------------------------------------------------------------------------

/// `<workbookProtection>` do `workbook.xml`.
pub(crate) fn parse_book(wb: &[u8]) -> Option<BookProtection> {
    let t = find_tag(wb, 0, b"workbookProtection")?;
    let a = t.attrs;
    let on = |k: &[u8]| matches!(attr(a, k), Some(b"1") | Some(b"true"));
    let known =
        ["lockStructure", "lockWindows", BOOK.legacy, BOOK.algorithm, BOOK.hash, BOOK.salt, BOOK.spin];
    let mut extra = Vec::new();
    for_each_attr(a, |k, whole| {
        if !known.iter().any(|n| n.as_bytes() == k) {
            extra.push(b' ');
            extra.extend_from_slice(whole);
        }
    });
    Some(BookProtection {
        structure: on(b"lockStructure"),
        windows: on(b"lockWindows"),
        password: read_password(a, &BOOK),
        extra: String::from_utf8_lossy(&extra).into_owned(),
    })
}

fn book_xml(p: &BookProtection) -> Vec<u8> {
    let mut out = b"<workbookProtection".to_vec();
    write_password(&p.password, &BOOK, &mut out);
    if p.structure {
        push(&mut out, "lockStructure", "1");
    }
    if p.windows {
        push(&mut out, "lockWindows", "1");
    }
    out.extend_from_slice(p.extra.as_bytes());
    out.extend_from_slice(b"/>");
    out
}

/// O `workbook.xml` com a proteção trocada (ou tirada).
pub(crate) fn patch_book(wb: &[u8], p: Option<&BookProtection>) -> Vec<u8> {
    let new = p.map(book_xml).unwrap_or_default();
    if let Some(t) = find_tag(wb, 0, b"workbookProtection") {
        let Tag { start, mut end, .. } = t;
        if !t.self_closing {
            const CLOSE: &[u8] = b"</workbookProtection>";
            end = memchr::memmem::find(&wb[end..], CLOSE).map_or(end, |k| end + k + CLOSE.len());
        }
        return [&wb[..start], &new, &wb[end..]].concat();
    }
    if new.is_empty() {
        return wb.to_vec();
    }
    // Na ordem do esquema: antes de `bookViews` ou de `sheets`.
    let at = [&b"bookViews"[..], b"sheets"].iter().filter_map(|n| find_tag(wb, 0, n)).map(|t| t.start).min();
    match at {
        Some(at) => [&wb[..at], &new, &wb[at..]].concat(),
        None => wb.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_hashes() {
        // Valores conhecidos do formato antigo.
        assert_eq!(legacy_hash("password"), 0x83AF);
        assert_eq!(legacy_hash("test"), 0xCBEB);
        assert_eq!(legacy_hash("pato"), 0xCA7B, "como o openpyxl grava");
        let p = Password { legacy: Some(0x83AF), ..Default::default() };
        assert_eq!(check(&p, "password"), Check::Ok);
        assert_eq!(check(&p, "Password"), Check::Wrong);
        assert_eq!(check(&p, ""), Check::Wrong);
    }

    /// Conferido com outra implementação (`hashlib` do Python) do mesmo algoritmo.
    #[test]
    fn strong_hashes() {
        let salt: Vec<u8> = (0..16).collect();
        let hex = |s: &str| -> Vec<u8> {
            (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
        };
        for (alg, spin, want) in
            [("SHA-512", 1000, SHA512), ("SHA-384", 10, SHA384), ("SHA-256", 10, SHA256), ("SHA-1", 10, SHA1)]
        {
            let p =
                Password { algorithm: alg.into(), hash: hex(want), salt: salt.clone(), spin, legacy: None };
            assert_eq!(check(&p, "Pätö1"), Check::Ok, "{alg}");
            assert_eq!(check(&p, "Pato1"), Check::Wrong, "{alg}");
        }
        let p = Password { algorithm: "WHIRLPOOL".into(), hash: vec![1], ..Default::default() };
        assert_eq!(check(&p, "x"), Check::Unknown);
    }

    const SHA512: &str = "02b0c05ea510bf18a5845db85648c90dd1c1205162b237c4824cc834e260d36ec279fd53846c7387d7855562e4039b154e11c155292b30564895c349a927c3f1";
    const SHA384: &str =
        "1c470501292463c1fe1b7db2ff6280dc844084a56ebbee3d707d7200df865dddb87dc2be0a527f336215534589f11584";
    const SHA256: &str = "6e2869796f5981004b081a39e74dc5d8f74bc260f60a4f82e641ab7b2d59d17e";
    const SHA1: &str = "d43ac9c1d71642f6ce6706b6482fd72295909725";

    #[test]
    fn new_password_round_trip() {
        let p = new_password("segredo");
        assert_eq!((p.algorithm.as_str(), p.spin, p.salt.len(), p.hash.len()), ("SHA-512", 100_000, 16, 64));
        assert_eq!(check(&p, "segredo"), Check::Ok);
        assert_eq!(check(&p, "Segredo"), Check::Wrong);
        assert_ne!(new_password("segredo").salt, p.salt);
        assert!(!new_password("").is_set());
        for n in 0..20u8 {
            let b: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(from_base64(base64(&b).as_bytes()), b);
        }
        assert_eq!(base64(b"Ma"), "TWE=");
    }

    #[test]
    fn sheet_attributes() {
        let a = br#" password="CC1A" sheet="1" objects="1" formatCells="0" sort="0" selectLockedCells="1""#;
        let p = parse_sheet(a);
        assert!(p.enabled);
        assert_eq!(p.password.legacy, Some(0xCC1A));
        assert!(p.deny & deny::OBJECTS != 0 && p.deny & deny::SCENARIOS == 0);
        assert!(p.deny & deny::FORMAT_CELLS == 0 && p.deny & deny::FORMAT_ROWS != 0);
        assert!(p.deny & deny::SORT == 0 && p.deny & deny::SELECT_LOCKED != 0);
        let x = sheet_xml(Some(&p));
        let t = find_tag(&x, 0, b"sheetProtection").unwrap();
        assert_eq!(parse_sheet(t.attrs), p);
        assert!(sheet_xml(None).is_empty());
    }

    #[test]
    fn protected_ranges_keep_descriptors() {
        let orig = br#"<protectedRanges><protectedRange password="CC1A" sqref="A1:B2 D4" name="R&amp;1" securityDescriptor="O:X"/></protectedRanges>"#;
        let t = find_tag(orig, 0, b"protectedRange").unwrap();
        let p = parse_range(t.attrs).unwrap();
        assert_eq!((p.name.as_str(), p.ranges.len()), ("R&1", 2));
        let out = ranges_xml(&[p], orig);
        assert_eq!(String::from_utf8_lossy(&out), String::from_utf8_lossy(orig));
    }

    #[test]
    fn book_protection() {
        let wb = br#"<workbook><workbookPr/><bookViews/><sheets/></workbook>"#;
        assert_eq!(parse_book(wb), None);
        let p = BookProtection { structure: true, extra: " lockRevision=\"1\"".into(), ..Default::default() };
        let out = patch_book(wb, Some(&p));
        assert_eq!(
            String::from_utf8_lossy(&out),
            r#"<workbook><workbookPr/><workbookProtection lockStructure="1" lockRevision="1"/><bookViews/><sheets/></workbook>"#
        );
        assert_eq!(parse_book(&out), Some(p));
        assert_eq!(patch_book(&out, None), wb);
    }
}
