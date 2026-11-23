//! Ícones da formatação condicional, desenhados como formas (não dependem das
//! fontes do sistema e ficam nítidos em qualquer zoom).

use duck_ui::{Painter, Rect, Rgba};

const RED: Rgba = [0xE0, 0x44, 0x3E, 255];
const YELLOW: Rgba = [0xF1, 0xC2, 0x32, 255];
const GREEN: Rgba = [0x3E, 0x9B, 0x3E, 255];
const PINK: Rgba = [0xF4, 0xA6, 0xA6, 255];
const GRAY: Rgba = [0x80, 0x80, 0x80, 255];
const LIGHT: Rgba = [0xD9, 0xD9, 0xD9, 255];
const BLACK: Rgba = [0x3B, 0x3B, 0x3B, 255];
const BLUE: Rgba = [0x44, 0x72, 0xC4, 255];
const WHITE: Rgba = [255, 255, 255, 255];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    None,
    Disc,
    /// Círculo dentro de uma moldura escura (semáforo com aro).
    Rimmed,
    /// Seta; graus no sentido horário a partir de "para cima".
    Arrow(f32),
    Up,
    Down,
    Dash,
    Diamond,
    Flag,
    /// Símbolos (`true` = dentro de um círculo).
    Check(bool),
    Bang(bool),
    Cross(bool),
    /// Estrela com 0, 1 ou 2 metades cheias.
    Star(u8),
    /// Círculo com quartos cheios.
    Pie(u8),
    /// Barras de sinal e quadrados: quantos estão cheios.
    Bars(u8),
    Boxes(u8),
}

/// Um ícone já escolhido.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Icon {
    shape: Shape,
    color: Rgba,
}

/// Ícone `idx` (0 = o mais baixo) de um conjunto de `n`.
pub fn icon(set: &str, idx: usize, n: usize) -> Icon {
    let ramp = |colors: &[Rgba]| colors[(idx * colors.len() / n.max(1)).min(colors.len() - 1)];
    let traffic = ramp(&[RED, YELLOW, GREEN]);
    let i3 = idx.min(2);
    let arrow = match (n, idx) {
        (_, 0) => 180.0,
        (3, 1) | (5, 2) => 90.0,
        (4, 1) | (5, 1) => 135.0,
        (4, 2) | (5, 3) => 45.0,
        _ => 0.0,
    };
    let warm = if idx == 0 {
        RED
    } else if idx + 1 >= n {
        GREEN
    } else {
        YELLOW
    };
    let (shape, color) = match set {
        "NoIcons" => (Shape::None, WHITE),
        "3Arrows" | "4Arrows" | "5Arrows" => (Shape::Arrow(arrow), warm),
        "3ArrowsGray" | "4ArrowsGray" | "5ArrowsGray" => (Shape::Arrow(arrow), GRAY),
        "3Flags" => (Shape::Flag, traffic),
        "3Symbols" | "3Symbols2" => {
            let round = set == "3Symbols";
            ([Shape::Cross(round), Shape::Bang(round), Shape::Check(round)][i3], traffic)
        }
        "3Signs" => ([Shape::Diamond, Shape::Up, Shape::Disc][i3], traffic),
        "3Stars" => (Shape::Star(i3 as u8), YELLOW),
        "3Triangles" => ([Shape::Down, Shape::Dash, Shape::Up][i3], traffic),
        "3TrafficLights2" => (Shape::Rimmed, traffic),
        "4TrafficLights" => (Shape::Disc, [BLACK, RED, YELLOW, GREEN][idx.min(3)]),
        "4RedToBlack" => (Shape::Disc, [BLACK, GRAY, PINK, RED][idx.min(3)]),
        "4Rating" => (Shape::Bars(idx.min(3) as u8 + 1), BLUE),
        "5Rating" => (Shape::Bars(idx.min(4) as u8), BLUE),
        "5Quarters" => (Shape::Pie(idx.min(4) as u8), BLACK),
        "5Boxes" => (Shape::Boxes(idx.min(4) as u8), BLUE),
        // `3TrafficLights1` e o padrão.
        _ => (Shape::Disc, traffic),
    };
    Icon { shape, color }
}

/// Desenha o ícone num quadrado de lado `s` com canto em (x, y).
pub fn draw(p: &mut Painter, ic: Icon, x: f32, y: f32, s: f32, clip: &Rect) {
    let at = |u: f32, v: f32| [x + u * s, y + v * s];
    let poly = |pts: &[[f32; 2]]| pts.iter().map(|q| at(q[0], q[1])).collect::<Vec<_>>();
    let c = ic.color;
    let center = at(0.5, 0.5);
    let bar = |u0: f32, v0: f32, u1: f32, v1: f32| {
        let (a, b) = (at(u0, v0), at(u1, v1));
        Rect::new(a[0].round(), a[1].round(), b[0].round(), b[1].round())
    };
    match ic.shape {
        Shape::None => {}
        Shape::Disc => p.disc(center, s * 0.43, clip, c),
        Shape::Rimmed => {
            p.fill(bar(0.04, 0.04, 0.96, 0.96), clip, BLACK);
            p.disc(center, s * 0.36, clip, c);
        }
        Shape::Arrow(deg) => {
            // Para cima, girada em volta do centro.
            let (sin, cos) = deg.to_radians().sin_cos();
            let pts: Vec<[f32; 2]> =
                [[0.5, 0.04], [0.9, 0.5], [0.65, 0.5], [0.65, 0.94], [0.35, 0.94], [0.35, 0.5], [0.1, 0.5]]
                    .iter()
                    .map(|q| {
                        let (u, v) = (q[0] - 0.5, q[1] - 0.5);
                        at(0.5 + u * cos - v * sin, 0.5 + u * sin + v * cos)
                    })
                    .collect();
            p.shape(&pts, clip, c);
        }
        Shape::Up => p.shape(&poly(&[[0.5, 0.12], [0.96, 0.86], [0.04, 0.86]]), clip, c),
        Shape::Down => p.shape(&poly(&[[0.04, 0.14], [0.96, 0.14], [0.5, 0.88]]), clip, c),
        Shape::Dash => p.fill(bar(0.05, 0.36, 0.95, 0.64), clip, c),
        Shape::Diamond => {
            p.shape(&poly(&[[0.5, 0.04], [0.96, 0.5], [0.5, 0.96], [0.04, 0.5]]), clip, c);
        }
        Shape::Flag => {
            p.fill(bar(0.14, 0.06, 0.24, 0.96), clip, GRAY);
            p.shape(&poly(&[[0.24, 0.08], [0.94, 0.34], [0.24, 0.6]]), clip, c);
        }
        Shape::Check(round) => {
            let (ink, w) = symbol(p, round, center, s, c, clip);
            p.line(at(0.24, 0.52), at(0.43, 0.7), w, clip, ink);
            p.line(at(0.43, 0.7), at(0.76, 0.32), w, clip, ink);
        }
        Shape::Cross(round) => {
            let (ink, w) = symbol(p, round, center, s, c, clip);
            p.line(at(0.3, 0.3), at(0.7, 0.7), w, clip, ink);
            p.line(at(0.7, 0.3), at(0.3, 0.7), w, clip, ink);
        }
        Shape::Bang(round) => {
            let (ink, w) = symbol(p, round, center, s, c, clip);
            p.line(at(0.5, 0.22), at(0.5, 0.58), w, clip, ink);
            p.disc(at(0.5, 0.75), w * 0.6, clip, ink);
        }
        Shape::Star(halves) => {
            let pts: Vec<[f32; 2]> = (0..10)
                .map(|k| {
                    let r = if k % 2 == 0 { 0.48 } else { 0.2 };
                    let (sin, cos) = (k as f32 * 36.0).to_radians().sin_cos();
                    at(0.5 + r * sin, 0.53 - r * cos)
                })
                .collect();
            p.shape(&pts, clip, if halves == 2 { c } else { LIGHT });
            if halves == 1 {
                // Metade esquerda: do topo (0) à ponta de baixo (5).
                let left: Vec<[f32; 2]> = [0, 9, 8, 7, 6, 5].iter().map(|&k: &usize| pts[k]).collect();
                p.shape(&left, clip, c);
            }
        }
        Shape::Pie(quarters) => {
            p.disc(center, s * 0.46, clip, c);
            if quarters < 4 {
                p.disc(center, s * 0.38, clip, WHITE);
                // Fatia a partir do topo, no sentido horário.
                let steps = quarters as usize * 6;
                let mut pts = vec![center];
                pts.extend((0..=steps).map(|k| {
                    let (sin, cos) = (k as f32 * 15.0).to_radians().sin_cos();
                    [center[0] + s * 0.4 * sin, center[1] - s * 0.4 * cos]
                }));
                p.polygon(&pts, clip, c);
            }
        }
        Shape::Bars(full) => {
            for k in 0..4 {
                let u = 0.06 + 0.23 * k as f32;
                let top = 0.7 - 0.2 * k as f32;
                p.fill(bar(u, top, u + 0.18, 0.94), clip, if k < full { c } else { LIGHT });
            }
        }
        Shape::Boxes(full) => {
            // De baixo para cima, da esquerda para a direita.
            for k in 0..4u8 {
                let (u, v) = (0.08 + 0.45 * (k % 2) as f32, 0.53 - 0.45 * (k / 2) as f32);
                p.fill(bar(u, v, u + 0.39, v + 0.39), clip, if k < full { c } else { LIGHT });
            }
        }
    }
}

/// Fundo de um símbolo; devolve a cor e a espessura do traço.
fn symbol(p: &mut Painter, round: bool, center: [f32; 2], s: f32, c: Rgba, clip: &Rect) -> (Rgba, f32) {
    if round {
        p.disc(center, s * 0.46, clip, c);
        (WHITE, s * 0.13)
    } else {
        (c, s * 0.17)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O mais baixo é o ruim; o mais alto, o bom.
    #[test]
    fn sets() {
        assert_eq!(icon("3TrafficLights1", 0, 3), Icon { shape: Shape::Disc, color: RED });
        assert_eq!(icon("3TrafficLights1", 2, 3), Icon { shape: Shape::Disc, color: GREEN });
        assert_eq!(icon("3Triangles", 1, 3), Icon { shape: Shape::Dash, color: YELLOW });
        assert_eq!(icon("3Arrows", 1, 3), Icon { shape: Shape::Arrow(90.0), color: YELLOW });
        assert_eq!(icon("5Arrows", 3, 5), Icon { shape: Shape::Arrow(45.0), color: YELLOW });
        assert_eq!(icon("4TrafficLights", 0, 4).color, BLACK);
        assert_eq!(icon("5Quarters", 4, 5).shape, Shape::Pie(4));
        assert_eq!(icon("NoIcons", 0, 0).shape, Shape::None);
        // Um conjunto desconhecido ou um índice alto demais não quebra.
        for set in ["3Stars", "3Flags", "3Signs", "3Symbols", "4Rating", "5Boxes", "outro"] {
            icon(set, 9, 3);
        }
    }
}
