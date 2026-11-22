//! Filtros automáticos (`<autoFilter>`) e tabelas do Excel (`xl/tables/`).

use duck_core::{AutoFilter, FilterColumn, FilterKind, Table};

use crate::package::Package;
use crate::sheet::parse_range;
use crate::xml::{attr, attr_str, find_tag, for_each, inner, parse_u32};

/// Um `<autoFilter ref=…>…</autoFilter>` (`attrs` da tag, `body` o conteúdo).
pub fn parse_autofilter(attrs: &[u8], body: &[u8]) -> Option<AutoFilter> {
    let range = attr(attrs, b"ref").and_then(parse_range)?;
    let mut columns = Vec::new();
    let mut pos = 0;
    while let Some(t) = find_tag(body, pos, b"filterColumn") {
        let content = inner(body, &t, b"filterColumn");
        let end = if t.self_closing { t.end } else { t.end + content.len() + 15 };
        let Some(col) = attr(t.attrs, b"colId").and_then(parse_u32) else {
            pos = end;
            continue;
        };
        // Só `<filters>` com `<filter val>` simples é editável; o resto fica guardado.
        let kind = match find_tag(content, 0, b"filters") {
            Some(f) if !content.windows(13).any(|w| w == b"dateGroupItem") => {
                let blank = attr(f.attrs, b"blank").is_some_and(|v| v == b"1" || v == b"true");
                let mut values = Vec::new();
                for_each(inner(content, &f, b"filters"), b"filter", |a, _| {
                    if let Some(v) = attr_str(a, b"val") {
                        values.push(v);
                    }
                });
                FilterKind::Values { values, blank }
            }
            _ => FilterKind::Other(String::from_utf8_lossy(&body[t.start..end.min(body.len())]).into_owned()),
        };
        columns.push(FilterColumn { col, kind });
        pos = end;
    }
    Some(AutoFilter { range, columns })
}

/// Lê as tabelas de uma aba: os `r:id` guardados em `tables[..].part` viram
/// o caminho da parte, e a parte é lida.
pub fn read_tables(pkg: &Package, sheet_part: &str, tables: &mut Vec<Table>) {
    if tables.is_empty() {
        return;
    }
    let targets = crate::parts::rel_targets(pkg, sheet_part);
    let ids: Vec<String> = tables.drain(..).map(|t| t.part).collect();
    for id in ids {
        let Some(path) = targets.iter().find(|(i, _)| *i == id).map(|(_, p)| p.clone()) else { continue };
        let Ok(xml) = pkg.read(&path) else { continue };
        if let Some(mut t) = parse_table(&xml) {
            t.part = path;
            tables.push(t);
        }
    }
}

/// O XML de uma tabela.
pub fn parse_table(xml: &[u8]) -> Option<Table> {
    let t = find_tag(xml, 0, b"table")?;
    let a = t.attrs;
    let range = attr(a, b"ref").and_then(parse_range)?;
    let num = |k: &[u8], d: u32| attr(a, k).and_then(parse_u32).unwrap_or(d);
    let mut table = Table {
        part: String::new(),
        name: attr_str(a, b"displayName").or_else(|| attr_str(a, b"name")).unwrap_or_default(),
        range,
        header_rows: num(b"headerRowCount", 1),
        totals_rows: num(b"totalsRowCount", 0),
        ..Default::default()
    };
    if let Some(af) = find_tag(xml, 0, b"autoFilter") {
        table.filter = parse_autofilter(af.attrs, inner(xml, &af, b"autoFilter"));
    }
    let mut pos = 0;
    while let Some(c) = find_tag(xml, pos, b"tableColumn") {
        let end = if c.self_closing {
            c.end
        } else {
            memchr::memmem::find(&xml[c.end..], b"</tableColumn>").map_or(c.end, |x| c.end + x + 14)
        };
        table.columns.push(attr_str(c.attrs, b"name").unwrap_or_default());
        table.column_xml.push(String::from_utf8_lossy(&xml[c.start..end]).into_owned());
        pos = end;
    }
    if let Some(si) = find_tag(xml, 0, b"tableStyleInfo") {
        let on = |k: &[u8]| attr(si.attrs, k).is_some_and(|v| v == b"1" || v == b"true");
        table.style = attr_str(si.attrs, b"name");
        table.row_stripes = on(b"showRowStripes");
        table.col_stripes = on(b"showColumnStripes");
        table.first_col = on(b"showFirstColumn");
        table.last_col = on(b"showLastColumn");
    }
    Some(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_and_tables() {
        let body = br#"<filterColumn colId="1"><filters blank="1"><filter val="SP"/><filter val="RJ"/></filters></filterColumn><filterColumn colId="3"><top10 val="5"/></filterColumn>"#;
        let af = parse_autofilter(br#" ref="A1:D10""#, body).unwrap();
        assert_eq!(af.range.last_row, 9);
        assert_eq!(
            af.columns[0],
            FilterColumn {
                col: 1,
                kind: FilterKind::Values { values: vec!["SP".into(), "RJ".into()], blank: true }
            }
        );
        assert!(matches!(&af.columns[1].kind, FilterKind::Other(x) if x.contains("top10")));
        let t = parse_table(br#"<table id="1" name="T" displayName="Vendas" ref="B2:D20" totalsRowShown="0"><autoFilter ref="B2:D20"/><tableColumns count="3"><tableColumn id="1" name="Loja"/><tableColumn id="2" name="UF"/><tableColumn id="3" name="Valor"/></tableColumns><tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/></table>"#).unwrap();
        assert_eq!(t.name, "Vendas");
        assert_eq!((t.range.first_row, t.range.first_col, t.header_rows), (1, 1, 1));
        assert_eq!(t.columns, vec!["Loja", "UF", "Valor"]);
        assert_eq!(t.style.as_deref(), Some("TableStyleMedium2"));
        assert!(t.row_stripes && t.filter.is_some());
    }
}
