//! Formas do DrawingML (F5): geometria predefinida ou livre, preenchimento,
//! linha e texto formatado. A geometria sai como polígonos (curvas já
//! quebradas em segmentos) no retângulo da forma.

use crate::drawing::ChartColor;

#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    /// `a:prstGeom`: nome e ajustes (`a:gd name="adj1" fmla="val 25000"`).
    Preset(String, Vec<(String, f64)>),
    /// `a:custGeom`: contornos em frações do retângulo.
    Custom(Vec<Vec<[f32; 2]>>),
}

impl Default for Geom {
    fn default() -> Self {
        Geom::Preset("rect".into(), Vec::new())
    }
}

/// Cor com opacidade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub color: ChartColor,
    pub alpha: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Fill {
    #[default]
    None,
    Solid(Paint),
    /// Paradas (posição 0–1) e ângulo em graus (0 = da esquerda para a
    /// direita, 90 = de cima para baixo).
    Gradient(Vec<(f32, Paint)>, f32),
    /// Imagem (parte do pacote).
    Picture(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub paint: Paint,
    /// Espessura em pontos.
    pub width: f32,
    pub dash: bool,
    /// Pontas de seta no começo e no fim (conectores).
    pub head: bool,
    pub tail: bool,
}

/// Sombra externa: deslocamento e desfoque em pixels a 100%.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub paint: Paint,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAnchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub text: String,
    /// Tamanho em pontos (padrão 11).
    pub size: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<ChartColor>,
    pub font: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Para {
    pub align: TextAlign,
    pub runs: Vec<Run>,
    /// Tamanho da linha vazia (`a:endParaRPr`).
    pub end_size: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextBody {
    pub paras: Vec<Para>,
    pub anchor: TextAnchor,
    /// Margens (esquerda, cima, direita, baixo) em pixels a 100%.
    pub insets: [f32; 4],
    pub wrap: bool,
    /// Redução do autoajuste (`normAutofit fontScale`).
    pub scale: f32,
    /// Cor do texto sem cor própria (`xdr:style/a:fontRef`).
    pub color: Option<ChartColor>,
}

impl Default for TextBody {
    fn default() -> Self {
        Self {
            paras: Vec::new(),
            anchor: TextAnchor::Top,
            insets: [9.6, 4.8, 9.6, 4.8],
            wrap: true,
            scale: 1.0,
            color: None,
        }
    }
}

impl TextBody {
    pub fn is_empty(&self) -> bool {
        self.paras.iter().all(|p| p.runs.iter().all(|r| r.text.trim().is_empty()))
    }

    /// O texto sem formatação, um parágrafo por linha.
    pub fn plain(&self) -> String {
        self.paras
            .iter()
            .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub geom: Geom,
    pub fill: Fill,
    pub line: Option<Line>,
    pub shadow: Option<Shadow>,
    pub text: TextBody,
    /// Rotação em graus (sentido horário) e espelhamento.
    pub rot: f32,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Shape {
    /// Contornos da forma num retângulo de `w` × `h` (origem no canto).
    pub fn outline(&self, w: f32, h: f32) -> Vec<Vec<[f32; 2]>> {
        let mut paths: Vec<Vec<[f32; 2]>> = match &self.geom {
            Geom::Custom(p) => p.iter().map(|c| c.iter().map(|q| [q[0] * w, q[1] * h]).collect()).collect(),
            Geom::Preset(name, adj) => preset(name, adj, w, h),
        };
        let (cx, cy) = (w / 2.0, h / 2.0);
        let (sin, cos) = self.rot.to_radians().sin_cos();
        for q in paths.iter_mut().flatten() {
            if self.flip_h {
                q[0] = w - q[0];
            }
            if self.flip_v {
                q[1] = h - q[1];
            }
            if self.rot != 0.0 {
                let (x, y) = (q[0] - cx, q[1] - cy);
                *q = [cx + x * cos - y * sin, cy + x * sin + y * cos];
            }
        }
        paths
    }

    /// Só uma linha (sem área): conectores e `prst="line"`.
    pub fn is_line(&self) -> bool {
        matches!(&self.geom, Geom::Preset(n, _) if n == "line" || n.contains("Connector"))
    }

    /// Retângulo sem giro: dá para desenhar com as primitivas da grade.
    pub fn is_rect(&self) -> bool {
        self.rot == 0.0 && matches!(&self.geom, Geom::Preset(n, _) if n == "rect")
    }
}

/// Arco de elipse de `a0` a `a1` graus (0 = direita, 90 = baixo).
fn arc(out: &mut Vec<[f32; 2]>, cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32) {
    let n = ((rx.max(ry) * (a1 - a0).abs().to_radians() / 2.5).ceil() as usize).clamp(4, 64);
    for i in 0..=n {
        let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
        out.push([cx + rx * a.cos(), cy + ry * a.sin()]);
    }
}

/// Retângulo com cada canto (sup. esq., sup. dir., inf. dir., inf. esq.)
/// cortado (`snip`) ou arredondado pela medida dele.
fn corners(w: f32, h: f32, r: [f32; 4], snip: bool) -> Vec<Vec<[f32; 2]>> {
    let mut p = Vec::new();
    for (i, &(x, y)) in [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].iter().enumerate() {
        let r = r[i].min(w / 2.0).min(h / 2.0);
        if r <= 0.0 {
            p.push([x, y]);
            continue;
        }
        let (cx, cy) = (if x > 0.0 { w - r } else { r }, if y > 0.0 { h - r } else { r });
        if snip {
            if i % 2 == 0 {
                p.extend([[x, cy], [cx, y]]);
            } else {
                p.extend([[cx, y], [x, cy]]);
            }
        } else {
            let a0 = 180.0 + 90.0 * i as f32;
            arc(&mut p, cx, cy, r, r, a0, a0 + 90.0);
        }
    }
    vec![p]
}

fn preset(name: &str, adj: &[(String, f64)], w: f32, h: f32) -> Vec<Vec<[f32; 2]>> {
    // Ajuste em frações (o arquivo guarda em 100000 avos).
    let a = |k: &str, d: f32| {
        adj.iter().find(|(n, _)| n == k).map_or(d, |(_, v)| (*v / 100_000.0) as f32).max(0.0)
    };
    let ss = w.min(h);
    let pts: Vec<[f32; 2]> = match name {
        "ellipse" | "flowChartConnector" => {
            let mut p = Vec::new();
            arc(&mut p, w / 2.0, h / 2.0, w / 2.0, h / 2.0, 0.0, 360.0);
            p.pop();
            p
        }
        "roundRect" | "flowChartAlternateProcess" => {
            return corners(w, h, [ss * a("adj", 0.16667).min(0.5); 4], false);
        }
        "round1Rect" => return corners(w, h, [0.0, ss * a("adj", 0.16667), 0.0, 0.0], false),
        "round2SameRect" => {
            let (t, b) = (ss * a("adj1", 0.16667), ss * a("adj2", 0.0));
            return corners(w, h, [t, t, b, b], false);
        }
        "round2DiagRect" => {
            let (t, b) = (ss * a("adj1", 0.16667), ss * a("adj2", 0.0));
            return corners(w, h, [t, b, t, b], false);
        }
        "snip1Rect" => return corners(w, h, [0.0, ss * a("adj", 0.16667), 0.0, 0.0], true),
        "snip2SameRect" => {
            let (t, b) = (ss * a("adj1", 0.16667), ss * a("adj2", 0.0));
            return corners(w, h, [t, t, b, b], true);
        }
        "snip2DiagRect" => {
            let (t, b) = (ss * a("adj1", 0.0), ss * a("adj2", 0.16667));
            return corners(w, h, [t, b, t, b], true);
        }
        "octagon" => return corners(w, h, [ss * a("adj", 0.29289); 4], true),
        "line" | "straightConnector1" => vec![[0.0, 0.0], [w, h]],
        "bentConnector3" => {
            let x = w * a("adj1", 0.5);
            vec![[0.0, 0.0], [x, 0.0], [x, h], [w, h]]
        }
        "triangle" | "flowChartExtract" => vec![[w * a("adj", 0.5).min(1.0), 0.0], [w, h], [0.0, h]],
        "rtTriangle" => vec![[0.0, 0.0], [w, h], [0.0, h]],
        "diamond" | "flowChartDecision" => vec![[w / 2.0, 0.0], [w, h / 2.0], [w / 2.0, h], [0.0, h / 2.0]],
        "parallelogram" => {
            let x = ss * a("adj", 0.25);
            vec![[x, 0.0], [w, 0.0], [w - x, h], [0.0, h]]
        }
        "trapezoid" => {
            let x = ss * a("adj", 0.25);
            vec![[x, 0.0], [w - x, 0.0], [w, h], [0.0, h]]
        }
        "hexagon" => {
            let x = ss * a("adj", 0.25);
            vec![[0.0, h / 2.0], [x, 0.0], [w - x, 0.0], [w, h / 2.0], [w - x, h], [x, h]]
        }
        "plus" => {
            let x = ss * a("adj", 0.25);
            vec![
                [x, 0.0],
                [w - x, 0.0],
                [w - x, x],
                [w, x],
                [w, h - x],
                [w - x, h - x],
                [w - x, h],
                [x, h],
                [x, h - x],
                [0.0, h - x],
                [0.0, x],
                [x, x],
            ]
        }
        "homePlate" => {
            let x = w - (ss * a("adj", 0.5)).min(w);
            vec![[0.0, 0.0], [x, 0.0], [w, h / 2.0], [x, h], [0.0, h]]
        }
        "chevron" => {
            let d = (ss * a("adj", 0.5)).min(w);
            vec![[0.0, 0.0], [w - d, 0.0], [w, h / 2.0], [w - d, h], [0.0, h], [d, h / 2.0]]
        }
        "rightArrow" | "leftArrow" | "upArrow" | "downArrow" => {
            // Desenhada para a direita e depois virada.
            let vert = name == "upArrow" || name == "downArrow";
            let (l, t) = if vert { (h, w) } else { (w, h) };
            let body = t * a("adj1", 0.5).min(1.0);
            let head = (ss * a("adj2", 0.5)).min(l);
            let (y0, y1) = ((t - body) / 2.0, (t + body) / 2.0);
            [
                [0.0, y0],
                [l - head, y0],
                [l - head, 0.0],
                [l, t / 2.0],
                [l - head, t],
                [l - head, y1],
                [0.0, y1],
            ]
            .iter()
            .map(|&[x, y]| match name {
                "leftArrow" => [l - x, y],
                "downArrow" => [y, x],
                "upArrow" => [y, l - x],
                _ => [x, y],
            })
            .collect()
        }
        "leftRightArrow" => {
            let body = h * a("adj1", 0.5).min(1.0);
            let head = (ss * a("adj2", 0.5)).min(w / 2.0);
            let (y0, y1) = ((h - body) / 2.0, (h + body) / 2.0);
            vec![
                [0.0, h / 2.0],
                [head, 0.0],
                [head, y0],
                [w - head, y0],
                [w - head, 0.0],
                [w, h / 2.0],
                [w - head, h],
                [w - head, y1],
                [head, y1],
                [head, h],
            ]
        }
        _ => vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]],
    };
    vec![pts]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(p: &[[f32; 2]]) -> f32 {
        let n = p.len();
        (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f32>().abs() / 2.0
    }

    fn shape(name: &str, adj: &[(&str, f64)]) -> Shape {
        let adj = adj.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        Shape { geom: Geom::Preset(name.into(), adj), ..Default::default() }
    }

    #[test]
    fn preset_areas() {
        let a = |n: &str, adj: &[(&str, f64)]| area(&shape(n, adj).outline(200.0, 100.0)[0]);
        assert_eq!(a("rect", &[]), 20_000.0);
        assert!((a("ellipse", &[]) - std::f32::consts::PI * 5000.0).abs() < 60.0);
        // Cantos arredondados: tira 4 cantos de (1 − π/4) r².
        let r = 16.667f32;
        let want = 20_000.0 - (4.0 - std::f32::consts::PI) * r * r;
        assert!((a("roundRect", &[]) - want).abs() < 10.0);
        // Dois cantos de cima cortados (triângulos de 50 × 50).
        assert_eq!(a("snip2SameRect", &[("adj1", 50_000.0), ("adj2", 0.0)]), 20_000.0 - 2500.0);
        // Seta: corpo 150 × 50 + ponta de 50 × 100 / 2.
        assert_eq!(a("rightArrow", &[]), 150.0 * 50.0 + 2500.0);
        assert_eq!(a("leftArrow", &[]), a("rightArrow", &[]));
        // homePlate: 150 × 100 + triângulo.
        assert_eq!(a("homePlate", &[]), 17_500.0);
        assert_eq!(a("desconhecida", &[]), 20_000.0);
    }

    #[test]
    fn flips_and_rotation() {
        let mut s = shape("homePlate", &[]);
        s.flip_h = true;
        assert!(s.outline(200.0, 100.0)[0].contains(&[0.0, 50.0]));
        let mut s = shape("line", &[]);
        assert!(s.is_line());
        s.flip_v = true;
        assert_eq!(s.outline(10.0, 20.0)[0], [[0.0, 20.0], [10.0, 0.0]]);
        let mut s = shape("rect", &[]);
        s.rot = 90.0;
        let p = s.outline(200.0, 100.0)[0][0];
        assert!((p[0] - 150.0).abs() < 1e-3 && (p[1] + 50.0).abs() < 1e-3, "{p:?}");
    }
}
