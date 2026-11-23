//! Mover células (borda da seleção), inserir e excluir só células, AutoSoma,
//! data e hora de agora, Ctrl+Enter e as estatísticas da seleção.

use std::sync::Arc;

use duck_core::cells::CellOp;
use duck_core::input::{Input, parse_input};
use duck_core::{Axis, CellChange, CellRange, CellValue, CfCtx, Command, Shift, Value};
use rayon::prelude::*;

use crate::app::{MAX_COLS, MAX_ROWS, SheetsApp, fmt_int};
use crate::edit::{Step, should_apply};
use crate::fill::{cells, range_of};
use crate::forms::{Field, Form, FormId};
use crate::look::Looks;
use crate::paste::PasteOpts;
use crate::tabs::{TabOp, book};

/// Áreas selecionadas (recortadas aos dados) das estatísticas guardadas.
pub type StatsKey = (usize, Vec<(u32, u32, u32, u32)>);

/// Estatísticas da seleção (barra de status).
#[derive(Clone, Copy, Debug)]
pub struct SelStats {
    pub count: u64,
    pub numbers: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
}

impl Default for SelStats {
    fn default() -> Self {
        Self { count: 0, numbers: 0, sum: 0.0, min: f64::INFINITY, max: f64::NEG_INFINITY }
    }
}

impl SelStats {
    fn add(mut self, o: Self) -> Self {
        self.count += o.count;
        self.numbers += o.numbers;
        self.sum += o.sum;
        self.min = self.min.min(o.min);
        self.max = self.max.max(o.max);
        self
    }

    /// Texto da barra de status.
    pub fn text(&self) -> String {
        if self.count < 2 {
            return String::new();
        }
        let num = |x: f64, w: usize| {
            let mut s = String::new();
            duck_core::numfmt::format_general(x, w, &mut s);
            s
        };
        let mut s = String::new();
        if self.numbers > 0 {
            s.push_str(&format!("Média: {}   ", num(self.sum / self.numbers as f64, 11)));
        }
        s.push_str(&format!("Contagem: {}   ", fmt_int(self.count)));
        if self.numbers > 0 {
            if self.numbers != self.count {
                s.push_str(&format!("Números: {}   ", fmt_int(self.numbers)));
            }
            s.push_str(&format!("Mín: {}   Máx: {}   ", num(self.min, 11), num(self.max, 11)));
            s.push_str(&format!("Soma: {}   ", num(self.sum, 15)));
        }
        s
    }
}

impl SheetsApp {
    /// Contagem, soma, média, mínimo e máximo da seleção, em paralelo por
    /// coluna, quando a seleção muda. `None` com uma célula só.
    pub fn sel_stats(&mut self) -> Option<SelStats> {
        let doc = self.doc.as_ref()?;
        let (sheet, v) = doc.current()?;
        let areas: Vec<(u32, u32, u32, u32)> = v
            .areas()
            .into_iter()
            .map(|(r0, c0, r1, c1)| {
                let r1 = r1.min(sheet.rows().max(r0 + 1) - 1).max(r0);
                let c1 = c1.min(sheet.cols().max(c0 + 1) - 1).max(c0);
                (r0, c0, r1, c1)
            })
            .collect();
        let (s0, s1, s2, s3) = v.sel();
        if areas.len() == 1 && (s0, s1) == (s2, s3) {
            return None;
        }
        let key = (doc.active, areas);
        if let Some((k, s)) = &self.stats
            && *k == key
        {
            return Some(*s);
        }
        let strings = &doc.strings;
        let pool = self.ui_pool.get_or_insert_with(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(duck_par::physical_cores().clamp(1, 8))
                .thread_name(|i| format!("duck-ui-{i}"))
                .build()
                .expect("pool da interface")
        });
        let jobs: Vec<(u32, u32, u32)> =
            key.1.iter().flat_map(|&(r0, c0, r1, c1)| (c0..=c1).map(move |c| (c, r0, r1))).collect();
        let st = pool.install(|| {
            jobs.into_par_iter()
                .map(|(c, r0, r1)| {
                    let mut s = SelStats::default();
                    sheet.for_each_value(c, r0..r1 + 1, strings, |_, cell| {
                        s.count += 1;
                        if let Value::Number(n) = cell.value {
                            s.numbers += 1;
                            s.sum += n;
                            s.min = s.min.min(n);
                            s.max = s.max.max(n);
                        }
                        true
                    });
                    s
                })
                .reduce(SelStats::default, SelStats::add)
        });
        self.stats = Some((key, st));
        Some(st)
    }

    /// As áreas da seleção para formatar ou limpar: linha ou coluna inteira só
    /// até onde há dados. `None` (com aviso) se passar de `max` células.
    pub fn sel_areas(&mut self, max: u64) -> Option<Vec<(u32, u32, u32, u32)>> {
        let doc = self.doc.as_ref()?;
        let (sheet, v) = doc.current()?;
        let (nr, nc) = (v.rows.len(), v.cols.len());
        let areas: Vec<(u32, u32, u32, u32)> = v
            .areas()
            .into_iter()
            .map(|(r0, c0, r1, c1)| {
                let r1 = if r1 - r0 + 1 == nr { r1.min(sheet.rows().max(r0 + 1) - 1) } else { r1 };
                let c1 = if c1 - c0 + 1 == nc { c1.min(sheet.cols().max(c0 + 1) - 1) } else { c1 };
                (r0, c0, r1, c1)
            })
            .collect();
        if areas.iter().map(|a| cells(&range_of(*a))).sum::<u64>() > max {
            self.message = "Seleção grande demais para fazer de uma vez".into();
            return None;
        }
        Some(areas)
    }

    // --- mover e deslocar -----------------------------------------------------

    /// Aplica uma operação de células na aba ativa (num passo só).
    pub fn run_cell_op(&mut self, op: CellOp) -> bool {
        self.commit_edit();
        if !self.editable() {
            return false;
        }
        let Some(doc) = &self.doc else { return false };
        if doc.finished.is_none() || doc.sheets.iter().any(|s| s.data.is_none() || s.preview) {
            self.message = "Aguarde terminar de carregar".into();
            return false;
        }
        let active = doc.active;
        let refs: Vec<&duck_core::Sheet> = doc.sheets.iter().filter_map(|s| s.data.as_deref()).collect();
        let (mut step, names): (Step, _) =
            match duck_calc::cellops::plan(&refs, &doc.strings, &doc.calc.extras, active, op) {
                Ok(x) => x,
                Err(e) => {
                    self.message = e;
                    return false;
                }
            };
        if names.names != doc.calc.extras.names {
            step.push((active, book(TabOp::Refs(names))));
        }
        if !self.run_step(step) {
            return false;
        }
        if let Some(t) = op.target() {
            self.select_range(t);
        }
        self.message = match op {
            CellOp::Move { src, .. } => format!("{} células movidas", fmt_int(cells(&src))),
            CellOp::Shift { shift, lo, hi } => {
                let n = fmt_int(shift.n as u64 * (hi - lo + 1) as u64);
                format!("{n} células {}", if shift.insert { "inseridas" } else { "excluídas" })
            }
        };
        true
    }

    /// Solta o bloco `src` em `target` (Ctrl copia).
    pub fn drop_cells(&mut self, src: CellRange, target: CellRange, copy: bool) {
        if copy {
            self.select_range(src);
            if let Some(clip) = self.capture(&[src], false, String::new(), false) {
                let keep = self.clip.take();
                self.paste_clip(&clip, target, PasteOpts::default());
                // A mudança cancelou um recorte que estivesse esperando.
                self.clip = keep.filter(|c| !c.cut);
            }
            return;
        }
        let has = self.doc.as_ref().and_then(|d| d.current()).is_some_and(|(s, _)| {
            duck_core::cells::subtract(&target, &src).iter().any(|r| {
                (r.first_col..=r.last_col.min(s.cols().max(1) - 1)).any(|c| {
                    (r.first_row..=r.last_row.min(s.rows().max(1) - 1)).any(|row| s.has_value(row, c))
                })
            })
        });
        if has && !self.ask("Já há dados no destino. Substituir?") {
            return;
        }
        let dr = target.first_row as i64 - src.first_row as i64;
        let dc = target.first_col as i64 - src.first_col as i64;
        self.run_cell_op(CellOp::Move { src, dr, dc });
    }

    /// Abre a caixa de inserir ou excluir células.
    pub fn ask_cells(&mut self, insert: bool) {
        self.commit_edit();
        let Some(v) = self.doc.as_ref().and_then(|d| d.current()).map(|x| x.1) else { return };
        if v.multi() {
            self.message = "Escolha uma área só".into();
            return;
        }
        let (r0, c0, r1, c1) = v.sel();
        // Linhas ou colunas inteiras: não há o que perguntar.
        if r1 - r0 + 1 >= v.rows.len() || c1 - c0 + 1 >= v.cols.len() {
            self.shift_selection(insert, None);
            return;
        }
        // Mais larga que alta: o normal é deslocar para baixo (ou para cima).
        let down = c1 - c0 >= r1 - r0;
        let form = if insert {
            Form::new(
                FormId::InsertCells,
                "Inserir células",
                vec![Field::radio(
                    "to",
                    "",
                    &[
                        "Deslocar células para a direita",
                        "Deslocar células para baixo",
                        "Linha inteira",
                        "Coluna inteira",
                    ],
                    usize::from(down),
                )],
                &["OK", "Cancelar"],
            )
        } else {
            Form::new(
                FormId::DeleteCells,
                "Excluir células",
                vec![Field::radio(
                    "to",
                    "",
                    &[
                        "Deslocar células para a esquerda",
                        "Deslocar células para cima",
                        "Linha inteira",
                        "Coluna inteira",
                    ],
                    usize::from(down),
                )],
                &["OK", "Cancelar"],
            )
        };
        self.form = Some(form);
    }

    /// OK na caixa: 0 = na horizontal, 1 = na vertical, 2 = linhas, 3 = colunas.
    pub fn shift_cells(&mut self, insert: bool, to: usize) {
        if to >= 2 {
            self.shift_selection(insert, Some(if to == 2 { Axis::Rows } else { Axis::Cols }));
            return;
        }
        let Some(v) = self.doc.as_ref().and_then(|d| d.current()).map(|x| x.1) else { return };
        let (r0, c0, r1, c1) = v.sel();
        let (shift, lo, hi) = if to == 1 {
            (Shift { axis: Axis::Rows, at: r0, n: r1 - r0 + 1, insert }, c0, c1)
        } else {
            (Shift { axis: Axis::Cols, at: c0, n: c1 - c0 + 1, insert }, r0, r1)
        };
        self.run_cell_op(CellOp::Shift { shift, lo, hi });
    }

    // --- AutoSoma, data e hora --------------------------------------------------

    /// Alt+=: uma célula abre a edição com a soma sugerida; várias recebem as
    /// somas direto.
    pub fn autosum(&mut self) {
        self.commit_edit();
        let today = self.today;
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r0, c0, r1, c1) = v.sel();
        let list = duck_calc::editing::autosum(sheet, &doc.strings, range_of((r0, c0, r1, c1)));
        if (r0, c0) == (r1, c1) {
            let Some((_, _, f)) = list.first() else { return };
            let text = format!("={}", duck_calc::locale::to_pt(f));
            self.start_edit(Some(&text), false);
            if let Some(ed) = &mut self.editor {
                ed.enter_mode = false;
                // O intervalo sugerido fica selecionado: digitar ou apontar troca.
                if let (Some(a), Some(b)) = (ed.input.text.find('('), ed.input.text.rfind(')')) {
                    ed.input.select(a + 1, b);
                }
            }
            return;
        }
        let cfx = CfCtx { sheet, shared: &doc.strings, today };
        let changes: Vec<CellChange> = list
            .into_iter()
            .map(|(row, col, f)| CellChange {
                row,
                col,
                value: cfx.eval_formula(&f, row, col).unwrap_or(CellValue::Empty),
                formula: Some(f),
                // O formato do primeiro número somado.
                style: None,
            })
            .collect();
        let n = changes.len();
        let active = doc.active;
        if self.run_step(vec![(active, Command::SetCells(changes))]) {
            self.message = format!("{n} somas escritas");
        }
    }

    /// Ctrl+; (data) e Ctrl+Shift+; (hora): escreve no editor.
    pub fn insert_now(&mut self, time: bool) {
        let t = duck_platform::local_time();
        let text = if time {
            format!("{:02}:{:02}", t.hour, t.minute)
        } else {
            format!("{:02}/{:02}/{:04}", t.day, t.month, t.year)
        };
        match &mut self.editor {
            Some(ed) => ed.input.insert(&text),
            None => {
                self.start_edit(Some(&text), false);
                if let Some(ed) = &mut self.editor {
                    ed.enter_mode = false;
                }
            }
        }
    }

    /// Ctrl+Enter: o que foi digitado vai para todas as células selecionadas
    /// (as fórmulas ajustadas a cada uma).
    pub fn commit_edit_all(&mut self) {
        let Some(areas) = self.sel_areas(1_000_000) else { return };
        let single = matches!(areas.as_slice(), [(r0, c0, r1, c1)] if (r0, c0) == (r1, c1));
        if single {
            self.commit_edit();
            return;
        }
        let Some(ed) = self.editor.take() else { return };
        if self.doc.as_ref().is_some_and(|d| d.active != ed.sheet) {
            self.switch_sheet(ed.sheet);
        }
        let text = crate::pointing::close_parens(&ed.input.text).unwrap_or(ed.input.text);
        let today = self.today;
        let Some(doc) = self.doc.as_mut() else { return };
        let active = doc.active;
        let mut styles = doc.styles.clone();
        let before = styles.xfs.len();
        let Some((sheet, v)) = doc.current() else { return };
        let cfx = CfCtx { sheet, shared: &doc.strings, today };
        let input = parse_input(&text, doc.date1904, duck_core::input::today());
        let mut changes = Vec::new();
        for (r0, c0, r1, c1) in areas {
            for r in r0..=r1.min(MAX_ROWS - 1) {
                for c in c0..=c1.min(MAX_COLS - 1) {
                    // Só o canto de cada mesclagem.
                    if v.merges.at(r, c).is_some_and(|m| (m.first_row, m.first_col) != (r, c)) {
                        continue;
                    }
                    let s = sheet.raw(r, c).style;
                    let base = if s == 0 { v.blank_style(r, c) } else { s };
                    changes.push(match &input {
                        Input::Formula(f) => {
                            let f = duck_calc::locale::from_pt(f);
                            let f = duck_calc::shift(&f, r as i64 - ed.row as i64, c as i64 - ed.col as i64);
                            let value = cfx.eval_formula(&f, r, c).unwrap_or(CellValue::Empty);
                            CellChange { row: r, col: c, value, formula: Some(f), style: Some(base) }
                        }
                        Input::Value(value, fmt) => {
                            let cur = styles.xfs.get(base as usize).map_or(0, |x| x.num_fmt);
                            let style = match fmt {
                                Some(f) if should_apply(&styles, cur, *f) => {
                                    Arc::make_mut(&mut styles).derive_xf(base, |x| x.num_fmt = *f)
                                }
                                _ => base,
                            };
                            CellChange {
                                row: r,
                                col: c,
                                value: value.clone(),
                                formula: None,
                                style: Some(style),
                            }
                        }
                    });
                }
            }
        }
        if styles.xfs.len() != before {
            doc.looks = Arc::new(Looks::new(&styles));
            doc.styles = styles;
            self.font_key = (0, 0);
        }
        let n = changes.len() as u64;
        if self.run_step(vec![(active, Command::SetCells(changes))]) {
            self.message = format!("{} células preenchidas", fmt_int(n));
        }
    }

    /// `--edit` de células. `true` = era um deles.
    pub fn scripted_cells(&mut self, e: &str) -> bool {
        // `mover=A1:B2>D5`.
        if let Some(arg) = e.strip_prefix("mover=").or_else(|| e.strip_prefix("copiar-arrastando=")) {
            let rg = |s: &str| {
                let (a, b) = crate::app::parse_range(s)?;
                Some(range_of((a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))))
            };
            match arg.split_once('>').and_then(|(a, b)| Some((rg(a)?, rg(b)?))) {
                Some((src, at)) => {
                    let t = crate::fill::move_target(&src, (0, 0), at.first_row, at.first_col);
                    self.drop_cells(src, t, e.starts_with("copiar"));
                }
                None => eprintln!("--edit: use mover=A1:B2>D5"),
            }
            eprintln!("--edit {e}: {}", self.message);
            return true;
        }
        // `preencher-todas=texto:A1:B3;D1` (Ctrl+Enter).
        if let Some(arg) = e.strip_prefix("preencher-todas=") {
            if let Some((text, sel)) = arg.split_once(':')
                && self.scripted_select(sel)
            {
                self.start_edit(Some(text), false);
                self.commit_edit_all();
            }
            eprintln!("--edit {e}: {}", self.message);
            return true;
        }
        let Some((op, arg)) = e.split_once(':') else { return false };
        let known = matches!(
            op,
            "inserir-celulas-baixo"
                | "inserir-celulas-direita"
                | "excluir-celulas-cima"
                | "excluir-celulas-esquerda"
                | "autosoma"
                | "data"
                | "hora"
                | "selecionar"
                | "estatisticas"
        );
        if !known {
            return false;
        }
        if !self.scripted_select(arg) {
            eprintln!("--edit: intervalo inválido: {arg}");
            return true;
        }
        match op {
            "inserir-celulas-baixo" => self.shift_cells(true, 1),
            "inserir-celulas-direita" => self.shift_cells(true, 0),
            "excluir-celulas-cima" => self.shift_cells(false, 1),
            "excluir-celulas-esquerda" => self.shift_cells(false, 0),
            "autosoma" => {
                self.autosum();
                self.commit_edit();
            }
            "data" | "hora" => {
                self.insert_now(op == "hora");
                self.commit_edit();
            }
            "estatisticas" => {
                self.message = self.sel_stats().map(|s| s.text()).unwrap_or_default();
            }
            _ => return true,
        }
        eprintln!("--edit {op}: {}", self.message);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, get, select, style, type_in};

    fn rg(s: &str) -> CellRange {
        let (a, b) = crate::app::parse_range(s).unwrap();
        range_of((a.0, a.1, b.0, b.1))
    }

    #[test]
    fn drag_border_moves_and_copies() {
        let mut a =
            app(&[("P", &[("A1", "1"), ("A2", "=A1*2"), ("C1", "=SOMA(A1:A2)")]), ("Q", &[("A1", "=P!A2")])]);
        a.drop_cells(rg("A1:A2"), rg("E5:E6"), false);
        assert_eq!((get(&a, 0, "A1"), get(&a, 0, "E6")), ("".into(), "=E5*2".into()));
        assert_eq!((get(&a, 0, "C1"), get(&a, 1, "A1")), ("=SOMA(E5:E6)".into(), "=P!E6".into()));
        a.undo();
        assert_eq!(
            (get(&a, 0, "A2"), get(&a, 0, "C1"), get(&a, 1, "A1")),
            ("=A1*2".into(), "=SOMA(A1:A2)".into(), "=P!A2".into())
        );
        a.redo();
        assert_eq!(get(&a, 0, "E5"), "1");
        a.undo();
        a.drop_cells(rg("A1:A2"), rg("E5:E6"), true);
        assert_eq!((get(&a, 0, "A2"), get(&a, 0, "E6")), ("=A1*2".into(), "=E5*2".into()));
        assert_eq!(get(&a, 0, "C1"), "=SOMA(A1:A2)");
    }

    #[test]
    fn insert_and_delete_cells() {
        let mut a = app(&[("P", &[("A1", "1"), ("A2", "2"), ("B2", "9"), ("C1", "=SOMA(A1:A2)")])]);
        select(&mut a, "A1");
        a.shift_cells(true, 1);
        assert_eq!(
            (get(&a, 0, "A1"), get(&a, 0, "A3"), get(&a, 0, "B2")),
            ("".into(), "2".into(), "9".into())
        );
        assert_eq!(get(&a, 0, "C1"), "=SOMA(A2:A3)");
        a.undo();
        select(&mut a, "A2");
        a.shift_cells(false, 0);
        assert_eq!((get(&a, 0, "A2"), get(&a, 0, "B2")), ("9".into(), "".into()));
        a.undo();
        assert_eq!(
            (get(&a, 0, "A2"), get(&a, 0, "B2"), get(&a, 0, "C1")),
            ("2".into(), "9".into(), "=SOMA(A1:A2)".into())
        );
        // A caixa pergunta; com linhas inteiras, não.
        select(&mut a, "A1:B1");
        a.ask_cells(true);
        assert_eq!(a.form.take().unwrap().sel("to"), 1);
        a.shift_cells(true, 2);
        assert_eq!(get(&a, 0, "A2"), "1");
    }

    #[test]
    fn sums_dates_and_ctrl_enter() {
        let mut a = app(&[("P", &[("A1", "1"), ("A2", "2"), ("B1", "3"), ("B2", "4")])]);
        select(&mut a, "A3");
        a.autosum();
        assert_eq!(a.editor.as_ref().unwrap().input.text, "=SOMA(A1:A2)");
        a.commit_edit();
        assert_eq!(get(&a, 0, "A3"), "=SOMA(A1:A2)");
        select(&mut a, "A1:B2");
        a.autosum();
        assert_eq!((get(&a, 0, "A3"), get(&a, 0, "B3")), ("=SOMA(A1:A2)".into(), "=SOMA(B1:B2)".into()));
        select(&mut a, "D1");
        a.insert_now(false);
        a.commit_edit();
        assert!(get(&a, 0, "D1").parse::<f64>().unwrap() > 45000.0);
        assert_eq!(style(&a, 0, "D1").num_fmt, 14);
        select(&mut a, "D2");
        a.insert_now(true);
        a.commit_edit();
        assert!((0.0..1.0).contains(&get(&a, 0, "D2").replace(',', ".").parse::<f64>().unwrap()));
        // Ctrl+Enter em duas áreas.
        assert!(a.scripted_select("F1:F2;H1"));
        a.start_edit(Some("=A1*10"), false);
        // A célula editada é a ativa (H1).
        a.commit_edit_all();
        assert_eq!((get(&a, 0, "H1"), get(&a, 0, "F2")), ("=A1*10".into(), "=#REF!*10".into()));
        a.undo();
        assert!(a.scripted_select("F1:G2"));
        a.start_edit(Some("10%"), false);
        a.commit_edit_all();
        assert_eq!((get(&a, 0, "F1"), style(&a, 0, "G2").num_fmt), ("0,1".into(), 9));
        a.undo();
        assert_eq!(get(&a, 0, "F1"), "");
    }

    #[test]
    fn stats_of_several_areas() {
        let mut a = app(&[("P", &[("A1", "1"), ("A2", "5"), ("C1", "-2"), ("C2", "texto")])]);
        assert!(a.scripted_select("A1:A2;C1:C2"));
        let s = a.sel_stats().unwrap();
        assert_eq!((s.count, s.numbers, s.sum, s.min, s.max), (4, 3, 4.0, -2.0, 5.0));
        assert!(s.text().contains("Mín: -2") && s.text().contains("Máx: 5"), "{}", s.text());
        select(&mut a, "A1");
        assert!(a.sel_stats().is_none());
        type_in(&mut a, "A1", "7");
    }

    #[test]
    fn format_and_clear_several_areas() {
        let mut a = app(&[("P", &[("A1", "1"), ("C1", "2"), ("B1", "3")])]);
        assert!(a.scripted_select("A1;C1"));
        a.apply_fmt(crate::format::Fmt::Bold);
        let bold = |a: &SheetsApp, c: &str| {
            let f = style(a, 0, c).font;
            a.doc.as_ref().unwrap().styles.fonts[f as usize].bold
        };
        assert!(bold(&a, "A1") && bold(&a, "C1") && !bold(&a, "B1"));
        a.clear_selection();
        assert_eq!(
            (get(&a, 0, "A1"), get(&a, 0, "B1"), get(&a, 0, "C1")),
            ("".into(), "3".into(), "".into())
        );
        a.undo();
        assert_eq!(get(&a, 0, "C1"), "2");
        a.undo();
        assert!(!bold(&a, "C1"));
        // Uma seleção comum deixa as outras áreas para trás.
        select(&mut a, "B1");
        assert!(!a.doc.as_ref().unwrap().current().unwrap().1.multi());
    }
}
