//! Validação de dados: os itens da lista suspensa e se um valor é aceito.
//!
//! As fórmulas da regra são compiladas para a base dela e avaliadas na
//! célula, então as referências relativas andam como no Excel. Na regra
//! personalizada, a célula já aparece com o valor novo.

use std::sync::RwLock;

use duck_core::{CellValue, DataValidation, DvKind, DvOp};

use crate::eval::{Book, Engine, Eval, Overlay, to_text};
use crate::parse::{Formula, Node, compile, flag};
use crate::value::{Sid, V, compare, text_eq};

/// Um item da lista suspensa.
#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub value: CellValue,
    /// Estilo da célula de origem (o item aparece formatado como ela).
    /// `None`: item escrito na própria regra, que entra como texto digitado.
    pub style: Option<u16>,
}

/// Mais que isto ninguém rola (uma coluna inteira preenchida).
const MAX_ITEMS: usize = 100_000;

/// Itens da lista da regra na célula. `None`: não é lista, ou a origem não
/// resolve para itens (erro, ou algo que o motor não calcula).
pub fn list_items(book: &Book, sheet: u32, dv: &DataValidation, row: u32, col: u32) -> Option<Vec<ListItem>> {
    if dv.kind != DvKind::List {
        return None;
    }
    if let Some(items) = dv.literal_items() {
        let item = |s: String| ListItem { value: CellValue::Text(s), style: None };
        return Some(items.into_iter().filter(|s| !s.is_empty()).map(item).collect());
    }
    let eng = Engine::names_only(book);
    let f = rule_formula(book, sheet, dv, &dv.f1);
    let mut e = Eval::new(book, &eng, &f, sheet, row, col);
    let v = e.eval(f.root);
    if e.opaque || f.flags & flag::OPAQUE != 0 {
        return None;
    }
    let mut out = Vec::new();
    match v {
        V::Ref(r) => {
            let r1 = r.r1.min(book.used_rows(r.sheet).saturating_sub(1));
            let c1 = r.c1.min(book.used_cols(r.sheet).saturating_sub(1));
            let src = match r.sheet {
                Sid::Local(i) => book.sheets.get(i as usize),
                Sid::Ext(..) => None,
            };
            'cells: for y in r.r0..=r1 {
                for x in r.c0..=c1 {
                    if out.len() >= MAX_ITEMS {
                        break 'cells;
                    }
                    let value = own(&book.get(r.sheet, y, x));
                    out.push(ListItem { value, style: Some(src.map_or(0, |s| s.raw(y, x).style)) });
                }
            }
            // Células vazias no fim (o intervalo com folga) não viram itens.
            while out.last().is_some_and(|i: &ListItem| i.value == CellValue::Empty) {
                out.pop();
            }
        }
        V::Arr(a) => {
            out.extend(a.v.iter().take(MAX_ITEMS).map(|x| ListItem { value: own(x), style: Some(0) }));
        }
        V::Err(_) | V::Empty => return None,
        v => out.push(ListItem { value: v.to_cell(), style: Some(0) }),
    }
    Some(out)
}

/// O valor é aceito na célula? `None` = não dá para saber (a regra usa algo
/// que o motor não calcula): quem chama aceita.
pub fn check(
    book: &Book,
    sheet: u32,
    dv: &DataValidation,
    row: u32,
    col: u32,
    value: &CellValue,
) -> Option<bool> {
    if dv.kind == DvKind::Any {
        return Some(true);
    }
    if *value == CellValue::Empty {
        return Some(dv.allow_blank);
    }
    match dv.kind {
        DvKind::List => return check_list(book, sheet, dv, row, col, value),
        DvKind::Custom => return check_custom(book, sheet, dv, row, col, value),
        _ => {}
    }
    let x = match (dv.kind, value) {
        (DvKind::TextLength, CellValue::Error(_)) => return Some(false),
        (DvKind::TextLength, v) => to_text(cell_v(v)).map_or(0.0, |t| t.encode_utf16().count() as f64),
        (DvKind::Whole, CellValue::Number(x)) if x.fract() != 0.0 => return Some(false),
        (_, CellValue::Number(x)) => *x,
        _ => return Some(false),
    };
    let eng = Engine::names_only(book);
    // `Err(None)` = a regra aceita qualquer valor (limite em branco com
    // "ignorar em branco"); `Err(Some(r))` = o resultado já é esse.
    let limit = |text: &str| -> Result<f64, Option<bool>> {
        let f = rule_formula(book, sheet, dv, text);
        let mut e = Eval::new(book, &eng, &f, sheet, row, col);
        let v = e.eval(f.root);
        let v = e.to_scalar(v);
        if e.opaque || f.flags & flag::OPAQUE != 0 {
            return Err(None);
        }
        if matches!(v, V::Empty) && dv.allow_blank {
            return Err(Some(true));
        }
        e.to_num(&v).map_err(|_| Some(false))
    };
    let a = match limit(&dv.f1) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let b = if dv.op.two() {
        match limit(&dv.f2) {
            Ok(b) => b,
            Err(r) => return r,
        }
    } else {
        0.0
    };
    Some(match dv.op {
        DvOp::Between => a <= x && x <= b,
        DvOp::NotBetween => x < a || x > b,
        DvOp::Equal => x == a,
        DvOp::NotEqual => x != a,
        DvOp::Greater => x > a,
        DvOp::Less => x < a,
        DvOp::GreaterEqual => x >= a,
        DvOp::LessEqual => x <= a,
    })
}

/// Resultado de uma fórmula digitada na célula, antes de ela entrar.
/// `None` = o motor não calcula.
pub fn typed_formula(book: &Book, sheet: u32, row: u32, col: u32, text: &str) -> Option<CellValue> {
    let f = compile(text, sheet, row, col, book);
    let eng = Engine::names_only(book);
    let mut e = Eval::new(book, &eng, &f, sheet, row, col);
    let v = e.eval(f.root);
    let v = e.final_value(v);
    (!e.opaque && f.flags & flag::OPAQUE == 0).then(|| v.to_cell())
}

/// O valor (uma célula vazia continua vazia, sem virar 0).
fn own(v: &V) -> CellValue {
    if matches!(v, V::Empty) { CellValue::Empty } else { v.to_cell() }
}

fn rule_formula(book: &Book, sheet: u32, dv: &DataValidation, text: &str) -> Formula {
    let (r, c) = dv.base();
    compile(text.trim_start_matches('='), sheet, r, c, book)
}

fn check_list(
    book: &Book,
    sheet: u32,
    dv: &DataValidation,
    row: u32,
    col: u32,
    value: &CellValue,
) -> Option<bool> {
    let items = list_items(book, sheet, dv, row, col)?;
    let literal = dv.literal_items().is_some();
    let v = cell_v(value);
    let hit = items.iter().any(|i| match (&i.value, value) {
        // Escrita na regra, a lista diferencia maiúsculas; vinda de células, não.
        (CellValue::Text(a), CellValue::Text(b)) if literal => a == b,
        (CellValue::Text(a), CellValue::Text(b)) => text_eq(a, b),
        (CellValue::Text(a), _) if literal => {
            let typed = duck_core::input::parse_input(a, book.date1904, crate::value::today());
            matches!(typed, duck_core::input::Input::Value(x, _) if x == *value)
        }
        (CellValue::Empty, _) => false,
        (x, _) => compare(&cell_v(x), &v).is_eq(),
    });
    if hit {
        return Some(true);
    }
    // Como o Excel: com "ignorar em branco", uma origem dada por um nome e
    // com células vazias aceita qualquer valor.
    let named = || {
        let f = rule_formula(book, sheet, dv, &dv.f1);
        matches!(f.node(f.root), Node::Name(_))
    };
    Some(dv.allow_blank && items.iter().any(|i| i.value == CellValue::Empty) && named())
}

fn check_custom(
    book: &Book,
    sheet: u32,
    dv: &DataValidation,
    row: u32,
    col: u32,
    value: &CellValue,
) -> Option<bool> {
    let mut o = Overlay::default();
    o.set(sheet, row, col, value.clone());
    let local = RwLock::new(o);
    let mut b = book.clone();
    b.local = Some(&local);
    b.forced = Some((sheet, row, col));
    b.max_rows = b.max_rows.max(row + 1);
    let eng = Engine::names_only(&b);
    let f = rule_formula(&b, sheet, dv, &dv.f1);
    let mut e = Eval::new(&b, &eng, &f, sheet, row, col);
    let v = e.eval(f.root);
    let v = e.to_scalar(v);
    if e.opaque || f.flags & flag::OPAQUE != 0 {
        return None;
    }
    Some(match v {
        V::Bool(x) => x,
        V::Num(x) => x != 0.0,
        V::Empty => dv.allow_blank,
        _ => false,
    })
}

fn cell_v(v: &CellValue) -> V<'_> {
    match v {
        CellValue::Empty => V::Empty,
        CellValue::Number(x) => V::Num(*x),
        CellValue::Text(s) => V::text(s.as_str()),
        CellValue::Bool(b) => V::Bool(*b),
        CellValue::Error(e) => V::Err(*e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{BookExtras, CellRange, DefinedName, Sheet, StringTable};

    fn r(a: u32, b: u32, c: u32, d: u32) -> CellRange {
        CellRange { first_row: a, first_col: b, last_row: c, last_col: d }
    }

    /// Plan1: A1:A3 = SP, RJ, (vazia); B1:B4 = números; D1 = "Frutas".
    /// 'Outra aba': A1:A3 = Banana, Maçã, Uva.
    fn data() -> (Vec<Sheet>, BookExtras) {
        let mut p = Sheet::new("Plan1");
        let t = |s: &str| CellValue::Text(s.into());
        p.set_cell(0, 0, t("SP"), None, None);
        p.set_cell(1, 0, t("RJ"), None, None);
        for i in 0..4 {
            p.set_cell(i, 1, CellValue::Number(10.0 * (i + 1) as f64), None, Some(i as u16));
        }
        p.set_cell(0, 3, t("Frutas"), None, None);
        let mut o = Sheet::new("Outra aba");
        for (i, s) in ["Banana", "Maçã", "Uva"].iter().enumerate() {
            o.set_cell(i as u32, 0, t(s), None, None);
        }
        let name =
            |n: &str, f: &str| DefinedName { name: n.into(), sheet: None, formula: f.into(), hidden: false };
        let extras = BookExtras {
            names: vec![name("Frutas", "'Outra aba'!$A$1:$A$3"), name("UFs", "Plan1!$A$1:$A$4")],
            ..Default::default()
        };
        (vec![p, o], extras)
    }

    fn texts(items: Option<Vec<ListItem>>) -> Vec<String> {
        items
            .unwrap_or_default()
            .into_iter()
            .map(|i| match i.value {
                CellValue::Text(s) => s,
                CellValue::Number(x) => x.to_string(),
                v => format!("{v:?}"),
            })
            .collect()
    }

    fn list(f: &str) -> DataValidation {
        DataValidation { ranges: vec![r(0, 5, 9, 5)], kind: DvKind::List, f1: f.into(), ..Default::default() }
    }

    #[test]
    fn list_sources() {
        let (sheets, extras) = data();
        let shared = StringTable::new();
        let book = Book::new(sheets.iter().collect(), &shared, &extras, false);
        let items = |f: &str, row: u32| texts(list_items(&book, 0, &list(f), row, 5));
        assert_eq!(items("\"Sim, Não\"", 0), ["Sim", "Não"]);
        assert_eq!(items("$A$1:$A$9", 0), ["SP", "RJ"], "vazias do fim saem");
        assert_eq!(items("'Outra aba'!$A$1:$A$3", 0), ["Banana", "Maçã", "Uva"]);
        assert_eq!(items("Frutas", 0), ["Banana", "Maçã", "Uva"]);
        assert_eq!(items("INDIRECT($D$1)", 0), ["Banana", "Maçã", "Uva"]);
        assert_eq!(items("OFFSET($B$1,0,0,2,1)", 0), ["10", "20"]);
        assert_eq!(items("IF($A$1=\"SP\",$B$1:$B$2,$A$1:$A$2)", 0), ["10", "20"]);
        // Relativa: a regra começa em F1; na linha 3, B1:B2 vira B3:B4.
        assert_eq!(items("B1:B2", 2), ["30", "40"]);
        assert!(list_items(&book, 0, &list("NaoExiste"), 0, 5).is_none());
        let styled = list_items(&book, 0, &list("$B$1:$B$3"), 0, 5).unwrap();
        assert_eq!(styled[2].style, Some(2), "o item leva o estilo da célula");
    }

    #[test]
    fn list_values() {
        let (sheets, extras) = data();
        let shared = StringTable::new();
        let book = Book::new(sheets.iter().collect(), &shared, &extras, false);
        let ok = |f: &str, v: CellValue| check(&book, 0, &list(f), 0, 5, &v);
        let t = |s: &str| CellValue::Text(s.into());
        assert_eq!(ok("\"Sim,Não\"", t("Sim")), Some(true));
        assert_eq!(ok("\"Sim,Não\"", t("sim")), Some(false), "escrita na regra: diferencia");
        assert_eq!(ok("\"1,2\"", CellValue::Number(2.0)), Some(true));
        assert_eq!(ok("\"1,2\"", CellValue::Number(3.0)), Some(false));
        assert_eq!(ok("Frutas", t("maçã")), Some(true), "de células: não diferencia");
        assert_eq!(ok("Frutas", t("Pera")), Some(false));
        assert_eq!(ok("$B$1:$B$4", CellValue::Number(30.0)), Some(true));
        assert_eq!(ok("$B$1:$B$4", t("30")), Some(false));
        // Origem com erro ou desconhecida: não dá para conferir.
        assert_eq!(ok("NaoExiste", t("x")), None);
        // Vazio: depende de "ignorar em branco".
        assert_eq!(ok("Frutas", CellValue::Empty), Some(true));
        let strict = DataValidation { allow_blank: false, ..list("Frutas") };
        assert_eq!(check(&book, 0, &strict, 0, 5, &CellValue::Empty), Some(false));
    }

    #[test]
    fn numbers_dates_and_length() {
        let (sheets, extras) = data();
        let shared = StringTable::new();
        let book = Book::new(sheets.iter().collect(), &shared, &extras, false);
        let rule = |kind, op, f1: &str, f2: &str| DataValidation {
            ranges: vec![r(0, 5, 9, 5)],
            kind,
            op,
            f1: f1.into(),
            f2: f2.into(),
            ..Default::default()
        };
        let n = CellValue::Number;
        let ok = |d: &DataValidation, row: u32, v: CellValue| check(&book, 0, d, row, 5, &v);
        let d = rule(DvKind::Whole, DvOp::Between, "1", "10");
        assert_eq!(ok(&d, 0, n(10.0)), Some(true));
        assert_eq!(ok(&d, 0, n(10.5)), Some(false));
        assert_eq!(ok(&d, 0, n(11.0)), Some(false));
        assert_eq!(ok(&d, 0, CellValue::Text("5".into())), Some(false));
        assert_eq!(ok(&d, 0, CellValue::Bool(true)), Some(false));
        let d = rule(DvKind::Decimal, DvOp::NotBetween, "1", "2");
        assert_eq!((ok(&d, 0, n(1.5)), ok(&d, 0, n(2.5))), (Some(false), Some(true)));
        // Limite numa célula, relativo: F1 usa B1, F3 usa B3.
        let d = rule(DvKind::Decimal, DvOp::LessEqual, "B1", "");
        assert_eq!((ok(&d, 0, n(30.0)), ok(&d, 2, n(30.0))), (Some(false), Some(true)));
        assert_eq!(ok(&d, 8, n(99.0)), Some(true), "limite em branco com \"ignorar em branco\"");
        let d = DataValidation { allow_blank: false, ..d };
        assert_eq!(ok(&d, 8, n(99.0)), Some(false));
        let d = rule(DvKind::Date, DvOp::GreaterEqual, "DATE(2026,1,1)", "");
        assert_eq!((ok(&d, 0, n(46_022.0)), ok(&d, 0, n(46_023.0))), (Some(false), Some(true)));
        let d = rule(DvKind::TextLength, DvOp::Equal, "4", "");
        assert_eq!(ok(&d, 0, CellValue::Text("maçã".into())), Some(true));
        assert_eq!((ok(&d, 0, n(1234.0)), ok(&d, 0, n(12.0))), (Some(true), Some(false)));
        let d = rule(DvKind::Whole, DvOp::Equal, "1/0", "");
        assert_eq!(ok(&d, 0, n(1.0)), Some(false));
        assert_eq!(ok(&rule(DvKind::Any, DvOp::Equal, "", ""), 0, n(1.0)), Some(true));
    }

    #[test]
    fn custom_sees_the_new_value() {
        let (sheets, extras) = data();
        let shared = StringTable::new();
        let book = Book::new(sheets.iter().collect(), &shared, &extras, false);
        // Sem repetidos na coluna B (a regra vale em B1:B100).
        let d = DataValidation {
            ranges: vec![r(0, 1, 99, 1)],
            kind: DvKind::Custom,
            f1: "COUNTIF($B:$B,B1)=1".into(),
            ..Default::default()
        };
        let ok = |row, v| check(&book, 0, &d, row, 1, &CellValue::Number(v));
        assert_eq!(ok(0, 20.0), Some(false), "já existe em B2");
        assert_eq!(ok(0, 25.0), Some(true));
        assert_eq!(ok(1, 20.0), Some(true), "a própria célula não conta duas vezes");
        assert_eq!(ok(50, 77.0), Some(true), "depois da última linha usada");
        assert_eq!(ok(50, 40.0), Some(false));
        let d = DataValidation { f1: "ISTEXT(B1)".into(), ..d };
        assert_eq!(check(&book, 0, &d, 0, 1, &CellValue::Text("x".into())), Some(true));
        assert_eq!(check(&book, 0, &d, 0, 1, &CellValue::Number(1.0)), Some(false));
        assert_eq!(typed_formula(&book, 0, 0, 1, "B2*2"), Some(CellValue::Number(40.0)));
    }

    /// Inserir e apagar linhas: intervalos e fórmulas andam.
    #[test]
    fn rules_follow_inserted_and_deleted_rows() {
        use duck_core::{Axis, Command, OpsHandle, Shift};
        let mut s = Sheet::new("Plan1");
        s.set_cell(30, 0, CellValue::Number(1.0), None, None);
        s.layout.validations = vec![
            DataValidation {
                ranges: vec![r(4, 0, 9, 0)],
                kind: DvKind::Custom,
                f1: "B5>$D$20".into(),
                ..Default::default()
            },
            DataValidation {
                ranges: vec![r(6, 2, 6, 2)],
                kind: DvKind::List,
                f1: "\"A5,B6\"".into(),
                ..Default::default()
            },
        ];
        let before = s.layout.validations.clone();
        let run = |s: &mut Sheet, shift: Shift| {
            let ops =
                OpsHandle(std::sync::Arc::new(crate::ShiftOps { shift, sheet: "Plan1".into(), own: true }));
            Command::Shift(shift, ops).apply(s)
        };
        let undo = run(&mut s, Shift { axis: Axis::Rows, at: 2, n: 3, insert: true });
        assert_eq!(s.layout.validations[0].ranges, [r(7, 0, 12, 0)]);
        assert_eq!(s.layout.validations[0].f1, "B8>$D$23");
        assert_eq!(s.layout.validations[1].f1, "\"A5,B6\"", "texto não é referência");
        assert!(s.edits().validations);
        undo.apply(&mut s);
        assert_eq!(s.layout.validations, before);
        // Apagar a primeira linha do intervalo: a regra passa a valer da
        // seguinte, com a fórmula dela.
        let undo = run(&mut s, Shift { axis: Axis::Rows, at: 4, n: 1, insert: false });
        assert_eq!(s.layout.validations[0].ranges, [r(4, 0, 8, 0)]);
        assert_eq!(s.layout.validations[0].f1, "B5>$D$19");
        undo.apply(&mut s);
        let undo = run(&mut s, Shift { axis: Axis::Rows, at: 3, n: 8, insert: false });
        assert!(s.layout.validations.is_empty());
        undo.apply(&mut s);
        assert_eq!(s.layout.validations, before);
        // Colunas.
        run(&mut s, Shift { axis: Axis::Cols, at: 1, n: 1, insert: true });
        assert_eq!(s.layout.validations[0].f1, "C5>$E$20");
        assert_eq!(s.layout.validations[1].ranges, [r(6, 3, 6, 3)]);
    }
}
