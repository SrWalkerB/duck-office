//! Remendo das tabelas dinâmicas e das segmentações de dados ao salvar (F5):
//! itens escondidos, área e as linhas/colunas do resultado
//! (`rowItems`/`colItems`, no formato que o Excel grava), e a seleção de
//! cada segmentação. O resto das partes fica como está.

use std::collections::HashMap;

use duck_core::pivot::{AxisItem, PivotFilter, PivotTable};
use memchr::memmem;

use super::cellxml::{cell_ref, escape_attr};
use crate::xml::{find_tag, for_each_attr, tag_end};

/// Reescreve uma tag de abertura mudando/tirando atributos.
fn retag(tag_src: &[u8], set: &[(&[u8], Option<Vec<u8>>)]) -> Vec<u8> {
    // tag_src = "<nome ...>" ou "<nome .../>"
    let self_closing = tag_src.ends_with(b"/>");
    let name_end = tag_src.iter().position(|&b| b == b' ' || b == b'/' || b == b'>').unwrap_or(tag_src.len());
    let attrs_end = if self_closing { tag_src.len() - 2 } else { tag_src.len() - 1 };
    let attrs = &tag_src[name_end..attrs_end.max(name_end)];
    let mut out = tag_src[..name_end].to_vec();
    let mut done = vec![false; set.len()];
    for_each_attr(attrs, |k, whole| {
        if let Some(i) = set.iter().position(|(n, _)| *n == k) {
            done[i] = true;
            if let Some(v) = &set[i].1 {
                out.push(b' ');
                out.extend_from_slice(k);
                out.extend_from_slice(b"=\"");
                out.extend_from_slice(v);
                out.push(b'"');
            }
        } else {
            out.push(b' ');
            out.extend_from_slice(whole);
        }
    });
    for (i, (k, v)) in set.iter().enumerate() {
        if !done[i]
            && let Some(v) = v
        {
            out.push(b' ');
            out.extend_from_slice(k);
            out.extend_from_slice(b"=\"");
            out.extend_from_slice(v);
            out.push(b'"');
        }
    }
    out.extend_from_slice(if self_closing { b"/>" } else { b">" });
    out
}

fn axis_xml(tag: &str, items: &[AxisItem]) -> Vec<u8> {
    let mut s = format!("<{tag} count=\"{}\">", items.len());
    for it in items {
        s.push_str("<i");
        if let Some(t) = &it.t {
            s.push_str(&format!(" t=\"{t}\""));
        }
        if it.r > 0 {
            s.push_str(&format!(" r=\"{}\"", it.r));
        }
        if it.i > 0 {
            s.push_str(&format!(" i=\"{}\"", it.i));
        }
        if it.x.is_empty() {
            s.push_str("/>");
            continue;
        }
        s.push('>');
        for &x in &it.x {
            if x == 0 {
                s.push_str("<x/>");
            } else {
                s.push_str(&format!("<x v=\"{x}\"/>"));
            }
        }
        s.push_str("</i>");
    }
    s.push_str(&format!("</{tag}>"));
    s.into_bytes()
}

/// Troca o elemento `<name>…</name>` (ou `<name/>`) inteiro; se não existir,
/// insere antes de `before`.
fn replace_or_insert(xml: &[u8], name: &[u8], new: &[u8], before: &[&[u8]]) -> Vec<u8> {
    if let Some(t) = find_tag(xml, 0, name) {
        let end = if t.self_closing {
            t.end
        } else {
            let mut close = b"</".to_vec();
            close.extend_from_slice(name);
            close.push(b'>');
            memmem::find(&xml[t.end..], &close).map_or(xml.len(), |i| t.end + i + close.len())
        };
        return [&xml[..t.start], new, &xml[end..]].concat();
    }
    for b in before {
        if let Some(t) = find_tag(xml, 0, b) {
            return [&xml[..t.start], new, &xml[t.start..]].concat();
        }
    }
    xml.to_vec()
}

pub fn patch_pivot_table(xml: &[u8], t: &PivotTable) -> Vec<u8> {
    let mut out = xml.to_vec();
    // Área.
    if let Some(loc) = find_tag(&out, 0, b"location") {
        let mut r = Vec::new();
        cell_ref(t.range.first_row, t.range.first_col, &mut r);
        r.push(b':');
        cell_ref(t.range.last_row, t.range.last_col, &mut r);
        let new = retag(&out[loc.start..loc.end], &[(b"ref", Some(r))]);
        out = [&out[..loc.start], &new[..], &out[loc.end..]].concat();
    }
    // Itens escondidos, campo a campo.
    if let Some(pfs) = find_tag(&out, 0, b"pivotFields") {
        let mut pos = pfs.end;
        let mut fi = 0usize;
        let mut rebuilt: Vec<u8> = out[..pos].to_vec();
        while let Some(pf) = find_tag(&out, pos, b"pivotField") {
            rebuilt.extend_from_slice(&out[pos..pf.end]);
            pos = pf.end;
            if pf.self_closing {
                fi += 1;
                continue;
            }
            let close = memmem::find(&out[pf.end..], b"</pivotField>").map_or(out.len(), |i| pf.end + i);
            // Itens deste campo.
            let mut k = 0usize;
            let mut q = pos;
            while let Some(it) = find_tag(&out[..close], q, b"item") {
                rebuilt.extend_from_slice(&out[q..it.start]);
                let model = t.fields.get(fi).and_then(|f| f.items.get(k));
                let tag = &out[it.start..it.end];
                match model {
                    Some(m) if m.t.is_none() => {
                        let h: Option<Vec<u8>> = m.hidden.then(|| b"1".to_vec());
                        rebuilt.extend_from_slice(&retag(tag, &[(b"h", h)]));
                    }
                    _ => rebuilt.extend_from_slice(tag),
                }
                q = it.end;
                k += 1;
            }
            rebuilt.extend_from_slice(&out[q..close]);
            pos = close;
            fi += 1;
            if find_tag(&out, pos, b"pivotField").is_none_or(|n| {
                memmem::find(&out[pos..], b"</pivotFields>").is_some_and(|e| pos + e < n.start)
            }) {
                break;
            }
        }
        rebuilt.extend_from_slice(&out[pos..]);
        out = rebuilt;
    }
    // Linhas e colunas do resultado.
    out = replace_or_insert(
        &out,
        b"rowItems",
        &axis_xml("rowItems", &t.row_items),
        &[b"colFields", b"colItems", b"pageFields", b"dataFields"],
    );
    out = replace_or_insert(
        &out,
        b"colItems",
        &axis_xml("colItems", &t.col_items),
        &[b"pageFields", b"dataFields", b"formats"],
    );
    patch_filters(&out, &t.filters)
}

fn attr_into(out: &mut Vec<u8>, k: &str, v: &str) {
    out.push(b' ');
    out.extend_from_slice(k.as_bytes());
    out.extend_from_slice(b"=\"");
    escape_attr(v, out);
    out.push(b'"');
}

/// Um filtro escrito a partir do modelo (criado ou mudado).
fn filter_xml(f: &PivotFilter) -> Vec<u8> {
    let mut o = b"<filter".to_vec();
    attr_into(&mut o, "fld", &f.fld.to_string());
    attr_into(&mut o, "type", &f.kind);
    attr_into(&mut o, "evalOrder", &f.eval_order.to_string());
    attr_into(&mut o, "id", &f.id.to_string());
    if f.by_value() {
        attr_into(&mut o, "iMeasureFld", &f.measure.to_string());
    }
    if let Some(n) = &f.name {
        attr_into(&mut o, "name", n);
    }
    o.extend_from_slice(br#"><autoFilter ref="A1"><filterColumn colId="0">"#);
    if let Some((top, n)) = f.top {
        o.extend_from_slice(b"<top10");
        if !top {
            o.extend_from_slice(br#" top="0""#);
        }
        if f.kind == "percent" {
            o.extend_from_slice(br#" percent="1""#);
        }
        attr_into(&mut o, "val", &n.to_string());
        o.extend_from_slice(b"/>");
    } else {
        o.extend_from_slice(b"<customFilters");
        if f.and {
            o.extend_from_slice(br#" and="1""#);
        }
        o.push(b'>');
        for (op, v) in &f.custom {
            o.extend_from_slice(b"<customFilter");
            if op != "equal" {
                attr_into(&mut o, "operator", op);
            }
            attr_into(&mut o, "val", v);
            o.extend_from_slice(b"/>");
        }
        o.extend_from_slice(b"</customFilters>");
    }
    o.extend_from_slice(b"</filterColumn></autoFilter>");
    if f.whole_day {
        o.extend_from_slice(
            br#"<extLst><ext uri="{0605FD5F-26C8-4aeb-8148-2DB25E43C511}" xmlns:x15="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main"><x15:pivotFilter useWholeDay="1"/></ext></extLst>"#,
        );
    }
    o.extend_from_slice(b"</filter>");
    o
}

/// `<filters>` a partir do modelo: os que não mudaram saem como estavam.
fn patch_filters(xml: &[u8], filters: &[PivotFilter]) -> Vec<u8> {
    let cur = find_tag(xml, 0, b"filters");
    let unchanged = || {
        let mut n = 0;
        crate::xml::for_each(xml, b"filter", |_, _| n += 1);
        n == filters.len() && filters.iter().all(|f| f.raw.is_some())
    };
    if cur.is_some() && unchanged() || cur.is_none() && filters.is_empty() {
        return xml.to_vec();
    }
    let mut block = Vec::new();
    if !filters.is_empty() {
        block.extend_from_slice(format!("<filters count=\"{}\">", filters.len()).as_bytes());
        for f in filters {
            match &f.raw {
                Some(r) => block.extend_from_slice(r.as_bytes()),
                None => block.extend_from_slice(&filter_xml(f)),
            }
        }
        block.extend_from_slice(b"</filters>");
    }
    if cur.is_some() {
        return replace_or_insert(xml, b"filters", &block, &[]);
    }
    // Novo: depois do estilo da tabela (ordem do esquema), ou antes do fim.
    let at = match find_tag(xml, 0, b"pivotTableStyleInfo") {
        Some(t) if t.self_closing => t.end,
        Some(t) => memmem::find(&xml[t.end..], b"</pivotTableStyleInfo>").map_or(t.end, |i| t.end + i + 22),
        None => memmem::rfind(xml, b"</pivotTableDefinition>").unwrap_or(xml.len()),
    };
    [&xml[..at], &block[..], &xml[at..]].concat()
}

/// Data (serial) como o Excel grava nas linhas do tempo.
fn iso(serial: f64) -> String {
    let d = duck_core::numfmt::serial_to_datetime(serial, false, 0);
    d.map_or_else(String::new, |d| format!("{:04}-{:02}-{:02}T00:00:00", d.year, d.month, d.day))
}

/// Seleção de uma linha do tempo (`<state><selection>`).
pub fn patch_timeline_cache(xml: &[u8], sel: Option<(f64, f64)>) -> Vec<u8> {
    let Some(st) = find_tag(xml, 0, b"state") else { return xml.to_vec() };
    let ty: Option<Vec<u8>> = sel.map(|_| b"dateBetween".to_vec());
    let new_state = retag(&xml[st.start..st.end], &[(b"filterType", ty)]);
    let mut out = [&xml[..st.start], &new_state[..], &xml[st.end..]].concat();
    let new_sel = sel.map(|(a, b)| format!(r#"<selection startDate="{}" endDate="{}"/>"#, iso(a), iso(b)));
    if let Some(s) = find_tag(&out, 0, b"selection") {
        let end = if s.self_closing {
            s.end
        } else {
            memmem::find(&out[s.end..], b"</selection>").map_or(s.end, |i| s.end + i + 12)
        };
        let ins = new_sel.unwrap_or_default();
        out = [&out[..s.start], ins.as_bytes(), &out[end..]].concat();
    } else if let Some(ins) = new_sel
        && let Some(st) = find_tag(&out, 0, b"state")
        && !st.self_closing
    {
        out = [&out[..st.end], ins.as_bytes(), &out[st.end..]].concat();
    }
    out
}

/// Seleção de uma segmentação: `s="1"` nos itens escolhidos (por índice no cache).
pub fn patch_slicer_cache(xml: &[u8], selected: &HashMap<u32, bool>) -> Vec<u8> {
    let Some(tab) = find_tag(xml, 0, b"tabular") else { return xml.to_vec() };
    let mut out = xml[..tab.end].to_vec();
    let mut pos = tab.end;
    while let Some(lt) = memchr::memmem::find(&xml[pos..], b"<i ").map(|i| pos + i) {
        let Some(end) = tag_end(xml, lt) else { break };
        out.extend_from_slice(&xml[pos..lt]);
        let tag = &xml[lt..end];
        let x = crate::xml::attr(&tag[2..], b"x").and_then(crate::xml::parse_u32).unwrap_or(0);
        let s = selected.get(&x).copied().unwrap_or(true);
        out.extend_from_slice(&retag(tag, &[(b"s", s.then(|| b"1".to_vec()))]));
        pos = end;
        if memmem::find(&xml[pos..], b"</items>")
            .is_some_and(|e| memmem::find(&xml[pos..], b"<i ").is_none_or(|n| e < n))
        {
            break;
        }
    }
    out.extend_from_slice(&xml[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicer_selection() {
        let xml = br#"<slicerCacheDefinition><data><tabular pivotCacheId="7"><items count="2"><i x="0" s="1"/><i x="1" s="1" nd="1"/></items></tabular></data></slicerCacheDefinition>"#;
        let sel: HashMap<u32, bool> = [(0, false), (1, true)].into_iter().collect();
        let out = String::from_utf8(patch_slicer_cache(xml, &sel)).unwrap();
        assert!(out.contains(r#"<i x="0"/>"#), "{out}");
        assert!(out.contains(r#"<i x="1" s="1" nd="1"/>"#), "{out}");
    }

    #[test]
    fn timeline_selection() {
        let xml = br#"<timelineCacheDefinition><state pivotCacheId="1" filterType="dateBetween"><selection startDate="2025-05-01T00:00:00" endDate="2026-11-30T00:00:00"/><bounds startDate="2025-01-01T00:00:00" endDate="2027-01-01T00:00:00"/></state></timelineCacheDefinition>"#;
        let jan = duck_core::input::serial(2026, 1, 1);
        let out = String::from_utf8(patch_timeline_cache(xml, Some((jan, jan + 30.0)))).unwrap();
        assert!(
            out.contains(
                r#"<selection startDate="2026-01-01T00:00:00" endDate="2026-01-31T00:00:00"/><bounds"#
            ),
            "{out}"
        );
        let out = String::from_utf8(patch_timeline_cache(xml, None)).unwrap();
        assert!(out.contains(r#"<state pivotCacheId="1"><bounds"#), "{out}");
    }

    #[test]
    fn new_filter() {
        let xml = br#"<pivotTableDefinition><location ref="A1:B2"/><pivotTableStyleInfo name="x"/></pivotTableDefinition>"#;
        let f = PivotFilter {
            fld: 3,
            kind: "dateBetween".into(),
            eval_order: -1,
            id: 7,
            custom: vec![
                ("greaterThanOrEqual".into(), "45000".into()),
                ("lessThanOrEqual".into(), "45030".into()),
            ],
            and: true,
            whole_day: true,
            ..Default::default()
        };
        let out = String::from_utf8(patch_filters(xml, &[f])).unwrap();
        assert!(out.contains(r#"<pivotTableStyleInfo name="x"/><filters count="1"><filter fld="3" type="dateBetween" evalOrder="-1" id="7"><autoFilter ref="A1"><filterColumn colId="0"><customFilters and="1"><customFilter operator="greaterThanOrEqual" val="45000"/>"#), "{out}");
        // Sem filtros: o bloco sai.
        assert_eq!(patch_filters(out.as_bytes(), &[]), xml.to_vec());
    }

    #[test]
    fn axis_items() {
        let items = vec![
            AxisItem { t: None, r: 0, i: 0, x: vec![0] },
            AxisItem { t: None, r: 1, i: 0, x: vec![3] },
            AxisItem { t: Some("grand".into()), r: 0, i: 0, x: vec![0] },
        ];
        assert_eq!(
            String::from_utf8(axis_xml("rowItems", &items)).unwrap(),
            r#"<rowItems count="3"><i><x/></i><i r="1"><x v="3"/></i><i t="grand"><x/></i></rowItems>"#
        );
    }
}
