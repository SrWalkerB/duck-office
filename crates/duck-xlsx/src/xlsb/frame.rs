//! O que uma aba tem além das células (visão, larguras, mesclagens,
//! formatação condicional, validação, links...) → XML, na ordem do esquema.

use duck_core::CellRange;

use super::out::{attr, cell, flag, range, ranges, text, xattr};
use super::ptg::{self, Names};
use super::rec::{Cur, LAST_COL, Records, color_attrs, id};

const PANES: [&str; 4] = ["bottomRight", "topRight", "bottomLeft", "topLeft"];

/// Começo da aba, até antes de `<sheetData>`.
pub fn head(bin: &[u8]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">",
    );
    let (mut prop, mut dim, mut views, mut fmt, mut cols) =
        (String::new(), String::new(), String::new(), String::new(), String::new());
    for (kind, b) in Records::new(bin) {
        let mut c = Cur::new(b);
        let _ = match kind {
            id::WS_PROP => sheet_pr(&mut c, &mut prop),
            id::WS_DIM => c.range().map(|r| dim = format!("<dimension ref=\"{}\"/>", range(&r))),
            id::BEGIN_WS_VIEW => view(&mut c, &mut views),
            id::PANE => pane(&mut c, &mut views),
            id::SEL => selection(&mut c, &mut views),
            id::END_WS_VIEW => {
                views.push_str("</sheetView>");
                Some(())
            }
            id::WS_FMT_INFO => format_pr(&mut c, &mut fmt),
            id::COL_INFO => col(&mut c, &mut cols),
            _ => None,
        };
    }
    s.push_str(&prop);
    s.push_str(&dim);
    if !views.is_empty() {
        s.push_str(&format!("<sheetViews>{views}</sheetViews>"));
    }
    s.push_str(&fmt);
    if !cols.is_empty() {
        s.push_str(&format!("<cols>{cols}</cols>"));
    }
    s
}

fn sheet_pr(c: &mut Cur, out: &mut String) -> Option<()> {
    let (lo, _) = (c.u16()?, c.u8()?);
    let color = c.take(8)?;
    c.skip(8)?;
    let code = c.string()?;
    out.push_str("<sheetPr");
    if !code.is_empty() {
        out.push_str(&attr("codeName", &code));
    }
    out.push('>');
    // Cor da guia (automática = sem cor).
    if color[0] >> 1 != 0
        && let Some(a) = color_attrs(color)
    {
        out.push_str(&format!("<tabColor{a}/>"));
    }
    let (below, right) = (lo & 0x40 != 0, lo & 0x80 != 0);
    if !below || !right {
        out.push_str(&format!(
            "<outlinePr summaryBelow=\"{}\" summaryRight=\"{}\"/>",
            below as u8, right as u8
        ));
    }
    if lo & 0x100 != 0 {
        out.push_str("<pageSetUpPr fitToPage=\"1\"/>");
    }
    out.push_str("</sheetPr>");
    Some(())
}

fn view(c: &mut Cur, out: &mut String) -> Option<()> {
    let (bits, kind, row, col) = (c.u16()?, c.u32()?, c.u32()?, c.u32()?);
    c.skip(4)?;
    let (zoom, normal) = (c.u16()?, c.u16()?);
    out.push_str("<sheetView");
    for (mask, name) in [(4, "showGridLines"), (8, "showRowColHeaders"), (0x10, "showZeros")] {
        if bits & mask == 0 {
            out.push_str(&format!(" {name}=\"0\""));
        }
    }
    out.push_str(&flag("showFormulas", bits & 2 != 0));
    out.push_str(&flag("rightToLeft", bits & 0x20 != 0));
    out.push_str(&flag("tabSelected", bits & 0x40 != 0));
    match kind {
        1 => out.push_str(" view=\"pageBreakPreview\""),
        2 => out.push_str(" view=\"pageLayout\""),
        _ => {}
    }
    if (row, col) != (0, 0) {
        out.push_str(&attr("topLeftCell", &cell(row, col)));
    }
    for (name, n) in [("zoomScale", zoom), ("zoomScaleNormal", normal)] {
        if n != 0 && n != 100 {
            out.push_str(&format!(" {name}=\"{n}\""));
        }
    }
    out.push_str(" workbookViewId=\"0\">");
    Some(())
}

fn pane(c: &mut Cur, out: &mut String) -> Option<()> {
    let (x, y, row, col, active, bits) = (c.f64()?, c.f64()?, c.u32()?, c.u32()?, c.u32()?, c.u8()?);
    out.push_str("<pane");
    for (name, n) in [("xSplit", x), ("ySplit", y)] {
        if n != 0.0 {
            out.push_str(&format!(" {name}=\"{n}\""));
        }
    }
    out.push_str(&attr("topLeftCell", &cell(row, col)));
    out.push_str(&attr("activePane", PANES.get(active as usize)?));
    match (bits & 1 != 0, bits & 2 != 0) {
        (true, true) => out.push_str(" state=\"frozen\""),
        (true, false) => out.push_str(" state=\"frozenSplit\""),
        _ => {}
    }
    out.push_str("/>");
    Some(())
}

fn selection(c: &mut Cur, out: &mut String) -> Option<()> {
    let (pane, row, col, which) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?);
    let list = c.ranges()?;
    out.push_str("<selection");
    if pane != 3 {
        out.push_str(&attr("pane", PANES.get(pane as usize)?));
    }
    out.push_str(&attr("activeCell", &cell(row, col)));
    if which != 0 {
        out.push_str(&format!(" activeCellId=\"{which}\""));
    }
    if !list.is_empty() {
        out.push_str(&attr("sqref", &ranges(&list)));
    }
    out.push_str("/>");
    Some(())
}

fn format_pr(c: &mut Cur, out: &mut String) -> Option<()> {
    let (width, base, height, bits) = (c.u32()?, c.u16()?, c.u16()?, c.u16()?);
    let (rows, cols) = (c.u8()?, c.u8()?);
    out.push_str("<sheetFormatPr");
    if base != 8 {
        out.push_str(&format!(" baseColWidth=\"{base}\""));
    }
    if width != u32::MAX {
        out.push_str(&format!(" defaultColWidth=\"{}\"", width as f64 / 256.0));
    }
    out.push_str(&format!(" defaultRowHeight=\"{}\"", height as f64 / 20.0));
    out.push_str(&flag("customHeight", bits & 1 != 0));
    out.push_str(&flag("zeroHeight", bits & 2 != 0));
    for (name, n) in [("outlineLevelRow", rows), ("outlineLevelCol", cols)] {
        if n != 0 {
            out.push_str(&format!(" {name}=\"{n}\""));
        }
    }
    out.push_str("/>");
    Some(())
}

fn col(c: &mut Cur, out: &mut String) -> Option<()> {
    let (first, last, width, style, bits) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?, c.u16()?);
    if first > last || last > LAST_COL {
        return None;
    }
    out.push_str(&format!(
        "<col min=\"{}\" max=\"{}\" width=\"{}\"",
        first + 1,
        last + 1,
        width as f64 / 256.0
    ));
    if style != 0 {
        out.push_str(&format!(" style=\"{style}\""));
    }
    out.push_str(&flag("hidden", bits & 1 != 0));
    out.push_str(&flag("customWidth", bits & 2 != 0));
    out.push_str(&flag("bestFit", bits & 4 != 0));
    let level = bits >> 8 & 7;
    if level != 0 {
        out.push_str(&format!(" outlineLevel=\"{level}\""));
    }
    out.push_str(&flag("collapsed", bits & 0x1000 != 0));
    out.push_str("/>");
    Some(())
}

/// Fim da aba, a partir de depois de `</sheetData>`. `unread` conta as regras
/// e fórmulas que não deu para ler.
pub fn tail(bin: &[u8], names: &Names, unread: &mut u64) -> String {
    let (mut filter, mut merges, mut cond, mut valid, mut links) =
        (String::new(), 0usize, String::new(), 0usize, String::new());
    let (mut merge_xml, mut valid_xml, mut page, mut drawing, mut tables) =
        (String::new(), String::new(), String::new(), String::new(), String::new());
    let mut n_tables = 0;
    let mut af = FilterXml::default();
    let mut cf = CfBlock::default();
    for (kind, b) in Records::new(bin) {
        let mut c = Cur::new(b);
        if af.record(kind, b, &mut filter) || cf.record(kind, b, names, &mut cond, unread) {
            continue;
        }
        match kind {
            id::MERGE_CELL => {
                if let Some(r) = c.range() {
                    merge_xml.push_str(&format!("<mergeCell ref=\"{}\"/>", range(&r)));
                    merges += 1;
                }
            }
            id::DVAL => match validation(&mut c, names) {
                Some(x) => {
                    valid_xml.push_str(&x);
                    valid += 1;
                }
                None => *unread += 1,
            },
            id::HLINK => {
                let _ = link(&mut c, &mut links);
            }
            id::MARGINS => {
                let _ = margins(&mut c, &mut page);
            }
            id::PAGE_SETUP => {
                let _ = page_setup(&mut c, &mut page);
            }
            id::DRAWING => {
                if let Some(rid) = c.string() {
                    drawing = format!("<drawing{}/>", attr("r:id", &rid));
                }
            }
            id::LIST_PART => {
                if let Some(rid) = c.string() {
                    tables.push_str(&format!("<tablePart{}/>", attr("r:id", &rid)));
                    n_tables += 1;
                }
            }
            _ => {}
        }
    }
    let mut s = filter;
    if merges > 0 {
        s.push_str(&format!("<mergeCells count=\"{merges}\">{merge_xml}</mergeCells>"));
    }
    s.push_str(&cond);
    if valid > 0 {
        s.push_str(&format!("<dataValidations count=\"{valid}\">{valid_xml}</dataValidations>"));
    }
    if !links.is_empty() {
        s.push_str(&format!("<hyperlinks>{links}</hyperlinks>"));
    }
    s.push_str(&page);
    s.push_str(&drawing);
    if n_tables > 0 {
        s.push_str(&format!("<tableParts count=\"{n_tables}\">{tables}</tableParts>"));
    }
    s.push_str("</worksheet>");
    s
}

fn link(c: &mut Cur, out: &mut String) -> Option<()> {
    let r = c.range()?;
    let (rid, location, tip, display) = (c.string()?, c.string()?, c.string()?, c.string()?);
    out.push_str(&format!("<hyperlink ref=\"{}\"", range(&r)));
    for (name, v) in [("r:id", &rid), ("location", &location), ("tooltip", &tip), ("display", &display)] {
        if !v.is_empty() {
            out.push_str(&xattr(name, v));
        }
    }
    out.push_str("/>");
    Some(())
}

fn margins(c: &mut Cur, out: &mut String) -> Option<()> {
    let mut s = String::from("<pageMargins");
    for name in ["left", "right", "top", "bottom", "header", "footer"] {
        s.push_str(&format!(" {name}=\"{}\"", c.f64()?));
    }
    out.push_str(&s);
    out.push_str("/>");
    Some(())
}

fn page_setup(c: &mut Cur, out: &mut String) -> Option<()> {
    let (paper, scale) = (c.u32()?, c.u32()?);
    c.skip(16)?;
    let (width, height, bits) = (c.u32()?, c.u32()?, c.u16()?);
    out.push_str("<pageSetup");
    for (name, n, default) in [("paperSize", paper, 1), ("scale", scale, 100)] {
        if n != 0 && n != default {
            out.push_str(&format!(" {name}=\"{n}\""));
        }
    }
    for (name, n) in [("fitToWidth", width), ("fitToHeight", height)] {
        if n != 1 {
            out.push_str(&format!(" {name}=\"{n}\""));
        }
    }
    out.push_str(if bits & 2 != 0 { " orientation=\"landscape\"/>" } else { " orientation=\"portrait\"/>" });
    Some(())
}

/// Fórmula opcional (`cce` = 0 quando falta).
fn formula(c: &mut Cur, at: (u32, u32), names: &Names) -> Option<Option<String>> {
    if c.is_empty() {
        return Some(None);
    }
    let (rgce, rgcb) = c.formula()?;
    if rgce.is_empty() {
        return Some(None);
    }
    Some(Some(ptg::text(rgce, rgcb, at, names)?))
}

fn validation(c: &mut Cur, names: &Names) -> Option<String> {
    const TYPES: [&str; 8] = ["none", "whole", "decimal", "list", "date", "time", "textLength", "custom"];
    const STYLES: [&str; 3] = ["stop", "warning", "information"];
    const OPS: [&str; 8] = [
        "between",
        "notBetween",
        "equal",
        "notEqual",
        "greaterThan",
        "lessThan",
        "greaterThanOrEqual",
        "lessThanOrEqual",
    ];
    let bits = c.u32()?;
    let list = c.ranges()?;
    let (etitle, error, ptitle, prompt) = (c.string()?, c.string()?, c.string()?, c.string()?);
    let at = list.first().map_or((0, 0), |r| (r.first_row, r.first_col));
    let (f1, f2) = (formula(c, at, names)?, formula(c, at, names)?);
    let mut s = format!("<dataValidation type=\"{}\"", TYPES.get((bits & 15) as usize)?);
    let style = STYLES.get((bits >> 4 & 7) as usize)?;
    if *style != "stop" {
        s.push_str(&attr("errorStyle", style));
    }
    let op = OPS.get((bits >> 20 & 15) as usize)?;
    if *op != "between" {
        s.push_str(&attr("operator", op));
    }
    for (bit, name) in
        [(8, "allowBlank"), (9, "showDropDown"), (18, "showInputMessage"), (19, "showErrorMessage")]
    {
        s.push_str(&flag(name, bits >> bit & 1 != 0));
    }
    for (name, v) in
        [("errorTitle", &etitle), ("error", &error), ("promptTitle", &ptitle), ("prompt", &prompt)]
    {
        if !v.is_empty() {
            s.push_str(&xattr(name, v));
        }
    }
    s.push_str(&attr("sqref", &ranges(&list)));
    s.push('>');
    for (tag, f) in [("formula1", f1), ("formula2", f2)] {
        if let Some(f) = f {
            s.push_str(&format!("<{tag}>{}</{tag}>", text(&f)));
        }
    }
    s.push_str("</dataValidation>");
    Some(s)
}

// ---------------------------------------------------------------------------
// Filtro automático (abas e tabelas)
// ---------------------------------------------------------------------------

/// `<autoFilter>` a partir dos registros dele.
#[derive(Default)]
pub struct FilterXml {
    open: bool,
    columns: String,
    range: String,
}

impl FilterXml {
    /// Trata o registro, se for de filtro. O XML vai para `out` no fim do bloco.
    pub fn record(&mut self, kind: u16, b: &[u8], out: &mut String) -> bool {
        let mut c = Cur::new(b);
        if kind == id::BEGIN_AFILTER {
            self.range = c.range().map(|r| range(&r)).unwrap_or_default();
            self.columns.clear();
            self.open = true;
            return true;
        }
        if !self.open {
            return false;
        }
        match kind {
            id::END_AFILTER => {
                self.open = false;
                if !self.range.is_empty() {
                    out.push_str(&format!(
                        "<autoFilter ref=\"{}\">{}</autoFilter>",
                        self.range, self.columns
                    ));
                }
            }
            id::BEGIN_FILTER_COLUMN => {
                let col = c.u32().unwrap_or(0);
                let bits = c.u16().unwrap_or(0);
                self.columns.push_str(&format!("<filterColumn colId=\"{col}\""));
                self.columns.push_str(&flag("hiddenButton", bits & 1 != 0));
                self.columns.push('>');
            }
            id::END_FILTER_COLUMN => self.columns.push_str("</filterColumn>"),
            id::BEGIN_FILTERS => {
                let blank = c.u32().unwrap_or(0) != 0;
                self.columns.push_str(&format!("<filters{}>", flag("blank", blank)));
            }
            id::END_FILTERS => self.columns.push_str("</filters>"),
            id::FILTER => {
                if let Some(v) = c.string() {
                    self.columns.push_str(&format!("<filter{}/>", xattr("val", &v)));
                }
            }
            id::DATE_GROUP_ITEM => {
                let _ = date_group(&mut c, &mut self.columns);
            }
            id::TOP10_FILTER => {
                if let (Some(bits), Some(v), Some(f)) = (c.u8(), c.f64(), c.f64()) {
                    self.columns.push_str(&format!(
                        "<top10 top=\"{}\" percent=\"{}\" val=\"{v}\" filterVal=\"{f}\"/>",
                        bits & 1,
                        bits >> 1 & 1
                    ));
                }
            }
            id::BEGIN_CUSTOM_FILTERS => {
                // 0 = as duas condições valem juntas.
                let and = c.u32() == Some(0);
                self.columns.push_str(&format!("<customFilters{}>", flag("and", and)));
            }
            id::END_CUSTOM_FILTERS => self.columns.push_str("</customFilters>"),
            id::CUSTOM_FILTER => {
                let _ = custom(&mut c, &mut self.columns);
            }
            _ => {}
        }
        true
    }
}

fn date_group(c: &mut Cur, out: &mut String) -> Option<()> {
    const LEVELS: [&str; 6] = ["year", "month", "day", "hour", "minute", "second"];
    let mut v = [0u16; 6];
    for x in &mut v {
        *x = c.u16()?;
    }
    c.skip(2)?;
    let level = c.u32()? as usize;
    out.push_str("<dateGroupItem");
    for k in 0..=level.min(5) {
        out.push_str(&format!(" {}=\"{}\"", LEVELS[k], v[k]));
    }
    out.push_str(&attr("dateTimeGrouping", LEVELS.get(level)?));
    out.push_str("/>");
    Some(())
}

fn custom(c: &mut Cur, out: &mut String) -> Option<()> {
    const OPS: [&str; 7] =
        ["equal", "lessThan", "equal", "lessThanOrEqual", "greaterThan", "notEqual", "greaterThanOrEqual"];
    let (kind, op) = (c.u8()?, c.u8()?);
    let raw = c.take(8)?;
    let val = match kind {
        4 => f64::from_le_bytes(*raw.first_chunk()?).to_string(),
        6 => c.string()?,
        8 => (raw[0] != 0).to_string().to_uppercase(),
        _ => " ".to_string(),
    };
    let op = OPS.get(op as usize)?;
    out.push_str("<customFilter");
    if *op != "equal" {
        out.push_str(&attr("operator", op));
    }
    out.push_str(&xattr("val", &val));
    out.push_str("/>");
    Some(())
}

// ---------------------------------------------------------------------------
// Formatação condicional
// ---------------------------------------------------------------------------

/// `<conditionalFormatting>` a partir dos registros dele.
#[derive(Default)]
struct CfBlock {
    open: bool,
    ranges: Vec<CellRange>,
    rules: String,
    /// Regra aberta que deu para ler.
    rule: bool,
    /// Cores vêm depois de todos os limites.
    colors: String,
    /// Elemento aberto dentro da regra.
    inner: &'static str,
}

impl CfBlock {
    fn record(&mut self, kind: u16, b: &[u8], names: &Names, out: &mut String, unread: &mut u64) -> bool {
        let mut c = Cur::new(b);
        if kind == id::BEGIN_CF {
            *self = Self { open: true, ..Default::default() };
            self.ranges = c.skip(8).and_then(|_| c.ranges()).unwrap_or_default();
            return true;
        }
        if !self.open {
            return false;
        }
        let at = self.ranges.first().map_or((0, 0), |r| (r.first_row, r.first_col));
        match kind {
            id::END_CF => {
                self.open = false;
                if !self.ranges.is_empty() && !self.rules.is_empty() {
                    out.push_str(&format!(
                        "<conditionalFormatting sqref=\"{}\">{}</conditionalFormatting>",
                        ranges(&self.ranges),
                        self.rules
                    ));
                }
            }
            id::BEGIN_CF_RULE => match rule(&mut c, at, names) {
                Some(x) => {
                    self.rules.push_str(&x);
                    self.rule = true;
                }
                None => {
                    self.rule = false;
                    *unread += 1;
                }
            },
            id::END_CF_RULE if self.rule => {
                if !self.inner.is_empty() {
                    let colors = std::mem::take(&mut self.colors);
                    self.rules.push_str(&format!("{colors}</{}>", std::mem::take(&mut self.inner)));
                }
                self.rules.push_str("</cfRule>");
                self.rule = false;
            }
            id::BEGIN_ICON_SET if self.rule => {
                let (set, bits) = (c.u32().unwrap_or(0) as usize, c.u16().unwrap_or(0));
                let name = ICON_SETS.get(set).copied().unwrap_or("3TrafficLights1");
                self.inner = "iconSet";
                self.rules.push_str(&format!("<iconSet{}", attr("iconSet", name)));
                // bit 1 = só o ícone; bit 2 = ordem inversa.
                if bits & 2 != 0 {
                    self.rules.push_str(" showValue=\"0\"");
                }
                self.rules.push_str(&flag("reverse", bits & 4 != 0));
                self.rules.push('>');
            }
            id::BEGIN_DATABAR if self.rule => {
                let (lo, hi, show) = (c.u8().unwrap_or(10), c.u8().unwrap_or(90), c.u8().unwrap_or(1));
                self.inner = "dataBar";
                self.rules.push_str("<dataBar");
                for (name, n, default) in [("minLength", lo, 10), ("maxLength", hi, 90)] {
                    if n != default {
                        self.rules.push_str(&format!(" {name}=\"{n}\""));
                    }
                }
                self.rules.push_str(if show == 0 { " showValue=\"0\">" } else { ">" });
            }
            id::BEGIN_COLOR_SCALE if self.rule => {
                self.inner = "colorScale";
                self.rules.push_str("<colorScale>");
            }
            id::CFVO if self.rule && !self.inner.is_empty() => {
                if cfvo(&mut c, at, names, &mut self.rules).is_none() {
                    *unread += 1;
                }
            }
            id::COLOR if self.rule => {
                if let Some(a) = color_attrs(b) {
                    self.colors.push_str(&format!("<color{a}/>"));
                }
            }
            _ => {}
        }
        true
    }
}

const ICON_SETS: [&str; 17] = [
    "3Arrows",
    "3ArrowsGray",
    "3Flags",
    "3TrafficLights1",
    "3TrafficLights2",
    "3Signs",
    "3Symbols",
    "3Symbols2",
    "4Arrows",
    "4ArrowsGray",
    "4RedToBlack",
    "4Rating",
    "4TrafficLights",
    "5Arrows",
    "5ArrowsGray",
    "5Rating",
    "5Quarters",
];

fn cfvo(c: &mut Cur, at: (u32, u32), names: &Names, out: &mut String) -> Option<()> {
    let (kind, num, _save, gte, _) = (c.u32()?, c.f64()?, c.u32()?, c.u32()?, c.u32()?);
    let name = match kind {
        1 => "num",
        2 => "min",
        3 => "max",
        4 => "percent",
        5 => "percentile",
        7 => "formula",
        _ => return None,
    };
    let val = match formula(c, at, names)? {
        Some(f) => Some(f),
        None if matches!(kind, 1 | 4 | 5) => Some(num.to_string()),
        None => None,
    };
    out.push_str(&format!("<cfvo type=\"{name}\""));
    if let Some(v) = val {
        out.push_str(&attr("val", &v));
    }
    if gte == 0 {
        out.push_str(" gte=\"0\"");
    }
    out.push_str("/>");
    Some(())
}

/// Abertura de `<cfRule>` com as fórmulas (o fechamento vem no fim da regra).
fn rule(c: &mut Cur, at: (u32, u32), names: &Names) -> Option<String> {
    const OPS: [&str; 9] = [
        "",
        "between",
        "notBetween",
        "equal",
        "notEqual",
        "greaterThan",
        "lessThan",
        "greaterThanOrEqual",
        "lessThanOrEqual",
    ];
    const TEXT: [(&str, &str); 4] = [
        ("containsText", "containsText"),
        ("notContainsText", "notContains"),
        ("beginsWith", "beginsWith"),
        ("endsWith", "endsWith"),
    ];
    let (kind, sub, dxf, priority, param) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?, c.u32()?);
    c.skip(8)?;
    let bits = c.u16()?;
    c.skip(12)?;
    let text_arg = c.string()?;
    let mut formulas = Vec::new();
    while !c.is_empty() {
        if let Some(f) = formula(c, at, names)? {
            formulas.push(f);
        }
    }
    let mut extra = String::new();
    let name = match (kind, sub) {
        (1, _) => {
            extra = attr("operator", OPS.get(param as usize).filter(|o| !o.is_empty())?);
            "cellIs"
        }
        (3, _) => "colorScale",
        (4, _) => "dataBar",
        (5, _) => {
            extra = format!(
                " rank=\"{param}\"{}{}",
                flag("percent", bits & 0x10 != 0),
                flag("bottom", bits & 8 != 0)
            );
            "top10"
        }
        (6, _) => "iconSet",
        (_, 7) => "uniqueValues",
        (_, 8) => {
            let (t, op) = TEXT.get(param as usize)?;
            extra = format!("{}{}", attr("operator", op), xattr("text", &text_arg));
            t
        }
        (_, 9) => "containsBlanks",
        (_, 10) => "notContainsBlanks",
        (_, 11) => "containsErrors",
        (_, 12) => "notContainsErrors",
        (_, 15..=24) => {
            const PERIODS: [&str; 10] = [
                "today",
                "tomorrow",
                "yesterday",
                "last7Days",
                "lastMonth",
                "nextMonth",
                "thisWeek",
                "nextWeek",
                "lastWeek",
                "thisMonth",
            ];
            extra = attr("timePeriod", PERIODS[sub as usize - 15]);
            "timePeriod"
        }
        (_, 25 | 26 | 29 | 30) => {
            if matches!(sub, 26 | 30) {
                extra.push_str(" aboveAverage=\"0\"");
            }
            extra.push_str(&flag("equalAverage", sub >= 29));
            if param != 0 {
                extra.push_str(&format!(" stdDev=\"{param}\""));
            }
            "aboveAverage"
        }
        (_, 27) => "duplicateValues",
        _ => "expression",
    };
    let mut s = format!("<cfRule type=\"{name}\"");
    if dxf != u32::MAX {
        s.push_str(&format!(" dxfId=\"{dxf}\""));
    }
    s.push_str(&format!(" priority=\"{priority}\""));
    s.push_str(&flag("stopIfTrue", bits & 2 != 0));
    s.push_str(&extra);
    s.push('>');
    for f in formulas {
        s.push_str(&format!("<formula>{}</formula>", text(&f)));
    }
    Some(s)
}
