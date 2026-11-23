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
    // O estilo 0 existe, como em todo arquivo.
    let mut styles = Styles::default();
    if styles.xfs.is_empty() {
        styles.fonts.push(duck_core::Font { name: "Calibri".into(), size: 11.0, ..Default::default() });
        styles.fills.push(Default::default());
        styles.borders.push(Default::default());
        styles.xfs.push(Default::default());
    }
    let styles = Arc::new(styles);
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

/// O mouse, sem janela: mesmas rotinas que os eventos chamam.
mod mouse {
    use super::*;
    use duck_platform::Modifiers;

    const NONE: Modifiers = Modifiers { ctrl: false, shift: false, alt: false, logo: false };
    const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };

    /// Um ponto dentro da célula (no meio dela).
    fn at(a: &mut SheetsApp, cell: &str) -> (f32, f32) {
        let want = parse_cell(cell).unwrap();
        let g = a.geom();
        let v = a.doc.as_ref().unwrap().current().unwrap().1;
        let inside = |x: f32, y: f32| g.grid.contains(x, y) && g.hit(v, x, y) == Some(want);
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        for y in (g.grid.y0 as i32..g.grid.y1 as i32).step_by(3) {
            for x in (g.grid.x0 as i32..g.grid.x1 as i32).step_by(3) {
                if inside(x as f32, y as f32) {
                    xs.push(x);
                    ys.push(y);
                }
            }
        }
        assert!(!xs.is_empty(), "{cell} fora da tela");
        ((xs[0] + xs[xs.len() - 1]) as f32 / 2.0, (ys[0] + ys[ys.len() - 1]) as f32 / 2.0)
    }

    fn press(a: &mut SheetsApp, p: (f32, f32), mods: Modifiers, clicks: u8) {
        let g = a.geom();
        if !a.press_handle(&g, p.0, p.1, mods, clicks) {
            a.press_cell(&g, p.0, p.1, mods, clicks);
        }
    }

    fn drag(a: &mut SheetsApp, from: (f32, f32), to: &str, mods: Modifiers) {
        press(a, from, mods, 1);
        let p = at(a, to);
        a.mouse_move(p.0, p.1);
        a.mouse_up(mods);
    }

    fn handle(a: &mut SheetsApp) -> (f32, f32) {
        let g = a.geom();
        let r = crate::fill::handle(&g, a.doc.as_ref().unwrap().current().unwrap().1).unwrap();
        ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
    }

    #[test]
    fn fill_handle() {
        let mut a = app(&[("P", &[("A1", "jan"), ("B1", "1"), ("B2", "2"), ("A2", "x"), ("A3", "y")])]);
        a.go_to(0, 0);
        let h = handle(&mut a);
        drag(&mut a, h, "A5", NONE);
        assert_eq!((get(&a, 0, "A2"), get(&a, 0, "A5")), ("fev".into(), "mai".into()));
        a.undo();
        assert_eq!((get(&a, 0, "A2"), get(&a, 0, "A5")), ("x".into(), "".into()));
        // Com Ctrl: o contrário (copia).
        a.go_to(0, 0);
        let h = handle(&mut a);
        drag(&mut a, h, "A2", CTRL);
        assert_eq!(get(&a, 0, "A2"), "jan");
        a.undo();
        // Para a direita, várias células.
        select(&mut a, "B1:B2");
        let h = handle(&mut a);
        drag(&mut a, h, "B6", NONE);
        assert_eq!(get(&a, 0, "B6"), "6");
        a.undo();
        // Duplo clique: até onde vai a coluna ao lado.
        select(&mut a, "B1:B2");
        let h = handle(&mut a);
        press(&mut a, h, NONE, 2);
        a.mouse_up(NONE);
        assert_eq!((get(&a, 0, "B3"), get(&a, 0, "B4")), ("3".into(), "".into()));
        a.undo();
        assert_eq!(get(&a, 0, "B3"), "");
        // Soltar na própria seleção não muda nada.
        select(&mut a, "B1:B2");
        let h = handle(&mut a);
        drag(&mut a, h, "B2", NONE);
        assert!(a.doc.as_ref().unwrap().history.undo.is_empty());
    }

    #[test]
    fn drag_the_border() {
        let mut a = app(&[("P", &[("A1", "7"), ("C1", "=A1*2")])]);
        for mods in [NONE, CTRL] {
            a.go_to(0, 0);
            let (x, y) = at(&mut a, "A1");
            let g = a.geom();
            // A borda de baixo, longe da alça.
            let edge = (0..40).map(|d| y + d as f32).find(|&y| {
                let v = a.doc.as_ref().unwrap().current().unwrap().1;
                crate::fill::on_border(&g, v, x, y)
            });
            drag(&mut a, (x, edge.unwrap()), "B4", mods);
            assert_eq!(get(&a, 0, "B4"), "7");
            if mods.ctrl {
                assert_eq!((get(&a, 0, "A1"), get(&a, 0, "C1")), ("7".into(), "=A1*2".into()));
            } else {
                assert_eq!((get(&a, 0, "A1"), get(&a, 0, "C1")), ("".into(), "=B4*2".into()));
            }
            a.undo();
            assert_eq!((get(&a, 0, "A1"), get(&a, 0, "B4")), ("7".into(), "".into()));
        }
    }

    #[test]
    fn several_areas_and_the_painter() {
        let mut a = app(&[("P", &[("A1", "1"), ("A2", "2"), ("C2", "5"), ("C3", "6")])]);
        let p = at(&mut a, "A1");
        drag(&mut a, p, "A2", NONE);
        let p = at(&mut a, "C2");
        drag(&mut a, p, "C3", CTRL);
        let s = a.sel_stats().unwrap();
        assert_eq!((s.count, s.sum), (4, 14.0));
        // Um clique comum volta a uma área só.
        let p = at(&mut a, "A1");
        drag(&mut a, p, "A1", NONE);
        assert!(!a.doc.as_ref().unwrap().current().unwrap().1.multi());
        // Pincel: copia o formato para o que for arrastado e desliga.
        a.apply_fmt(crate::format::Fmt::Bold);
        a.toggle_painter(false);
        let p = at(&mut a, "C2");
        drag(&mut a, p, "C3", NONE);
        assert_eq!(style(&a, 0, "C3").font, style(&a, 0, "A1").font);
        assert_ne!(style(&a, 0, "C3").font, style(&a, 0, "A2").font);
        assert!(a.painter.is_none());
        // Duplo clique edita.
        let p = at(&mut a, "C2");
        press(&mut a, p, NONE, 2);
        assert!(a.editor.is_some());
    }
}
