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
use duck_platform::{Context, Key, Modifiers, SaveChoice};
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

/// O comando muda a estrutura (linhas/colunas inseridas ou apagadas)?
fn structural(cmd: &Command) -> bool {
    match cmd {
        Command::Shift(..) => true,
        Command::Group(v) => v.iter().any(structural),
        _ => false,
    }
}

/// A aba para escrita. A busca pode estar terminando numa thread com uma
/// cópia do `Arc`: espera um pouco por ela (a busca já foi cancelada).
fn sheet_mut(slot: &mut Slot) -> Option<&mut Sheet> {
    let arc = slot.data.as_mut()?;
    for _ in 0..300 {
        if Arc::strong_count(arc) == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Arc::get_mut(arc)
}

/// O formato sugerido pelo que foi digitado (`new`) substitui o da célula
/// (`current`)? Como no Excel: sempre numa célula "Geral"; datas, horas e
/// percentuais também trocam um formato de outro tipo (uma data digitada
/// numa célula de moeda vira data); os outros só valem em "Geral".
fn should_apply(styles: &duck_core::Styles, current: u16, new: u16) -> bool {
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
    fn editable(&mut self) -> bool {
        if self.saving.is_some() {
            self.message = "Aguarde: salvando…".into();
            return false;
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
        let inverse = cmd.apply(sheet);
        doc.history.undo.push(vec![(idx, inverse)]);
        doc.history.redo.clear();
        doc.dirty = true;
        self.after_change(idx, false);
        true
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
        let mut inverse = Vec::with_capacity(step.len());
        let mut touched = Vec::new();
        for (idx, cmd) in step {
            let st = structural(&cmd);
            let Some(sheet) = doc.sheets.get_mut(idx).and_then(sheet_mut) else { continue };
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
        for (idx, st) in touched {
            self.after_change(idx, st);
        }
        true
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
        if first != doc.active {
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
        let (from, to) = if undo {
            (&mut doc.history.undo, &mut doc.history.redo)
        } else {
            (&mut doc.history.redo, &mut doc.history.undo)
        };
        let Some(step) = from.pop() else { return };
        // Confere antes que todas as abas do passo estão livres.
        if step.iter().any(|(i, _)| doc.sheets.get_mut(*i).and_then(sheet_mut).is_none()) {
            from.push(step);
            self.message = "A aba está em uso; tente de novo".into();
            return;
        }
        let cells = step.first().map(|(_, c)| c.cells()).unwrap_or_default();
        let mut inverse = Vec::with_capacity(step.len());
        let mut touched = Vec::new();
        for (idx, cmd) in step {
            let st = structural(&cmd);
            let sheet = doc.sheets.get_mut(idx).and_then(sheet_mut).expect("aba livre");
            inverse.push((idx, cmd.apply(sheet)));
            touched.push((idx, st));
        }
        inverse.reverse();
        to.push(inverse);
        doc.dirty = true;
        for (idx, st) in touched {
            self.after_change(idx, st);
        }
        // Mostra o que mudou.
        if let Some(&(r, c)) = cells.first() {
            self.go_to(r, c);
        }
        self.message = if undo { "Desfeito" } else { "Refeito" }.into();
    }

    /// Depois de mudar a aba `idx`: tamanhos na tela, formatação condicional
    /// (os dados calculados das regras ficam velhos), estatísticas e título.
    fn after_change(&mut self, idx: usize, structural: bool) {
        self.stats = None;
        let Some(doc) = &mut self.doc else { return };
        let mdw = doc.mdw;
        let slot = &mut doc.sheets[idx];
        if structural {
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
        let e = sheet.edits();
        let def_col = v.cols.default_size();
        let def_row = v.rows.default_size();
        for &c in &e.cols {
            if c < v.cols.len() {
                let px = sheet.col_width(c).map_or(def_col, |w| col_px(w, mdw));
                if v.cols.size(c) != px {
                    v.resize_col(c, px);
                }
            }
        }
        for &r in &e.rows {
            if r < v.rows.len() {
                let px = sheet.row_height(r).map_or(def_row, row_px);
                if v.rows.size(r) != px {
                    v.resize_row(r, px);
                }
            }
        }
        self.update_title();
    }

    pub fn update_title(&self) {
        let (Some(w), Some(doc)) = (&self.window, &self.doc) else { return };
        let name = doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dot = if doc.dirty { "• " } else { "" };
        w.set_title(&format!("{dot}{name} — duck-sheets"));
    }

    /// Largura da coluna como está na tela vira comando (fim do arraste, ajuste).
    pub fn commit_col_size(&mut self, col: u32) {
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let px = v.cols.size(col);
        let w = if px == v.cols.default_size() && sheet.col_width(col).is_none() {
            return;
        } else {
            Some(px_to_width(px, doc.mdw))
        };
        if w != sheet.col_width(col) {
            self.run_command(Command::ColWidths(vec![(col, w)]));
        }
    }

    /// Altura da linha como está na tela vira comando. `reset` = automática.
    pub fn commit_row_size(&mut self, row: u32, reset: bool) {
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let h = if reset { None } else { Some(v.rows.size(row) as f32 * 0.75) };
        if h != sheet.row_height(row) {
            self.run_command(Command::RowHeights(vec![(row, h)]));
        }
    }

    /// Delete: apaga os valores da seleção (o formato fica).
    pub fn clear_selection(&mut self) {
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r0, c0, r1, c1) = v.sel();
        let (r1, c1) = (r1.min(sheet.rows().max(1) - 1), c1.min(sheet.cols().max(1) - 1));
        let mut changes = Vec::new();
        const MAX: usize = 2_000_000;
        for c in c0..=c1 {
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
        self.editor = Some(Editor { row, col, input, enter_mode, in_bar });
    }

    pub fn cancel_edit(&mut self) {
        self.editor = None;
    }

    /// Confirma o que foi digitado. `false` se não havia editor.
    pub fn commit_edit(&mut self) -> bool {
        let Some(ed) = self.editor.take() else { return false };
        let (r, c) = (ed.row, ed.col);
        let text = ed.input.text;
        // Sem mudança (F2 e Enter sem mexer): nada a fazer.
        let mut before = String::new();
        self.formula_text_at(r, c, &mut before);
        if before == text {
            return true;
        }
        let today = self.today;
        let Some(doc) = &mut self.doc else { return true };
        let Some((sheet, v)) = doc.current() else { return true };
        let cell = sheet.cell(r, c, &doc.strings);
        let base = if cell.style == 0 { v.blank_style(r, c) } else { cell.style };
        let change = match parse_input(&text, doc.date1904, duck_core::input::today()) {
            Input::Formula(f) => {
                let cx = CfCtx { sheet, shared: &doc.strings, today };
                let value = cx.eval_formula(&f, r, c).unwrap_or(CellValue::Empty);
                CellChange { row: r, col: c, value, formula: Some(f), style: Some(base) }
            }
            Input::Value(value, fmt) => {
                let mut style = base;
                let current = doc.styles.xfs.get(base as usize).map_or(0, |x| x.num_fmt);
                if let Some(f) = fmt
                    && should_apply(&doc.styles, current, f)
                {
                    let n = doc.styles.xfs.len();
                    style = Arc::make_mut(&mut doc.styles).derive_xf(base, |x| x.num_fmt = f);
                    if doc.styles.xfs.len() != n {
                        doc.looks = Arc::new(Looks::new(&doc.styles));
                        self.font_key = (0, 0);
                    }
                }
                CellChange { row: r, col: c, value, formula: None, style: Some(style) }
            }
        };
        self.run_command(Command::SetCells(vec![change]));
        true
    }

    /// Texto da barra de fórmulas para uma célula (a ativa é o caso comum).
    fn formula_text_at(&self, r: u32, c: u32, out: &mut String) {
        out.clear();
        let Some(doc) = &self.doc else { return };
        let Some((sheet, _)) = doc.current() else { return };
        if let Some(f) = duck_calc::cell_formula(sheet, r, c) {
            out.push('=');
            out.push_str(&f);
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
        cx: &mut Context,
        key: &Key,
        mods: Modifiers,
        text: Option<&str>,
    ) -> Option<bool> {
        let ed = self.editor.as_mut()?;
        let shift = mods.shift;
        let mv = |app: &mut Self, dr: i32, dc: i32| {
            app.commit_edit();
            app.move_cur(dr, dc, false, false);
        };
        match key {
            Key::Escape => self.cancel_edit(),
            Key::Enter => mv(self, if shift { -1 } else { 1 }, 0),
            Key::Tab => mv(self, 0, if shift { -1 } else { 1 }),
            Key::ArrowUp if ed.enter_mode => mv(self, -1, 0),
            Key::ArrowDown if ed.enter_mode => mv(self, 1, 0),
            Key::ArrowLeft if ed.enter_mode => mv(self, 0, -1),
            Key::ArrowRight if ed.enter_mode => mv(self, 0, 1),
            Key::F(2) => ed.enter_mode = !ed.enter_mode,
            _ => {
                let r = ed.input.key(key, mods, text, cx.clipboard());
                return Some(r.handled || !(mods.ctrl || mods.logo));
            }
        }
        Some(true)
    }

    /// `--edit`, `--undo` e `--save-to` (testes sem mexer na janela).
    pub fn scripted_edits(&mut self) {
        let edits = std::mem::take(&mut self.opts.edits);
        let mut clipboard = String::new();
        for e in &edits {
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
                continue;
            }
            // `copiar:A1:B2`, `recortar:A1:B2`, `colar:D5` (clipboard interno).
            if let Some((op, arg)) = e.split_once(':')
                && matches!(op, "copiar" | "recortar" | "colar")
            {
                let Some(((r0, c0), (r1, c1))) = crate::app::parse_range(arg) else {
                    eprintln!("--edit: intervalo inválido: {arg}");
                    continue;
                };
                self.go_to(r0, c0);
                if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                    v.anchor = (r0, c0);
                    v.cur = (r1, c1);
                }
                match op {
                    "colar" => self.paste_text(clipboard.clone()),
                    _ => clipboard = self.copy_text(op == "recortar").unwrap_or_default(),
                }
                continue;
            }
            let Some((cell, text)) = e.split_once('=') else { continue };
            let Some((r, c)) = crate::app::parse_cell(cell.trim()) else {
                eprintln!("--edit: referência inválida: {cell}");
                continue;
            };
            self.go_to(r, c);
            self.start_edit(Some(text), false);
            self.commit_edit();
        }
        for _ in 0..std::mem::take(&mut self.opts.undo) {
            self.undo();
        }
        if let Some(dest) = self.opts.save_to.clone() {
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
        let dest = if save_as {
            let Some(w) = &self.window else { return };
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let filter: &[(&str, &[&str])] = &[("Mesmo formato", &[ext.as_str()])];
            let Some(mut d) = w.save_file("Salvar como", src.parent(), &name, filter) else { return };
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
                        &duck_xlsx::SaveOptions::default(),
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
                    doc.path = job.dest;
                    doc.dirty = false;
                }
                self.message = format!(
                    "Salvo em {} ms ({} abas regravadas, conferido)",
                    job.started.elapsed().as_millis(),
                    rep.sheets_rewritten.len()
                );
                self.update_title();
                true
            }
            Err(e) => {
                self.message = "Erro ao salvar: nada foi alterado no arquivo".into();
                duck_platform::message_box("Não foi possível salvar", &e);
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
        let name = self
            .doc
            .as_ref()
            .and_then(|d| d.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
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
        if slot.info.objects && !self.objects_ok {
            let Some(w) = &self.window else { return };
            let ok = w.confirm(
                "duck-sheets",
                "Esta aba tem gráficos, imagens, tabelas ou tabelas dinâmicas. O duck-sheets ainda não \
                 ajusta a posição deles ao inserir ou apagar linhas e colunas. Continuar?",
            );
            if !ok {
                return;
            }
            self.objects_ok = true;
        }
        let shift = Shift { axis, at, n, insert };
        let name = slot.info.name.clone();
        let ops = OpsHandle(Arc::new(duck_calc::ShiftOps { shift, sheet: name.clone(), own: true }));
        let mut step = vec![(doc.active, Command::Shift(shift, ops))];
        for (i, other) in doc.sheets.iter().enumerate() {
            if i == doc.active || other.preview {
                continue;
            }
            let Some(data) = &other.data else { continue };
            let changes = other_sheet_changes(data, &doc.strings, &name, shift);
            if !changes.is_empty() {
                step.push((i, Command::SetCells(changes)));
            }
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

// ---------------------------------------------------------------------------
// Copiar, recortar e colar (F3.3)
// ---------------------------------------------------------------------------

/// O que foi copiado dentro do app: além do texto (TSV) que foi para o
/// clipboard do sistema, as células como estão (fórmulas e estilos).
pub struct Clip {
    /// O texto posto no clipboard: se ainda for o mesmo ao colar, a cópia
    /// veio daqui e as fórmulas e estilos valem.
    pub text: String,
    pub sheet: usize,
    pub r0: u32,
    pub c0: u32,
    pub rows: u32,
    pub cols: u32,
    /// Linha a linha.
    pub cells: Vec<ClipCell>,
    /// Recortar: a origem é apagada ao colar (uma vez).
    pub cut: bool,
}

pub struct ClipCell {
    pub value: CellValue,
    pub formula: Option<String>,
    pub style: u16,
}

/// Máximo de células copiadas com fórmulas e estilos (acima disso, só o texto).
const MAX_CLIP: u64 = 1_000_000;
/// Máximo de células coladas de uma vez.
const MAX_PASTE: u64 = 2_000_000;

/// Lê o TSV do clipboard (como o Excel e o Google Planilhas gravam: campos
/// com tab, quebra ou aspas vêm entre aspas, com aspas dobradas).
pub fn parse_tsv(s: &str) -> Vec<Vec<String>> {
    let mut rows = vec![Vec::new()];
    let mut field = String::new();
    let mut chars = s.chars().peekable();
    let mut at_start = true;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if at_start => {
                // Campo entre aspas.
                while let Some(q) = chars.next() {
                    if q == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            field.push('"');
                        } else {
                            break;
                        }
                    } else {
                        field.push(q);
                    }
                }
                at_start = false;
            }
            '\t' => {
                rows.last_mut().unwrap().push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                rows.last_mut().unwrap().push(std::mem::take(&mut field));
                rows.push(Vec::new());
                at_start = true;
            }
            c => {
                field.push(c);
                at_start = false;
            }
        }
    }
    if !field.is_empty() || !rows.last().unwrap().is_empty() {
        rows.last_mut().unwrap().push(field);
    } else {
        rows.pop();
    }
    rows
}

impl SheetsApp {
    /// Ctrl+C / Ctrl+X: guarda as células copiadas (o texto já foi para o
    /// clipboard por `copy`).
    pub fn remember_clip(&mut self, text: String, cut: bool) {
        self.clip = None;
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r0, c0, r1, c1) = v.sel();
        let (r1, c1) =
            (r1.min(sheet.rows().max(r0 + 1) - 1).max(r0), c1.min(sheet.cols().max(c0 + 1) - 1).max(c0));
        let (rows, cols) = (r1 - r0 + 1, c1 - c0 + 1);
        if rows as u64 * cols as u64 > MAX_CLIP {
            return;
        }
        let mut cells = Vec::with_capacity((rows * cols) as usize);
        for r in r0..=r1 {
            for c in c0..=c1 {
                let cell = sheet.cell(r, c, &doc.strings);
                let value = match cell.value {
                    Value::Empty => CellValue::Empty,
                    Value::Number(x) => CellValue::Number(x),
                    Value::Text(t) => CellValue::Text(t.to_string()),
                    Value::Bool(b) => CellValue::Bool(b),
                    Value::Error(e) => CellValue::Error(e),
                };
                let formula = duck_calc::cell_formula(sheet, r, c).map(|f| f.into_owned());
                let style = if cell.style == 0 { v.blank_style(r, c) } else { cell.style };
                cells.push(ClipCell { value, formula, style });
            }
        }
        self.clip = Some(Clip { text, sheet: doc.active, r0, c0, rows, cols, cells, cut });
        if cut {
            self.message = "Recortado: cole com Ctrl+V".into();
        }
    }

    /// Ctrl+V: cola na seleção. Uma cópia feita aqui leva fórmulas (com as
    /// referências deslocadas) e estilos; texto de outro programa é
    /// interpretado como se digitado. Um bloco menor que a seleção é
    /// repetido se couber um número inteiro de vezes (como no Excel).
    pub fn paste(&mut self, cx: &mut Context) {
        if let Some(text) = cx.clipboard().get_text() {
            self.paste_text(text);
        }
    }

    /// Cola `text` (o conteúdo do clipboard; ver [`Self::paste`]).
    pub fn paste_text(&mut self, text: String) {
        self.commit_edit();
        if !self.editable() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let internal = self.clip.as_ref().filter(|c| c.text == text);
        // Bloco a colar: (linhas, colunas) e uma função que dá cada célula.
        let mut tsv = Vec::new();
        let (rows, cols) = match internal {
            Some(c) => (c.rows, c.cols),
            None => {
                tsv = parse_tsv(&text);
                let cols = tsv.iter().map(Vec::len).max().unwrap_or(0) as u32;
                (tsv.len() as u32, cols)
            }
        };
        if rows == 0 || cols == 0 {
            return;
        }
        let (s0, s1, s2, s3) = v.sel();
        let (sel_rows, sel_cols) = (s2 - s0 + 1, s3 - s1 + 1);
        // Repete o bloco se a seleção for um múltiplo exato dele.
        let (reps_r, reps_c) =
            if sel_rows % rows == 0 && sel_cols % cols == 0 && (sel_rows, sel_cols) != (rows, cols) {
                (sel_rows / rows, sel_cols / cols)
            } else {
                (1, 1)
            };
        let (tr, tc) = (s0, s1);
        let total = rows as u64 * cols as u64 * reps_r as u64 * reps_c as u64;
        if total > MAX_PASTE {
            self.message = format!("Colagem grande demais ({} células)", crate::app::fmt_int(total));
            return;
        }
        let last_r =
            (tr as u64 + rows as u64 * reps_r as u64 - 1).min(crate::app::MAX_ROWS as u64 - 1) as u32;
        let last_c =
            (tc as u64 + cols as u64 * reps_c as u64 - 1).min(crate::app::MAX_COLS as u64 - 1) as u32;
        let cfx = CfCtx { sheet, shared: &doc.strings, today: self.today };
        let mut changes = Vec::with_capacity(total as usize);
        let mut styles_needed: Vec<(usize, u16, u16)> = Vec::new();
        for r in tr..=last_r {
            for c in tc..=last_c {
                let (i, j) = ((r - tr) % rows, (c - tc) % cols);
                match internal {
                    Some(clip) => {
                        let cell = &clip.cells[(i * cols + j) as usize];
                        let (dr, dc) = (r as i64 - (clip.r0 + i) as i64, c as i64 - (clip.c0 + j) as i64);
                        let (value, formula) = match &cell.formula {
                            Some(f) => {
                                let f = duck_calc::shift(f, dr, dc);
                                let v = if (dr, dc) == (0, 0) {
                                    cell.value.clone()
                                } else {
                                    cfx.eval_formula(&f, r, c).unwrap_or(CellValue::Empty)
                                };
                                (v, Some(f))
                            }
                            None => (cell.value.clone(), None),
                        };
                        changes.push(CellChange { row: r, col: c, value, formula, style: Some(cell.style) });
                    }
                    None => {
                        let field = tsv
                            .get(i as usize)
                            .and_then(|row| row.get(j as usize))
                            .map_or("", String::as_str);
                        let current = sheet.cell(r, c, &doc.strings).style;
                        let base = if current == 0 { v.blank_style(r, c) } else { current };
                        match parse_input(field, doc.date1904, duck_core::input::today()) {
                            Input::Formula(f) => {
                                let value = cfx.eval_formula(&f, r, c).unwrap_or(CellValue::Empty);
                                changes.push(CellChange {
                                    row: r,
                                    col: c,
                                    value,
                                    formula: Some(f),
                                    style: Some(base),
                                });
                            }
                            Input::Value(value, fmt) => {
                                if let Some(f) = fmt {
                                    let cur_fmt = doc.styles.xfs.get(base as usize).map_or(0, |x| x.num_fmt);
                                    if should_apply(&doc.styles, cur_fmt, f) {
                                        styles_needed.push((changes.len(), base, f));
                                    }
                                }
                                changes.push(CellChange {
                                    row: r,
                                    col: c,
                                    value,
                                    formula: None,
                                    style: Some(base),
                                });
                            }
                        }
                    }
                }
            }
        }
        // Recortar: apaga a origem (o que não foi coberto pelo destino).
        let mut cut_done = false;
        if let Some(clip) = internal
            && clip.cut
        {
            if clip.sheet == doc.active {
                for r in clip.r0..clip.r0 + clip.rows {
                    for c in clip.c0..clip.c0 + clip.cols {
                        if !(tr..=last_r).contains(&r) || !(tc..=last_c).contains(&c) {
                            changes.push(CellChange {
                                row: r,
                                col: c,
                                value: CellValue::Empty,
                                formula: None,
                                style: Some(0),
                            });
                        }
                    }
                }
                // Os apagados vêm depois, mas não podem sobrescrever o colado.
                cut_done = true;
            } else {
                self.message = "Recortar entre abas: colado como cópia".into();
            }
        }
        // Formatos sugeridos pelo texto colado (datas, %, moeda).
        if !styles_needed.is_empty() {
            let doc = self.doc.as_mut().expect("documento");
            let n = doc.styles.xfs.len();
            let st = Arc::make_mut(&mut doc.styles);
            for (k, base, f) in styles_needed {
                changes[k].style = Some(st.derive_xf(base, |x| x.num_fmt = f));
            }
            if doc.styles.xfs.len() != n {
                doc.looks = Arc::new(Looks::new(&doc.styles));
                self.font_key = (0, 0);
            }
        }
        let n = (last_r - tr + 1) as u64 * (last_c - tc + 1) as u64;
        if self.run_command(Command::SetCells(changes)) {
            if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                v.anchor = (tr, tc);
                v.cur = (last_r, last_c);
            }
            if cut_done {
                self.clip = None;
            }
            self.message = format!("{} células coladas", crate::app::fmt_int(n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_from_other_programs() {
        assert_eq!(parse_tsv("a\tb\r\n1\t2\r\n"), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(parse_tsv("\"x\ny\"\t\"di \"\"aspas\"\"\"\n"), vec![vec!["x\ny", "di \"aspas\""]]);
        assert_eq!(parse_tsv("só"), vec![vec!["só"]]);
        assert_eq!(parse_tsv("a\t\tc"), vec![vec!["a", "", "c"]]);
    }

    #[test]
    fn widths_round_trip_in_pixels() {
        for mdw in [6.0f32, 7.0, 8.0, 9.0, 11.0] {
            for px in 1..400u32 {
                assert_eq!(col_px(px_to_width(px, mdw), mdw), px, "mdw {mdw}, px {px}");
            }
        }
    }
}
