//! Edição no app (F3): editor na célula e na barra de fórmulas, comandos
//! com desfazer/refazer, larguras e alturas, e salvar.
//!
//! - Toda mudança vira um [`Command`] aplicado à aba (ADR-009); o inverso
//!   vai para a pilha de desfazer.
//! - A aba fica num `Arc` (a busca lê numa thread): para editar, o app pega
//!   a única referência ([`sheet_mut`]). Durante o salvamento, que lê as abas
//!   numa thread, a edição espera.
//! - Salvar roda numa thread e grava por remendo, com conferência antes de
//!   substituir o arquivo (`duck_xlsx::save_file`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use duck_core::input::{Input, parse_input};
use duck_core::{CellChange, CellValue, CfCtx, CfState, Command, Sheet, Value};
use duck_platform::{Clipboard, Key, Modifiers, SaveChoice};
use duck_ui::TextInput;

use crate::app::{SheetsApp, Slot, col_px, row_px};
use crate::look::Looks;

/// Célula sendo editada.
pub struct Editor {
    pub row: u32,
    pub col: u32,
    pub input: TextInput,
    /// Começou digitando por cima: as setas confirmam e andam (como no Excel).
    /// Com F2, duplo clique ou na barra de fórmulas, as setas andam no texto.
    pub enter_mode: bool,
    /// O cursor está na barra de fórmulas (senão, na célula).
    pub in_bar: bool,
    /// A célula quebra o texto: o editor também.
    pub wrap: bool,
    /// Aba da célula editada, e se outra está à vista (apontando lá).
    pub sheet: usize,
    pub away: bool,
    /// Referência recém-apontada: o próximo clique ou seta a troca.
    pub point: Option<crate::pointing::Point>,
    /// Lista de sugestões e assinatura da função.
    pub complete: crate::complete::State,
}

/// Salvamento em andamento.
pub struct SaveJob {
    rx: Receiver<Result<duck_xlsx::SaveReport, String>>,
    dest: PathBuf,
    started: Instant,
}

/// Um passo de desfazer/refazer: comandos (aba, comando), aplicados em
/// ordem. Inserir linhas numa aba também ajusta fórmulas de outras abas:
/// tudo vira um passo só.
pub type Step = Vec<(usize, Command)>;

#[derive(Default)]
pub struct History {
    pub undo: Vec<Step>,
    pub redo: Vec<Step>,
}

/// O que um comando muda na tela (além das células).
#[derive(Default)]
pub struct Affected {
    /// Linhas com células mudadas (altura automática); poucas.
    auto: Vec<u32>,
    /// Refazer a visão inteira do layout (estrutura, ou mudanças grandes).
    rebuild: bool,
    rows: Vec<u32>,
    cols: Vec<u32>,
}

/// Mais que isso de linhas/colunas com tamanho mudado: a visão é refeita de
/// uma vez (ajustar uma a uma refaz as somas de alturas a cada vez).
const REBUILD_OVER: usize = 2000;

fn affected(cmd: &Command) -> Affected {
    let mut a = Affected::default();
    collect_affected(cmd, &mut a);
    a.auto = auto_list(cmd);
    if a.rows.len() > REBUILD_OVER || a.cols.len() > REBUILD_OVER {
        a.rebuild = true;
        a.rows.clear();
        a.cols.clear();
    }
    a
}

/// Linhas das células de um comando, se são poucas.
fn auto_list(cmd: &Command) -> Vec<u32> {
    let size = |c: &Command| match c {
        Command::SetCells(v) => v.len(),
        Command::Restore(v) => v.len(),
        Command::SetStyles(v) => v.len(),
        _ => 0,
    };
    let total = match cmd {
        Command::Group(v) => v.iter().map(size).sum(),
        c => size(c),
    };
    if total == 0 || total > 2000 {
        return Vec::new();
    }
    let mut rows: Vec<u32> = cmd.cells().into_iter().map(|c| c.0).collect();
    rows.sort_unstable();
    rows.dedup();
    rows.truncate(200);
    rows
}

fn collect_affected(cmd: &Command, a: &mut Affected) {
    match cmd {
        Command::Shift(..) => a.rebuild = true,
        Command::RowHeights(v) => a.rows.extend(v.iter().map(|x| x.0)),
        Command::RowsHidden(v) => a.rows.extend(v.iter().map(|x| x.0)),
        Command::ColWidths(v) => a.cols.extend(v.iter().map(|x| x.0)),
        Command::ColsHidden(v) => a.cols.extend(v.iter().map(|x| x.0)),
        Command::Group(v) => v.iter().for_each(|c| collect_affected(c, a)),
        _ => {}
    }
}

/// A aba para escrita. A busca pode estar terminando numa thread com uma
/// cópia do `Arc`: espera um pouco por ela (a busca já foi cancelada).
pub(crate) fn sheet_mut(slot: &mut Slot) -> Option<&mut Sheet> {
    let arc = slot.data.as_mut()?;
    for _ in 0..300 {
        if Arc::strong_count(arc) == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Arc::get_mut(arc)
}

/// Nenhuma aba está sendo lida por uma thread.
fn free(doc: &mut crate::app::Doc) -> bool {
    doc.sheets.iter_mut().all(|s| sheet_mut(s).is_some())
}

/// O formato sugerido pelo que foi digitado (`new`) substitui o da célula
/// (`current`)? Como no Excel: sempre numa célula "Geral"; datas, horas e
/// percentuais também trocam um formato de outro tipo (uma data digitada
/// numa célula de moeda vira data); os outros só valem em "Geral".
pub(crate) fn should_apply(styles: &duck_core::Styles, current: u16, new: u16) -> bool {
    use duck_core::input::fmt;
    if current == 0 {
        return true;
    }
    let code = |id| duck_core::NumFmt::parse(styles.num_fmt_code(id));
    let cur = code(current);
    if cur.is_general() {
        return true;
    }
    let is_pct =
        |id: u16, f: &duck_core::NumFmt| id == fmt::PERCENT || id == fmt::PERCENT_DEC || f.is_percent();
    match new {
        fmt::DATE | fmt::TIME | fmt::TIME_SEC | fmt::DATE_TIME => !cur.is_date(),
        fmt::PERCENT | fmt::PERCENT_DEC => !is_pct(current, &cur),
        _ => false,
    }
}

/// Largura em caracteres (como no arquivo) que dá `px` pixels (inverso de [`col_px`]).
pub fn px_to_width(px: u32, mdw: f32) -> f32 {
    if px == 0 {
        return 0.0;
    }
    let t = (128.0 / mdw).trunc();
    let mut w = (((px as f32 + 0.5) / mdw - t / 256.0) * 256.0).round() / 256.0;
    for _ in 0..8 {
        match col_px(w, mdw).cmp(&px) {
            std::cmp::Ordering::Less => w += 1.0 / 256.0,
            std::cmp::Ordering::Greater => w -= 1.0 / 256.0,
            std::cmp::Ordering::Equal => break,
        }
    }
    w.max(0.0)
}

impl SheetsApp {
    // --- comandos -------------------------------------------------------------

    /// A aba ativa pode ser editada agora? Se não, explica na barra de status.
    pub(crate) fn editable(&mut self) -> bool {
        if self.saving.is_some() {
            self.message = "Aguarde: salvando…".into();
            return false;
        }
        // O cálculo em andamento lê as abas: para (em milissegundos) e
        // recomeça depois da edição com o que faltava.
        if let Some(doc) = &mut self.doc {
            doc.calc.cancel();
        }
        let Some(doc) = &self.doc else { return false };
        let slot = doc.slot();
        if slot.data.is_none() || slot.preview {
            self.message = "A aba ainda está carregando".into();
            return false;
        }
        true
    }

    /// Aplica um comando na aba ativa e guarda o inverso para desfazer.
    pub fn run_command(&mut self, cmd: Command) -> bool {
        if !self.editable() {
            return false;
        }
        // A busca lê a aba numa thread: cancela (o resultado ficaria velho).
        if let Some(f) = &mut self.find {
            f.job = None;
            f.result = None;
            f.done_query.clear();
        }
        let Some(doc) = &mut self.doc else { return false };
        let idx = doc.active;
        let Some(sheet) = sheet_mut(&mut doc.sheets[idx]) else {
            self.message = "A aba está em uso; tente de novo".into();
            return false;
        };
        let aff = affected(&cmd);
        doc.calc.note(idx, &cmd);
        let filter_changed = matches!(cmd, Command::SetFilter { .. } | Command::Group(_));
        let inverse = cmd.apply(sheet);
        doc.history.undo.push(vec![(idx, inverse)]);
        doc.history.redo.clear();
        doc.dirty = true;
        self.after_change(idx, aff);
        if filter_changed {
            self.refresh_table_slicers();
        }
        self.refresh_charts();
        self.calc_kick();
        true
    }

    /// Junta os dois últimos passos de desfazer num só (ex.: gravar o filtro
    /// e esconder as linhas).
    pub fn merge_last_steps(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        let h = &mut doc.history.undo;
        if h.len() >= 2 {
            let mut last = h.pop().unwrap_or_default();
            let prev = h.pop().unwrap_or_default();
            last.extend(prev);
            h.push(last);
        }
    }

    /// Aplica vários comandos (em abas diferentes) como um passo só.
    /// Os que não puderem ser aplicados são pulados (com aviso).
    pub fn run_step(&mut self, step: Step) -> bool {
        if !self.editable() {
            return false;
        }
        if let Some(f) = &mut self.find {
            f.job = None;
            f.result = None;
            f.done_query.clear();
        }
        let Some(doc) = &mut self.doc else { return false };
        let user_cells: Vec<(usize, u32, u32)> =
            step.iter().flat_map(|(i, c)| c.cells().into_iter().map(move |(r, cc)| (*i, r, cc))).collect();
        let shifted = step.iter().any(|(_, c)| matches!(c, Command::Shift(..)));
        let book = crate::tabs::has_book(&step);
        let lists_only = crate::tabs::lists_only(&step);
        if book && !free(doc) {
            self.message = "Uma aba está em uso; tente de novo".into();
            return false;
        }
        let mut inverse = Vec::with_capacity(step.len());
        let mut touched = Vec::new();
        for (idx, cmd) in step {
            if let Command::Book(op) = &cmd {
                inverse.extend(crate::tabs::apply(doc, op).map(|c| (idx, c)));
                continue;
            }
            let st = affected(&cmd);
            let Some(sheet) = doc.sheets.get_mut(idx).and_then(sheet_mut) else { continue };
            doc.calc.note(idx, &cmd);
            inverse.push((idx, cmd.apply(sheet)));
            touched.push((idx, st));
        }
        if inverse.is_empty() {
            return false;
        }
        inverse.reverse();
        doc.history.undo.push(inverse);
        doc.history.redo.clear();
        doc.dirty = true;
        self.after_step(touched, book, lists_only);
        if shifted || book {
            self.init_spills();
        } else {
            self.check_spills(&user_cells);
        }
        self.refresh_charts();
        self.calc_kick();
        true
    }

    /// Depois de aplicar um passo: atualiza as abas tocadas. Com a lista de
    /// abas mudada, os índices do passo não valem mais: atualiza todas.
    fn after_step(&mut self, touched: Vec<(usize, Affected)>, book: bool, lists_only: bool) {
        if !book {
            for (idx, st) in touched {
                self.after_change(idx, st);
            }
            return;
        }
        let rebuild = touched.iter().any(|(_, a)| a.rebuild);
        let n = self.doc.as_ref().map_or(0, |d| d.sheets.len());
        for i in 0..n {
            self.after_change(i, Affected { rebuild, ..Default::default() });
        }
        if !lists_only {
            self.clip = None;
        }
        self.tab_edit = None;
        self.tab_revealed = None;
        self.drag = None;
        if let Some(doc) = &mut self.doc {
            doc.pivots.cube = Default::default();
        }
        self.refresh_table_slicers();
        self.clamp_scroll();
    }

    pub fn undo(&mut self) {
        self.step_history(true);
    }

    pub fn redo(&mut self) {
        self.step_history(false);
    }

    fn step_history(&mut self, undo: bool) {
        self.cancel_edit();
        let Some(doc) = &self.doc else { return };
        let next = if undo { doc.history.undo.last() } else { doc.history.redo.last() };
        let Some(first) = next.and_then(|step| step.first()).map(|x| x.0) else {
            self.message = if undo { "Nada para desfazer" } else { "Nada para refazer" }.into();
            return;
        };
        // Vai para a aba do passo, se ele não mexe na que está à vista.
        if !next.is_some_and(|step| step.iter().any(|x| x.0 == doc.active)) {
            self.switch_sheet(first);
        }
        if !self.editable() {
            return;
        }
        if let Some(f) = &mut self.find {
            f.job = None;
            f.result = None;
            f.done_query.clear();
        }
        let Some(doc) = &mut self.doc else { return };
        let from = if undo { &mut doc.history.undo } else { &mut doc.history.redo };
        let Some(step) = from.pop() else { return };
        // Confere antes que todas as abas do passo estão livres.
        let book = crate::tabs::has_book(&step);
        let lists_only = crate::tabs::lists_only(&step);
        let busy = if book {
            doc.sheets.iter_mut().any(|s| sheet_mut(s).is_none())
        } else {
            step.iter().any(|(i, _)| doc.sheets.get_mut(*i).and_then(sheet_mut).is_none())
        };
        if busy {
            from.push(step);
            self.message = "A aba está em uso; tente de novo".into();
            return;
        }
        let shown = doc.active;
        let cells = step.iter().find(|(i, _)| *i == shown).map(|(_, c)| c.cells()).unwrap_or_default();
        let user_cells: Vec<(usize, u32, u32)> =
            step.iter().flat_map(|(i, c)| c.cells().into_iter().map(move |(r, cc)| (*i, r, cc))).collect();
        let shifted = step.iter().any(|(_, c)| matches!(c, Command::Shift(..)));
        let mut inverse = Vec::with_capacity(step.len());
        let mut touched = Vec::new();
        for (idx, cmd) in step {
            if let Command::Book(op) = &cmd {
                inverse.extend(crate::tabs::apply(doc, op).map(|c| (idx, c)));
                continue;
            }
            let st = affected(&cmd);
            doc.calc.note(idx, &cmd);
            let Some(sheet) = doc.sheets.get_mut(idx).and_then(sheet_mut) else { continue };
            inverse.push((idx, cmd.apply(sheet)));
            touched.push((idx, st));
        }
        inverse.reverse();
        let (undo_list, redo_list) = (&mut doc.history.undo, &mut doc.history.redo);
        if undo { redo_list } else { undo_list }.push(inverse);
        doc.dirty = true;
        self.after_step(touched, book, lists_only);
        if shifted || book {
            self.init_spills();
        } else {
            self.check_spills(&user_cells);
        }
        self.calc_kick();
        // Mostra o que mudou.
        if let Some(&(r, c)) = cells.first() {
            self.go_to(r, c);
        }
        self.message = if undo { "Desfeito" } else { "Refeito" }.into();
    }

    /// Depois de mudar a aba `idx`: tamanhos na tela, formatação condicional
    /// (os dados calculados das regras ficam velhos), estatísticas e título.
    fn after_change(&mut self, idx: usize, aff: Affected) {
        self.unmark();
        self.stats = None;
        let Some(doc) = &mut self.doc else { return };
        let mdw = doc.mdw;
        let Some(slot) = doc.sheets.get_mut(idx) else { return };
        if aff.rebuild {
            // Linhas/colunas andaram: a visão (tamanhos, mesclagens, estilos
            // de linha e coluna) é refeita do layout novo.
            if let (Some(sheet), Some(mut v)) = (slot.data.as_ref(), slot.view.take()) {
                v.resized_cols.clear();
                v.resized_rows.clear();
                slot.view = Some(crate::app::build_view(sheet, mdw, Some(v), None));
            }
            self.update_title();
            return;
        }
        let (Some(sheet), Some(v)) = (slot.data.as_ref(), slot.view.as_mut()) else { return };
        v.cf = sheet.layout.cond.iter().map(|_| CfState::default()).collect();
        // Mesclagens e painéis congelados vêm do layout.
        v.merges = crate::app::Merges::new(sheet.layout.merges.clone());
        let pane = sheet.layout.pane.unwrap_or_default();
        let (fr, fc) = (pane.frozen_rows.min(v.rows.len() - 1), pane.frozen_cols.min(v.cols.len() - 1));
        if (fr, fc) != (v.frozen_rows, v.frozen_cols) {
            v.frozen_rows = fr;
            v.frozen_cols = fc;
            v.scroll_y = v.scroll_y.max(v.rows.offset(fr) as f64);
            v.scroll_x = v.scroll_x.max(v.cols.offset(fc) as f64);
        }
        let def_col = v.cols.default_size();
        let def_row = v.rows.default_size();
        for &c in &aff.cols {
            if c < v.cols.len() {
                let px = if sheet.col_hidden(c) {
                    0
                } else {
                    sheet.col_width(c).map_or(def_col, |w| col_px(w, mdw))
                };
                if v.cols.size(c) != px {
                    v.resize_col(c, px);
                }
            }
        }
        for &r in &aff.rows {
            if r < v.rows.len() {
                let px = if sheet.row_hidden(r) { 0 } else { sheet.row_height(r).map_or(def_row, row_px) };
                if v.rows.size(r) != px {
                    v.resize_row(r, px);
                }
            }
        }
        let auto = aff.auto;
        self.auto_rows(idx, &auto);
        self.update_title();
    }

    pub fn update_title(&self) {
        let (Some(w), Some(doc)) = (&self.window, &self.doc) else { return };
        let name = self.file_label(&doc.path);
        let dot = if doc.dirty { "• " } else { "" };
        w.set_title(&format!("{dot}{name} — duck-sheets"));
    }

    /// As colunas que mudam junto com `col`: se ela está numa seleção de
    /// colunas inteiras, todas as selecionadas (menos as ocultas).
    pub fn cols_with(&self, col: u32) -> Vec<u32> {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return vec![col] };
        let last = v.rows.len() - 1;
        let whole: Vec<_> = v.areas().into_iter().filter(|a| a.0 == 0 && a.2 == last).collect();
        if !whole.iter().any(|a| (a.1..=a.3).contains(&col)) {
            return vec![col];
        }
        let mut list: Vec<u32> = whole.iter().flat_map(|a| a.1..=a.3).collect();
        list.sort_unstable();
        list.dedup();
        list.retain(|&c| c == col || !sheet.col_hidden(c));
        list
    }

    /// As linhas que mudam junto com `row` (ver [`Self::cols_with`]).
    pub fn rows_with(&self, row: u32) -> Vec<u32> {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return vec![row] };
        let last = v.cols.len() - 1;
        let whole: Vec<_> = v.areas().into_iter().filter(|a| a.1 == 0 && a.3 == last).collect();
        if !whole.iter().any(|a| (a.0..=a.2).contains(&row)) {
            return vec![row];
        }
        let mut list: Vec<u32> = whole.iter().flat_map(|a| a.0..=a.2.min(a.0 + 100_000)).collect();
        list.push(row);
        list.sort_unstable();
        list.dedup();
        list.retain(|&r| r == row || !sheet.row_hidden(r));
        list
    }

    /// Larguras (em pixels a 100%) viram comando, num passo de desfazer só.
    pub fn commit_cols(&mut self, list: &[(u32, u32)]) {
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let default = v.cols.default_size();
        let changes: Vec<(u32, Option<f32>)> = list
            .iter()
            .filter(|(c, px)| *px != default || sheet.col_width(*c).is_some())
            .map(|(c, px)| (*c, Some(px_to_width(*px, doc.mdw))))
            .filter(|(c, w)| *w != sheet.col_width(*c))
            .collect();
        if !changes.is_empty() {
            self.run_command(Command::ColWidths(changes));
        }
    }

    /// Fim do arraste da borda de uma coluna, que tinha `before` pixels.
    pub fn commit_col_size(&mut self, col: u32, before: u32) {
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let px = v.cols.size(col);
        // Só um clique na borda: nada muda.
        if px != before {
            let list: Vec<(u32, u32)> = self.cols_with(col).into_iter().map(|c| (c, px)).collect();
            self.commit_cols(&list);
        }
    }

    /// Fim do arraste da borda de uma linha, que tinha `before` pixels.
    pub fn commit_row_size(&mut self, row: u32, before: u32) {
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let px = v.rows.size(row);
        if px != before {
            let list = self.rows_with(row);
            self.commit_rows(&list, Some(px as f32 * 0.75));
        }
    }

    /// Altura das linhas (`None` = automática), num passo de desfazer só.
    pub fn commit_rows(&mut self, rows: &[u32], height: Option<f32>) {
        let Some((sheet, _)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let list: Vec<(u32, Option<f32>)> =
            rows.iter().filter(|&&r| sheet.row_height(r) != height).map(|&r| (r, height)).collect();
        if !list.is_empty() {
            self.run_command(Command::RowHeights(list));
        }
    }

    /// Delete: apaga os valores da seleção (o formato fica).
    pub fn clear_selection(&mut self) {
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let mut changes = Vec::new();
        const MAX: usize = 2_000_000;
        let (last_row, last_col) = (sheet.rows().max(1) - 1, sheet.cols().max(1) - 1);
        let cols = v
            .areas()
            .into_iter()
            .flat_map(|(r0, c0, r1, c1)| (c0..=c1.min(last_col)).map(move |c| (c, r0, r1.min(last_row))));
        for (c, r0, r1) in cols {
            sheet.for_each_value(c, r0..r1.saturating_add(1), &doc.strings, |r, _| {
                changes.push(CellChange {
                    row: r,
                    col: c,
                    value: CellValue::Empty,
                    formula: None,
                    style: None,
                });
                changes.len() <= MAX
            });
            if changes.len() > MAX {
                self.message = "Seleção grande demais para apagar de uma vez".into();
                return;
            }
        }
        if changes.is_empty() {
            return;
        }
        let n = changes.len();
        if self.run_command(Command::SetCells(changes)) {
            self.message = format!("{} células apagadas", crate::app::fmt_int(n as u64));
        }
    }

    // --- editor ---------------------------------------------------------------

    /// Começa a editar a célula ativa. `initial = None`: com o conteúdo atual
    /// (F2, duplo clique, barra de fórmulas); `Some(t)`: por cima, com `t`.
    pub fn start_edit(&mut self, initial: Option<&str>, in_bar: bool) {
        if !self.editable() {
            return;
        }
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let (mut row, mut col) = v.cur;
        if let Some(m) = v.merges.at(row, col) {
            (row, col) = (m.first_row, m.first_col);
        }
        let mut input = TextInput::new();
        input.multiline = true;
        let wrap = self.doc.as_ref().and_then(|d| Some((d, d.current()?))).is_some_and(|(d, (s, v))| {
            let st = s.raw(row, col).style;
            let st = if st == 0 { v.blank_style(row, col) } else { st };
            d.styles.xfs.get(st as usize).is_some_and(|x| x.alignment.wrap)
        });
        let enter_mode = initial.is_some() && !in_bar;
        match initial {
            Some(t) => input.set_text(t),
            None => {
                let mut s = String::new();
                self.formula_text(&mut s);
                input.set_text(&s);
            }
        }
        self.namebox_focus = false;
        if let Some(f) = &mut self.find {
            f.focused = false;
        }
        let sheet = self.doc.as_ref().map_or(0, |d| d.active);
        self.editor = Some(Editor {
            row,
            col,
            input,
            enter_mode,
            in_bar,
            wrap,
            sheet,
            away: false,
            point: None,
            complete: Default::default(),
        });
        // A lista só abre ao digitar; ao editar o que já existe, só a assinatura.
        self.refresh_complete(initial.is_some());
    }

    pub fn cancel_edit(&mut self) {
        if let Some(ed) = self.editor.take() {
            self.back_home(&ed);
        }
    }

    /// De volta para a aba da célula editada (depois de apontar em outra).
    fn back_home(&mut self, ed: &Editor) {
        if self.doc.as_ref().is_some_and(|d| d.active != ed.sheet) {
            self.switch_sheet(ed.sheet);
        }
    }

    /// Confirma o que foi digitado. `false` se não havia editor.
    pub fn commit_edit(&mut self) -> bool {
        let Some(ed) = self.editor.take() else { return false };
        self.back_home(&ed);
        let (r, c) = (ed.row, ed.col);
        // Parênteses que faltam no fim (`=SOMA(B2:B10`).
        let text = crate::pointing::close_parens(&ed.input.text).unwrap_or(ed.input.text);
        // Sem mudança (F2 e Enter sem mexer): nada a fazer.
        let mut before = String::new();
        self.formula_text_at(r, c, &mut before);
        if before == text {
            return true;
        }
        let mut rich = None;
        let today = self.today;
        let Some(doc) = &mut self.doc else { return true };
        let Some((sheet, v)) = doc.current() else { return true };
        let cell = sheet.cell(r, c, &doc.strings);
        let base = if cell.style == 0 { v.blank_style(r, c) } else { cell.style };
        let change = match parse_input(&text, doc.date1904, duck_core::input::today()) {
            Input::Formula(f) => {
                // Valor provisório (o recálculo em segundo plano dá o certo).
                let cx = CfCtx { sheet, shared: &doc.strings, today };
                let value = cx.eval_formula(&f, r, c).unwrap_or(CellValue::Empty);
                let f = duck_calc::locale::from_pt(&f);
                CellChange { row: r, col: c, value, formula: Some(f), style: Some(base) }
            }
            Input::Value(value, fmt) => {
                let mut style = base;
                let current = doc.styles.xfs.get(base as usize).map_or(0, |x| x.num_fmt);
                // Texto rico: os trechos que não mudaram guardam a fonte.
                if matches!(value, CellValue::Text(_))
                    && let Some(runs) = sheet.rich_at(r, c, &doc.strings)
                    && let Some(runs) = duck_core::rich::rebase(&before, runs, &text)
                {
                    rich = Some(Command::RichText(vec![(r, c, runs)]));
                }
                let breaks = matches!(&value, CellValue::Text(t) if t.contains('\n'));
                let n = doc.styles.xfs.len();
                if let Some(f) = fmt
                    && should_apply(&doc.styles, current, f)
                {
                    style = Arc::make_mut(&mut doc.styles).derive_xf(base, |x| x.num_fmt = f);
                } else if breaks && !doc.styles.xfs.get(base as usize).is_some_and(|x| x.alignment.wrap) {
                    // Uma quebra escrita liga a quebra de texto, como no Excel.
                    style = Arc::make_mut(&mut doc.styles).derive_xf(base, |x| x.alignment.wrap = true);
                }
                if doc.styles.xfs.len() != n {
                    doc.looks = Arc::new(Looks::new(&doc.styles));
                    self.font_key = (0, 0);
                }
                CellChange { row: r, col: c, value, formula: None, style: Some(style) }
            }
        };
        let set = Command::SetCells(vec![change]);
        self.run_command(match rich {
            Some(x) => Command::Group(vec![set, x]),
            None => set,
        });
        true
    }

    /// Texto da barra de fórmulas para uma célula (a ativa é o caso comum).
    fn formula_text_at(&self, r: u32, c: u32, out: &mut String) {
        out.clear();
        let Some(doc) = &self.doc else { return };
        let Some((sheet, _)) = doc.current() else { return };
        if let Some(f) = duck_calc::cell_formula(sheet, r, c) {
            out.push('=');
            out.push_str(&duck_calc::locale::to_pt(&f));
            return;
        }
        let cell = sheet.cell(r, c, &doc.strings);
        if matches!(cell.value, Value::Empty) {
            return;
        }
        crate::look::edit_text(&doc.looks, doc.looks.get(cell.style), cell.value, doc.date1904, out);
    }

    /// Teclas com o editor aberto. `None` = sem editor (segue o fluxo normal).
    pub fn editor_key(
        &mut self,
        clipboard: &mut Clipboard,
        key: &Key,
        mods: Modifiers,
        text: Option<&str>,
    ) -> Option<bool> {
        self.editor.as_ref()?;
        if self.complete_key(key, mods) {
            return Some(true);
        }
        let shift = mods.shift;
        let ctrl = mods.ctrl || mods.logo;
        let arrow = match key {
            Key::ArrowUp => (-1, 0),
            Key::ArrowDown => (1, 0),
            Key::ArrowLeft => (0, -1),
            Key::ArrowRight => (0, 1),
            _ => (0, 0),
        };
        if arrow != (0, 0) && !mods.alt && self.point_key(arrow.0, arrow.1, shift, ctrl) {
            return Some(true);
        }
        // Ctrl+PageUp/PageDown: apontar em outra aba.
        if ctrl && matches!(key, Key::PageUp | Key::PageDown) && self.can_point() {
            self.cycle_sheet(*key == Key::PageDown);
            return Some(true);
        }
        if *key == Key::F(4) && !ctrl && !mods.alt {
            let ed = self.editor.as_mut()?;
            if crate::pointing::cycle_in(ed) {
                self.refresh_complete(false);
            }
            return Some(true);
        }
        let ed = self.editor.as_mut()?;
        let mv = |app: &mut Self, dr: i32, dc: i32| {
            app.commit_edit();
            app.move_cur(dr, dc, false, false);
        };
        match key {
            Key::Escape => self.cancel_edit(),
            // Alt+Enter quebra a linha; Ctrl+Enter preenche a seleção.
            Key::Enter if mods.alt => {
                ed.enter_mode = false;
                ed.point = None;
                ed.input.insert("\n");
                self.refresh_complete(false);
            }
            Key::Enter if ctrl => self.commit_edit_all(),
            Key::Enter => mv(self, if shift { -1 } else { 1 }, 0),
            Key::Char(c) if ctrl && matches!(c.as_str(), ";" | ":") => self.insert_now(shift),
            Key::Tab => mv(self, 0, if shift { -1 } else { 1 }),
            Key::ArrowUp if ed.enter_mode => mv(self, -1, 0),
            Key::ArrowDown if ed.enter_mode => mv(self, 1, 0),
            Key::ArrowLeft if ed.enter_mode => mv(self, 0, -1),
            Key::ArrowRight if ed.enter_mode => mv(self, 0, 1),
            Key::F(2) => self.toggle_pointing(),
            // Várias linhas: as setas sobem e descem no texto.
            Key::ArrowUp | Key::ArrowDown if ed.wrap || ed.input.text.contains('\n') => {
                let down = *key == Key::ArrowDown;
                self.editor_line(down, shift);
            }
            _ => {
                let r = ed.input.key(key, mods, text, clipboard);
                if r.handled {
                    ed.point = None;
                    self.refresh_complete(r.changed);
                }
                return Some(r.handled || !(mods.ctrl || mods.logo));
            }
        }
        Some(true)
    }

    /// Sobe ou desce uma linha no editor.
    fn editor_line(&mut self, down: bool, extend: bool) {
        let g = self.geom();
        let Some(mut ed) = self.editor.take() else { return };
        if let (Some((_, v)), Some(text)) = (self.doc.as_ref().and_then(|d| d.current()), self.text.as_mut())
        {
            let ui = text.default_style();
            let (r, style, pad) = crate::complete::editor_rect(&ed, &g, v, text, ui);
            let lines = if ed.in_bar {
                ed.input.lines(None, text, style)
            } else {
                crate::complete::editor_lines(&ed, r, pad, text, style).unwrap_or_default()
            };
            if !ed.input.move_line(&lines, down, extend, text, style) {
                let to = if down { ed.input.text.len() } else { 0 };
                if extend {
                    let a = ed.input.selection().map_or(ed.input.cursor(), |s| if down { s.0 } else { s.1 });
                    ed.input.select(a, to);
                } else {
                    ed.input.set_cursor(to);
                }
            }
        }
        ed.point = None;
        self.editor = Some(ed);
        self.refresh_complete(false);
    }

    /// Clique dentro do editor (na célula ou na barra aberta): põe o cursor.
    /// `true` = era nele.
    pub fn editor_place(&mut self, g: &crate::app::Geom, x: f32, y: f32, extend: bool) -> bool {
        let Some(mut ed) = self.editor.take() else { return false };
        let mut hit = false;
        if let (Some((_, v)), Some(text)) = (self.doc.as_ref().and_then(|d| d.current()), self.text.as_mut())
        {
            let ui = text.default_style();
            let (r, style, pad) = crate::complete::editor_rect(&ed, g, v, text, ui);
            if r.contains(x, y) {
                hit = true;
                let lines = if ed.in_bar {
                    Some(ed.input.lines(None, text, style)).filter(|l| l.len() > 1)
                } else {
                    crate::complete::editor_lines(&ed, r, pad, text, style)
                };
                let at = match lines {
                    Some(l) => ed.input.hit(&l, r, pad, x, y, text, style),
                    None => {
                        // Uma linha, rolada até o cursor.
                        let caret = text.measure_styled(&ed.input.text[..ed.input.cursor()], style);
                        let shift = (caret - (r.w() - 2.0 * pad)).max(0.0);
                        let one = [(0, ed.input.text.len())];
                        ed.input.hit(&one, r, pad - shift, x, r.y0 + 2.0, text, style)
                    }
                };
                if extend {
                    let a = ed.input.selection().map_or(ed.input.cursor(), |s| s.0);
                    ed.input.select(a, at);
                } else {
                    ed.input.set_cursor(at);
                }
                ed.enter_mode = false;
                ed.point = None;
            }
        }
        self.editor = Some(ed);
        if hit {
            self.refresh_complete(false);
        }
        hit
    }

    /// Altura das linhas sem altura própria com texto quebrado: cresce para
    /// caber (só na tela, como a altura automática do Excel).
    fn auto_rows(&mut self, idx: usize, rows: &[u32]) {
        const MAX_COLS: u32 = 200;
        let Some(doc) = &mut self.doc else { return };
        let (looks, strings) = (doc.looks.clone(), doc.strings.clone());
        let Some(slot) = doc.sheets.get_mut(idx) else { return };
        let (Some(sheet), Some(v)) = (slot.data.as_ref(), slot.view.as_mut()) else { return };
        let Some(text) = self.text.as_mut() else { return };
        for &r in rows {
            if r >= v.rows.len() || sheet.row_height(r).is_some() || sheet.row_hidden(r) {
                continue;
            }
            let mut px = v.rows.default_size();
            for c in 0..sheet.cols().min(MAX_COLS) {
                let cell = sheet.cell(r, c, &strings);
                let Value::Text(t) = cell.value else { continue };
                let look = looks.get(if cell.style == 0 { v.blank_style(r, c) } else { cell.style });
                if !look.wrap || v.merges.at(r, c).is_some() || v.cols.size(c) == 0 {
                    continue;
                }
                let f = &looks.fonts[(look.font as usize).min(looks.fonts.len() - 1)];
                let st = text.style(&f.name, f.pt * 96.0 / 72.0, f.bold, f.italic);
                let w = text.wrap(t, st, (v.cols.size(c) as f32 - 6.0).max(1.0));
                px = px.max((w.lines() as f32 * w.line_height()).ceil() as u32 + 2);
            }
            if v.rows.size(r) != px {
                v.resize_row(r, px);
            }
        }
    }

    /// `--edit`, `--undo` e `--save-to` (testes sem mexer na janela).
    pub fn scripted_edits(&mut self) {
        let edits = std::mem::take(&mut self.opts.edits);
        let mut clipboard = String::new();
        for e in &edits {
            if self.scripted_tabs(e)
                || self.scripted_fill(e)
                || self.scripted_object(e)
                || self.scripted_cells(e)
                || self.scripted_replace(e)
                || self.scripted_format(e)
                || self.scripted_paste(e, &mut clipboard)
            {
                continue;
            }
            // `formatar:negrito:A1:B2` (ver `format::by_name`).
            if let Some(rest) = e.strip_prefix("formatar:")
                && let Some((name, range)) = rest.split_once(':')
            {
                let (Some(f), Some(((r0, c0), (r1, c1)))) =
                    (crate::format::by_name(name), crate::app::parse_range(range))
                else {
                    eprintln!("--edit: formato ou intervalo inválido: {rest}");
                    continue;
                };
                self.go_to(r0, c0);
                if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                    v.anchor = (r0, c0);
                    v.cur = (r1, c1);
                }
                self.apply_fmt(f);
                continue;
            }
            // `filtro:A1` (liga/desliga), `classificar:B2`, `classificar-desc:B2`,
            // `filtrar=SP|RJ:B5` (critério da coluna de B5, no filtro que a contém).
            if let Some((op, arg)) = e.split_once(':')
                && (matches!(op, "filtro" | "classificar" | "classificar-desc") || op.starts_with("filtrar="))
            {
                let Some(((r0, c0), (r1, c1))) = crate::app::parse_range(arg) else { continue };
                self.go_to(r0, c0);
                if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                    v.anchor = (r1, c1);
                    v.cur = (r0, c0);
                }
                match op {
                    "filtro" => self.toggle_filter(),
                    "classificar" => self.sort_here(false),
                    "classificar-desc" => self.sort_here(true),
                    o => {
                        let want: Vec<String> =
                            o["filtrar=".len()..].split('|').map(|s| s.to_lowercase()).collect();
                        let g = self.geom();
                        let found = self.doc.as_ref().and_then(|d| d.current()).and_then(|(s, _)| {
                            crate::filter::filters(s).into_iter().find(|f| f.filter.range.contains(r0, c0))
                        });
                        if let Some(f) = found {
                            self.open_filter_panel(f, c0, duck_ui::Rect::new(0.0, 0.0, g.w, g.grid.y0));
                            if let Some(p) = &mut self.filter_panel {
                                for it in &mut p.items {
                                    it.checked = want.contains(&it.text.to_lowercase());
                                }
                                p.blank = p.blank.map(|_| want.iter().any(|w| w == "(vazias)"));
                            }
                            self.apply_filter_panel();
                        }
                    }
                }
                continue;
            }
            // `mesclar:A1:B2`, `ocultar-linhas:A3:A4`, `congelar:B2`, `largura=12:C1`...
            if let Some((op, arg)) = e.split_once(':')
                && (matches!(
                    op,
                    "mesclar"
                        | "mesclar-centralizar"
                        | "mesclar-atraves"
                        | "desmesclar"
                        | "ocultar-linhas"
                        | "ocultar-colunas"
                        | "reexibir-linhas"
                        | "reexibir-colunas"
                        | "congelar"
                        | "descongelar"
                        | "limpar-formatos"
                        | "limpar-tudo"
                ) || op.starts_with("largura=")
                    || op.starts_with("altura="))
            {
                use crate::ribbon::{Clear, Freeze, Merge};
                let Some(((r0, c0), (r1, c1))) = crate::app::parse_range(arg) else { continue };
                self.go_to(r0, c0);
                if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                    v.anchor = (r0, c0);
                    v.cur = (r1, c1);
                }
                if op == "congelar"
                    && let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut())
                {
                    v.anchor = (r0, c0);
                    v.cur = (r0, c0);
                }
                let num =
                    |s: &str| s.split_once('=').and_then(|(_, v)| v.replace(',', ".").parse::<f32>().ok());
                match op {
                    "mesclar" => self.merge(Merge::Cells),
                    "mesclar-centralizar" => self.merge(Merge::Center),
                    "mesclar-atraves" => self.merge(Merge::Across),
                    "desmesclar" => self.merge(Merge::Unmerge),
                    "ocultar-linhas" => self.hide(true, true),
                    "ocultar-colunas" => self.hide(false, true),
                    "reexibir-linhas" => self.hide(true, false),
                    "reexibir-colunas" => self.hide(false, false),
                    "congelar" => self.freeze(Freeze::Here),
                    "descongelar" => self.freeze(Freeze::Off),
                    "limpar-formatos" => self.clear(Clear::Formats),
                    "limpar-tudo" => self.clear(Clear::All),
                    o if o.starts_with("largura=") => {
                        if let Some(w) = num(o) {
                            self.run_command(Command::ColWidths((c0..=c1).map(|c| (c, Some(w))).collect()));
                        }
                    }
                    o => {
                        if let Some(h) = num(o) {
                            self.run_command(Command::RowHeights((r0..=r1).map(|r| (r, Some(h))).collect()));
                        }
                    }
                }
                continue;
            }
            // `inserir:A5:A6`, `apagar:A5`, `inserir-colunas:C1:D1`, `apagar-colunas:C1`.
            if let Some((op, arg)) = e.split_once(':')
                && matches!(op, "inserir" | "apagar" | "inserir-colunas" | "apagar-colunas")
            {
                let Some(((r0, c0), (r1, c1))) = crate::app::parse_range(arg) else { continue };
                self.go_to(r0, c0);
                if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                    v.anchor = (r0, c0);
                    v.cur = (r1, c1);
                }
                let axis =
                    if op.ends_with("colunas") { duck_core::Axis::Cols } else { duck_core::Axis::Rows };
                self.shift_selection(op.starts_with("inserir"), Some(axis));
                eprintln!("--edit {e}: {}", self.message);
                continue;
            }

            // `objeto=N` clica no N-ésimo objeto clicável da aba (link, controle
            // ou macro); `macro=Nome` executa uma macro.
            if let Some(n) = e.strip_prefix("macro=") {
                self.run_macro(n);
                eprintln!("--macro: {}", if self.message.starts_with("Erro") { "erro" } else { "ok" });
                continue;
            }
            if let Some(n) = e.strip_prefix("objeto=") {
                let n: usize = n.parse().unwrap_or(0);
                let obj = self.doc.as_ref().and_then(|d| d.current()).and_then(|(s, _)| {
                    let hit = |o: &duck_core::drawing::Object| {
                        o.link.is_some()
                            || o.macro_name.is_some()
                            || matches!(o.kind, duck_core::drawing::ObjectKind::Control(_))
                    };
                    s.layout.objects.iter().enumerate().filter(|(_, o)| hit(o)).nth(n).map(|(k, _)| k)
                });
                match obj {
                    Some(k) => {
                        self.object_run(k);
                        let cells: usize = self
                            .doc
                            .as_ref()
                            .and_then(|d| d.history.undo.last())
                            .map_or(0, |s| s.iter().map(|(_, c)| c.cells().len()).sum());
                        let state = if self.message.starts_with("Erro") { "erro" } else { "ok" };
                        eprintln!("--objeto: {state} ({cells} células)");
                    }
                    None => eprintln!("--edit: objeto {n} não encontrado"),
                }
                continue;
            }
            // `grupo=linhas:N` / `grupo=colunas:N` abre ou fecha o N-ésimo grupo;
            // `nivel=linhas:N` mostra até o nível N.
            if let Some((op, arg)) = e.split_once('=')
                && matches!(op, "grupo" | "nivel")
                && let Some((axis, n)) = arg.split_once(':')
            {
                let rows = axis == "linhas";
                let n: usize = n.parse().unwrap_or(0);
                let cmd = self.doc.as_ref().and_then(|d| d.current()).and_then(|(s, v)| {
                    if op == "nivel" {
                        return crate::outline::show_level(s, rows, n as u8);
                    }
                    let groups = if rows { &v.row_groups } else { &v.col_groups };
                    crate::outline::toggle(s, rows, *groups.get(n)?)
                });
                match cmd {
                    Some(c) => {
                        self.run_command(c);
                    }
                    None => eprintln!("--edit: grupo {n} não encontrado"),
                }
                continue;
            }
            // `segmentacao=N:I` (item I da N-ésima segmentação da aba; `limpar`),
            // `segmentacao=N:I+` soma o item (Ctrl+clique). `linhadotempo=N:I`
            // escolhe o período I (`I+` estende; `limpar`).
            let target = e
                .strip_prefix("segmentacao=")
                .map(|a| (a, true))
                .or_else(|| e.strip_prefix("linhadotempo=").map(|a| (a, false)));
            if let Some((arg, slicer)) = target
                && let Some((n, i)) = arg.split_once(':')
            {
                use duck_core::drawing::ObjectKind;
                let n: usize = n.parse().unwrap_or(0);
                let obj = self.doc.as_ref().and_then(|d| d.current()).and_then(|(s, _)| {
                    s.layout
                        .objects
                        .iter()
                        .enumerate()
                        .filter(|(_, o)| match o.kind {
                            ObjectKind::Slicer(_) => slicer,
                            ObjectKind::Timeline(_) => !slicer,
                            _ => false,
                        })
                        .nth(n)
                        .map(|(k, _)| k)
                });
                let Some(obj) = obj else {
                    eprintln!("--edit: objeto {n} não encontrado");
                    continue;
                };
                let ctrl = i.ends_with('+');
                let item = if i == "limpar" { None } else { i.trim_end_matches('+').parse().ok() };
                let cube = self.doc.as_ref().and_then(|d| d.current()).is_some_and(|(s, _)| {
                    matches!(&s.layout.objects[obj].kind,
                        ObjectKind::Slicer(x) if x.pivot.as_ref().is_some_and(|p| p.is_cube()))
                });
                let t0 = std::time::Instant::now();
                self.slicer_click(obj, item, ctrl);
                if slicer {
                    let what = if cube { "modelo" } else { "comum" };
                    let ms = t0.elapsed().as_millis();
                    let m = if self.message.starts_with("Filtro") { "filtro" } else { &self.message };
                    eprintln!("--segmentacao: {what}, {ms} ms: {m}");
                }
                continue;
            }
            // `A1=texto` (`\n` no texto é quebra de linha).
            let Some((cell, text)) = e.split_once('=') else { continue };
            let Some((r, c)) = crate::app::parse_cell(cell.trim()) else {
                eprintln!("--edit: referência inválida: {cell}");
                continue;
            };
            self.go_to(r, c);
            self.start_edit(Some(&text.replace("\\n", "\n")), false);
            self.commit_edit();
        }
        for _ in 0..std::mem::take(&mut self.opts.undo) {
            self.undo();
        }
        self.scripted_ui();
        if let Some(dest) = self.opts.save_to.clone() {
            // Os valores calculados entram no arquivo salvo.
            self.calc_finish();
            self.save_to(dest);
        }
    }

    // --- salvar ---------------------------------------------------------------

    /// Ctrl+S (`save_as = false`) e Ctrl+Shift+S. Salvar sem mudanças no
    /// mesmo arquivo não faz nada.
    pub fn save(&mut self, save_as: bool) {
        self.commit_edit();
        if self.saving.is_some() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        if doc.finished.is_none() {
            self.message = "Aguarde terminar de carregar para salvar".into();
            return;
        }
        let src = doc.path.clone();
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("xlsx").to_ascii_lowercase();
        // Planilha nova: a primeira vez pede o lugar.
        let untitled = self.untitled_path.as_deref() == Some(src.as_path());
        let dest = if save_as || untitled {
            let Some(w) = &self.window else { return };
            let name = if untitled {
                "Planilha.xlsx".to_string()
            } else {
                src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            };
            let dir = if untitled { None } else { src.parent() };
            let filter: &[(&str, &[&str])] = &[("Mesmo formato", &[ext.as_str()])];
            let Some(mut d) = w.save_file("Salvar como", dir, &name, filter) else { return };
            if d.extension().is_none() {
                d.set_extension(&ext);
            }
            if d.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref()
                != Some(ext.as_str())
            {
                self.message = format!("Salve com a mesma extensão (.{ext})");
                return;
            }
            d
        } else {
            if !doc.dirty {
                self.message = "Nada a salvar".into();
                return;
            }
            src.clone()
        };
        self.save_to(dest);
    }

    /// Salva em `dest` (o próprio arquivo ou uma cópia).
    pub fn save_to(&mut self, dest: PathBuf) {
        self.commit_edit();
        if let Some(doc) = &mut self.doc {
            doc.calc.cancel();
        }
        let Some(doc) = &self.doc else { return };
        if self.saving.is_some() || doc.finished.is_none() {
            return;
        }
        let src = doc.path.clone();
        if crate::loader::is_csv(&src) {
            self.save_csv(&src, dest);
            return;
        }
        let sheets: Vec<Option<Arc<Sheet>>> =
            doc.sheets.iter().map(|s| if s.preview { None } else { s.data.clone() }).collect();
        let (strings, styles) = (doc.strings.clone(), doc.styles.clone());
        let tabs = Some(crate::tabs::plan(doc));
        let (tx, rx) = channel();
        let Some(waker) = self.waker.clone() else { return };
        let d = dest.clone();
        std::thread::Builder::new()
            .name("duck-save".into())
            .spawn(move || {
                let pool = duck_par::pool(0);
                let r = pool.install(|| {
                    let refs: Vec<Option<&Sheet>> = sheets.iter().map(|s| s.as_deref()).collect();
                    duck_xlsx::save_file(
                        &src,
                        &d,
                        &refs,
                        &strings,
                        &styles,
                        &duck_xlsx::SaveOptions { tabs, ..Default::default() },
                    )
                });
                // As abas voltam a ter um dono só antes de avisar a janela.
                drop(sheets);
                let _ = tx.send(r.map_err(|e| e.to_string()));
                waker.wake();
            })
            .expect("thread de salvar");
        self.message = "Salvando…".into();
        self.saving = Some(SaveJob { rx, dest, started: Instant::now() });
    }

    fn save_csv(&mut self, src: &std::path::Path, dest: PathBuf) {
        let Some(doc) = &mut self.doc else { return };
        let fmt = std::fs::read(src).map(|b| duck_xlsx::csv::detect_format(&b)).unwrap_or_default();
        let Some(sheet) = doc.sheets.first().and_then(|s| s.data.as_deref()) else { return };
        let t = Instant::now();
        match duck_xlsx::csv::write_csv(&dest, sheet, &doc.strings, fmt) {
            Ok(()) => {
                if let Some(s) = doc.sheets.first_mut().and_then(sheet_mut) {
                    s.clear_edits();
                }
                doc.path = dest;
                doc.dirty = false;
                self.message = format!("Salvo em {} ms (CSV: só os valores)", t.elapsed().as_millis());
                self.update_title();
            }
            Err(e) => duck_platform::message_box("Não foi possível salvar", &e.to_string()),
        }
    }

    /// Resultado do salvamento, se chegou. `block` espera por ele.
    pub fn poll_save(&mut self, block: bool) -> Option<bool> {
        let job = self.saving.as_ref()?;
        let r = if block { job.rx.recv().ok()? } else { job.rx.try_recv().ok()? };
        let job = self.saving.take()?;
        let ok = match r {
            Ok(rep) => {
                if let Some(doc) = &mut self.doc {
                    for slot in &mut doc.sheets {
                        if let Some(s) = sheet_mut(slot) {
                            s.clear_edits();
                        }
                    }
                    // Os estilos novos agora estão no arquivo.
                    let st = Arc::make_mut(&mut doc.styles);
                    st.file_counts = duck_core::FileCounts {
                        xfs: st.xfs.len(),
                        fonts: st.fonts.len(),
                        fills: st.fills.len(),
                        borders: st.borders.len(),
                    };
                    st.xf_base.clear();
                    st.new_num_fmts.clear();
                    doc.path = job.dest.clone();
                    doc.dirty = false;
                    crate::tabs::saved(doc, &rep);
                }
                self.remember_recent(&job.dest);
                self.message = format!(
                    "Salvo em {} ms ({} abas regravadas, conferido)",
                    job.started.elapsed().as_millis(),
                    rep.sheets_rewritten.len()
                );
                self.update_title();
                self.calc_kick();
                true
            }
            Err(e) => {
                self.message = "Erro ao salvar: nada foi alterado no arquivo".into();
                if self.opts.save_to.is_some() {
                    // Execução de teste: sem caixa de diálogo (ela esperaria um clique).
                    eprintln!("Não foi possível salvar: {e}");
                } else {
                    duck_platform::message_box("Não foi possível salvar", &e);
                }
                false
            }
        };
        Some(ok)
    }

    /// Antes de fechar ou abrir outro arquivo: pergunta se há alterações.
    /// `true` = pode seguir.
    pub fn confirm_discard(&mut self) -> bool {
        self.commit_edit();
        if self.saving.is_some() {
            self.poll_save(true);
        }
        let dirty = self.doc.as_ref().is_some_and(|d| d.dirty);
        if !dirty {
            return true;
        }
        let Some(w) = &self.window else { return true };
        let name = self.doc.as_ref().map(|d| self.file_label(&d.path)).unwrap_or_default();
        match w.ask_save("duck-sheets", &format!("Salvar as alterações em {name}?")) {
            SaveChoice::Discard => true,
            SaveChoice::Cancel => false,
            SaveChoice::Save => {
                self.save(false);
                if self.saving.is_some() {
                    self.poll_save(true).unwrap_or(false)
                } else {
                    !self.doc.as_ref().is_some_and(|d| d.dirty)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Inserir e apagar linhas e colunas (F3.5)
// ---------------------------------------------------------------------------

/// Fórmulas de outra aba que apontam para `name` e mudam com `s`.
fn other_sheet_changes(
    sheet: &Sheet,
    strings: &duck_core::StringTable,
    name: &str,
    s: duck_core::Shift,
) -> Vec<CellChange> {
    let lname = name.to_lowercase();
    let mut mentions: std::collections::HashMap<usize, bool> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for (r, c) in sheet.formula_positions() {
        let Some(f) = sheet.formula(r, c) else { continue };
        let hit = *mentions
            .entry(f.text.as_ptr() as usize)
            .or_insert_with(|| f.text.to_lowercase().contains(&lname));
        if !hit {
            continue;
        }
        let Some(a1) = duck_calc::cell_formula(sheet, r, c) else { continue };
        if let Some((new, _)) = duck_calc::adjust_refs(&a1, s, false, name) {
            let value = match sheet.cell(r, c, strings).value {
                Value::Empty => CellValue::Empty,
                Value::Number(x) => CellValue::Number(x),
                Value::Text(t) => CellValue::Text(t.to_string()),
                Value::Bool(b) => CellValue::Bool(b),
                Value::Error(e) => CellValue::Error(e),
            };
            out.push(CellChange { row: r, col: c, value, formula: Some(new), style: None });
        }
    }
    out
}

impl SheetsApp {
    /// Insere (acima/à esquerda) ou apaga as linhas ou colunas da seleção.
    /// `axis = None`: colunas se a seleção for de colunas inteiras, senão linhas.
    pub fn shift_selection(&mut self, insert: bool, axis: Option<duck_core::Axis>) {
        use duck_core::{Axis, OpsHandle, Shift};
        self.commit_edit();
        if !self.editable() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r0, c0, r1, c1) = v.sel();
        let whole_cols = r1 - r0 + 1 == v.rows.len() && c1 - c0 + 1 < v.cols.len();
        let axis = axis.unwrap_or(if whole_cols { Axis::Cols } else { Axis::Rows });
        let (at, n, used, max) = match axis {
            Axis::Rows => (r0, r1 - r0 + 1, sheet.rows(), crate::app::MAX_ROWS),
            Axis::Cols => (c0, c1 - c0 + 1, sheet.cols(), crate::app::MAX_COLS),
        };
        if insert && used as u64 + n as u64 > max as u64 {
            self.message = "Não cabe: a planilha chegaria ao limite de linhas/colunas".into();
            return;
        }
        let slot = doc.slot();
        let shift = Shift { axis, at, n, insert };
        // Como o Excel: não dá para inserir/apagar no meio de uma tabela dinâmica.
        let cuts = |r: &duck_core::CellRange| {
            let (lo, hi) = match axis {
                Axis::Rows => (r.first_row, r.last_row),
                Axis::Cols => (r.first_col, r.last_col),
            };
            if insert { at > lo && at <= hi } else { at <= hi && at + n > lo }
        };
        if sheet.layout.pivots.iter().any(|p| cuts(&p.range)) {
            self.message = "Não é possível mudar parte de uma tabela dinâmica".into();
            return;
        }
        let name = slot.info.name.clone();
        let ops = OpsHandle(Arc::new(duck_calc::ShiftOps { shift, sheet: name.clone(), own: true }));
        let mut step = vec![(doc.active, Command::Shift(shift, ops))];
        for (i, other) in doc.sheets.iter().enumerate() {
            if i == doc.active || other.preview {
                continue;
            }
            let Some(data) = &other.data else { continue };
            let changes = other_sheet_changes(data, &doc.strings, &name, shift);
            let mut cmds = Vec::new();
            if !changes.is_empty() {
                cmds.push(Command::SetCells(changes));
            }
            // Gráficos desta aba com séries na aba mudada.
            let mut layout = data.layout.clone();
            for o in &mut layout.objects {
                if let duck_core::drawing::ObjectKind::Chart(c) = &mut o.kind {
                    for r in c.refs_mut() {
                        if let Some((new, _)) = duck_calc::adjust_refs(r, shift, false, &name) {
                            *r = new;
                        }
                    }
                }
            }
            if layout != data.layout {
                cmds.push(Command::Layout(Box::new(layout)));
            }
            if !cmds.is_empty() {
                step.push((i, Command::Group(cmds)));
            }
        }
        let names = duck_calc::tabs::shift_names(&doc.calc.extras, shift, &name);
        if names.names != doc.calc.extras.names {
            step.push((doc.active, crate::tabs::book(crate::tabs::TabOp::Refs(names))));
        }
        let t = Instant::now();
        if self.run_step(step) {
            let what = match (axis, n) {
                (Axis::Rows, 1) => "1 linha".to_string(),
                (Axis::Rows, n) => format!("{n} linhas"),
                (Axis::Cols, 1) => "1 coluna".to_string(),
                (Axis::Cols, n) => format!("{n} colunas"),
            };
            let verb = if insert {
                if n == 1 { "inserida" } else { "inseridas" }
            } else if n == 1 {
                "apagada"
            } else {
                "apagadas"
            };
            self.message = format!("{what} {verb} ({} ms)", t.elapsed().as_millis());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_round_trip_in_pixels() {
        for mdw in [6.0f32, 7.0, 8.0, 9.0, 11.0] {
            for px in 1..400u32 {
                assert_eq!(col_px(px_to_width(px, mdw), mdw), px, "mdw {mdw}, px {px}");
            }
        }
    }
}
