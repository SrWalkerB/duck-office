//! Valores das séries dos gráficos lidos das células (F5): o gráfico mostra
//! o que está na planilha agora, sem as linhas e colunas escondidas (como o
//! `plotVisOnly` do Excel).

use duck_core::Value;
use duck_core::drawing::Series;

use crate::eval::Book;
use crate::parse::{self, Node, SheetSel};

/// Células de um intervalo absoluto de uma aba do arquivo.
const MAX_CELLS: u64 = 1_000_000;
/// Mais pontos que pixels: fica o que veio no arquivo.
const MAX_POINTS: usize = 32_000;

/// Células do intervalo `f` (`'Aba'!$B$2:$B$13`), em ordem de linha; `all` =
/// também as escondidas. `None` se não for um intervalo fixo do arquivo.
pub fn range_values<'a>(book: &Book<'a>, f: &str, host: u32, all: bool) -> Option<Vec<Value<'a>>> {
    let fm = parse::compile(f, host, 0, 0, book);
    let Node::Area(a) = fm.node(fm.root) else { return None };
    let s = match a.sheet {
        SheetSel::Local(s) if s != u32::MAX => s,
        SheetSel::This => host,
        _ => return None,
    };
    if !(a.r0.abs && a.r1.abs && a.c0.abs && a.c1.abs) {
        return None;
    }
    let sh = *book.sheets.get(s as usize)?;
    let (r0, r1, c0, c1) = (a.r0.v as u32, a.r1.v as u32, a.c0.v as u32, a.c1.v as u32);
    if (r1 - r0 + 1) as u64 * (c1 - c0 + 1) as u64 > MAX_CELLS {
        return None;
    }
    let l = &sh.layout;
    let cols: Vec<u32> = (c0..=c1).filter(|&c| all || !l.col_hidden(c)).collect();
    let mut out = Vec::new();
    let mut row = |r: u32| out.extend(cols.iter().map(|&c| sh.cell(r, c, book.shared).value));
    if all {
        (r0..=r1).for_each(&mut row);
    } else {
        l.visible_rows(r0, r1).for_each(&mut row);
    }
    (out.len() <= MAX_POINTS).then_some(out)
}

/// O que mudou numa série em relação ao modelo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Update {
    pub cats: Option<Vec<String>>,
    pub vals: Option<Vec<Option<f64>>>,
    pub name: Option<String>,
    pub hidden: bool,
}

fn text(v: Value) -> String {
    let mut t = String::new();
    match v {
        Value::Text(s) => t.push_str(s),
        Value::Number(n) => duck_core::numfmt::format_general(n, 11, &mut t),
        Value::Bool(b) => t.push_str(if b { "VERDADEIRO" } else { "FALSO" }),
        _ => {}
    }
    t
}

/// Relê uma série; `None` = nada mudou.
pub fn update(book: &Book, s: &Series, host: u32, plot_hidden: bool) -> Option<Update> {
    let vals = s.val_ref.as_deref().and_then(|f| range_values(book, f, host, plot_hidden)).map(|v| {
        v.into_iter()
            .map(|x| match x {
                Value::Number(n) => Some(n),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    let cats = s
        .cat_ref
        .as_deref()
        .and_then(|f| range_values(book, f, host, plot_hidden))
        .map(|v| v.into_iter().map(text).collect::<Vec<_>>());
    let name = s
        .name_ref
        .as_deref()
        .and_then(|f| range_values(book, f, host, true))
        .and_then(|v| v.into_iter().next())
        .map(text);
    let hidden = vals.as_ref().is_some_and(|v| v.is_empty());
    let up = Update {
        vals: vals.filter(|v| *v != s.vals),
        cats: cats.filter(|v| *v != s.cats),
        name: name.filter(|n| *n != s.name),
        hidden,
    };
    (up.vals.is_some() || up.cats.is_some() || up.name.is_some() || hidden != s.hidden).then_some(up)
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{BookExtras, CellValue, Layout, RowSpec, Sheet, StringTable};

    fn sheet() -> Sheet {
        let mut sh = Sheet::new("A");
        for r in 0..5u32 {
            sh.set_cell(r, 0, CellValue::Text(format!("c{r}")), None, None);
            sh.set_cell(r, 1, CellValue::Number(r as f64), None, None);
        }
        sh
    }

    #[test]
    fn hidden_rows_stay_out() {
        let mut sh = sheet();
        sh.layout = Layout {
            rows: vec![
                RowSpec { row: 1, height: None, hidden: true, style: None },
                RowSpec { row: 2, height: Some(30.0), hidden: false, style: None },
                RowSpec { row: 3, height: None, hidden: true, style: None },
            ],
            ..Default::default()
        };
        let strings = StringTable::new();
        let extras = BookExtras::default();
        let book = Book::new(vec![&sh], &strings, &extras, false);
        let s = Series {
            cat_ref: Some("A!$A$1:$A$5".into()),
            val_ref: Some("A!$B$1:$B$5".into()),
            ..Default::default()
        };
        let u = update(&book, &s, 0, false).unwrap();
        assert_eq!(u.vals.unwrap(), [Some(0.0), Some(2.0), Some(4.0)]);
        assert_eq!(u.cats.unwrap(), ["c0", "c2", "c4"]);
        assert!(!u.hidden);
        assert_eq!(update(&book, &s, 0, true).unwrap().vals.unwrap().len(), 5);
        let gone = Series { val_ref: Some("A!$B$2".into()), ..Default::default() };
        assert!(update(&book, &gone, 0, false).unwrap().hidden);
    }
}
