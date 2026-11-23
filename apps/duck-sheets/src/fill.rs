//! Alça de preenchimento (arrastar e duplo clique), Ctrl+D e Ctrl+R, e o
//! arraste da borda da seleção (mover células).
//!
//! A continuação das listas (números, datas, meses, dias da semana) fica no
//! `duck_core::fill`; aqui estão a geometria, as células e o comando.

use duck_core::fill::{Filler, Item, Mode, NumKind, Out};
use duck_core::{CellChange, CellRange, CellValue, CfCtx, Command, NumFmt, RawCell, Sheet, Value, kind};
use duck_ui::{Painter, Rect};

use crate::app::{Doc, Geom, MAX_COLS, MAX_ROWS, SheetsApp, View};
use crate::theme::Theme;

/// Máximo de células preenchidas de uma vez.
const MAX_FILL: u64 = 2_000_000;

pub fn range_of((r0, c0, r1, c1): (u32, u32, u32, u32)) -> CellRange {
    CellRange { first_row: r0, first_col: c0, last_row: r1, last_col: c1 }
}

pub fn cells(r: &CellRange) -> u64 {
    (r.last_row - r.first_row + 1) as u64 * (r.last_col - r.first_col + 1) as u64
}

/// O retângulo da seleção na tela, em cada painel em que aparece.
fn on_screen(g: &Geom, v: &View, r: &CellRange) -> Vec<(Rect, Rect)> {
    g.quads
        .iter()
        .filter(|q| !q.rect.is_empty())
        .filter_map(|q| {
            let x = |c: u32| crate::draw::pos(&v.cols, q.ox, c, q.rect.x0, g.g);
            let y = |r: u32| crate::draw::pos(&v.rows, q.oy, r, q.rect.y0, g.g);
            let rect = Rect::new(x(r.first_col), y(r.first_row), x(r.last_col + 1), y(r.last_row + 1));
            let room = Rect::new(q.rect.x0 - 1.0, q.rect.y0 - 1.0, q.rect.x1 + 1.0, q.rect.y1 + 1.0);
            (!rect.intersect(&room).is_empty()).then_some((rect, room))
        })
        .collect()
}

/// A seleção (com a mesclagem inteira, se é uma só) e se pode ser arrastada.
fn selected(v: &View) -> Option<CellRange> {
    if v.multi() {
        return None;
    }
    let s = range_of(v.sel());
    let s = match v.merges.at(s.first_row, s.first_col) {
        Some(m) if m.contains(s.last_row, s.last_col) => m,
        _ => s,
    };
    // Linhas e colunas inteiras: sem alça.
    let whole = s.last_row - s.first_row + 1 >= v.rows.len() || s.last_col - s.first_col + 1 >= v.cols.len();
    (!whole).then_some(s)
}

/// O quadradinho no canto da seleção.
pub fn handle(g: &Geom, v: &View) -> Option<Rect> {
    let s = selected(v)?;
    let h = (3.5 * g.s).round();
    on_screen(g, v, &s).into_iter().find_map(|(r, room)| {
        room.contains(r.x1, r.y1).then(|| Rect::new(r.x1 - h, r.y1 - h, r.x1 + h, r.y1 + h))
    })
}

/// O ponto está na alça (com uma folga)?
pub fn on_handle(g: &Geom, v: &View, x: f32, y: f32) -> bool {
    handle(g, v).is_some_and(|r| r.inset(-2.0 * g.s).contains(x, y))
}

/// O ponto está na borda da seleção (para mover)?
pub fn on_border(g: &Geom, v: &View, x: f32, y: f32) -> bool {
    if !g.grid.contains(x, y) {
        return false;
    }
    let Some(s) = selected(v) else { return false };
    let d = 3.0 * g.s;
    on_screen(g, v, &s).iter().any(|(r, room)| {
        let near = r.inset(-d);
        room.contains(x, y)
            && near.contains(x, y)
            && ((x - r.x0).abs() <= d
                || (x - r.x1).abs() <= d
                || (y - r.y0).abs() <= d
                || (y - r.y1).abs() <= d)
    })
}

pub fn draw_handle(p: &mut Painter, g: &Geom, v: &View, th: &Theme) {
    let Some(r) = handle(g, v) else { return };
    let px = g.s.round().max(1.0);
    let clip = Rect::new(g.grid.x0, g.grid.y0, g.grid.x1, g.grid.y1);
    p.fill(r.inset(-px), &clip, th.paper);
    p.fill(r, &clip, th.selection);
}

/// Até onde o arraste da alça leva a origem `src` com o mouse em (`r`, `c`):
/// uma direção só, a que sai mais da origem.
pub fn fill_target(src: &CellRange, r: u32, c: u32) -> Option<CellRange> {
    let down = r.saturating_sub(src.last_row);
    let up = src.first_row.saturating_sub(r);
    let right = c.saturating_sub(src.last_col);
    let left = src.first_col.saturating_sub(c);
    let (v, h) = (down.max(up), right.max(left));
    if v == 0 && h == 0 {
        return None;
    }
    let mut t = *src;
    if v >= h {
        t.first_row = src.first_row.min(r);
        t.last_row = src.last_row.max(r);
    } else {
        t.first_col = src.first_col.min(c);
        t.last_col = src.last_col.max(c);
    }
    Some(t)
}

/// Onde o bloco `src` fica com o mouse em (`r`, `c`), pego em `grab`
/// (deslocamento dentro do bloco).
pub fn move_target(src: &CellRange, grab: (u32, u32), r: u32, c: u32) -> CellRange {
    let (h, w) = (src.last_row - src.first_row, src.last_col - src.first_col);
    let r0 = r.saturating_sub(grab.0).min(MAX_ROWS - 1 - h);
    let c0 = c.saturating_sub(grab.1).min(MAX_COLS - 1 - w);
    CellRange { first_row: r0, first_col: c0, last_row: r0 + h, last_col: c0 + w }
}

/// Data, hora ou número, pelo formato da célula.
fn num_kind(doc: &Doc, style: u16) -> NumKind {
    let id = doc.styles.xfs.get(style as usize).map_or(0, |x| x.num_fmt);
    let code = doc.styles.num_fmt_code(id);
    let f = NumFmt::parse(code);
    if f.is_percent() {
        return NumKind::Percent;
    }
    if !f.is_date() {
        return NumKind::Number;
    }
    // Só hora: sem dia, mês por extenso nem ano.
    let lower = code.to_lowercase();
    let date =
        lower.contains('d') || lower.contains('y') || lower.contains("mmm") || !lower.contains(['h', 's']);
    if date { NumKind::Date } else { NumKind::Time }
}

fn item(doc: &Doc, sheet: &Sheet, r: u32, c: u32) -> Item {
    let cell = sheet.cell(r, c, &doc.strings);
    let value = match cell.value {
        Value::Number(x) => CellValue::Number(x),
        Value::Text(t) => CellValue::Text(t.to_string()),
        Value::Bool(b) => CellValue::Bool(b),
        Value::Error(e) => CellValue::Error(e),
        Value::Empty => CellValue::Empty,
    };
    let num = if matches!(value, CellValue::Number(_)) { num_kind(doc, cell.style) } else { NumKind::Number };
    Item { value, num, formula: sheet.formula(r, c).is_some() }
}

impl SheetsApp {
    /// Preenche `target` (que contém `src`) a partir de `src`. `toggle` troca
    /// entre copiar e continuar a série (Ctrl ao arrastar).
    pub fn fill_to(&mut self, src: CellRange, target: CellRange, mode: Mode, toggle: bool) -> bool {
        self.commit_edit();
        let today = self.today;
        let Some(doc) = &self.doc else { return false };
        let Some((sheet, v)) = doc.current() else { return false };
        let vertical = target.first_col == src.first_col && target.last_col == src.last_col;
        let grown = cells(&target) - cells(&src);
        if grown == 0 {
            return false;
        }
        if grown > MAX_FILL {
            self.message = format!("Preenchimento grande demais ({} células)", crate::app::fmt_int(grown));
            return false;
        }
        let outside = duck_core::cells::subtract(&target, &src);
        if v.merges.in_rows(target.first_row, target.last_row).any(|m| {
            outside.iter().any(|o| duck_core::cells::intersect(m, o).is_some())
                && !(m.first_row >= src.first_row
                    && m.last_row <= src.last_row
                    && m.first_col >= src.first_col
                    && m.last_col <= src.last_col)
        }) {
            self.message = "O destino tem células mescladas".into();
            return false;
        }
        // Linhas escondidas por filtro ficam de fora, como no Excel.
        let filtered = sheet.layout.autofilter.is_some()
            || sheet.layout.tables.iter().any(|t| t.filter.as_ref().is_some_and(|f| !f.columns.is_empty()));
        let skip = |r: u32| filtered && sheet.row_hidden(r);
        let rows = |a: u32, b: u32| -> Vec<u32> { (a..=b).filter(|&r| !skip(r)).collect() };
        // Posições ao longo do preenchimento (linhas ou colunas) e as faixas.
        let (along, start, n, lines): (Vec<u32>, usize, usize, Vec<u32>) = if vertical {
            let all = rows(target.first_row, target.last_row);
            let inside = |r: &u32| (src.first_row..=src.last_row).contains(r);
            let start = all.iter().position(inside).unwrap_or(0);
            let n = all.iter().filter(|r| inside(r)).count();
            (all, start, n, (src.first_col..=src.last_col).collect())
        } else {
            let all: Vec<u32> = (target.first_col..=target.last_col).collect();
            let start = (src.first_col - target.first_col) as usize;
            let n = (src.last_col - src.first_col + 1) as usize;
            (all, start, n, rows(src.first_row, src.last_row))
        };
        if n == 0 {
            return false;
        }
        let at = |line: u32, pos: u32| if vertical { (pos, line) } else { (line, pos) };
        let cfx = CfCtx { sheet, shared: &doc.strings, today };
        let mut raws: Vec<(u32, u32, RawCell)> = Vec::new();
        let mut changes: Vec<CellChange> = Vec::new();
        let mut series = false;
        for &line in &lines {
            let items: Vec<Item> = along[start..start + n]
                .iter()
                .map(|&p| {
                    let (r, c) = at(line, p);
                    item(doc, sheet, r, c)
                })
                .collect();
            let mut f = Filler::new(items.clone(), mode, doc.date1904);
            if toggle {
                let copies = (0..n as i64).all(|i| f.at(i + n as i64).1 == Out::Copy);
                f = Filler::new(items, if copies { Mode::Series } else { Mode::Copy }, doc.date1904);
            }
            for (k, &p) in along.iter().enumerate() {
                if (start..start + n).contains(&k) {
                    continue;
                }
                let (from, out) = f.at(k as i64 - start as i64);
                let (sr, sc) = at(line, along[start + from]);
                let (r, c) = at(line, p);
                let raw = sheet.raw(sr, sc);
                let style = if raw.style == 0 { v.blank_style(sr, sc) } else { raw.style };
                match (out, duck_calc::cell_formula(sheet, sr, sc)) {
                    (Out::Value(value), _) => {
                        series = true;
                        changes.push(CellChange { row: r, col: c, value, formula: None, style: Some(style) })
                    }
                    (Out::Copy, Some(f)) => {
                        let f = duck_calc::shift(&f, r as i64 - sr as i64, c as i64 - sc as i64);
                        let value = cfx.eval_formula(&f, r, c).unwrap_or(CellValue::Empty);
                        changes.push(CellChange {
                            row: r,
                            col: c,
                            value,
                            formula: Some(f),
                            style: Some(style),
                        });
                    }
                    // A célula como está: guarda também o texto rico.
                    (Out::Copy, None) => raws.push((
                        r,
                        c,
                        RawCell { kind: raw.kind & kind::TYPE_MASK, style, payload: raw.payload, formula: 0 },
                    )),
                }
            }
        }
        // As mesclagens da origem se repetem.
        let mut add = Vec::new();
        let size = if vertical { src.last_row - src.first_row + 1 } else { src.last_col - src.first_col + 1 };
        if !filtered {
            for m in v.merges.in_rows(src.first_row, src.last_row).copied().collect::<Vec<_>>() {
                if m.first_col < src.first_col || m.last_col > src.last_col || m.first_row < src.first_row {
                    continue;
                }
                let (lo, hi) = if vertical {
                    (target.first_row, target.last_row)
                } else {
                    (target.first_col, target.last_col)
                };
                let (a, b) = if vertical { (m.first_row, m.last_row) } else { (m.first_col, m.last_col) };
                let reach = ((hi - lo) / size + 1) as i64;
                for k in -reach..=reach {
                    let d = k * size as i64;
                    let (na, nb) = (a as i64 + d, b as i64 + d);
                    if k == 0 || na < lo as i64 || nb > hi as i64 {
                        continue;
                    }
                    add.push(if vertical {
                        CellRange { first_row: na as u32, last_row: nb as u32, ..m }
                    } else {
                        CellRange { first_col: na as u32, last_col: nb as u32, ..m }
                    });
                }
            }
        }
        let mut cmds = Vec::new();
        if !raws.is_empty() {
            cmds.push(Command::Restore(raws));
        }
        if !changes.is_empty() {
            cmds.push(Command::SetCells(changes));
        }
        if !add.is_empty() {
            cmds.push(Command::Merges { add, remove: Vec::new() });
        }
        let i = doc.active;
        if !self.run_step(vec![(i, Command::Group(cmds))]) {
            return false;
        }
        if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
            v.anchor = (target.first_row, target.first_col);
            v.cur = (target.last_row, target.last_col);
        }
        self.message = format!("{} células preenchidas", crate::app::fmt_int(grown));
        if series && mode == Mode::Auto && !toggle {
            self.message.push_str(" (com Ctrl, só copia)");
        }
        true
    }

    /// Ctrl+D (`down`) e Ctrl+R: copia a primeira linha (ou coluna) da seleção
    /// no resto; com uma só, a de cima (ou da esquerda).
    pub fn fill_copy(&mut self, down: bool) {
        let Some((r0, c0, r1, c1)) = self.sel_cells(MAX_FILL) else { return };
        let target = range_of((r0, c0, r1, c1));
        let (src, target) = if down {
            if r0 == r1 {
                let Some(p) = r0.checked_sub(1) else { return };
                (range_of((p, c0, p, c1)), range_of((p, c0, r1, c1)))
            } else {
                (CellRange { last_row: r0, ..target }, target)
            }
        } else if c0 == c1 {
            let Some(p) = c0.checked_sub(1) else { return };
            (range_of((r0, p, r1, p)), range_of((r0, p, r1, c1)))
        } else {
            (CellRange { last_col: c0, ..target }, target)
        };
        if self.fill_to(src, target, Mode::Copy, false)
            && let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut())
        {
            v.anchor = (r0, c0);
            v.cur = (r1, c1);
        }
    }

    /// Duplo clique na alça: preenche para baixo até onde vai o bloco vizinho
    /// (coluna da esquerda; senão, a da direita; senão, o que já há embaixo).
    pub fn fill_double(&mut self) {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let Some(src) = selected(v) else { return };
        let end = |c: u32| {
            let mut r = src.last_row;
            while r + 1 < MAX_ROWS && sheet.has_value(r + 1, c) {
                r += 1;
            }
            r
        };
        let mut cols = Vec::new();
        cols.extend(src.first_col.checked_sub(1));
        cols.push(src.last_col + 1);
        let last =
            cols.into_iter().map(end).find(|&r| r > src.last_row).unwrap_or_else(|| end(src.first_col));
        if last == src.last_row {
            self.message = "Nenhum dado ao lado para seguir".into();
            return;
        }
        self.fill_to(src, CellRange { last_row: last, ..src }, Mode::Auto, false);
    }

    /// `--edit` de preencher. `true` = era um deles.
    pub fn scripted_fill(&mut self, e: &str) -> bool {
        let select = |app: &mut Self, r: &str| -> bool {
            let Some((a, b)) = crate::app::parse_range(r) else { return false };
            app.go_to(a.0, a.1);
            if let Some(v) = app.doc.as_mut().and_then(|d| d.view_mut()) {
                v.anchor = a;
                v.cur = b;
            }
            true
        };
        if let Some((op, arg)) = e.split_once('=')
            && let Some(mode) = match op {
                "preencher" => Some(Mode::Auto),
                "preencher-copia" => Some(Mode::Copy),
                "preencher-serie" => Some(Mode::Series),
                _ => None,
            }
        {
            let rg = |s: &str| {
                let (a, b) = crate::app::parse_range(s)?;
                Some(range_of((a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))))
            };
            match arg.split_once('>').and_then(|(a, b)| Some((rg(a)?, rg(b)?))) {
                Some((src, target)) => {
                    self.fill_to(src, target, mode, false);
                }
                None => eprintln!("--edit: use preencher=A1:A2>A1:A10"),
            }
        } else if let Some((op, arg)) = e.split_once(':')
            && matches!(op, "preencher-baixo" | "preencher-direita" | "preencher-duplo")
        {
            if !select(self, arg) {
                eprintln!("--edit: intervalo inválido: {arg}");
                return true;
            }
            match op {
                "preencher-baixo" => self.fill_copy(true),
                "preencher-direita" => self.fill_copy(false),
                _ => self.fill_double(),
            }
        } else {
            return false;
        }
        eprintln!("--edit {e}: {}", self.message);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, get, select};

    fn rg(s: &str) -> CellRange {
        let (a, b) = crate::app::parse_range(s).unwrap();
        range_of((a.0, a.1, b.0, b.1))
    }

    fn col(a: &SheetsApp, c: &str, rows: std::ops::RangeInclusive<u32>) -> Vec<String> {
        rows.map(|r| get(a, 0, &format!("{c}{r}"))).collect()
    }

    #[test]
    fn targets() {
        let s = rg("B2:C3");
        assert_eq!(fill_target(&s, 2, 2), None);
        assert_eq!(fill_target(&s, 9, 1), Some(rg("B2:C10")));
        assert_eq!(fill_target(&s, 0, 1), Some(rg("B1:C3")));
        assert_eq!(fill_target(&s, 2, 7), Some(rg("B2:H3")));
        assert_eq!(fill_target(&s, 5, 4), Some(rg("B2:C6")));
        assert_eq!(move_target(&s, (1, 1), 5, 5), rg("E5:F6"));
        assert_eq!(move_target(&s, (1, 1), 0, 0), rg("A1:B2"));
    }

    #[test]
    fn series_and_undo() {
        let mut a = app(&[(
            "P",
            &[("A1", "1"), ("A2", "3"), ("B1", "jan"), ("C1", "=A1*2"), ("D1", "31/12/2026"), ("E1", "5")],
        )]);
        assert!(a.fill_to(rg("A1:A2"), rg("A1:A5"), Mode::Auto, false));
        assert_eq!(col(&a, "A", 1..=5), ["1", "3", "5", "7", "9"]);
        assert!(a.fill_to(rg("B1:E1"), rg("B1:E3"), Mode::Auto, false));
        assert_eq!(col(&a, "B", 1..=3), ["jan", "fev", "mar"]);
        assert_eq!(col(&a, "C", 1..=3), ["=A1*2", "=A2*2", "=A3*2"]);
        assert_eq!(col(&a, "D", 1..=3), ["46387", "46388", "46389"]);
        assert_eq!(col(&a, "E", 1..=3), ["5", "6", "7"]);
        a.undo();
        assert_eq!(col(&a, "B", 1..=3), ["jan", "", ""]);
        a.redo();
        assert_eq!(col(&a, "C", 1..=3), ["=A1*2", "=A2*2", "=A3*2"]);
        // Ctrl: a série vira cópia, e a porcentagem sozinha vira série.
        assert!(a.fill_to(rg("E1"), rg("E1:E3"), Mode::Auto, true));
        assert_eq!(col(&a, "E", 1..=3), ["5", "5", "5"]);
        crate::testing::type_in(&mut a, "F1", "10%");
        assert!(a.fill_to(rg("F1"), rg("F1:F2"), Mode::Auto, false));
        assert_eq!(col(&a, "F", 1..=2), ["0,1", "0,1"]);
        assert!(a.fill_to(rg("F1"), rg("F1:F2"), Mode::Auto, true));
        assert_eq!(col(&a, "F", 1..=2), ["0,1", "1,1"]);
        assert!(a.fill_to(rg("A1:A2"), rg("A1:A5"), Mode::Auto, true));
        assert_eq!(col(&a, "A", 1..=5), ["1", "3", "1", "3", "1"]);
    }

    #[test]
    fn upwards_and_sideways() {
        let mut a = app(&[("P", &[("C5", "10"), ("D5", "20")])]);
        assert!(a.fill_to(rg("C5:D5"), rg("A5:D5"), Mode::Auto, false));
        assert_eq!((get(&a, 0, "A5"), get(&a, 0, "B5")), ("-10".into(), "0".into()));
        assert!(a.fill_to(rg("C5:D5"), rg("C3:D5"), Mode::Copy, false));
        assert_eq!(col(&a, "D", 3..=5), ["20", "20", "20"]);
    }

    #[test]
    fn copy_down_right_and_double_click() {
        let mut a = app(&[(
            "P",
            &[("A1", "x"), ("A2", "x"), ("A3", "x"), ("A4", "x"), ("B1", "=LIN()"), ("C1", "7")],
        )]);
        select(&mut a, "B1:B1");
        a.fill_double();
        assert_eq!(col(&a, "B", 1..=5), ["=LIN()", "=LIN()", "=LIN()", "=LIN()", ""]);
        select(&mut a, "C1:C3");
        a.fill_copy(true);
        assert_eq!(col(&a, "C", 1..=4), ["7", "7", "7", ""]);
        // Uma linha só: copia a de cima.
        select(&mut a, "C4:C4");
        a.fill_copy(true);
        assert_eq!(get(&a, 0, "C4"), "7");
        select(&mut a, "C1:E1");
        a.fill_copy(false);
        assert_eq!((get(&a, 0, "D1"), get(&a, 0, "E1")), ("7".into(), "7".into()));
        let v = a.doc.as_ref().unwrap().current().unwrap().1;
        assert_eq!(v.sel(), (0, 2, 0, 4));
    }

    #[test]
    fn merges_repeat() {
        let mut a = app(&[("P", &[("A1", "1")])]);
        select(&mut a, "A1:B2");
        a.merge(crate::ribbon::Merge::Cells);
        assert!(a.fill_to(rg("A1:B2"), rg("A1:B6"), Mode::Copy, false));
        let m = &a.doc.as_ref().unwrap().current().unwrap().0.layout.merges;
        assert_eq!(m.as_slice(), [rg("A1:B2"), rg("A3:B4"), rg("A5:B6")]);
        // Destino com mesclagem: recusa.
        assert!(!a.fill_to(rg("C1"), rg("C1:C1"), Mode::Copy, false));
        assert!(!a.fill_to(rg("A7:B7"), rg("A5:B7"), Mode::Copy, false));
        a.undo();
        assert_eq!(a.doc.as_ref().unwrap().current().unwrap().0.layout.merges.len(), 1);
    }
}
