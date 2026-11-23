//! Leitura do Modelo de Dados: `xl/model/item.data` é um backup do Analysis
//! Services (ABF) com as partes comprimidas em Xpress (LZ77, MS-XCA). Só as
//! definições (tabelas, colunas calculadas, medidas) são lidas; os dados vêm
//! das tabelas das abas a que o modelo está ligado.

use std::collections::HashMap;

use duck_core::cube::{
    CubeCache, CubeField, CubePivot, Hierarchy, ModelColumn, ModelDef, ModelKind, ModelTable,
};

use crate::drawing::{child, children, num_attr};
use crate::package::Package;
use crate::xml::{attr, attr_str, find_tag, for_each, inner, unescape_into};

/// LZ77 simples do MS-XCA (2.4).
pub fn xpress(d: &[u8], size: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size);
    let mut p = 0;
    let mut flags = 0u32;
    let mut left = 0;
    // Posição do meio byte de comprimento guardado.
    let mut half: Option<usize> = None;
    let u16_at = |p: usize| Some(u16::from_le_bytes(d.get(p..p + 2)?.try_into().ok()?) as usize);
    while out.len() < size {
        if left == 0 {
            flags = u32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?);
            p += 4;
            left = 32;
        }
        left -= 1;
        if flags >> left & 1 == 0 {
            out.push(*d.get(p)?);
            p += 1;
            continue;
        }
        if p >= d.len() {
            break;
        }
        let m = u16_at(p)?;
        p += 2;
        let off = (m >> 3) + 1;
        let mut len = m & 7;
        if len == 7 {
            len = match half.take() {
                None => {
                    half = Some(p);
                    p += 1;
                    (*d.get(p - 1)? & 15) as usize
                }
                Some(h) => (d[h] >> 4) as usize,
            };
            if len == 15 {
                len = *d.get(p)? as usize;
                p += 1;
                if len == 255 {
                    len = u16_at(p)?;
                    p += 2;
                    if len == 0 {
                        len = u32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?) as usize;
                        p += 4;
                    }
                    len = len.checked_sub(22)?;
                }
                len += 15;
            }
            len += 7;
        }
        len += 3;
        if off > out.len() {
            return None;
        }
        for _ in 0..len {
            out.push(out[out.len() - off]);
        }
    }
    out.truncate(size);
    Some(out)
}

/// Uma parte do backup: blocos com (tamanho, tamanho comprimido) e 4 bytes
/// de conferência no fim.
fn unpack(raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let end = raw.len().checked_sub(4)?;
    let mut p = 0;
    while p + 4 <= end {
        let n = |i: usize| {
            let v = u16::from_le_bytes([raw[i], raw[i + 1]]) as usize;
            if v == 0 { 65_536 } else { v }
        };
        let (size, packed) = (n(p), n(p + 2));
        p += 4;
        let block = raw.get(p..p + packed)?;
        p += packed;
        if size == packed {
            out.extend_from_slice(block);
        } else {
            out.extend(xpress(block, size)?);
        }
    }
    Some(out)
}

fn utf16(b: &[u8]) -> String {
    let b = b.strip_prefix(&[0xFF, 0xFE]).unwrap_or(b);
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
}

fn tag_text<'a>(s: &'a str, tag: &str) -> Option<&'a str> {
    let a = s.find(&format!("<{tag}>"))? + tag.len() + 2;
    Some(&s[a..a + s[a..].find(&format!("</{tag}>"))?])
}

/// Textos de `<BackupFile>`: um por arquivo.
fn files(s: &str) -> impl Iterator<Item = &str> {
    s.split("<BackupFile>").skip(1).map(|f| f.split("</BackupFile>").next().unwrap_or(f))
}

/// Partes XML do backup cujos nomes terminam em um dos sufixos.
fn metadata(d: &[u8], suffixes: &[&str]) -> Option<Vec<(String, Vec<u8>)>> {
    let head = utf16(d.get(..4096.min(d.len()))?);
    let head = &head[head.find("<BackupLog>")?..];
    let off: usize = tag_text(head, "m_cbOffsetHeader")?.parse().ok()?;
    let size: usize = tag_text(head, "DataSize")?.parse().ok()?;
    if tag_text(head, "EncryptionFlag") == Some("true") {
        return None;
    }
    let compressed = tag_text(head, "ApplyCompression") == Some("true");
    let dir = utf16(d.get(off..off.checked_add(size)?)?);
    let mut place: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut last = None;
    for f in files(&dir) {
        let path = tag_text(f, "Path")?;
        let at = (
            tag_text(f, "m_cbOffsetHeader")?.parse::<usize>().ok()?,
            tag_text(f, "Size")?.parse::<usize>().ok()?,
        );
        place.insert(path, at);
        last = Some(at);
    }
    // O último arquivo é o índice: nome real → nome no backup.
    let (o, n) = last?;
    let log = utf16(d.get(o..o.checked_add(n)?)?);
    let mut out = Vec::new();
    for f in files(&log) {
        let (Some(path), Some(store)) = (tag_text(f, "Path"), tag_text(f, "StoragePath")) else { continue };
        if !suffixes.iter().any(|s| path.ends_with(s)) {
            continue;
        }
        let Some(&(o, n)) = place.get(store) else { continue };
        let raw = d.get(o..o.checked_add(n)?)?;
        let data = if compressed { unpack(raw)? } else { raw.to_vec() };
        let data = if data.starts_with(&[0xFF, 0xFE]) { utf16(&data).into_bytes() } else { data };
        out.push((path.to_string(), data));
    }
    Some(out)
}

fn text_of(xml: &[u8], name: &[u8]) -> Option<String> {
    let e = child(xml, name)?;
    let mut s = String::new();
    unescape_into(e.body, &mut s);
    Some(s)
}

fn table(xml: &[u8]) -> Option<ModelTable> {
    let d = find_tag(xml, 0, b"Dimension")?;
    let body = inner(xml, &d, b"Dimension");
    let mut t = ModelTable { name: text_of(body, b"Name")?, ..Default::default() };
    let attrs = child(body, b"Attributes")?;
    // Colunas pela identificação interna (a ordem aponta por ela).
    let mut ids: HashMap<String, String> = HashMap::new();
    for a in children(attrs.body).into_iter().filter(|a| a.name == b"Attribute") {
        if let (Some(id), Some(n)) = (text_of(a.body, b"ID"), text_of(a.body, b"Name")) {
            ids.insert(id, n);
        }
    }
    for a in children(attrs.body).into_iter().filter(|a| a.name == b"Attribute") {
        let name = text_of(a.body, b"Name")?;
        // A chave interna de linhas.
        if text_of(a.body, b"Type").as_deref() == Some("RowNumber") {
            continue;
        }
        let mut kind = ModelKind::Any;
        for part in [b"NameColumn".as_slice(), b"KeyColumn"] {
            if let Some(k) = find_tag(a.body, 0, part) {
                kind = match text_of(inner(a.body, &k, part), b"DataType").as_deref() {
                    Some("WChar") => ModelKind::Text,
                    Some("Double" | "BigInt" | "Currency" | "Integer" | "Decimal") => ModelKind::Number,
                    Some("Date") => ModelKind::Date,
                    Some("Boolean") => ModelKind::Bool,
                    _ => kind,
                };
            }
        }
        let mut expr = None;
        if let Some(e) = find_tag(a.body, 0, b"Expression") {
            let mut s = String::new();
            unescape_into(inner(a.body, &e, b"Expression"), &mut s);
            expr = Some(s).filter(|s| !s.trim().is_empty());
        }
        let sort_by = text_of(a.body, b"OrderByAttributeID").and_then(|id| ids.get(&id).cloned());
        t.columns.push(ModelColumn { name, kind, expr, sort_by });
    }
    Some(t)
}

/// O Modelo de Dados do arquivo, se houver.
pub fn read_model(pkg: &Package, workbook: &str) -> Option<ModelDef> {
    let data = pkg.read("xl/model/item.data").ok()?;
    let mut m = ModelDef::default();
    for (path, x) in metadata(&data, &[".dim.xml", ".scr.xml"])? {
        if path.ends_with(".dim.xml") {
            m.tables.extend(table(&x));
        } else {
            let mut s = String::new();
            unescape_into(&x, &mut s);
            m.measures.extend(duck_dax_measures(&s));
        }
    }
    // Relações e de qual tabela do Excel vem cada tabela do modelo.
    let wb = pkg.read(workbook).ok()?;
    for_each(&wb, b"x15:modelRelationship", |a, _| {
        let g = |k: &[u8]| attr_str(a, k).unwrap_or_default();
        m.rels.push((g(b"fromTable"), g(b"fromColumn"), g(b"toTable"), g(b"toColumn")));
    });
    let mut names: HashMap<String, String> = HashMap::new();
    for_each(&wb, b"definedName", |a, body| {
        if let Some(n) = attr_str(a, b"name") {
            let mut f = String::new();
            unescape_into(body, &mut f);
            names.insert(n, f);
        }
    });
    // Conexões do tipo "tabela da pasta": id da tabela do modelo → origem.
    let mut sources: HashMap<String, String> = HashMap::new();
    for (_, kind, target) in crate::parts::rel_targets_typed(pkg, workbook) {
        if !kind.ends_with("/connections") {
            continue;
        }
        let Ok(x) = pkg.read(&target) else { continue };
        for_each(&x, b"x15:connection", |a, body| {
            if let (Some(id), Some(r)) = (attr_str(a, b"id"), find_tag(body, 0, b"x15:rangePr"))
                && let Some(n) = attr_str(r.attrs, b"sourceName")
                && let Some(f) = names.get(&n)
            {
                sources.insert(id, f.split('[').next().unwrap_or(f).to_string());
            }
        });
    }
    for_each(&wb, b"x15:modelTable", |a, _| {
        let (Some(id), Some(name)) = (attr_str(a, b"id"), attr_str(a, b"name")) else { return };
        if let Some(t) = m.tables.iter_mut().find(|t| t.name == name) {
            t.source = sources.get(&id).cloned();
        }
    });
    (!m.tables.is_empty()).then_some(m)
}

/// `CREATE MEASURE` do script (o analisador completo fica no `duck-dax`; aqui
/// só o texto passa adiante).
fn duck_dax_measures(s: &str) -> Vec<(String, String, String)> {
    vec![(String::new(), String::new(), s.to_string())]
}

/// Hierarquias e campos do cache de uma dinâmica do modelo.
pub fn read_cube_cache(pkg: &Package, part: &str) -> Option<CubeCache> {
    let x = pkg.read(part).ok()?;
    let mut c = CubeCache::default();
    for_each(&x, b"cacheField", |a, _| {
        c.fields.push(CubeField {
            name: attr_str(a, b"name").unwrap_or_default(),
            hierarchy: num_attr(a, b"hierarchy").map(|v| v as u32),
            caption: attr_str(a, b"caption").unwrap_or_default(),
        })
    });
    for_each(&x, b"cacheHierarchy", |a, _| {
        c.hierarchies.push(Hierarchy {
            unique: attr_str(a, b"uniqueName").unwrap_or_default(),
            caption: attr_str(a, b"caption").unwrap_or_default(),
            measure: attr(a, b"measure") == Some(b"1"),
        })
    });
    (!c.hierarchies.is_empty()).then_some(c)
}

/// Filtros próprios de uma dinâmica do modelo (`pivotHierarchies`).
pub fn parse_cube_pivot(x: &[u8]) -> Option<CubePivot> {
    let ph = find_tag(x, 0, b"pivotHierarchies")?;
    let mut c = CubePivot::default();
    for_each(x, b"pageField", |a, _| {
        c.pages.push((
            num_attr(a, b"fld").unwrap_or(0.0) as u32,
            num_attr(a, b"hier").unwrap_or(-1.0) as i32,
            attr_str(a, b"name"),
        ));
    });
    for (i, h) in children(inner(x, &ph, b"pivotHierarchies")).into_iter().enumerate() {
        let mut list = Vec::new();
        for_each(h.body, b"member", |a, _| list.extend(attr_str(a, b"name")));
        if !list.is_empty() {
            c.members.push((i as u32, list));
        }
    }
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xpress_blocks() {
        // Exemplos do MS-XCA 3.1 (LZ77 simples).
        let plain = [
            0x3f, 0x00, 0x00, 0x00, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c,
            0x6d, 0x6e, 0x6f, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
        ];
        assert_eq!(xpress(&plain, 26).unwrap(), b"abcdefghijklmnopqrstuvwxyz");
        let abc = [0xff, 0xff, 0xff, 0x1f, 0x61, 0x62, 0x63, 0x17, 0x00, 0x0f, 0xff, 0x26, 0x01];
        assert_eq!(xpress(&abc, 300).unwrap(), b"abc".repeat(100));
        assert_eq!(xpress(&[1, 2], 10), None);
    }

    #[test]
    fn pivot_members() {
        let x = br#"<d><pageFields><pageField fld="4" hier="45" name="[T].[c].&amp;[x]" cap="x"/></pageFields><pivotHierarchies count="3"><pivotHierarchy/><pivotHierarchy multipleItemSelectionAllowed="1"><members count="1" level="1"><member name="[T].[c].&amp;[a]"/></members></pivotHierarchy><pivotHierarchy/></pivotHierarchies></d>"#;
        let c = parse_cube_pivot(x).unwrap();
        assert_eq!(c.pages, [(4, 45, Some("[T].[c].&[x]".to_string()))]);
        assert_eq!(c.members, [(1, vec!["[T].[c].&[a]".to_string()])]);
        assert_eq!(parse_cube_pivot(b"<d/>"), None);
    }
}
