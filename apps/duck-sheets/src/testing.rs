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
        binary: None,
        macro_free: Default::default(),
        protection: None,
        protection_changed: false,
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
pub mod mouse {
    use super::*;
    use duck_platform::Modifiers;

    pub const NONE: Modifiers = Modifiers { ctrl: false, shift: false, alt: false, logo: false };
    pub const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };

    /// Um ponto dentro da célula (no meio dela).
    pub fn at(a: &mut SheetsApp, cell: &str) -> (f32, f32) {
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

    pub fn press(a: &mut SheetsApp, p: (f32, f32), mods: Modifiers, clicks: u8) {
        let g = a.geom();
        if a.press_grip(&g, p.0, p.1, mods) || a.press_object(&g, p.0, p.1, mods) {
            return;
        }
        if !a.press_headers(&g, p.0, p.1, mods, clicks) && !a.press_handle(&g, p.0, p.1, mods, clicks) {
            a.press_cell(&g, p.0, p.1, mods, clicks);
        }
    }

    pub fn drag(a: &mut SheetsApp, from: (f32, f32), to: &str, mods: Modifiers) {
        press(a, from, mods, 1);
        let p = at(a, to);
        a.mouse_move(p.0, p.1);
        a.mouse_up(mods);
    }

    pub fn handle(a: &mut SheetsApp) -> (f32, f32) {
        let g = a.geom();
        let r = crate::fill::handle(&g, a.doc.as_ref().unwrap().current().unwrap().1).unwrap();
        ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
    }

    /// 13 arrastado: 14, 15… (com Ctrl, 13 de novo).
    #[test]
    fn fill_a_lone_number() {
        for (to, cells) in [("D12", ["D11", "D12"]), ("F10", ["E10", "F10"])] {
            let mut a = app(&[("P", &[("D10", "13")])]);
            a.go_to(9, 3);
            let h = handle(&mut a);
            drag(&mut a, h, to, NONE);
            assert_eq!(cells.map(|c| get(&a, 0, c)), ["14", "15"]);
            a.undo();
            assert_eq!(cells.map(|c| get(&a, 0, c)), ["", ""]);
            a.go_to(9, 3);
            let h = handle(&mut a);
            drag(&mut a, h, to, CTRL);
            assert_eq!(cells.map(|c| get(&a, 0, c)), ["13", "13"]);
        }
        // Moeda também conta; para cima, desce.
        let mut a = app(&[("P", &[("D10", "R$ 13,00")])]);
        a.go_to(9, 3);
        let h = handle(&mut a);
        drag(&mut a, h, "D8", NONE);
        assert_eq!((get(&a, 0, "D9"), get(&a, 0, "D8")), ("12".into(), "11".into()));
        assert_eq!(style(&a, 0, "D8").num_fmt, style(&a, 0, "D10").num_fmt);
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

    /// Seleciona a célula e dá um ponto na borda de baixo dela, longe da alça.
    pub fn border(a: &mut SheetsApp, cell: &str) -> (f32, f32) {
        let (r, c) = parse_cell(cell).unwrap();
        a.go_to(r, c);
        let (x, y) = at(a, cell);
        let g = a.geom();
        let v = a.doc.as_ref().unwrap().current().unwrap().1;
        let y = (0..40).map(|d| y + d as f32).find(|&y| crate::fill::on_border(&g, v, x, y));
        (x, y.unwrap())
    }

    pub fn mouse_up(a: &mut SheetsApp) {
        a.mouse_up(NONE);
    }

    /// Um ponto no cabeçalho, a `d` pixels da borda direita da coluna.
    pub fn col_edge(a: &mut SheetsApp, c: u32, d: f32) -> (f32, f32) {
        let g = a.geom();
        let v = a.doc.as_ref().unwrap().current().unwrap().1;
        let q = &g.quads[1];
        let x = crate::draw::pos(&v.cols, q.ox, c + 1, q.rect.x0, g.g);
        (x + d, (g.colhdr.y0 + g.colhdr.y1) / 2.0)
    }

    pub fn row_edge(a: &mut SheetsApp, r: u32, d: f32) -> (f32, f32) {
        let g = a.geom();
        let v = a.doc.as_ref().unwrap().current().unwrap().1;
        let q = &g.quads[2];
        let y = crate::draw::pos(&v.rows, q.oy, r + 1, q.rect.y0, g.g);
        ((g.rowhdr.x0 + g.rowhdr.x1) / 2.0, y + d)
    }

    pub fn col_px(a: &SheetsApp, c: u32) -> u32 {
        a.doc.as_ref().unwrap().current().unwrap().1.cols.size(c)
    }

    pub fn row_px(a: &SheetsApp, r: u32) -> u32 {
        a.doc.as_ref().unwrap().current().unwrap().1.rows.size(r)
    }

    pub fn pull(a: &mut SheetsApp, from: (f32, f32), dx: f32, dy: f32) {
        press(a, from, NONE, 1);
        a.mouse_move(from.0 + dx / 2.0, from.1 + dy / 2.0);
        a.mouse_move(from.0 + dx, from.1 + dy);
        a.mouse_up(NONE);
    }

    #[test]
    fn widen_by_the_header_edge() {
        for (scale, zoom, frozen) in
            [(1.0, 100, 0), (1.0, 100, 2), (1.25, 100, 0), (1.5, 90, 0), (2.0, 150, 1)]
        {
            let mut a = app(&[("P", &[("D10", "13")])]);
            let pane = duck_core::Pane { frozen_rows: frozen, frozen_cols: frozen };
            if frozen > 0 {
                a.run_command(duck_core::Command::Pane(Some(pane)));
                a.doc.as_mut().unwrap().history = Default::default();
            }
            a.scale = scale;
            a.set_zoom(zoom);
            let before = col_px(&a, 3);
            let g = a.geom().g;
            // De qualquer lado da borda.
            for d in [-4.0, 0.0, 4.0] {
                let p = col_edge(&mut a, 3, d * scale);
                pull(&mut a, p, 40.0 * g, 0.0);
                assert_eq!(col_px(&a, 3), before + 40, "escala {scale}, zoom {zoom}, folga {d}");
                assert!(a.doc.as_ref().unwrap().current().unwrap().0.col_width(3).is_some());
                a.undo();
                assert_eq!(col_px(&a, 3), before);
            }
            let before = row_px(&a, 4);
            let p = row_edge(&mut a, 4, 0.0);
            pull(&mut a, p, 0.0, 25.0 * g);
            assert_eq!(row_px(&a, 4), before + 25);
            a.undo();
            assert_eq!(row_px(&a, 4), before);
            // Longe da borda, seleciona a coluna (e não muda nada).
            let p = col_edge(&mut a, 3, -(before as f32) * g / 2.0 - 8.0 * scale);
            pull(&mut a, p, 0.0, 0.0);
            let v = a.doc.as_ref().unwrap().current().unwrap().1;
            assert_eq!((v.anchor, v.cur.1), ((0, 3), 3));
            assert!(a.doc.as_ref().unwrap().history.undo.is_empty());
        }
    }

    #[test]
    fn widen_all_selected() {
        let mut a = app(&[("P", &[("B1", "um texto bem mais largo que a coluna"), ("C1", "1")])]);
        let before = col_px(&a, 1);
        // De B a D, e depois a borda da C.
        let (b, d) = (col_edge(&mut a, 1, -20.0), col_edge(&mut a, 3, -20.0));
        press(&mut a, b, NONE, 1);
        a.mouse_move(d.0, d.1);
        a.mouse_up(NONE);
        let p = col_edge(&mut a, 2, 0.0);
        // Só um clique na borda não iguala as outras.
        pull(&mut a, p, 0.0, 0.0);
        assert!(a.doc.as_ref().unwrap().history.undo.is_empty());
        pull(&mut a, p, 30.0, 0.0);
        let all: Vec<u32> = (0..5).map(|c| col_px(&a, c)).collect();
        assert_eq!(all, [before, before + 30, before + 30, before + 30, before]);
        a.undo();
        assert_eq!(col_px(&a, 1), before);
        // Duplo clique ajusta cada uma ao que mostra.
        let p = col_edge(&mut a, 2, 0.0);
        press(&mut a, p, NONE, 2);
        a.mouse_up(NONE);
        assert!(col_px(&a, 1) > before && col_px(&a, 2) < before, "{:?}", (col_px(&a, 1), col_px(&a, 2)));
        a.undo();
        assert_eq!((col_px(&a, 1), col_px(&a, 2)), (before, before));
        // Fora da seleção, só a própria.
        let p = col_edge(&mut a, 5, 0.0);
        pull(&mut a, p, 30.0, 0.0);
        assert_eq!((col_px(&a, 1), col_px(&a, 5)), (before, before + 30));
        // Linhas 2 a 4.
        let before = row_px(&a, 1);
        let (b, d) = (row_edge(&mut a, 1, -10.0), row_edge(&mut a, 3, -10.0));
        press(&mut a, b, NONE, 1);
        a.mouse_move(d.0, d.1);
        a.mouse_up(NONE);
        let p = row_edge(&mut a, 3, 0.0);
        pull(&mut a, p, 0.0, 12.0);
        let all: Vec<u32> = (0..5).map(|r| row_px(&a, r)).collect();
        assert_eq!(all, [before, before + 12, before + 12, before + 12, before]);
    }

    const SHIFT: Modifiers = Modifiers { ctrl: false, shift: true, alt: false, logo: false };

    /// Põe objetos na aba ativa, cada um com a sua âncora, em pixels a 100%.
    pub fn put(a: &mut SheetsApp, list: Vec<([f64; 4], duck_core::drawing::ObjectKind, bool)>) {
        use duck_core::drawing::{Anchor, DrawingAnchor, Object};
        let slot = &mut a.doc.as_mut().unwrap().sheets[0];
        let v = slot.view.as_ref().unwrap();
        let sheet = Arc::get_mut(slot.data.as_mut().unwrap()).unwrap();
        sheet.clear_edits();
        let l = &mut sheet.layout;
        for (i, (rect, kind, two)) in list.into_iter().enumerate() {
            let base = match two {
                true => Anchor::Two { from: Default::default(), to: Default::default() },
                false => Anchor::One { from: Default::default(), cx: 0, cy: 0 },
            };
            let anchor = crate::objects::anchor_at(rect, &base, v);
            let mut o = Object::new(anchor, kind);
            o.slot = i as u32;
            l.objects.push(o);
            l.drawing_anchors.push(DrawingAnchor { part: "d.xml".into(), anchor, fallback: false });
        }
    }

    pub fn chart() -> duck_core::drawing::ObjectKind {
        duck_core::drawing::ObjectKind::Chart(Box::default())
    }

    pub fn rect_of(a: &SheetsApp, i: usize) -> [f64; 4] {
        let (s, v) = a.doc.as_ref().unwrap().current().unwrap();
        crate::chart::anchor_rect(&s.layout.objects[i].anchor, v)
    }

    /// O ponto da aba (pixels a 100%) na tela.
    pub fn on_screen(a: &mut SheetsApp, x: f64, y: f64) -> (f32, f32) {
        let g = a.geom();
        let q = &g.quads[3];
        (q.rect.x0 + ((x - q.ox) * g.g as f64) as f32, q.rect.y0 + ((y - q.oy) * g.g as f64) as f32)
    }

    pub fn pull_to(a: &mut SheetsApp, from: (f64, f64), d: (f64, f64), mods: Modifiers) {
        let p = on_screen(a, from.0, from.1);
        let g = a.geom().g as f64;
        press(a, p, mods, 1);
        for k in [0.5, 1.0] {
            a.mouse_move(p.0 + (d.0 * g * k) as f32, p.1 + (d.1 * g * k) as f32);
        }
        a.mouse_up(mods);
    }

    #[test]
    fn drag_and_resize_a_chart() {
        for (scale, zoom, two) in [(1.0, 100, true), (1.0, 100, false), (1.5, 80, true), (2.0, 60, false)] {
            let mut a = app(&[("P", &[("A1", "1")])]);
            a.scale = scale;
            a.set_zoom(zoom);
            put(&mut a, vec![([100.0, 60.0, 200.0, 120.0], chart(), two)]);
            let start = rect_of(&a, 0);
            assert_eq!(start, [100.0, 60.0, 200.0, 120.0]);
            let sel = a.doc.as_ref().unwrap().current().unwrap().1.sel();
            // Só um clique: seleciona o gráfico, e não a célula de trás.
            pull_to(&mut a, (180.0, 100.0), (0.0, 0.0), NONE);
            assert_eq!(a.object, Some(0));
            assert_eq!(a.doc.as_ref().unwrap().current().unwrap().1.sel(), sel);
            assert!(a.doc.as_ref().unwrap().history.undo.is_empty());
            // Arrastar pelo meio.
            pull_to(&mut a, (180.0, 100.0), (50.0, 30.0), NONE);
            assert_eq!(rect_of(&a, 0), [150.0, 90.0, 200.0, 120.0], "{scale} {zoom} {two}");
            assert!(a.moving.is_none());
            // Aumentar pelo canto de baixo e diminuir pelo lado esquerdo.
            pull_to(&mut a, (350.0, 210.0), (40.0, 20.0), NONE);
            assert_eq!(rect_of(&a, 0), [150.0, 90.0, 240.0, 140.0]);
            pull_to(&mut a, (150.0, 160.0), (30.0, 0.0), NONE);
            assert_eq!(rect_of(&a, 0), [180.0, 90.0, 210.0, 140.0]);
            // Com Shift: um eixo só, ou a mesma proporção.
            pull_to(&mut a, (250.0, 150.0), (10.0, 40.0), SHIFT);
            assert_eq!(rect_of(&a, 0), [180.0, 130.0, 210.0, 140.0]);
            let e = a.doc.as_ref().unwrap().current().unwrap().0.edits();
            assert!(e.is_drawings_only());
            for _ in 0..4 {
                a.undo();
            }
            assert_eq!(rect_of(&a, 0), start);
            a.redo();
            assert_eq!(rect_of(&a, 0), [150.0, 90.0, 200.0, 120.0]);
        }
    }

    #[test]
    fn keys_and_clicks_around_a_chart() {
        use duck_platform::Key;
        let mut a = app(&[("P", &[("A1", "1"), ("B5", "2")])]);
        put(&mut a, vec![([100.0, 60.0, 200.0, 120.0], chart(), true)]);
        pull_to(&mut a, (180.0, 100.0), (0.0, 0.0), NONE);
        assert!(a.object_key(&Key::ArrowRight, NONE) && a.object_key(&Key::ArrowDown, CTRL));
        assert_eq!(rect_of(&a, 0), [108.0, 61.0, 200.0, 120.0]);
        // Delete não apaga as células de trás.
        assert!(a.object_key(&Key::Delete, NONE));
        assert_eq!(get(&a, 0, "B5"), "2");
        // Esc no meio do arraste desiste dele; de novo, tira a seleção.
        let p = on_screen(&mut a, 180.0, 100.0);
        press(&mut a, p, NONE, 1);
        a.mouse_move(p.0 + 60.0, p.1);
        assert!(a.moving.is_some());
        assert!(a.object_key(&Key::Escape, NONE));
        a.mouse_up(NONE);
        assert_eq!(rect_of(&a, 0), [108.0, 61.0, 200.0, 120.0]);
        assert!(a.object_key(&Key::Escape, NONE) && a.object.is_none());
        assert!(!a.object_key(&Key::ArrowRight, NONE));
        // Clicar numa célula tira a seleção do objeto.
        pull_to(&mut a, (180.0, 100.0), (0.0, 0.0), NONE);
        assert!(a.object.is_some());
        let p = at(&mut a, "A1");
        press(&mut a, p, NONE, 1);
        a.mouse_up(NONE);
        assert!(a.object.is_none());
        assert_eq!(a.doc.as_ref().unwrap().current().unwrap().1.sel(), (0, 0, 0, 0));
    }

    /// O objeto de cima ganha; uma forma vazada só pega na borda.
    #[test]
    fn what_is_on_top() {
        use duck_core::drawing::ObjectKind;
        let mut a = app(&[("P", &[("A1", "1")])]);
        let hollow = ObjectKind::Shape(Box::new(duck_core::shape::Shape {
            geom: duck_core::shape::Geom::Preset("rect".into(), vec![]),
            fill: duck_core::shape::Fill::None,
            line: None,
            shadow: None,
            text: Default::default(),
            rot: 0.0,
            flip_h: false,
            flip_v: false,
        }));
        put(
            &mut a,
            vec![
                ([100.0, 60.0, 200.0, 120.0], chart(), true),
                ([150.0, 80.0, 200.0, 120.0], chart(), true),
                ([20.0, 20.0, 400.0, 260.0], hollow, true),
            ],
        );
        for (p, want) in [
            ((160.0, 100.0), Some(1)),
            ((110.0, 70.0), Some(0)),
            ((21.0, 150.0), Some(2)),
            ((60.0, 250.0), None),
        ] {
            a.object = None;
            pull_to(&mut a, p, (0.0, 0.0), NONE);
            assert_eq!(a.object, want, "{p:?}");
        }
    }

    /// Os objetos de um grupo andam juntos.
    #[test]
    fn groups_move_together() {
        let mut a = app(&[("P", &[("A1", "1")])]);
        put(&mut a, vec![([100.0, 60.0, 200.0, 120.0], chart(), true)]);
        let l = &mut Arc::get_mut(a.doc.as_mut().unwrap().sheets[0].data.as_mut().unwrap()).unwrap().layout;
        let mut other = l.objects[0].clone();
        l.objects[0].frac = [0.0, 0.0, 0.5, 1.0];
        other.frac = [0.5, 0.0, 0.5, 1.0];
        l.objects.push(other);
        pull_to(&mut a, (280.0, 100.0), (40.0, 0.0), NONE);
        assert_eq!(a.object, Some(1));
        assert_eq!(
            (rect_of(&a, 0), rect_of(&a, 1)),
            ([140.0, 60.0, 200.0, 120.0], [140.0, 60.0, 200.0, 120.0])
        );
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
