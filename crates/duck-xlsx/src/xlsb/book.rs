//! `workbook.bin`, `tableN.bin` e `externalLinkN.bin` → XML, e os nomes que
//! as fórmulas citam por número ([`Names`]).

use std::sync::Arc;

use duck_calc::sheetrefs::quoted;

use super::out::{attr, cell, flag, range, text, xattr};
use super::ptg::{self, Names, Xti};
use super::rec::{self, Cur, Records, id};

const MAIN: &str = "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"";
const RELS: &str = "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";
const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Uma aba de `BrtBundleSh`.
pub struct Tab {
    pub state: u32,
    pub id: u32,
    pub rid: String,
    pub name: String,
}

/// Origem de referências (`BrtSup*`).
enum Sup {
    Own,
    /// Arquivo externo: `r:id` e o número `[n]` (a partir de 1).
    Book(String, usize),
    Other,
}

/// Opções de cálculo.
struct Calc {
    mode: u32,
    iterations: u32,
    delta: f64,
    bits: u16,
}

struct Name<'a> {
    bits: u32,
    /// Aba do nome (`u32::MAX` = arquivo).
    tab: u32,
    name: String,
    formula: Option<(&'a [u8], &'a [u8])>,
}

/// O `workbook.bin` lido.
#[derive(Default)]
pub struct Book<'a> {
    pub tabs: Vec<Tab>,
    names: Vec<Name<'a>>,
    sups: Vec<Sup>,
    xti: Vec<(u32, i32, i32)>,
    date1904: bool,
    code_name: String,
    active: u32,
    calc: Option<Calc>,
}

impl<'a> Book<'a> {
    pub fn parse(bin: &'a [u8]) -> Self {
        let mut w = Self::default();
        let mut externals = 0;
        let mut seen_view = false;
        for (kind, b) in Records::new(bin) {
            let mut c = Cur::new(b);
            match kind {
                id::BUNDLE_SH => {
                    if let (Some(state), Some(id), Some(rid), Some(name)) =
                        (c.u32(), c.u32(), c.string(), c.string())
                    {
                        w.tabs.push(Tab { state, id, rid, name });
                    }
                }
                id::WB_PROP => {
                    w.date1904 = c.u32().unwrap_or(0) & 1 != 0;
                    let _ = c.u32();
                    w.code_name = c.string().unwrap_or_default();
                }
                id::BOOK_VIEW if !seen_view => {
                    seen_view = true;
                    w.active = c.skip(24).and_then(|_| c.u32()).unwrap_or(0);
                }
                id::CALC_PROP => w.calc = calc(&mut c),
                id::NAME => w.names.push(name(&mut c)),
                id::SUP_BOOK_SRC => {
                    externals += 1;
                    w.sups.push(Sup::Book(c.string().unwrap_or_default(), externals));
                }
                id::SUP_SELF | id::SUP_SAME => w.sups.push(Sup::Own),
                id::SUP_ADDIN => w.sups.push(Sup::Other),
                id::EXTERN_SHEET => {
                    for _ in 0..c.u32().unwrap_or(0) {
                        let (Some(s), Some(a), Some(b)) = (c.u32(), c.i32(), c.i32()) else { break };
                        w.xti.push((s, a, b));
                    }
                }
                _ => {}
            }
        }
        w
    }

    /// `r:id` dos arquivos externos, na ordem do `[n]`.
    pub fn externals(&self) -> impl Iterator<Item = &str> {
        self.sups.iter().filter_map(|s| match s {
            Sup::Book(rid, _) => Some(rid.as_str()),
            _ => None,
        })
    }

    /// O que as fórmulas citam. `external` lê (abas, nomes) de um arquivo
    /// externo pelo `r:id`; `is_sheet` diz se a aba é de planilha.
    pub fn names(
        &self,
        tables: std::collections::HashMap<u32, (String, Vec<String>)>,
        is_sheet: impl Fn(&Tab) -> bool,
        external: impl Fn(&str) -> (Vec<String>, Vec<String>),
    ) -> Names {
        let books: Vec<_> = self
            .sups
            .iter()
            .map(|s| match s {
                Sup::Book(rid, _) => {
                    let (tabs, names) = external(rid);
                    Some((tabs, Arc::new(names)))
                }
                _ => None,
            })
            .collect();
        let own = |i: i32| self.tabs.get(usize::try_from(i).ok()?).filter(|t| is_sheet(t)).map(|t| &t.name);
        let xti = self
            .xti
            .iter()
            .map(|&(sup, first, last)| match self.sups.get(sup as usize) {
                Some(Sup::Own) => {
                    let prefix = match (first, last) {
                        // Nome do arquivo todo: sem aba.
                        (-2, _) => Some(String::new()),
                        _ => own(first).zip(own(last)).map(|(a, b)| qualifier("", a, b)),
                    };
                    Xti { prefix, ..Default::default() }
                }
                Some(Sup::Book(_, n)) => {
                    let (tabs, names) = books[sup as usize].as_ref().expect("arquivo externo lido");
                    let tab = |i: i32| tabs.get(usize::try_from(i).ok()?);
                    let book = format!("[{n}]");
                    let prefix = tab(first).zip(tab(last)).map(|(a, b)| qualifier(&book, a, b));
                    Xti { prefix, book: format!("{book}!"), names: Some(names.clone()) }
                }
                // Suplementos (XLL): os nomes das funções não estão no arquivo.
                _ => Xti { names: Some(Arc::default()), ..Default::default() },
            })
            .collect();
        Names { names: self.names.iter().map(|n| n.name.clone()).collect(), xti, tables }
    }

    /// O `workbook.xml` equivalente, só com as abas de planilha.
    pub fn to_xml(&self, names: &Names, is_sheet: impl Fn(&Tab) -> bool, unread: &mut u64) -> Vec<u8> {
        let mut s = format!("{HEADER}<workbook {MAIN} {RELS}><workbookPr");
        s.push_str(&flag("date1904", self.date1904));
        if !self.code_name.is_empty() {
            s.push_str(&attr("codeName", &self.code_name));
        }
        // As abas de gráfico e de macro não entram: os índices andam.
        let kept: Vec<usize> = (0..self.tabs.len()).filter(|&i| is_sheet(&self.tabs[i])).collect();
        let place = |tab: u32| kept.iter().position(|&i| i == tab as usize);
        let active = place(self.active).unwrap_or(0);
        s.push_str(&format!("/><bookViews><workbookView activeTab=\"{active}\"/></bookViews><sheets>"));
        for t in kept.iter().map(|&i| &self.tabs[i]) {
            s.push_str(&format!("<sheet{} sheetId=\"{}\"", attr("name", &t.name), t.id));
            match t.state {
                1 => s.push_str(" state=\"hidden\""),
                2 => s.push_str(" state=\"veryHidden\""),
                _ => {}
            }
            s.push_str(&attr("r:id", &t.rid));
            s.push_str("/>");
        }
        s.push_str("</sheets>");
        let ext: String =
            self.externals().map(|rid| format!("<externalReference{}/>", attr("r:id", rid))).collect();
        if !ext.is_empty() {
            s.push_str(&format!("<externalReferences>{ext}</externalReferences>"));
        }
        let mut defined = String::new();
        for n in &self.names {
            // Nomes de função (`_xlfn.XLOOKUP`, macros) não têm fórmula própria.
            if n.bits & (F_FUNC | F_PROC) != 0 || n.name.starts_with("_xl") && !n.name.starts_with("_xlnm.") {
                continue;
            }
            let tab = match n.tab {
                u32::MAX => None,
                t => match place(t) {
                    Some(i) => Some(i),
                    None => continue,
                },
            };
            let Some(f) = n.formula.filter(|f| !f.0.is_empty()) else { continue };
            let Some(f) = ptg::text(f.0, f.1, (0, 0), names) else {
                *unread += 1;
                continue;
            };
            defined.push_str(&format!("<definedName{}", attr("name", &n.name)));
            if let Some(i) = tab {
                defined.push_str(&format!(" localSheetId=\"{i}\""));
            }
            defined.push_str(&flag("hidden", n.bits & 1 != 0));
            defined.push_str(&format!(">{}</definedName>", text(&f)));
        }
        if !defined.is_empty() {
            s.push_str(&format!("<definedNames>{defined}</definedNames>"));
        }
        s.push_str("<calcPr calcId=\"191029\"");
        if let Some(c) = &self.calc {
            match c.mode {
                0 => s.push_str(" calcMode=\"manual\""),
                2 => s.push_str(" calcMode=\"autoNoTable\""),
                _ => {}
            }
            if c.bits & 4 != 0 {
                s.push_str(&format!(
                    " iterate=\"1\" iterateCount=\"{}\" iterateDelta=\"{}\"",
                    c.iterations, c.delta
                ));
            }
        }
        s.push_str("/></workbook>");
        s.into_bytes()
    }
}

const F_FUNC: u32 = 1 << 1;
const F_PROC: u32 = 1 << 3;
const F_BUILTIN: u32 = 1 << 5;

fn calc(c: &mut Cur) -> Option<Calc> {
    let (_, mode, iterations, delta) = (c.u32()?, c.u32()?, c.u32()?, c.f64()?);
    let (_threads, bits) = (c.u32()?, c.u16()?);
    Some(Calc { mode, iterations, delta, bits })
}

/// Um `BrtName`. Mesmo truncado, ocupa o lugar dele (os tokens contam).
fn name<'a>(c: &mut Cur<'a>) -> Name<'a> {
    let bits = c.u32().unwrap_or(0);
    let _ = c.u8();
    let tab = c.u32().unwrap_or(u32::MAX);
    let mut name = c.string().unwrap_or_default();
    if bits & F_BUILTIN != 0 && !name.starts_with("_xlnm.") {
        name.insert_str(0, "_xlnm.");
    }
    Name { bits, tab, name, formula: c.formula() }
}

/// `Aba!`, `'Minha aba'!`, `[1]Aba!`, `'[1]Jan 1:Mar 1'!`.
fn qualifier(book: &str, first: &str, last: &str) -> String {
    let plain = quoted(first) == first && quoted(last) == last;
    let mut s = format!("{book}{first}");
    if first != last {
        s.push(':');
        s.push_str(last);
    }
    if plain { format!("{s}!") } else { format!("'{}'!", s.replace('\'', "''")) }
}

// ---------------------------------------------------------------------------
// Tabelas
// ---------------------------------------------------------------------------

/// `idList`, nome e colunas de uma tabela.
pub fn table_columns(bin: &[u8]) -> Option<(u32, (String, Vec<String>))> {
    let mut found = None;
    let mut cols = Vec::new();
    for (kind, b) in Records::new(bin) {
        let mut c = Cur::new(b);
        match kind {
            id::BEGIN_LIST => {
                c.skip(20)?;
                let id = c.u32()?;
                c.skip(40)?;
                let (name, display) = (c.string()?, c.string()?);
                found = Some((id, if display.is_empty() { name } else { display }));
            }
            id::BEGIN_LIST_COL => {
                c.skip(24)?;
                let (unique, caption) = (c.string()?, c.string()?);
                cols.push(if caption.is_empty() { unique } else { caption });
            }
            _ => {}
        }
    }
    found.map(|(id, name)| (id, (name, cols)))
}

/// O `tableN.xml` equivalente.
pub fn table_xml(bin: &[u8], names: &Names, unread: &mut u64) -> Option<Vec<u8>> {
    const TOTALS: [&str; 10] =
        ["none", "average", "count", "countNums", "max", "min", "sum", "stdDev", "var", "custom"];
    let (mut head, mut filter, mut cols, mut style) =
        (String::new(), String::new(), String::new(), String::new());
    let mut af = super::frame::FilterXml::default();
    let mut n = 0;
    // Fórmulas valem na primeira linha de dados.
    let mut origin = (0, 0);
    let mut open = false;
    for (kind, b) in Records::new(bin) {
        let mut c = Cur::new(b);
        if af.record(kind, b, &mut filter) {
            continue;
        }
        match kind {
            id::BEGIN_LIST => {
                let r = c.range()?;
                let (_source, tid, header, totals) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?);
                let bits = c.u32()?;
                c.skip(28)?;
                let (name, display) = (c.string()?, c.string()?);
                origin = (r.first_row.saturating_add(header).min(rec::LAST_ROW), r.first_col);
                let display = if display.is_empty() { &name } else { &display };
                head = format!(
                    "{HEADER}<table {MAIN} id=\"{tid}\"{}{} ref=\"{}\"",
                    attr("name", if name.is_empty() { display } else { &name }),
                    attr("displayName", display),
                    range(&r)
                );
                if header != 1 {
                    head.push_str(&format!(" headerRowCount=\"{header}\""));
                }
                match totals {
                    0 => head.push_str(&format!(" totalsRowShown=\"{}\"", bits & 1)),
                    t => head.push_str(&format!(" totalsRowCount=\"{t}\"")),
                }
                head.push('>');
            }
            id::BEGIN_LIST_COL => {
                let (field, total) = (c.u32()?, c.u32()? as usize);
                c.skip(16)?;
                let (unique, caption, label) = (c.string()?, c.string()?, c.string()?);
                let name = if caption.is_empty() { unique } else { caption };
                cols.push_str(&format!("<tableColumn id=\"{field}\"{}", xattr("name", &name)));
                if !label.is_empty() {
                    cols.push_str(&xattr("totalsRowLabel", &label));
                }
                if total != 0 {
                    cols.push_str(&attr("totalsRowFunction", TOTALS.get(total)?));
                }
                cols.push('>');
                n += 1;
                open = true;
            }
            id::END_LIST_COL if open => {
                cols.push_str("</tableColumn>");
                open = false;
            }
            id::LIST_CC_FMLA | id::LIST_TR_FMLA if open => {
                let tag =
                    if kind == id::LIST_CC_FMLA { "calculatedColumnFormula" } else { "totalsRowFormula" };
                let array = c.u8()? & 1 != 0;
                match c.formula().and_then(|f| ptg::text(f.0, f.1, origin, names)) {
                    Some(f) => cols.push_str(&format!("<{tag}{}>{}</{tag}>", flag("array", array), text(&f))),
                    None => *unread += 1,
                }
            }
            id::TABLE_STYLE_CLIENT => {
                let (bits, name) = (c.u16()?, c.string()?);
                style = format!("<tableStyleInfo{}", attr("name", &name));
                for (k, a) in ["showFirstColumn", "showLastColumn", "showRowStripes", "showColumnStripes"]
                    .iter()
                    .enumerate()
                {
                    style.push_str(&format!(" {a}=\"{}\"", bits >> k & 1));
                }
                style.push_str("/>");
            }
            _ => {}
        }
    }
    if head.is_empty() {
        return None;
    }
    let body = format!("<tableColumns count=\"{n}\">{cols}</tableColumns>");
    Some(format!("{head}{filter}{body}{style}</table>").into_bytes())
}

// ---------------------------------------------------------------------------
// Arquivos externos
// ---------------------------------------------------------------------------

/// Nomes das abas e nomes definidos de um arquivo externo.
pub fn external_names(bin: &[u8]) -> (Vec<String>, Vec<String>) {
    let (mut tabs, mut names) = (Vec::new(), Vec::new());
    for (kind, b) in Records::new(bin) {
        let mut c = Cur::new(b);
        match kind {
            id::SUP_TABS => tabs = sup_tabs(&mut c),
            id::SUP_NAME_START => names.push(c.string().unwrap_or_default()),
            _ => {}
        }
    }
    (tabs, names)
}

fn sup_tabs(c: &mut Cur) -> Vec<String> {
    let n = c.u32().unwrap_or(0) as usize;
    let mut v = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let Some(s) = c.string() else { break };
        v.push(s);
    }
    v
}

/// O `externalLinkN.xml` equivalente (com os valores guardados).
pub fn external_xml(bin: &[u8]) -> Vec<u8> {
    let (mut rid, mut tabs, mut names) = (String::new(), Vec::new(), Vec::<String>::new());
    let mut data = String::new();
    let (mut row, mut open_row) = (0u32, false);
    for (kind, b) in Records::new(bin) {
        let mut c = Cur::new(b);
        match kind {
            // Tipo 0 = pasta de trabalho (1 = DDE, 2 = OLE).
            id::BEGIN_SUP_BOOK => {
                if c.u16() == Some(0) {
                    rid = c.string().unwrap_or_default();
                }
            }
            id::SUP_TABS => tabs = sup_tabs(&mut c),
            id::SUP_NAME_START => names.push(c.string().unwrap_or_default()),
            id::EXTERN_TABLE_START => {
                data.push_str(&format!("<sheetData sheetId=\"{}\">", c.u32().unwrap_or(0)));
            }
            id::EXTERN_ROW => {
                if open_row {
                    data.push_str("</row>");
                }
                row = c.u32().unwrap_or(0).min(rec::LAST_ROW);
                open_row = true;
                data.push_str(&format!("<row r=\"{}\">", row + 1));
            }
            id::EXTERN_TABLE_END => {
                if std::mem::take(&mut open_row) {
                    data.push_str("</row>");
                }
                data.push_str("</sheetData>");
            }
            id::EXTERN_BLANK..=id::EXTERN_STRING => {
                let Some(col) = c.u32() else { continue };
                if kind == id::EXTERN_BLANK {
                    data.push_str(&format!("<cell r=\"{}\"/>", cell(row, col)));
                    continue;
                }
                let (t, v) = match kind {
                    id::EXTERN_REAL => ("", c.f64().unwrap_or(0.0).to_string()),
                    id::EXTERN_BOOL => (" t=\"b\"", u8::from(c.u8() == Some(1)).to_string()),
                    id::EXTERN_ERROR => (" t=\"e\"", rec::error(c.u8().unwrap_or(0x2A)).as_str().to_string()),
                    _ => (" t=\"str\"", text(&c.string().unwrap_or_default())),
                };
                data.push_str(&format!("<cell r=\"{}\"{t}><v>{v}</v></cell>", cell(row, col)));
            }
            _ => {}
        }
    }
    let mut s = format!("{HEADER}<externalLink {MAIN} {RELS}><externalBook");
    if !rid.is_empty() {
        s.push_str(&attr("r:id", &rid));
    }
    s.push('>');
    if !tabs.is_empty() {
        let list: String = tabs.iter().map(|t| format!("<sheetName{}/>", attr("val", t))).collect();
        s.push_str(&format!("<sheetNames>{list}</sheetNames>"));
    }
    if !names.is_empty() {
        let list: String = names.iter().map(|n| format!("<definedName{}/>", attr("name", n))).collect();
        s.push_str(&format!("<definedNames>{list}</definedNames>"));
    }
    if !data.is_empty() {
        s.push_str(&format!("<sheetDataSet>{data}</sheetDataSet>"));
    }
    s.push_str("</externalBook></externalLink>");
    s.into_bytes()
}
