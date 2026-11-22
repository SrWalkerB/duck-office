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
            Act::Paste => self.paste(cx),
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
            Act::Quit => {
                if self.confirm_discard() {
                    cx.exit();
                }
            }
            Act::Drop(_) => {}
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
                let z = if d == 0 { 100 } else { self.zoom() + 10 * d as i32 };
                self.set_zoom(z);
            }
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
            Act::Freeze(f) => self.freeze(f),
            Act::Hide { rows, hide } => self.hide(rows, hide),
            Act::ColWidth | Act::RowHeight => self.ask_size(act == Act::ColWidth),
            Act::Sort(desc) => self.sort_here(desc),
            Act::Filter => self.toggle_filter(),
            Act::ClearFilter => self.clear_filters(),
            Act::Reapply => self.reapply_filter(),
            Act::Autofit => {
                let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
                let (_, c0, _, c1) = v.sel();
                for c in c0..=c1.min(c0 + 200) {
                    self.autofit_col(c);
                }
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
            Drop::Insert => vec![
                Item::new("Linhas acima", Act::Shift { insert: true, cols: false }).key("Ctrl+Alt+="),
                Item::new("Colunas à esquerda", Act::Shift { insert: true, cols: true }),
            ],
            Drop::Delete => vec![
                Item::new("Linhas", Act::Shift { insert: false, cols: false }).key("Ctrl+Alt+-"),
                Item::new("Colunas", Act::Shift { insert: false, cols: true }),
            ],
            Drop::Format => vec![
                Item::new("Altura da linha…", Act::RowHeight),
                Item::new("Largura da coluna…", Act::ColWidth),
                Item::new("Ajustar largura da coluna", Act::Autofit),
                Item::sep(),
                Item::new("Ocultar linhas", Act::Hide { rows: true, hide: true }),
                Item::new("Ocultar colunas", Act::Hide { rows: false, hide: true }),
                Item::new("Reexibir linhas", Act::Hide { rows: true, hide: false }),
                Item::new("Reexibir colunas", Act::Hide { rows: false, hide: false }),
            ],
            Drop::Clear => vec![
                Item::new("Limpar tudo", Act::Clear(Clear::All)),
                Item::new("Limpar formatos", Act::Clear(Clear::Formats)),
                Item::new("Limpar conteúdo", Act::Clear(Clear::Contents)).key("Delete"),
            ],
            // As paletas abrem por `open_drop`.
            Drop::TextColor | Drop::FillColor => Vec::new(),
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
        let (r0, c0, r1, c1) = v.sel();
        let (nr, nc) = (v.rows.len(), v.cols.len());
        let inside = |r: u32, c: u32| (r0..=r1).contains(&r) && (c0..=c1).contains(&c);
        if g.colhdr.contains(x, y) {
            let q = if x < g.grid.x0 + g.fw { &g.quads[0] } else { &g.quads[1] };
            let (_, c) = g.cell_in(v, q, x, q.rect.y0);
            if !(inside(0, c) && r0 == 0 && r1 == nr - 1) {
                v.anchor = (0, c);
                v.cur = (nr - 1, c);
            }
        } else if g.rowhdr.contains(x, y) {
            let q = if y < g.grid.y0 + g.fh { &g.quads[0] } else { &g.quads[2] };
            let (r, _) = g.cell_in(v, q, q.rect.x0, y);
            if !(inside(r, 0) && c0 == 0 && c1 == nc - 1) {
                v.anchor = (r, 0);
                v.cur = (r, nc - 1);
            }
        } else if let Some((r, c)) = g.hit(v, x, y) {
            if !inside(r, c) {
                v.anchor = (r, c);
                v.cur = (r, c);
            }
        } else {
            return;
        }
        let merged = self.active_format().is_some_and(|a| a.merged);
        let mut items = vec![
            Item::new("Recortar", Act::Cut).key("Ctrl+X"),
            Item::new("Copiar", Act::Copy).key("Ctrl+C"),
            Item::new("Colar", Act::Paste).key("Ctrl+V"),
            Item::sep(),
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
            Item::new("Filtro", Act::Filter),
            Item::sep(),
        ];
        items.push(if merged {
            Item::new("Desfazer mesclagem", Act::Merge(Merge::Unmerge))
        } else {
            Item::new("Mesclar e centralizar", Act::Merge(Merge::Center))
        });
        items.extend([
            Item::sep(),
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
        let Some((r0, c0, r1, c1)) = self.sel_cells(2_000_000) else { return };
        let Some((sheet, _)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let cmd = match c {
            Clear::All => {
                let mut v = Vec::new();
                for r in r0..=r1 {
                    for col in c0..=c1 {
                        if !sheet.raw(r, col).is_blank() {
                            v.push(CellChange {
                                row: r,
                                col,
                                value: CellValue::Empty,
                                formula: None,
                                style: Some(0),
                            });
                        }
                    }
                }
                Command::SetCells(v)
            }
            _ => {
                let mut v = Vec::new();
                for r in r0..=r1 {
                    for col in c0..=c1 {
                        if sheet.raw(r, col).style != 0 {
                            v.push((r, col, 0));
                        }
                    }
                }
                Command::SetStyles(v)
            }
        };
        self.run_command(cmd);
    }

    pub(crate) fn hide(&mut self, rows: bool, hide: bool) {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let (r0, c0, r1, c1) = v.sel();
        let cmd = if rows {
            let list: Vec<(u32, bool)> = if hide {
                (r0..=r1.min(r0 + 1_000_000)).map(|r| (r, true)).collect()
            } else {
                sheet
                    .layout
                    .rows
                    .iter()
                    .filter(|s| s.hidden && (r0..=r1).contains(&s.row))
                    .map(|s| (s.row, false))
                    .collect()
            };
            if list.is_empty() {
                return;
            }
            Command::RowsHidden(list)
        } else {
            let list: Vec<(u32, bool)> = if hide {
                (c0..=c1).map(|c| (c, true)).collect()
            } else {
                (c0..=c1).filter(|&c| sheet.col_hidden(c)).map(|c| (c, false)).collect()
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
        let (r0, c0, r1, c1) = v.sel();
        match p.ask {
            Ask::ColWidth => {
                let w = x.clamp(0.0, 255.0);
                let list = (c0..=c1).map(|c| (c, Some(w))).collect();
                self.run_command(Command::ColWidths(list));
            }
            Ask::RowHeight => {
                let h = x.clamp(0.0, 409.0);
                let list = (r0..=r1.min(r0 + 100_000)).map(|r| (r, Some(h))).collect();
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
            "Adicionar o duck-sheets ao \"Abrir com\" do sistema para arquivos .xlsx, .xlsm e .csv? \
             Isso registra este programa só para o seu usuário; o programa padrão continua o mesmo.",
        );
        if !ok {
            return;
        }
        let exe = std::env::current_exe().unwrap_or_default();
        match duck_platform::register_file_types(&exe, "duck-sheets", &["xlsx", "xlsm", "csv"]) {
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
                "exibir" => Tab::View,
                _ => Tab::Home,
            };
        }
        let Some(m) = self.opts.menu.take() else { return };
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

    /// Pergunta sim/não. Nas execuções de teste (`--edit`, `--save-to`,
    /// `--screenshot`), responde sim sem abrir a caixa (ninguém responderia).
    pub fn ask(&self, text: &str) -> bool {
        let scripted = self.opts.save_to.is_some() || self.opts.screenshot.is_some() || self.opts.bench;
        if scripted {
            return true;
        }
        self.window.as_ref().is_none_or(|w| w.confirm("duck-sheets", text))
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
