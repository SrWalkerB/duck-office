//! Fórmulas digitadas: conferidas antes de irem para a célula. Uma que o
//! Excel não lê faria ele acusar o arquivo salvo de danificado.

use duck_calc::parse::{Resolve, TableInfo};
use duck_calc::value::text_eq;

use crate::app::{Doc, SheetsApp};

fn info(sheet: u32, t: &duck_core::Table) -> TableInfo<'_> {
    TableInfo {
        name: &t.name,
        sheet,
        range: t.range,
        header_rows: t.header_rows,
        totals_rows: t.totals_rows,
        columns: &t.columns,
    }
}

impl Resolve for Doc {
    fn sheet(&self, name: &str) -> Option<u32> {
        self.sheets.iter().position(|s| text_eq(&s.info.name, name)).map(|i| i as u32)
    }

    fn ext_sheet(&self, book: u16, name: &str) -> Option<u16> {
        let b = self.calc.extras.externals.get(book as usize)?;
        b.sheets.iter().position(|s| text_eq(&s.name, name)).map(|i| i as u16)
    }

    fn name(&self, name: &str, sheet: u32) -> Option<u32> {
        let names = &self.calc.extras.names;
        let find = |s: Option<u32>| names.iter().position(|n| n.sheet == s && text_eq(&n.name, name));
        find(Some(sheet)).or_else(|| find(None)).map(|i| i as u32)
    }

    fn table(&self, name: &str) -> Option<TableInfo<'_>> {
        self.sheets.iter().enumerate().find_map(|(i, s)| {
            let t = s.data.as_ref()?.layout.tables.iter().find(|t| text_eq(&t.name, name))?;
            Some(info(i as u32, t))
        })
    }

    fn table_at(&self, sheet: u32, row: u32, col: u32) -> Option<TableInfo<'_>> {
        let s = self.sheets.get(sheet as usize)?.data.as_ref()?;
        s.layout.tables.iter().find(|t| t.range.contains(row, col)).map(|t| info(sheet, t))
    }
}

impl Doc {
    /// Fórmula em português (sem `=`), digitada na célula (`r`, `c`) da aba
    /// `sheet`, como o arquivo guarda. `Err` = por que ela não serve.
    pub fn file_formula(&self, sheet: usize, pt: &str, r: u32, c: u32) -> Result<String, String> {
        self.check_formula(sheet, &duck_calc::locale::from_pt(pt), r, c)
    }

    /// O mesmo, para uma fórmula já em inglês (macros).
    pub fn check_formula(&self, sheet: usize, f: &str, r: u32, c: u32) -> Result<String, String> {
        let out = duck_calc::check::prepare(f, sheet as u32, r, c, self);
        // Ainda carregando, as tabelas das outras abas não são conhecidas.
        if out.is_err() && self.finished.is_none() {
            return duck_calc::check::syntax(f).map(|()| f.to_string());
        }
        out
    }
}

impl SheetsApp {
    /// [`Doc::file_formula`] na aba ativa.
    pub fn file_formula(&self, pt: &str, r: u32, c: u32) -> Result<String, String> {
        match &self.doc {
            Some(d) => d.file_formula(d.active, pt, r, c),
            None => Err(String::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::dialog::Outcome;
    use crate::testing::{app, get, select, type_in};

    #[test]
    fn broken_formulas_do_not_reach_the_cell() {
        let mut a = app(&[("Plan1", &[("A1", "5")]), ("Outra aba", &[])]);
        for f in ["=SOMA(", "=1+*2", "=SOMA(A1))", "=Nada!A1", "=Nada[x]", "=SE()", "=A1#"] {
            type_in(&mut a, "B1", f);
            assert!(a.answering(), "{f}");
            // Corrigir: volta ao editor com o texto.
            a.dialog_event(Outcome::Button(0));
            assert_eq!(a.editor.as_ref().unwrap().input.text, f);
            a.cancel_edit();
            assert_eq!(get(&a, 0, "B1"), "", "{f}");
        }
        assert!(!a.doc.as_ref().unwrap().dirty);
        // Cancelar deixa a célula como estava.
        type_in(&mut a, "A1", "=SOMA(");
        a.dialog_event(Outcome::Cancel);
        assert!(a.editor.is_none() && a.dialog.is_none());
        assert_eq!(get(&a, 0, "A1"), "5");
        for (f, shown) in [
            ("=SOMA(A1:A3", "=SOMA(A1:A3)"),
            ("='Outra aba'!A1+1,5", "='Outra aba'!A1+1,5"),
            ("=,5*2", "=,5*2"),
            ("=NaoExiste(1)", "=NAOEXISTE(1)"),
        ] {
            type_in(&mut a, "B1", f);
            assert!(a.dialog.is_none(), "{f}");
            assert_eq!(get(&a, 0, "B1"), shown);
        }
    }

    #[test]
    fn ctrl_enter_paste_and_macro() {
        let mut a = app(&[("Plan1", &[("A1", "5")])]);
        assert!(a.scripted_select("E1:E3"));
        a.start_edit(Some("=1+*2"), false);
        a.commit_edit_all();
        assert!(a.answering());
        a.dialog_event(Outcome::Cancel);
        assert_eq!(get(&a, 0, "E2"), "");
        // Colar de outro programa: fica como texto, sem perder nada.
        select(&mut a, "G1:G1");
        a.paste_text("=SOMA(\t=A1*2".into(), Default::default());
        assert_eq!((get(&a, 0, "G1"), get(&a, 0, "H1")), ("=SOMA(".into(), "=A1*2".into()));
        let d = a.doc.as_ref().unwrap();
        assert!(duck_calc::cell_formula(d.sheets[0].data.as_ref().unwrap(), 0, 6).is_none());
        // Macros (em inglês).
        assert_eq!(d.check_formula(0, "SUM(A1,2)", 0, 0).as_deref(), Ok("SUM(A1,2)"));
        assert!(d.check_formula(0, "SUM(A1", 0, 0).is_err());
    }
}
