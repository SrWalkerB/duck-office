//! Filtro automático e classificar (F3.5, segunda parte).
//!
//! - Botões nos cabeçalhos do filtro da aba e das tabelas com filtro.
//! - Painel com classificar A→Z / Z→A, busca e a lista de valores da coluna
//!   (com caixas de marcar), calculada em paralelo. Como no Excel, a lista
//!   mostra só os valores das linhas que passam nos filtros das outras colunas.
//! - Aplicar esconde as linhas que não passam (comando `RowsHidden`, em
//!   massa) e guarda o critério (`SetFilter`), num passo de desfazer só.
//! - Classificar ordena as linhas de dados em paralelo (ordem do Excel:
//!   números, textos sem diferenciar maiúsculas e acentos, lógicos, erros,
//!   vazias por último) e reaplica o filtro.

use std::collections::{HashMap, HashSet};

use duck_core::{AutoFilter, CellRange, Command, FilterColumn, FilterKind, Sheet, StringTable, Value};
use duck_text::TextStyle;
use duck_ui::{Align, InputColors, Painter, Rect, TextInput};
use rayon::prelude::*;

use crate::app::{Geom, SheetsApp, View};
use crate::look::{Looks, show};
use crate::search::fold;

/// Máximo de valores na lista (o resto não aparece, mas o filtro vale).
const MAX_ITEMS: usize = 50_000;
/// Linhas por pedaço nas passadas em paralelo.
const CHUNK: u32 = 8192;

/// De onde vem o filtro: da aba ou de uma tabela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Sheet,
    Table(u32),
}

/// Um filtro na tela: alvo, intervalo (com o cabeçalho) e critérios.
#[derive(Clone, Debug)]
pub struct Active {
    pub target: Target,
    pub filter: AutoFilter,
}

impl Active {
    /// Linhas de dados (sem o cabeçalho; nas tabelas, sem a linha de totais).
    pub fn data_rows(&self) -> (u32, u32) {
        let r = self.filter.range;
        (r.first_row + 1, r.last_row)
    }
}

/// Os filtros da aba: o dela e os das tabelas que têm filtro.
pub fn filters(sheet: &Sheet) -> Vec<Active> {
    let mut v = Vec::new();
    if let Some(af) = &sheet.layout.autofilter {
        v.push(Active { target: Target::Sheet, filter: af.clone() });
    }
    for (i, t) in sheet.layout.tables.iter().enumerate() {
        if let Some(f) = &t.filter
            && t.header_rows > 0
        {
            v.push(Active { target: Target::Table(i as u32), filter: f.clone() });
        }
    }
    v
}

/// Texto mostrado na célula (é o que o filtro do Excel compara).
#[allow(clippy::too_many_arguments)]
fn shown(
    sheet: &Sheet,
    strings: &StringTable,
    looks: &Looks,
    view: &View,
    date1904: bool,
    r: u32,
    c: u32,
    buf: &mut String,
) -> bool {
    let cell = sheet.cell(r, c, strings);
    if matches!(cell.value, Value::Empty) {
        return false;
    }
    let s = if cell.style == 0 { view.blank_style(r, c) } else { cell.style };
    buf.clear();
    show(looks, looks.get(s), cell.value, date1904, buf);
    !buf.is_empty()
}

/// Critérios de valores já prontos para comparar (texto em minúsculas).
struct Compiled {
    cols: Vec<(u32, HashSet<String>, bool)>,
}

fn compile(af: &AutoFilter, except: Option<u32>) -> Compiled {
    let cols = af
        .columns
        .iter()
        .filter(|c| Some(c.col) != except)
        .filter_map(|c| match &c.kind {
            FilterKind::Values { values, blank } => {
                Some((af.range.first_col + c.col, values.iter().map(|v| v.to_lowercase()).collect(), *blank))
            }
            FilterKind::Other(_) => None,
        })
        .collect();
    Compiled { cols }
}

/// A linha passa nos critérios de valores?
#[allow(clippy::too_many_arguments)]
fn passes(
    cp: &Compiled,
    sheet: &Sheet,
    strings: &StringTable,
    looks: &Looks,
    view: &View,
    d1904: bool,
    r: u32,
    buf: &mut String,
) -> bool {
    cp.cols.iter().all(|(c, set, blank)| {
        if shown(sheet, strings, looks, view, d1904, r, *c, buf) {
            set.contains(&buf.to_lowercase())
        } else {
            *blank
        }
    })
}

/// Um valor da lista.
#[derive(Clone, Debug)]
pub struct Item {
    pub text: String,
    num: Option<f64>,
    pub checked: bool,
}

/// Painel aberto de uma coluna.
pub struct Panel {
    pub active: Active,
    /// Coluna (absoluta).
    pub col: u32,
    pub items: Vec<Item>,
    /// Há células vazias na coluna (e se estão marcadas).
    pub blank: Option<bool>,
    pub search: TextInput,
    pub scroll: usize,
    pub rect: Rect,
    /// A lista foi cortada em [`MAX_ITEMS`].
    pub truncated: bool,
}

const W: f32 = 290.0;
const ROW: f32 = 24.0;
const LIST_ROWS: usize = 11;

/// Áreas do painel (pixels físicos).
pub struct Areas {
    pub sort_asc: Rect,
    pub sort_desc: Rect,
    pub search: Rect,
    pub list: Rect,
    pub ok: Rect,
    pub cancel: Rect,
}

impl Panel {
    pub fn areas(&self, s: f32) -> Areas {
        let r = self.rect;
        let x0 = r.x0 + 8.0 * s;
        let x1 = r.x1 - 8.0 * s;
        let mut y = r.y0 + 6.0 * s;
        let mut next = |h: f32| {
            let a = Rect::new(x0, y, x1, y + h * s);
            y += h * s;
            a
        };
        let sort_asc = next(ROW + 4.0);
        let sort_desc = next(ROW + 4.0);
        next(8.0);
        let search = next(30.0);
        next(6.0);
        let list = next(ROW * LIST_ROWS as f32);
        next(10.0);
        let buttons = next(30.0);
        let bw = 90.0 * s;
        let cancel = Rect::new(buttons.x1 - bw, buttons.y0, buttons.x1, buttons.y1);
        let ok = Rect::new(cancel.x0 - 8.0 * s - bw, buttons.y0, cancel.x0 - 8.0 * s, buttons.y1);
        Areas { sort_asc, sort_desc, search, list, ok, cancel }
    }

    pub fn height(s: f32) -> f32 {
        (6.0 + 2.0 * (ROW + 4.0) + 8.0 + 30.0 + 6.0 + ROW * LIST_ROWS as f32 + 10.0 + 30.0 + 10.0) * s
    }

    /// Linhas da lista depois da busca: `None` = "(Selecionar tudo)",
    /// `Some(None)` = "(Vazias)", `Some(Some(i))` = item `i`.
    fn visible(&self) -> Vec<Option<Option<usize>>> {
        let q = fold(self.search.text.trim());
        let mut v: Vec<Option<Option<usize>>> = vec![None];
        v.extend(
            self.items
                .iter()
                .enumerate()
                .filter(|(_, it)| q.is_empty() || fold(&it.text).contains(&q))
                .map(|(i, _)| Some(Some(i))),
        );
        if self.blank.is_some() && (q.is_empty() || "(vazias)".contains(&q)) {
            v.push(Some(None));
        }
        v
    }

    /// Clique numa linha da lista.
    pub fn click_list(&mut self, s: f32, y: f32) {
        let a = self.areas(s);
        let k = ((y - a.list.y0) / (ROW * s)).floor() as usize + self.scroll;
        let vis = self.visible();
        match vis.get(k) {
            Some(None) => {
                // Selecionar tudo (só os que a busca mostra).
                let all = vis.iter().skip(1).all(|x| self.is_checked(*x));
                for x in vis.iter().skip(1) {
                    self.set_checked(*x, !all);
                }
            }
            Some(x) => {
                let on = self.is_checked(*x);
                self.set_checked(*x, !on);
            }
            None => {}
        }
    }

    fn is_checked(&self, x: Option<Option<usize>>) -> bool {
        match x {
            Some(Some(i)) => self.items[i].checked,
            Some(None) => self.blank.unwrap_or(false),
            None => true,
        }
    }

    fn set_checked(&mut self, x: Option<Option<usize>>, on: bool) {
        match x {
            Some(Some(i)) => self.items[i].checked = on,
            Some(None) => self.blank = self.blank.map(|_| on),
            None => {}
        }
    }

    pub fn scroll_by(&mut self, lines: i32) {
        let n = self.visible().len();
        let max = n.saturating_sub(LIST_ROWS);
        self.scroll = (self.scroll as i64 + lines as i64).clamp(0, max as i64) as usize;
    }

    /// O critério novo da coluna (`None` = tudo marcado: sem critério).
    fn criterion(&self) -> Option<FilterKind> {
        let all = self.items.iter().all(|i| i.checked) && self.blank.unwrap_or(true) && !self.truncated;
        if all {
            return None;
        }
        Some(FilterKind::Values {
            values: self.items.iter().filter(|i| i.checked).map(|i| i.text.clone()).collect(),
            blank: self.blank.unwrap_or(false),
        })
    }

    pub fn draw(
        &self,
        p: &mut Painter,
        g: &Geom,
        ui: TextStyle,
        mouse: (f32, f32),
        th: &crate::ribbon::Theme,
    ) {
        let s = g.s;
        let px = s.round().max(1.0);
        let all = Rect::everything();
        let r = self.rect;
        p.fill(
            Rect::new(r.x0 + 2.0 * s, r.y0 + 2.0 * s, r.x1 + 3.0 * s, r.y1 + 3.0 * s),
            &all,
            [0, 0, 0, 30],
        );
        p.fill(r.inset(-px), &all, th.line);
        p.fill(r, &all, [255, 255, 255, 255]);
        let a = self.areas(s);
        for (rect, label) in [(a.sort_asc, "Classificar de A a Z"), (a.sort_desc, "Classificar de Z a A")] {
            if rect.contains(mouse.0, mouse.1) {
                p.fill(rect, &all, th.hover);
            }
            p.label(label, ui, rect, Align::Left, 8.0 * s, th.text);
        }
        let sep = (a.sort_desc.y1 + 4.0 * s).round();
        p.fill(Rect::new(r.x0 + 8.0 * s, sep, r.x1 - 8.0 * s, sep + px), &all, th.line);
        self.search.draw(p, a.search, ui, 6.0 * s, true, &InputColors::default(), "Buscar");
        p.stroke(a.list, &all, px, th.line);
        let vis = self.visible();
        for (k, x) in vis.iter().enumerate().skip(self.scroll).take(LIST_ROWS) {
            let y0 = a.list.y0 + (k - self.scroll) as f32 * ROW * s;
            let row = Rect::new(a.list.x0 + px, y0, a.list.x1 - px, y0 + ROW * s);
            if row.contains(mouse.0, mouse.1) {
                p.fill(row, &a.list, th.hover);
            }
            let on = match x {
                None => vis.iter().skip(1).all(|y| self.is_checked(*y)),
                _ => self.is_checked(*x),
            };
            let bx = Rect::new(row.x0 + 6.0 * s, row.y0 + 5.0 * s, row.x0 + 20.0 * s, row.y0 + 19.0 * s);
            p.fill(bx, &a.list, if on { th.accent } else { [255, 255, 255, 255] });
            p.stroke(bx, &a.list, px, if on { th.accent } else { th.muted });
            if on {
                // Marca: dois traços.
                p.fill(
                    Rect::new(bx.x0 + 3.0 * s, bx.y0 + 6.0 * s, bx.x0 + 6.0 * s, bx.y0 + 9.0 * s),
                    &a.list,
                    [255, 255, 255, 255],
                );
                p.fill(
                    Rect::new(bx.x0 + 5.0 * s, bx.y0 + 8.0 * s, bx.x1 - 3.0 * s, bx.y0 + 10.0 * s),
                    &a.list,
                    [255, 255, 255, 255],
                );
            }
            let label = match x {
                None => "(Selecionar tudo)",
                Some(None) => "(Vazias)",
                Some(Some(i)) => self.items[*i].text.as_str(),
            };
            let lr = Rect::new(bx.x1 + 6.0 * s, row.y0, row.x1, row.y1);
            p.label(label, ui, lr, Align::Left, 0.0, th.text);
        }
        if self.truncated {
            let note = Rect::new(a.list.x0, a.list.y1, a.list.x1, a.list.y1 + 10.0 * s);
            p.label(
                "lista cortada em 50 mil valores",
                ui.with_px((10.0 * s).round()),
                note,
                Align::Left,
                4.0 * s,
                th.muted,
            );
        }
        for (rect, label, primary) in [(a.ok, "OK", true), (a.cancel, "Cancelar", false)] {
            p.fill(rect, &all, if primary { th.accent } else { [255, 255, 255, 255] });
            p.stroke(rect, &all, px, if primary { th.accent } else { th.line });
            p.label(
                label,
                ui,
                rect,
                Align::Center,
                0.0,
                if primary { [255, 255, 255, 255] } else { th.text },
            );
        }
    }
}

/// Chave de ordenação de uma célula (ordem do Excel).
#[derive(Clone, Debug, PartialEq, PartialOrd)]
enum Key {
    Num(f64),
    Text(String),
    Bool(bool),
    Err(u8),
    Empty,
}

fn sort_key(v: Value) -> Key {
    match v {
        Value::Number(x) => Key::Num(x),
        Value::Text("") => Key::Empty,
        Value::Text(t) => Key::Text(fold(t)),
        Value::Bool(b) => Key::Bool(b),
        Value::Error(e) => Key::Err(e as u8),
        Value::Empty => Key::Empty,
    }
}

impl SheetsApp {
    /// Botões de filtro visíveis: (retângulo, filtro, coluna).
    pub fn filter_buttons(&self, g: &Geom) -> Vec<(Rect, Active, u32)> {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return Vec::new() };
        let mut out = Vec::new();
        for f in filters(sheet) {
            let r = f.filter.range;
            for c in r.first_col..=r.last_col {
                let Some(cell) = crate::draw::cell_screen_rect(g, v, r.first_row, c) else { continue };
                if v.merges.at(r.first_row, c).is_some_and(|m| m.first_col != c) {
                    continue;
                }
                let size = (16.0 * g.g).round().min(cell.h() - 2.0).max(6.0);
                let b = Rect::new(cell.x1 - size - 2.0, cell.y1 - size - 2.0, cell.x1 - 2.0, cell.y1 - 2.0);
                if b.x0 > cell.x0 {
                    out.push((b, f.clone(), c));
                }
            }
        }
        out
    }
}

/// Desenha os botões de filtro (ver [`SheetsApp::filter_buttons`]).
pub fn draw_buttons(p: &mut Painter, g: &Geom, buttons: &[(Rect, Active, u32)], th: &crate::ribbon::Theme) {
    {
        let s = g.g;
        let px = g.s.round().max(1.0);
        for (b, f, c) in buttons {
            let (b, c) = (*b, *c);
            let clip = g.grid;
            let filtered = f.filter.columns.iter().any(|x| f.filter.range.first_col + x.col == c);
            p.fill(b, &clip, if filtered { th.accent } else { [255, 255, 255, 255] });
            p.stroke(b, &clip, px, if filtered { th.accent } else { th.muted });
            let fg = if filtered { [255, 255, 255, 255] } else { th.text };
            let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
            for i in 0..3 {
                let w = (6.0 - 2.0 * i as f32) * s;
                let y = (cy - 1.0 * s + i as f32 * s).round();
                p.fill(
                    Rect::new((cx - w / 2.0).round(), y, (cx + w / 2.0).round(), y + s.max(1.0)),
                    &clip,
                    fg,
                );
            }
        }
    }
}

impl SheetsApp {
    /// Abre o painel do filtro `f` na coluna `col`, embaixo do botão `b`.
    pub fn open_filter_panel(&mut self, f: Active, col: u32, b: Rect) {
        self.commit_edit();
        let g = self.geom();
        let Some(doc) = self.doc.as_ref() else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r0, r1) = f.data_rows();
        let cp = compile(&f.filter, Some(col - f.filter.range.first_col));
        let (strings, looks, d1904) = (&*doc.strings, &*doc.looks, doc.date1904);
        // Valores distintos (texto mostrado), em paralelo por pedaços de linhas.
        let chunks: Vec<u32> = (r0..=r1).step_by(CHUNK as usize).collect();
        let parts: Vec<(HashMap<String, Option<f64>>, bool)> = chunks
            .par_iter()
            .map(|&a| {
                let mut map = HashMap::new();
                let mut blank = false;
                let mut buf = String::new();
                let mut b2 = String::new();
                for r in a..(a + CHUNK).min(r1 + 1) {
                    if !passes(&cp, sheet, strings, looks, v, d1904, r, &mut b2) {
                        continue;
                    }
                    if shown(sheet, strings, looks, v, d1904, r, col, &mut buf) {
                        if !map.contains_key(&buf) {
                            let num = match sheet.cell(r, col, strings).value {
                                Value::Number(x) => Some(x),
                                _ => None,
                            };
                            map.insert(buf.clone(), num);
                        }
                    } else {
                        blank = true;
                    }
                }
                (map, blank)
            })
            .collect();
        let mut all: HashMap<String, Option<f64>> = HashMap::new();
        let mut blank = false;
        for (m, b) in parts {
            blank |= b;
            for (k, n) in m {
                all.entry(k).or_insert(n);
            }
        }
        let current =
            f.filter.columns.iter().find(|x| f.filter.range.first_col + x.col == col).map(|x| &x.kind);
        let chosen: Option<(HashSet<String>, bool)> = match current {
            Some(FilterKind::Values { values, blank }) => {
                Some((values.iter().map(|v| v.to_lowercase()).collect(), *blank))
            }
            _ => None,
        };
        let mut items: Vec<Item> = all
            .into_iter()
            .map(|(text, num)| {
                let checked = chosen.as_ref().is_none_or(|c| c.0.contains(&text.to_lowercase()));
                Item { text, num, checked }
            })
            .collect();
        items.par_sort_by(|a, b| match (a.num, b.num) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => fold(&a.text).cmp(&fold(&b.text)),
        });
        let truncated = items.len() > MAX_ITEMS;
        items.truncate(MAX_ITEMS);
        let blank = blank.then(|| chosen.as_ref().is_none_or(|c| c.1));
        let s = g.s;
        let w = (W * s).round();
        let h = Panel::height(s).round();
        let x0 = b.x1.min(g.w) - w;
        let x0 = x0.clamp(0.0, (g.w - w).max(0.0));
        let y0 = (b.y1 + 2.0).min((g.status.y0 - h).max(0.0));
        self.filter_panel = Some(Panel {
            active: f,
            col,
            items,
            blank,
            search: TextInput::new(),
            scroll: 0,
            rect: Rect::new(x0, y0, x0 + w, y0 + h),
            truncated,
        });
    }

    /// OK no painel: grava o critério e esconde/mostra as linhas.
    pub fn apply_filter_panel(&mut self) {
        let Some(panel) = self.filter_panel.take() else { return };
        let mut af = panel.active.filter.clone();
        let k = panel.col - af.range.first_col;
        af.columns.retain(|c| c.col != k);
        if let Some(kind) = panel.criterion() {
            af.columns.push(FilterColumn { col: k, kind });
            af.columns.sort_by_key(|c| c.col);
        }
        self.set_filter(panel.active.target, Some(af), None);
    }

    /// Grava o filtro e acerta as linhas escondidas (com `pre` antes, ex.:
    /// classificar), num passo só.
    pub fn set_filter(&mut self, target: Target, af: Option<AutoFilter>, pre: Option<Command>) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some((sheet, v)) = doc.current() else { return };
        // Linhas a esconder ou mostrar depois do `pre` (que só reordena: o
        // filtro é avaliado no modelo depois de reordenar, abaixo).
        let mut cmds = Vec::new();
        if let Some(c) = pre {
            cmds.push(c);
        }
        let table = match target {
            Target::Sheet => None,
            Target::Table(i) => Some(i),
        };
        cmds.push(Command::SetFilter { table, filter: af.clone() });
        let range = af.as_ref().map(|f| f.range).or_else(|| match target {
            Target::Sheet => sheet.layout.autofilter.as_ref().map(|f| f.range),
            Target::Table(i) => sheet.layout.tables.get(i as usize).map(|t| t.range),
        });
        let _ = v;
        if !self.run_command(Command::Group(cmds)) {
            return;
        }
        // Agora (depois de reordenar), quais linhas ficam escondidas; entra
        // no mesmo passo de desfazer.
        let Some(range) = range else { return };
        let changes = self.filter_changes(range, af.as_ref());
        let n = changes.iter().filter(|c| c.1).count();
        if !changes.is_empty() && self.run_command(Command::RowsHidden(changes)) {
            self.merge_last_steps();
        }
        if af.as_ref().is_some_and(|f| !f.columns.is_empty()) {
            self.message =
                format!("Filtro aplicado ({} linhas escondidas agora)", crate::app::fmt_int(n as u64));
        }
    }

    /// Linhas de dados de `range` cuja visibilidade muda com o filtro `af`.
    fn filter_changes(&self, range: CellRange, af: Option<&AutoFilter>) -> Vec<(u32, bool)> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let Some((sheet, v)) = doc.current() else { return Vec::new() };
        let (r0, r1) = (range.first_row + 1, range.last_row);
        if r0 > r1 {
            return Vec::new();
        }
        let has_other = af.is_some_and(|f| f.columns.iter().any(|c| matches!(c.kind, FilterKind::Other(_))));
        let cp = af.map(|f| compile(f, None));
        let (strings, looks, d1904) = (&*doc.strings, &*doc.looks, doc.date1904);
        let chunks: Vec<u32> = (r0..=r1).step_by(CHUNK as usize).collect();
        chunks
            .par_iter()
            .flat_map_iter(|&a| {
                let mut buf = String::new();
                let mut out = Vec::new();
                for r in a..(a + CHUNK).min(r1 + 1) {
                    let now = sheet.row_hidden(r);
                    let pass =
                        cp.as_ref().is_none_or(|cp| passes(cp, sheet, strings, looks, v, d1904, r, &mut buf));
                    // Critério que não sabemos avaliar: o que já estava escondido fica.
                    let hide = !pass || (has_other && now);
                    if hide != now {
                        out.push((r, hide));
                    }
                }
                out
            })
            .collect()
    }

    /// Classifica as linhas de dados do filtro `f` pela coluna `col`.
    pub fn sort_filter(&mut self, f: Active, col: u32, desc: bool) {
        let (r0, r1) = f.data_rows();
        let range = f.filter.range;
        let Some(perm) = self.sort_perm(r0, r1, col, desc) else { return };
        let cmd = Command::SortRows { r0, c0: range.first_col, c1: range.last_col, perm };
        self.set_filter(f.target, Some(f.filter), Some(cmd));
    }

    /// Ordem das linhas `r0..=r1` pela coluna `col` (estável, em paralelo).
    pub fn sort_perm(&mut self, r0: u32, r1: u32, col: u32, desc: bool) -> Option<Vec<u32>> {
        if r1 <= r0 {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let (sheet, _) = doc.current()?;
        let strings = &*doc.strings;
        let keys: Vec<Key> =
            (r0..=r1).into_par_iter().map(|r| sort_key(sheet.cell(r, col, strings).value)).collect();
        let mut perm: Vec<u32> = (0..keys.len() as u32).collect();
        perm.par_sort_by(|&a, &b| {
            let (x, y) = (&keys[a as usize], &keys[b as usize]);
            // Vazias sempre no fim, nas duas ordens.
            match (x == &Key::Empty, y == &Key::Empty) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => {
                    let o = x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal);
                    if desc { o.reverse() } else { o }
                }
            }
        });
        if perm.iter().enumerate().all(|(i, &p)| i as u32 == p) {
            self.message = "Já está em ordem".into();
            return None;
        }
        Some(perm)
    }

    /// Classifica pela coluna da célula ativa: dentro do filtro ou tabela que
    /// a contém, senão na região contínua de dados em volta (com cabeçalho
    /// se a primeira linha for só texto).
    pub fn sort_here(&mut self, desc: bool) {
        self.commit_edit();
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let (r, c) = v.cur;
        if let Some(f) = filters(sheet).into_iter().find(|f| f.filter.range.contains(r, c)) {
            self.sort_filter(f, c, desc);
            return;
        }
        let Some(reg) = current_region(sheet, r, c) else { return };
        let header = (reg.first_col..=reg.last_col).all(|col| {
            matches!(
                sheet.cell(reg.first_row, col, &self.doc.as_ref().unwrap().strings).value,
                Value::Text(_) | Value::Empty
            )
        }) && reg.last_row > reg.first_row;
        let r0 = if header { reg.first_row + 1 } else { reg.first_row };
        let Some(perm) = self.sort_perm(r0, reg.last_row, c, desc) else { return };
        self.run_command(Command::SortRows { r0, c0: reg.first_col, c1: reg.last_col, perm });
    }

    /// Liga/desliga o filtro da aba na região da célula ativa.
    pub fn toggle_filter(&mut self) {
        self.commit_edit();
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let (r, c) = v.cur;
        if let Some(f) = filters(sheet).into_iter().find(|f| f.filter.range.contains(r, c)) {
            match f.target {
                // Desligar: as linhas voltam e o filtro sai.
                Target::Sheet => self.set_filter(Target::Sheet, None, None),
                Target::Table(_) => self.clear_filters(),
            }
            return;
        }
        if sheet.layout.autofilter.is_some() {
            self.set_filter(Target::Sheet, None, None);
            return;
        }
        // Várias células selecionadas: o filtro vale para a seleção.
        let (r0, c0, r1, c1) = v.sel();
        let reg = if (r0, c0) != (r1, c1) && r1 - r0 + 1 < v.rows.len() {
            Some(CellRange { first_row: r0, first_col: c0, last_row: r1, last_col: c1 })
        } else {
            current_region(sheet, r, c)
        };
        let Some(reg) = reg else {
            self.message = "Selecione uma célula dentro dos dados".into();
            return;
        };
        self.set_filter(Target::Sheet, Some(AutoFilter { range: reg, columns: Vec::new() }), None);
    }

    /// Tira os critérios (do filtro que contém a célula ativa) e mostra as linhas.
    pub fn clear_filters(&mut self) {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let (r, c) = v.cur;
        let Some(f) = filters(sheet)
            .into_iter()
            .find(|f| f.filter.range.contains(r, c))
            .or_else(|| filters(sheet).into_iter().next())
        else {
            return;
        };
        let mut af = f.filter.clone();
        af.columns.clear();
        self.set_filter(f.target, Some(af), None);
    }

    /// Reaplica o filtro (depois de editar os dados).
    pub fn reapply_filter(&mut self) {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let (r, c) = v.cur;
        let Some(f) = filters(sheet)
            .into_iter()
            .find(|f| f.filter.range.contains(r, c))
            .or_else(|| filters(sheet).into_iter().next())
        else {
            return;
        };
        self.set_filter(f.target, Some(f.filter), None);
    }
}

/// A região contínua de dados em volta de (`r`, `c`), como a "região atual"
/// do Excel (cresce enquanto houver valores encostados).
pub fn current_region(sheet: &Sheet, r: u32, c: u32) -> Option<CellRange> {
    let has = |r: u32, c: u32| sheet.has_value(r, c);
    let mut g = CellRange { first_row: r, first_col: c, last_row: r, last_col: c };
    let max_r = sheet.rows().max(1) - 1;
    let max_c = sheet.cols().max(1) - 1;
    for _ in 0..8 {
        let before = g;
        // Linhas: cresce enquanto a linha vizinha tiver algo na faixa de colunas.
        let row_has = |row: u32, g: &CellRange| {
            (g.first_col.saturating_sub(1)..=(g.last_col + 1).min(max_c)).any(|cc| has(row, cc))
        };
        while g.first_row > 0 && row_has(g.first_row - 1, &g) {
            g.first_row -= 1;
        }
        while g.last_row < max_r && row_has(g.last_row + 1, &g) {
            g.last_row += 1;
        }
        // Colunas: uma coluna vizinha com algo na faixa de linhas (varre só
        // as linhas com valor, pulando blocos vazios).
        let col_has = |col: u32, g: &CellRange| {
            let mut found = false;
            sheet.for_each_value(
                col,
                g.first_row.saturating_sub(1)..(g.last_row + 2).min(max_r + 1),
                &duck_core::StringTable::new(),
                |_, _| {
                    found = true;
                    false
                },
            );
            found
        };
        while g.first_col > 0 && col_has(g.first_col - 1, &g) {
            g.first_col -= 1;
        }
        while g.last_col < max_c && col_has(g.last_col + 1, &g) {
            g.last_col += 1;
        }
        if g == before {
            break;
        }
    }
    (g.first_row != g.last_row || g.first_col != g.last_col || has(r, c)).then_some(g)
}
