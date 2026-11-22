//! Leitura da formatação condicional (`<conditionalFormatting>` de uma aba).
//! As regras da extensão do Excel 2010 (`<x14:conditionalFormatting>`, no
//! `<extLst>`) ficam de fora.

use duck_core::{CfKind, CfOp, CfRule, Cfvo, CfvoKind, Color, Period, TextOp};

use crate::sheet::parse_range;
use crate::xml::{attr, attr_str, find_tag, for_each, inner, parse_u32, unescape_into};

/// Lê um bloco: `attrs` são os atributos de `<conditionalFormatting>` e
/// `body`, o conteúdo dele.
pub(crate) fn parse_block(attrs: &[u8], body: &[u8], out: &mut Vec<CfRule>) {
    let Some(sqref) = attr(attrs, b"sqref") else { return };
    let ranges: Vec<_> =
        sqref.split(|&b| b == b' ').filter(|s| !s.is_empty()).filter_map(parse_range).collect();
    if ranges.is_empty() {
        return;
    }
    for_each(body, b"cfRule", |a, c| {
        let flag = |k: &[u8], default: bool| match attr(a, k) {
            Some(b"1") | Some(b"true") => true,
            Some(b"0") | Some(b"false") => false,
            _ => default,
        };
        let num = |k: &[u8]| attr(a, k).and_then(parse_u32);
        let mut formulas = Vec::new();
        for_each(c, b"formula", |_, f| {
            let mut s = String::new();
            unescape_into(f, &mut s);
            formulas.push(s);
        });
        let text = attr_str(a, b"text").unwrap_or_default();
        let kind = match attr(a, b"type").unwrap_or_default() {
            b"cellIs" => {
                let op = match attr(a, b"operator").unwrap_or_default() {
                    b"lessThan" => CfOp::Lt,
                    b"lessThanOrEqual" => CfOp::Le,
                    b"greaterThan" => CfOp::Gt,
                    b"greaterThanOrEqual" => CfOp::Ge,
                    b"equal" => CfOp::Eq,
                    b"notEqual" => CfOp::Ne,
                    b"between" => CfOp::Between,
                    b"notBetween" => CfOp::NotBetween,
                    _ => CfOp::Eq,
                };
                match formulas.first() {
                    Some(f1) => CfKind::CellIs { op, f1: f1.clone(), f2: formulas.get(1).cloned() },
                    None => CfKind::Unsupported,
                }
            }
            b"expression" => formulas.first().cloned().map_or(CfKind::Unsupported, CfKind::Expression),
            b"colorScale" => {
                let cs =
                    find_tag(c, 0, b"colorScale").map(|t| inner(c, &t, b"colorScale")).unwrap_or_default();
                let cfvos = cfvos(cs);
                let mut colors = Vec::new();
                for_each(cs, b"color", |a, _| colors.push(crate::parts::color(a)));
                if cfvos.len() >= 2 && cfvos.len() == colors.len() {
                    CfKind::ColorScale(cfvos.into_iter().zip(colors).collect())
                } else {
                    CfKind::Unsupported
                }
            }
            b"dataBar" => match find_tag(c, 0, b"dataBar") {
                Some(t) => {
                    let db = inner(c, &t, b"dataBar");
                    let mut v = cfvos(db).into_iter();
                    let color = find_tag(db, 0, b"color")
                        .map_or(Color::Rgb(0xFF638EC6), |t| crate::parts::color(t.attrs));
                    match (v.next(), v.next()) {
                        (Some(min), Some(max)) => CfKind::DataBar {
                            min,
                            max,
                            color,
                            show_value: !matches!(attr(t.attrs, b"showValue"), Some(b"0") | Some(b"false")),
                        },
                        _ => CfKind::Unsupported,
                    }
                }
                None => CfKind::Unsupported,
            },
            b"iconSet" => match find_tag(c, 0, b"iconSet") {
                Some(t) => {
                    let set = attr_str(t.attrs, b"iconSet").unwrap_or_else(|| "3TrafficLights1".into());
                    let on = |k: &[u8]| matches!(attr(t.attrs, k), Some(b"1") | Some(b"true"));
                    CfKind::IconSet {
                        set,
                        cfvos: cfvos(inner(c, &t, b"iconSet")),
                        reverse: on(b"reverse"),
                        show_value: !matches!(attr(t.attrs, b"showValue"), Some(b"0") | Some(b"false")),
                    }
                }
                None => CfKind::Unsupported,
            },
            b"top10" => CfKind::Top10 {
                rank: num(b"rank").unwrap_or(10),
                percent: flag(b"percent", false),
                bottom: flag(b"bottom", false),
            },
            b"aboveAverage" => CfKind::AboveAverage {
                above: flag(b"aboveAverage", true),
                equal: flag(b"equalAverage", false),
                std_dev: num(b"stdDev").unwrap_or(0),
            },
            b"duplicateValues" => CfKind::Duplicate(true),
            b"uniqueValues" => CfKind::Duplicate(false),
            b"containsText" => CfKind::Text { op: TextOp::Contains, text },
            b"notContainsText" => CfKind::Text { op: TextOp::NotContains, text },
            b"beginsWith" => CfKind::Text { op: TextOp::Begins, text },
            b"endsWith" => CfKind::Text { op: TextOp::Ends, text },
            b"containsBlanks" => CfKind::Blanks(true),
            b"notContainsBlanks" => CfKind::Blanks(false),
            b"containsErrors" => CfKind::Errors(true),
            b"notContainsErrors" => CfKind::Errors(false),
            b"timePeriod" => match attr(a, b"timePeriod").unwrap_or_default() {
                b"today" => CfKind::TimePeriod(Period::Today),
                b"yesterday" => CfKind::TimePeriod(Period::Yesterday),
                b"tomorrow" => CfKind::TimePeriod(Period::Tomorrow),
                b"last7Days" => CfKind::TimePeriod(Period::Last7Days),
                b"thisWeek" => CfKind::TimePeriod(Period::ThisWeek),
                b"lastWeek" => CfKind::TimePeriod(Period::LastWeek),
                b"nextWeek" => CfKind::TimePeriod(Period::NextWeek),
                b"thisMonth" => CfKind::TimePeriod(Period::ThisMonth),
                b"lastMonth" => CfKind::TimePeriod(Period::LastMonth),
                b"nextMonth" => CfKind::TimePeriod(Period::NextMonth),
                _ => CfKind::Unsupported,
            },
            _ => CfKind::Unsupported,
        };
        out.push(CfRule {
            ranges: ranges.clone(),
            priority: attr(a, b"priority")
                .and_then(|v| std::str::from_utf8(v).ok())
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(i32::MAX),
            stop: flag(b"stopIfTrue", false),
            dxf: num(b"dxfId"),
            kind,
        });
    });
}

fn cfvos(body: &[u8]) -> Vec<Cfvo> {
    let mut out = Vec::new();
    for_each(body, b"cfvo", |a, _| {
        let kind = match attr(a, b"type").unwrap_or_default() {
            b"min" | b"autoMin" => CfvoKind::Min,
            b"max" | b"autoMax" => CfvoKind::Max,
            b"percent" => CfvoKind::Percent,
            b"percentile" => CfvoKind::Percentile,
            b"formula" => CfvoKind::Formula,
            _ => CfvoKind::Num,
        };
        let mut val = String::new();
        if let Some(v) = attr(a, b"val") {
            unescape_into(v, &mut val);
        }
        out.push(Cfvo { kind, val, gte: !matches!(attr(a, b"gte"), Some(b"0") | Some(b"false")) });
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rules() {
        let body = br#"<cfRule type="cellIs" dxfId="3" priority="2" operator="between"><formula>1</formula><formula>&quot;x&quot;</formula></cfRule>
<cfRule type="expression" dxfId="1" priority="1" stopIfTrue="1"><formula>$A1&lt;&gt;""</formula></cfRule>
<cfRule type="colorScale" priority="3"><colorScale><cfvo type="min"/><cfvo type="percentile" val="50"/><cfvo type="max"/><color rgb="FFF8696B"/><color rgb="FFFFEB84"/><color rgb="FF63BE7B"/></colorScale></cfRule>
<cfRule type="dataBar" priority="4"><dataBar showValue="0"><cfvo type="min"/><cfvo type="max"/><color rgb="FF638EC6"/></dataBar></cfRule>
<cfRule type="iconSet" priority="5"><iconSet iconSet="3Arrows" reverse="1"><cfvo type="percent" val="0"/><cfvo type="percent" val="33"/><cfvo type="percent" val="67" gte="0"/></iconSet></cfRule>
<cfRule type="duplicateValues" dxfId="0" priority="6"/><cfRule type="containsText" dxfId="0" priority="7" operator="containsText" text="ok"><formula>NOT(ISERROR(SEARCH("ok",A1)))</formula></cfRule>
<cfRule type="weird" priority="8"/>"#;
        let mut out = Vec::new();
        parse_block(br#"sqref="A1:A10 C3""#, body, &mut out);
        assert_eq!(out.len(), 8);
        assert_eq!(out[0].ranges.len(), 2);
        assert_eq!(
            out[0].kind,
            CfKind::CellIs { op: CfOp::Between, f1: "1".into(), f2: Some("\"x\"".into()) }
        );
        assert_eq!((out[0].dxf, out[0].priority), (Some(3), 2));
        assert_eq!(out[1].kind, CfKind::Expression("$A1<>\"\"".into()));
        assert!(out[1].stop);
        match &out[2].kind {
            CfKind::ColorScale(s) => {
                assert_eq!(s.len(), 3);
                assert_eq!(s[1].0.kind, CfvoKind::Percentile);
                assert_eq!(s[2].1, Color::Rgb(0xFF63BE7B));
            }
            k => panic!("{k:?}"),
        }
        assert!(matches!(&out[3].kind, CfKind::DataBar { show_value: false, .. }));
        match &out[4].kind {
            CfKind::IconSet { set, cfvos, reverse, .. } => {
                assert_eq!(set, "3Arrows");
                assert!(*reverse);
                assert!(!cfvos[2].gte);
            }
            k => panic!("{k:?}"),
        }
        assert_eq!(out[5].kind, CfKind::Duplicate(true));
        assert_eq!(out[6].kind, CfKind::Text { op: TextOp::Contains, text: "ok".into() });
        assert_eq!(out[7].kind, CfKind::Unsupported);
    }
}
