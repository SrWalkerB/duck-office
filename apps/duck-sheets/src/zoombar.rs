//! Controle de zoom da barra de status: botões, barra deslizante e a
//! porcentagem, que abre a lista de níveis (como no Excel).

use duck_text::TextStyle;
use duck_ui::{Align, Painter, Rect};

use crate::app::Geom;
use crate::ribbon::{Act, Drop};
use crate::theme::Theme;

pub const MIN: i32 = 10;
pub const MAX: i32 = 400;
/// Níveis da lista.
pub const LEVELS: [u16; 7] = [200, 150, 125, 100, 75, 50, 25];

pub struct ZoomBar {
    pub minus: Rect,
    pub track: Rect,
    pub plus: Rect,
    pub label: Rect,
}

pub fn layout(g: &Geom) -> ZoomBar {
    let (s, st) = (g.s, g.status);
    let part = |x1: f32, w: f32| Rect::new((x1 - w * s).round(), st.y0, x1, st.y1);
    let label = part(g.w - (6.0 * s).round(), 50.0);
    let plus = part(label.x0, 24.0);
    let track = part(plus.x0, 110.0);
    let minus = part(track.x0, 24.0);
    ZoomBar { minus, track, plus, label }
}

impl ZoomBar {
    pub fn area(&self) -> Rect {
        Rect::new(self.minus.x0, self.minus.y0, self.label.x1, self.label.y1)
    }

    /// Onde a lista de níveis se apoia: sobe a partir do topo da barra.
    pub fn menu_anchor(&self, g: &Geom) -> Rect {
        let y = self.label.y0 - 2.0 * g.s;
        Rect::new(self.label.x0, y, self.label.x1, y)
    }

    /// Botão no ponto: ação e dica.
    pub fn hit(&self, x: f32, y: f32) -> Option<(Act, &'static str)> {
        if self.minus.contains(x, y) {
            Some((Act::Zoom(-1), "Diminuir o zoom (Ctrl+- ou Ctrl+roda do mouse)"))
        } else if self.plus.contains(x, y) {
            Some((Act::Zoom(1), "Aumentar o zoom (Ctrl++ ou Ctrl+roda do mouse)"))
        } else if self.label.contains(x, y) {
            Some((Act::Drop(Drop::Zoom), "Escolher o nível de zoom"))
        } else {
            None
        }
    }

    /// Zoom da posição `x` na barra: 100% no meio, em passos de 5.
    pub fn zoom_at(&self, x: f32) -> i32 {
        let t = ((x - self.track.x0) / self.track.w().max(1.0)).clamp(0.0, 1.0);
        let z = if t < 0.5 { 10.0 + t * 2.0 * 90.0 } else { 100.0 + (t - 0.5) * 2.0 * 300.0 };
        // Perto do meio gruda em 100%.
        if (t - 0.5).abs() < 0.03 { 100 } else { ((z / 5.0).round() as i32 * 5).clamp(MIN, MAX) }
    }

    fn pos(&self, zoom: i32) -> f32 {
        let z = zoom as f32;
        let t = if z < 100.0 { (z - 10.0) / 180.0 } else { 0.5 + (z - 100.0) / 600.0 };
        self.track.x0 + t.clamp(0.0, 1.0) * self.track.w()
    }

    pub fn draw(&self, p: &mut Painter, g: &Geom, ui: TextStyle, zoom: i32, hover: Option<Act>, th: &Theme) {
        let s = g.s;
        let px = s.round().max(1.0);
        let all = Rect::everything();
        for (r, act) in
            [(self.minus, Act::Zoom(-1)), (self.plus, Act::Zoom(1)), (self.label, Act::Drop(Drop::Zoom))]
        {
            if hover == Some(act) {
                p.fill(Rect::new(r.x0, r.y0 + px, r.x1, r.y1), &all, th.hover);
            }
        }
        let cy = ((self.track.y0 + self.track.y1) / 2.0).round();
        let t = (2.0 * s).round().max(1.0);
        let half = (5.0 * s).round();
        for (r, plus) in [(self.minus, false), (self.plus, true)] {
            let cx = ((r.x0 + r.x1) / 2.0).round();
            p.fill(Rect::new(cx - half, cy - t / 2.0, cx + half, cy + t / 2.0), &all, th.text);
            if plus {
                p.fill(Rect::new(cx - t / 2.0, cy - half, cx + t / 2.0, cy + half), &all, th.text);
            }
        }
        let tr = self.track;
        p.fill(Rect::new(tr.x0, cy - px, tr.x1, cy + px), &all, th.line);
        let mid = ((tr.x0 + tr.x1) / 2.0).round();
        p.fill(Rect::new(mid - px / 2.0, cy - 4.0 * s, mid + px / 2.0, cy + 4.0 * s), &all, th.muted);
        let x = self.pos(zoom).round();
        let (hw, hh) = ((4.0 * s).round(), (7.0 * s).round());
        p.fill(Rect::new(x - hw, cy - hh, x + hw, cy + hh), &all, th.accent);
        p.label(&format!("{zoom}%"), ui, self.label, Align::Center, 0.0, th.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_positions() {
        let r = |x0: f32, x1: f32| Rect::new(x0, 0.0, x1, 20.0);
        let b = ZoomBar {
            minus: r(0.0, 20.0),
            track: r(20.0, 120.0),
            plus: r(120.0, 140.0),
            label: r(140.0, 190.0),
        };
        assert_eq!(b.zoom_at(0.0), 10);
        assert_eq!(b.zoom_at(70.0), 100);
        assert_eq!(b.zoom_at(500.0), 400);
        for z in [10, 25, 60, 100, 150, 200, 400] {
            assert_eq!(b.zoom_at(b.pos(z)), z);
        }
        assert_eq!(b.hit(10.0, 5.0).map(|h| h.0), Some(Act::Zoom(-1)));
        assert_eq!(b.hit(70.0, 5.0), None);
    }
}
