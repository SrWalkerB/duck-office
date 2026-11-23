//! Documento pequeno para testar o app sem janela nem GPU.

use std::sync::Arc;
use std::time::{Duration, Instant};

use duck_core::{CellValue, Sheet, Styles, Value};
use duck_text::TextSystem;
use duck_xlsx::{SheetInfo, TabOrigin};

use crate::app::{Doc, SheetsApp, Slot, build_view, parse_cell, parse_range};
use crate::look::Looks;

/// Um app com as abas dadas (nome, células `REF` → texto como digitado).
pub fn app(sheets: &[(&str, &[(&str, &str)])]) -> SheetsApp {
    let mut a = SheetsApp::build(
        Instant::now(),
        crate::Opts { save_to: Some("x".into()), ..Default::default() },
        None,
    );
    a.text = Some(TextSystem::new_fast(13.0, 1024));
    let styles = Arc::new(Styles::default());
    let looks = Arc::new(Looks::new(&styles));
    let slots = sheets
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let mut s = Sheet::new(*name);
            s.tab_id = i as u32 + 1;
            s.layout.view = Some(Default::default());
            let part = format!("xl/worksheets/sheet{}.xml", i + 1);
            Slot {
                info: SheetInfo {
                    name: name.to_string(),
                    id: i as u32 + 1,
                    part: part.clone(),
                    ..Default::default()
                },
                view: Some(build_view(&s, 7.0, None, None)),
                data: Some(Arc::new(s)),
                preview: false,
                origin: TabOrigin::File(part),
            }
        })
        .collect();
    a.doc = Some(Doc {
        path: "teste.xlsx".into(),
        sheets: slots,
        active: 0,
        styles,
        looks,
        strings: Arc::default(),
        date1904: false,
        mdw: 7.0,
        loaded: sheets.len(),
        finished: Some(Duration::ZERO),
        history: Default::default(),
        dirty: false,
        calc: crate::calc::Calc::new(Arc::default()),
        pivots: Default::default(),
        macros: None,
        macros_ok: false,
    });
    for (i, (_, cells)) in sheets.iter().enumerate() {
        a.switch_sheet(i);
        for (cell, text) in *cells {
            type_in(&mut a, cell, text);
        }
    }
    a.switch_sheet(0);
    let doc = a.doc.as_mut().unwrap();
    doc.history = Default::default();
    doc.dirty = false;
    a
}

/// Digita numa célula da aba ativa.
pub fn type_in(a: &mut SheetsApp, cell: &str, text: &str) {
    let (r, c) = parse_cell(cell).unwrap();
    a.go_to(r, c);
    a.start_edit(Some(text), false);
    a.commit_edit();
}

/// Seleciona `A1:B2` (célula ativa no fim, como um arraste).
pub fn select(a: &mut SheetsApp, range: &str) {
    let (x, y) = parse_range(range).unwrap();
    a.go_to(x.0, x.1);
    let v = a.doc.as_mut().unwrap().view_mut().unwrap();
    v.anchor = x;
    v.cur = y;
}

/// Conteúdo de uma célula: fórmula (com `=`, em português) ou valor.
pub fn get(a: &SheetsApp, sheet: usize, cell: &str) -> String {
    let (r, c) = parse_cell(cell).unwrap();
    let doc = a.doc.as_ref().unwrap();
    let s = doc.sheets[sheet].data.as_ref().unwrap();
    if let Some(f) = duck_calc::cell_formula(s, r, c) {
        return format!("={}", duck_calc::locale::to_pt(&f));
    }
    match s.cell(r, c, &doc.strings).value {
        Value::Empty => String::new(),
        Value::Number(x) => format!("{x}").replace('.', ","),
        Value::Text(t) => t.to_string(),
        Value::Bool(b) => format!("{b}"),
        Value::Error(e) => e.as_str().to_string(),
    }
}

/// Estilo de uma célula.
pub fn style(a: &SheetsApp, sheet: usize, cell: &str) -> duck_core::CellXf {
    let (r, c) = parse_cell(cell).unwrap();
    let doc = a.doc.as_ref().unwrap();
    let s = doc.sheets[sheet].data.as_ref().unwrap().cell(r, c, &doc.strings).style;
    doc.styles.xfs.get(s as usize).copied().unwrap_or_default()
}

pub fn names(a: &SheetsApp) -> Vec<String> {
    a.doc.as_ref().unwrap().sheets.iter().map(|s| s.info.name.clone()).collect()
}

/// Um valor de célula qualquer (para montar casos).
#[allow(dead_code)]
pub fn num(x: f64) -> CellValue {
    CellValue::Number(x)
}
