//! Leitura do `vbaProject.bin`: um arquivo composto (CFB, MS-CFB) com os
//! módulos comprimidos (MS-OVBA 2.4.1). Só lê o código-fonte.

/// Um módulo do projeto: nome do fluxo e código sem os `Attribute`.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub name: String,
    pub source: String,
}

fn u16_at(b: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as usize)
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

const END: u32 = 0xFFFF_FFFA;

/// Fluxos de um arquivo composto: (nome, conteúdo).
pub fn streams(b: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    if !b.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return None;
    }
    let ss = 1usize << u16_at(b, 30)?.min(16);
    let mss = 1usize << u16_at(b, 32)?.min(12);
    let nfat = u32_at(b, 44)? as usize;
    let dir0 = u32_at(b, 48)?;
    let cutoff = u32_at(b, 56)? as usize;
    let minifat0 = u32_at(b, 60)?;
    let sector = |i: u32| b.get((i as usize + 1) * ss..((i as usize + 2) * ss).min(b.len()));
    let words =
        |s: &[u8]| -> Vec<u32> { s.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect() };
    // FAT: as 109 entradas do cabeçalho e os setores DIFAT.
    let mut difat: Vec<u32> = words(b.get(76..512)?);
    let mut next = u32_at(b, 68)?;
    let mut guard = 0;
    while next < END && guard < 10_000 {
        let s = words(sector(next)?);
        let (last, list) = s.split_last()?;
        difat.extend_from_slice(list);
        next = *last;
        guard += 1;
    }
    let mut fat: Vec<u32> = Vec::new();
    for &s in difat.iter().take(nfat) {
        if s < END {
            fat.extend(words(sector(s)?));
        }
    }
    let chain = |start: u32| -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut i = start;
        let mut n = 0;
        while i < END {
            out.extend_from_slice(sector(i)?);
            i = *fat.get(i as usize)?;
            n += 1;
            if n > fat.len() {
                return None;
            }
        }
        Some(out)
    };
    let dir = chain(dir0)?;
    let minifat = if minifat0 < END { words(&chain(minifat0)?) } else { Vec::new() };
    let mut root = Vec::new();
    let mut out = Vec::new();
    for e in dir.as_chunks::<128>().0 {
        let len = u16_at(e, 64)?;
        if !(2..=64).contains(&len) {
            continue;
        }
        let name: Vec<u16> = e[..len - 2].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let name = String::from_utf16_lossy(&name);
        let (start, size) = (u32_at(e, 116)?, u32_at(e, 120)? as usize);
        match e[66] {
            5 => root = if start < END { chain(start)? } else { Vec::new() },
            2 if size >= cutoff => {
                let mut d = chain(start)?;
                d.truncate(size);
                out.push((name, d));
            }
            2 => {
                let mut d = Vec::new();
                let mut i = start;
                while i < END && d.len() < size {
                    let a = i as usize * mss;
                    d.extend_from_slice(root.get(a..(a + mss).min(root.len()))?);
                    i = *minifat.get(i as usize)?;
                }
                d.truncate(size);
                out.push((name, d));
            }
            _ => {}
        }
    }
    Some(out)
}

/// Descomprime um contêiner do MS-OVBA.
pub fn decompress(d: &[u8]) -> Option<Vec<u8>> {
    if d.first() != Some(&1) {
        return None;
    }
    let mut out = Vec::new();
    let mut p = 1;
    while p + 2 <= d.len() {
        let h = u16_at(d, p)?;
        let end = (p + (h & 0xFFF) + 3).min(d.len());
        p += 2;
        let start = out.len();
        if h >> 15 == 0 {
            out.extend_from_slice(&d[p..end]);
            p = end;
            continue;
        }
        while p < end {
            let flags = d[p];
            p += 1;
            for bit in 0..8 {
                if p >= end {
                    break;
                }
                if (flags >> bit) & 1 == 0 {
                    out.push(d[p]);
                    p += 1;
                    continue;
                }
                let t = u16_at(d, p)?;
                p += 2;
                let diff = out.len() - start;
                let mut bits = 4;
                while (1usize << bits) < diff {
                    bits += 1;
                }
                let len = (t & (0xFFFF >> bits)) + 3;
                let off = (t >> (16 - bits)) + 1;
                if off > out.len() {
                    return None;
                }
                for _ in 0..len {
                    out.push(out[out.len() - off]);
                }
            }
        }
    }
    Some(out)
}

/// Texto em Windows-1252 (a página de código do Excel em português).
fn cp1252(b: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    b.iter()
        .map(|&c| match c {
            0x80..=0x9F => HIGH[(c - 0x80) as usize],
            c => c as char,
        })
        .collect()
}

/// Módulos com código de um `vbaProject.bin`.
pub fn modules(bin: &[u8]) -> Vec<Module> {
    let mut out = Vec::new();
    for (name, data) in streams(bin).unwrap_or_default() {
        let lower = name.to_ascii_lowercase();
        if matches!(lower.as_str(), "dir" | "project" | "projectwm" | "_vba_project")
            || name.starts_with("__SRP")
        {
            continue;
        }
        // O código comprimido vem depois do cache de desempenho: procura o
        // contêiner que começa com "Attribut".
        let found = (0..data.len().saturating_sub(12)).find(|&i| {
            data[i] == 1
                && data[i + 3] == 0
                && data[i + 4..].starts_with(b"Attribut")
                && data[i + 2] >> 4 == 0xB
        });
        let Some(src) = found.and_then(|i| decompress(&data[i..])) else { continue };
        let text = cp1252(&src).replace("\r\n", "\n");
        let source: String =
            text.lines().filter(|l| !l.starts_with("Attribute ")).map(|l| format!("{l}\n")).collect();
        if source.trim().is_empty() {
            continue;
        }
        out.push(Module { name, source });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ovba_example() {
        // Exemplo 3.2.2 da especificação MS-OVBA.
        let data = [
            0x01, 0x2F, 0xB0, 0x00, 0x23, 0x61, 0x61, 0x61, 0x62, 0x63, 0x64, 0x65, 0x82, 0x66, 0x00, 0x70,
            0x61, 0x67, 0x68, 0x69, 0x6A, 0x01, 0x38, 0x08, 0x61, 0x6B, 0x6C, 0x00, 0x30, 0x6D, 0x6E, 0x6F,
            0x70, 0x06, 0x71, 0x02, 0x70, 0x04, 0x10, 0x72, 0x73, 0x74, 0x75, 0x76, 0x10, 0x77, 0x78, 0x79,
            0x7A, 0x00, 0x3C,
        ];
        assert_eq!(decompress(&data).unwrap(), b"#aaabcdefaaaaghijaaaaaklaaamnopqaaaaaaaaaaaarstuvwxyzaaa");
        assert!(modules(b"lixo").is_empty());
    }

    #[test]
    fn code_page() {
        assert_eq!(cp1252(b"Implanta\xE7\xE3o \x80"), "Implantação €");
    }
}
