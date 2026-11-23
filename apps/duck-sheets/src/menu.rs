//! Menus suspensos (listas e paleta de cores) e a caixa de diálogo de um
//! campo (largura da coluna, altura da linha), desenhados pelo próprio app.

use duck_text::TextStyle;
use duck_ui::{Align, Painter, Rect, Rgba, TextInput};

use crate::app::Geom;
use crate::format::{Fmt, PALETTE};
use crate::ribbon::Act;

/// Um item de lista. `act = None` é um separador.
#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub key: &'static str,
    pub act: Option<Act>,
    pub checked: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, act: Act) -> Self {
        Self { label: label.into(), key: "", act: Some(act), checked: false }
    }

    pub fn key(mut self, k: &'static str) -> Self {
        self.key = k;
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    pub fn sep() -> Self {
        Self { label: String::new(), key: "", act: None, checked: false }
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    List(Vec<Item>),
    /// Paleta de cores: do texto (`true`) ou do fundo.
    Palette(bool),
}

/// Menu aberto.
#[derive(Clone, Debug)]
pub struct Popup {
    pub kind: Kind,
    pub rect: Rect,
    /// O botão que abriu (clicar nele de novo fecha).
    pub owner: Option<Act>,
}

const ITEM_H: f32 = 26.0;
const SEP_H: f32 = 9.0;
const SWATCH: f32 = 20.0;
const GAP: f32 = 4.0;

impl Popup {
    /// Lista em (`x`, `y`), mantida dentro da janela.
    pub fn list(
        items: Vec<Item>,
        x: f32,
        y: f32,
        g: &Geom,
        text: &mut duck_text::TextSystem,
        ui: TextStyle,
    ) -> Self {
        let s = g.s;
        let w = items
            .iter()
            .map(|i| text.measure_styled(&i.label, ui) + text.measure_styled(i.key, ui) + 70.0 * s)
            .fold(180.0 * s, f32::max)
            .round();
        let h = items.iter().map(|i| if i.act.is_some() { ITEM_H } else { SEP_H }).sum::<f32>() * s + 8.0 * s;
        let x0 = x.min(g.w - w - 4.0 * s).max(0.0);
        let y0 = if y + h > g.status.y1 { (y - h).max(0.0) } else { y };
        Popup { kind: Kind::List(items), rect: Rect::new(x0, y0, x0 + w, y0 + h), owner: None }
    }

    /// Paleta embaixo do retângulo `b`.
    pub fn palette(text_color: bool, b: Rect, g: &Geom) -> Self {
        let s = g.s;
        let w = (10.0 * SWATCH + 11.0 * GAP) * s;
        let h = (3.0 * SWATCH + 4.0 * GAP + 26.0) * s;
        let x0 = b.x0.min(g.w - w - 4.0 * s).max(0.0);
        Popup {
            kind: Kind::Palette(text_color),
            rect: Rect::new(x0, b.y1 + 2.0 * s, x0 + w, b.y1 + 2.0 * s + h),
            owner: None,
        }
    }

    /// Retângulos dos itens de uma lista.
    fn rows(&self, s: f32) -> Vec<(Rect, Option<&Item>)> {
        let Kind::List(items) = &self.kind else { return Vec::new() };
        let mut y = self.rect.y0 + 4.0 * s;
        items
            .iter()
            .map(|it| {
                let h = if it.act.is_some() { ITEM_H } else { SEP_H } * s;
                let r = Rect::new(self.rect.x0, y, self.rect.x1, y + h);
                y += h;
                (r, it.act.is_some().then_some(it))
            })
            .collect()
    }

    /// O que há no ponto (`None` fora do menu ou num separador).
    pub fn hit(&self, g: &Geom, x: f32, y: f32) -> Option<Act> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let s = g.s;
        match &self.kind {
            Kind::List(_) => {
                self.rows(s).into_iter().find(|r| r.0.contains(x, y)).and_then(|r| r.1).and_then(|i| i.act)
            }
            Kind::Palette(text) => {
                let (lx, ly) = ((x - self.rect.x0) / s, (y - self.rect.y0) / s);
                let color = if ly < 26.0 {
                    None
                } else {
                    let col = ((lx - GAP) / (SWATCH + GAP)).floor();
                    let row = ((ly - 26.0 - GAP) / (SWATCH + GAP)).floor();
                    if !(0.0..10.0).contains(&col) || !(0.0..3.0).contains(&row) {
                        return None;
                    }
                    Some(PALETTE[(row as usize) * 10 + col as usize])
                };
                Some(Act::Fmt(if *text { Fmt::FontColor(color) } else { Fmt::FillColor(color) }))
            }
        }
    }

    pub fn draw(
        &self,
        p: &mut Painter,
        g: &Geom,
        ui: TextStyle,
        mouse: (f32, f32),
        th: &crate::theme::Theme,
    ) {
        let s = g.s;
        let px = s.round().max(1.0);
        let all = Rect::everything();
        let r = self.rect;
        // Sombra simples e borda.
        p.fill(Rect::new(r.x0 + 2.0 * s, r.y0 + 2.0 * s, r.x1 + 3.0 * s, r.y1 + 3.0 * s), &all, th.shadow);
        p.fill(r.inset(-px), &all, th.line);
        p.fill(r, &all, th.menu);
        match &self.kind {
            Kind::List(_) => {
                for (row, item) in self.rows(s) {
                    let Some(it) = item else {
                        let y = ((row.y0 + row.y1) / 2.0).round();
                        p.fill(Rect::new(row.x0 + 8.0 * s, y, row.x1 - 8.0 * s, y + px), &all, th.line);
                        continue;
                    };
                    if row.contains(mouse.0, mouse.1) {
                        p.fill(row, &all, th.hover);
                    }
                    if it.checked {
                        p.label(
                            "•",
                            ui,
                            Rect::new(row.x0, row.y0, row.x0 + 26.0 * s, row.y1),
                            Align::Center,
                            0.0,
                            th.accent,
                        );
                    }
                    p.label(&it.label, ui, row, Align::Left, 28.0 * s, th.text);
                    if !it.key.is_empty() {
                        p.label(it.key, ui, row, Align::Right, 12.0 * s, th.muted);
                    }
                }
            }
            Kind::Palette(text) => {
                let label = if *text { "Automática" } else { "Sem preenchimento" };
                let top = Rect::new(r.x0, r.y0, r.x1, r.y0 + 26.0 * s);
                if top.contains(mouse.0, mouse.1) {
                    p.fill(top, &all, th.hover);
                }
                p.label(label, ui, top, Align::Left, 8.0 * s, th.text);
                for (i, &c) in PALETTE.iter().enumerate() {
                    let (row, col) = ((i / 10) as f32, (i % 10) as f32);
                    let x0 = (r.x0 + (GAP + col * (SWATCH + GAP)) * s).round();
                    let y0 = (r.y0 + (26.0 + GAP + row * (SWATCH + GAP)) * s).round();
                    let sw = Rect::new(x0, y0, x0 + (SWATCH * s).round(), y0 + (SWATCH * s).round());
                    let hot = sw.contains(mouse.0, mouse.1);
                    p.fill(
                        sw.inset(if hot { -2.0 * px } else { -px }),
                        &all,
                        if hot { th.accent } else { th.line },
                    );
                    p.fill(sw, &all, [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]);
                }
            }
        }
    }
}

/// O que a caixa de diálogo pergunta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Largura das colunas da seleção, em caracteres.
    ColWidth,
    /// Altura das linhas da seleção, em pontos.
    RowHeight,
}

/// Caixa de diálogo de um campo, no meio da grade.
pub struct Prompt {
    pub ask: Ask,
    pub title: String,
    pub input: TextInput,
}

/// Retângulos da caixa: (caixa, campo, OK, Cancelar).
pub fn prompt_rects(g: &Geom) -> (Rect, Rect, Rect, Rect) {
    let s = g.s;
    let (w, h) = ((340.0 * s).round(), (138.0 * s).round());
    let cx = (g.grid.x0 + g.grid.x1) / 2.0;
    let cy = g.grid.y0 + (g.grid.h() * 0.3).max(h / 2.0 + 10.0 * s);
    let bx = Rect::new(
        (cx - w / 2.0).round(),
        (cy - h / 2.0).round(),
        (cx + w / 2.0).round(),
        (cy + h / 2.0).round(),
    );
    let input = Rect::new(bx.x0 + 16.0 * s, bx.y0 + 44.0 * s, bx.x1 - 16.0 * s, bx.y0 + 74.0 * s);
    let bw = (96.0 * s).round();
    let by = Rect::new(0.0, bx.y1 - 46.0 * s, 0.0, bx.y1 - 16.0 * s);
    let cancel = Rect::new(bx.x1 - 16.0 * s - bw, by.y0, bx.x1 - 16.0 * s, by.y1);
    let ok = Rect::new(cancel.x0 - 8.0 * s - bw, by.y0, cancel.x0 - 8.0 * s, by.y1);
    (bx, input, ok, cancel)
}

impl Prompt {
    pub fn draw(&self, p: &mut Painter, g: &Geom, ui: TextStyle, th: &crate::theme::Theme) {
        let s = g.s;
        let px = s.round().max(1.0);
        let all = Rect::everything();
        let (bx, input, ok, cancel) = prompt_rects(g);
        p.fill(Rect::new(0.0, 0.0, g.w, g.status.y1), &all, th.veil);
        p.fill(bx.inset(-px), &all, th.line);
        p.fill(bx, &all, th.menu);
        let title = Rect::new(bx.x0, bx.y0 + 8.0 * s, bx.x1, bx.y0 + 36.0 * s);
        let mut bold = ui;
        bold.bold = true;
        p.label(&self.title, bold, title, Align::Left, 16.0 * s, th.text);
        self.input.draw(p, input, ui, 6.0 * s, true, &th.input, "");
        for (r, label, primary) in [(ok, "OK", true), (cancel, "Cancelar", false)] {
            let bg: Rgba = if primary { th.accent } else { th.menu };
            p.fill(r, &all, bg);
            p.stroke(r, &all, px, if primary { th.accent } else { th.line });
            p.label(label, ui, r, Align::Center, 0.0, if primary { th.on_accent } else { th.text });
        }
    }
}
