//! Leitura de `.csv` (e `.tsv`/`.txt` separados): uma aba só, como o Excel
//! abre um CSV.
//!
//! - Codificação: UTF-8 (com ou sem BOM); se não for UTF-8 válido, Windows-1252
//!   (o padrão do Excel em português).
//! - Separador: o mais regular entre `;`, `,`, tab e `|` nas primeiras linhas.
//! - Números: com separador `;`, vírgula decimal e ponto de milhar
//!   (`1.234,56`), como o Excel em pt-BR; com os outros, ponto decimal.
//!   Datas e o resto ficam como texto.

use std::path::Path;

use duck_core::{Sheet, SheetPart, kind};

use crate::Error;

/// Lê um CSV inteiro numa aba.
pub fn read_csv(path: impl AsRef<Path>) -> Result<Sheet, Error> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(Error::Io)?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "CSV".into());
    Ok(parse_csv(&bytes, &name))
}

/// Interpreta os bytes de um CSV (ver o módulo).
pub fn parse_csv(bytes: &[u8], name: &str) -> Sheet {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => std::borrow::Cow::Borrowed(s),
        Err(_) => std::borrow::Cow::Owned(windows_1252(bytes)),
    };
    let sep = detect_separator(&text);
    let comma_decimal = sep == b';';
    let mut part = SheetPart::new();
    let mut field = String::new();
    let (mut row, mut col) = (0u32, 0u32);
    let b = text.as_bytes();
    let mut i = 0;
    let mut quoted_field = false;
    let flush = |part: &mut SheetPart, field: &mut String, quoted: bool, row: u32, col: u32| {
        if !field.is_empty() {
            match (!quoted).then(|| number(field, comma_decimal)).flatten() {
                Some(x) => part.set(row, col, kind::NUMBER, 0, x.to_bits()),
                None => {
                    let id = part.strings.push(field);
                    part.set(row, col, kind::LOCAL_STR, 0, id as u64);
                }
            }
        }
        field.clear();
    };
    while i < b.len() {
        let c = b[i];
        if c == b'"' && field.is_empty() && !quoted_field {
            // Campo entre aspas: vai até a aspa que não for dobrada.
            quoted_field = true;
            i += 1;
            let start = i;
            let mut seg = start;
            loop {
                match memchr::memchr(b'"', &b[i..]) {
                    Some(k) => {
                        i += k;
                        if b.get(i + 1) == Some(&b'"') {
                            field.push_str(&text[seg..i + 1]);
                            i += 2;
                            seg = i;
                        } else {
                            field.push_str(&text[seg..i]);
                            i += 1;
                            break;
                        }
                    }
                    None => {
                        field.push_str(&text[seg..]);
                        i = b.len();
                        break;
                    }
                }
            }
            continue;
        }
        if c == sep {
            flush(&mut part, &mut field, quoted_field, row, col);
            quoted_field = false;
            col += 1;
            i += 1;
            continue;
        }
        if c == b'\n' || c == b'\r' {
            flush(&mut part, &mut field, quoted_field, row, col);
            quoted_field = false;
            if c == b'\r' && b.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
            i += 1;
            row += 1;
            col = 0;
            continue;
        }
        // Texto comum até o próximo separador, aspas ou quebra.
        let start = i;
        while i < b.len()
            && b[i] != sep
            && b[i] != b'\n'
            && b[i] != b'\r'
            && !(b[i] == b'"' && i == start && field.is_empty())
        {
            i += 1;
        }
        if i == start {
            i += 1;
        }
        field.push_str(&text[start..i]);
    }
    flush(&mut part, &mut field, quoted_field, row, col);
    let mut sheet = Sheet::new(name);
    sheet.merge_part(part);
    sheet.finish();
    sheet
}

/// Como um CSV foi gravado: separador e codificação (para salvar igual).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvFormat {
    pub sep: u8,
    pub bom: bool,
    /// UTF-8 (senão, Windows-1252).
    pub utf8: bool,
}

impl Default for CsvFormat {
    fn default() -> Self {
        Self { sep: b';', bom: true, utf8: true }
    }
}

/// O formato de um CSV existente.
pub fn detect_format(bytes: &[u8]) -> CsvFormat {
    let bom = bytes.starts_with(b"\xEF\xBB\xBF");
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let (utf8, text) = match std::str::from_utf8(body) {
        Ok(s) => (true, std::borrow::Cow::Borrowed(s)),
        Err(_) => (false, std::borrow::Cow::Owned(windows_1252(body))),
    };
    CsvFormat { sep: detect_separator(&text), bom, utf8 }
}

/// Grava a aba em CSV (valores, não fórmulas, como o Excel). Com `;` como
/// separador, números com vírgula decimal. Grava num temporário e troca.
pub fn write_csv(
    path: &Path,
    sheet: &Sheet,
    shared: &duck_core::StringTable,
    fmt: CsvFormat,
) -> Result<(), Error> {
    use duck_core::Value;
    let sep = fmt.sep as char;
    let mut out = String::with_capacity(sheet.rows() as usize * sheet.cols() as usize * 8);
    for r in 0..sheet.rows() {
        // Sem as colunas vazias do fim da linha.
        let last = (0..sheet.cols()).rev().find(|&c| sheet.has_value(r, c));
        if let Some(last) = last {
            for c in 0..=last {
                if c > 0 {
                    out.push(sep);
                }
                match sheet.cell(r, c, shared).value {
                    Value::Empty => {}
                    Value::Number(x) => {
                        let mut s = String::new();
                        duck_core::numfmt::format_general(x, 15, &mut s);
                        if fmt.sep == b';' {
                            s = s.replace('.', ",");
                        }
                        out.push_str(&s);
                    }
                    Value::Text(t) => {
                        if t.contains([sep, '"', '\n', '\r']) || t.starts_with(' ') || t.ends_with(' ') {
                            out.push('"');
                            out.push_str(&t.replace('"', "\"\""));
                            out.push('"');
                        } else {
                            out.push_str(t);
                        }
                    }
                    Value::Bool(b) => out.push_str(if b { "VERDADEIRO" } else { "FALSO" }),
                    Value::Error(e) => out.push_str(e.as_str()),
                }
            }
        }
        out.push_str("\r\n");
    }
    let mut bytes = Vec::with_capacity(out.len() + 3);
    if fmt.utf8 {
        if fmt.bom {
            bytes.extend_from_slice(b"\xEF\xBB\xBF");
        }
        bytes.extend_from_slice(out.as_bytes());
    } else {
        bytes.extend(out.chars().map(to_windows_1252));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("~{name}.duck-salvando"));
    std::fs::write(&tmp, &bytes).map_err(Error::Io)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Io(e));
    }
    Ok(())
}

/// Um caractere em Windows-1252 (`?` se não existir lá).
fn to_windows_1252(c: char) -> u8 {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    match c as u32 {
        0..=0x7F | 0xA0..=0xFF => c as u32 as u8,
        _ => HIGH.iter().position(|&h| h == c).map_or(b'?', |i| 0x80 + i as u8),
    }
}

/// O separador mais regular nas primeiras linhas (fora de aspas).
fn detect_separator(text: &str) -> u8 {
    const CANDIDATES: [u8; 4] = *b";,\t|";
    let mut counts: Vec<[usize; 4]> = Vec::new();
    let mut cur = [0usize; 4];
    let mut quoted = false;
    for &c in text.as_bytes().iter().take(64 * 1024) {
        match c {
            b'"' => quoted = !quoted,
            b'\n' if !quoted => {
                counts.push(cur);
                cur = [0; 4];
                if counts.len() >= 50 {
                    break;
                }
            }
            _ if !quoted => {
                if let Some(k) = CANDIDATES.iter().position(|&s| s == c) {
                    cur[k] += 1;
                }
            }
            _ => {}
        }
    }
    if cur.iter().any(|&n| n > 0) {
        counts.push(cur);
    }
    // Pontuação: linhas em que o separador aparece com a mesma contagem da primeira.
    let mut best = (0usize, b',');
    for (k, &sep) in CANDIDATES.iter().enumerate() {
        let Some(first) = counts.first().map(|c| c[k]).filter(|&n| n > 0) else { continue };
        let score = counts.iter().filter(|c| c[k] == first).count() * 1000 + first;
        if score > best.0 {
            best = (score, sep);
        }
    }
    best.1
}

/// Número como o Excel entenderia. Com `comma_decimal`: `1.234,56`, `-3,5`.
fn number(s: &str, comma_decimal: bool) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || t.len() > 40 {
        return None;
    }
    let first = t.as_bytes()[0];
    if !(first.is_ascii_digit() || matches!(first, b'-' | b'+' | b'.' | b',')) {
        return None;
    }
    // Zeros à esquerda (códigos, CEPs) viram texto? O Excel converte; aqui também.
    if comma_decimal {
        let mut out = String::with_capacity(t.len());
        let (int, frac) = match t.rfind(',') {
            Some(k) => (&t[..k], Some(&t[k + 1..])),
            None => (t, None),
        };
        if int.contains('.') {
            // Ponto só como milhar: grupos de 3 dígitos.
            let digits = int.trim_start_matches(['-', '+']);
            let groups: Vec<&str> = digits.split('.').collect();
            if groups[0].is_empty() || groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3) {
                return None;
            }
        }
        out.push_str(&int.replace('.', ""));
        if let Some(f) = frac {
            if f.contains(['.', ',']) {
                return None;
            }
            out.push('.');
            out.push_str(f);
        }
        return out.parse().ok().filter(|x: &f64| x.is_finite());
    }
    if t.contains(',') {
        return None;
    }
    t.parse().ok().filter(|x: &f64| x.is_finite())
}

/// Windows-1252 → UTF-8.
fn windows_1252(b: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    b.iter()
        .map(|&c| match c {
            0x80..=0x9F => HIGH[(c - 0x80) as usize],
            c => c as char,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{StringTable, Value};

    #[test]
    fn semicolon_with_comma_decimal() {
        let csv = "Nome;Valor;Obs\r\nAna;1.234,56;\"tem ; e \"\"aspas\"\"\"\r\nBruno;-3,5;\"duas\nlinhas\"\r\n;7;\n";
        let s = parse_csv(csv.as_bytes(), "t");
        let sh = StringTable::new();
        let c = |r, c| s.cell(r, c, &sh).value;
        assert_eq!(c(0, 0), Value::Text("Nome"));
        assert_eq!(c(1, 1), Value::Number(1234.56));
        assert_eq!(c(1, 2), Value::Text("tem ; e \"aspas\""));
        assert_eq!(c(2, 1), Value::Number(-3.5));
        assert_eq!(c(2, 2), Value::Text("duas\nlinhas"));
        assert_eq!(c(3, 0), Value::Empty);
        assert_eq!(c(3, 1), Value::Number(7.0));
        assert_eq!((s.rows(), s.cols()), (4, 3));
    }

    #[test]
    fn comma_and_tab_separators() {
        let s = parse_csv(b"a,b\n1.5,\"2,5\"\n", "t");
        let sh = StringTable::new();
        assert_eq!(s.cell(1, 0, &sh).value, Value::Number(1.5));
        // Entre aspas fica texto.
        assert_eq!(s.cell(1, 1, &sh).value, Value::Text("2,5"));
        let s = parse_csv(b"x\ty\n10\t20\n", "t");
        assert_eq!(s.cell(1, 1, &sh).value, Value::Number(20.0));
    }

    #[test]
    fn write_and_read_back() {
        let src = "Nome;Valor\r\nJoão;1,5\r\n\"a;b\";-3\r\n";
        let mut s = parse_csv(src.as_bytes(), "t");
        s.set_cell(2, 1, duck_core::CellValue::Number(1234.25), None, None);
        let dir = std::env::temp_dir().join(format!("duck-csv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sh = StringTable::new();
        for fmt in
            [CsvFormat { sep: b';', bom: true, utf8: true }, CsvFormat { sep: b';', bom: false, utf8: false }]
        {
            let p = dir.join("t.csv");
            write_csv(&p, &s, &sh, fmt).unwrap();
            let bytes = std::fs::read(&p).unwrap();
            assert_eq!(detect_format(&bytes), fmt);
            let back = parse_csv(&bytes, "t");
            assert_eq!(back.cell(1, 0, &sh).value, Value::Text("João"));
            assert_eq!(back.cell(1, 1, &sh).value, Value::Number(1.5));
            assert_eq!(back.cell(2, 0, &sh).value, Value::Text("a;b"));
            assert_eq!(back.cell(2, 1, &sh).value, Value::Number(1234.25));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn numbers_and_encoding() {
        assert_eq!(number("1.234.567,8", true), Some(1234567.8));
        assert_eq!(number("12.34", true), None);
        assert_eq!(number("1,2,3", true), None);
        assert_eq!(number("abc", true), None);
        assert_eq!(number("1e3", false), Some(1000.0));
        let s = parse_csv(b"Jo\xE3o;\x80 5\n", "t");
        let sh = StringTable::new();
        assert_eq!(s.cell(0, 0, &sh).value, Value::Text("João"));
        assert_eq!(s.cell(0, 1, &sh).value, Value::Text("€ 5"));
        assert_eq!(detect_separator("a;b;c\n1;2;3\n"), b';');
    }
}
