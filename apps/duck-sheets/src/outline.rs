//! Agrupamento de linhas e colunas: as faixas ao lado dos cabeçalhos, com os
//! botões `+`/`−` de cada grupo e os números dos níveis, como no Excel.

use duck_core::outline::{Group, Outline};
use duck_core::{Command, Sheet};
use duck_text::TextStyle;
use duck_ui::{Align, Painter, Rect, Rgba, rgb};

use crate::app::{Geom, View};

const STEP: f32 = 14.0;
const LINE: Rgba = rgb(0x7F7F7F);
const BOX: Rgba = rgb(0xFFFFFF);
const BG: Rgba = rgb(0xF3F4F6);

/// Larguras das faixas (linhas, colunas) em pixels; 0 sem grupos.
pub fn gutters(o: &Outline, s: f32) -> (f32, f32) {
    let w = |max: u8| if max == 0 { 0.0 } else { ((max as f32 + 1.0) * STEP * s + 4.0 * s).round() };
    (w(o.rows.max()), w(o.cols.max()))
}

/// O que há no ponto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// Botão de um grupo (linhas = `true`).
    Toggle(bool, Group),
    /// Número do nível: mostra até ele.
    Level(bool, u8),
}

/// Um eixo na tela: onde começa o índice `i` e os trechos visíveis.
struct Strip<'a> {
    v: &'a View,
    g: &'a Geom,
    rows: bool,
}

impl Strip<'_> {
    fn groups(&self) -> &[Group] {
        if self.rows { &self.v.row_groups } else { &self.v.col_groups }
    }

    fn offset(&self, i: u32) -> f64 {
        let ax = if self.rows { &self.v.rows } else { &self.v.cols };
        ax.offset(i.min(ax.len())) as f64
    }

    /// Trechos da tela: (início, fim, deslocamento da aba, primeiro índice, fim).
    fn bands(&self) -> [(f32, f32, f64, u32, u32); 2] {
        let (a, b) = (&self.g.quads[0], &self.g.quads[3]);
        if self.rows {
            [
                (a.rect.y0, a.rect.y1, a.oy, a.row_start, a.row_end),
                (b.rect.y0, b.rect.y1, b.oy, b.row_start, b.row_end),
            ]
        } else {
            [
                (a.rect.x0, a.rect.x1, a.ox, a.col_start, a.col_end),
                (b.rect.x0, b.rect.x1, b.ox, b.col_start, b.col_end),
            ]
        }
    }

    /// Posição na tela do começo do índice `i` no trecho `k`.
    fn at(&self, k: usize, i: u32) -> f32 {
        let (a, _, o, ..) = self.bands()[k];
        a + ((self.offset(i) - o) * self.g.g as f64) as f32
    }

    fn after(&self) -> bool {
        if self.rows { self.v.outline.below } else { self.v.outline.right }
    }

    /// Centro da faixa do nível (na direção do cabeçalho).
    fn lane(&self, level: u8) -> f32 {
        let base = if self.rows { 0.0 } else { self.g.fbar.y1 };
        (base + (level as f32 - 0.5) * STEP * self.g.s + 2.0 * self.g.s).round()
    }

    /// Quadrado do botão de um grupo, se aparece na tela.
    fn button(&self, g: &Group) -> Option<Rect> {
        let i = g.button(self.after())?;
        let half = (5.0 * self.g.s).round();
        for (k, &(a, b, _, first, end)) in self.bands().iter().enumerate() {
            if b <= a || !(first..end).contains(&i) {
                continue;
            }
            let c = ((self.at(k, i) + self.at(k, i + 1)) / 2.0).round();
            if c - half < a - 1.0 || c + half > b + 1.0 {
                continue;
            }
            let l = self.lane(g.level);
            let (x, y) = if self.rows { (l, c) } else { (c, l) };
            return Some(Rect::new(x - half, y - half, x + half, y + half));
        }
        None
    }

    fn collapsed(&self, g: &Group) -> bool {
        self.offset(g.last + 1) - self.offset(g.first) < 0.5
    }

    /// Quadrados dos números dos níveis.
    fn levels(&self) -> Vec<(u8, Rect)> {
        let o = &self.v.outline;
        let max = if self.rows { o.rows.max() } else { o.cols.max() };
        if max == 0 {
            return Vec::new();
        }
        let (s, half) = (self.g.s, (6.0 * self.g.s).round());
        (1..=max + 1)
            .map(|l| {
                let lane = self.lane(l);
                let (x, y) = if self.rows {
                    (lane, ((self.g.colhdr.y0 + self.g.colhdr.y1) / 2.0).round())
                } else {
                    (((self.g.rowhdr.x0 + self.g.rowhdr.x1) / 2.0).max(8.0 * s).round(), lane)
                };
                (l, Rect::new(x - half, y - half, x + half, y + half))
            })
            .collect()
    }
}

pub fn hit(g: &Geom, v: &View, x: f32, y: f32) -> Option<Hit> {
    for rows in [true, false] {
        let st = Strip { v, g, rows };
        if st.groups().is_empty() {
            continue;
        }
        if let Some((l, _)) = st.levels().into_iter().find(|b| b.1.contains(x, y)) {
            return Some(Hit::Level(rows, l));
        }
        let gr = st.groups().iter().find(|gr| st.button(gr).is_some_and(|b| b.contains(x, y)));
        if let Some(gr) = gr {
            return Some(Hit::Toggle(rows, *gr));
        }
    }
    None
}

pub fn draw(p: &mut Painter, g: &Geom, v: &View, ui: TextStyle) {
    let (px, all) = (g.s.round().max(1.0), Rect::everything());
    let small = ui.with_px((10.0 * g.s).round());
    for rows in [true, false] {
        let st = Strip { v, g, rows };
        if st.groups().is_empty() {
            continue;
        }
        let area = if rows {
            Rect::new(0.0, g.fbar.y1, g.rowhdr.x0, g.grid.y1)
        } else {
            Rect::new(0.0, g.fbar.y1, g.grid.x1, g.colhdr.y0)
        };
        p.fill(area, &all, BG);
        for gr in st.groups() {
            let closed = st.collapsed(gr);
            let lane = st.lane(gr.level);
            if !closed {
                for (k, &(a, b, _, first, end)) in st.bands().iter().enumerate() {
                    let (f, l) = (gr.first.max(first), (gr.last + 1).min(end));
                    if b <= a || f >= l {
                        continue;
                    }
                    let (p0, p1) = (st.at(k, f).max(a), st.at(k, l).min(b));
                    if p1 <= p0 {
                        continue;
                    }
                    let (bar, tick) = if rows {
                        (
                            Rect::new(lane - px / 2.0, p0, lane + px / 2.0, p1),
                            Rect::new(lane, p0 + 2.0 * px, lane + 5.0 * g.s, p0 + 3.0 * px),
                        )
                    } else {
                        (
                            Rect::new(p0, lane - px / 2.0, p1, lane + px / 2.0),
                            Rect::new(p0 + 2.0 * px, lane, p0 + 3.0 * px, lane + 5.0 * g.s),
                        )
                    };
                    p.fill(bar, &all, LINE);
                    if f == gr.first {
                        p.fill(tick, &all, LINE);
                    }
                }
            }
            if let Some(b) = st.button(gr) {
                p.fill(b, &all, BOX);
                p.stroke(b, &all, px, LINE);
                let (cx, cy) = (((b.x0 + b.x1) / 2.0).floor(), ((b.y0 + b.y1) / 2.0).floor());
                let r = (2.5 * g.s).round();
                p.fill(Rect::new(cx - r, cy, cx + r + px, cy + px), &all, LINE);
                if closed {
                    p.fill(Rect::new(cx, cy - r, cx + px, cy + r + px), &all, LINE);
                }
            }
        }
        for (l, b) in st.levels() {
            p.fill(b, &all, BOX);
            p.stroke(b, &all, px, LINE);
            p.label(&l.to_string(), small, b, Align::Center, 0.0, rgb(0x404040));
        }
    }
}

/// Comando que abre ou fecha um grupo. Ao abrir, os grupos de dentro que
/// estavam fechados continuam fechados.
pub fn toggle(sheet: &Sheet, rows: bool, g: Group) -> Option<Command> {
    let mut o = sheet.layout.outline.clone();
    let after = if rows { o.below } else { o.right };
    let hidden = |i: u32| if rows { sheet.row_hidden(i) } else { sheet.col_hidden(i) };
    let close = (g.first..=g.last).any(|i| !hidden(i));
    let groups = if rows { o.rows.groups() } else { o.cols.groups() };
    let set = if rows { &mut o.closed_rows } else { &mut o.closed_cols };
    let mut list: Vec<(u32, bool)> = Vec::new();
    if close {
        set.extend(g.button(after));
        list.extend((g.first..=g.last).filter(|&i| !hidden(i)).map(|i| (i, true)));
    } else {
        if let Some(b) = g.button(after) {
            set.remove(&b);
        }
        // Grupos fechados de dentro.
        let inner: Vec<&Group> = groups
            .iter()
            .filter(|x| x.level > g.level && x.first >= g.first && x.last <= g.last)
            .filter(|x| x.button(after).is_some_and(|b| set.contains(&b)))
            .collect();
        list.extend(
            (g.first..=g.last)
                .filter(|i| hidden(*i) && !inner.iter().any(|x| (x.first..=x.last).contains(i)))
                .map(|i| (i, false)),
        );
    }
    finish(sheet, rows, list, g.button(after), o)
}

/// Mostra só até o nível `level` (1 = todos os grupos fechados).
pub fn show_level(sheet: &Sheet, rows: bool, level: u8) -> Option<Command> {
    let mut o = sheet.layout.outline.clone();
    let after = if rows { o.below } else { o.right };
    let lv = if rows { o.rows.clone() } else { o.cols.clone() };
    let hidden = |i: u32| if rows { sheet.row_hidden(i) } else { sheet.col_hidden(i) };
    let set = if rows { &mut o.closed_rows } else { &mut o.closed_cols };
    let mut list = Vec::new();
    let mut buttons = Vec::new();
    for g in lv.groups() {
        if let Some(b) = g.button(after) {
            buttons.push(b);
            if g.level >= level {
                set.insert(b);
            } else {
                set.remove(&b);
            }
        }
    }
    for &(a, b, l) in &lv.0 {
        let hide = l >= level;
        list.extend((a..=b).filter(|&i| hidden(i) != hide).map(|i| (i, hide)));
    }
    finish(sheet, rows, list, buttons, o)
}

fn finish(
    sheet: &Sheet,
    rows: bool,
    mut list: Vec<(u32, bool)>,
    buttons: impl IntoIterator<Item = u32>,
    o: Outline,
) -> Option<Command> {
    if list.is_empty() && o == sheet.layout.outline {
        return None;
    }
    // As linhas de resumo entram sem mudar: o gravador regrava o `collapsed`.
    for b in buttons {
        if !list.iter().any(|x| x.0 == b) {
            let h = if rows { sheet.row_hidden(b) } else { sheet.col_hidden(b) };
            list.push((b, h));
        }
    }
    list.sort_unstable();
    let hide = if rows { Command::RowsHidden(list) } else { Command::ColsHidden(list) };
    Some(Command::Group(vec![hide, Command::Outline(Box::new(o))]))
}

impl crate::app::SheetsApp {
    /// Clique nas faixas de grupos. `true` = era um botão.
    pub fn outline_click(&mut self, g: &Geom, x: f32, y: f32) -> bool {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return false };
        let Some(h) = hit(g, v, x, y) else { return false };
        let cmd = match h {
            Hit::Toggle(rows, gr) => toggle(sheet, rows, gr),
            Hit::Level(rows, l) => show_level(sheet, rows, l),
        };
        if let Some(c) = cmd {
            self.run_command(c);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet() -> Sheet {
        let mut s = Sheet::new("A");
        // 1–8 no nível 1, com 3–5 no nível 2; resumos em 9 e 6.
        for i in 1..=8 {
            s.layout.outline.rows.push(i, i, if (3..=5).contains(&i) { 2 } else { 1 });
        }
        s
    }

    fn hidden(s: &Sheet) -> Vec<u32> {
        (0..12).filter(|&r| s.row_hidden(r)).collect()
    }

    #[test]
    fn inner_group_stays_closed() {
        let mut s = sheet();
        let g = s.layout.outline.rows.groups();
        toggle(&s, true, g[1]).unwrap().apply(&mut s);
        assert_eq!(hidden(&s), [3, 4, 5]);
        toggle(&s, true, g[0]).unwrap().apply(&mut s);
        assert_eq!(hidden(&s), (1..=8).collect::<Vec<_>>());
        let undo = toggle(&s, true, g[0]).unwrap().apply(&mut s);
        assert_eq!(hidden(&s), [3, 4, 5]);
        assert!(s.layout.outline.closed_rows.contains(&6) && !s.layout.outline.closed_rows.contains(&9));
        undo.apply(&mut s);
        assert_eq!(hidden(&s), (1..=8).collect::<Vec<_>>());
        assert!(s.layout.outline.closed_rows.contains(&9));
    }

    #[test]
    fn levels() {
        let mut s = sheet();
        show_level(&s, true, 2).unwrap().apply(&mut s);
        assert_eq!(hidden(&s), [3, 4, 5]);
        show_level(&s, true, 1).unwrap().apply(&mut s);
        assert_eq!(hidden(&s).len(), 8);
        show_level(&s, true, 3).unwrap().apply(&mut s);
        assert!(hidden(&s).is_empty() && s.layout.outline.closed_rows.is_empty());
        assert!(show_level(&s, true, 3).is_none());
    }
}
