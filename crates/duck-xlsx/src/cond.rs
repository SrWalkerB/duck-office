//! Leitura da formatação condicional (`<conditionalFormatting>` de uma aba).
//! e das regras da extensão do Excel 2010 (`<x14:conditionalFormatting>`, no
//! `<extLst>`): ícones personalizados, barras de dados completas e regras com
//! formato próprio.

use duck_core::{BarAxis, BarLook, CfKind, CfOp, CfRule, Cfvo, CfvoKind, Color, ExtId, Period, TextOp};

use crate::sheet::parse_range;
use crate::xml::{attr, attr_str, find_tag, for_each, inner, parse_u32, unescape_into};

/// Lê um bloco: `attrs` são os atributos de `<conditionalFormatting>` e
/// `body`, o conteúdo dele.
pub(crate) fn parse_block(attrs: &[u8], body: &[u8], out: &mut Vec<CfRule>) {
    if let Some(sqref) = attr(attrs, b"sqref") {
        parse_rules(sqref, body, false, out);
    }
}

/// Lê um bloco da extensão. Os nomes são os mesmos com prefixo, e o intervalo
/// e as fórmulas vêm em elementos (`<xm:sqref>`, `<xm:f>`).
pub(crate) fn parse_ext_block(body: &[u8], out: &mut Vec<CfRule>) {
    let plain = without_prefixes(body);
    if let Some(t) = find_tag(&plain, 0, b"sqref") {
        parse_rules(inner(&plain, &t, b"sqref"), &plain, true, out);
    }
}

/// `<x14:cfvo>` → `<cfvo>`, `<xm:f>` → `<formula>`.
fn without_prefixes(x: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(x.len() + 64);
    let mut pos = 0;
    while let Some(i) = memchr::memchr(b'<', &x[pos..]).map(|i| pos + i + 1) {
        out.extend_from_slice(&x[pos..i]);
        let mut k = i;
        if x.get(k) == Some(&b'/') {
            out.push(b'/');
            k += 1;
        }
        for p in [&b"x14:"[..], b"xm:"] {
            if x[k..].starts_with(p) {
                k += p.len();
                if p == b"xm:" && x[k..].starts_with(b"f>") {
                    out.extend_from_slice(b"formula");
                    k += 1;
                }
                break;
            }
        }
        pos = k;
    }
    out.extend_from_slice(&x[pos..]);
    out
}

fn parse_rules(sqref: &[u8], body: &[u8], extension: bool, out: &mut Vec<CfRule>) {
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
        let text = attr_str(a, b"text");
        let no_text = text.is_none();
        let text = text.unwrap_or_default();
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
                    let color = match find_tag(db, 0, b"fillColor") {
                        Some(t) => crate::parts::color(t.attrs),
                        // Na extensão, a cor costuma ficar só na regra comum.
                        None if extension => Color::Auto,
                        None => color,
                    };
                    let look = if extension { bar_look(t.attrs, db) } else { BarLook::default() };
                    match (v.next(), v.next()) {
                        (Some(min), Some(max)) => CfKind::DataBar {
                            min,
                            max,
                            color,
                            look: Box::new(look),
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
                    let body = inner(c, &t, b"iconSet");
                    let mut icons = Vec::new();
                    if on(b"custom") {
                        for_each(body, b"cfIcon", |a, _| {
                            let id = attr(a, b"iconId").and_then(parse_u32).unwrap_or(0).min(4) as u8;
                            icons.push((attr_str(a, b"iconSet").unwrap_or_default(), id));
                        });
                    }
                    CfKind::IconSet {
                        set,
                        cfvos: cfvos(body),
                        icons,
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
            // Na extensão, o texto pode vir de uma célula: só a fórmula vale.
            b"containsText" | b"notContainsText" | b"beginsWith" | b"endsWith" if no_text && extension => {
                formulas.first().cloned().map_or(CfKind::Unsupported, CfKind::Expression)
            }
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
        // A ligação: `id` na extensão, `<x14:id>` na regra comum.
        let id = if extension {
            attr_str(a, b"id")
        } else {
            find_tag(c, 0, b"x14:id").and_then(|t| {
                let v = inner(c, &t, b"x14:id");
                std::str::from_utf8(v).ok().map(|s| s.trim().to_string())
            })
        };
        let own = if extension { find_tag(c, 0, b"dxf") } else { None };
        out.push(CfRule {
            own: own.map(|t| Box::new(crate::parts::dxf(inner(c, &t, b"dxf")))),
            ext: id.map(|id| Box::new(ExtId { id, extension })),
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

/// Aparência de uma barra da extensão (`<x14:dataBar>`).
fn bar_look(a: &[u8], body: &[u8]) -> BarLook {
    let flag = |k: &[u8], default: bool| match attr(a, k) {
        Some(b"1") | Some(b"true") => true,
        Some(b"0") | Some(b"false") => false,
        _ => default,
    };
    let len =
        |k: &[u8], default: u32| attr(a, k).and_then(parse_u32).unwrap_or(default).min(100) as f32 / 100.0;
    let color = |n: &[u8]| find_tag(body, 0, n).map(|t| crate::parts::color(t.attrs));
    let border = flag(b"border", false).then(|| color(b"borderColor")).flatten();
    BarLook {
        min_len: len(b"minLength", 10),
        max_len: len(b"maxLength", 90),
        gradient: flag(b"gradient", true),
        border,
        negative: (!flag(b"negativeBarColorSameAsPositive", false))
            .then(|| color(b"negativeFillColor"))
            .flatten(),
        negative_border: (!flag(b"negativeBarBorderColorSameAsPositive", true))
            .then(|| color(b"negativeBorderColor"))
            .flatten(),
        axis: match attr(a, b"axisPosition") {
            Some(b"middle") => BarAxis::Middle,
            Some(b"none") => BarAxis::None,
            _ => BarAxis::Auto,
        },
        axis_color: color(b"axisColor"),
        rtl: attr(a, b"direction") == Some(b"rightToLeft"),
    }
}

fn cfvos(body: &[u8]) -> Vec<Cfvo> {
    let mut out = Vec::new();
    for_each(body, b"cfvo", |a, c| {
        let kind = match attr(a, b"type").unwrap_or_default() {
            b"min" => CfvoKind::Min,
            b"max" => CfvoKind::Max,
            b"autoMin" => CfvoKind::AutoMin,
            b"autoMax" => CfvoKind::AutoMax,
            b"percent" => CfvoKind::Percent,
            b"percentile" => CfvoKind::Percentile,
            b"formula" => CfvoKind::Formula,
            _ => CfvoKind::Num,
        };
        let mut val = String::new();
        if let Some(v) = attr(a, b"val") {
            unescape_into(v, &mut val);
        } else if let Some(t) = find_tag(c, 0, b"formula") {
            unescape_into(inner(c, &t, b"formula"), &mut val);
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

    #[test]
    fn reads_extension_rules() {
        let icons = br#"<x14:cfRule type="iconSet" priority="4" id="{1}"><x14:iconSet iconSet="3Arrows" showValue="0" custom="1"><x14:cfvo type="percent"><xm:f>0</xm:f></x14:cfvo><x14:cfvo type="num"><xm:f>-1.5</xm:f></x14:cfvo><x14:cfvo type="num" gte="0"><xm:f>$B$1&amp;""</xm:f></x14:cfvo><x14:cfIcon iconSet="3TrafficLights1" iconId="2"/><x14:cfIcon iconSet="3Triangles" iconId="1"/><x14:cfIcon iconSet="NoIcons" iconId="0"/></x14:iconSet></x14:cfRule><xm:sqref>A1:A5 C3</xm:sqref>"#;
        let mut out = Vec::new();
        parse_ext_block(icons, &mut out);
        assert_eq!((out.len(), out[0].ranges.len(), out[0].priority), (1, 2, 4));
        match &out[0].kind {
            CfKind::IconSet { cfvos, icons, show_value: false, .. } => {
                assert_eq!(cfvos[1].val, "-1.5");
                assert_eq!((cfvos[2].val.as_str(), cfvos[2].gte), ("$B$1&\"\"", false));
                assert_eq!(icons[0], ("3TrafficLights1".to_string(), 2));
                assert_eq!(icons[2].0, "NoIcons");
            }
            k => panic!("{k:?}"),
        }
        // As duas metades de uma barra de dados.
        let mut l = duck_core::Layout::default();
        parse_block(
            br#"sqref="B1:B5""#,
            br#"<cfRule type="dataBar" priority="1"><dataBar><cfvo type="num" val="0"/><cfvo type="num" val="1"/><color rgb="FF638EC6"/></dataBar><extLst><ext uri="{B025F937-C7B1-47D3-B67F-A62EFF666E3E}"><x14:id>{9}</x14:id></ext></extLst></cfRule>"#,
            &mut l.cond,
        );
        parse_ext_block(
            br#"<x14:cfRule type="dataBar" id="{9}"><x14:dataBar minLength="0" maxLength="100" gradient="0" border="1"><x14:cfvo type="autoMin"/><x14:cfvo type="num"><xm:f>2</xm:f></x14:cfvo><x14:borderColor rgb="FF112233"/><x14:negativeFillColor rgb="FFFF0000"/><x14:axisColor rgb="FF000000"/></x14:dataBar></x14:cfRule><xm:sqref>B1:B5</xm:sqref>"#,
            &mut l.cond,
        );
        l.finish();
        assert_eq!(l.cond.len(), 1);
        match &l.cond[0].kind {
            CfKind::DataBar { min, max, color, look, .. } => {
                assert_eq!((min.kind, max.val.as_str()), (CfvoKind::AutoMin, "2"));
                assert_eq!(*color, Color::Rgb(0xFF63_8EC6));
                assert_eq!((look.min_len, look.max_len, look.gradient), (0.0, 1.0, false));
                assert_eq!(look.border, Some(Color::Rgb(0xFF11_2233)));
                assert_eq!(look.negative, Some(Color::Rgb(0xFFFF_0000)));
            }
            k => panic!("{k:?}"),
        }
        // Regra com formato próprio.
        let mut out = Vec::new();
        parse_ext_block(
            br#"<x14:cfRule type="expression" priority="2" id="{3}"><xm:f>$A1&gt;3</xm:f><x14:dxf><fill><patternFill><bgColor rgb="FFFFC7CE"/></patternFill></fill></x14:dxf></x14:cfRule><xm:sqref>D1</xm:sqref>"#,
            &mut out,
        );
        assert_eq!(out[0].kind, CfKind::Expression("$A1>3".into()));
        assert_eq!(out[0].own.as_ref().and_then(|d| d.fill), Some(Color::Rgb(0xFFFF_C7CE)));
    }
}
