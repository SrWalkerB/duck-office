//! Abas no app: criar, renomear, excluir, duplicar, mover, cor, ocultar e
//! reexibir, com desfazer; o menu e o arraste das guias.
//!
//! O que muda nas outras abas (fórmulas, gráficos, links) vem do
//! `duck_calc::tabs`; a lista de abas em si muda por [`TabOp`], que vai no
//! histórico dentro de um `Command::Book`, junto com os comandos das abas.

use std::sync::{Arc, Mutex};

use duck_calc::tabs::BookRefs;
use duck_core::{BookOp, Color, Command, Sheet};
use duck_platform::{Clipboard, Key, Modifiers};
use duck_text::TextStyle;
use duck_ui::{Painter, Rect, TextInput};
use duck_xlsx::{SheetInfo, TabOrigin, TabOut, TabPlan};

use crate::app::{Doc, Geom, SheetsApp, Slot, build_view};
use crate::edit::Step;
use crate::menu::{Item, Popup};
use crate::ribbon::Act;
use crate::theme::Theme;

/// Mudança na lista de abas (ou nas listas do arquivo que citam abas).
pub enum TabOp {
    Insert {
        at: usize,
        slot: Box<Slot>,
    },
    Remove {
        at: usize,
    },
    Move {
        from: usize,
        to: usize,
    },
    Rename {
        at: usize,
        name: String,
    },
    Hide {
        at: usize,
        hidden: bool,
        very: bool,
    },
    /// Nomes definidos e fontes das dinâmicas.
    Refs(BookRefs),
}

/// Ações do menu da guia.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabAct {
    New,
    Delete,
    Rename,
    Duplicate,
    /// Abre a paleta.
    Color,
    SetColor(Option<u32>),
    Hide,
    /// Abre a lista das escondidas.
    Unhide,
    Show(u16),
}

/// Renomeando uma guia.
pub struct TabEdit {
    pub sheet: usize,
    pub input: TextInput,
}

/// Embrulha para o histórico.
pub fn book(op: TabOp) -> Command {
    Command::Book(BookOp(Arc::new(Mutex::new(Some(op)))))
}

fn unwrap(op: &BookOp) -> Option<TabOp> {
    op.0.downcast_ref::<Mutex<Option<TabOp>>>()?.lock().ok()?.take()
}

/// O passo mexe na lista de abas ou nas listas do arquivo?
pub fn has_book(step: &Step) -> bool {
    step.iter().any(|(_, c)| matches!(c, Command::Book(_)))
}

/// Aplica no documento e devolve o inverso.
pub fn apply(doc: &mut Doc, op: &BookOp) -> Option<Command> {
    let inverse = match unwrap(op)? {
        TabOp::Insert { at, slot } => {
            let at = at.min(doc.sheets.len());
            doc.sheets.insert(at, *slot);
            doc.calc.remap(|i| Some(if i >= at { i + 1 } else { i }));
            doc.active = at;
            TabOp::Remove { at }
        }
        TabOp::Remove { at } => {
            let slot = Box::new(doc.sheets.remove(at));
            doc.calc.remap(|i| match i.cmp(&at) {
                std::cmp::Ordering::Less => Some(i),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(i - 1),
            });
            if doc.active > at {
                doc.active -= 1;
            } else if doc.active == at {
                doc.active = neighbor(doc, at.min(doc.sheets.len().saturating_sub(1)));
            }
            TabOp::Insert { at, slot }
        }
        TabOp::Move { from, to } => {
            let slot = doc.sheets.remove(from);
            doc.sheets.insert(to, slot);
            let map = |i: usize| duck_calc::tabs::moved(i as u32, from as u32, to as u32) as usize;
            doc.calc.remap(|i| Some(map(i)));
            doc.active = map(doc.active);
            TabOp::Move { from: to, to: from }
        }
        TabOp::Rename { at, name } => {
            let slot = doc.sheets.get_mut(at)?;
            let old = std::mem::replace(&mut slot.info.name, name.clone());
            if let Some(s) = crate::edit::sheet_mut(slot) {
                s.name = name;
            }
            TabOp::Rename { at, name: old }
        }
        TabOp::Hide { at, hidden, very } => {
            let slot = doc.sheets.get_mut(at)?;
            let old = TabOp::Hide { at, hidden: slot.info.hidden, very: slot.info.very_hidden };
            slot.info.hidden = hidden;
            slot.info.very_hidden = very;
            if let Some(s) = crate::edit::sheet_mut(slot) {
                s.hidden = hidden;
            }
            if hidden && doc.active == at {
                doc.active = neighbor(doc, at);
            } else if !hidden {
                doc.active = at;
            }
            old
        }
        TabOp::Refs(refs) => TabOp::Refs(refs.swap(Arc::make_mut(&mut doc.calc.extras))),
    };
    doc.calc.request(crate::calc::Pending::All);
    Some(book(inverse))
}

/// A aba visível mais perto de `i` (à direita primeiro, como o Excel).
fn neighbor(doc: &Doc, i: usize) -> usize {
    let vis = |k: &usize| !doc.sheets[*k].info.hidden;
    (i..doc.sheets.len()).find(vis).or_else(|| (0..i).rev().find(vis)).unwrap_or(0)
}

/// O plano de gravação com as abas do documento.
pub fn plan(doc: &Doc) -> TabPlan {
    TabPlan {
        tabs: doc
            .sheets
            .iter()
            .map(|s| TabOut {
                origin: s.origin.clone(),
                name: s.info.name.clone(),
                id: s.info.id,
                hidden: s.info.hidden,
            })
            .collect(),
        names: doc.calc.extras.names.clone(),
    }
}

/// Depois de salvar: as abas novas agora estão no arquivo, e os passos do
/// histórico que devolveriam uma aba que saiu dele não valem mais.
pub fn saved(doc: &mut Doc, rep: &duck_xlsx::SaveReport) {
    for (i, slot) in doc.sheets.iter_mut().enumerate() {
        let Some(part) = rep.sheet_parts.get(i) else { continue };
        slot.info.part = part.clone();
        slot.origin = TabOrigin::File(part.clone());
        let renames: std::collections::HashMap<&str, &str> =
            rep.part_renames.iter().filter(|r| r.0 == i).map(|r| (r.1.as_str(), r.2.as_str())).collect();
        if !renames.is_empty()
            && let Some(s) = crate::edit::sheet_mut(slot)
        {
            s.layout.map_parts(&mut |p| renames.get(p).map(|n| n.to_string()));
        }
    }
    let stale = |step: &Step| {
        step.iter().any(|(_, c)| match c {
            Command::Book(op) => op.0.downcast_ref::<Mutex<Option<TabOp>>>().is_some_and(|m| {
                m.lock().is_ok_and(
                    |o| matches!(&*o, Some(TabOp::Insert { slot, .. }) if slot.origin != TabOrigin::Blank),
                )
            }),
            _ => false,
        })
    };
    for list in [&mut doc.history.undo, &mut doc.history.redo] {
        if let Some(k) = list.iter().rposition(stale) {
            list.drain(..=k);
        }
    }
}

/// A faixa das guias e o botão "+" (logo depois da última, ou no fim).
pub fn strip(g: &Geom, content: f32) -> (Rect, Rect) {
    let w = (30.0 * g.s).round();
    let room = Rect::new(g.tabs.x0, g.tabs.y0, (g.tabs.x1 - w).max(g.tabs.x0), g.tabs.y1);
    let x = (room.x0 + content).min(room.x1);
    (room, Rect::new(x, g.tabs.y0, x + w, g.tabs.y1))
}

impl SheetsApp {
    /// As abas podem mudar agora? Senão, explica.
    fn tabs_editable(&mut self) -> bool {
        self.commit_edit();
        if self.saving.is_some() {
            self.message = "Aguarde: salvando…".into();
            return false;
        }
        let Some(doc) = &self.doc else { return false };
        if doc.finished.is_none() || doc.sheets.iter().any(|s| s.data.is_none() || s.preview) {
            self.message = "Aguarde terminar de carregar".into();
            return false;
        }
        if crate::loader::is_csv(&doc.path) {
            self.message = "Um arquivo CSV tem uma aba só; salve como .xlsx para ter mais".into();
            return false;
        }
        true
    }

    fn refs(&self) -> Vec<&Sheet> {
        self.doc
            .as_ref()
            .map(|d| d.sheets.iter().filter_map(|s| s.data.as_deref()).collect())
            .unwrap_or_default()
    }

    fn tab_names(&self) -> Vec<String> {
        self.doc.as_ref().map(|d| d.sheets.iter().map(|s| s.info.name.clone()).collect()).unwrap_or_default()
    }

    /// `sheetId` livre (também entre as abas excluídas que podem voltar).
    fn next_tab_id(&self) -> u32 {
        let Some(doc) = &self.doc else { return 1 };
        let live = doc.sheets.iter().map(|s| s.info.id).max().unwrap_or(0);
        live.max(self.last_tab_id) + 1
    }

    fn new_slot(&mut self, sheet: Sheet, origin: TabOrigin) -> Box<Slot> {
        let mdw = self.doc.as_ref().map_or(7.0, |d| d.mdw);
        self.last_tab_id = self.last_tab_id.max(sheet.tab_id);
        let view = build_view(&sheet, mdw, None, None);
        Box::new(Slot {
            info: SheetInfo {
                name: sheet.name.clone(),
                id: sheet.tab_id,
                objects: !sheet.layout.objects.is_empty(),
                ..Default::default()
            },
            data: Some(Arc::new(sheet)),
            preview: false,
            view: Some(view),
            origin,
        })
    }

    /// Aba nova depois da ativa (ou na posição `at`).
    pub fn tab_new(&mut self, name: Option<&str>, at: Option<usize>) {
        if !self.tabs_editable() {
            return;
        }
        let names = self.tab_names();
        let others: Vec<&str> = names.iter().map(String::as_str).collect();
        let name = name.map(str::to_string).unwrap_or_else(|| duck_core::tabs::next_name(&others));
        if let Err(e) = duck_core::tabs::check_name(&name, &others) {
            self.message = e;
            return;
        }
        let mut sheet = Sheet::new(name.clone());
        sheet.tab_id = self.next_tab_id();
        sheet.layout.view = Some(Default::default());
        sheet.layout.default_row_height = Some(15.0);
        let Some(doc) = &self.doc else { return };
        let at = at.unwrap_or(doc.active + 1).min(doc.sheets.len());
        let refs = duck_calc::tabs::insert(&doc.calc.extras, at);
        let slot = self.new_slot(sheet, TabOrigin::Blank);
        if self.run_step(vec![(at, book(TabOp::Refs(refs))), (at, book(TabOp::Insert { at, slot }))]) {
            self.message = format!("Aba \"{name}\" criada");
        }
    }

    pub fn tab_rename(&mut self, i: usize, name: &str) {
        if !self.tabs_editable() {
            return;
        }
        let names = self.tab_names();
        let Some(old) = names.get(i).cloned() else { return };
        if old == name {
            return;
        }
        let others: Vec<&str> =
            names.iter().enumerate().filter(|(k, _)| *k != i).map(|(_, n)| n.as_str()).collect();
        if let Err(e) = duck_core::tabs::check_name(name, &others) {
            self.message = e;
            return;
        }
        let Some(doc) = &self.doc else { return };
        let (mut step, refs) = duck_calc::tabs::rename(&self.refs(), &doc.calc.extras, &old, name);
        let n = step.len();
        step.push((i, book(TabOp::Refs(refs))));
        step.push((i, book(TabOp::Rename { at: i, name: name.into() })));
        if self.run_step(step) {
            self.message = match n {
                0 => format!("Aba renomeada para \"{name}\""),
                n => format!("Aba renomeada para \"{name}\" ({n} abas com referências ajustadas)"),
            };
        }
    }

    pub fn tab_delete(&mut self, i: usize) {
        if !self.tabs_editable() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        if i >= doc.sheets.len() {
            return;
        }
        let (mut step, refs) = match duck_calc::tabs::delete(&self.refs(), &doc.calc.extras, i) {
            Ok(x) => x,
            Err(e) => {
                self.message = e;
                return;
            }
        };
        let slot = &doc.sheets[i];
        let used =
            slot.data.as_ref().is_some_and(|s| s.counts().values() > 0 || !s.layout.objects.is_empty());
        let name = slot.info.name.clone();
        if used && !self.ask(&format!("Excluir a aba \"{name}\"? Dá para desfazer até salvar o arquivo.")) {
            return;
        }
        step.push((i, book(TabOp::Refs(refs))));
        step.push((i, book(TabOp::Remove { at: i })));
        if self.run_step(step) {
            self.message = format!("Aba \"{name}\" excluída");
        }
    }

    /// Copia a aba `i` para a posição `at`.
    pub fn tab_duplicate(&mut self, i: usize, at: usize) {
        if !self.tabs_editable() {
            return;
        }
        let names = self.tab_names();
        let Some(old) = names.get(i) else { return };
        let others: Vec<&str> = names.iter().map(String::as_str).collect();
        let name = duck_core::tabs::copy_name(old, &others);
        let id = self.next_tab_id();
        let Some(doc) = &self.doc else { return };
        let at = at.min(doc.sheets.len());
        let c = duck_calc::tabs::duplicate(&self.refs(), &doc.calc.extras, i, at, &name, id);
        let origin = match &doc.sheets[i].origin {
            TabOrigin::File(part) => TabOrigin::Copy { part: part.clone(), renamed: c.renamed },
            // Cópia de uma cópia que ainda não está no arquivo.
            TabOrigin::Copy { part, renamed } => TabOrigin::Copy {
                part: part.clone(),
                renamed: renamed
                    .iter()
                    .map(|(a, b)| {
                        let n = c.renamed.iter().find(|r| r.0 == *b).map_or(b, |r| &r.1);
                        (a.clone(), n.clone())
                    })
                    .collect(),
            },
            TabOrigin::Blank => TabOrigin::Blank,
        };
        let slot = self.new_slot(c.sheet, origin);
        if self.run_step(vec![(at, book(TabOp::Refs(c.refs))), (at, book(TabOp::Insert { at, slot }))]) {
            self.message = format!("Aba copiada: \"{name}\"");
        }
    }

    /// Move a aba `from` para a posição `to` (depois de tirada).
    pub fn tab_move(&mut self, from: usize, to: usize) {
        if !self.tabs_editable() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let to = to.min(doc.sheets.len().saturating_sub(1));
        if from == to || from >= doc.sheets.len() {
            return;
        }
        let refs = duck_calc::tabs::reorder(&doc.calc.extras, from, to);
        if self.run_step(vec![(from, book(TabOp::Refs(refs))), (from, book(TabOp::Move { from, to }))]) {
            self.message = format!("Aba movida para a posição {}", to + 1);
        }
    }

    pub fn tab_color(&mut self, i: usize, rgb: Option<u32>) {
        if !self.tabs_editable() {
            return;
        }
        if self.run_step(vec![(i, Command::TabColor(rgb.map(|c| Color::Rgb(0xFF00_0000 | c))))]) {
            self.message = if rgb.is_some() { "Cor da guia mudada" } else { "Guia sem cor" }.into();
        }
    }

    pub fn tab_hide(&mut self, i: usize, hide: bool) {
        if !self.tabs_editable() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let Some(slot) = doc.sheets.get(i) else { return };
        if slot.info.hidden == hide {
            return;
        }
        if hide && doc.sheets.iter().enumerate().all(|(k, s)| k == i || s.info.hidden) {
            self.message = "O arquivo precisa ter pelo menos uma aba visível".into();
            return;
        }
        let name = slot.info.name.clone();
        if self.run_step(vec![(i, book(TabOp::Hide { at: i, hidden: hide, very: false }))]) {
            self.message = format!("Aba \"{name}\" {}", if hide { "oculta" } else { "reexibida" });
        }
    }

    /// Começa a renomear na própria guia.
    pub fn tab_edit_start(&mut self, i: usize) {
        if !self.tabs_editable() {
            return;
        }
        let Some(name) = self.tab_names().get(i).cloned() else { return };
        let mut input = TextInput::new();
        input.set_text(&name);
        input.select_all();
        self.tab_edit = Some(TabEdit { sheet: i, input });
    }

    /// Confirma o nome digitado na guia.
    pub fn tab_edit_commit(&mut self) {
        if let Some(e) = self.tab_edit.take() {
            self.tab_rename(e.sheet, &e.input.text);
        }
    }

    /// Teclas enquanto renomeia. `None` = não está renomeando.
    pub fn tab_edit_key(
        &mut self,
        clipboard: &mut Clipboard,
        key: &Key,
        mods: Modifiers,
        text: Option<&str>,
    ) -> Option<bool> {
        let e = self.tab_edit.as_mut()?;
        match key {
            Key::Enter | Key::Tab => self.tab_edit_commit(),
            Key::Escape => self.tab_edit = None,
            _ => {
                e.input.key(key, mods, text, clipboard);
            }
        }
        Some(true)
    }

    pub fn run_tab_act(&mut self, act: TabAct) {
        let Some(doc) = &self.doc else { return };
        let i = self.tab_menu.unwrap_or(doc.active).min(doc.sheets.len().saturating_sub(1));
        match act {
            TabAct::New => self.tab_new(None, Some(i + 1)),
            TabAct::Delete => self.tab_delete(i),
            TabAct::Rename => self.tab_edit_start(i),
            TabAct::Duplicate => self.tab_duplicate(i, i + 1),
            TabAct::Color => {
                let g = self.geom();
                let vis = self.visible_sheet_indices().iter().position(|&k| k == i).unwrap_or(0);
                let (strip, _) = strip(&g, self.tabbar.content_w());
                let x = strip.x0 + self.tabbar.span(vis).map_or(0.0, |s| s.0);
                self.popup = Some(Popup::tab_palette(Rect::new(x, g.tabs.y0, x, g.tabs.y0), &g));
            }
            TabAct::SetColor(c) => self.tab_color(i, c),
            TabAct::Hide => self.tab_hide(i, true),
            TabAct::Unhide => {
                let items: Vec<Item> = doc
                    .sheets
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.info.hidden && !s.info.very_hidden)
                    .map(|(k, s)| Item::new(s.info.name.clone(), Act::Tab(TabAct::Show(k as u16))))
                    .collect();
                if items.is_empty() {
                    self.message = "Nenhuma aba oculta".into();
                    return;
                }
                let g = self.geom();
                let (x, y) = self.mouse;
                let Some(text) = self.text.as_mut() else { return };
                let ui = text.default_style();
                self.popup = Some(Popup::list(items, x, y, &g, text, ui));
            }
            TabAct::Show(k) => self.tab_hide(k as usize, false),
        }
    }

    /// Menu do clique direito na guia sob `x`.
    pub fn tab_context_menu(&mut self, x: f32, y: f32) {
        self.tab_edit_commit();
        let g = self.geom();
        let (strip, _) = strip(&g, self.tabbar.content_w());
        let hit = strip
            .contains(x, y)
            .then(|| self.tabbar.hit(x - strip.x0))
            .flatten()
            .and_then(|k| self.visible_sheet_indices().get(k).copied());
        if let Some(i) = hit {
            self.open_tab_menu(i, x, y);
        }
    }

    fn open_tab_menu(&mut self, i: usize, x: f32, y: f32) {
        self.switch_sheet(i);
        self.tab_menu = Some(i);
        let g = self.geom();
        let hidden =
            self.doc.as_ref().is_some_and(|d| d.sheets.iter().any(|s| s.info.hidden && !s.info.very_hidden));
        let t = |label: &str, a: TabAct| Item::new(label, Act::Tab(a));
        let mut items = vec![
            t("Inserir aba", TabAct::New).key("Shift+F11"),
            t("Excluir", TabAct::Delete),
            t("Renomear", TabAct::Rename).key("duplo clique"),
            t("Duplicar", TabAct::Duplicate).key("Ctrl+arrastar"),
            Item::sep(),
            t("Cor da guia…", TabAct::Color),
            Item::sep(),
            t("Ocultar", TabAct::Hide),
        ];
        if hidden {
            items.push(t("Reexibir…", TabAct::Unhide));
        }
        let Some(text) = self.text.as_mut() else { return };
        let ui = text.default_style();
        self.popup = Some(Popup::list(items, x, y, &g, text, ui));
    }

    /// Clique na faixa das guias. Devolve o arraste a começar.
    pub fn tab_click(&mut self, g: &Geom, x: f32, y: f32, clicks: u8) -> Option<usize> {
        let (strip, plus) = strip(g, self.tabbar.content_w());
        if plus.contains(x, y) {
            self.tab_new(None, None);
            return None;
        }
        let k = strip.contains(x, y).then(|| self.tabbar.hit(x - strip.x0)).flatten()?;
        let i = self.visible_sheet_indices().get(k).copied()?;
        self.switch_sheet(i);
        if clicks >= 2 {
            self.tab_edit_start(i);
            return None;
        }
        Some(i)
    }

    /// Posição (na ordem das guias) onde a aba arrastada entraria em `x`.
    pub fn tab_drop_slot(&self, g: &Geom, x: f32) -> usize {
        let (strip, _) = strip(g, self.tabbar.content_w());
        self.tabbar.slot(x - strip.x0)
    }

    /// Solta a aba `from` na posição `slot` das guias visíveis.
    pub fn tab_drop(&mut self, from: usize, slot: usize, copy: bool) {
        let vis = self.visible_sheet_indices();
        // Antes da visível `slot` (ou no fim).
        let at = vis.get(slot).copied().unwrap_or_else(|| vis.last().map_or(0, |l| l + 1));
        if copy {
            self.tab_duplicate(from, at);
        } else {
            // A posição é contada sem a própria aba.
            let to = if at > from { at - 1 } else { at };
            self.tab_move(from, to);
        }
    }

    /// `--menu guia`, `guia-cor` e `guia-renomear` (screenshots).
    pub fn scripted_tab_menu(&mut self, m: &str) -> bool {
        let Some(active) = self.doc.as_ref().map(|d| d.active) else { return false };
        let g = self.geom();
        let vis = self.visible_sheet_indices().iter().position(|&k| k == active).unwrap_or(0);
        let x = g.tabs.x0 + self.tabbar.span(vis).map_or(10.0, |s| (s.0 + s.1) / 2.0);
        match m {
            "guia" => self.open_tab_menu(active, x, g.tabs.y0 + 4.0),
            "guia-cor" => {
                self.tab_menu = Some(active);
                self.run_tab_act(TabAct::Color);
            }
            "guia-renomear" => self.tab_edit_start(active),
            _ => return false,
        }
        true
    }

    /// `--edit` das abas (números a partir de 1). `true` = era uma delas.
    pub fn scripted_tabs(&mut self, e: &str) -> bool {
        let (op, arg) = e.split_once('=').unwrap_or((e, ""));
        let num = |s: &str| s.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1));
        let (first, rest) = arg.split_once(':').unwrap_or((arg, ""));
        match (op, num(first)) {
            ("aba-nova", _) => self.tab_new((!arg.is_empty()).then_some(arg), None),
            ("aba", Some(i)) => {
                self.switch_sheet(i);
                return true;
            }
            ("aba-renomear", Some(i)) => self.tab_rename(i, rest),
            ("aba-excluir", Some(i)) => self.tab_delete(i),
            ("aba-duplicar", Some(i)) => self.tab_duplicate(i, num(rest).unwrap_or(i + 1)),
            ("aba-mover", Some(i)) => {
                if let Some(to) = num(rest) {
                    self.tab_move(i, to);
                }
            }
            ("aba-cor", Some(i)) => self.tab_color(i, u32::from_str_radix(rest, 16).ok()),
            ("aba-ocultar", Some(i)) => self.tab_hide(i, true),
            ("aba-reexibir", Some(i)) => self.tab_hide(i, false),
            _ => return false,
        }
        eprintln!("--edit {op}: {}", self.message);
        true
    }
}

/// Desenha o botão "+", o campo de renomear e a marca do arraste.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    p: &mut Painter,
    g: &Geom,
    bar: &duck_ui::TabBar,
    edit: Option<(usize, &TextInput)>,
    drop: Option<usize>,
    mouse: (f32, f32),
    ui: TextStyle,
    th: &Theme,
) {
    let s = g.s;
    let px = s.round().max(1.0);
    let (room, plus) = strip(g, bar.content_w());
    p.fill(plus, &plus, th.tabs.background);
    p.fill(Rect::new(plus.x0, plus.y0, plus.x1, plus.y0 + px), &plus, th.line);
    if plus.contains(mouse.0, mouse.1) {
        p.fill(plus.inset(3.0 * s), &plus, th.hover);
    }
    let (cx, cy) = ((plus.x0 + plus.x1) / 2.0, (plus.y0 + plus.y1) / 2.0);
    let r = 5.0 * s;
    let w = (1.5 * s).max(1.0);
    p.fill(Rect::new(cx - r, cy - w / 2.0, cx + r, cy + w / 2.0), &plus, th.text);
    p.fill(Rect::new(cx - w / 2.0, cy - r, cx + w / 2.0, cy + r), &plus, th.text);
    if let Some((k, input)) = edit
        && let Some((a, b)) = bar.span(k)
    {
        let w = p.text.measure_styled(&input.text, ui) + 24.0 * s;
        let x0 = (room.x0 + a).max(room.x0);
        let box_ = Rect::new(x0, room.y0 + 2.0 * s, (x0 + (b - a).max(w)).min(g.w), room.y1 - 2.0 * s);
        input.draw(p, box_, ui, 10.0 * s, true, &th.input, "");
    }
    if let Some(k) = drop {
        let x = bar.slot_x(k);
        let x = (room.x0 + x).clamp(room.x0, room.x1);
        let all = Rect::everything();
        p.triangle(
            [x - 6.0 * s, room.y0 - 7.0 * s],
            [x + 6.0 * s, room.y0 - 7.0 * s],
            [x, room.y0],
            &all,
            th.accent,
        );
        p.fill(Rect::new(x - px, room.y0, x + px, room.y1), &all, th.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, get, names, type_in};

    fn doc(a: &mut SheetsApp) -> &mut Doc {
        a.doc.as_mut().unwrap()
    }

    fn named(a: &SheetsApp) -> Vec<(String, Option<u32>, String)> {
        let e = &a.doc.as_ref().unwrap().calc.extras;
        e.names.iter().map(|n| (n.name.clone(), n.sheet, n.formula.clone())).collect()
    }

    #[test]
    fn create_rename_delete_and_undo() {
        let mut a = app(&[("Dados", &[("A1", "5")]), ("Conta", &[("A1", "=Dados!A1*2")])]);
        Arc::make_mut(&mut doc(&mut a).calc.extras).names = vec![
            duck_core::DefinedName {
                name: "Total".into(),
                sheet: None,
                formula: "Dados!$A$1".into(),
                hidden: false,
            },
            duck_core::DefinedName {
                name: "Aqui".into(),
                sheet: Some(1),
                formula: "Conta!$A$1".into(),
                hidden: false,
            },
        ];
        a.tab_new(Some("Nova"), Some(1));
        assert_eq!(names(&a), ["Dados", "Nova", "Conta"]);
        assert_eq!(doc(&mut a).active, 1);
        assert_eq!(named(&a)[1].1, Some(2));
        a.tab_rename(0, "Meus dados");
        assert_eq!(get(&a, 2, "A1"), "='Meus dados'!A1*2");
        assert_eq!(named(&a)[0].2, "'Meus dados'!$A$1");
        a.tab_delete(0);
        assert_eq!(names(&a), ["Nova", "Conta"]);
        assert_eq!(get(&a, 1, "A1"), "=#REF!*2");
        assert_eq!((named(&a)[0].2.as_str(), named(&a)[1].1), ("#REF!", Some(1)));
        // Desfaz tudo, refaz tudo.
        for _ in 0..3 {
            a.undo();
        }
        assert_eq!(names(&a), ["Dados", "Conta"]);
        assert_eq!(get(&a, 1, "A1"), "=Dados!A1*2");
        assert_eq!(
            named(&a),
            [("Total".into(), None, "Dados!$A$1".into()), ("Aqui".into(), Some(1), "Conta!$A$1".into())]
        );
        for _ in 0..3 {
            a.redo();
        }
        assert_eq!(names(&a), ["Nova", "Conta"]);
        assert_eq!(get(&a, 1, "A1"), "=#REF!*2");
        a.undo();
        assert_eq!(get(&a, 2, "A1"), "='Meus dados'!A1*2");
        assert_eq!(get(&a, 0, "A1"), "5");
    }

    #[test]
    fn invalid_names_change_nothing() {
        let mut a = app(&[("A", &[]), ("B", &[])]);
        for bad in ["", "b", "x/y", &"z".repeat(32)] {
            a.tab_rename(0, bad);
            assert_eq!(names(&a), ["A", "B"], "{bad}");
        }
        assert!(doc(&mut a).history.undo.is_empty());
        // A última visível fica.
        a.tab_hide(1, true);
        a.tab_hide(0, true);
        assert!(!doc(&mut a).sheets[0].info.hidden);
        a.tab_delete(0);
        assert_eq!(names(&a), ["A", "B"]);
        a.undo();
        assert!(!doc(&mut a).sheets[1].info.hidden);
    }

    #[test]
    fn move_duplicate_color() {
        let mut a = app(&[("A", &[("A1", "1"), ("B1", "=A!A1+C!A1")]), ("B", &[]), ("C", &[("A1", "3")])]);
        a.tab_move(0, 2);
        assert_eq!(names(&a), ["B", "C", "A"]);
        assert_eq!(doc(&mut a).active, 2);
        a.undo();
        assert_eq!(names(&a), ["A", "B", "C"]);
        a.redo();
        a.tab_duplicate(2, 0);
        assert_eq!(names(&a), ["A (2)", "B", "C", "A"]);
        assert_eq!(get(&a, 0, "B1"), "='A (2)'!A1+C!A1");
        assert_eq!(doc(&mut a).sheets[0].info.id, 4);
        // Mexer na cópia não mexe na original.
        a.switch_sheet(0);
        type_in(&mut a, "A1", "99");
        assert_eq!((get(&a, 0, "A1"), get(&a, 3, "A1")), ("99".into(), "1".into()));
        a.tab_color(0, Some(0x00FF00));
        let color = |a: &SheetsApp| a.doc.as_ref().unwrap().sheets[0].data.as_ref().unwrap().tab_color();
        assert_eq!(color(&a), Some(Color::Rgb(0xFF00_FF00)));
        a.undo();
        assert_eq!(color(&a), None);
        // Soltar: antes da terceira visível, e no fim.
        a.tab_drop(0, 3, false);
        assert_eq!(names(&a), ["B", "C", "A (2)", "A"]);
        a.tab_drop(0, 4, false);
        assert_eq!(names(&a), ["C", "A (2)", "A", "B"]);
        let p = plan(a.doc.as_ref().unwrap());
        assert!(matches!(&p.tabs[1].origin, TabOrigin::Copy { part, .. } if part.ends_with("sheet1.xml")));
    }

    #[test]
    fn names_follow_rows_in_memory() {
        let mut a = app(&[("A", &[("A1", "1")])]);
        Arc::make_mut(&mut doc(&mut a).calc.extras).names = vec![duck_core::DefinedName {
            name: "Fim".into(),
            sheet: None,
            formula: "A!$A$5".into(),
            hidden: false,
        }];
        crate::testing::select(&mut a, "A2:A3");
        a.shift_selection(true, Some(duck_core::Axis::Rows));
        assert_eq!(named(&a)[0].2, "A!$A$7");
        a.undo();
        assert_eq!(named(&a)[0].2, "A!$A$5");
    }

    #[test]
    fn saved_tabs_are_in_the_file() {
        let mut a = app(&[("A", &[]), ("B", &[])]);
        a.tab_delete(1);
        a.tab_new(Some("N"), None);
        let rep = duck_xlsx::SaveReport {
            sheet_parts: vec!["xl/worksheets/sheet1.xml".into(), "xl/worksheets/sheet3.xml".into()],
            ..Default::default()
        };
        saved(doc(&mut a), &rep);
        assert_eq!(doc(&mut a).sheets[1].origin, TabOrigin::File("xl/worksheets/sheet3.xml".into()));
        // A exclusão não pode mais ser desfeita; criar, sim.
        assert_eq!(doc(&mut a).history.undo.len(), 1);
        a.undo();
        assert_eq!(names(&a), ["A"]);
        // Salvar sem ela: refazer também não vale mais.
        let rep = duck_xlsx::SaveReport {
            sheet_parts: vec!["xl/worksheets/sheet1.xml".into()],
            ..Default::default()
        };
        saved(doc(&mut a), &rep);
        a.redo();
        assert_eq!(names(&a), ["A"]);
    }
}
