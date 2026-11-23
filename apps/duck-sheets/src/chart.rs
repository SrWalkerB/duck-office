//! Desenho dos objetos da aba (F5): gráficos, segmentações de dados e formas.
//!
//! Os objetos ficam presos às células (âncora): a posição na tela sai das
//! larguras e alturas da visão, então acompanham rolagem, zoom e painéis
//! congelados como as células. Cada gráfico é desenhado com retângulos,
//! triângulos e texto do `Painter`.

use duck_core::drawing::{
    Anchor, Chart, ChartColor, Grouping, LegendPos, Object, ObjectKind, Plot, PlotKind, Slicer, Timeline,
    map_rgb,
};
use duck_core::{NumFmt, Styles};
use duck_text::TextStyle;
use duck_ui::{Align, Painter, Rect, Rgba, rgb};

use crate::app::{Quad, View};

const EMU_PER_PX: f64 = 9525.0;
const AXIS_TEXT: Rgba = rgb(0x595959);
const GRIDLINE: Rgba = rgb(0xD9D9D9);
const FRAME_LINE: Rgba = rgb(0xD9D9D9);

/// Retângulo do objeto em pixels lógicos da aba (100%).
pub fn object_rect(o: &Object, v: &View) -> [f64; 4] {
    let x = |c: u32, off: i64| v.cols.offset(c.min(v.cols.len())) as f64 + off as f64 / EMU_PER_PX;
    let y = |r: u32, off: i64| v.rows.offset(r.min(v.rows.len())) as f64 + off as f64 / EMU_PER_PX;
    let (x0, y0, x1, y1) = match o.anchor {
        Anchor::Two { from, to } => (
            x(from.col, from.col_off),
            y(from.row, from.row_off),
            x(to.col, to.col_off),
            y(to.row, to.row_off),
        ),
        Anchor::One { from, cx, cy } => {
            let (a, b) = (x(from.col, from.col_off), y(from.row, from.row_off));
            (a, b, a + cx as f64 / EMU_PER_PX, b + cy as f64 / EMU_PER_PX)
        }
        Anchor::Abs { x, y, cx, cy } => {
            let (a, b) = (x as f64 / EMU_PER_PX, y as f64 / EMU_PER_PX);
            (a, b, a + cx as f64 / EMU_PER_PX, b + cy as f64 / EMU_PER_PX)
        }
    };
    let (w, h) = (x1 - x0, y1 - y0);
    let f = o.frac;
    [x0 + f[0] as f64 * w, y0 + f[1] as f64 * h, f[2] as f64 * w, f[3] as f64 * h]
}

/// Cor final (tema e ajustes do DrawingML).
pub fn resolve(c: ChartColor, st: &Styles) -> u32 {
    match c {
        ChartColor::Rgb(v) => v,
        ChartColor::Scheme { idx, lum_mod, lum_off, shade, tint } => {
            duck_core::drawing::adjust_color(st.theme.color(idx), lum_mod, lum_off, shade, tint)
        }
    }
}

fn rgba(v: u32) -> Rgba {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
}

/// Cor padrão da série `k` (destaques do tema, mais claros/escuros depois do 6º).
fn auto_color(k: usize, st: &Styles) -> u32 {
    let base = st.theme.color(4 + (k % 6) as u8);
    match k / 6 {
        0 => base,
        1 => map_rgb(base, |c| c * 0.6),
        2 => map_rgb(base, |c| c + (255.0 - c) * 0.4),
        _ => map_rgb(base, |c| c * 0.8),
    }
}

/// Tudo o que o desenho dos objetos precisa.
pub struct ObjCtx<'a> {
    pub styles: &'a Styles,
    pub font: TextStyle,
    pub g: f32,
    pub date1904: bool,
    /// Imagens já na GPU: parte → região no atlas.
    pub images: &'a std::collections::HashMap<String, [f32; 4]>,
}

/// Desenha o intervalo de uma câmera num retângulo, recortado; `false` = usar
/// a imagem guardada.
pub type Camera<'a> = &'a mut dyn FnMut(&mut Painter, &str, Rect, &Rect) -> bool;

/// Desenha os objetos da aba que aparecem no painel `q`.
pub fn draw_objects(p: &mut Painter, cx: &ObjCtx, objects: &[Object], v: &View, q: &Quad, live: Camera<'_>) {
    if objects.is_empty() || q.rect.is_empty() {
        return;
    }
    for o in objects {
        if o.hidden {
            continue;
        }
        let [x, y, w, h] = object_rect(o, v);
        let r = Rect::new(
            q.rect.x0 + ((x - q.ox) * cx.g as f64) as f32,
            q.rect.y0 + ((y - q.oy) * cx.g as f64) as f32,
            q.rect.x0 + ((x + w - q.ox) * cx.g as f64) as f32,
            q.rect.y0 + ((y + h - q.oy) * cx.g as f64) as f32,
        );
        // Conectores podem ter altura ou largura zero.
        let line = matches!(&o.kind, ObjectKind::Shape(s) if s.is_line());
        let small = !line && (r.w() < 2.0 || r.h() < 2.0);
        if small || r.inset(-2.0).intersect(&q.rect).is_empty() {
            continue;
        }
        let clip = r.intersect(&q.rect);
        match &o.kind {
            ObjectKind::Chart(c) => draw_chart(p, cx, c, r, &clip),
            ObjectKind::Slicer(s) => draw_slicer(p, cx, s, r, &clip),
            ObjectKind::Timeline(t) => draw_timeline(p, cx, t, r, &clip),
            ObjectKind::Control(c) => crate::control::draw(p, cx, c, r, &clip),
            ObjectKind::Shape(s) => crate::shape::draw_shape(p, cx, s, r, &q.rect),
            ObjectKind::Picture { part, crop, line, camera } => {
                if !camera.as_ref().is_some_and(|c| live(p, c, r, &clip)) {
                    crate::shape::draw_picture(p, cx, part, *crop, r, &clip);
                }
                if let Some(l) = line {
                    let w = (l.width * 96.0 / 72.0 * cx.g).max(1.0);
                    p.stroke(r, &clip, w, paint_rgba(l.paint, cx.styles));
                }
            }
        }
    }
}

pub fn paint_rgba(p: duck_core::shape::Paint, st: &Styles) -> Rgba {
    let mut c = rgba(resolve(p.color, st));
    c[3] = (p.alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    c
}

fn luminance(v: u32) -> f64 {
    let (r, g, b) = (((v >> 16) & 0xFF) as f64, ((v >> 8) & 0xFF) as f64, (v & 0xFF) as f64);
    (0.299 * r + 0.587 * g + 0.114 * b) / 255.0
}

// ---------------------------------------------------------------------------
// Números
// ---------------------------------------------------------------------------

fn fmt_num(v: f64, code: Option<&str>, date1904: bool) -> String {
    let mut s = String::new();
    match code.filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("general")) {
        Some(c) => {
            NumFmt::parse(c).format_number(v, date1904, &mut s);
        }
        None => duck_core::numfmt::format_general(v, 11, &mut s),
    }
    s
}

/// Escala "bonita": (mínimo, máximo, passo) com uns 5 intervalos.
fn nice_scale(lo: f64, hi: f64, fixed_min: Option<f64>, fixed_max: Option<f64>) -> (f64, f64, f64) {
    let (mut lo, mut hi) = (lo.min(0.0), hi.max(0.0));
    if let Some(m) = fixed_min {
        lo = m;
    }
    if let Some(m) = fixed_max {
        hi = m;
    }
    if hi <= lo {
        hi = lo + 1.0;
    }
    let raw = (hi - lo) / 5.0;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = mag
        * if norm <= 1.0 {
            1.0
        } else if norm <= 2.0 {
            2.0
        } else if norm <= 2.5 {
            2.5
        } else if norm <= 5.0 {
            5.0
        } else {
            10.0
        };
    let lo = if fixed_min.is_some() { lo } else { (lo / step).floor() * step };
    let hi = if fixed_max.is_some() { hi } else { (hi / step).ceil() * step };
    (lo, hi, step)
}

// ---------------------------------------------------------------------------
// Gráficos
// ---------------------------------------------------------------------------

/// Estilo e cor de um texto do gráfico (`c:txPr`), com o tamanho padrão `pt`.
fn text_of(
    cx: &ObjCtx,
    tp: Option<duck_core::drawing::TextProps>,
    pt: f32,
    color: Rgba,
) -> (TextStyle, Rgba) {
    let size = tp.and_then(|t| t.size).unwrap_or(pt);
    let mut st = cx.font.with_px((size * 96.0 / 72.0 * cx.g).round().max(5.0));
    if let Some(b) = tp.and_then(|t| t.bold) {
        st.bold = b;
    }
    let c = tp.and_then(|t| t.color).map_or(color, |c| rgba(resolve(c, cx.styles)));
    (st, c)
}

struct Legend {
    entries: Vec<(String, Rgba, bool)>,
}

fn series_color(s: &duck_core::drawing::Series, k: usize, line: bool, st: &Styles) -> Rgba {
    let c = if line { s.line.or(s.fill) } else { s.fill.or(s.line) };
    rgba(c.map_or_else(|| auto_color(k, st), |c| resolve(c, st)))
}

fn draw_chart(p: &mut Painter, cx: &ObjCtx, c: &Chart, r: Rect, clip: &Rect) {
    // Séries com todas as células escondidas ficam de fora.
    let shown;
    let c = if c.plots.iter().any(|p| p.series.iter().any(|s| s.hidden)) {
        let mut v = c.clone();
        v.plots.iter_mut().for_each(|p| p.series.retain(|s| !s.hidden));
        v.plots.retain(|p| !p.series.is_empty());
        shown = v;
        &shown
    } else {
        c
    };
    let g = cx.g;
    let st = cx.styles;
    if !c.no_background {
        let bg = c.background.map_or(0xFFFFFF, |b| resolve(b, st));
        p.fill(r, clip, rgba(bg));
    }
    if let Some(b) = c.border {
        p.stroke(r, clip, g.max(1.0), rgba(resolve(b, st)));
    } else if !c.no_background && c.background.is_none() {
        p.stroke(r, clip, g.max(1.0), FRAME_LINE);
    }
    let small = cx.font.with_px((9.0 * 96.0 / 72.0 * g).round().max(5.0));
    let lh = p.text.line_height_of(small);
    let pad = 6.0 * g;
    let mut area = r.inset(pad);

    if let Some(t) = &c.title
        && !t.is_empty()
    {
        let (ts, tc) = text_of(cx, c.title_text, 14.0, rgb(0x595959));
        let tlh = p.text.line_height_of(ts);
        p.text_aligned(t, ts, area.x0, area.x1, area.y0, Align::Center, clip, tc);
        area.y0 += tlh + 4.0 * g;
    }

    // Legenda.
    let pie = c.plots.first().is_some_and(|p| matches!(p.kind, PlotKind::Pie | PlotKind::Doughnut));
    let mut legend = Legend { entries: Vec::new() };
    let mut k = 0;
    for plot in &c.plots {
        for s in &plot.series {
            if pie || plot.vary_colors && plot.series.len() == 1 {
                for (i, cat) in s.cats.iter().enumerate() {
                    let col = s
                        .points
                        .iter()
                        .find(|pt| pt.0 as usize == i)
                        .map_or_else(|| rgba(auto_color(i, st)), |pt| rgba(resolve(pt.1, st)));
                    legend.entries.push((cat.clone(), col, false));
                }
            } else {
                let line = matches!(plot.kind, PlotKind::Line | PlotKind::Scatter);
                legend.entries.push((s.name.clone(), series_color(s, k, line, st), line));
            }
            k += 1;
        }
    }
    if let Some(pos) = c.legend {
        let (small, legend_color) = text_of(cx, c.legend_text, 9.0, AXIS_TEXT);
        let lh = p.text.line_height_of(small);
        let sw = 8.0 * g;
        match pos {
            LegendPos::Bottom | LegendPos::Top => {
                let widths: Vec<f32> = legend
                    .entries
                    .iter()
                    .map(|e| sw + 4.0 * g + p.text.measure_styled(&e.0, small) + 10.0 * g)
                    .collect();
                let total: f32 = widths.iter().sum();
                let y = if pos == LegendPos::Bottom { area.y1 - lh } else { area.y0 };
                let mut x = (area.x0 + (area.w() - total) / 2.0).max(area.x0);
                for (e, w) in legend.entries.iter().zip(&widths) {
                    draw_key(p, e, x, y, sw, lh, g, clip);
                    p.text(&e.0, small, x + sw + 4.0 * g, y, clip, legend_color);
                    x += w;
                }
                if pos == LegendPos::Bottom {
                    area.y1 -= lh + 4.0 * g;
                } else {
                    area.y0 += lh + 4.0 * g;
                }
            }
            LegendPos::Right | LegendPos::Left => {
                let w = legend.entries.iter().map(|e| p.text.measure_styled(&e.0, small)).fold(0.0, f32::max)
                    + sw
                    + 10.0 * g;
                let w = w.min(area.w() * 0.4);
                let x = if pos == LegendPos::Right { area.x1 - w } else { area.x0 };
                let mut y =
                    area.y0 + (area.h() - legend.entries.len() as f32 * (lh + 2.0 * g)).max(0.0) / 2.0;
                for e in &legend.entries {
                    draw_key(p, e, x, y, sw, lh, g, clip);
                    p.text(
                        &e.0,
                        small,
                        x + sw + 4.0 * g,
                        y,
                        &clip.intersect(&Rect::new(x, y, x + w, y + lh)),
                        legend_color,
                    );
                    y += lh + 2.0 * g;
                }
                if pos == LegendPos::Right {
                    area.x1 -= w + 4.0 * g;
                } else {
                    area.x0 += w + 4.0 * g;
                }
            }
        }
    }

    if pie {
        draw_pie(p, cx, &c.plots[0], area, clip, small);
        return;
    }

    // Área do gráfico: manual (frações do quadro) ou o que sobrou.
    let mut plot = match c.plot_layout {
        Some([x, y, w, h]) => {
            let fr =
                Rect::new(r.x0 + x * r.w(), r.y0 + y * r.h(), r.x0 + (x + w) * r.w(), r.y0 + (y + h) * r.h());
            fr.intersect(&r)
        }
        None => area,
    };
    let horizontal = c.plots.iter().any(|p| p.kind == PlotKind::BarH);
    let n_cats = c
        .plots
        .iter()
        .flat_map(|p| &p.series)
        .map(|s| s.vals.len().max(s.cats.len()))
        .max()
        .unwrap_or(0)
        .max(1);
    let cats: Vec<String> = c
        .plots
        .iter()
        .flat_map(|p| &p.series)
        .find(|s| !s.cats.is_empty())
        .map(|s| s.cats.clone())
        .unwrap_or_else(|| (1..=n_cats).map(|i| i.to_string()).collect());

    // Escalas (principal e secundária).
    let scale_of = |sec: bool| {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        let mut any = false;
        for pl in c.plots.iter().filter(|p| p.secondary == sec) {
            match pl.grouping {
                Grouping::Percent
                    if !matches!(pl.kind, PlotKind::Pie | PlotKind::Doughnut | PlotKind::Scatter) =>
                {
                    lo = lo.min(0.0);
                    hi = hi.max(1.0);
                    any = true;
                }
                Grouping::Stacked
                    if !matches!(pl.kind, PlotKind::Pie | PlotKind::Doughnut | PlotKind::Scatter) =>
                {
                    for i in 0..n_cats {
                        let (mut pos, mut neg) = (0.0, 0.0);
                        for s in &pl.series {
                            let v = s.vals.get(i).copied().flatten().unwrap_or(0.0);
                            if v >= 0.0 { pos += v } else { neg += v }
                        }
                        lo = lo.min(neg);
                        hi = hi.max(pos);
                        any = true;
                    }
                }
                _ => {
                    for s in &pl.series {
                        for v in s.vals.iter().flatten() {
                            lo = lo.min(*v);
                            hi = hi.max(*v);
                            any = true;
                        }
                    }
                }
            }
        }
        any.then_some((lo, hi))
    };
    let prim = scale_of(false).unwrap_or((0.0, 1.0));
    let (lo, hi, step) = nice_scale(prim.0, prim.1, c.val_axis.min, c.val_axis.max);
    let sec = scale_of(true).map(|(a, b)| {
        let ax = c.sec_axis.clone().unwrap_or_default();
        nice_scale(a, b, ax.min, ax.max)
    });
    let val_fmt =
        c.val_axis.fmt.clone().or_else(|| c.plots.iter().flat_map(|p| &p.series).find_map(|s| s.fmt.clone()));

    // Espaço dos rótulos dos eixos (quando a área não é manual).
    if c.plot_layout.is_none() {
        if !c.val_axis.deleted {
            let w = [lo, hi]
                .iter()
                .map(|v| p.text.measure_styled(&fmt_num(*v, val_fmt.as_deref(), cx.date1904), small))
                .fold(0.0, f32::max);
            if horizontal {
                plot.y1 -= lh + 4.0 * g;
            } else {
                plot.x0 += w + 6.0 * g;
            }
        }
        if sec.is_some() && c.sec_axis.as_ref().is_some_and(|a| !a.deleted) {
            plot.x1 -= 40.0 * g;
        }
        if !c.cat_axis.deleted {
            if horizontal {
                let w = cats
                    .iter()
                    .map(|s| p.text.measure_styled(s, small))
                    .fold(0.0, f32::max)
                    .min(plot.w() * 0.35);
                plot.x0 += w + 6.0 * g;
            } else {
                plot.y1 -= lh + 4.0 * g;
            }
        }
    }
    if plot.w() < 4.0 || plot.h() < 4.0 {
        return;
    }

    let (val_style, val_color) = text_of(cx, c.val_axis.text, 9.0, AXIS_TEXT);
    let (cat_style, cat_color) = text_of(cx, c.cat_axis.text, 9.0, AXIS_TEXT);
    let (sec_style, sec_color) = text_of(cx, c.sec_axis.as_ref().and_then(|a| a.text), 9.0, AXIS_TEXT);
    // Linhas de grade e rótulos do eixo de valores.
    let to_px = |v: f64, (lo, hi): (f64, f64)| -> f32 {
        let t = ((v - lo) / (hi - lo)).clamp(-0.05, 1.05) as f32;
        if horizontal { plot.x0 + t * plot.w() } else { plot.y1 - t * plot.h() }
    };
    let mut t = lo;
    let mut guard = 0;
    while t <= hi + step * 1e-9 && guard < 100 {
        let pos = to_px(t, (lo, hi));
        if c.val_axis.gridlines || c.val_axis.deleted {
            if horizontal {
                p.fill(Rect::new(pos, plot.y0, pos + g.max(1.0), plot.y1), clip, GRIDLINE);
            } else {
                p.fill(Rect::new(plot.x0, pos, plot.x1, pos + g.max(1.0)), clip, GRIDLINE);
            }
        }
        if !c.val_axis.deleted {
            let s = fmt_num(t, val_fmt.as_deref(), cx.date1904);
            if horizontal {
                p.text_aligned(
                    &s,
                    val_style,
                    pos - 40.0 * g,
                    pos + 40.0 * g,
                    plot.y1 + 2.0 * g,
                    Align::Center,
                    clip,
                    val_color,
                );
            } else {
                p.text_aligned(
                    &s,
                    val_style,
                    r.x0,
                    plot.x0 - 4.0 * g,
                    pos - lh / 2.0,
                    Align::Right,
                    clip,
                    val_color,
                );
            }
        }
        t += step;
        guard += 1;
    }
    if let (Some((slo, shi, sstep)), Some(ax)) = (sec, &c.sec_axis)
        && !ax.deleted
        && !horizontal
    {
        let mut t = slo;
        let mut guard = 0;
        while t <= shi + sstep * 1e-9 && guard < 100 {
            let pos = to_px(t, (slo, shi));
            let s = fmt_num(t, ax.fmt.as_deref(), cx.date1904);
            p.text(&s, sec_style, plot.x1 + 4.0 * g, pos - lh / 2.0, clip, sec_color);
            t += sstep;
            guard += 1;
        }
    }
    // Eixo de categorias.
    let slot = if horizontal { plot.h() / n_cats as f32 } else { plot.w() / n_cats as f32 };
    let center = |i: usize| -> f32 {
        if horizontal { plot.y0 + (i as f32 + 0.5) * slot } else { plot.x0 + (i as f32 + 0.5) * slot }
    };
    let base = to_px(0.0f64.clamp(lo, hi), (lo, hi));
    if horizontal {
        p.fill(Rect::new(base, plot.y0, base + g.max(1.0), plot.y1), clip, rgb(0xBFBFBF));
    } else {
        p.fill(Rect::new(plot.x0, base, plot.x1, base + g.max(1.0)), clip, rgb(0xBFBFBF));
    }
    if !c.cat_axis.deleted {
        let widest = cats.iter().map(|s| p.text.measure_styled(s, small)).fold(0.0, f32::max);
        let every = if horizontal {
            ((lh / slot).ceil() as usize).max(1)
        } else {
            ((widest / slot).ceil() as usize).max(1)
        };
        for (i, s) in cats.iter().enumerate().take(n_cats) {
            if i % every != 0 {
                continue;
            }
            if horizontal {
                p.text_aligned(
                    s,
                    cat_style,
                    r.x0,
                    plot.x0 - 4.0 * g,
                    center(i) - lh / 2.0,
                    Align::Right,
                    clip,
                    cat_color,
                );
            } else {
                let x = center(i);
                let half = (slot * every as f32) / 2.0;
                let lclip = clip.intersect(&Rect::new(x - half, plot.y1, x + half, r.y1));
                p.text_aligned(
                    s,
                    cat_style,
                    x - half,
                    x + half,
                    plot.y1 + 3.0 * g,
                    Align::Center,
                    &lclip,
                    cat_color,
                );
            }
        }
    }

    // Séries.
    let mut k = 0;
    for pl in &c.plots {
        let scale = if pl.secondary { sec.map_or((lo, hi), |s| (s.0, s.1)) } else { (lo, hi) };
        let fmt_s = |s: &duck_core::drawing::Series| s.fmt.clone().or_else(|| val_fmt.clone());
        match pl.kind {
            PlotKind::Bar | PlotKind::BarH => {
                let ns = pl.series.len().max(1) as f32;
                let gap = pl.gap as f32 / 100.0;
                let stacked = pl.grouping != Grouping::Clustered;
                let ov = if stacked { 1.0 } else { pl.overlap as f32 / 100.0 };
                let bars = if stacked { 1.0 } else { ns - (ns - 1.0) * ov };
                let bw = slot / (bars + gap);
                for i in 0..n_cats {
                    let total: f64 = if pl.grouping == Grouping::Percent {
                        pl.series.iter().map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0).abs()).sum()
                    } else {
                        1.0
                    };
                    let (mut acc_pos, mut acc_neg) = (0.0f64, 0.0f64);
                    let start = center(i) - (bars * bw) / 2.0;
                    for (j, s) in pl.series.iter().enumerate() {
                        let Some(mut v) = s.vals.get(i).copied().flatten() else { continue };
                        if pl.grouping == Grouping::Percent && total > 0.0 {
                            v /= total;
                        }
                        let (a, b) = if stacked {
                            let a = if v >= 0.0 { acc_pos } else { acc_neg };
                            let b = a + v;
                            if v >= 0.0 {
                                acc_pos = b
                            } else {
                                acc_neg = b
                            }
                            (a, b)
                        } else {
                            (0.0, v)
                        };
                        let off = if stacked { 0.0 } else { j as f32 * bw * (1.0 - ov) };
                        let (p0, p1) = (
                            to_px(a.clamp(scale.0, scale.1), scale),
                            to_px(b.clamp(scale.0, scale.1), scale),
                        );
                        let col = s
                            .points
                            .iter()
                            .find(|pt| pt.0 as usize == i)
                            .map_or_else(|| series_color(s, k + j, false, st), |pt| rgba(resolve(pt.1, st)));
                        let rect = if horizontal {
                            Rect::new(p0.min(p1), start + off, p0.max(p1), start + off + bw)
                        } else {
                            Rect::new(start + off, p0.min(p1), start + off + bw, p0.max(p1))
                        };
                        if !s.no_fill {
                            p.fill(rect, &clip.intersect(&plot.inset(-1.0)), col);
                        }
                        if s.labels.unwrap_or(pl.labels) {
                            let (small, given) =
                                text_of(cx, s.label_text.or(pl.label_text), 9.0, rgb(0x404040));
                            let explicit = s.label_text.or(pl.label_text).and_then(|t| t.color).is_some();
                            let lh = p.text.line_height_of(small);
                            let txt = fmt_num(
                                s.vals.get(i).copied().flatten().unwrap_or(0.0),
                                fmt_s(s).as_deref(),
                                cx.date1904,
                            );
                            let tw = p.text.measure_styled(&txt, small);
                            let fits = if horizontal { rect.w() > tw + 4.0 * g } else { rect.h() > lh };
                            let color = if explicit {
                                given
                            } else if stacked
                                && fits
                                && luminance(u32::from_be_bytes([0, col[0], col[1], col[2]])) < 0.6
                            {
                                rgb(0xFFFFFF)
                            } else {
                                given
                            };
                            let fill = s.label_fill.or(pl.label_fill).map(|f| rgba(resolve(f, st)));
                            let (x, y) = if horizontal {
                                (
                                    if stacked { (rect.x0 + rect.x1 - tw) / 2.0 } else { rect.x1 + 3.0 * g },
                                    rect.y0 + (rect.h() - lh) / 2.0,
                                )
                            } else {
                                (
                                    (rect.x0 + rect.x1 - tw) / 2.0,
                                    if stacked {
                                        (rect.y0 + rect.y1 - lh) / 2.0
                                    } else {
                                        rect.y0 - lh - 2.0 * g
                                    },
                                )
                            };
                            if let Some(f) = fill {
                                p.fill(Rect::new(x - 2.0 * g, y, x + tw + 2.0 * g, y + lh), clip, f);
                            }
                            p.text(&txt, small, x, y, clip, color);
                        }
                    }
                }
                k += pl.series.len();
            }
            PlotKind::Line | PlotKind::Scatter | PlotKind::Area => {
                // Empilhadas: cada série soma às anteriores; em percentual, do total.
                let totals: Vec<f64> = (0..n_cats)
                    .map(|i| {
                        pl.series.iter().map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0).abs()).sum()
                    })
                    .collect();
                let mut acc = vec![0.0f64; n_cats];
                for s in &pl.series {
                    let col = series_color(s, k, pl.kind != PlotKind::Area, st);
                    let w = (s.line_width.unwrap_or(2.25) * 96.0 / 72.0 * g).max(1.0);
                    let pts: Vec<Option<[f32; 2]>> = (0..n_cats)
                        .map(|i| {
                            s.vals.get(i).copied().flatten().map(|v| {
                                let v = match pl.grouping {
                                    Grouping::Stacked => {
                                        acc[i] += v;
                                        acc[i]
                                    }
                                    Grouping::Percent => {
                                        acc[i] += if totals[i] > 0.0 { v / totals[i] } else { 0.0 };
                                        acc[i]
                                    }
                                    Grouping::Clustered => v,
                                };
                                let vp = to_px(v.clamp(scale.0, scale.1), scale);
                                if horizontal { [vp, center(i)] } else { [center(i), vp] }
                            })
                        })
                        .collect();
                    let pclip = clip.intersect(&plot.inset(-4.0 * g));
                    if pl.kind == PlotKind::Area {
                        for win in pts.windows(2) {
                            if let [Some(a), Some(b)] = win {
                                p.polygon(
                                    &[[a[0], base], *a, *b, [b[0], base]],
                                    &pclip,
                                    [col[0], col[1], col[2], 200],
                                );
                            }
                        }
                    } else if !s.no_line {
                        for win in pts.windows(2) {
                            if let [Some(a), Some(b)] = win {
                                if s.dash {
                                    dashed_line(p, *a, *b, w, &pclip, col);
                                } else {
                                    p.line(*a, *b, w, &pclip, col);
                                }
                            }
                        }
                    }
                    for (i, pt) in pts.iter().enumerate() {
                        let Some(pt) = pt else { continue };
                        if s.marker || pl.kind == PlotKind::Scatter {
                            let m = 3.0 * g;
                            p.fill(Rect::new(pt[0] - m, pt[1] - m, pt[0] + m, pt[1] + m), &pclip, col);
                        }
                        if s.labels.unwrap_or(pl.labels) {
                            let (small, color) =
                                text_of(cx, s.label_text.or(pl.label_text), 9.0, rgb(0x404040));
                            let lh = p.text.line_height_of(small);
                            let txt = fmt_num(
                                s.vals.get(i).copied().flatten().unwrap_or(0.0),
                                fmt_s(s).as_deref(),
                                cx.date1904,
                            );
                            let tw = p.text.measure_styled(&txt, small);
                            let (x, y) = (pt[0] - tw / 2.0, pt[1] - lh - 3.0 * g);
                            if let Some(f) = s.label_fill.or(pl.label_fill) {
                                p.fill(
                                    Rect::new(x - 2.0 * g, y, x + tw + 2.0 * g, y + lh),
                                    clip,
                                    rgba(resolve(f, st)),
                                );
                            }
                            p.text(&txt, small, x, y, clip, color);
                        }
                    }
                    k += 1;
                }
            }
            _ => {}
        }
    }
}

/// Linha tracejada (traços de 4× a espessura).
fn dashed_line(p: &mut Painter, a: [f32; 2], b: [f32; 2], w: f32, clip: &Rect, color: Rgba) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return;
    }
    let dash = (4.0 * w).max(3.0);
    let mut t = 0.0;
    while t < len {
        let t1 = (t + dash).min(len);
        let pa = [a[0] + dx * t / len, a[1] + dy * t / len];
        let pb = [a[0] + dx * t1 / len, a[1] + dy * t1 / len];
        p.line(pa, pb, w, clip, color);
        t += dash * 2.0;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_key(
    p: &mut Painter,
    e: &(String, Rgba, bool),
    x: f32,
    y: f32,
    sw: f32,
    lh: f32,
    g: f32,
    clip: &Rect,
) {
    let cy = y + lh / 2.0;
    if e.2 {
        p.fill(Rect::new(x, cy - g.max(1.0), x + sw * 1.6, cy + g.max(1.0)), clip, e.1);
    } else {
        p.fill(Rect::new(x, cy - sw / 2.0, x + sw, cy + sw / 2.0), clip, e.1);
    }
}

fn draw_pie(p: &mut Painter, cx: &ObjCtx, pl: &Plot, area: Rect, clip: &Rect, small: TextStyle) {
    let Some(s) = pl.series.first() else { return };
    let vals: Vec<f64> = s.vals.iter().map(|v| v.unwrap_or(0.0).max(0.0)).collect();
    let total: f64 = vals.iter().sum();
    if total <= 0.0 {
        return;
    }
    let (cxp, cyp) = ((area.x0 + area.x1) / 2.0, (area.y0 + area.y1) / 2.0);
    let rad = area.w().min(area.h()) / 2.0 * 0.9;
    let hole = if pl.kind == PlotKind::Doughnut { rad * pl.hole as f32 / 100.0 } else { 0.0 };
    let mut a0 = -std::f64::consts::FRAC_PI_2;
    for (i, v) in vals.iter().enumerate() {
        let sweep = v / total * std::f64::consts::TAU;
        let col = s
            .points
            .iter()
            .find(|pt| pt.0 as usize == i)
            .map_or_else(|| rgba(auto_color(i, cx.styles)), |pt| rgba(resolve(pt.1, cx.styles)));
        let steps = ((sweep / 0.05).ceil() as usize).max(1);
        for k in 0..steps {
            let t0 = a0 + sweep * k as f64 / steps as f64;
            let t1 = a0 + sweep * (k + 1) as f64 / steps as f64;
            let pt = |t: f64, r: f32| [cxp + r * t.cos() as f32, cyp + r * t.sin() as f32];
            if hole > 0.0 {
                p.polygon(&[pt(t0, hole), pt(t0, rad), pt(t1, rad), pt(t1, hole)], clip, col);
            } else {
                p.triangle([cxp, cyp], pt(t0, rad), pt(t1, rad), clip, col);
            }
        }
        if s.labels.unwrap_or(pl.labels) && sweep > 0.15 {
            let mid = a0 + sweep / 2.0;
            let lr = (rad + hole) / 2.0;
            let txt = fmt_num(*v, s.fmt.as_deref(), cx.date1904);
            let (x, y) = (cxp + lr * mid.cos() as f32, cyp + lr * mid.sin() as f32);
            let lh = p.text.line_height_of(small);
            p.text_aligned(&txt, small, x - 40.0, x + 40.0, y - lh / 2.0, Align::Center, clip, rgb(0xFFFFFF));
        }
        a0 += sweep;
    }
}

// ---------------------------------------------------------------------------
// Segmentações de dados
// ---------------------------------------------------------------------------

/// Cores dos botões: (fundo, borda, texto) por estado.
struct SlicerLook {
    selected: (Rgba, Rgba, Rgba),
    selected_nodata: (Rgba, Rgba, Rgba),
    unselected: (Rgba, Rgba, Rgba),
    unselected_nodata: (Rgba, Rgba, Rgba),
}

fn slicer_look(name: Option<&str>, st: &Styles) -> SlicerLook {
    let accent = |k: u8| st.theme.color(4 + k);
    let light = |v: u32, t: f64| rgba(map_rgb(v, |c| c + (255.0 - c) * t));
    // Estilos prontos: SlicerStyleLight1..6, SlicerStyleDark1..6, SlicerStyleOther1..2.
    let builtin = |name: &str| -> SlicerLook {
        let (fam, n) = name
            .strip_prefix("SlicerStyle")
            .map(|r| {
                let d = r.trim_start_matches(|c: char| c.is_ascii_alphabetic());
                (&r[..r.len() - d.len()], d.parse::<u8>().unwrap_or(1).max(1))
            })
            .unwrap_or(("Light", 1));
        let a = accent((n - 1) % 6);
        match fam {
            "Dark" => SlicerLook {
                selected: (rgba(a), rgba(a), rgb(0xFFFFFF)),
                selected_nodata: (light(a, 0.5), light(a, 0.5), rgb(0xF2F2F2)),
                unselected: (light(a, 0.75), light(a, 0.6), rgb(0x262626)),
                unselected_nodata: (rgb(0xFFFFFF), rgb(0xD9D9D9), rgb(0xA6A6A6)),
            },
            _ => SlicerLook {
                selected: (light(a, 0.55), light(a, 0.25), rgb(0x262626)),
                selected_nodata: (light(a, 0.8), light(a, 0.6), rgb(0xA6A6A6)),
                unselected: (rgb(0xFFFFFF), rgb(0xD9D9D9), rgb(0x262626)),
                unselected_nodata: (rgb(0xFFFFFF), rgb(0xD9D9D9), rgb(0xA6A6A6)),
            },
        }
    };
    let name = name.unwrap_or("SlicerStyleLight1");
    let mut look = builtin(name);
    // Estilo do arquivo: cada elemento aponta para um formato diferencial.
    if let Some((_, els)) = st.slicer_styles.iter().find(|(n, _)| n == name) {
        for (kind, dxf) in els {
            let Some(d) = st.dxfs.get(*dxf as usize) else { continue };
            let fill = d.fill.and_then(|c| st.rgb(c)).map(rgba);
            let text = d.color.and_then(|c| st.rgb(c)).map(rgba);
            let line = d.border.iter().flatten().find_map(|b| st.rgb(b.color)).map(rgba);
            let slot = match kind.as_str() {
                "selectedItemWithData" => &mut look.selected,
                "selectedItemWithNoData" => &mut look.selected_nodata,
                "unselectedItemWithData" => &mut look.unselected,
                "unselectedItemWithNoData" => &mut look.unselected_nodata,
                _ => continue,
            };
            if let Some(f) = fill {
                slot.0 = f;
                slot.1 = line.unwrap_or(f);
            }
            if let Some(t) = text {
                slot.2 = t;
            }
        }
    }
    look
}

/// Onde ficam as partes de uma segmentação na tela.
pub struct SlicerGeom {
    /// Título (`None` se escondido).
    pub caption: Option<(f32, f32)>,
    /// Ícone de limpar o filtro.
    pub clear: Rect,
    /// Botão de cada item (os que cabem).
    pub items: Vec<Rect>,
}

pub fn slicer_geom(s: &Slicer, r: Rect, g: f32, lh: f32) -> SlicerGeom {
    let pad = 5.0 * g;
    let mut y = r.y0 + pad;
    let iy = y + lh / 2.0;
    let x = r.x1 - pad - 14.0 * g;
    let clear = Rect::new(x, iy - 6.0 * g, x + 12.0 * g, iy + 6.0 * g);
    let caption = if s.show_caption {
        let c = (r.x0 + pad, y);
        y += lh + 6.0 * g;
        Some(c)
    } else {
        None
    };
    let cols = s.columns.max(1) as f32;
    let gap = 4.0 * g;
    let bh = ((s.row_height as f64 / EMU_PER_PX) as f32 * g).max(lh + 2.0);
    let bw = ((r.w() - 2.0 * pad - (cols - 1.0) * gap) / cols).max(4.0);
    let mut items = Vec::new();
    for i in 0..s.items.len() {
        let row = (i as f32 / cols).floor();
        let colf = i as f32 - row * cols;
        let x = r.x0 + pad + colf * (bw + gap);
        let by = y + row * (bh + gap);
        if by > r.y1 - pad {
            break;
        }
        items.push(Rect::new(x, by, x + bw, (by + bh).min(r.y1 - pad)));
    }
    SlicerGeom { caption, clear, items }
}

fn draw_slicer(p: &mut Painter, cx: &ObjCtx, s: &Slicer, r: Rect, clip: &Rect) {
    let g = cx.g;
    let st = cx.styles;
    p.fill(r, clip, rgb(0xFFFFFF));
    p.stroke(r, clip, g.max(1.0), rgb(0xBFBFBF));
    let font = cx.font.with_px((8.0 * 96.0 / 72.0 * g).round().max(5.0));
    let lh = p.text.line_height_of(font);
    let geo = slicer_geom(s, r, g, lh);
    let any_unselected = s.items.iter().any(|i| !i.1);
    if let Some((x, y)) = geo.caption {
        let mut cap = font;
        cap.bold = true;
        p.text(
            &s.caption,
            cap,
            x,
            y,
            &clip.intersect(&Rect::new(r.x0, r.y0, geo.clear.x0 - 18.0 * g, r.y1)),
            rgb(0x262626),
        );
        // Ícone "várias seleções" e "limpar filtro" (este, ativo quando há filtro).
        let multi = Rect::new(geo.clear.x0 - 16.0 * g, geo.clear.y0, geo.clear.x0 - 4.0 * g, geo.clear.y1);
        p.stroke(multi, clip, g.max(1.0), rgb(0x8C8C8C));
        let c = if any_unselected { rgb(0xC00000) } else { rgb(0xBFBFBF) };
        p.stroke(geo.clear, clip, g.max(1.0), c);
        p.line(
            [geo.clear.x0 + 3.0 * g, geo.clear.y0 + 3.0 * g],
            [geo.clear.x1 - 3.0 * g, geo.clear.y1 - 3.0 * g],
            g.max(1.0),
            clip,
            c,
        );
        p.line(
            [geo.clear.x1 - 3.0 * g, geo.clear.y0 + 3.0 * g],
            [geo.clear.x0 + 3.0 * g, geo.clear.y1 - 3.0 * g],
            g.max(1.0),
            clip,
            c,
        );
    }
    let look = slicer_look(s.style.as_deref(), st);
    for (b, (text, sel, has)) in geo.items.iter().zip(&s.items) {
        let bclip = clip.intersect(b);
        // Tudo selecionado (sem filtro) também aparece destacado, como no Excel.
        let on = *sel || !any_unselected;
        let (fill, line, color) = match (on, *has) {
            (true, true) => look.selected,
            (true, false) => look.selected_nodata,
            (false, true) => look.unselected,
            (false, false) => look.unselected_nodata,
        };
        p.fill(*b, clip, fill);
        p.stroke(*b, clip, g.max(1.0), line);
        let ty = b.y0 + (b.h() - lh) / 2.0;
        p.text_aligned(text, font, b.x0 + 4.0 * g, b.x1 - 4.0 * g, ty, Align::Center, &bclip, color);
    }
}

impl crate::app::SheetsApp {
    /// Itens das segmentações de tabela do Excel: os valores da coluna (como
    /// aparecem nas células), marcados conforme o filtro da tabela. Feito ao
    /// abrir e quando um filtro muda.
    pub fn refresh_table_slicers(&mut self) {
        use rayon::prelude::*;
        let Some(doc) = &mut self.doc else { return };
        // (aba do objeto, objeto, aba da tabela, tabela, coluna na tabela)
        let mut jobs: Vec<(usize, usize, usize, usize, u32)> = Vec::new();
        for (si, slot) in doc.sheets.iter().enumerate() {
            let Some(sheet) = &slot.data else { continue };
            for (oi, o) in sheet.layout.objects.iter().enumerate() {
                let ObjectKind::Slicer(s) = &o.kind else { continue };
                let Some((tid, cid)) = &s.table else { continue };
                for (ti_sheet, slot2) in doc.sheets.iter().enumerate() {
                    let Some(sh2) = &slot2.data else { continue };
                    for (ti, t) in sh2.layout.tables.iter().enumerate() {
                        if t.id.to_string() != *tid {
                            continue;
                        }
                        let pat = format!(" id=\"{cid}\"");
                        if let Some(k) = t.column_xml.iter().position(|x| x.contains(&pat)) {
                            jobs.push((si, oi, ti_sheet, ti, k as u32));
                        }
                    }
                }
            }
        }
        if jobs.is_empty() {
            return;
        }
        let looks = doc.looks.clone();
        let strings = doc.strings.clone();
        let d1904 = doc.date1904;
        let sheets: Vec<Option<std::sync::Arc<duck_core::Sheet>>> =
            doc.sheets.iter().map(|s| s.data.clone()).collect();
        type Items = Vec<(String, bool, bool)>;
        let results: Vec<(usize, usize, Items)> = jobs
            .par_iter()
            .filter_map(|&(si, oi, ts, ti, k)| {
                let sheet = sheets[ts].as_ref()?;
                let t = &sheet.layout.tables[ti];
                let col = t.range.first_col + k;
                let r0 = t.range.first_row + t.header_rows;
                let r1 = t.range.last_row.saturating_sub(t.totals_rows);
                let mut seen: std::collections::HashMap<String, Option<f64>> =
                    std::collections::HashMap::new();
                let mut buf = String::new();
                let mut blank = false;
                for r in r0..=r1 {
                    let cell = sheet.cell(r, col, &strings);
                    if matches!(cell.value, duck_core::Value::Empty) {
                        blank = true;
                        continue;
                    }
                    crate::look::show(&looks, looks.get(cell.style), cell.value, d1904, &mut buf);
                    if buf.is_empty() {
                        blank = true;
                        continue;
                    }
                    if !seen.contains_key(buf.as_str()) {
                        let num = match cell.value {
                            duck_core::Value::Number(x) => Some(x),
                            _ => None,
                        };
                        seen.insert(buf.clone(), num);
                    }
                }
                let wanted: Option<(Vec<String>, bool)> = t.filter.as_ref().and_then(|f| {
                    f.columns.iter().find(|c| c.col == k).and_then(|c| match &c.kind {
                        duck_core::FilterKind::Values { values, blank } => {
                            Some((values.iter().map(|v| v.to_lowercase()).collect(), *blank))
                        }
                        _ => None,
                    })
                });
                let mut items: Vec<(String, Option<f64>)> = seen.into_iter().collect();
                items.sort_by(|a, b| match (a.1, b.1) {
                    (Some(x), Some(y)) => x.total_cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    _ => duck_calc::value::text_cmp(&a.0, &b.0),
                });
                let mut out: Vec<(String, bool, bool)> = items
                    .into_iter()
                    .map(|(text, _)| {
                        let sel = wanted.as_ref().is_none_or(|(w, _)| w.contains(&text.to_lowercase()));
                        (text, sel, true)
                    })
                    .collect();
                if blank {
                    let sel = wanted.as_ref().is_none_or(|w| w.1);
                    out.push(("(vazio)".into(), sel, true));
                }
                Some((si, oi, out))
            })
            .collect();
        drop(sheets);
        for (si, oi, items) in results {
            let Some(sheet) = doc.sheets.get_mut(si).and_then(crate::edit::sheet_mut) else { continue };
            if let Some(ObjectKind::Slicer(s)) = sheet.layout.objects.get_mut(oi).map(|o| &mut o.kind) {
                s.items = items;
            }
        }
    }
}

impl crate::app::SheetsApp {
    /// Relê das células os valores dos gráficos cujas séries apontam para
    /// intervalos do arquivo (`'Aba'!$B$2:$B$13`): depois de editar e de
    /// recalcular, os gráficos mostram os valores novos.
    pub fn refresh_charts(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        if !doc.sheets.iter().any(|s| {
            s.data
                .as_ref()
                .is_some_and(|d| d.layout.objects.iter().any(|o| matches!(o.kind, ObjectKind::Chart(_))))
        }) {
            return;
        }
        let sheets: Vec<std::sync::Arc<duck_core::Sheet>> =
            doc.sheets.iter().filter_map(|s| s.data.clone()).collect();
        if sheets.len() != doc.sheets.len() {
            return;
        }
        let strings = doc.strings.clone();
        let extras = doc.calc.extras.clone();
        // (aba, objeto, plot, série, mudança)
        let mut updates: Vec<(usize, usize, usize, usize, duck_calc::chartdata::Update)> = Vec::new();
        {
            let refs: Vec<&duck_core::Sheet> = sheets.iter().map(|s| &**s).collect();
            let book = duck_calc::Book::new(refs, &strings, &extras, doc.date1904);
            for (si, sheet) in sheets.iter().enumerate() {
                for (oi, o) in sheet.layout.objects.iter().enumerate() {
                    let ObjectKind::Chart(c) = &o.kind else { continue };
                    for (pi, pl) in c.plots.iter().enumerate() {
                        for (ki, s) in pl.series.iter().enumerate() {
                            if let Some(u) = duck_calc::chartdata::update(&book, s, si as u32, c.plot_hidden)
                            {
                                updates.push((si, oi, pi, ki, u));
                            }
                        }
                    }
                }
            }
        }
        drop(sheets);
        for (si, oi, pi, ki, u) in updates {
            let Some(sheet) = doc.sheets.get_mut(si).and_then(crate::edit::sheet_mut) else { continue };
            if let Some(ObjectKind::Chart(c)) = sheet.layout.objects.get_mut(oi).map(|o| &mut o.kind)
                && let Some(s) = c.plots.get_mut(pi).and_then(|p| p.series.get_mut(ki))
            {
                s.hidden = u.hidden;
                if let Some(v) = u.vals {
                    s.vals = v;
                }
                if let Some(cs) = u.cats {
                    s.cats = cs;
                }
                if let Some(n) = u.name {
                    s.name = n;
                }
            }
        }
    }
}

impl crate::app::SheetsApp {
    /// Segmentação sob o mouse: (objeto, item; `None` = limpar o filtro).
    pub fn slicer_hit(&mut self, g: &crate::app::Geom, x: f32, y: f32) -> Option<(usize, Option<usize>)> {
        let doc = self.doc.as_ref()?;
        let (sheet, v) = doc.current()?;
        if sheet.layout.objects.is_empty() {
            return None;
        }
        let font = self.font_styles.first().copied()?;
        let text = self.text.as_mut()?;
        let lh = text.line_height_of(font.with_px((8.0 * 96.0 / 72.0 * g.g).round().max(5.0)));
        for q in g.quads.iter() {
            if q.rect.is_empty() || !q.rect.contains(x, y) {
                continue;
            }
            // De cima para baixo: o último desenhado é o de cima.
            for (oi, o) in sheet.layout.objects.iter().enumerate().rev() {
                if !matches!(o.kind, ObjectKind::Slicer(_) | ObjectKind::Timeline(_)) {
                    continue;
                }
                let [ox, oy, w, h] = object_rect(o, v);
                let r = Rect::new(
                    q.rect.x0 + ((ox - q.ox) * g.g as f64) as f32,
                    q.rect.y0 + ((oy - q.oy) * g.g as f64) as f32,
                    q.rect.x0 + ((ox + w - q.ox) * g.g as f64) as f32,
                    q.rect.y0 + ((oy + h - q.oy) * g.g as f64) as f32,
                );
                if !r.contains(x, y) {
                    continue;
                }
                let s = match &o.kind {
                    ObjectKind::Slicer(s) => s,
                    ObjectKind::Timeline(t) => {
                        let periods = timeline_periods(t);
                        let geo = timeline_geom(t, &periods, r, g.g, lh);
                        if t.show_header && geo.clear.inset(-2.0 * g.g).contains(x, y) {
                            return Some((oi, None));
                        }
                        let hit =
                            geo.bars.iter().find(|b| b.1.x0 <= x && x < b.1.x1 + g.g && y >= geo.label_y);
                        return hit.map(|b| (oi, Some(b.0)));
                    }
                    _ => continue,
                };
                let geo = slicer_geom(s, r, g.g, lh);
                if geo.clear.inset(-2.0 * g.g).contains(x, y) {
                    return Some((oi, None));
                }
                return Some((oi, geo.items.iter().position(|b| b.contains(x, y)).map(Some).unwrap_or(None)))
                    .filter(|h| h.1.is_some());
            }
        }
        None
    }

    /// Clique numa segmentação de tabela do Excel: filtra a coluna da tabela
    /// (Ctrl soma ou tira o item; o ícone limpa). As de tabela dinâmica
    /// dependem do motor de tabelas dinâmicas.
    pub fn slicer_click(&mut self, obj: usize, item: Option<usize>, ctrl: bool) {
        use duck_core::{AutoFilter, CellRange, FilterColumn, FilterKind};
        let Some(doc) = self.doc.as_ref() else { return };
        let Some((sheet, _)) = doc.current() else { return };
        // Linha do tempo: Ctrl/Shift estende a seleção.
        if matches!(sheet.layout.objects.get(obj).map(|o| &o.kind), Some(ObjectKind::Timeline(_))) {
            self.timeline_click(obj, item, ctrl);
            return;
        }
        let Some(ObjectKind::Slicer(s)) = sheet.layout.objects.get(obj).map(|o| &o.kind) else { return };
        if s.pivot.is_some() {
            self.pivot_slicer_click(obj, item, ctrl);
            return;
        }
        let Some((tid, cid)) = s.table.clone() else { return };
        let pat = format!(" id=\"{cid}\"");
        let Some((ti, t)) = sheet.layout.tables.iter().enumerate().find(|(_, t)| t.id.to_string() == tid)
        else {
            self.message = "A tabela desta segmentação está em outra aba".into();
            return;
        };
        let Some(k) = t.column_xml.iter().position(|x| x.contains(&pat)) else { return };
        let k = k as u32;
        let all = s.items.iter().all(|i| i.1);
        let mut chosen: Vec<bool> = s.items.iter().map(|i| i.1 && !all).collect();
        match item {
            None => chosen.iter_mut().for_each(|c| *c = false),
            Some(i) if ctrl => {
                if all {
                    // Sem filtro, Ctrl+clique tira o item.
                    chosen = s.items.iter().map(|_| true).collect();
                }
                if let Some(c) = chosen.get_mut(i) {
                    *c = !*c;
                }
            }
            Some(i) => {
                chosen = (0..s.items.len()).map(|j| j == i).collect();
            }
        }
        let none_or_all = chosen.iter().all(|c| *c) || chosen.iter().all(|c| !*c);
        let mut af = t.filter.clone().unwrap_or_else(|| AutoFilter {
            range: CellRange { last_row: t.range.last_row.saturating_sub(t.totals_rows), ..t.range },
            columns: Vec::new(),
        });
        af.columns.retain(|c| c.col != k);
        if !none_or_all {
            let mut values = Vec::new();
            let mut blank = false;
            for (it, on) in s.items.iter().zip(&chosen) {
                if !*on {
                    continue;
                }
                if it.0 == "(vazio)" {
                    blank = true;
                } else {
                    values.push(it.0.clone());
                }
            }
            af.columns.push(FilterColumn { col: k, kind: FilterKind::Values { values, blank } });
            af.columns.sort_by_key(|c| c.col);
        }
        self.set_filter(crate::filter::Target::Table(ti as u32), Some(af), None);
    }
}

// ---------------------------------------------------------------------------
// Linhas do tempo
// ---------------------------------------------------------------------------

/// Um período da linha do tempo: [início, fim) em seriais, o rótulo e o
/// grupo de cima (o ano; nos dias, o mês e o ano).
pub struct Period {
    pub start: f64,
    pub end: f64,
    pub label: String,
    pub group: String,
}

const MONTHS: [&str; 12] =
    ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"];

/// Os períodos do nível da linha do tempo, dentro do período dos dados.
pub fn timeline_periods(t: &Timeline) -> Vec<Period> {
    use duck_core::input::serial;
    let (a, b) = t.bounds;
    let Some(d0) = duck_core::numfmt::serial_to_datetime(a.max(1.0), false, 0) else { return Vec::new() };
    let mut out = Vec::new();
    let (mut y, mut m, mut d) = (d0.year, d0.month, d0.day);
    while out.len() < 5000 {
        let (start, end, label, group) = match t.level {
            0 => (serial(y, 1, 1), serial(y + 1, 1, 1), y.to_string(), String::new()),
            1 => {
                let q = (m - 1) / 3;
                let s = serial(y, q * 3 + 1, 1);
                let e = if q == 3 { serial(y + 1, 1, 1) } else { serial(y, q * 3 + 4, 1) };
                (s, e, format!("T{}", q + 1), y.to_string())
            }
            3 => {
                let s = serial(y, m, d);
                (s, s + 1.0, d.to_string(), format!("{} {y}", MONTHS[(m - 1) as usize]))
            }
            _ => {
                let s = serial(y, m, 1);
                let e = if m == 12 { serial(y + 1, 1, 1) } else { serial(y, m + 1, 1) };
                (s, e, MONTHS[(m - 1) as usize].to_uppercase(), y.to_string())
            }
        };
        if start >= b && !out.is_empty() {
            break;
        }
        out.push(Period { start, end, label, group });
        let Some(n) = duck_core::numfmt::serial_to_datetime(end, false, 0) else { break };
        (y, m, d) = (n.year, n.month, n.day);
    }
    out
}

/// Texto da seleção ("mai 2025 - nov 2026").
fn timeline_label(t: &Timeline) -> String {
    let Some((a, b)) = t.selection else { return "Todos os Períodos".into() };
    let part = |s: f64| -> String {
        let Some(d) = duck_core::numfmt::serial_to_datetime(s, false, 0) else { return String::new() };
        match t.level {
            0 => d.year.to_string(),
            1 => format!("T{} {}", (d.month - 1) / 3 + 1, d.year),
            3 => format!("{:02}/{:02}/{}", d.day, d.month, d.year),
            _ => format!("{} {}", MONTHS[(d.month - 1) as usize], d.year),
        }
    };
    let (x, y) = (part(a), part(b));
    if x == y { x } else { format!("{x} - {y}") }
}

/// Onde ficam as partes de uma linha do tempo na tela.
pub struct TimelineGeom {
    pub caption: Option<(f32, f32)>,
    pub clear: Rect,
    /// Linha do texto da seleção e do nível.
    pub info_y: Option<f32>,
    pub group_y: f32,
    pub label_y: f32,
    /// Barra de cada período visível: (índice em `timeline_periods`, retângulo).
    pub bars: Vec<(usize, Rect)>,
}

pub fn timeline_geom(t: &Timeline, periods: &[Period], r: Rect, g: f32, lh: f32) -> TimelineGeom {
    let pad = 6.0 * g;
    let mut y = r.y0 + pad;
    let x = r.x1 - pad - 14.0 * g;
    let clear = Rect::new(x, y + lh / 2.0 - 6.0 * g, x + 12.0 * g, y + lh / 2.0 + 6.0 * g);
    let caption = if t.show_header {
        let c = (r.x0 + pad, y);
        y += lh + 4.0 * g;
        Some(c)
    } else {
        None
    };
    let info_y = if t.show_selection_label || t.show_time_level {
        let v = y;
        y += lh + 6.0 * g;
        Some(v)
    } else {
        None
    };
    let group_y = y;
    y += lh + 2.0 * g;
    let label_y = y;
    y += lh + 3.0 * g;
    let bh = (12.0 * g).max(4.0);
    let pw = [64.0, 52.0, 40.0, 24.0][t.level.min(3) as usize] * g;
    let first = periods.iter().position(|p| p.end > t.scroll).unwrap_or(0);
    let mut bars = Vec::new();
    let mut bx = r.x0 + pad;
    for (i, _) in periods.iter().enumerate().skip(first) {
        if bx + pw > r.x1 - pad + 0.5 || y + bh > r.y1 - 2.0 {
            break;
        }
        bars.push((i, Rect::new(bx, y, bx + pw - 1.0 * g, y + bh)));
        bx += pw;
    }
    TimelineGeom { caption, clear, info_y, group_y, label_y, bars }
}

fn draw_timeline(p: &mut Painter, cx: &ObjCtx, t: &Timeline, r: Rect, clip: &Rect) {
    let g = cx.g;
    p.fill(r, clip, rgb(0xFFFFFF));
    p.stroke(r, clip, g.max(1.0), rgb(0xBFBFBF));
    let font = cx.font.with_px((8.0 * 96.0 / 72.0 * g).round().max(5.0));
    let lh = p.text.line_height_of(font);
    let periods = timeline_periods(t);
    let geo = timeline_geom(t, &periods, r, g, lh);
    let ink = rgb(0x262626);
    let soft = rgb(0x7F7F7F);
    if let Some((x, y)) = geo.caption {
        let mut cap = font;
        cap.bold = true;
        let cclip = clip.intersect(&Rect::new(r.x0, r.y0, geo.clear.x0 - 4.0 * g, r.y1));
        p.text(&t.caption, cap, x, y, &cclip, ink);
        let c = if t.selection.is_some() { rgb(0xC00000) } else { rgb(0xBFBFBF) };
        p.stroke(geo.clear, clip, g.max(1.0), c);
        let (a, b) = (geo.clear.inset(3.0 * g), g.max(1.0));
        p.line([a.x0, a.y0], [a.x1, a.y1], b, clip, c);
        p.line([a.x1, a.y0], [a.x0, a.y1], b, clip, c);
    }
    let pad = 6.0 * g;
    if let Some(y) = geo.info_y {
        if t.show_selection_label {
            p.text(&timeline_label(t), font, r.x0 + pad, y, clip, ink);
        }
        if t.show_time_level {
            let lv = ["ANOS", "TRIMESTRES", "MESES", "DIAS"][t.level.min(3) as usize];
            p.text_aligned(lv, font, r.x0 + pad, r.x1 - pad, y, Align::Right, clip, soft);
        }
    }
    // Cor da seleção: o primeiro tom de destaque do tema.
    let accent = cx.styles.theme.color(4);
    let on = rgba(map_rgb(accent, |c| c + (255.0 - c) * 0.25));
    let off = rgb(0xE7E6E6);
    let mut last_group = None;
    for &(i, b) in &geo.bars {
        let pd = &periods[i];
        if last_group.as_deref() != Some(pd.group.as_str()) {
            p.text(&pd.group, font, b.x0, geo.group_y, clip, ink);
            last_group = Some(pd.group.clone());
        }
        let lclip = clip.intersect(&Rect::new(b.x0, geo.label_y, b.x1, geo.label_y + lh));
        p.text_aligned(&pd.label, font, b.x0, b.x1, geo.label_y, Align::Center, &lclip, soft);
        let sel = t.selection.is_none_or(|(a, z)| pd.start <= z && pd.end - 1.0 >= a);
        p.fill(b, clip, if sel { on } else { off });
    }
}
