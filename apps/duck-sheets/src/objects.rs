//! Objetos da aba (gráficos, imagens, formas, segmentações): selecionar,
//! arrastar e redimensionar.
//!
//! Quem se move é a âncora do desenho: os objetos de um grupo andam juntos.
//! Durante o arraste só a tela muda ([`Moving`]); o comando sai ao soltar e o
//! gravador remenda a âncora na parte do desenho.

use duck_core::Command;
use duck_core::drawing::{Anchor, Marker, NO_SLOT, Object, ObjectKind};
use duck_core::shape::Fill;
use duck_platform::{Cursor, Key, Modifiers};
use duck_ui::{Painter, Rect};

use crate::app::{Drag, Geom, Quad, SheetsApp, View};
use crate::chart::anchor_rect;
use crate::theme::Theme;

const EMU_PER_PX: f64 = 9525.0;

/// Menor lado de um objeto (pixels a 100%).
const MIN_SIDE: f64 = 16.0;

/// Por onde o objeto foi pego: (0, 0) é o corpo; -1 e 1 são os lados.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grip(pub i8, pub i8);

/// As oito alças (os cantos primeiro: valem mais que os lados).
const GRIPS: [Grip; 8] =
    [Grip(-1, -1), Grip(1, -1), Grip(-1, 1), Grip(1, 1), Grip(0, -1), Grip(0, 1), Grip(-1, 0), Grip(1, 0)];

/// Posição provisória da âncora `slot` (x, y, largura, altura a 100%).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moving {
    pub slot: u32,
    pub rect: [f64; 4],
}

/// O retângulo `from` pego em `grip` e arrastado (`dx`, `dy`). Com `keep`
/// (Shift), mover segue um eixo só e um canto guarda a proporção.
pub fn dragged(from: [f64; 4], grip: Grip, dx: f64, dy: f64, keep: bool) -> [f64; 4] {
    let [x, y, w, h] = from;
    if grip == Grip(0, 0) {
        let (dx, dy) = match keep {
            true if dx.abs() >= dy.abs() => (dx, 0.0),
            true => (0.0, dy),
            false => (dx, dy),
        };
        return [(x + dx).max(0.0), (y + dy).max(0.0), w, h];
    }
    let side = |grip: i8, d: f64, size: f64, start: f64| match grip {
        // O começo anda até 0 e até sobrar o mínimo.
        -1 => (size - d).min(size + start).max(MIN_SIDE),
        1 => (size + d).max(MIN_SIDE),
        _ => size,
    };
    let (mut nw, mut nh) = (side(grip.0, dx, w, x), side(grip.1, dy, h, y));
    if keep && grip.0 != 0 && grip.1 != 0 && w > 0.0 && h > 0.0 {
        let (kw, kh) = (nw / w, nh / h);
        let k = if (kw - 1.0).abs() >= (kh - 1.0).abs() { kw } else { kh };
        (nw, nh) = ((w * k).max(MIN_SIDE), (h * k).max(MIN_SIDE));
    }
    let nx = if grip.0 < 0 { (x + w - nw).max(0.0) } else { x };
    let ny = if grip.1 < 0 { (y + h - nh).max(0.0) } else { y };
    [nx, ny, nw, nh]
}

/// A âncora, do mesmo tipo de `kind`, que põe o objeto em `rect`.
pub fn anchor_at(rect: [f64; 4], kind: &Anchor, v: &View) -> Anchor {
    let emu = |px: f64| (px * EMU_PER_PX).round() as i64;
    let marker = |x: f64, y: f64| {
        let along = |axis: &crate::axis::Axis, p: f64| {
            let p = p.clamp(0.0, axis.total() as f64);
            let i = axis.index_at(p.floor() as i64);
            (i, emu(p - axis.offset(i) as f64).max(0))
        };
        let ((col, col_off), (row, row_off)) = (along(&v.cols, x), along(&v.rows, y));
        Marker { col, col_off, row, row_off }
    };
    let [x, y, w, h] = rect;
    match kind {
        Anchor::Two { .. } => Anchor::Two { from: marker(x, y), to: marker(x + w, y + h) },
        Anchor::One { .. } => Anchor::One { from: marker(x, y), cx: emu(w), cy: emu(h) },
        Anchor::Abs { .. } => Anchor::Abs { x: emu(x), y: emu(y), cx: emu(w), cy: emu(h) },
    }
}

/// O retângulo (a 100%) na tela, num painel.
fn on_pane(q: &Quad, r: [f64; 4], g: f32) -> Rect {
    let x = |p: f64| q.rect.x0 + ((p - q.ox) * g as f64) as f32;
    let y = |p: f64| q.rect.y0 + ((p - q.oy) * g as f64) as f32;
    Rect::new(x(r[0]), y(r[1]), x(r[0] + r[2]), y(r[1] + r[3]))
}

fn grip_point(r: &Rect, grip: Grip) -> [f32; 2] {
    let at = |k: i8, a: f32, b: f32| match k {
        -1 => a,
        1 => b,
        _ => (a + b) / 2.0,
    };
    [at(grip.0, r.x0, r.x1), at(grip.1, r.y0, r.y1)]
}

/// O ponto está no objeto, com o retângulo `r` na tela? Formas sem
/// preenchimento e linhas só pegam perto do traço: o que está atrás continua
/// clicável.
fn touches(o: &Object, r: &Rect, x: f32, y: f32, s: f32) -> bool {
    let near = 5.0 * s;
    if !r.inset(-near).contains(x, y) {
        return false;
    }
    match &o.kind {
        ObjectKind::Shape(sh) if sh.is_line() => {
            // Distância ao segmento.
            let (mut y0, mut y1) = (r.y0, r.y1);
            if sh.flip_h != sh.flip_v {
                (y0, y1) = (y1, y0);
            }
            let (dx, dy) = (r.x1 - r.x0, y1 - y0);
            let len = (dx * dx + dy * dy).max(1e-3);
            let t = (((x - r.x0) * dx + (y - y0) * dy) / len).clamp(0.0, 1.0);
            let (px, py) = (r.x0 + t * dx - x, y0 + t * dy - y);
            (px * px + py * py).sqrt() <= near
        }
        ObjectKind::Shape(sh) if matches!(sh.fill, Fill::None) && sh.text.plain().trim().is_empty() => {
            !r.inset(near).contains(x, y)
        }
        _ => r.contains(x, y),
    }
}

/// Moldura e alças do objeto selecionado.
pub fn draw_selection(p: &mut Painter, g: &Geom, rect: [f64; 4], th: &Theme) {
    let px = g.s.round().max(1.0);
    let (outer, inner) = (4.5 * g.s, 3.0 * g.s);
    for q in g.quads.iter().filter(|q| !q.rect.is_empty()) {
        let r = on_pane(q, rect, g.g);
        if r.inset(-outer).intersect(&q.rect).is_empty() {
            continue;
        }
        p.stroke(r.inset(-px), &q.rect, px, th.selection);
        for grip in GRIPS {
            let c = grip_point(&r, grip);
            if q.rect.inset(-outer).contains(c[0], c[1]) {
                p.disc(c, outer, &q.rect, th.selection);
                p.disc(c, inner, &q.rect, th.paper);
            }
        }
    }
}

impl SheetsApp {
    /// O objeto selecionado na aba à vista: (índice, âncora, retângulo).
    pub fn selected_object(&self) -> Option<(usize, u32, [f64; 4])> {
        let i = self.object?;
        let (sheet, v) = self.doc.as_ref()?.current()?;
        let o = sheet.layout.objects.get(i).filter(|o| !o.hidden && o.slot != NO_SLOT)?;
        let rect = match self.moving {
            Some(m) if m.slot == o.slot => m.rect,
            _ => anchor_rect(&o.anchor, v),
        };
        Some((i, o.slot, rect))
    }

    /// O objeto de cima no ponto (os controles de formulário ficam de fora:
    /// a posição deles também está em outras partes do arquivo).
    fn object_at(&self, g: &Geom, x: f32, y: f32) -> Option<usize> {
        // Protegidos, não se deixam selecionar: o clique vai para a célula.
        if self.objects_locked() {
            return None;
        }
        let (sheet, v) = self.doc.as_ref()?.current()?;
        let q = g.quads.iter().find(|q| !q.rect.is_empty() && q.rect.contains(x, y))?;
        let hit = sheet.layout.objects.iter().enumerate().rev().find(|(_, o)| {
            !o.hidden && touches(o, &on_pane(q, crate::chart::object_rect(o, v, None), g.g), x, y, g.s)
        });
        hit.filter(|(_, o)| o.slot != NO_SLOT && !matches!(o.kind, ObjectKind::Control(_))).map(|(i, _)| i)
    }

    /// A alça do objeto selecionado no ponto.
    fn grip_at(&self, g: &Geom, x: f32, y: f32) -> Option<Grip> {
        let (_, _, rect) = self.selected_object()?;
        if !g.grid.contains(x, y) {
            return None;
        }
        let near = 7.0 * g.s;
        g.quads.iter().filter(|q| !q.rect.is_empty() && q.rect.inset(-near).contains(x, y)).find_map(|q| {
            let r = on_pane(q, rect, g.g);
            GRIPS.into_iter().find(|k| {
                let c = grip_point(&r, *k);
                (c[0] - x).abs() <= near && (c[1] - y).abs() <= near
            })
        })
    }

    /// Cursor sobre um objeto que pode ser movido ou redimensionado.
    pub fn object_cursor(&self, g: &Geom, x: f32, y: f32) -> Option<Cursor> {
        if self.editor.is_some() || !g.grid.contains(x, y) {
            return None;
        }
        if let Some(k) = self.grip_at(g, x, y) {
            return Some(match (k.0, k.1) {
                (0, _) => Cursor::ResizeRow,
                (_, 0) => Cursor::ResizeColumn,
                (a, b) if a == b => Cursor::ResizeNwse,
                _ => Cursor::ResizeNesw,
            });
        }
        self.object_at(g, x, y).map(|_| Cursor::Move)
    }

    /// Botão nas alças do objeto selecionado. `true` = era numa.
    pub fn press_grip(&mut self, g: &Geom, x: f32, y: f32, mods: Modifiers) -> bool {
        let (Some(grip), Some((_, slot, from))) = (self.grip_at(g, x, y), self.selected_object()) else {
            return false;
        };
        self.drag = Some(Drag::Object { slot, grip, from, x, y, keep: mods.shift });
        true
    }

    /// Botão num objeto: seleciona e começa o arraste. `true` = era num.
    pub fn press_object(&mut self, g: &Geom, x: f32, y: f32, mods: Modifiers) -> bool {
        if !g.grid.contains(x, y) {
            return false;
        }
        self.object = self.object_at(g, x, y);
        let Some((_, slot, from)) = self.selected_object() else { return false };
        self.commit_edit();
        let grip = Grip(0, 0);
        self.drag = Some(Drag::Object { slot, grip, from, x, y, keep: mods.shift });
        true
    }

    /// O mouse andou com um objeto pego.
    pub fn drag_object(&mut self, d: Drag, g: &Geom, x: f32, y: f32) {
        let Drag::Object { slot, grip, from, x: x0, y: y0, keep } = d else { return };
        // Um clique que treme não é arraste.
        if self.moving.is_none() && (x - x0).abs().max((y - y0).abs()) < 3.0 * g.s {
            return;
        }
        let (dx, dy) = (((x - x0) / g.g) as f64, ((y - y0) / g.g) as f64);
        let rect = dragged(from, grip, dx, dy, keep).map(f64::round);
        self.message = format!("{} × {} pixels", rect[2], rect[3]);
        self.moving = Some(Moving { slot, rect });
    }

    /// Fim do arraste: vira comando.
    pub fn drop_object(&mut self) {
        let Some(m) = self.moving.take() else { return };
        self.message.clear();
        self.place_object(m);
    }

    /// Põe a âncora no lugar novo (um passo de desfazer).
    pub fn place_object(&mut self, m: Moving) -> bool {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return false };
        let Some(a) = sheet.layout.drawing_anchors.get(m.slot as usize) else { return false };
        let new = anchor_at(m.rect, &a.anchor, v);
        if new == a.anchor {
            return false;
        }
        self.run_command(Command::Anchors(vec![(m.slot, new)]))
    }

    /// Teclas com um objeto selecionado. `true` = tratada.
    pub fn object_key(&mut self, key: &Key, mods: Modifiers) -> bool {
        let Some((_, slot, rect)) = self.selected_object() else { return false };
        let step = if mods.ctrl || mods.logo { 1.0 } else { 8.0 };
        let (dx, dy) = match key {
            Key::ArrowLeft => (-step, 0.0),
            Key::ArrowRight => (step, 0.0),
            Key::ArrowUp => (0.0, -step),
            Key::ArrowDown => (0.0, step),
            Key::Escape => {
                // No meio do arraste, só desiste dele.
                if self.moving.take().is_some() {
                    self.drag = None;
                    self.message.clear();
                } else {
                    self.object = None;
                }
                return true;
            }
            Key::Delete | Key::Backspace => {
                self.message = "Excluir objetos ainda não é possível".into();
                return true;
            }
            _ => return false,
        };
        self.place_object(Moving { slot, rect: dragged(rect, Grip(0, 0), dx, dy, false) });
        true
    }

    /// `--edit mover-objeto=N:dx,dy` e `--edit tamanho-objeto=N:dl,da` (o
    /// N-ésimo objeto da aba, em pixels a 100%). `true` = era um deles.
    pub fn scripted_object(&mut self, e: &str) -> bool {
        let Some((op, arg)) = e.split_once('=') else { return false };
        let grip = match op {
            "mover-objeto" => Grip(0, 0),
            "tamanho-objeto" => Grip(1, 1),
            _ => return false,
        };
        let nums = arg.split_once(':').and_then(|(n, d)| {
            let (dx, dy) = d.split_once(',')?;
            Some((n.parse().ok()?, dx.parse::<f64>().ok()?, dy.parse::<f64>().ok()?))
        });
        let Some((n, dx, dy)) = nums else {
            eprintln!("--edit: use {op}=0:40,20");
            return true;
        };
        self.object = Some(n);
        match self.selected_object() {
            Some((_, slot, rect)) => {
                let done = self.place_object(Moving { slot, rect: dragged(rect, grip, dx, dy, false) });
                eprintln!("--edit {op}: {}", if done { "ok" } else { "nada mudou" });
            }
            None => eprintln!("--edit: objeto {n} não encontrado"),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drags() {
        let r = [100.0, 50.0, 200.0, 100.0];
        assert_eq!(dragged(r, Grip(0, 0), 30.0, -20.0, false), [130.0, 30.0, 200.0, 100.0]);
        // Não sai pela borda da aba; com Shift, um eixo só.
        assert_eq!(dragged(r, Grip(0, 0), -300.0, -70.0, false), [0.0, 0.0, 200.0, 100.0]);
        assert_eq!(dragged(r, Grip(0, 0), 30.0, -20.0, true), [130.0, 50.0, 200.0, 100.0]);
        assert_eq!(dragged(r, Grip(1, 1), 40.0, 10.0, false), [100.0, 50.0, 240.0, 110.0]);
        assert_eq!(dragged(r, Grip(-1, 0), 40.0, 10.0, false), [140.0, 50.0, 160.0, 100.0]);
        assert_eq!(dragged(r, Grip(0, -1), 40.0, -60.0, false), [100.0, 0.0, 200.0, 150.0]);
        // Nunca menor que o mínimo, e o lado oposto fica parado.
        assert_eq!(dragged(r, Grip(-1, 0), 900.0, 0.0, false), [284.0, 50.0, 16.0, 100.0]);
        // Canto com Shift: a proporção fica.
        assert_eq!(dragged(r, Grip(1, 1), 100.0, 5.0, true), [100.0, 50.0, 300.0, 150.0]);
        assert_eq!(dragged(r, Grip(-1, -1), 100.0, 5.0, true), [200.0, 100.0, 100.0, 50.0]);
    }
}
