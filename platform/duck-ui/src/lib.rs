//! `duck-ui`: desenho 2D e widgets mínimos sobre `duck-gfx` + `duck-text`.
//!
//! Não é um framework de layout: os apps calculam a geometria (que numa
//! planilha é quase toda da grade) e usam daqui o [`Painter`] e os widgets
//! que se repetem entre os apps da suíte ([`TextInput`], [`TabBar`]).
//! Tudo trabalha em pixels físicos.

use duck_gfx::Instance;
use duck_platform::{Clipboard, Key, Modifiers};
use duck_text::{TextStyle, TextSystem};

/// Cor RGBA 8 bits.
pub type Rgba = [u8; 4];

/// RGB `0xRRGGBB` → RGBA opaco.
pub const fn rgb(c: u32) -> Rgba {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]
}

/// Retângulo (x0, y0, x1, y1) em pixels físicos.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub const fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    /// Retângulo "infinito" (sem recorte).
    pub const fn everything() -> Self {
        Self::new(-1e9, -1e9, 1e9, 1e9)
    }

    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1))
    }

    /// Encolhe (positivo) ou expande (negativo) em todos os lados.
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x0 + d, self.y0 + d, self.x1 - d, self.y1 - d)
    }

    pub fn as_clip(&self) -> [f32; 4] {
        [self.x0, self.y0, self.x1, self.y1]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Junta instâncias de um frame: retângulos, linhas e texto, com recorte.
pub struct Painter<'a> {
    pub out: &'a mut Vec<Instance>,
    pub text: &'a mut TextSystem,
}

impl<'a> Painter<'a> {
    pub fn new(out: &'a mut Vec<Instance>, text: &'a mut TextSystem) -> Self {
        Self { out, text }
    }

    /// Retângulo cheio recortado por `clip`.
    #[inline]
    pub fn fill(&mut self, r: Rect, clip: &Rect, color: Rgba) {
        let c = r.intersect(clip);
        if !c.is_empty() && color[3] > 0 {
            self.out.push(Instance::solid(c.x0, c.y0, c.w(), c.h(), color));
        }
    }

    /// Triângulo cheio, recortado por `clip` (na CPU: o polígono recortado
    /// vira um leque de triângulos).
    pub fn triangle(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], clip: &Rect, color: Rgba) {
        if color[3] == 0 {
            return;
        }
        let inside = |p: &[f32; 2]| p[0] >= clip.x0 && p[0] <= clip.x1 && p[1] >= clip.y0 && p[1] <= clip.y1;
        if inside(&a) && inside(&b) && inside(&c) {
            self.out.push(Instance::triangle(a, b, c, color));
            return;
        }
        let poly = clip_polygon(&[a, b, c], clip);
        for i in 1..poly.len().saturating_sub(1) {
            self.out.push(Instance::triangle(poly[0], poly[i], poly[i + 1], color));
        }
    }

    /// Polígono convexo cheio (leque a partir do primeiro vértice).
    pub fn polygon(&mut self, pts: &[[f32; 2]], clip: &Rect, color: Rgba) {
        for i in 1..pts.len().saturating_sub(1) {
            self.triangle(pts[0], pts[i], pts[i + 1], clip, color);
        }
    }

    /// Segmento de reta com `w` pixels de espessura.
    pub fn line(&mut self, a: [f32; 2], b: [f32; 2], w: f32, clip: &Rect, color: Rgba) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-3 {
            return;
        }
        if dx.abs() < 1e-3 || dy.abs() < 1e-3 {
            // Reta na horizontal ou vertical: retângulo (mais nítido).
            let h = w / 2.0;
            let r = Rect::new(
                a[0].min(b[0]) - if dx.abs() < 1e-3 { h } else { 0.0 },
                a[1].min(b[1]) - if dy.abs() < 1e-3 { h } else { 0.0 },
                a[0].max(b[0]) + if dx.abs() < 1e-3 { h } else { 0.0 },
                a[1].max(b[1]) + if dy.abs() < 1e-3 { h } else { 0.0 },
            );
            self.fill(r, clip, color);
            return;
        }
        let (nx, ny) = (-dy / len * w / 2.0, dx / len * w / 2.0);
        let p =
            [[a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny], [b[0] - nx, b[1] - ny], [a[0] - nx, a[1] - ny]];
        self.polygon(&p, clip, color);
    }

    /// Retângulo tracejado (bordas pontilhadas e tracejadas).
    pub fn dashed(&mut self, r: Rect, clip: &Rect, color: Rgba, period: f32, on: f32) {
        let c = r.intersect(clip);
        if !c.is_empty() {
            self.out.push(Instance::dashed(c.x0, c.y0, c.w(), c.h(), color, period.max(1.0), on.max(0.5)));
        }
    }

    /// Contorno de `w` pixels por dentro de `r`.
    pub fn stroke(&mut self, r: Rect, clip: &Rect, w: f32, color: Rgba) {
        self.fill(Rect::new(r.x0, r.y0, r.x1, r.y0 + w), clip, color);
        self.fill(Rect::new(r.x0, r.y1 - w, r.x1, r.y1), clip, color);
        self.fill(Rect::new(r.x0, r.y0, r.x0 + w, r.y1), clip, color);
        self.fill(Rect::new(r.x1 - w, r.y0, r.x1, r.y1), clip, color);
    }

    /// Texto com o topo-esquerdo da linha em (x, y). Retorna a largura.
    pub fn text(&mut self, s: &str, style: TextStyle, x: f32, y: f32, clip: &Rect, color: Rgba) -> f32 {
        let out = &mut *self.out;
        self.text
            .draw_styled(s, style, x, y, clip.as_clip(), |q| out.push(Instance::glyph(q.rect, q.uv, color)))
    }

    /// Texto girado `degrees` graus no sentido anti-horário em torno de
    /// (x, y), o canto superior esquerdo da linha antes de girar. Glifos com o
    /// centro fora de `clip` ficam de fora (o renderer não recorta glifos
    /// girados).
    #[allow(clippy::too_many_arguments)]
    pub fn text_rotated(
        &mut self,
        s: &str,
        style: TextStyle,
        x: f32,
        y: f32,
        degrees: f32,
        clip: &Rect,
        color: Rgba,
    ) -> f32 {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let rot = |dx: f32, dy: f32| (x + dx * cos + dy * sin, y - dx * sin + dy * cos);
        let out = &mut *self.out;
        let everything = [f32::MIN, f32::MIN, f32::MAX, f32::MAX];
        self.text.draw_styled(s, style, 0.0, 0.0, everything, |q| {
            let [gx, gy, gw, gh] = q.rect;
            // Sem recorte na GPU: entra o glifo cujo centro está na área (um
            // acento pode passar um pouco da borda; cortar a letra inteira é pior).
            let (cx, cy) = rot(gx + gw / 2.0, gy + gh / 2.0);
            let inside = cx >= clip.x0 && cx <= clip.x1 && cy >= clip.y0 && cy <= clip.y1;
            if inside {
                let (px, py) = rot(gx, gy);
                out.push(Instance::glyph_rotated([px, py, gw, gh], q.uv, color, degrees));
            }
        })
    }

    /// Texto alinhado entre `x0` e `x1`, com o topo da linha em `y`.
    #[allow(clippy::too_many_arguments)]
    pub fn text_aligned(
        &mut self,
        s: &str,
        style: TextStyle,
        x0: f32,
        x1: f32,
        y: f32,
        align: Align,
        clip: &Rect,
        color: Rgba,
    ) -> f32 {
        let x = match align {
            Align::Left => x0,
            Align::Right => x1 - self.text.measure_styled(s, style),
            Align::Center => ((x0 + x1 - self.text.measure_styled(s, style)) / 2.0).floor(),
        };
        self.text(s, style, x, y, clip, color)
    }

    /// Texto centralizado verticalmente em `r`.
    pub fn label(&mut self, s: &str, style: TextStyle, r: Rect, align: Align, pad: f32, color: Rgba) -> f32 {
        let lh = self.text.line_height_of(style);
        let y = (r.y0 + (r.h() - lh) / 2.0).floor();
        self.text_aligned(s, style, r.x0 + pad, r.x1 - pad, y, align, &r, color)
    }
}

// ---------------------------------------------------------------------------
// Campo de texto
// ---------------------------------------------------------------------------

/// O que uma tecla fez num [`TextInput`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputResult {
    /// A tecla foi tratada pelo campo.
    pub handled: bool,
    /// O texto mudou.
    pub changed: bool,
    /// Enter (com Shift: `submitted_back`).
    pub submitted: bool,
    pub submitted_back: bool,
    /// Esc.
    pub cancelled: bool,
}

/// Campo de texto de uma linha: cursor, seleção, colar e copiar.
#[derive(Clone, Debug, Default)]
pub struct TextInput {
    pub text: String,
    /// Posição do cursor (byte, sempre numa fronteira de caractere).
    cursor: usize,
    /// Outra ponta da seleção, se houver.
    anchor: Option<usize>,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_text(&mut self, s: &str) {
        self.text = s.to_string();
        self.cursor = self.text.len();
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.cursor).then(|| (a.min(self.cursor), a.max(self.cursor)))
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((a, b)) = self.selection() {
            self.text.replace_range(a..b, "");
            self.cursor = a;
            self.anchor = None;
            true
        } else {
            false
        }
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(p, _)| p)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to;
    }

    /// Insere texto no cursor (substitui a seleção).
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        // Uma linha só: quebras viram espaço.
        let s: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    /// Trata uma tecla pressionada. `text` é o texto que a tecla produziu.
    pub fn key(
        &mut self,
        key: &Key,
        mods: Modifiers,
        text: Option<&str>,
        clipboard: &mut Clipboard,
    ) -> InputResult {
        let mut r = InputResult { handled: true, ..Default::default() };
        let before = self.text.len();
        let shift = mods.shift;
        match key {
            Key::Enter => {
                if shift {
                    r.submitted_back = true;
                } else {
                    r.submitted = true;
                }
            }
            Key::Escape => r.cancelled = true,
            Key::ArrowLeft => {
                let to = match (self.selection(), shift) {
                    (Some((a, _)), false) => a,
                    _ => self.prev(self.cursor),
                };
                self.move_to(to, shift);
            }
            Key::ArrowRight => {
                let to = match (self.selection(), shift) {
                    (Some((_, b)), false) => b,
                    _ => self.next(self.cursor),
                };
                self.move_to(to, shift);
            }
            Key::Home => self.move_to(0, shift),
            Key::End => self.move_to(self.text.len(), shift),
            Key::Backspace => {
                if !self.delete_selection() && self.cursor > 0 {
                    let p = self.prev(self.cursor);
                    self.text.replace_range(p..self.cursor, "");
                    self.cursor = p;
                }
                r.changed = true;
            }
            Key::Delete => {
                if !self.delete_selection() && self.cursor < self.text.len() {
                    let n = self.next(self.cursor);
                    self.text.replace_range(self.cursor..n, "");
                }
                r.changed = true;
            }
            Key::Char(c) if mods.ctrl && c.eq_ignore_ascii_case("a") => self.select_all(),
            Key::Char(c) if mods.ctrl && c.eq_ignore_ascii_case("c") => {
                if let Some((a, b)) = self.selection() {
                    clipboard.set_text(&self.text[a..b]);
                }
            }
            Key::Char(c) if mods.ctrl && c.eq_ignore_ascii_case("x") => {
                if let Some((a, b)) = self.selection() {
                    clipboard.set_text(&self.text[a..b]);
                    self.delete_selection();
                    r.changed = true;
                }
            }
            Key::Char(c) if mods.ctrl && c.eq_ignore_ascii_case("v") => {
                if let Some(s) = clipboard.get_text() {
                    self.insert(&s);
                    r.changed = true;
                }
            }
            _ if mods.ctrl || mods.alt || mods.logo => r.handled = false,
            _ => match text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                Some(t) => {
                    self.insert(t);
                    r.changed = true;
                }
                None => r.handled = matches!(key, Key::Space),
            },
        }
        r.changed &= self.text.len() != before || r.changed;
        r
    }

    /// Desenha o campo: fundo, texto, seleção e cursor (se `focused`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        p: &mut Painter,
        r: Rect,
        style: TextStyle,
        pad: f32,
        focused: bool,
        colors: &InputColors,
        placeholder: &str,
    ) {
        p.fill(r, &r, colors.background);
        p.stroke(r, &r, 1.0, if focused { colors.focus } else { colors.border });
        let lh = p.text.line_height_of(style);
        let y = (r.y0 + (r.h() - lh) / 2.0).floor();
        let inner = r.inset(1.0);
        let x = r.x0 + pad;
        // Rola para o cursor ficar visível.
        let caret_x = p.text.measure_styled(&self.text[..self.cursor], style);
        let shift = (caret_x - (r.w() - 2.0 * pad)).max(0.0);
        let x = x - shift;
        if let Some((a, b)) = self.selection().filter(|_| focused) {
            let xa = x + p.text.measure_styled(&self.text[..a], style);
            let xb = x + p.text.measure_styled(&self.text[..b], style);
            p.fill(Rect::new(xa, y, xb, y + lh), &inner, colors.selection);
        }
        if self.text.is_empty() {
            p.text(placeholder, style, x, y, &inner, colors.placeholder);
        } else {
            p.text(&self.text, style, x, y, &inner, colors.text);
        }
        if focused {
            let cx = (x + caret_x).round();
            p.fill(Rect::new(cx, y + 1.0, cx + 1.0, y + lh - 1.0), &inner, colors.text);
        }
    }
}

/// Cores de um [`TextInput`].
#[derive(Clone, Copy, Debug)]
pub struct InputColors {
    pub background: Rgba,
    pub border: Rgba,
    pub focus: Rgba,
    pub text: Rgba,
    pub placeholder: Rgba,
    pub selection: Rgba,
}

impl Default for InputColors {
    fn default() -> Self {
        Self {
            background: rgb(0xFFFFFF),
            border: rgb(0xC4C7C5),
            focus: rgb(0x1A73E8),
            text: rgb(0x202124),
            placeholder: rgb(0x80868B),
            selection: rgb(0xC2DBFF),
        }
    }
}

// ---------------------------------------------------------------------------
// Barra de abas
// ---------------------------------------------------------------------------

/// Uma aba: nome e cor opcional (faixa embaixo do nome).
#[derive(Clone, Debug)]
pub struct Tab {
    pub label: String,
    pub color: Option<Rgba>,
}

/// Barra de abas com rolagem horizontal (planilhas com dezenas de abas).
#[derive(Clone, Debug, Default)]
pub struct TabBar {
    /// Deslocamento horizontal em pixels.
    pub scroll: f32,
    /// Posições (x0, x1) calculadas no último desenho, relativas à barra.
    spans: Vec<(f32, f32)>,
    content_w: f32,
}

/// Cores de uma [`TabBar`].
#[derive(Clone, Copy, Debug)]
pub struct TabColors {
    pub background: Rgba,
    pub tab: Rgba,
    pub active: Rgba,
    pub text: Rgba,
    pub active_text: Rgba,
    pub line: Rgba,
    pub accent: Rgba,
}

impl Default for TabColors {
    fn default() -> Self {
        Self {
            background: rgb(0xF8F9FA),
            tab: rgb(0xF8F9FA),
            active: rgb(0xFFFFFF),
            text: rgb(0x5F6368),
            active_text: rgb(0x188038),
            line: rgb(0xDADCE0),
            accent: rgb(0x188038),
        }
    }
}

impl TabBar {
    /// Desenha as abas em `r`. `active` = índice em `tabs`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        p: &mut Painter,
        r: Rect,
        tabs: &[Tab],
        active: usize,
        style: TextStyle,
        pad: f32,
        colors: &TabColors,
    ) {
        p.fill(r, &r, colors.background);
        p.fill(Rect::new(r.x0, r.y0, r.x1, r.y0 + 1.0), &r, colors.line);
        self.spans.clear();
        let mut x = 0.0;
        for t in tabs {
            let w = (p.text.measure_styled(&t.label, style) + 2.0 * pad).ceil();
            self.spans.push((x, x + w));
            x += w;
        }
        self.content_w = x;
        self.clamp(r.w());
        let base = r.x0 - self.scroll;
        for (i, t) in tabs.iter().enumerate() {
            let (a, b) = self.spans[i];
            let tr = Rect::new(base + a, r.y0 + 1.0, base + b, r.y1);
            if tr.x1 < r.x0 || tr.x0 > r.x1 {
                continue;
            }
            let on = i == active;
            p.fill(tr, &r, if on { colors.active } else { colors.tab });
            if let Some(c) = t.color {
                let h = (tr.h() * 0.12).max(3.0).round();
                p.fill(Rect::new(tr.x0 + 1.0, tr.y1 - h, tr.x1 - 1.0, tr.y1), &r, c);
            }
            if on {
                let h = 2.0f32.max((tr.h() * 0.06).round());
                p.fill(Rect::new(tr.x0, tr.y0, tr.x1, tr.y0 + h), &r, colors.accent);
            }
            p.fill(Rect::new(tr.x1 - 1.0, tr.y0 + 4.0, tr.x1, tr.y1 - 4.0), &r, colors.line);
            let clip = tr.intersect(&r);
            let lh = p.text.line_height_of(style);
            let y = (tr.y0 + (tr.h() - lh) / 2.0).floor();
            let color = if on { colors.active_text } else { colors.text };
            p.text_aligned(&t.label, style, tr.x0, tr.x1, y, Align::Center, &clip, color);
        }
    }

    fn clamp(&mut self, view_w: f32) {
        self.scroll = self.scroll.clamp(0.0, (self.content_w - view_w).max(0.0));
    }

    /// Rola a barra (roda do mouse sobre as abas).
    pub fn scroll_by(&mut self, dx: f32, view_w: f32) {
        self.scroll += dx;
        self.clamp(view_w);
    }

    /// Garante que a aba `i` está visível na barra de largura `view_w`.
    pub fn reveal(&mut self, i: usize, view_w: f32) {
        if let Some(&(a, b)) = self.spans.get(i) {
            if a < self.scroll {
                self.scroll = a;
            } else if b > self.scroll + view_w {
                self.scroll = b - view_w;
            }
        }
    }

    /// Aba sob o ponto `x` (relativo ao início da barra), pelas posições do último desenho.
    pub fn hit(&self, x: f32) -> Option<usize> {
        let x = x + self.scroll;
        self.spans.iter().position(|&(a, b)| x >= a && x < b)
    }
}

/// Recorta um polígono convexo pelo retângulo (Sutherland–Hodgman).
fn clip_polygon(pts: &[[f32; 2]], r: &Rect) -> Vec<[f32; 2]> {
    let mut poly: Vec<[f32; 2]> = pts.to_vec();
    // (eixo, limite, manter >=)
    for (axis, lim, keep_ge) in [(0, r.x0, true), (0, r.x1, false), (1, r.y0, true), (1, r.y1, false)] {
        if poly.is_empty() {
            break;
        }
        let inside = |p: &[f32; 2]| if keep_ge { p[axis] >= lim } else { p[axis] <= lim };
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let cur = poly[i];
            let prev = poly[(i + poly.len() - 1) % poly.len()];
            let (ci, pi) = (inside(&cur), inside(&prev));
            if ci != pi {
                let t = (lim - prev[axis]) / (cur[axis] - prev[axis]);
                out.push([prev[0] + (cur[0] - prev[0]) * t, prev[1] + (cur[1] - prev[1]) * t]);
            }
            if ci {
                out.push(cur);
            }
        }
        poly = out;
    }
    poly
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(t: &mut TextInput, key: Key, text: Option<&str>, mods: Modifiers) -> InputResult {
        let mut cb = Clipboard::default();
        t.key(&key, mods, text, &mut cb)
    }

    #[test]
    fn text_input_edits() {
        let mut t = TextInput::new();
        let none = Modifiers::default();
        let shift = Modifiers { shift: true, ..Default::default() };
        for c in ["a", "ç", "ã", "o"] {
            press(&mut t, Key::Char(c.into()), Some(c), none);
        }
        assert_eq!(t.text, "ação");
        press(&mut t, Key::ArrowLeft, None, none);
        press(&mut t, Key::Backspace, None, none);
        assert_eq!(t.text, "aço");
        press(&mut t, Key::Home, None, none);
        press(&mut t, Key::ArrowRight, None, shift);
        press(&mut t, Key::ArrowRight, None, shift);
        let r = press(&mut t, Key::Char("x".into()), Some("x"), none);
        assert!(r.changed);
        assert_eq!(t.text, "xo");
        assert!(press(&mut t, Key::Enter, None, none).submitted);
        assert!(press(&mut t, Key::Escape, None, none).cancelled);
        assert!(!press(&mut t, Key::Char("s".into()), Some("s"), Modifiers { ctrl: true, ..none }).handled);
    }

    #[test]
    fn rect_ops() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 20.0, 20.0);
        assert_eq!(a.intersect(&b), Rect::new(5.0, 5.0, 10.0, 10.0));
        assert!(a.contains(0.0, 9.9));
        assert!(!a.contains(10.0, 5.0));
        assert_eq!(rgb(0x102030), [0x10, 0x20, 0x30, 255]);
    }
}
