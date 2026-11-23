//! Desenho das formas e imagens da aba: preenchimento (cor, gradiente,
//! imagem), contorno, sombra e texto formatado com quebra de linha.

use duck_core::Styles;
use duck_core::drawing::ChartColor;
use duck_core::shape::{Fill, Line, Run, Shape, TextAlign, TextAnchor, TextBody};
use duck_text::{TextStyle, TextSystem};
use duck_ui::{Painter, Rect, Rgba, rgb, triangulate};

use crate::chart::{ObjCtx, paint_rgba, resolve};

/// Faixas de um gradiente.
const BANDS: usize = 32;

/// Desenha a forma em `r`, recortada por `clip` (o painel).
pub fn draw_shape(p: &mut Painter, cx: &ObjCtx, s: &Shape, r: Rect, clip: &Rect) {
    let paths: Vec<Vec<[f32; 2]>> = s
        .outline(r.w(), r.h())
        .into_iter()
        .map(|path| path.into_iter().map(|q| [q[0] + r.x0, q[1] + r.y0]).collect())
        .collect();
    if s.is_line() {
        if let (Some(l), Some(path)) = (&s.line, paths.first()) {
            draw_open_line(p, cx, l, path, clip);
        }
        return;
    }
    if let Some(sh) = s.shadow {
        draw_shadow(p, cx, sh, &paths, clip);
    }
    match &s.fill {
        Fill::None => {}
        Fill::Solid(c) => {
            let color = paint_rgba(*c, cx.styles);
            if s.is_rect() {
                p.fill(r, clip, color);
            } else {
                paths.iter().for_each(|path| p.shape(path, clip, color));
            }
        }
        Fill::Gradient(stops, angle) => {
            let stops: Vec<(f32, Rgba)> =
                stops.iter().map(|(t, c)| (*t, paint_rgba(*c, cx.styles))).collect();
            paths.iter().for_each(|path| gradient(p, path, r, &stops, *angle, clip));
        }
        Fill::Picture(part) => draw_picture(p, cx, part, [0.0; 4], r, &r.intersect(clip)),
    }
    if let Some(l) = &s.line {
        let color = paint_rgba(l.paint, cx.styles);
        let w = (l.width * 96.0 / 72.0 * cx.g).max(1.0);
        for path in &paths {
            if l.dash {
                dashes(p, path, w, clip, color);
            } else {
                p.outline(path, clip, w, color);
            }
        }
    }
    if !s.text.is_empty() {
        draw_text(p, cx, &s.text, r, &r.intersect(clip));
    }
}

fn draw_open_line(p: &mut Painter, cx: &ObjCtx, l: &Line, path: &[[f32; 2]], clip: &Rect) {
    let color = paint_rgba(l.paint, cx.styles);
    let w = (l.width * 96.0 / 72.0 * cx.g).max(1.0);
    for k in path.windows(2) {
        if l.dash {
            dashes(p, k, w, clip, color);
        } else {
            p.line(k[0], k[1], w, clip, color);
        }
    }
    let n = path.len();
    if n >= 2 {
        if l.tail {
            arrow(p, path[n - 2], path[n - 1], w, clip, color);
        }
        if l.head {
            arrow(p, path[1], path[0], w, clip, color);
        }
    }
}

/// Ponta de seta em `b`, vindo de `a`.
fn arrow(p: &mut Painter, a: [f32; 2], b: [f32; 2], w: f32, clip: &Rect, color: Rgba) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (ux, uy) = (dx / len, dy / len);
    let (l, h) = (w * 3.5 + 2.0, w * 1.8 + 1.0);
    let base = [b[0] - ux * l, b[1] - uy * l];
    p.shape(&[b, [base[0] - uy * h, base[1] + ux * h], [base[0] + uy * h, base[1] - ux * h]], clip, color);
}

/// Contorno tracejado (aberto se só tem dois pontos).
fn dashes(p: &mut Painter, path: &[[f32; 2]], w: f32, clip: &Rect, color: Rgba) {
    let (on, off) = (w * 4.0, w * 3.0);
    let n = path.len();
    let mut at = 0.0;
    for i in 0..if n == 2 { 1 } else { n } {
        let (a, b) = (path[i], path[(i + 1) % n]);
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let mut t = 0.0;
        while t < len {
            let phase = at % (on + off);
            let step = if phase < on { on - phase } else { on + off - phase }.min(len - t).max(0.1);
            if phase < on {
                let q = |t: f32| [a[0] + (b[0] - a[0]) * t / len, a[1] + (b[1] - a[1]) * t / len];
                p.line(q(t), q(t + step), w, clip, color);
            }
            t += step;
            at += step;
        }
    }
}

fn draw_shadow(
    p: &mut Painter,
    cx: &ObjCtx,
    sh: duck_core::shape::Shadow,
    paths: &[Vec<[f32; 2]>],
    clip: &Rect,
) {
    const LAYERS: usize = 6;
    let mut color = paint_rgba(sh.paint, cx.styles);
    // Camadas de tamanhos diferentes imitam o desfoque.
    color[3] = (sh.paint.alpha * 255.0 / LAYERS as f32).round().max(1.0) as u8;
    let blur = sh.blur * cx.g;
    for path in paths {
        let n = path.len().max(1) as f32;
        let (mx, my) = path.iter().fold((0.0, 0.0), |a, q| (a.0 + q[0] / n, a.1 + q[1] / n));
        let half = path.iter().fold(1.0f32, |m, q| m.max((q[0] - mx).abs()).max((q[1] - my).abs()));
        for i in 0..LAYERS {
            let grow = (i as f32 / (LAYERS - 1) as f32 - 0.5) * blur;
            let f = (half + grow) / half;
            let pts: Vec<[f32; 2]> = path
                .iter()
                .map(|q| [mx + (q[0] - mx) * f + sh.dx * cx.g, my + (q[1] - my) * f + sh.dy * cx.g])
                .collect();
            for t in triangulate(&pts) {
                p.triangle(pts[t[0]], pts[t[1]], pts[t[2]], clip, color);
            }
        }
    }
}

/// Cor do gradiente na posição `t`.
fn stop_color(stops: &[(f32, Rgba)], t: f32) -> Rgba {
    let k = stops.partition_point(|s| s.0 <= t);
    if k == 0 {
        return stops[0].1;
    }
    if k == stops.len() {
        return stops[k - 1].1;
    }
    let (a, b) = (stops[k - 1], stops[k]);
    let f = ((t - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
    std::array::from_fn(|i| (a.1[i] as f32 + (b.1[i] as f32 - a.1[i] as f32) * f).round() as u8)
}

/// A parte do polígono convexo com `d·p >= k`.
fn half_plane(poly: &[[f32; 2]], d: [f32; 2], k: f32) -> Vec<[f32; 2]> {
    let side = |q: &[f32; 2]| d[0] * q[0] + d[1] * q[1] - k;
    let mut out = Vec::with_capacity(poly.len() + 1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (sa, sb) = (side(&a), side(&b));
        if sa >= 0.0 {
            out.push(a);
        }
        if (sa >= 0.0) != (sb >= 0.0) {
            let t = sa / (sa - sb);
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

/// Gradiente linear em faixas (cada faixa cobre até o fim: sem frestas).
fn gradient(p: &mut Painter, path: &[[f32; 2]], r: Rect, stops: &[(f32, Rgba)], angle: f32, clip: &Rect) {
    let (sin, cos) = angle.to_radians().sin_cos();
    let d = [cos, sin];
    let corners = [[r.x0, r.y0], [r.x1, r.y0], [r.x0, r.y1], [r.x1, r.y1]];
    let proj = corners.map(|q| d[0] * q[0] + d[1] * q[1]);
    let (lo, hi) =
        (proj.iter().fold(f32::MAX, |a, &b| a.min(b)), proj.iter().fold(f32::MIN, |a, &b| a.max(b)));
    let bands = BANDS.min(((hi - lo) / 2.0).ceil().max(2.0) as usize);
    // Cores com transparência não podem se sobrepor: cada faixa fica só nela.
    let opaque = stops.iter().all(|s| s.1[3] == 255);
    let tris = triangulate(path);
    for i in 0..bands {
        let color = stop_color(stops, (i as f32 + 0.5) / bands as f32);
        let k0 = lo + (hi - lo) * i as f32 / bands as f32;
        let k1 = lo + (hi - lo) * (i + 1) as f32 / bands as f32;
        for t in &tris {
            let mut poly = vec![path[t[0]], path[t[1]], path[t[2]]];
            if i > 0 {
                poly = half_plane(&poly, d, k0);
            }
            if !opaque && i + 1 < bands {
                poly = half_plane(&poly, [-d[0], -d[1]], -k1);
            }
            p.polygon(&poly, clip, color);
        }
    }
    // Borda suave na cor do gradiente em cada trecho.
    if opaque {
        for i in 0..path.len() {
            let (a, b) = (path[i], path[(i + 1) % path.len()]);
            let m = d[0] * (a[0] + b[0]) / 2.0 + d[1] * (a[1] + b[1]) / 2.0;
            p.line(a, b, 0.7, clip, stop_color(stops, (m - lo) / (hi - lo).max(1e-3)));
        }
    }
}

/// Imagem em `r`, sem as bordas cortadas (`crop` em frações da imagem).
pub fn draw_picture(p: &mut Painter, cx: &ObjCtx, part: &str, crop: [f32; 4], r: Rect, clip: &Rect) {
    let Some(&[a0, b0, a1, b1]) = cx.images.get(part) else { return };
    let (du, dv) = (a1 - a0, b1 - b0);
    let (u0, u1) = (a0 + crop[0] * du, a1 - crop[2] * du);
    let (v0, v1) = (b0 + crop[1] * dv, b1 - crop[3] * dv);
    if clip.is_empty() || u1 <= u0 || v1 <= v0 {
        return;
    }
    // Recorta o retângulo e a região do atlas juntos.
    let fx = |x: f32| u0 + (x - r.x0) / r.w() * (u1 - u0);
    let fy = |y: f32| v0 + (y - r.y0) / r.h() * (v1 - v0);
    p.out.push(duck_gfx::Instance::image(
        [clip.x0, clip.y0, clip.w(), clip.h()],
        [fx(clip.x0), fy(clip.y0), fx(clip.x1), fy(clip.y1)],
        255,
    ));
}

// ---------------------------------------------------------------------------
// Texto
// ---------------------------------------------------------------------------

/// Um pedaço de texto numa linha: posição, largura e o trecho de origem.
struct Piece<'a> {
    text: &'a str,
    style: TextStyle,
    color: Rgba,
    run: &'a Run,
    w: f32,
}

struct TextLine<'a> {
    pieces: Vec<Piece<'a>>,
    w: f32,
    h: f32,
    align: TextAlign,
}

fn run_style(text: &mut TextSystem, st: &Styles, run: &Run, body: &TextBody, g: f32) -> TextStyle {
    let family = match run.font.as_deref() {
        None | Some("+mn-lt") => st.theme.minor_font.as_str(),
        Some("+mj-lt") => st.theme.major_font.as_str(),
        Some(f) => f,
    };
    let px = run.size.unwrap_or(11.0) * 96.0 / 72.0 * body.scale * g;
    text.style(family, px.round().max(4.0), run.bold, run.italic)
}

/// Quebra o texto em linhas de no máximo `width` pixels.
fn layout_text<'a>(
    text: &mut TextSystem,
    st: &Styles,
    body: &'a TextBody,
    width: f32,
    g: f32,
) -> Vec<TextLine<'a>> {
    let mut lines = Vec::new();
    for para in &body.paras {
        let new = || TextLine { pieces: Vec::new(), w: 0.0, h: 0.0, align: para.align };
        let mut line = new();
        for run in &para.runs {
            let style = run_style(text, st, run, body, g);
            let lh = text.line_height_of(style);
            let color = match run.color.or(body.color) {
                Some(c) => {
                    let v = resolve(c, st);
                    rgb(v)
                }
                None => rgb(resolve(
                    ChartColor::Scheme { idx: 1, lum_mod: 1.0, lum_off: 0.0, shade: 1.0, tint: 1.0 },
                    st,
                )),
            };
            for (i, seg) in run.text.split('\n').enumerate() {
                if i > 0 {
                    line.h = line.h.max(lh);
                    lines.push(std::mem::replace(&mut line, new()));
                }
                // Palavras com o espaço que vem depois delas.
                for word in seg.split_inclusive(' ') {
                    let trimmed = word.trim_end_matches(' ');
                    let w = text.measure_styled(word, style);
                    let tw = text.measure_styled(trimmed, style);
                    if body.wrap && line.w > 0.0 && line.w + tw > width + 0.5 {
                        lines.push(std::mem::replace(&mut line, new()));
                    }
                    line.h = line.h.max(lh);
                    line.w += w;
                    line.pieces.push(Piece { text: word, style, color, run, w });
                }
                line.h = line.h.max(lh);
            }
        }
        if para.runs.is_empty() {
            let r = Run { size: para.end_size, ..Default::default() };
            let s = run_style(text, st, &r, body, g);
            line.h = text.line_height_of(s);
        }
        lines.push(line);
    }
    // O espaço do fim não conta na largura da linha.
    for l in &mut lines {
        if let Some(p) = l.pieces.last_mut() {
            let t = p.text.trim_end_matches(' ');
            if t.len() < p.text.len() {
                let w = text.measure_styled(t, p.style);
                l.w -= p.w - w;
                p.w = w;
                p.text = t;
            }
        }
    }
    lines
}

pub fn draw_text(p: &mut Painter, cx: &ObjCtx, body: &TextBody, r: Rect, clip: &Rect) {
    if clip.is_empty() {
        return;
    }
    let g = cx.g;
    let i = body.insets.map(|v| v * g);
    let area = Rect::new(r.x0 + i[0], r.y0 + i[1], (r.x1 - i[2]).max(r.x0 + i[0]), r.y1 - i[3]);
    let lines = layout_text(p.text, cx.styles, body, area.w(), g);
    let total: f32 = lines.iter().map(|l| l.h).sum();
    let mut y = match body.anchor {
        TextAnchor::Top => area.y0,
        TextAnchor::Middle => area.y0 + (area.h() - total) / 2.0,
        TextAnchor::Bottom => area.y1 - total,
    }
    .floor();
    for l in &lines {
        let mut x = match l.align {
            TextAlign::Left => area.x0,
            TextAlign::Center => area.x0 + (area.w() - l.w) / 2.0,
            TextAlign::Right => area.x1 - l.w,
        }
        .floor();
        if y + l.h >= clip.y0 && y <= clip.y1 {
            for piece in &l.pieces {
                // Trechos menores apoiam na base da linha.
                let lh = p.text.line_height_of(piece.style);
                let ty = y + (l.h - lh) * 0.8;
                p.text(piece.text, piece.style, x, ty, clip, piece.color);
                let t = (piece.style.px() / 14.0).round().max(1.0);
                if piece.run.underline {
                    let uy = (ty + lh * 0.86).round();
                    p.fill(Rect::new(x, uy, x + piece.w, uy + t), clip, piece.color);
                }
                if piece.run.strike {
                    let uy = (ty + lh * 0.55).round();
                    p.fill(Rect::new(x, uy, x + piece.w, uy + t), clip, piece.color);
                }
                x += piece.w;
            }
        }
        y += l.h;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_stops() {
        let s = [(0.0, [0, 0, 0, 255]), (0.5, [100, 0, 0, 255]), (1.0, [100, 200, 0, 255])];
        assert_eq!(stop_color(&s, 0.25), [50, 0, 0, 255]);
        assert_eq!(stop_color(&s, 0.75), [100, 100, 0, 255]);
        assert_eq!(stop_color(&s, 1.0), [100, 200, 0, 255]);
    }

    #[test]
    fn half_planes() {
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let right = half_plane(&sq, [1.0, 0.0], 4.0);
        assert!(right.iter().all(|q| q[0] >= 4.0) && right.len() == 4, "{right:?}");
        assert!(half_plane(&sq, [1.0, 0.0], 11.0).is_empty());
    }
}
