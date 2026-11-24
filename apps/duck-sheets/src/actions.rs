//! O que os botões da faixa de opções, os menus e o clique direito fazem
//! (F3.5): formatar, mesclar, limpar, ocultar, congelar painéis, largura e
//! altura, e o menu Arquivo (novo, abrir, recentes, salvar, "Abrir com").

use std::path::PathBuf;

use duck_core::{CellChange, CellRange, CellValue, Command, Pane};
use duck_platform::Context;
use duck_ui::Rect;

use crate::app::{Geom, SheetsApp};
use crate::format::{Borders, FONTS, Fmt, NUMBER_FORMATS, SIZES};
use crate::menu::{Ask, Item, Popup, Prompt};
use crate::ribbon::{Act, Clear, Drop, Freeze, Merge, Tab};
use crate::theme::Mode;

/// Máximo de arquivos recentes no menu Arquivo.
const MAX_RECENT: usize = 9;

impl SheetsApp {
    /// Executa uma ação da faixa de opções, de um menu ou do clique direito.
    pub fn run_act(&mut self, cx: &mut Context, act: Act) {
        self.popup = None;
        match act {
            Act::Fmt(f @ (Fmt::FontColor(c) | Fmt::FillColor(c))) => {
                if matches!(f, Fmt::FontColor(_)) {
                    self.colors.0 = c.unwrap_or(0);
                } else {
                    self.colors.1 = c.unwrap_or(0xFFFFFF);
                }
                self.apply_fmt(f);
            }
            Act::Fmt(f) => self.apply_fmt(f),
            Act::Paste => self.paste(cx, Default::default()),
            Act::Cut | Act::Copy => {
                if let Some(t) = self.copy_text(act == Act::Cut) {
                    cx.clipboard().set_text(&t);
                }
            }
            Act::Undo => self.undo(),
            Act::Redo => self.redo(),
            Act::Save => self.save(false),
            Act::SaveAs => self.save(true),
            Act::New => self.new_file(),
            Act::Open => self.pick_and_open(cx),
            Act::Recent(i) => {
                if let Some(p) = self.recent.get(i as usize).cloned() {
                    if p.exists() {
                        self.open(p, cx.waker());
                    } else {
                        self.message = format!("Arquivo não encontrado: {}", p.display());
                        self.recent.retain(|x| *x != p);
                        save_recent(&self.recent);
                    }
                }
            }
            Act::Register => self.register_file_types(),
            Act::About => {
                let gpu = self
                    .gfx
                    .as_ref()
                    .map(|g| {
                        let i = g.adapter_info();
                        format!("{} ({}{})", i.name, i.backend, if i.software { ", software" } else { "" })
                    })
                    .unwrap_or_default();
                duck_platform::info_box(
                    "Sobre o duck-sheets",
                    &format!(
                        "duck-sheets {}\nPlanilhas .xlsx, .xlsm, .xlsb e .csv, nativo e rápido.\n\nGráficos: {gpu}\nLicença MIT · duck-office",
                        env!("CARGO_PKG_VERSION")
                    ),
                );
            }
            Act::Quit => {
                if self.confirm_discard() {
                    cx.exit();
                }
            }
            Act::Drop(_) => {}
            Act::Tab(t) => self.run_tab_act(t),
            Act::PasteAs(what, transpose) => {
                self.paste(cx, crate::paste::PasteOpts { what, transpose, ..Default::default() })
            }
            Act::PasteSpecial => {
                if let Some(t) = cx.clipboard().get_text() {
                    self.ask_paste_special(t);
                } else {
                    self.message = "Nada para colar".into();
                }
            }
            Act::Painter => self.toggle_painter(false),
            Act::Replace => self.open_replace(),
            Act::AutoSum => self.autosum(),
            Act::Cells(insert) => self.ask_cells(insert),
            Act::FillCopy(down) => self.fill_copy(down),
            Act::FormatCells => self.ask_format(0),
            Act::Shift { insert, cols } => {
                let axis = if cols { duck_core::Axis::Cols } else { duck_core::Axis::Rows };
                self.shift_selection(insert, Some(axis));
            }
            Act::Merge(m) => self.merge(m),
            Act::Clear(c) => self.clear(c),
            Act::Find => self.open_find(),
            Act::GoTo => {
                self.namebox_focus = true;
                self.cell_ref_into_namebox();
                self.namebox.select_all();
            }
            Act::Zoom(d) => {
                if d == 0 {
                    self.set_zoom(100);
                } else {
                    self.zoom_step(d > 0);
                }
            }
            Act::ZoomTo(z) => self.set_zoom(z as i32),
            Act::Grid => {
                if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                    v.grid = !v.grid;
                }
            }
            Act::Headers => {
                if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                    v.headers = !v.headers;
                }
            }
            Act::Theme(m) => self.set_theme(m),
            Act::Freeze(f) => self.freeze(f),
            Act::Hide { rows, hide } => self.hide(rows, hide),
            Act::ColWidth | Act::RowHeight => self.ask_size(act == Act::ColWidth),
            Act::Sort(desc) => self.sort_here(desc),
            Act::Filter => self.toggle_filter(),
            Act::ClearFilter => self.clear_filters(),
            Act::Reapply => self.reapply_filter(),
            Act::Validation => self.ask_validation(),
            Act::OpenList => {
                if !self.open_list() {
                    self.message = "A célula ativa não tem lista".into();
                }
            }
            Act::Note => self.ask_note(),
            Act::DeleteNotes => self.delete_notes(),
            Act::ShowNote => self.toggle_note(),
            Act::ShowNotes => self.show_notes = !self.show_notes,
            Act::NextNote(back) => self.next_note(back),
            Act::ProtectSheet(on) => self.protect_button(on, true),
            Act::ProtectBook(on) => self.protect_button(on, false),
            Act::Autofit => {
                let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
                let (_, c0, _, c1) = v.sel();
                self.autofit_cols(&(c0..=c1.min(c0 + 200)).collect::<Vec<_>>());
            }
        }
    }

    /// Abre a lista ou paleta de um botão `Drop`, embaixo de `rect`.
    pub fn open_drop(&mut self, d: Drop, rect: Rect, g: &Geom) {
        if d == Drop::TextColor || d == Drop::FillColor {
            let mut p = Popup::palette(d == Drop::TextColor, rect, g);
            p.owner = Some(Act::Drop(d));
            self.popup = Some(p);
            return;
        }
        let items = self.drop_items(d);
        let Some(text) = self.text.as_mut() else { return };
        let ui = text.default_style();
        let mut p = Popup::list(items, rect.x0, rect.y1 + 2.0 * g.s, g, text, ui);
        p.owner = Some(Act::Drop(d));
        self.popup = Some(p);
    }

    fn drop_items(&self, d: Drop) -> Vec<Item> {
        let active = self.active_format();
        match d {
            Drop::File => {
                let mut v = vec![
                    Item::new("Novo", Act::New),
                    Item::new("Abrir…", Act::Open).key("Ctrl+O"),
                    Item::sep(),
                ];
                for (i, p) in self.recent.iter().enumerate() {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    v.push(Item::new(format!("{}  {name}", i + 1), Act::Recent(i as u8)));
                }
                if !self.recent.is_empty() {
                    v.push(Item::sep());
                }
                v.extend([
                    Item::new("Salvar", Act::Save).key("Ctrl+S"),
                    Item::new("Salvar como…", Act::SaveAs).key("Ctrl+Shift+S"),
                    Item::sep(),
                    Item::new("Adicionar ao \"Abrir com\" do sistema…", Act::Register),
                    Item::new("Sobre o duck-sheets", Act::About),
                    Item::sep(),
                    Item::new("Sair", Act::Quit).key("Alt+F4"),
                ]);
                v
            }
            Drop::Font => FONTS
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let on = active.as_ref().is_some_and(|a| a.font.eq_ignore_ascii_case(f));
                    Item::new(*f, Act::Fmt(Fmt::Font(i as u8))).checked(on)
                })
                .collect(),
            Drop::Size => SIZES
                .iter()
                .map(|&s| {
                    let on = active.as_ref().is_some_and(|a| (a.size - s as f32).abs() < 0.01);
                    Item::new(s.to_string(), Act::Fmt(Fmt::Size(s * 10))).checked(on)
                })
                .collect(),
            Drop::NumFmt => NUMBER_FORMATS
                .iter()
                .map(|&(name, id)| {
                    let on = active.as_ref().is_some_and(|a| a.num_fmt == id);
                    Item::new(name, Act::Fmt(Fmt::Number(id))).checked(on)
                })
                .collect(),
            Drop::Borders => vec![
                Item::new("Todas as bordas", Act::Fmt(Fmt::Borders(Borders::All))),
                Item::new("Contorno", Act::Fmt(Fmt::Borders(Borders::Outline))).key("Ctrl+Shift+7"),
                Item::new("Contorno espesso", Act::Fmt(Fmt::Borders(Borders::ThickOutline))),
                Item::sep(),
                Item::new("Borda inferior", Act::Fmt(Fmt::Borders(Borders::Bottom))),
                Item::new("Borda superior", Act::Fmt(Fmt::Borders(Borders::Top))),
                Item::new("Borda esquerda", Act::Fmt(Fmt::Borders(Borders::Left))),
                Item::new("Borda direita", Act::Fmt(Fmt::Borders(Borders::Right))),
                Item::sep(),
                Item::new("Sem bordas", Act::Fmt(Fmt::Borders(Borders::None))).key("Ctrl+Shift+-"),
            ],
            Drop::Merge => vec![
                Item::new("Mesclar e centralizar", Act::Merge(Merge::Center)),
                Item::new("Mesclar células", Act::Merge(Merge::Cells)),
                Item::new("Mesclar através (por linha)", Act::Merge(Merge::Across)),
                Item::sep(),
                Item::new("Desfazer mesclagem", Act::Merge(Merge::Unmerge)),
            ],
            Drop::Fill => vec![
                Item::new("Para baixo", Act::FillCopy(true)).key("Ctrl+D"),
                Item::new("Para a direita", Act::FillCopy(false)).key("Ctrl+R"),
            ],
            Drop::Paste => {
                use crate::paste::What;
                vec![
                    Item::new("Valores", Act::PasteAs(What::Values, false)).key("Ctrl+Shift+V"),
                    Item::new("Fórmulas", Act::PasteAs(What::Formulas, false)),
                    Item::new("Formatos", Act::PasteAs(What::Formats, false)),
                    Item::new("Larguras das colunas", Act::PasteAs(What::Widths, false)),
                    Item::new("Transpor", Act::PasteAs(What::All, true)),
                    Item::sep(),
                    Item::new("Colar especial…", Act::PasteSpecial).key("Ctrl+Alt+V"),
                ]
            }
            Drop::Insert => vec![
                Item::new("Células…", Act::Cells(true)),
                Item::new("Linhas acima", Act::Shift { insert: true, cols: false }).key("Ctrl+Alt+="),
                Item::new("Colunas à esquerda", Act::Shift { insert: true, cols: true }),
            ],
            Drop::Delete => vec![
                Item::new("Células…", Act::Cells(false)),
                Item::new("Linhas", Act::Shift { insert: false, cols: false }).key("Ctrl+Alt+-"),
                Item::new("Colunas", Act::Shift { insert: false, cols: true }),
            ],
            Drop::Format => vec![
                Item::new("Formatar células…", Act::FormatCells).key("Ctrl+1"),
                Item::sep(),
                Item::new("Altura da linha…", Act::RowHeight),
                Item::new("Largura da coluna…", Act::ColWidth),
                Item::new("Ajustar largura da coluna", Act::Autofit),
                Item::sep(),
                Item::new("Ocultar linhas", Act::Hide { rows: true, hide: true }),
                Item::new("Ocultar colunas", Act::Hide { rows: false, hide: true }),
                Item::new("Reexibir linhas", Act::Hide { rows: true, hide: false }),
                Item::new("Reexibir colunas", Act::Hide { rows: false, hide: false }),
                Item::sep(),
                Item::new("Bloquear célula", Act::Fmt(Fmt::Lock))
                    .checked(active.as_ref().is_some_and(|a| a.locked)),
                Item::new("Ocultar fórmula", Act::Fmt(Fmt::HideFormula))
                    .checked(active.as_ref().is_some_and(|a| a.hide_formula)),
            ],
            Drop::Clear => vec![
                Item::new("Limpar tudo", Act::Clear(Clear::All)),
                Item::new("Limpar formatos", Act::Clear(Clear::Formats)),
                Item::new("Limpar conteúdo", Act::Clear(Clear::Contents)).key("Delete"),
                Item::new("Limpar notas", Act::DeleteNotes),
            ],
            // As paletas abrem por `open_drop`.
            Drop::TextColor | Drop::FillColor => Vec::new(),
            Drop::Zoom => {
                let cur = self.zoom();
                crate::zoombar::LEVELS
                    .iter()
                    .map(|&z| Item::new(format!("{z}%"), Act::ZoomTo(z)).checked(cur == z as i32))
                    .collect()
            }
            Drop::Freeze => {
                let pane =
                    self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| (v.frozen_rows, v.frozen_cols));
                let frozen = pane.is_some_and(|p| p != (0, 0));
                vec![
                    Item::new("Congelar painéis (até a célula atual)", Act::Freeze(Freeze::Here)),
                    Item::new("Congelar linha superior", Act::Freeze(Freeze::TopRow))
                        .checked(pane == Some((1, 0))),
                    Item::new("Congelar primeira coluna", Act::Freeze(Freeze::FirstCol))
                        .checked(pane == Some((0, 1))),
                    Item::sep(),
                    Item::new("Descongelar painéis", Act::Freeze(Freeze::Off)).checked(!frozen),
                ]
            }
        }
    }

    /// Clique direito em (`x`, `y`): seleciona a célula, linha ou coluna se
    /// ela não estiver na seleção, e abre o menu.
    pub fn context_menu(&mut self, x: f32, y: f32) {
        self.commit_edit();
        let g = self.geom();
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return };
        let areas = v.areas();
        let (nr, nc) = (v.rows.len(), v.cols.len());
        let inside =
            |r: u32, c: u32| areas.iter().any(|a| (a.0..=a.2).contains(&r) && (a.1..=a.3).contains(&c));
        let full_col = |c: u32| areas.iter().any(|a| a.0 == 0 && a.2 == nr - 1 && (a.1..=a.3).contains(&c));
        let full_row = |r: u32| areas.iter().any(|a| a.1 == 0 && a.3 == nc - 1 && (a.0..=a.2).contains(&r));
        if g.colhdr.contains(x, y) {
            let q = if x < g.grid.x0 + g.fw { &g.quads[0] } else { &g.quads[1] };
            let (_, c) = g.cell_in(v, q, x, q.rect.y0);
            if !full_col(c) {
                v.single();
                v.anchor = (0, c);
                v.cur = (nr - 1, c);
            }
        } else if g.rowhdr.contains(x, y) {
            let q = if y < g.grid.y0 + g.fh { &g.quads[0] } else { &g.quads[2] };
            let (r, _) = g.cell_in(v, q, q.rect.x0, y);
            if !full_row(r) {
                v.single();
                v.anchor = (r, 0);
                v.cur = (r, nc - 1);
            }
        } else if let Some((r, c)) = g.hit(v, x, y) {
            if !inside(r, c) {
                v.single();
                v.anchor = (r, c);
                v.cur = (r, c);
            }
        } else {
            return;
        }
        let merged = self.active_format().is_some_and(|a| a.merged);
        let note = self.has_note();
        let mut items = vec![
            Item::new("Recortar", Act::Cut).key("Ctrl+X"),
            Item::new("Copiar", Act::Copy).key("Ctrl+C"),
            Item::new("Colar", Act::Paste).key("Ctrl+V"),
            Item::new("Colar especial…", Act::PasteSpecial).key("Ctrl+Alt+V"),
            Item::sep(),
            Item::new("Inserir células…", Act::Cells(true)),
            Item::new("Excluir células…", Act::Cells(false)),
            Item::new("Inserir linhas acima", Act::Shift { insert: true, cols: false }),
            Item::new("Inserir colunas à esquerda", Act::Shift { insert: true, cols: true }),
            Item::new("Excluir linhas", Act::Shift { insert: false, cols: false }),
            Item::new("Excluir colunas", Act::Shift { insert: false, cols: true }),
            Item::sep(),
            Item::new("Limpar conteúdo", Act::Clear(Clear::Contents)).key("Delete"),
            Item::new("Limpar formatos", Act::Clear(Clear::Formats)),
            Item::sep(),
            Item::new("Classificar A→Z", Act::Sort(false)),
            Item::new("Classificar Z→A", Act::Sort(true)),
            Item::new("Filtro", Act::Filter).key("Ctrl+Shift+L"),
            Item::sep(),
            Item::new(if note { "Editar nota…" } else { "Nova nota…" }, Act::Note).key("Shift+F2"),
        ];
        if note {
            items.push(Item::new("Excluir nota", Act::DeleteNotes));
            items.push(Item::new("Mostrar ou ocultar a nota", Act::ShowNote));
        }
        items.push(Item::new("Validação de dados…", Act::Validation));
        items.push(Item::sep());
        items.push(if merged {
            Item::new("Desfazer mesclagem", Act::Merge(Merge::Unmerge))
        } else {
            Item::new("Mesclar e centralizar", Act::Merge(Merge::Center))
        });
        items.extend([
            Item::sep(),
            Item::new("Formatar células…", Act::FormatCells).key("Ctrl+1"),
            Item::new("Largura da coluna…", Act::ColWidth),
            Item::new("Altura da linha…", Act::RowHeight),
            Item::new("Ocultar linhas", Act::Hide { rows: true, hide: true }),
            Item::new("Ocultar colunas", Act::Hide { rows: false, hide: true }),
            Item::new("Reexibir linhas", Act::Hide { rows: true, hide: false }),
            Item::new("Reexibir colunas", Act::Hide { rows: false, hide: false }),
        ]);
        let Some(text) = self.text.as_mut() else { return };
        let ui = text.default_style();
        self.popup = Some(Popup::list(items, x, y, &g, text, ui));
    }

    // --- mesclar, limpar, ocultar, congelar ------------------------------------

    pub(crate) fn merge(&mut self, m: Merge) {
        self.commit_edit();
        let Some(doc) = self.doc.as_ref() else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r0, c0, r1, c1) = v.sel();
        let sel = CellRange { first_row: r0, first_col: c0, last_row: r1, last_col: c1 };
        let overlaps = |a: &CellRange, b: &CellRange| {
            a.first_row <= b.last_row
                && b.first_row <= a.last_row
                && a.first_col <= b.last_col
                && b.first_col <= a.last_col
        };
        let existing: Vec<CellRange> =
            sheet.layout.merges.iter().filter(|x| overlaps(x, &sel)).copied().collect();
        if m == Merge::Unmerge {
            if existing.is_empty() {
                self.message = "Não há células mescladas na seleção".into();
                return;
            }
            self.run_command(Command::Merges { add: Vec::new(), remove: existing });
            return;
        }
        if (r0, c0) == (r1, c1) {
            return;
        }
        if (r1 - r0 + 1) as u64 * (c1 - c0 + 1) as u64 > 100_000 {
            self.message = "Seleção grande demais para mesclar".into();
            return;
        }
        let ranges: Vec<CellRange> = match m {
            Merge::Across => (r0..=r1)
                .filter(|_| c1 > c0)
                .map(|r| CellRange { first_row: r, first_col: c0, last_row: r, last_col: c1 })
                .collect(),
            _ => vec![sel],
        };
        // Só a célula de cima à esquerda de cada mesclagem fica com o valor.
        let mut clear = Vec::new();
        for rg in &ranges {
            for r in rg.first_row..=rg.last_row {
                for c in rg.first_col..=rg.last_col {
                    if (r, c) != (rg.first_row, rg.first_col) && sheet.has_value(r, c) {
                        clear.push(CellChange {
                            row: r,
                            col: c,
                            value: CellValue::Empty,
                            formula: None,
                            style: None,
                        });
                    }
                }
            }
        }
        if !clear.is_empty()
            && !self.ask("Mesclar mantém só o valor da célula de cima à esquerda. Continuar?")
        {
            return;
        }
        let mut cmds = Vec::new();
        if !clear.is_empty() {
            cmds.push(Command::SetCells(clear));
        }
        cmds.push(Command::Merges { add: ranges, remove: existing });
        if m == Merge::Center
            && let Some(list) = self.restyled((r0, c0, r1, c1), Fmt::Align(duck_core::HAlign::Center), true)
        {
            cmds.push(Command::SetStyles(list));
        }
        self.run_command(Command::Group(cmds));
    }

    pub(crate) fn clear(&mut self, c: Clear) {
        if c == Clear::Contents {
            self.clear_selection();
            return;
        }
        let Some(areas) = self.sel_areas(2_000_000) else { return };
        let Some((sheet, _)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let mut more = Vec::new();
        if c == Clear::All {
            // Como no Excel: notas e validação saem junto.
            let (mut notes, mut rules) = (sheet.layout.notes.clone(), sheet.layout.validations.clone());
            for &(r0, c0, r1, c1) in &areas {
                let range = CellRange { first_row: r0, first_col: c0, last_row: r1, last_col: c1 };
                notes.retain(|n| !range.contains(n.row, n.col) || n.thread.is_some());
                duck_core::valid::clear(&mut rules, range, &duck_calc::shift);
            }
            if notes.len() != sheet.layout.notes.len() {
                more.push(Command::Notes(notes));
            }
            if rules != sheet.layout.validations {
                more.push(Command::Validations(rules));
            }
        }
        let all = areas
            .into_iter()
            .flat_map(|(r0, c0, r1, c1)| (r0..=r1).flat_map(move |r| (c0..=c1).map(move |c| (r, c))));
        let cmd = match c {
            Clear::All => Command::SetCells(
                all.filter(|&(r, c)| !sheet.raw(r, c).is_blank())
                    .map(|(row, col)| CellChange {
                        row,
                        col,
                        value: CellValue::Empty,
                        formula: None,
                        style: Some(0),
                    })
                    .collect(),
            ),
            _ => Command::SetStyles(
                all.filter(|&(r, c)| sheet.raw(r, c).style != 0).map(|(r, c)| (r, c, 0)).collect(),
            ),
        };
        more.insert(0, cmd);
        self.run_command(Command::Group(more));
    }

    pub(crate) fn hide(&mut self, rows: bool, hide: bool) {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let areas = v.areas();
        let cmd = if rows {
            let list: Vec<(u32, bool)> = if hide {
                areas.iter().flat_map(|a| a.0..=a.2.min(a.0 + 1_000_000)).map(|r| (r, true)).collect()
            } else {
                sheet
                    .layout
                    .rows
                    .iter()
                    .filter(|s| s.hidden && areas.iter().any(|a| (a.0..=a.2).contains(&s.row)))
                    .map(|s| (s.row, false))
                    .collect()
            };
            if list.is_empty() {
                return;
            }
            Command::RowsHidden(list)
        } else {
            let cols = areas.iter().flat_map(|a| a.1..=a.3);
            let list: Vec<(u32, bool)> = if hide {
                cols.map(|c| (c, true)).collect()
            } else {
                cols.filter(|&c| sheet.col_hidden(c)).map(|c| (c, false)).collect()
            };
            if list.is_empty() {
                return;
            }
            Command::ColsHidden(list)
        };
        self.run_command(cmd);
    }

    pub(crate) fn freeze(&mut self, f: Freeze) {
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let (rows, cols) = match f {
            Freeze::Here => v.cur,
            Freeze::TopRow => (1, 0),
            Freeze::FirstCol => (0, 1),
            Freeze::Off => (0, 0),
        };
        let pane = ((rows, cols) != (0, 0)).then_some(Pane { frozen_rows: rows, frozen_cols: cols });
        self.run_command(Command::Pane(pane));
    }

    // --- largura e altura -----------------------------------------------------

    fn ask_size(&mut self, cols: bool) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r, c) = v.cur;
        let cur = if cols {
            sheet.col_width(c).unwrap_or_else(|| crate::edit::px_to_width(v.cols.default_size(), doc.mdw))
        } else {
            sheet.row_height(r).unwrap_or(v.rows.default_size() as f32 * 0.75)
        };
        let mut input = duck_ui::TextInput::new();
        input.set_text(&format!("{}", (cur * 100.0).round() / 100.0).replace('.', ","));
        input.select_all();
        let (ask, title) = if cols {
            (Ask::ColWidth, "Largura da coluna (caracteres)")
        } else {
            (Ask::RowHeight, "Altura da linha (pontos)")
        };
        self.prompt = Some(Prompt { ask, title: title.into(), input });
    }

    /// OK na caixa de diálogo.
    pub fn prompt_ok(&mut self) {
        let Some(p) = self.prompt.take() else { return };
        let Ok(x) = p.input.text.trim().replace(',', ".").parse::<f32>() else {
            self.message = "Valor inválido".into();
            return;
        };
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let areas = v.areas();
        match p.ask {
            Ask::ColWidth => {
                let w = x.clamp(0.0, 255.0);
                let list = areas.iter().flat_map(|a| a.1..=a.3).map(|c| (c, Some(w))).collect();
                self.run_command(Command::ColWidths(list));
            }
            Ask::RowHeight => {
                let h = x.clamp(0.0, 409.0);
                let list =
                    areas.iter().flat_map(|a| a.0..=a.2.min(a.0 + 100_000)).map(|r| (r, Some(h))).collect();
                self.run_command(Command::RowHeights(list));
            }
        }
    }

    // --- arquivo --------------------------------------------------------------

    /// Planilha nova em branco (salvar pede o lugar na primeira vez).
    pub fn new_file(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let dir = std::env::temp_dir().join("duck-sheets");
        let _ = std::fs::create_dir_all(&dir);
        self.new_count += 1;
        let path = dir.join(format!("Planilha{}-{}.xlsx", self.new_count, std::process::id()));
        if let Err(e) = duck_xlsx::write::write_blank(&path) {
            duck_platform::message_box("Não foi possível criar a planilha", &e.to_string());
            return;
        }
        let Some(waker) = self.waker.clone() else { return };
        self.untitled_path = Some(path.clone());
        self.open(path, waker);
    }

    /// Nome do arquivo para mostrar: `Planilha1` na planilha nova ainda não salva.
    pub fn file_label(&self, path: &std::path::Path) -> String {
        if self.untitled_path.as_deref() == Some(path) {
            return format!("Planilha{}", self.new_count);
        }
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Ao sair: apaga os arquivos temporários das planilhas novas.
    pub fn cleanup(&mut self) {
        if let Some(l) = self.loader.take() {
            l.cancel();
        }
        self.doc = None;
        let dir = std::env::temp_dir().join("duck-sheets");
        for n in 1..=self.new_count {
            let _ = std::fs::remove_file(dir.join(format!("Planilha{n}-{}.xlsx", std::process::id())));
        }
    }

    /// Lembra o arquivo na lista de recentes (no topo).
    pub fn remember_recent(&mut self, path: &std::path::Path) {
        if self.untitled_path.as_deref() == Some(path) {
            return;
        }
        let p = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.recent.retain(|x| *x != p);
        self.recent.insert(0, p);
        self.recent.truncate(MAX_RECENT);
        save_recent(&self.recent);
    }

    fn register_file_types(&mut self) {
        let Some(w) = &self.window else { return };
        let ok = w.confirm(
            "duck-sheets",
            "Adicionar o duck-sheets ao \"Abrir com\" do sistema para arquivos .xlsx, .xlsm, .xlsb e .csv? \
             Isso registra este programa só para o seu usuário; o programa padrão continua o mesmo.",
        );
        if !ok {
            return;
        }
        let exe = std::env::current_exe().unwrap_or_default();
        match duck_platform::register_file_types(&exe, "duck-sheets", &["xlsx", "xlsm", "xlsb", "csv"]) {
            Ok(()) => self.message = "duck-sheets adicionado ao \"Abrir com\"".into(),
            Err(e) => duck_platform::message_box("Não foi possível registrar", &e),
        }
    }

    /// `--aba` e `--menu` (testes e screenshots sem mexer na janela).
    pub fn scripted_ui(&mut self) {
        if let Some(t) = self.opts.ribbon.take() {
            self.ribbon_tab = match t.as_str() {
                "inserir" => Tab::Insert,
                "dados" => Tab::Data,
                "revisao" => Tab::Review,
                "exibir" => Tab::View,
                _ => Tab::Home,
            };
        }
        if let Some(t) = self.opts.typing.take() {
            self.scripted_typing(&t);
        }
        let Some(m) = self.opts.menu.take() else { return };
        if self.scripted_dialog(&m) || self.scripted_tab_menu(&m) {
            return;
        }
        let g = self.geom();
        if m == "filtro" {
            let cur = self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| v.cur);
            if let Some((_, c)) = cur
                && let Some((b, f, col)) = self.filter_buttons(&g).into_iter().find(|x| x.2 == c)
            {
                self.open_filter_panel(f, col, b);
            }
            return;
        }
        if m == "contexto" {
            let Some(cell) = self
                .doc
                .as_ref()
                .and_then(|d| d.current())
                .and_then(|(_, v)| crate::draw::cell_screen_rect(&g, v, v.cur.0, v.cur.1))
            else {
                return;
            };
            self.context_menu(cell.x0 + 10.0, cell.y0 + 5.0);
            return;
        }
        let d = match m.as_str() {
            "arquivo" => Drop::File,
            "fonte" => Drop::Font,
            "tamanho" => Drop::Size,
            "numero" => Drop::NumFmt,
            "bordas" => Drop::Borders,
            "cor" => Drop::TextColor,
            "fundo" => Drop::FillColor,
            "mesclar" => Drop::Merge,
            "inserir" => Drop::Insert,
            "excluir" => Drop::Delete,
            "formatar" => Drop::Format,
            "limpar" => Drop::Clear,
            "congelar" => Drop::Freeze,
            "colar" => Drop::Paste,
            "preencher" => Drop::Fill,
            "caixa-colar" => return self.ask_paste_special("a\tb".into()),
            "caixa-substituir" => return self.open_replace(),
            "caixa-inserir" => return self.ask_cells(true),
            "caixa-excluir" => return self.ask_cells(false),
            m if m.starts_with("caixa-formatar") => {
                return self.ask_format(m.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0));
            }
            "zoom" => {
                let r = crate::zoombar::layout(&g).menu_anchor(&g);
                self.open_drop(Drop::Zoom, r, &g);
                return;
            }
            _ => return,
        };
        if matches!(d, Drop::Freeze) {
            self.ribbon_tab = Tab::View;
        }
        let tab = self.ribbon_tab;
        let Some(text) = self.text.as_mut() else { return };
        let ui = text.default_style();
        let lay = crate::ribbon::layout(&g, tab, text, ui);
        let rect = if d == Drop::File { Some(lay.file_tab()) } else { lay.rect_of(Act::Drop(d)) };
        if let Some(r) = rect {
            self.open_drop(d, r, &g);
        }
    }

    /// `--digitar`: abre a edição da célula ativa e digita um texto.
    /// `{barra}` no começo edita na barra de fórmulas. Entre chaves também:
    ///
    /// - teclas: `{cima}`, `{baixo}`, `{esquerda}`, `{direita}` (com `shift-`,
    ///   `ctrl-` ou `alt-` antes), `{tab}`, `{enter}`, `{esc}`, `{f2}`, `{f4}`;
    /// - mouse: `{clique:B2}`, `{shift-clique:B5}`, `{arrastar:B2:B10}`,
    ///   `{coluna:B}`, `{linha:3}` e `{aba:Nome}`.
    fn scripted_typing(&mut self, t: &str) {
        use duck_platform::{Clipboard, Key, Modifiers};
        let (t, bar) = match t.strip_prefix("{barra}") {
            Some(rest) => (rest, true),
            None => (t, false),
        };
        let mut clipboard = Clipboard::default();
        self.start_edit(Some(""), bar);
        let mut rest = t;
        while let Some(c) = rest.chars().next() {
            if let Some((cmd, after)) = rest.strip_prefix('{').and_then(|r| r.split_once('}')) {
                let mut mods = Modifiers::default();
                let name = match cmd.split_once('-') {
                    Some(("shift", k)) => {
                        mods.shift = true;
                        k
                    }
                    Some(("ctrl", k)) => {
                        mods.ctrl = true;
                        k
                    }
                    Some(("alt", k)) => {
                        mods.alt = true;
                        k
                    }
                    _ => cmd,
                };
                let key = match name {
                    "cima" => Some(Key::ArrowUp),
                    "baixo" => Some(Key::ArrowDown),
                    "esquerda" => Some(Key::ArrowLeft),
                    "direita" => Some(Key::ArrowRight),
                    "tab" => Some(Key::Tab),
                    "enter" => Some(Key::Enter),
                    "esc" => Some(Key::Escape),
                    "f2" => Some(Key::F(2)),
                    "f4" => Some(Key::F(4)),
                    _ => None,
                };
                if let Some(key) = key {
                    self.editor_key(&mut clipboard, &key, mods, None);
                    rest = after;
                    continue;
                }
                if let Some((op, arg)) = name.split_once(':') {
                    if !self.scripted_click(op, arg, mods) {
                        eprintln!("--digitar: clique inválido ou fora da tela: {cmd}");
                    }
                    rest = after;
                    continue;
                }
            }
            rest = &rest[c.len_utf8()..];
            let key = Key::Char(c.to_string());
            self.editor_key(&mut clipboard, &key, Modifiers::default(), Some(c.encode_utf8(&mut [0; 4])));
        }
    }

    /// Um clique (ou arraste) de [`SheetsApp::scripted_typing`], pelo mesmo
    /// caminho do mouse: se não aponta, confirma a edição.
    fn scripted_click(&mut self, op: &str, arg: &str, mods: duck_platform::Modifiers) -> bool {
        // A célula precisa estar à vista, como para um clique de verdade.
        if matches!(op, "clique" | "arrastar")
            && let Some((a, _)) = crate::app::parse_range(arg)
        {
            self.reveal(a.0, a.1);
        }
        let g = self.geom();
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return false };
        let alone = |r: u32, c: u32| v.merges.at(r, c).is_none();
        let center = |r: u32, c: u32| {
            crate::draw::cell_screen_rect(&g, v, r, c).map(|b| ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0))
        };
        let (from, to) = match op {
            "clique" | "arrastar" => {
                let Some((a, b)) = crate::app::parse_range(arg) else { return false };
                (center(a.0, a.1), (op == "arrastar").then(|| center(b.0, b.1)).flatten())
            }
            "coluna" => {
                let Some((_, c)) = crate::app::parse_cell(&format!("{arg}1")) else { return false };
                let first = (0..v.rows.len()).filter(|&r| alone(r, c)).find_map(|r| center(r, c));
                (first.map(|p| (p.0, (g.colhdr.y0 + g.colhdr.y1) / 2.0)), None)
            }
            "linha" => {
                let Some((r, _)) = crate::app::parse_cell(&format!("A{arg}")) else { return false };
                let first = (0..v.cols.len()).filter(|&c| alone(r, c)).find_map(|c| center(r, c));
                (first.map(|p| ((g.rowhdr.x0 + g.rowhdr.x1) / 2.0, p.1)), None)
            }
            "aba" => {
                let Some(doc) = &self.doc else { return false };
                let found = doc.sheets.iter().position(|s| s.info.name.eq_ignore_ascii_case(arg));
                let Some(i) = found else { return false };
                if !self.can_point() {
                    self.commit_edit();
                }
                self.switch_sheet(i);
                return true;
            }
            _ => return false,
        };
        let Some((x, y)) = from else { return false };
        if !self.editor_click(&g, x, y, mods) {
            self.commit_edit();
        }
        if let Some((x, y)) = to {
            self.mouse_move(x, y);
        }
        self.drag = None;
        true
    }

    /// Pergunta sim/não. Nas execuções de teste (`--edit`, `--save-to`,
    /// `--screenshot`), responde sim sem abrir a caixa (ninguém responderia).
    pub fn ask(&self, text: &str) -> bool {
        let scripted = self.opts.save_to.is_some() || self.opts.screenshot.is_some() || self.opts.bench;
        if scripted {
            return true;
        }
        self.window.as_ref().is_none_or(|w| w.confirm("duck-sheets", text))
    }

    /// Troca o tema e lembra a escolha para as próximas vezes.
    pub fn set_theme(&mut self, mode: Mode) {
        self.mode = mode;
        if let Some(w) = &self.window {
            w.set_color_scheme(mode.frame());
            if mode == Mode::System {
                self.system = w.color_scheme().unwrap_or(self.system);
            }
        }
        if let Some(Err(e)) = crate::theme::pref_file().map(|f| crate::theme::save(&f, mode)) {
            self.message = format!("O tema mudou, mas não foi possível lembrar a escolha: {e}");
        }
    }

    /// Aba ativa da faixa.
    pub fn set_ribbon_tab(&mut self, t: Tab) {
        self.ribbon_tab = t;
        self.popup = None;
    }
}

/// Arquivo da lista de recentes.
fn recent_file() -> Option<PathBuf> {
    duck_platform::config_dir("duck-office").map(|d| d.join("recentes.txt"))
}

pub fn load_recent() -> Vec<PathBuf> {
    let Some(f) = recent_file() else { return Vec::new() };
    std::fs::read_to_string(f)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).take(MAX_RECENT).collect())
        .unwrap_or_default()
}

fn save_recent(list: &[PathBuf]) {
    let Some(f) = recent_file() else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = std::fs::write(f, text);
}
