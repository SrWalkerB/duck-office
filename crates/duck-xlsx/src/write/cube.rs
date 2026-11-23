//! Remendo das partes do Modelo de Dados ao salvar: os membros dos campos, os
//! filtros e as hierarquias de cada dinâmica, os itens no cache dela e a
//! seleção das segmentações. O modelo em si (`item.data`) não muda.

use duck_core::cube::{CubeCache, bracket, split_unique};
use duck_core::drawing::{Slicer, SlicerPivot};
use duck_core::pivot::{Axis, PItem, PivotTable};
use memchr::memmem;

use super::cellxml::escape_attr;
use crate::drawing::children;
use crate::xml::{attr, find_tag, inner};

fn esc(s: &str) -> String {
    let mut o = Vec::new();
    escape_attr(s, &mut o);
    String::from_utf8(o).unwrap_or_default()
}

/// Fim do elemento que abre em `t` (com o fechamento).
fn element_end(xml: &[u8], t: &crate::xml::Tag, name: &[u8]) -> usize {
    if t.self_closing {
        return t.end;
    }
    let mut close = b"</".to_vec();
    close.extend_from_slice(name);
    close.push(b'>');
    memmem::find(&xml[t.end..], &close).map_or(xml.len(), |i| t.end + i + close.len())
}

/// Troca um elemento inteiro; sem ele, insere antes do primeiro de `before`.
fn put(xml: &[u8], name: &[u8], new: &[u8], before: &[&[u8]]) -> Vec<u8> {
    if let Some(t) = find_tag(xml, 0, name) {
        let end = element_end(xml, &t, name);
        return [&xml[..t.start], new, &xml[end..]].concat();
    }
    for b in before {
        if let Some(i) = memmem::find(xml, b) {
            return [&xml[..i], new, &xml[i..]].concat();
        }
    }
    xml.to_vec()
}

/// Data no formato das partes.
fn iso(d: f64) -> String {
    duck_core::numfmt::serial_to_datetime(d, false, 0).map_or_else(String::new, |t| {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
    })
}

/// Nome único de um membro.
fn unique(hierarchy: &str, it: &PItem) -> String {
    let key = match it {
        PItem::Missing => return format!("{hierarchy}.&"),
        PItem::Str(s) => s.clone(),
        PItem::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
        PItem::Num(n) => format!("{n:E}"),
        PItem::Date(d) => iso(*d),
        PItem::Bool(b) => if *b { "True" } else { "False" }.into(),
        PItem::Err(e) => e.clone(),
    };
    format!("{hierarchy}.&{}", bracket(&key))
}

/// `<sharedItems>` (e os nomes únicos) de um campo recalculado.
fn shared_items(hierarchy: &str, items: &[PItem]) -> String {
    let mut body = String::new();
    let (mut text, mut num, mut date, mut blank, mut int) = (false, false, false, false, true);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for it in items {
        match it {
            PItem::Str(s) => {
                text = true;
                body.push_str(&format!("<s v=\"{}\"/>", esc(s)));
            }
            PItem::Num(n) => {
                num = true;
                int &= n.fract() == 0.0;
                (lo, hi) = (lo.min(*n), hi.max(*n));
                body.push_str(&format!("<n v=\"{n}\"/>"));
            }
            PItem::Date(d) => {
                date = true;
                (lo, hi) = (lo.min(*d), hi.max(*d));
                body.push_str(&format!("<d v=\"{}\"/>", iso(*d)));
            }
            PItem::Bool(b) => body.push_str(&format!("<b v=\"{}\"/>", *b as u8)),
            PItem::Err(e) => body.push_str(&format!("<e v=\"{}\"/>", esc(e))),
            PItem::Missing => {
                blank = true;
                body.push_str("<m/>");
            }
        }
    }
    let mut a = String::new();
    let mixed = text as u8 + (num || date) as u8 > 1;
    if !text {
        a.push_str(" containsSemiMixedTypes=\"0\"");
        if date {
            a.push_str(" containsNonDate=\"0\" containsDate=\"1\"");
        }
        a.push_str(" containsString=\"0\"");
    }
    if blank {
        a.push_str(" containsBlank=\"1\"");
    }
    if mixed {
        a.push_str(" containsMixedTypes=\"1\"");
    }
    if num {
        a.push_str(" containsNumber=\"1\"");
        if int {
            a.push_str(" containsInteger=\"1\"");
        }
        a.push_str(&format!(" minValue=\"{lo}\" maxValue=\"{hi}\""));
    }
    if date {
        a.push_str(&format!(" minDate=\"{}\" maxDate=\"{}\"", iso(lo), iso(hi + 1.0)));
    }
    let names: String = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            format!("<x15:cachedUniqueName index=\"{i}\" name=\"{}\"/>", esc(&unique(hierarchy, it)))
        })
        .collect();
    format!(
        "<sharedItems{a} count=\"{}\">{body}</sharedItems><extLst><ext uri=\"{{4F2E5C28-24EA-4eb8-9CBF-B6C8F9C3D259}}\" xmlns:x15=\"http://schemas.microsoft.com/office/spreadsheetml/2010/11/main\"><x15:cachedUniqueNames>{names}</x15:cachedUniqueNames></ext></extLst>",
        items.len()
    )
}

/// Cache de uma dinâmica do modelo: membros dos campos e campos novos.
pub fn patch_cache(xml: &[u8], t: &PivotTable, cache: &CubeCache) -> Vec<u8> {
    let Some(p) = &t.cube else { return xml.to_vec() };
    let Some(fields) = find_tag(xml, 0, b"cacheFields") else { return xml.to_vec() };
    let now = cache.with_new(p);
    let body = inner(xml, &fields, b"cacheFields");
    let mut out = String::new();
    for (i, f) in now.fields.iter().enumerate() {
        let items = p.items.iter().find(|x| x.0 == i as u32).map(|x| &x.1);
        let el = children(body).into_iter().filter(|e| e.name == b"cacheField").nth(i);
        let hierarchy = now.hierarchy_of(i).map(|h| h.unique.clone()).unwrap_or_default();
        match (el, items) {
            (Some(e), None) => {
                // Como estava.
                out.push_str(&format!("<cacheField{}", String::from_utf8_lossy(e.attrs)));
                if e.body.is_empty() {
                    out.push_str("/>");
                } else {
                    out.push_str(&format!(">{}</cacheField>", String::from_utf8_lossy(e.body)));
                }
            }
            (Some(e), Some(items)) => {
                out.push_str(&format!(
                    "<cacheField{}>{}</cacheField>",
                    String::from_utf8_lossy(e.attrs),
                    shared_items(&hierarchy, items)
                ));
            }
            (None, items) => {
                let inner = items.map_or("<sharedItems/>".to_string(), |x| shared_items(&hierarchy, x));
                out.push_str(&format!(
                    "<cacheField name=\"{}\" caption=\"{}\" numFmtId=\"0\" hierarchy=\"{}\" level=\"1\">{inner}</cacheField>",
                    esc(&f.name),
                    esc(&f.caption),
                    f.hierarchy.unwrap_or(0),
                ));
            }
        }
    }
    let new = format!("<cacheFields count=\"{}\">{out}</cacheFields>", now.fields.len());
    let mut x = put(xml, b"cacheFields", new.as_bytes(), &[]);
    // Cada hierarquia nova diz os campos que usa (todos os membros e o nível).
    for h in &p.new_fields {
        let Some(hier) = cache.hierarchies.get(*h as usize) else { continue };
        let needle = format!("uniqueName=\"{}\"", esc(&hier.unique));
        let mut pos = 0;
        while let Some(t) = find_tag(&x, pos, b"cacheHierarchy") {
            pos = t.end;
            let tag = String::from_utf8_lossy(&x[t.start..t.end]).into_owned();
            if !tag.contains(&needle) {
                continue;
            }
            let field = now.field_of(*h as usize).unwrap_or(0);
            let usage = format!(
                "<fieldsUsage count=\"2\"><fieldUsage x=\"-1\"/><fieldUsage x=\"{field}\"/></fieldsUsage>"
            );
            let end = element_end(&x, &t, b"cacheHierarchy");
            let el = &x[t.start..end];
            let new: Vec<u8> = if t.self_closing {
                let open = tag.trim_end_matches("/>").to_string();
                format!("{open}>{usage}</cacheHierarchy>").into_bytes()
            } else if memmem::find(el, b"<fieldsUsage").is_some() {
                break;
            } else {
                let at = t.end - t.start;
                [&el[..at], usage.as_bytes(), &el[at..]].concat()
            };
            x = [&x[..t.start], &new[..], &x[end..]].concat();
            break;
        }
    }
    x
}

/// A dinâmica: itens dos campos recalculados, campos novos, filtros por
/// membro e hierarquias dos eixos. Entra depois do remendo comum.
pub fn patch_table(xml: &[u8], t: &PivotTable, cache: &CubeCache) -> Vec<u8> {
    let Some(p) = &t.cube else { return xml.to_vec() };
    let now = cache.with_new(p);
    let mut out = xml.to_vec();
    if let Some(fields) = find_tag(&out, 0, b"pivotFields") {
        let body = inner(&out, &fields, b"pivotFields").to_vec();
        let mut s = String::new();
        let els = children(&body);
        for (i, f) in t.fields.iter().enumerate() {
            let recalculated = p.items.iter().any(|x| x.0 == i as u32);
            let el = els.iter().filter(|e| e.name == b"pivotField").nth(i);
            let items = || {
                let mut x = format!("<items count=\"{}\">", f.items.len());
                for (k, it) in f.items.iter().enumerate() {
                    let e = if it.collapsed { " e=\"0\"" } else { "" };
                    x.push_str(&format!("<item x=\"{k}\"{e}/>"));
                }
                x.push_str("</items>");
                x
            };
            match el {
                Some(e) => {
                    let attrs = String::from_utf8_lossy(e.attrs).into_owned();
                    let mut inner_xml = e.body.to_vec();
                    if recalculated {
                        let on_axis = matches!(f.axis, Axis::Row | Axis::Col);
                        let new = if on_axis { items() } else { String::new() };
                        inner_xml = match find_tag(&inner_xml, 0, b"items") {
                            Some(_) => put(&inner_xml, b"items", new.as_bytes(), &[]),
                            None => [new.as_bytes(), &inner_xml].concat(),
                        };
                    }
                    if inner_xml.is_empty() {
                        s.push_str(&format!("<pivotField{attrs}/>"));
                    } else {
                        s.push_str(&format!(
                            "<pivotField{attrs}>{}</pivotField>",
                            String::from_utf8_lossy(&inner_xml)
                        ));
                    }
                }
                None => {
                    let axis = match f.axis {
                        Axis::Row => " axis=\"axisRow\"",
                        Axis::Col => " axis=\"axisCol\"",
                        _ => "",
                    };
                    let c = if f.compact { "" } else { " compact=\"0\"" };
                    let o = if f.outline { "" } else { " outline=\"0\"" };
                    s.push_str(&format!(
                        "<pivotField{axis}{c} allDrilled=\"1\"{o} subtotalTop=\"0\" showAll=\"0\" dataSourceSort=\"1\" defaultSubtotal=\"0\" defaultAttributeDrillState=\"1\">{}</pivotField>",
                        if f.items.is_empty() { String::new() } else { items() }
                    ));
                }
            }
        }
        let new = format!("<pivotFields count=\"{}\">{s}</pivotFields>", t.fields.len());
        out = put(&out, b"pivotFields", new.as_bytes(), &[]);
    }
    // Filtros de página: o membro escolhido.
    if let Some(pages) = find_tag(&out, 0, b"pageFields") {
        let mut s = String::new();
        for (f, h, name) in &p.pages {
            s.push_str(&format!("<pageField fld=\"{f}\" hier=\"{h}\""));
            if let Some(n) = name {
                let cap = split_unique(n).map(|_| caption(n)).unwrap_or_default();
                s.push_str(&format!(" name=\"{}\" cap=\"{}\"", esc(n), esc(&cap)));
            }
            s.push_str("/>");
        }
        let _ = pages;
        let new = format!("<pageFields count=\"{}\">{s}</pageFields>", p.pages.len());
        out = put(&out, b"pageFields", new.as_bytes(), &[]);
    }
    // Membros escolhidos por hierarquia.
    if let Some(ph) = find_tag(&out, 0, b"pivotHierarchies") {
        let body = inner(&out, &ph, b"pivotHierarchies").to_vec();
        let mut s = String::new();
        let mut n = 0;
        for (i, e) in children(&body).into_iter().enumerate() {
            n += 1;
            let list = p.members.iter().find(|m| m.0 == i as u32).map(|m| &m.1);
            let mut attrs = String::from_utf8_lossy(e.attrs).into_owned();
            if list.is_some() && attr(e.attrs, b"multipleItemSelectionAllowed").is_none() {
                attrs = format!(" multipleItemSelectionAllowed=\"1\"{attrs}");
            }
            let mut inner_xml = e.body.to_vec();
            if let Some(mt) = find_tag(&inner_xml, 0, b"members") {
                let end = element_end(&inner_xml, &mt, b"members");
                inner_xml.drain(mt.start..end);
            }
            if let Some(list) = list {
                let m: String = list.iter().map(|u| format!("<member name=\"{}\"/>", esc(u))).collect();
                let m = format!("<members count=\"{}\" level=\"1\">{m}</members>", list.len());
                // Depois de `mps`, antes de `extLst`.
                let at = memmem::find(&inner_xml, b"<extLst").unwrap_or(inner_xml.len());
                inner_xml.splice(at..at, m.into_bytes());
            }
            if inner_xml.is_empty() {
                s.push_str(&format!("<pivotHierarchy{attrs}/>"));
            } else {
                s.push_str(&format!(
                    "<pivotHierarchy{attrs}>{}</pivotHierarchy>",
                    String::from_utf8_lossy(&inner_xml)
                ));
            }
        }
        let new = format!("<pivotHierarchies count=\"{n}\">{s}</pivotHierarchies>");
        out = put(&out, b"pivotHierarchies", new.as_bytes(), &[]);
    }
    // Hierarquias nas linhas e colunas.
    for (tag, one, list) in [
        (b"rowHierarchiesUsage".as_slice(), "rowHierarchyUsage", &t.row_fields),
        (b"colHierarchiesUsage", "colHierarchyUsage", &t.col_fields),
    ] {
        let ids: Vec<i64> = list
            .iter()
            .filter_map(|f| {
                if *f < 0 { Some(-2) } else { now.fields.get(*f as usize)?.hierarchy.map(|h| h as i64) }
            })
            .collect();
        let name = String::from_utf8_lossy(tag);
        let body: String = ids.iter().map(|i| format!("<{one} hierarchyUsage=\"{i}\"/>")).collect();
        let new = if ids.is_empty() {
            String::new()
        } else {
            format!("<{name} count=\"{}\">{body}</{name}>", ids.len())
        };
        // Fora da lista, entram antes das extensões do fim da tabela.
        let ext = memmem::rfind(&out, b"<extLst>")
            .filter(|i| memmem::rfind(&out, b"</pivotHierarchies>").is_none_or(|h| h < *i));
        if find_tag(&out, 0, tag).is_some() {
            out = put(&out, tag, new.as_bytes(), &[]);
        } else if !new.is_empty() {
            let at = ext.or_else(|| memmem::rfind(&out, b"</pivotTableDefinition>")).unwrap_or(out.len());
            out.splice(at..at, new.into_bytes());
        }
    }
    out
}

/// Rótulo de um membro pelo nome único.
fn caption(unique: &str) -> String {
    match unique.find("].&") {
        Some(i) => {
            let k = &unique[i + 3..];
            k.strip_prefix('[').and_then(|k| k.strip_suffix(']')).unwrap_or(k).replace("]]", "]")
        }
        None => unique.rsplit('[').next().unwrap_or("").trim_end_matches(']').to_string(),
    }
}

/// Segmentação do modelo: membros com dados e a seleção.
pub fn patch_slicer(xml: &[u8], s: &Slicer, link: &SlicerPivot) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut pos = 0;
    let Some(olap) = find_tag(xml, 0, b"olap") else { return xml.to_vec() };
    let mut at = olap.end;
    // Membros sem dados (`nd`).
    while let Some(t) = find_tag(xml, at, b"i") {
        at = t.end;
        let n = crate::xml::attr_str(t.attrs, b"n").unwrap_or_default();
        let Some(k) = link.members.iter().position(|m| *m == n) else { continue };
        let nd = attr(t.attrs, b"nd").is_some_and(|v| v == b"1");
        if nd == s.items[k].2 {
            out.extend_from_slice(&xml[pos..t.start]);
            let tag = String::from_utf8_lossy(&xml[t.start..t.end]).replace(" nd=\"1\"", "");
            let tag = if s.items[k].2 { tag } else { tag.replacen("<i ", "<i nd=\"1\" ", 1) };
            // `nd` vem depois do nome, como o Excel grava.
            out.extend_from_slice(tag.as_bytes());
            pos = t.end;
        }
    }
    out.extend_from_slice(&xml[pos..]);
    let all = s.items.iter().all(|i| i.1);
    let chosen: Vec<String> = if all {
        vec![format!("{}.[All]", link.source)]
    } else {
        link.members.iter().zip(&s.items).filter(|x| x.1.1).map(|x| x.0.clone()).collect()
    };
    let body: String = chosen.iter().map(|c| format!("<selection n=\"{}\"/>", esc(c))).collect();
    let new = format!("<selections count=\"{}\">{body}</selections>", chosen.len());
    put(&out, b"selections", new.as_bytes(), &[b"</olap>"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::cube::{CubeField, CubePivot, Hierarchy};

    fn cache() -> CubeCache {
        let h = |u: &str, m| Hierarchy { unique: u.into(), caption: "x".into(), measure: m };
        CubeCache {
            hierarchies: vec![h("[T].[a]", false), h("[T].[b]", false), h("[Measures].[m]", true)],
            fields: vec![
                CubeField { name: "[T].[a].[a]".into(), hierarchy: Some(0), caption: "a".into() },
                CubeField { name: "[Measures].[m]".into(), hierarchy: Some(2), caption: "m".into() },
            ],
        }
    }

    const TABLE: &[u8] = br#"<pivotTableDefinition name="T" cacheId="9"><location ref="A1:B3" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/><pivotFields count="2"><pivotField axis="axisRow" allDrilled="1" showAll="0" dataSourceSort="1"><items count="1"><item x="0"/></items></pivotField><pivotField dataField="1" showAll="0"/></pivotFields><rowFields count="1"><field x="0"/></rowFields><rowItems count="1"><i><x/></i></rowItems><colItems count="1"><i/></colItems><pageFields count="1"><pageField fld="0" hier="1" name="[T].[b].[All]" cap="All"/></pageFields><dataFields count="1"><dataField name="m" fld="1"/></dataFields><pivotHierarchies count="3"><pivotHierarchy/><pivotHierarchy dragToData="1"><members count="1" level="1"><member name="velho"/></members></pivotHierarchy><pivotHierarchy/></pivotHierarchies><rowHierarchiesUsage count="1"><rowHierarchyUsage hierarchyUsage="0"/></rowHierarchiesUsage><extLst><ext uri="u"/></extLst></pivotTableDefinition>"#;

    #[test]
    fn table_members_and_new_field() {
        let mut t = crate::pivot::parse_pivot_table(TABLE).unwrap();
        // Entra a hierarquia `b` no lugar da `a`, com dois membros.
        t.fields[0].axis = Axis::None;
        t.fields.push(duck_core::pivot::PivotField {
            axis: Axis::Row,
            compact: false,
            items: vec![
                duck_core::pivot::FieldItem { x: Some(0), collapsed: true, ..Default::default() },
                duck_core::pivot::FieldItem { x: Some(1), ..Default::default() },
            ],
            ..Default::default()
        });
        t.row_fields = vec![2];
        let p = t.cube.as_mut().unwrap();
        p.new_fields.push(1);
        p.items = vec![(2, vec![PItem::Str("x & y".into()), PItem::Missing])];
        p.choose(1, "[T].[b]", Some(vec!["[T].[b].&[1]".into(), "[T].[b].&".into()]));
        let base = super::super::pivot::patch_pivot_table(TABLE, &t);
        let out = patch_table(&base, &t, &cache());
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains(r#"<pivotFields count="3">"#), "{s}");
        assert!(
            s.contains(r#"<pivotField axis="axisRow" compact="0" allDrilled="1" subtotalTop="0""#),
            "{s}"
        );
        assert!(s.contains(r#"<items count="2"><item x="0" e="0"/><item x="1"/></items>"#), "{s}");
        assert!(s.contains(r#"<rowHierarchyUsage hierarchyUsage="1"/>"#), "{s}");
        assert!(s.contains(r#"<pivotHierarchy multipleItemSelectionAllowed="1" dragToData="1"><members count="2" level="1"><member name="[T].[b].&amp;[1]"/><member name="[T].[b].&amp;"/></members></pivotHierarchy>"#), "{s}");
        assert!(s.contains(r#"name="[T].[b].&amp;[1]" cap="1"/>"#), "{s}");
        // Volta igual.
        let back = crate::pivot::parse_pivot_table(&out).unwrap();
        assert_eq!(back.row_fields, [2]);
        assert_eq!(back.fields[2].items, t.fields[2].items);
        assert_eq!(back.cube.as_ref().unwrap().members, t.cube.as_ref().unwrap().members);
    }

    #[test]
    fn cache_items() {
        let xml = br#"<pivotCacheDefinition><cacheFields count="2"><cacheField name="[T].[a].[a]" caption="a" numFmtId="0" hierarchy="0" level="1"><sharedItems count="1"><s v="velho"/></sharedItems></cacheField><cacheField name="[Measures].[m]" hierarchy="2" level="32767"/></cacheFields><cacheHierarchies count="3"><cacheHierarchy uniqueName="[T].[a]" caption="a"/><cacheHierarchy uniqueName="[T].[b]" caption="x"/><cacheHierarchy uniqueName="[Measures].[m]" measure="1"/></cacheHierarchies></pivotCacheDefinition>"#;
        let mut t = crate::pivot::parse_pivot_table(TABLE).unwrap();
        t.cube = Some(Box::new(CubePivot {
            items: vec![(0, vec![PItem::Str("novo".into())]), (2, vec![PItem::Date(46_000.0)])],
            new_fields: vec![1],
            ..Default::default()
        }));
        let out = patch_cache(xml, &t, &cache());
        let s = String::from_utf8_lossy(&out);
        assert!(
            s.contains(r#"<sharedItems count="1"><s v="novo"/></sharedItems>"#) && !s.contains("velho"),
            "{s}"
        );
        assert!(s.contains(r#"name="[T].[a].&amp;[novo]"/>"#), "{s}");
        assert!(
            s.contains(r#"<cacheField name="[T].[b].[b]" caption="x" numFmtId="0" hierarchy="1" level="1">"#),
            "{s}"
        );
        assert!(s.contains(r#"<d v="2025-12-09T00:00:00"/>"#), "{s}");
        assert!(s.contains(r#"<cacheHierarchy uniqueName="[T].[b]" caption="x"><fieldsUsage count="2"><fieldUsage x="-1"/><fieldUsage x="2"/></fieldsUsage></cacheHierarchy>"#), "{s}");
    }

    #[test]
    fn slicer() {
        let xml = br#"<d><data><olap pivotCacheId="1"><levels count="1"><level><ranges><range startItem="0"><i n="[T].[a].&amp;[x]" c="x"/><i n="[T].[a].&amp;[y]" c="y" nd="1"/></range></ranges></level></levels><selections count="1"><selection n="[T].[a].[All]"/></selections></olap></data></d>"#;
        let link = SlicerPivot {
            source: "[T].[a]".into(),
            members: vec!["[T].[a].&[x]".into(), "[T].[a].&[y]".into()],
            ..Default::default()
        };
        let s =
            Slicer { items: vec![("x".into(), false, true), ("y".into(), true, true)], ..Default::default() };
        let out = String::from_utf8(patch_slicer(xml, &s, &link)).unwrap();
        assert!(
            out.contains(r#"<selections count="1"><selection n="[T].[a].&amp;[y]"/></selections>"#),
            "{out}"
        );
        assert!(!out.contains("nd="), "{out}");
    }
}
