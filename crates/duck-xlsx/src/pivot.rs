//! Leitura das tabelas dinâmicas (F5): caches (`xl/pivotCache/`), registros
//! (sob demanda: um cache pode ter centenas de milhares de linhas) e as
//! definições (`xl/pivotTables/`).

use std::collections::HashMap;

use duck_core::pivot::{
    AxisItem, CacheField, DataField, FieldItem, Group, PItem, PivotCache, PivotField, PivotTable, Records,
    SortType,
};

use crate::package::Package;
use crate::parts;
use crate::xml::{attr, attr_str, find_tag, for_each, inner, parse_u32};

fn num(a: &[u8], k: &[u8]) -> Option<f64> {
    attr(a, k).and_then(|v| std::str::from_utf8(v).ok()?.trim().parse().ok())
}

fn flag(a: &[u8], k: &[u8], default: bool) -> bool {
    match attr(a, k) {
        Some(b"1") | Some(b"true") => true,
        Some(b"0") | Some(b"false") => false,
        _ => default,
    }
}

/// `2026-01-06T00:00:00` → serial.
pub fn iso_date(s: &str) -> Option<f64> {
    let (d, t) = s.split_once('T').unwrap_or((s, ""));
    let p: Vec<i64> = d.split('-').filter_map(|x| x.parse().ok()).collect();
    if p.len() != 3 {
        return None;
    }
    let mut tm = t.split(':').map(|x| x.parse::<f64>().unwrap_or(0.0));
    let (h, mi, se) = (tm.next().unwrap_or(0.0), tm.next().unwrap_or(0.0), tm.next().unwrap_or(0.0));
    Some(
        duck_core::input::serial(p[0] as i32, p[1] as u32, p[2] as u32)
            + (h * 3600.0 + mi * 60.0 + se) / 86400.0,
    )
}

fn item_of(name: &[u8], a: &[u8]) -> PItem {
    let v = || attr_str(a, b"v").unwrap_or_default();
    match name {
        b"n" => PItem::Num(v().parse().unwrap_or(0.0)),
        b"d" => iso_date(&v()).map_or(PItem::Missing, PItem::Date),
        b"b" => PItem::Bool(v() == "1" || v() == "true"),
        b"e" => PItem::Err(v()),
        b"m" => PItem::Missing,
        _ => PItem::Str(v()),
    }
}

/// Filhos diretos simples (`<s v=".."/>` etc.) de um trecho.
fn simple_children(xml: &[u8], mut f: impl FnMut(&[u8], &[u8])) {
    let mut i = 0;
    while let Some(lt) = memchr::memchr(b'<', &xml[i..]).map(|k| i + k) {
        let Some(end) = crate::xml::tag_end(xml, lt) else { break };
        let next = xml.get(lt + 1).copied().unwrap_or(0);
        if next != b'/' && next != b'?' && next != b'!' {
            let ne = (lt + 1..end).find(|&k| crate::xml::is_name_end(xml[k])).unwrap_or(end - 1);
            let self_closing = xml[end - 2] == b'/';
            f(&xml[lt + 1..ne], &xml[ne..if self_closing { end - 2 } else { end - 1 }]);
        }
        i = end;
    }
}

// ---------------------------------------------------------------------------
// Caches
// ---------------------------------------------------------------------------

/// Caches do arquivo (sem os registros).
pub fn read_caches(pkg: &Package, workbook: &str) -> Vec<PivotCache> {
    let Ok(wb) = pkg.read(workbook) else { return Vec::new() };
    let rels = parts::rel_targets(pkg, workbook);
    let mut out = Vec::new();
    for_each(&wb, b"pivotCache", |a, _| {
        let (Some(id), Some(rid)) = (attr(a, b"cacheId").and_then(parse_u32), attr_str(a, b"r:id")) else {
            return;
        };
        let Some(target) = rels.iter().find(|r| r.0 == rid).map(|r| r.1.clone()) else { return };
        if let Ok(x) = pkg.read(&target) {
            let mut c = parse_cache(&x);
            c.id = id;
            c.records_part = parts::rel_targets_typed(pkg, &target)
                .into_iter()
                .find(|r| r.1.ends_with("/pivotCacheRecords"))
                .map(|r| r.2);
            c.part = target;
            out.push(c);
        }
    });
    out
}

fn parse_cache(x: &[u8]) -> PivotCache {
    let mut c = PivotCache::default();
    if let Some(d) = find_tag(x, 0, b"pivotCacheDefinition") {
        c.record_count = num(d.attrs, b"recordCount").unwrap_or(0.0) as usize;
    }
    if let Some(s) = find_tag(x, 0, b"cacheSource") {
        c.external = attr(s.attrs, b"type").is_some_and(|t| t != b"worksheet");
    }
    c.x14_id = find_tag(x, 0, b"x14:pivotCacheDefinition").and_then(|t| attr_str(t.attrs, b"pivotCacheId"));
    for_each(x, b"cacheField", |a, body| {
        let mut f = CacheField {
            name: attr_str(a, b"name").unwrap_or_default(),
            num_fmt: num(a, b"numFmtId").unwrap_or(0.0) as u32,
            virtual_field: !flag(a, b"databaseField", true),
            formula: attr_str(a, b"formula"),
            ..Default::default()
        };
        if let Some(si) = find_tag(body, 0, b"sharedItems")
            && !si.self_closing
        {
            simple_children(inner(body, &si, b"sharedItems"), |n, at| f.items.push(item_of(n, at)));
        }
        if let Some(fg) = find_tag(body, 0, b"fieldGroup") {
            let g = inner(body, &fg, b"fieldGroup");
            let base = num(fg.attrs, b"base").map(|v| v as u32);
            let rule = if let Some(rp) = find_tag(g, 0, b"rangePr") {
                if let Some(by) = attr_str(rp.attrs, b"groupBy").filter(|b| b != "range") {
                    let d = |k: &[u8]| attr_str(rp.attrs, k).and_then(|v| iso_date(&v)).unwrap_or(0.0);
                    Some(Group::Dates { by, start: d(b"startDate"), end: d(b"endDate") })
                } else {
                    Some(Group::Numbers {
                        start: num(rp.attrs, b"startNum").unwrap_or(0.0),
                        end: num(rp.attrs, b"endNum").unwrap_or(0.0),
                        step: num(rp.attrs, b"groupInterval").unwrap_or(1.0),
                    })
                }
            } else if let Some(dp) = find_tag(g, 0, b"discretePr") {
                let mut v = Vec::new();
                simple_children(inner(g, &dp, b"discretePr"), |_, at| {
                    v.push(num(at, b"v").unwrap_or(0.0) as u32)
                });
                Some(Group::Discrete(v))
            } else {
                None
            };
            // Os itens que aparecem são os do grupo; os compartilhados são os
            // valores brutos (guardados para decodificar os registros).
            if let Some(gi) = find_tag(g, 0, b"groupItems") {
                let mut items = Vec::new();
                simple_children(inner(g, &gi, b"groupItems"), |n, at| items.push(item_of(n, at)));
                if let (Some(b), Some(r)) = (base, rule) {
                    f.group = Some((b, r));
                    f.raw = std::mem::replace(&mut f.items, items);
                } else {
                    f.items = items;
                }
            }
        }
        c.fields.push(f);
    });
    c
}

// ---------------------------------------------------------------------------
// Registros
// ---------------------------------------------------------------------------

/// Índice do grupo de um valor bruto.
fn group_index(g: &Group, v: &PItem, n_items: usize) -> u32 {
    let last = n_items.saturating_sub(1) as u32;
    match g {
        Group::Dates { by, start, end } => {
            // Data em branco: o Excel põe no primeiro item ("<início").
            let x = match v {
                PItem::Date(d) | PItem::Num(d) => *d,
                _ => return 0,
            };
            if x < start.floor() {
                return 0;
            }
            if x >= end.floor() + 1.0 {
                return last;
            }
            let Some(dt) = duck_core::numfmt::serial_to_datetime(x, false, 0) else { return last };
            let (_, sy, ..) = duck_core::numfmt::serial_to_datetime(*start, false, 0)
                .map_or((0, 0i32, 0u32), |s| (0, s.year, s.month));
            let idx = match by.as_str() {
                "years" => (dt.year - sy) as u32 + 1,
                "quarters" => (dt.month - 1) / 3 + 1,
                "months" => dt.month,
                "days" => {
                    // Posição num ano bissexto (o grupo tem 29/02).
                    const CUM: [u32; 12] = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
                    CUM[dt.month as usize - 1] + dt.day
                }
                "hours" => dt.hour + 1,
                "minutes" => dt.minute + 1,
                "seconds" => dt.second + 1,
                _ => 0,
            };
            idx.min(last)
        }
        Group::Numbers { start, end, step } => {
            let x = match v {
                PItem::Num(d) | PItem::Date(d) => *d,
                _ => return last,
            };
            if x < *start {
                0
            } else if x > *end {
                last
            } else {
                (((x - start) / step.max(1e-12)).floor() as u32 + 1).min(last)
            }
        }
        Group::Discrete(_) => 0,
    }
}

/// Lê os registros do cache: para cada campo, o índice do item (já no
/// grupo, para campos agrupados). Valores próprios dos registros viram
/// itens novos no campo.
pub fn read_records(pkg: &Package, cache: &mut PivotCache) -> Option<Records> {
    let part = cache.records_part.clone()?;
    let x = pkg.read(&part).ok()?;
    let n_fields = cache.fields.len();
    let real: Vec<usize> = (0..n_fields).filter(|&i| !cache.fields[i].virtual_field).collect();
    let mut cols: Vec<Vec<u32>> = vec![Vec::with_capacity(cache.record_count); n_fields];
    // Itens brutos (os que os registros indexam) e índice para deduplicar os novos.
    let mut raw: Vec<Vec<PItem>> = cache.fields.iter().map(|f| f.raw_items().to_vec()).collect();
    let mut dedupe: Vec<HashMap<String, u32>> = vec![HashMap::new(); n_fields];
    let mut count = 0;
    for_each(&x, b"r", |_, body| {
        let mut k = 0;
        simple_children(body, |n, a| {
            let Some(&fi) = real.get(k) else { return };
            k += 1;
            let idx = if n == b"x" {
                num(a, b"v").unwrap_or(0.0) as u32
            } else {
                let it = item_of(n, a);
                let key = format!("{it:?}");
                let list = &mut raw[fi];
                *dedupe[fi].entry(key).or_insert_with(|| {
                    list.push(it);
                    (list.len() - 1) as u32
                })
            };
            cols[fi].push(idx);
        });
        count += 1;
    });
    // Campos com grupo: índice bruto → índice do grupo (a partir dos índices
    // brutos: um campo base também pode estar agrupado).
    let raw_cols = cols.clone();
    for (fi, col) in cols.iter_mut().enumerate() {
        let Some((base, g)) = cache.fields[fi].group.clone() else { continue };
        let base = base as usize;
        let n_items = cache.fields[fi].items.len();
        let src = raw_cols.get(base).cloned().unwrap_or_default();
        let base_raw = raw.get(base).cloned().unwrap_or_default();
        let mut memo: HashMap<u32, u32> = HashMap::new();
        let mapped: Vec<u32> = src
            .iter()
            .map(|&ri| {
                *memo.entry(ri).or_insert_with(|| match &g {
                    Group::Discrete(map) => map.get(ri as usize).copied().unwrap_or(0),
                    g => group_index(g, base_raw.get(ri as usize).unwrap_or(&PItem::Missing), n_items),
                })
            })
            .collect();
        *col = mapped;
    }
    // Campos sem grupo: itens novos dos registros entram na lista do campo.
    for (f, r) in cache.fields.iter_mut().zip(&raw) {
        if f.group.is_none() && r.len() > f.items.len() {
            f.items = r.clone();
        }
    }
    let grouped_raw = (0..n_fields)
        .filter(|&f| cache.fields[f].group.is_some())
        .map(|f| (f as u32, raw_cols[f].clone()))
        .collect();
    Some(Records { count, cols, grouped_raw })
}

// ---------------------------------------------------------------------------
// Tabelas dinâmicas
// ---------------------------------------------------------------------------

fn axis_items(xml: &[u8], tag: &[u8]) -> Vec<AxisItem> {
    let mut out = Vec::new();
    let Some(t) = find_tag(xml, 0, tag) else { return out };
    if t.self_closing {
        return out;
    }
    for_each(inner(xml, &t, tag), b"i", |a, body| {
        let mut it = AxisItem {
            t: attr_str(a, b"t").filter(|t| t != "data"),
            r: num(a, b"r").unwrap_or(0.0) as u32,
            i: num(a, b"i").unwrap_or(0.0) as u32,
            x: Vec::new(),
        };
        for_each(body, b"x", |xa, _| it.x.push(num(xa, b"v").unwrap_or(0.0) as u32));
        out.push(it);
    });
    out
}

pub fn parse_pivot_table(x: &[u8]) -> Option<PivotTable> {
    let d = find_tag(x, 0, b"pivotTableDefinition")?;
    let a = d.attrs;
    let loc = find_tag(x, 0, b"location")?;
    let range = attr(loc.attrs, b"ref").and_then(crate::sheet::parse_range)?;
    let mut fields = Vec::new();
    if let Some(pf) = find_tag(x, 0, b"pivotFields") {
        let body = inner(x, &pf, b"pivotFields");
        // Cada `pivotField` (pode ser `<pivotField .../>` ou com filhos).
        let mut pos = 0;
        while let Some(t) = find_tag(body, pos, b"pivotField") {
            let inner_b = inner(body, &t, b"pivotField");
            let fa = t.attrs;
            let mut f = PivotField {
                axis: match attr(fa, b"axis") {
                    Some(b"axisRow") => duck_core::pivot::Axis::Row,
                    Some(b"axisCol") => duck_core::pivot::Axis::Col,
                    Some(b"axisPage") => duck_core::pivot::Axis::Page,
                    Some(b"axisValues") => duck_core::pivot::Axis::Values,
                    _ => duck_core::pivot::Axis::None,
                },
                data_field: flag(fa, b"dataField", false),
                show_all: flag(fa, b"showAll", true),
                compact: flag(fa, b"compact", true),
                outline: flag(fa, b"outline", true),
                subtotal_top: flag(fa, b"subtotalTop", true),
                default_subtotal: flag(fa, b"defaultSubtotal", true),
                sort: match attr(fa, b"sortType") {
                    Some(b"ascending") => SortType::Ascending,
                    Some(b"descending") => SortType::Descending,
                    _ => SortType::Manual,
                },
                name: attr_str(fa, b"name"),
                num_fmt: num(fa, b"numFmtId").map(|v| v as u32),
                insert_blank_row: flag(fa, b"insertBlankRow", false),
                ..Default::default()
            };
            if let Some(it) = find_tag(inner_b, 0, b"items") {
                for_each(inner(inner_b, &it, b"items"), b"item", |ia, _| {
                    f.items.push(FieldItem {
                        x: num(ia, b"x").map(|v| v as u32).or_else(|| attr(ia, b"t").is_none().then_some(0)),
                        t: attr_str(ia, b"t"),
                        hidden: flag(ia, b"h", false),
                        collapsed: !flag(ia, b"sd", true),
                        missing: flag(ia, b"m", false),
                        name: attr_str(ia, b"n"),
                    });
                });
            }
            if let Some(ss) = find_tag(inner_b, 0, b"autoSortScope")
                && let Some(r) = find_tag(inner(inner_b, &ss, b"autoSortScope"), 0, b"x")
            {
                f.sort_by_data = num(r.attrs, b"v").map(|v| v as u32);
            }
            fields.push(f);
            pos = if t.self_closing { t.end } else { t.end + inner_b.len() };
        }
    }
    let list = |tag: &[u8]| -> Vec<i32> {
        let mut v = Vec::new();
        if let Some(t) = find_tag(x, 0, tag)
            && !t.self_closing
        {
            for_each(inner(x, &t, tag), b"field", |fa, _| v.push(num(fa, b"x").unwrap_or(0.0) as i32));
        }
        v
    };
    let mut page_fields = Vec::new();
    for_each(x, b"pageField", |pa, _| {
        page_fields.push((num(pa, b"fld").unwrap_or(0.0) as u32, num(pa, b"item").map(|v| v as u32)));
    });
    let mut data_fields = Vec::new();
    for_each(x, b"dataField", |da, _| {
        data_fields.push(DataField {
            name: attr_str(da, b"name").unwrap_or_default(),
            fld: num(da, b"fld").unwrap_or(0.0) as u32,
            subtotal: attr_str(da, b"subtotal").unwrap_or_else(|| "sum".into()),
            show_as: attr_str(da, b"showDataAs"),
            num_fmt: num(da, b"numFmtId").map(|v| v as u32),
        });
    });
    Some(PivotTable {
        part: String::new(),
        name: attr_str(a, b"name").unwrap_or_default(),
        cache_id: num(a, b"cacheId").unwrap_or(0.0) as u32,
        range,
        first_header_row: num(loc.attrs, b"firstHeaderRow").unwrap_or(0.0) as u32,
        first_data_row: num(loc.attrs, b"firstDataRow").unwrap_or(0.0) as u32,
        first_data_col: num(loc.attrs, b"firstDataCol").unwrap_or(0.0) as u32,
        fields,
        row_fields: list(b"rowFields"),
        col_fields: list(b"colFields"),
        page_fields,
        data_fields,
        row_items: axis_items(x, b"rowItems"),
        col_items: axis_items(x, b"colItems"),
        compact: flag(a, b"compact", true),
        outline: flag(a, b"outline", false),
        row_grand: flag(a, b"rowGrandTotals", true),
        col_grand: flag(a, b"colGrandTotals", true),
        data_on_rows: flag(a, b"dataOnRows", false),
        data_caption: attr_str(a, b"dataCaption").unwrap_or_else(|| "Valores".into()),
        grand_caption: attr_str(a, b"grandTotalCaption"),
        row_header_caption: attr_str(a, b"rowHeaderCaption"),
        col_header_caption: attr_str(a, b"colHeaderCaption"),
        style: find_tag(x, 0, b"pivotTableStyleInfo").and_then(|t| attr_str(t.attrs, b"name")),
        show_headers: flag(a, b"showHeaders", true),
        show_error: flag(a, b"showError", false),
        error_caption: attr_str(a, b"errorCaption").unwrap_or_default(),
    })
}

/// Tabelas dinâmicas de uma aba.
pub fn read_pivot_tables(pkg: &Package, sheet_part: &str) -> Vec<PivotTable> {
    let mut out = Vec::new();
    for (_, kind, target) in parts::rel_targets_typed(pkg, sheet_part) {
        if !kind.ends_with("/pivotTable") {
            continue;
        }
        if let Ok(x) = pkg.read(&target)
            && let Some(mut t) = parse_pivot_table(&x)
        {
            t.part = target;
            out.push(t);
        }
    }
    out.sort_by_key(|t| (t.range.first_row, t.range.first_col));
    out
}
