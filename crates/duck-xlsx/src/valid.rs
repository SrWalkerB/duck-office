//! Validação de dados: leitura e gravação de `<dataValidations>` e do bloco
//! `<x14:dataValidations>` da extensão do Excel 2010 (listas em outra aba).

use duck_core::{CellRange, DataValidation, DvAlert, DvKind, DvOp};

use crate::sheet::parse_range;
use crate::write::{cell_ref, escape_attr, escape_text, push_u32};
use crate::xml::{attr, attr_str, find_tag, for_each, for_each_attr, inner, unescape_into};

/// Os `<dataValidation>` de um bloco.
pub(crate) fn parse_block(body: &[u8], out: &mut Vec<DataValidation>) {
    for_each(body, b"dataValidation", |a, c| {
        let Some(sqref) = attr(a, b"sqref") else { return };
        let mut v = attributes(a);
        v.ranges = ranges(sqref);
        v.f1 = text(c, b"formula1");
        v.f2 = text(c, b"formula2");
        if let Some(t) = find_tag(c, 0, b"x12ac:list") {
            let mut s = String::new();
            unescape_into(inner(c, &t, b"x12ac:list"), &mut s);
            v.list12 = Some(s);
        }
        if !v.ranges.is_empty() {
            out.push(v);
        }
    });
}

/// Os `<x14:dataValidation>` de um bloco da extensão: o intervalo e as
/// fórmulas vêm em elementos.
pub(crate) fn parse_ext_block(body: &[u8], out: &mut Vec<DataValidation>) {
    for_each(body, b"x14:dataValidation", |a, c| {
        let mut v = attributes(a);
        v.ext = true;
        let formula =
            |name: &[u8]| find_tag(c, 0, name).map(|t| text(inner(c, &t, name), b"xm:f")).unwrap_or_default();
        v.f1 = formula(b"x14:formula1");
        v.f2 = formula(b"x14:formula2");
        v.ranges = ranges(text(c, b"xm:sqref").as_bytes());
        if !v.ranges.is_empty() {
            out.push(v);
        }
    });
}

fn text(body: &[u8], name: &[u8]) -> String {
    let mut s = String::new();
    if let Some(t) = find_tag(body, 0, name) {
        unescape_into(inner(body, &t, name), &mut s);
    }
    s
}

pub(crate) fn ranges(sqref: &[u8]) -> Vec<CellRange> {
    sqref.split(|b| b.is_ascii_whitespace()).filter(|s| !s.is_empty()).filter_map(one_range).collect()
}

/// `A1:B2`, `A1` ou, como alguns programas gravam, `A:C` e `2:5`.
fn one_range(s: &[u8]) -> Option<CellRange> {
    use duck_core::shift::{MAX_COLS, MAX_ROWS};
    if let Some(r) = parse_range(s) {
        return Some(r);
    }
    let s = std::str::from_utf8(s).ok()?.replace('$', "");
    let (a, b) = s.split_once(':').unwrap_or((&s, &s));
    let whole = |a: &str, b: &str| {
        parse_range(format!("{a}:{b}").as_bytes()).filter(|r| r.last_col < MAX_COLS && r.last_row < MAX_ROWS)
    };
    if a.bytes().all(|c| c.is_ascii_alphabetic()) {
        return whole(&format!("{a}1"), &format!("{b}{MAX_ROWS}"));
    }
    let r = whole(&format!("A{a}"), &format!("A{b}"))?;
    Some(CellRange { last_col: MAX_COLS - 1, ..r })
}

const KNOWN: [&[u8]; 13] = [
    b"type",
    b"errorStyle",
    b"operator",
    b"allowBlank",
    b"showDropDown",
    b"showInputMessage",
    b"showErrorMessage",
    b"errorTitle",
    b"error",
    b"promptTitle",
    b"prompt",
    b"sqref",
    b"imeMode",
];

fn attributes(a: &[u8]) -> DataValidation {
    let on = |k: &[u8]| matches!(attr(a, k), Some(b"1") | Some(b"true"));
    let s = |k: &[u8]| attr_str(a, k).unwrap_or_default();
    let mut extra = Vec::new();
    for_each_attr(a, |k, whole| {
        // `imeMode` não é usado, mas volta no lugar.
        if k == b"imeMode" || !KNOWN.contains(&k) {
            extra.push(b' ');
            extra.extend_from_slice(whole);
        }
    });
    DataValidation {
        ranges: Vec::new(),
        kind: match attr(a, b"type").unwrap_or_default() {
            b"whole" => DvKind::Whole,
            b"decimal" => DvKind::Decimal,
            b"list" => DvKind::List,
            b"date" => DvKind::Date,
            b"time" => DvKind::Time,
            b"textLength" => DvKind::TextLength,
            b"custom" => DvKind::Custom,
            _ => DvKind::Any,
        },
        op: match attr(a, b"operator").unwrap_or_default() {
            b"notBetween" => DvOp::NotBetween,
            b"equal" => DvOp::Equal,
            b"notEqual" => DvOp::NotEqual,
            b"greaterThan" => DvOp::Greater,
            b"lessThan" => DvOp::Less,
            b"greaterThanOrEqual" => DvOp::GreaterEqual,
            b"lessThanOrEqual" => DvOp::LessEqual,
            _ => DvOp::Between,
        },
        f1: String::new(),
        f2: String::new(),
        allow_blank: on(b"allowBlank"),
        dropdown: !on(b"showDropDown"),
        show_input: on(b"showInputMessage"),
        show_error: on(b"showErrorMessage"),
        alert: match attr(a, b"errorStyle").unwrap_or_default() {
            b"warning" => DvAlert::Warning,
            b"information" => DvAlert::Information,
            _ => DvAlert::Stop,
        },
        error_title: s(b"errorTitle"),
        error: s(b"error"),
        prompt_title: s(b"promptTitle"),
        prompt: s(b"prompt"),
        ext: false,
        list12: None,
        extra: String::from_utf8_lossy(&extra).into_owned(),
    }
}

// ---------------------------------------------------------------------------
// Gravação
// ---------------------------------------------------------------------------

/// `<dataValidations>` com as regras comuns (vazio se não houver).
pub(crate) fn block_xml(list: &[DataValidation]) -> Vec<u8> {
    write_block(list.iter().filter(|v| !v.ext), false)
}

/// `<x14:dataValidations>` com as regras da extensão (vazio se não houver).
pub(crate) fn ext_block_xml(list: &[DataValidation]) -> Vec<u8> {
    write_block(list.iter().filter(|v| v.ext), true)
}

fn write_block<'a>(list: impl Iterator<Item = &'a DataValidation>, ext: bool) -> Vec<u8> {
    let mut items = Vec::new();
    let mut n = 0;
    for v in list {
        one(v, &mut items);
        n += 1;
    }
    if n == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(items.len() + 128);
    if ext {
        out.extend_from_slice(
            b"<x14:dataValidations xmlns:xm=\"http://schemas.microsoft.com/office/excel/2006/main\" count=\"",
        );
    } else {
        out.extend_from_slice(b"<dataValidations count=\"");
    }
    push_u32(&mut out, n);
    out.extend_from_slice(b"\">");
    out.extend_from_slice(&items);
    out.extend_from_slice(if ext { b"</x14:dataValidations>" } else { b"</dataValidations>" });
    out
}

fn push_attr(out: &mut Vec<u8>, k: &str, v: &str) {
    out.push(b' ');
    out.extend_from_slice(k.as_bytes());
    out.extend_from_slice(b"=\"");
    escape_attr(v, out);
    out.push(b'"');
}

pub(crate) fn sqref(ranges: &[CellRange], out: &mut Vec<u8>) {
    for (i, r) in ranges.iter().enumerate() {
        if i > 0 {
            out.push(b' ');
        }
        cell_ref(r.first_row, r.first_col, out);
        if (r.first_row, r.first_col) != (r.last_row, r.last_col) {
            out.push(b':');
            cell_ref(r.last_row, r.last_col, out);
        }
    }
}

fn one(v: &DataValidation, out: &mut Vec<u8>) {
    let p: &str = if v.ext { "x14:" } else { "" };
    out.extend_from_slice(format!("<{p}dataValidation").as_bytes());
    let kind = match v.kind {
        DvKind::Any => "",
        DvKind::Whole => "whole",
        DvKind::Decimal => "decimal",
        DvKind::List => "list",
        DvKind::Date => "date",
        DvKind::Time => "time",
        DvKind::TextLength => "textLength",
        DvKind::Custom => "custom",
    };
    if !kind.is_empty() {
        push_attr(out, "type", kind);
    }
    match v.alert {
        DvAlert::Stop => {}
        DvAlert::Warning => push_attr(out, "errorStyle", "warning"),
        DvAlert::Information => push_attr(out, "errorStyle", "information"),
    }
    let op = match v.op {
        DvOp::Between => "",
        DvOp::NotBetween => "notBetween",
        DvOp::Equal => "equal",
        DvOp::NotEqual => "notEqual",
        DvOp::Greater => "greaterThan",
        DvOp::Less => "lessThan",
        DvOp::GreaterEqual => "greaterThanOrEqual",
        DvOp::LessEqual => "lessThanOrEqual",
    };
    if !op.is_empty() {
        push_attr(out, "operator", op);
    }
    for (k, on) in [
        ("allowBlank", v.allow_blank),
        ("showDropDown", !v.dropdown),
        ("showInputMessage", v.show_input),
        ("showErrorMessage", v.show_error),
    ] {
        if on {
            push_attr(out, k, "1");
        }
    }
    for (k, s) in [
        ("errorTitle", &v.error_title),
        ("error", &v.error),
        ("promptTitle", &v.prompt_title),
        ("prompt", &v.prompt),
    ] {
        if !s.is_empty() {
            push_attr(out, k, s);
        }
    }
    if !v.ext {
        out.extend_from_slice(b" sqref=\"");
        sqref(&v.ranges, out);
        out.push(b'"');
    }
    out.extend_from_slice(v.extra.as_bytes());
    out.push(b'>');
    if v.ext {
        for (name, f) in [("x14:formula1", &v.f1), ("x14:formula2", &v.f2)] {
            if !f.is_empty() {
                out.extend_from_slice(format!("<{name}><xm:f>").as_bytes());
                escape_text(f, out);
                out.extend_from_slice(format!("</xm:f></{name}>").as_bytes());
            }
        }
        out.extend_from_slice(b"<xm:sqref>");
        sqref(&v.ranges, out);
        out.extend_from_slice(b"</xm:sqref></x14:dataValidation>");
        return;
    }
    if let Some(l) = &v.list12 {
        out.extend_from_slice(b"<mc:AlternateContent xmlns:x12ac=\"http://schemas.microsoft.com/office/spreadsheetml/2011/1/ac\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice Requires=\"x12ac\"><x12ac:list>");
        escape_text(l, out);
        out.extend_from_slice(b"</x12ac:list></mc:Choice><mc:Fallback><formula1>");
        escape_text(&v.f1, out);
        out.extend_from_slice(b"</formula1></mc:Fallback></mc:AlternateContent>");
    } else if !v.f1.is_empty() {
        out.extend_from_slice(b"<formula1>");
        escape_text(&v.f1, out);
        out.extend_from_slice(b"</formula1>");
    }
    if !v.f2.is_empty() {
        out.extend_from_slice(b"<formula2>");
        escape_text(&v.f2, out);
        out.extend_from_slice(b"</formula2>");
    }
    out.extend_from_slice(b"</dataValidation>");
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = concat!(
        r#"<dataValidation type="list" allowBlank="1" showInputMessage="1" showErrorMessage="1" errorTitle="Ops" error="Só &quot;Sim&quot; &amp; &quot;Não&quot;" promptTitle="Dica" prompt="linha 1_x000a_linha 2" sqref="B2:B10 D4" xr:uid="{AB}"><formula1>"Sim,Não"</formula1></dataValidation>"#,
        r#"<dataValidation type="whole" errorStyle="warning" operator="greaterThan" showDropDown="1" sqref="C1"><formula1>A1*2</formula1></dataValidation>"#,
        r#"<dataValidation type="date" imeMode="off" sqref="C2"><formula1>45000</formula1><formula2>46000</formula2></dataValidation>"#,
        r#"<dataValidation allowBlank="1" showInputMessage="1" prompt="Só mensagem" sqref="C3"/>"#,
        r#"<dataValidation type="list" sqref="C4"><mc:AlternateContent xmlns:x12ac="a" xmlns:mc="b"><mc:Choice Requires="x12ac"><x12ac:list>"1,5",2</x12ac:list></mc:Choice><mc:Fallback><formula1>"1,5,2"</formula1></mc:Fallback></mc:AlternateContent></dataValidation>"#,
    );

    #[test]
    fn reads_and_writes() {
        let mut v = Vec::new();
        parse_block(XML.as_bytes(), &mut v);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0].literal_items().unwrap(), ["Sim", "Não"]);
        assert_eq!(v[0].ranges.len(), 2);
        assert_eq!(v[0].error, "Só \"Sim\" & \"Não\"");
        assert_eq!(v[0].prompt, "linha 1\nlinha 2");
        assert_eq!(v[0].extra, " xr:uid=\"{AB}\"");
        assert!(v[0].dropdown && v[0].allow_blank && v[0].show_error);
        assert_eq!((v[1].kind, v[1].op, v[1].alert), (DvKind::Whole, DvOp::Greater, DvAlert::Warning));
        assert!(!v[1].dropdown && !v[1].show_error);
        assert_eq!(v[2].extra, " imeMode=\"off\"");
        assert_eq!((v[2].f1.as_str(), v[2].f2.as_str()), ("45000", "46000"));
        assert_eq!(v[3].kind, DvKind::Any);
        assert_eq!(v[4].literal_items().unwrap(), ["1,5", "2"]);
        // Gravado e relido, igual.
        let out = block_xml(&v);
        let mut back = Vec::new();
        parse_block(&out, &mut back);
        assert_eq!(back, v, "{}", String::from_utf8_lossy(&out));
        assert!(ext_block_xml(&v).is_empty());
    }

    #[test]
    fn whole_columns_and_rows() {
        let r = |a, b, c, d| CellRange { first_row: a, first_col: b, last_row: c, last_col: d };
        assert_eq!(
            ranges(b"A1:B2  $C$3 B:C 2:5 lixo"),
            [r(0, 0, 1, 1), r(2, 2, 2, 2), r(0, 1, 1_048_575, 2), r(1, 0, 4, 16_383)]
        );
    }

    #[test]
    fn extension() {
        let xml = r#"<x14:dataValidations xmlns:xm="x" count="1"><x14:dataValidation type="list" allowBlank="1" showErrorMessage="1" xr:uid="{1}"><x14:formula1><xm:f>Outra!$A$1:$A$3</xm:f></x14:formula1><xm:sqref>A1 C1:C5</xm:sqref></x14:dataValidation></x14:dataValidations>"#;
        let mut v = Vec::new();
        parse_ext_block(xml.as_bytes(), &mut v);
        assert_eq!(v.len(), 1);
        assert!(v[0].ext);
        assert_eq!(v[0].f1, "Outra!$A$1:$A$3");
        assert_eq!(v[0].ranges.len(), 2);
        let mut back = Vec::new();
        parse_ext_block(&ext_block_xml(&v), &mut back);
        assert_eq!(back, v);
        assert!(block_xml(&v).is_empty());
    }
}
