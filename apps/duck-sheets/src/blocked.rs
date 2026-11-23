//! Testes, sem janela, do que a proteção e a validação não deixam fazer por
//! cada caminho da tela.

use duck_core::{BookProtection, Command, DataValidation, DvAlert, DvKind, DvOp, SheetProtection};

use crate::app::SheetsApp;
use crate::dialog::Kind;
use crate::format::Fmt;
use crate::paste::{Op, PasteOpts, What};
use crate::replace::Query;
use crate::ribbon::{Clear, Merge};
use crate::testing::{app, get, mouse, names, select, style, type_in};
use crate::valid::with_rule;

fn sample() -> SheetsApp {
    app(&[
        ("P", &[("A1", "1"), ("A2", "2"), ("B1", "texto bem mais largo que a coluna"), ("C1", "=A1*2")]),
        ("Q", &[("A1", "=P!A1")]),
    ])
}

/// O que um comando recusado não pode ter mudado.
fn state(a: &SheetsApp) -> String {
    let doc = a.doc.as_ref().unwrap();
    let mut out = format!("{:?} {}", names(a), doc.history.undo.len());
    for (i, slot) in doc.sheets.iter().enumerate() {
        let s = slot.data.as_ref().unwrap();
        let v = slot.view.as_ref().unwrap();
        let (l, color) = (&s.layout, s.layout.view.map(|w| w.tab_color));
        out += &format!(" | {} {:?} {:?} {:?}", slot.info.hidden, l.merges, l.validations, color);
        for r in 0..8u32 {
            out += &format!(" {}", v.rows.size(r));
            for (c, col) in "ABCDEFGH".chars().enumerate() {
                let cell = format!("{col}{}", r + 1);
                out += &format!(" {};{:?};{}", get(a, i, &cell), style(a, i, &cell), v.cols.size(c as u32));
            }
        }
    }
    out
}

fn protect(a: &mut SheetsApp, on: bool) {
    assert!(a.run_command(Command::Protect(on.then(|| Box::new(SheetProtection::default())))));
}

/// O aviso está aberto? Fecha.
fn refused(a: &mut SheetsApp) -> bool {
    a.dialog.take().is_some_and(|d| matches!(d.kind, Kind::Message))
}

/// Copia `from` e cola em `to`.
fn copy_paste(a: &mut SheetsApp, from: &str, to: &str, cut: bool, opts: PasteOpts) {
    select(a, from);
    let (r0, c0, r1, c1) = a.doc.as_ref().unwrap().current().unwrap().1.sel();
    a.clip = a.capture(&[crate::fill::range_of((r0, c0, r1, c1))], false, "t".into(), cut);
    select(a, to);
    a.paste_text("t".into(), opts);
}

type Act = fn(&mut SheetsApp);

/// Cada ação é recusada com a planilha protegida (aviso, nada muda, nada a
/// desfazer) e vale sem a proteção.
#[test]
fn every_command_on_a_protected_sheet() {
    let acts: [(&str, Act); 40] = [
        ("digitar", |a| type_in(a, "E5", "9")),
        ("apagar", |a| {
            select(a, "A1:A2");
            a.clear_selection();
        }),
        ("limpar tudo", |a| {
            select(a, "A1");
            a.clear(Clear::All);
        }),
        ("Ctrl+Enter", |a| {
            select(a, "E1:F2");
            a.start_edit(Some("7"), false);
            a.commit_edit_all();
        }),
        ("alça", |a| {
            select(a, "A1:A2");
            let h = mouse::handle(a);
            mouse::drag(a, h, "A6", mouse::NONE);
        }),
        ("alça com Ctrl", |a| {
            select(a, "A1:A2");
            let h = mouse::handle(a);
            mouse::drag(a, h, "A6", mouse::CTRL);
        }),
        ("duplo clique na alça", |a| {
            select(a, "B1");
            let h = mouse::handle(a);
            mouse::press(a, h, mouse::NONE, 2);
            mouse::mouse_up(a);
        }),
        ("Ctrl+D", |a| {
            select(a, "A1:A4");
            a.fill_copy(true);
        }),
        ("Ctrl+R", |a| {
            select(a, "A1:B1");
            a.fill_copy(false);
        }),
        ("AutoSoma", |a| {
            select(a, "A1:A3");
            a.autosum();
        }),
        ("substituir tudo", |a| {
            a.replace_all(&Query {
                find: "texto".into(),
                with: "x".into(),
                formulas: true,
                ..Default::default()
            });
        }),
        ("colar", |a| copy_paste(a, "A1:A2", "E1", false, PasteOpts::default())),
        ("colar recortado", |a| copy_paste(a, "A1:A2", "E1", true, PasteOpts::default())),
        ("colar de fora", |a| {
            select(a, "E1");
            a.paste_text("1\t2\n3\t4".into(), PasteOpts::default());
        }),
        ("colar valores", |a| {
            copy_paste(a, "C1", "E1", false, PasteOpts { what: What::Values, ..Default::default() })
        }),
        ("colar formatos", |a| {
            select(a, "A1");
            a.unchecked = true;
            a.apply_fmt(Fmt::Bold);
            a.unchecked = false;
            copy_paste(a, "A1", "E1", false, PasteOpts { what: What::Formats, ..Default::default() })
        }),
        ("colar larguras", |a| {
            copy_paste(a, "A1", "E1", false, PasteOpts { what: What::Widths, ..Default::default() })
        }),
        ("colar somando", |a| {
            copy_paste(a, "A1", "A2", false, PasteOpts { op: Op::Add, ..Default::default() })
        }),
        ("pincel", |a| {
            select(a, "A1");
            a.unchecked = true;
            a.apply_fmt(Fmt::Bold);
            a.unchecked = false;
            a.toggle_painter(false);
            select(a, "E1");
            a.apply_painter();
        }),
        ("mover pela borda", |a| {
            let p = mouse::border(a, "A1");
            mouse::drag(a, p, "E4", mouse::NONE);
        }),
        ("copiar pela borda", |a| {
            let p = mouse::border(a, "A1");
            mouse::drag(a, p, "E4", mouse::CTRL);
        }),
        ("inserir células abaixo", |a| {
            select(a, "A1");
            a.shift_cells(true, 1);
        }),
        ("inserir células à direita", |a| {
            select(a, "A1");
            a.shift_cells(true, 0);
        }),
        ("excluir células acima", |a| {
            select(a, "A1");
            a.shift_cells(false, 1);
        }),
        ("excluir células à esquerda", |a| {
            select(a, "A1");
            a.shift_cells(false, 0);
        }),
        ("inserir linha", |a| {
            select(a, "A1");
            a.shift_cells(true, 2);
        }),
        ("inserir coluna", |a| {
            select(a, "A1");
            a.shift_cells(true, 3);
        }),
        ("excluir linha", |a| {
            select(a, "A1");
            a.shift_cells(false, 2);
        }),
        ("excluir coluna", |a| {
            select(a, "A1");
            a.shift_cells(false, 3);
        }),
        ("mesclar", |a| {
            select(a, "E1:F2");
            a.merge(Merge::Center);
        }),
        ("mesclar através", |a| {
            select(a, "E1:F2");
            a.merge(Merge::Across);
        }),
        ("negrito", |a| {
            select(a, "A1");
            a.apply_fmt(Fmt::Bold);
        }),
        ("desbloquear", |a| {
            select(a, "A1");
            a.apply_fmt(Fmt::Lock);
        }),
        ("Formatar células", |a| {
            select(a, "A1");
            a.ask_format(0);
            let mut f = a.form.take().unwrap();
            f.set_sel("cat", 10);
            f.set_text("code", "0,0");
            a.format_form_changed(&mut f, 0);
            assert_eq!(a.format_form_apply(&f).is_err(), a.dialog.is_some());
        }),
        ("largura pelo cabeçalho", |a| {
            let p = mouse::col_edge(a, 3, 0.0);
            mouse::pull(a, p, 40.0, 0.0);
        }),
        ("altura pelo cabeçalho", |a| {
            let p = mouse::row_edge(a, 3, 0.0);
            mouse::pull(a, p, 0.0, 25.0);
        }),
        ("ajustar a largura", |a| {
            let p = mouse::col_edge(a, 1, 0.0);
            mouse::press(a, p, mouse::NONE, 2);
            mouse::mouse_up(a);
        }),
        ("classificar", |a| {
            select(a, "A1");
            a.sort_here(true);
        }),
        ("validação", |a| {
            put_rule(a, "E1:E4", whole(DvAlert::Stop));
        }),
        ("cor da guia", |a| a.tab_color(0, Some(0xFF0000))),
    ];
    for (what, act) in acts {
        let mut a = sample();
        // O que a própria ação prepara (um formato a copiar) fica de fora.
        let prepared = {
            let mut b = sample();
            act(&mut b);
            assert!(b.dialog.is_none(), "{what}: sem proteção, vale");
            assert_ne!(state(&b), state(&a), "{what}: a ação muda algo");
            matches!(what, "colar formatos" | "pincel")
        };
        protect(&mut a, true);
        if prepared {
            select(&mut a, "A1");
            a.unchecked = true;
            a.apply_fmt(Fmt::Bold);
            a.apply_fmt(Fmt::Bold);
            a.unchecked = false;
        }
        let before = state(&a);
        act(&mut a);
        if prepared {
            a.undo();
        }
        assert!(refused(&mut a), "{what}: avisa");
        assert_eq!(state(&a), before, "{what}: nada muda");
        assert!(a.editor.is_none() && a.form.is_none(), "{what}: nada aberto");
    }
}

/// As células desbloqueadas continuam editáveis, e as permissões dadas valem.
#[test]
fn what_stays_allowed() {
    use duck_core::protect::deny;
    let mut a = sample();
    select(&mut a, "E1:F9");
    a.apply_fmt(Fmt::Lock);
    protect(&mut a, true);
    type_in(&mut a, "E1", "5");
    select(&mut a, "E1:F3");
    a.start_edit(Some("7"), false);
    a.commit_edit_all();
    select(&mut a, "F1:F4");
    a.fill_copy(true);
    copy_paste(&mut a, "E1", "E5", false, PasteOpts { what: What::Values, ..Default::default() });
    assert!(a.dialog.is_none());
    assert_eq!(["E1", "F3", "F4", "E5"].map(|c| get(&a, 0, c)), ["7", "7", "7", "7"]);
    // Uma bloqueada no meio segura tudo.
    select(&mut a, "E1:G1");
    a.fill_copy(false);
    assert!(refused(&mut a));
    assert_eq!((get(&a, 0, "F1"), get(&a, 0, "G1")), ("7".into(), "".into()));
    // Mover uma desbloqueada para uma bloqueada também não.
    let p = mouse::border(&mut a, "E1");
    mouse::drag(&mut a, p, "G4", mouse::NONE);
    assert!(refused(&mut a));
    // A fórmula de outra aba segue o que a aba protegida deixa mover.
    let p = mouse::border(&mut a, "E1");
    mouse::drag(&mut a, p, "F6", mouse::NONE);
    assert!(a.dialog.is_none());
    assert_eq!((get(&a, 0, "E1"), get(&a, 0, "F6")), ("".into(), "7".into()));
    let deny = deny::EXCEL & !(deny::FORMAT_CELLS | deny::FORMAT_COLUMNS | deny::INSERT_ROWS);
    let p = SheetProtection { deny, ..Default::default() };
    assert!(a.run_command(Command::Protect(Some(Box::new(p)))));
    select(&mut a, "A1");
    a.apply_fmt(Fmt::Bold);
    a.shift_cells(true, 2);
    let p = mouse::col_edge(&mut a, 3, 0.0);
    mouse::pull(&mut a, p, 40.0, 0.0);
    assert!(a.dialog.is_none());
    a.shift_cells(false, 2);
    assert!(refused(&mut a));
}

/// Com a fórmula oculta, F2 e Enter sem mexer não a apagam.
#[test]
fn hidden_formulas_survive_the_editor() {
    let mut a = sample();
    select(&mut a, "C1");
    a.apply_fmt(Fmt::Lock);
    a.apply_fmt(Fmt::HideFormula);
    protect(&mut a, true);
    let before = state(&a);
    a.start_edit(None, false);
    assert_eq!(a.editor.as_ref().unwrap().input.text, "", "não aparece no editor");
    a.commit_edit();
    assert_eq!(state(&a), before);
    type_in(&mut a, "C1", "3");
    assert_eq!(get(&a, 0, "C1"), "3");
}

/// Linhas inseridas numa aba livre ajustam as fórmulas de uma protegida.
#[test]
fn other_sheets_follow_even_protected() {
    let mut a = sample();
    a.switch_sheet(1);
    protect(&mut a, true);
    a.switch_sheet(0);
    select(&mut a, "A1");
    a.shift_cells(true, 2);
    assert!(a.dialog.is_none());
    assert_eq!(get(&a, 1, "A1"), "=P!A2");
    let p = mouse::border(&mut a, "A2");
    mouse::drag(&mut a, p, "E4", mouse::NONE);
    assert_eq!(get(&a, 1, "A1"), "=P!E4");
    a.tab_rename(0, "Nova");
    assert_eq!(get(&a, 1, "A1"), "=Nova!E4");
    // Substituir na pasta toda para na aba protegida (e não muda as outras).
    a.replace_all(&Query {
        find: "texto".into(),
        with: "x".into(),
        scope: crate::replace::Scope::Book,
        formulas: true,
        ..Default::default()
    });
    assert!(a.dialog.is_none() && get(&a, 0, "B2").starts_with("x bem"));
    a.replace_all(&Query {
        find: "E4".into(),
        with: "E5".into(),
        scope: crate::replace::Scope::Book,
        formulas: true,
        ..Default::default()
    });
    assert!(refused(&mut a));
    assert_eq!((get(&a, 0, "C2"), get(&a, 1, "A1")), ("=E4*2".into(), "=Nova!E4".into()));
}

/// Nomes definidos acompanham as linhas inseridas com a janela aberta.
#[test]
fn names_follow_inserted_rows() {
    let mut a = sample();
    let name =
        duck_core::DefinedName { name: "Ponto".into(), sheet: None, formula: "P!$A$2".into(), hidden: false };
    let extras = &mut a.doc.as_mut().unwrap().calc.extras;
    std::sync::Arc::make_mut(extras).names.push(name);
    let formula = |a: &SheetsApp| a.doc.as_ref().unwrap().calc.extras.names[0].formula.clone();
    select(&mut a, "A1");
    a.shift_cells(true, 2);
    assert_eq!(formula(&a), "P!$A$3");
    select(&mut a, "A1");
    a.shift_cells(true, 3);
    assert_eq!(formula(&a), "P!$B$3");
    a.undo();
    a.undo();
    assert_eq!(formula(&a), "P!$A$2");
    // Com a planilha protegida, nem as células nem os nomes andam.
    protect(&mut a, true);
    a.shift_cells(true, 2);
    assert!(refused(&mut a));
    assert_eq!(formula(&a), "P!$A$2");
}

/// Com a estrutura da pasta protegida, as abas ficam como estão.
#[test]
fn tabs_of_a_protected_book() {
    let acts: [(&str, Act); 9] = [
        ("criar", |a| a.tab_new(None, None)),
        ("renomear", |a| a.tab_rename(0, "Outro")),
        ("renomear na guia", |a| {
            a.tab_edit_start(0);
            if let Some(e) = &mut a.tab_edit {
                e.input.set_text("Outro");
            }
            a.tab_edit_commit();
        }),
        ("excluir", |a| a.tab_delete(1)),
        ("duplicar", |a| a.tab_duplicate(0, 1)),
        ("mover", |a| a.tab_move(0, 1)),
        ("colorir", |a| a.tab_color(0, Some(0x00FF00))),
        ("ocultar", |a| a.tab_hide(1, true)),
        ("botão +", |a| {
            let g = a.geom();
            let (_, plus) = crate::tabs::strip(&g, a.tabbar.content_w());
            a.tab_click(&g, (plus.x0 + plus.x1) / 2.0, (plus.y0 + plus.y1) / 2.0, 1);
        }),
    ];
    for (what, act) in acts {
        let mut a = sample();
        let before = state(&a);
        act(&mut a);
        assert!(a.dialog.is_none(), "{what}: sem proteção, vale");
        assert_ne!(state(&a), before, "{what}: a ação muda algo");
        let mut a = sample();
        a.set_book_protection(Some(BookProtection { structure: true, ..Default::default() }));
        let before = state(&a);
        act(&mut a);
        assert!(refused(&mut a), "{what}: avisa");
        assert!(a.tab_edit.is_none(), "{what}: não abre a edição");
        assert_eq!(state(&a), before, "{what}: nada muda");
        // As células não dependem da estrutura.
        type_in(&mut a, "A1", "5");
        assert_eq!(get(&a, 0, "A1"), "5");
    }
    // Reexibir.
    let mut a = sample();
    a.tab_hide(1, true);
    a.set_book_protection(Some(BookProtection { structure: true, ..Default::default() }));
    a.tab_hide(1, false);
    assert!(refused(&mut a) && a.doc.as_ref().unwrap().sheets[1].info.hidden);
    // Só as janelas protegidas: as abas mudam.
    let mut a = sample();
    a.set_book_protection(Some(BookProtection { windows: true, ..Default::default() }));
    a.tab_new(Some("Livre"), None);
    assert_eq!(names(&a).len(), 3);
}

fn whole(alert: DvAlert) -> DataValidation {
    DataValidation {
        kind: DvKind::Whole,
        op: DvOp::Between,
        f1: "1".into(),
        f2: "10".into(),
        alert,
        ..Default::default()
    }
}

fn put_rule(a: &mut SheetsApp, range: &str, rule: DataValidation) -> bool {
    select(a, range);
    let (r0, c0, r1, c1) = a.doc.as_ref().unwrap().current().unwrap().1.sel();
    let sheet = a.doc.as_ref().unwrap().current().unwrap().0;
    let rules = with_rule(sheet, crate::fill::range_of((r0, c0, r1, c1)), Some(rule), (r0, c0));
    a.run_command(Command::Validations(rules))
}

/// Responde ao aviso de valor inválido (`None` = Esc).
fn answer(a: &mut SheetsApp, button: Option<usize>) {
    use crate::dialog::Outcome;
    assert!(a.answering());
    a.dialog_event(button.map_or(Outcome::Cancel, Outcome::Button));
}

fn enter_all(a: &mut SheetsApp, range: &str, text: &str) {
    assert!(a.scripted_select(range));
    a.start_edit(Some(text), false);
    a.commit_edit_all();
}

/// O que se digita passa pela regra, numa célula ou em várias.
#[test]
fn typed_values_are_checked() {
    let mut a = sample();
    assert!(put_rule(&mut a, "E1:E3", whole(DvAlert::Stop)));
    let undo = a.doc.as_ref().unwrap().history.undo.len();
    type_in(&mut a, "E1", "11");
    // Enquanto não responde, nada mais muda.
    a.tab_new(None, None);
    select(&mut a, "A1");
    a.apply_fmt(Fmt::Bold);
    a.shift_selection(true, None);
    assert_eq!(a.doc.as_ref().unwrap().history.undo.len(), undo);
    // "Tentar de novo" volta à célula com o texto.
    answer(&mut a, Some(0));
    assert_eq!(a.editor.as_ref().unwrap().input.text, "11");
    assert_eq!(a.doc.as_ref().unwrap().current().unwrap().1.cur, (0, 4));
    a.cancel_edit();
    assert_eq!(get(&a, 0, "E1"), "");
    type_in(&mut a, "E1", "10");
    assert!(a.dialog.is_none());

    // Ctrl+Enter: G1 não tem regra, mas E2 não aceita: nada é escrito.
    let before = state(&a);
    enter_all(&mut a, "G1:G2;E2:E3", "99");
    answer(&mut a, Some(0));
    assert_eq!(a.editor.as_ref().unwrap().input.text, "99");
    assert!(a.doc.as_ref().unwrap().current().unwrap().1.multi(), "a seleção fica");
    a.cancel_edit();
    assert_eq!(state(&a), before);
    enter_all(&mut a, "G1:G2;E2:E3", "8");
    assert!(a.dialog.is_none());
    assert_eq!(["G1", "G2", "E2", "E3"].map(|c| get(&a, 0, c)), ["8", "8", "8", "8"]);
    // Fórmulas: cada célula com a dela (A1 = 1 vale; A3, vazia, não).
    enter_all(&mut a, "E1:E3", "=A3");
    assert!(a.answering(), "E1 = A1…");
    answer(&mut a, None);
    assert_eq!(get(&a, 0, "E2"), "8");
    enter_all(&mut a, "E1:E3", "=$A$2");
    assert!(a.dialog.is_none());

    // Com aviso: "Sim" escreve em todas, num passo só.
    assert!(put_rule(&mut a, "F1:F3", whole(DvAlert::Warning)));
    enter_all(&mut a, "F1:F3", "50");
    answer(&mut a, Some(1));
    assert_eq!((get(&a, 0, "F1"), get(&a, 0, "F3")), ("50".into(), "50".into()));
    a.undo();
    assert_eq!((get(&a, 0, "F1"), get(&a, 0, "F3")), ("".into(), "".into()));
    // Entre várias regras, a que recusa manda.
    enter_all(&mut a, "F1:F3;E1", "50");
    assert_eq!(a.dialog.as_ref().unwrap().buttons[0], "Tentar de novo");
    answer(&mut a, None);
    assert_eq!(get(&a, 0, "F1"), "");
    // Informação: OK escreve.
    assert!(put_rule(&mut a, "H1", whole(DvAlert::Information)));
    type_in(&mut a, "H1", "50");
    answer(&mut a, Some(0));
    assert_eq!(get(&a, 0, "H1"), "50");
}

/// Uma regra com LET e LAMBDA, criada como se digita.
#[test]
fn rules_with_new_functions() {
    let mut a = sample();
    let rule = |pt: &str| DataValidation {
        kind: DvKind::Custom,
        f1: duck_calc::locale::from_pt(pt),
        ..Default::default()
    };
    assert!(put_rule(&mut a, "E1:E3", rule("LET(x;E1;limite;SOMA($A$1:$A$2);x<=limite*1,5)")));
    put_rule(&mut a, "F1:F3", rule("LAMBDA(n;MOD(n;2)=0)(F1)"));
    type_in(&mut a, "E2", "4,5");
    assert!(a.dialog.is_none());
    type_in(&mut a, "E2", "4,6");
    answer(&mut a, None);
    enter_all(&mut a, "F1:F3", "4");
    assert!(a.dialog.is_none());
    enter_all(&mut a, "F1:F3", "5");
    answer(&mut a, None);
    assert_eq!(["E2", "F3"].map(|c| get(&a, 0, c)), ["4,5", "4"]);
}
