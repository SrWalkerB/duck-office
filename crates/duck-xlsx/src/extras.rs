//! O que o motor de cálculo precisa além das abas: nomes definidos
//! (`<definedName>`), arquivos externos com os valores que o Excel guardou
//! deles (`xl/externalLinks/`) e a data em que o arquivo foi salvo.

use duck_core::{BookExtras, DefinedName, ExternalBook, Sheet, SheetPart, kind};

use crate::package::Package;
use crate::parts::{self, WorkbookMeta};
use crate::sheet::parse_ref;
use crate::xml::{attr, attr_str, find_tag, for_each, inner, parse_u32, unescape_into};

pub fn read_extras(pkg: &Package, meta: &WorkbookMeta) -> BookExtras {
    let mut out = BookExtras::default();
    let Ok(wb) = pkg.read(&meta.path) else { return out };

    // Nomes definidos.
    for_each(&wb, b"definedName", |a, body| {
        let Some(name) = attr_str(a, b"name") else { return };
        let mut formula = String::new();
        unescape_into(body, &mut formula);
        out.names.push(DefinedName {
            name,
            // `localSheetId` conta também as abas de gráfico.
            sheet: attr(a, b"localSheetId")
                .and_then(parse_u32)
                .map(|t| meta.sheets.iter().position(|s| s.tab == t as usize).map_or(u32::MAX, |i| i as u32)),
            formula,
            hidden: attr(a, b"hidden").is_some_and(|v| v == b"1" || v == b"true"),
        });
    });

    // Arquivos externos, na ordem de `<externalReferences>` (o `[n]` das fórmulas).
    let rels = parts::rel_targets(pkg, &meta.path);
    for_each(&wb, b"externalReference", |a, _| {
        let target =
            attr_str(a, b"r:id").and_then(|id| rels.iter().find(|(i, _)| *i == id).map(|r| r.1.clone()));
        let book = target.and_then(|t| pkg.read(&t).ok().map(|x| read_external(&x))).unwrap_or_default();
        out.externals.push(book);
    });

    out.protection = crate::protect::parse_book(&wb);
    out.saved_at = pkg.read("docProps/core.xml").ok().and_then(|x| saved_at(&x));
    out
}

/// `<dcterms:modified>2026-11-18T23:42:10Z</dcterms:modified>` → serial
/// (UTC, sem fuso: só o dia importa para `HOJE()`).
fn saved_at(xml: &[u8]) -> Option<f64> {
    let t = find_tag(xml, 0, b"dcterms:modified")?;
    let s = std::str::from_utf8(inner(xml, &t, b"dcterms:modified")).ok()?;
    let (date, time) = s.split_once('T').unwrap_or((s, ""));
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, dd) = (d.next()??, d.next()??, d.next()??);
    let mut tm = time.trim_end_matches('Z').split(':').map(|p| p.parse::<f64>().unwrap_or(0.0));
    let (h, mi, se) = (tm.next().unwrap_or(0.0), tm.next().unwrap_or(0.0), tm.next().unwrap_or(0.0));
    Some(duck_core::input::serial(y as i32, m as u32, dd as u32) + (h * 3600.0 + mi * 60.0 + se) / 86400.0)
}

fn read_external(xml: &[u8]) -> ExternalBook {
    let mut book = ExternalBook::default();
    for_each(xml, b"sheetName", |a, _| {
        book.sheets.push(Sheet::new(attr_str(a, b"val").unwrap_or_default()));
    });
    for_each(xml, b"definedName", |a, _| {
        if let (Some(n), Some(r)) = (attr_str(a, b"name"), attr_str(a, b"refersTo")) {
            book.names.push((n, r.trim_start_matches('=').to_string()));
        }
    });
    for_each(xml, b"sheetData", |a, body| {
        let Some(id) = attr(a, b"sheetId").and_then(parse_u32) else { return };
        let Some(sheet) = book.sheets.get_mut(id as usize) else { return };
        let mut part = SheetPart::new();
        let mut text = String::new();
        for_each(body, b"cell", |ca, cb| {
            let Some((col, Some(row))) = attr(ca, b"r").and_then(parse_ref) else { return };
            let v = find_tag(cb, 0, b"v").map(|t| inner(cb, &t, b"v")).unwrap_or(b"");
            text.clear();
            unescape_into(v, &mut text);
            match attr(ca, b"t").unwrap_or(b"n") {
                b"b" => part.set(row, col, kind::BOOL, 0, (text.trim() == "1") as u64),
                b"e" => {
                    part.set(row, col, kind::ERROR, 0, duck_core::CellError::parse(text.as_bytes()) as u64)
                }
                b"n" => {
                    if let Ok(x) = text.trim().parse::<f64>() {
                        part.set(row, col, kind::NUMBER, 0, x.to_bits());
                    }
                }
                _ => {
                    let id = part.strings.push(&text);
                    part.set(row, col, kind::LOCAL_STR, 0, id as u64);
                }
            }
        });
        let name = std::mem::take(&mut sheet.name);
        let mut s = Sheet::new(name);
        s.merge_part(part);
        s.finish();
        *sheet = s;
    });
    book
}
