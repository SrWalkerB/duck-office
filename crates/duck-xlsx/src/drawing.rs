//! Desenhos da aba (F5): gráficos (`xl/charts/chartN.xml`), segmentações de
//! dados (`xl/slicers/`, `xl/slicerCaches/`) e formas com texto, lidos de
//! `xl/drawings/drawingN.xml`. Grupos de formas guardam a posição dos filhos
//! num espaço próprio (`chOff`/`chExt`), composto aqui até a âncora.

use std::collections::HashMap;

use duck_core::drawing::{
    Anchor, Chart, ChartAxis, ChartColor, Grouping, LegendPos, Marker, Object, ObjectKind, Plot, PlotKind,
    Series, Slicer,
};

use crate::package::Package;
use crate::parts;
use crate::xml::{attr, attr_str, find_tag, for_each, inner, unescape_into};

// ---------------------------------------------------------------------------
// XML: filhos diretos com aninhamento
// ---------------------------------------------------------------------------

/// Um elemento: nome, atributos e conteúdo (vazio se `<x/>`).
struct El<'a> {
    name: &'a [u8],
    attrs: &'a [u8],
    body: &'a [u8],
}

/// Filhos diretos de um trecho de XML (respeita elementos aninhados com o
/// mesmo nome, como grupos dentro de grupos).
fn children(xml: &[u8]) -> Vec<El<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(lt) = memchr::memchr(b'<', &xml[i..]).map(|k| i + k) {
        let next = xml.get(lt + 1).copied().unwrap_or(0);
        if next == b'/' || next == b'?' || next == b'!' {
            i = crate::xml::tag_end(xml, lt).unwrap_or(xml.len());
            continue;
        }
        let Some(end) = crate::xml::tag_end(xml, lt) else { break };
        let name_end = (lt + 1..end).find(|&k| crate::xml::is_name_end(xml[k])).unwrap_or(end - 1);
        let name = &xml[lt + 1..name_end];
        let self_closing = xml[end - 2] == b'/';
        let attrs = &xml[name_end..if self_closing { end - 2 } else { end - 1 }];
        if self_closing {
            out.push(El { name, attrs, body: &[] });
            i = end;
            continue;
        }
        // Procura o fechamento do mesmo nível.
        let mut depth = 1;
        let mut j = end;
        let mut body_end = xml.len();
        while let Some(k) = memchr::memchr(b'<', &xml[j..]).map(|k| j + k) {
            let Some(e) = crate::xml::tag_end(xml, k) else { break };
            let closing = xml.get(k + 1) == Some(&b'/');
            let nm_start = if closing { k + 2 } else { k + 1 };
            let nm_end = (nm_start..e).find(|&q| crate::xml::is_name_end(xml[q])).unwrap_or(e - 1);
            if &xml[nm_start..nm_end] == name {
                if closing {
                    depth -= 1;
                    if depth == 0 {
                        body_end = k;
                        j = e;
                        break;
                    }
                } else if xml[e - 2] != b'/' {
                    depth += 1;
                }
            }
            j = e;
        }
        out.push(El { name, attrs, body: &xml[end..body_end.max(end)] });
        i = j.max(end);
    }
    out
}

fn child<'a>(xml: &'a [u8], name: &[u8]) -> Option<El<'a>> {
    children(xml).into_iter().find(|e| e.name == name)
}

fn val_i64(xml: &[u8], name: &[u8]) -> i64 {
    find_tag(xml, 0, name)
        .map(|t| inner(xml, &t, name))
        .and_then(|v| std::str::from_utf8(v).ok()?.trim().parse().ok())
        .unwrap_or(0)
}

fn marker(xml: &[u8]) -> Marker {
    Marker {
        col: val_i64(xml, b"xdr:col").max(0) as u32,
        col_off: val_i64(xml, b"xdr:colOff"),
        row: val_i64(xml, b"xdr:row").max(0) as u32,
        row_off: val_i64(xml, b"xdr:rowOff"),
    }
}

fn num_attr(a: &[u8], k: &[u8]) -> Option<f64> {
    attr(a, k).and_then(|v| std::str::from_utf8(v).ok()?.parse().ok())
}

/// `<a:xfrm>`: (x, y, cx, cy) e, para grupos, (chx, chy, chcx, chcy).
fn xfrm(body: &[u8]) -> Option<([f64; 4], Option<[f64; 4]>)> {
    let x = find_tag(body, 0, b"a:xfrm")?;
    let b = inner(body, &x, b"a:xfrm");
    let pt = |name: &[u8], kx: &[u8], ky: &[u8]| {
        find_tag(b, 0, name)
            .map(|t| (num_attr(t.attrs, kx).unwrap_or(0.0), num_attr(t.attrs, ky).unwrap_or(0.0)))
    };
    let (ox, oy) = pt(b"a:off", b"x", b"y")?;
    let (cx, cy) = pt(b"a:ext", b"cx", b"cy")?;
    let ch = match (pt(b"a:chOff", b"x", b"y"), pt(b"a:chExt", b"cx", b"cy")) {
        (Some((a, b2)), Some((c, d))) => Some([a, b2, c, d]),
        _ => None,
    };
    Some(([ox, oy, cx, cy], ch))
}

// ---------------------------------------------------------------------------
// Cores
// ---------------------------------------------------------------------------

/// Ajustes de cor dentro de um elemento de cor (`lumMod`, `lumOff`, `shade`, `tint`).
fn mods(body: &[u8]) -> (f32, f32, f32, f32) {
    let m = |name: &[u8]| {
        find_tag(body, 0, name).and_then(|t| num_attr(t.attrs, b"val")).map(|v| v as f32 / 100_000.0)
    };
    (
        m(b"a:lumMod").unwrap_or(1.0),
        m(b"a:lumOff").unwrap_or(0.0),
        m(b"a:shade").unwrap_or(1.0),
        m(b"a:tint").unwrap_or(1.0),
    )
}

/// Cor de um `<a:solidFill>` (ou do conteúdo dele).
fn color(xml: &[u8]) -> Option<ChartColor> {
    for name in [b"a:srgbClr".as_slice(), b"a:sysClr"] {
        if let Some(t) = find_tag(xml, 0, name) {
            let key: &[u8] = if name == b"a:sysClr" { b"lastClr" } else { b"val" };
            let v = attr(t.attrs, key)?;
            let rgb = u32::from_str_radix(std::str::from_utf8(v).ok()?, 16).ok()?;
            let (lm, lo, sh, ti) = mods(inner(xml, &t, name));
            return Some(ChartColor::Rgb(duck_core::drawing::adjust_color(rgb, lm, lo, sh, ti)));
        }
    }
    if let Some(t) = find_tag(xml, 0, b"a:schemeClr") {
        let v = attr(t.attrs, b"val")?;
        let idx = match v {
            b"bg1" | b"lt1" => 0,
            b"tx1" | b"dk1" => 1,
            b"bg2" | b"lt2" => 2,
            b"tx2" | b"dk2" => 3,
            b"accent1" => 4,
            b"accent2" => 5,
            b"accent3" => 6,
            b"accent4" => 7,
            b"accent5" => 8,
            b"accent6" => 9,
            b"hlink" => 10,
            _ => 1,
        };
        let (lum_mod, lum_off, shade, tint) = mods(inner(xml, &t, b"a:schemeClr"));
        return Some(ChartColor::Scheme { idx, lum_mod, lum_off, shade, tint });
    }
    if let Some(t) = find_tag(xml, 0, b"a:prstClr") {
        return Some(ChartColor::Rgb(match attr(t.attrs, b"val") {
            Some(b"white") => 0xFFFFFF,
            Some(b"red") => 0xFF0000,
            _ => 0,
        }));
    }
    None
}

/// Preenchimento e linha de um `<c:spPr>`: (fill, sem fill, linha, sem linha, largura em pt, tracejada).
struct Sp {
    fill: Option<ChartColor>,
    no_fill: bool,
    line: Option<ChartColor>,
    no_line: bool,
    width: Option<f32>,
    dash: bool,
}

fn sp_pr(xml: &[u8]) -> Sp {
    let mut sp = Sp { fill: None, no_fill: false, line: None, no_line: false, width: None, dash: false };
    let Some(t) = find_tag(xml, 0, b"c:spPr").or_else(|| find_tag(xml, 0, b"xdr:spPr")) else { return sp };
    let name: &[u8] = if xml[t.start..].starts_with(b"<c:spPr") { b"c:spPr" } else { b"xdr:spPr" };
    let body = inner(xml, &t, name);
    for el in children(body) {
        match el.name {
            b"a:solidFill" => sp.fill = color(el.body),
            // Padrão: a cor da frente; gradiente: a primeira parada.
            b"a:pattFill" => sp.fill = child(el.body, b"a:fgClr").and_then(|c| color(c.body)),
            b"a:gradFill" => {
                sp.fill = find_tag(el.body, 0, b"a:gs").and_then(|t| color(inner(el.body, &t, b"a:gs")))
            }
            b"a:noFill" => sp.no_fill = true,
            b"a:ln" => {
                sp.width = num_attr(el.attrs, b"w").map(|w| (w / 12700.0) as f32);
                for l in children(el.body) {
                    match l.name {
                        b"a:solidFill" => sp.line = color(l.body),
                        b"a:noFill" => sp.no_line = true,
                        b"a:prstDash" => sp.dash = attr(l.attrs, b"val").is_some_and(|v| v != b"solid"),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    sp
}

// ---------------------------------------------------------------------------
// Gráficos
// ---------------------------------------------------------------------------

/// Fundo dos rótulos de dados (o `c:spPr` direto de `c:dLbls`, sem o de
/// rótulos individuais `c:dLbl`).
fn label_fill(dlbls: &[u8]) -> Option<ChartColor> {
    let sp = children(dlbls).into_iter().find(|e| e.name == b"c:spPr")?;
    let wrapped = [b"<c:spPr>".as_slice(), sp.body, b"</c:spPr>"].concat();
    sp_pr(&wrapped).fill
}

/// Propriedades do texto (`c:txPr` ou `c:tx/c:rich`): o primeiro `a:defRPr`
/// ou `a:rPr`.
fn text_props(xml: &[u8]) -> Option<duck_core::drawing::TextProps> {
    let (t, name): (_, &[u8]) = match find_tag(xml, 0, b"a:defRPr") {
        Some(t) => (t, b"a:defRPr"),
        None => (find_tag(xml, 0, b"a:rPr")?, b"a:rPr"),
    };
    let body = inner(xml, &t, name);
    Some(duck_core::drawing::TextProps {
        size: num_attr(t.attrs, b"sz").map(|v| (v / 100.0) as f32),
        bold: attr(t.attrs, b"b").map(|v| v == b"1"),
        color: find_tag(body, 0, b"a:solidFill").and_then(|f| color(inner(body, &f, b"a:solidFill"))),
    })
}

fn text_of(xml: &[u8]) -> String {
    let mut s = String::new();
    for_each(xml, b"a:t", |_, body| unescape_into(body, &mut s));
    s
}

/// Valores em cache de `c:strCache` / `c:numCache` (por `idx`).
fn cache_pts(xml: &[u8]) -> (Vec<(u32, String)>, Option<String>, u32) {
    let mut pts = Vec::new();
    let mut fmt = None;
    let mut count = 0;
    if let Some(t) = find_tag(xml, 0, b"c:formatCode") {
        let mut s = String::new();
        unescape_into(inner(xml, &t, b"c:formatCode"), &mut s);
        fmt = Some(s);
    }
    if let Some(t) = find_tag(xml, 0, b"c:ptCount") {
        count = num_attr(t.attrs, b"val").unwrap_or(0.0) as u32;
    }
    for_each(xml, b"c:pt", |a, body| {
        let idx = num_attr(a, b"idx").unwrap_or(0.0) as u32;
        let mut s = String::new();
        if let Some(v) = find_tag(body, 0, b"c:v") {
            unescape_into(inner(body, &v, b"c:v"), &mut s);
        }
        pts.push((idx, s));
    });
    (pts, fmt, count)
}

fn formula_of(xml: &[u8]) -> Option<String> {
    let t = find_tag(xml, 0, b"c:f")?;
    let mut s = String::new();
    unescape_into(inner(xml, &t, b"c:f"), &mut s);
    Some(s)
}

fn series(xml: &[u8]) -> Series {
    let mut s = Series::default();
    for el in children(xml) {
        match el.name {
            b"c:tx" => {
                s.name_ref = formula_of(el.body);
                let (pts, _, _) = cache_pts(el.body);
                s.name = pts.into_iter().next().map(|p| p.1).unwrap_or_else(|| {
                    find_tag(el.body, 0, b"c:v")
                        .map(|t| {
                            let mut x = String::new();
                            unescape_into(inner(el.body, &t, b"c:v"), &mut x);
                            x
                        })
                        .unwrap_or_default()
                });
            }
            b"c:cat" | b"c:xVal" => {
                s.cat_ref = formula_of(el.body);
                let (pts, _, count) = cache_pts(el.body);
                // Categorias de vários níveis: fica o nível mais de baixo.
                let n = count.max(pts.iter().map(|p| p.0 + 1).max().unwrap_or(0));
                let mut cats = vec![String::new(); n as usize];
                for (i, v) in pts {
                    if let Some(c) = cats.get_mut(i as usize)
                        && c.is_empty()
                    {
                        *c = v;
                    }
                }
                s.cats = cats;
            }
            b"c:val" | b"c:yVal" => {
                s.val_ref = formula_of(el.body);
                let (pts, fmt, count) = cache_pts(el.body);
                let n = count.max(pts.iter().map(|p| p.0 + 1).max().unwrap_or(0));
                let mut vals = vec![None; n as usize];
                for (i, v) in pts {
                    if let Some(slot) = vals.get_mut(i as usize) {
                        *slot = v.trim().parse().ok();
                    }
                }
                s.vals = vals;
                s.fmt = fmt;
            }
            b"c:spPr" => {
                let sp = sp_pr(xml);
                s.fill = sp.fill;
                s.no_fill = sp.no_fill;
                s.line = sp.line;
                s.no_line = sp.no_line;
                s.line_width = sp.width;
                s.dash = sp.dash;
            }
            b"c:marker" => {
                s.marker =
                    find_tag(el.body, 0, b"c:symbol").is_none_or(|t| attr(t.attrs, b"val") != Some(b"none"));
            }
            b"c:dLbls" => {
                s.label_text =
                    find_tag(el.body, 0, b"c:txPr").and_then(|t| text_props(inner(el.body, &t, b"c:txPr")));
                s.label_fill = label_fill(el.body);
                s.labels = find_tag(el.body, 0, b"c:showVal").map(|t| attr(t.attrs, b"val") == Some(b"1"));
                if find_tag(el.body, 0, b"c:delete").is_some_and(|t| attr(t.attrs, b"val") == Some(b"1")) {
                    s.labels = Some(false);
                }
            }
            b"c:dPt" => {
                let idx = find_tag(el.body, 0, b"c:idx")
                    .and_then(|t| num_attr(t.attrs, b"val"))
                    .unwrap_or(0.0) as u32;
                if let Some(c) = sp_pr(el.body).fill {
                    s.points.push((idx, c));
                }
            }
            _ => {}
        }
    }
    s
}

fn axis(xml: &[u8]) -> ChartAxis {
    let mut a = ChartAxis::default();
    for el in children(xml) {
        match el.name {
            b"c:delete" => a.deleted |= attr(el.attrs, b"val") == Some(b"1"),
            // Sem rótulos: para o desenho, é como se o eixo não existisse.
            b"c:tickLblPos" => a.deleted |= attr(el.attrs, b"val") == Some(b"none"),
            b"c:majorGridlines" => a.gridlines = true,
            b"c:txPr" => a.text = text_props(el.body),
            b"c:numFmt" => a.fmt = attr_str(el.attrs, b"formatCode"),
            b"c:scaling" => {
                for s in children(el.body) {
                    match s.name {
                        b"c:min" => a.min = num_attr(s.attrs, b"val"),
                        b"c:max" => a.max = num_attr(s.attrs, b"val"),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    a
}

pub fn parse_chart(xml: &[u8]) -> Option<Chart> {
    let cs = find_tag(xml, 0, b"c:chartSpace")?;
    let space = inner(xml, &cs, b"c:chartSpace");
    let chart_el = child(space, b"c:chart")?;
    let mut chart = Chart { pivot: find_tag(space, 0, b"c:pivotSource").is_some(), ..Default::default() };
    // Fundo do quadro.
    let bg = children(space).into_iter().find(|e| e.name == b"c:spPr");
    if let Some(bg) = bg {
        let wrapped = [b"<c:spPr>".as_slice(), bg.body, b"</c:spPr>"].concat();
        let sp = sp_pr(&wrapped);
        chart.background = sp.fill;
        chart.no_background = sp.no_fill;
        chart.border = if sp.no_line { None } else { sp.line };
    }
    let mut auto_deleted = false;
    let mut axes: Vec<(Vec<u8>, ChartAxis, bool)> = Vec::new(); // (id, eixo, é de valores)
    let mut plot_axes: Vec<Vec<Vec<u8>>> = Vec::new();
    for el in children(chart_el.body) {
        match el.name {
            b"c:title" => {
                let t = text_of(el.body);
                let t = if t.is_empty() {
                    let (pts, _, _) = cache_pts(el.body);
                    pts.into_iter().map(|p| p.1).collect::<Vec<_>>().join(" ")
                } else {
                    t
                };
                chart.title = Some(t);
                chart.title_text = text_props(el.body);
            }
            b"c:autoTitleDeleted" => auto_deleted = attr(el.attrs, b"val") == Some(b"1"),
            b"c:legend" => {
                chart.legend_text =
                    find_tag(el.body, 0, b"c:txPr").and_then(|t| text_props(inner(el.body, &t, b"c:txPr")));
                let pos = find_tag(el.body, 0, b"c:legendPos")
                    .and_then(|t| attr(t.attrs, b"val").map(|v| v.to_vec()));
                chart.legend = Some(match pos.as_deref() {
                    Some(b"b") => LegendPos::Bottom,
                    Some(b"t") => LegendPos::Top,
                    Some(b"l") => LegendPos::Left,
                    _ => LegendPos::Right,
                });
            }
            b"c:plotArea" => {
                for p in children(el.body) {
                    let kind = match p.name {
                        b"c:barChart" | b"c:bar3DChart" => {
                            let h = find_tag(p.body, 0, b"c:barDir")
                                .is_some_and(|t| attr(t.attrs, b"val") == Some(b"bar"));
                            Some(if h { PlotKind::BarH } else { PlotKind::Bar })
                        }
                        b"c:lineChart" | b"c:line3DChart" | b"c:stockChart" | b"c:radarChart" => {
                            Some(PlotKind::Line)
                        }
                        b"c:areaChart" | b"c:area3DChart" => Some(PlotKind::Area),
                        b"c:pieChart" | b"c:pie3DChart" | b"c:ofPieChart" => Some(PlotKind::Pie),
                        b"c:doughnutChart" => Some(PlotKind::Doughnut),
                        b"c:scatterChart" | b"c:bubbleChart" => Some(PlotKind::Scatter),
                        b"c:catAx" | b"c:dateAx" | b"c:serAx" | b"c:valAx" => {
                            let id = find_tag(p.body, 0, b"c:axId")
                                .and_then(|t| attr(t.attrs, b"val").map(|v| v.to_vec()))
                                .unwrap_or_default();
                            axes.push((id, axis(p.body), p.name == b"c:valAx"));
                            None
                        }
                        b"c:layout" => {
                            if let Some(ml) = find_tag(p.body, 0, b"c:manualLayout") {
                                let b = inner(p.body, &ml, b"c:manualLayout");
                                let v = |n: &[u8]| find_tag(b, 0, n).and_then(|t| num_attr(t.attrs, b"val"));
                                if let (Some(x), Some(y), Some(w), Some(h)) =
                                    (v(b"c:x"), v(b"c:y"), v(b"c:w"), v(b"c:h"))
                                {
                                    chart.plot_layout = Some([x as f32, y as f32, w as f32, h as f32]);
                                }
                            }
                            None
                        }
                        _ => None,
                    };
                    let Some(kind) = kind else { continue };
                    let mut plot = Plot { kind, gap: 150, ..Default::default() };
                    let mut ids = Vec::new();
                    for c in children(p.body) {
                        match c.name {
                            b"c:grouping" => {
                                plot.grouping = match attr(c.attrs, b"val") {
                                    Some(b"stacked") => Grouping::Stacked,
                                    Some(b"percentStacked") => Grouping::Percent,
                                    _ => Grouping::Clustered,
                                }
                            }
                            b"c:varyColors" => plot.vary_colors = attr(c.attrs, b"val") == Some(b"1"),
                            b"c:gapWidth" => plot.gap = num_attr(c.attrs, b"val").unwrap_or(150.0) as u32,
                            b"c:overlap" => plot.overlap = num_attr(c.attrs, b"val").unwrap_or(0.0) as i32,
                            b"c:holeSize" => plot.hole = num_attr(c.attrs, b"val").unwrap_or(50.0) as u32,
                            b"c:ser" => plot.series.push(series(c.body)),
                            b"c:dLbls" => {
                                plot.labels = find_tag(c.body, 0, b"c:showVal")
                                    .is_some_and(|t| attr(t.attrs, b"val") == Some(b"1"));
                                plot.label_text = find_tag(c.body, 0, b"c:txPr")
                                    .and_then(|t| text_props(inner(c.body, &t, b"c:txPr")));
                                plot.label_fill = label_fill(c.body);
                            }
                            b"c:axId" => ids.push(attr(c.attrs, b"val").unwrap_or_default().to_vec()),
                            _ => {}
                        }
                    }
                    if kind == PlotKind::Doughnut && plot.hole == 0 {
                        plot.hole = 50;
                    }
                    plot_axes.push(ids);
                    chart.plots.push(plot);
                }
            }
            _ => {}
        }
    }
    // Eixos: o de valores do primeiro gráfico é o principal; outro de valores, o secundário.
    let mut primary_val: Option<Vec<u8>> = None;
    for (i, ids) in plot_axes.iter().enumerate() {
        let val = ids.iter().find(|id| axes.iter().any(|a| &a.0 == *id && a.2)).cloned();
        let cat = ids.iter().find(|id| axes.iter().any(|a| &a.0 == *id && !a.2)).cloned();
        match (&primary_val, val) {
            (None, Some(v)) => {
                if let Some(a) = axes.iter().find(|a| a.0 == v) {
                    chart.val_axis = a.1.clone();
                }
                if let Some(c) = cat.and_then(|c| axes.iter().find(|a| a.0 == c)) {
                    chart.cat_axis = c.1.clone();
                }
                primary_val = Some(v);
            }
            (Some(p), Some(v)) if *p != v => {
                chart.plots[i].secondary = true;
                if let Some(a) = axes.iter().find(|a| a.0 == v) {
                    chart.sec_axis = Some(a.1.clone());
                }
            }
            _ => {}
        }
    }
    if chart.title.is_none()
        && !auto_deleted
        && chart.plots.iter().map(|p| p.series.len()).sum::<usize>() == 1
    {
        chart.title = chart
            .plots
            .first()
            .and_then(|p| p.series.first())
            .map(|s| s.name.clone())
            .filter(|n| !n.is_empty());
    }
    Some(chart)
}

// ---------------------------------------------------------------------------
// Segmentações de dados
// ---------------------------------------------------------------------------

/// Caches de segmentação do arquivo (por nome), com os itens.
#[derive(Default)]
pub struct SlicerCaches {
    by_name: HashMap<String, Slicer>,
}

/// Lê os caches de segmentação e os itens (dos caches de tabela dinâmica).
pub fn read_slicer_caches(pkg: &Package, workbook: &str) -> SlicerCaches {
    let mut out = SlicerCaches::default();
    let rels = parts::rel_targets_typed(pkg, workbook);
    // Caches de tabela dinâmica: id do x14 → itens de cada campo.
    let mut pivot_fields: HashMap<String, Vec<(String, Vec<String>)>> = HashMap::new();
    for (_, kind, target) in &rels {
        if !kind.ends_with("/pivotCacheDefinition") {
            continue;
        }
        let Ok(x) = pkg.read(target) else { continue };
        let id = find_tag(&x, 0, b"x14:pivotCacheDefinition")
            .and_then(|t| attr_str(t.attrs, b"pivotCacheId"))
            .unwrap_or_default();
        let mut fields = Vec::new();
        for_each(&x, b"cacheField", |a, body| {
            let name = attr_str(a, b"name").unwrap_or_default();
            let mut items = Vec::new();
            if let Some(si) = find_tag(body, 0, b"sharedItems") {
                for el in children(inner(body, &si, b"sharedItems")) {
                    let v = attr_str(el.attrs, b"v").unwrap_or_default();
                    items.push(match el.name {
                        b"m" => "(vazio)".to_string(),
                        b"d" => {
                            // 2026-01-31T00:00:00 → 31/01/2026
                            let d = v.split('T').next().unwrap_or("");
                            let p: Vec<&str> = d.split('-').collect();
                            if p.len() == 3 { format!("{}/{}/{}", p[2], p[1], p[0]) } else { v }
                        }
                        b"b" => (if v == "1" { "VERDADEIRO" } else { "FALSO" }).to_string(),
                        b"n" => v.replace('.', ","),
                        _ => v,
                    });
                }
            }
            fields.push((name, items));
        });
        if !id.is_empty() {
            pivot_fields.insert(id, fields);
        }
    }
    for (_, kind, target) in &rels {
        if !kind.ends_with("/slicerCache") {
            continue;
        }
        let Ok(x) = pkg.read(target) else { continue };
        let Some(def) = find_tag(&x, 0, b"slicerCacheDefinition") else { continue };
        let name = attr_str(def.attrs, b"name").unwrap_or_default();
        let source = attr_str(def.attrs, b"sourceName").unwrap_or_default();
        let mut s = Slicer { caption: source.clone(), ..Default::default() };
        if let Some(tab) = find_tag(&x, 0, b"tabular") {
            let pid = attr_str(tab.attrs, b"pivotCacheId").unwrap_or_default();
            let names: &[String] = pivot_fields
                .get(&pid)
                .and_then(|f| f.iter().find(|(n, _)| n.trim() == source.trim()).map(|f| f.1.as_slice()))
                .unwrap_or(&[]);
            let mut link = duck_core::drawing::SlicerPivot {
                part: target.clone(),
                source: source.clone(),
                cache_id: pid.clone(),
                ..Default::default()
            };
            for_each(&x, b"pivotTable", |a, _| {
                if let (Some(tab), Some(name)) = (num_attr(a, b"tabId"), attr_str(a, b"name")) {
                    link.tables.push((tab as u32, name));
                }
            });
            for_each(inner(&x, &tab, b"tabular"), b"i", |a, _| {
                let idx = num_attr(a, b"x").unwrap_or(0.0) as usize;
                let sel = attr(a, b"s").is_some_and(|v| v == b"1");
                let nd = attr(a, b"nd").is_some_and(|v| v == b"1");
                let text = names.get(idx).cloned().unwrap_or_default();
                s.items.push((text, sel, !nd));
                link.item_x.push(idx as u32);
            });
            s.pivot = Some(link);
        } else if let Some(t) = find_tag(&x, 0, b"x15:tableSlicerCache") {
            let tid = attr_str(t.attrs, b"tableId").unwrap_or_default();
            let col = attr_str(t.attrs, b"column").unwrap_or_default();
            s.table = Some((tid, col));
        }
        out.by_name.insert(name, s);
    }
    out
}

/// Definições de segmentação de uma aba (`xl/slicers/slicerN.xml`), por nome.
fn read_slicers(pkg: &Package, sheet_part: &str, caches: &SlicerCaches) -> HashMap<String, Slicer> {
    let mut out = HashMap::new();
    for (_, kind, target) in parts::rel_targets_typed(pkg, sheet_part) {
        if !kind.ends_with("/slicer") {
            continue;
        }
        let Ok(x) = pkg.read(&target) else { continue };
        for_each(&x, b"slicer", |a, _| {
            let Some(name) = attr_str(a, b"name") else { return };
            let cache = attr_str(a, b"cache").unwrap_or_default();
            let mut s = caches.by_name.get(&cache).cloned().unwrap_or_default();
            s.name = name.clone();
            if let Some(c) = attr_str(a, b"caption") {
                s.caption = c;
            }
            s.show_caption = attr(a, b"showCaption") != Some(b"0");
            s.columns = num_attr(a, b"columnCount").unwrap_or(1.0).max(1.0) as u32;
            s.row_height = num_attr(a, b"rowHeight").unwrap_or(241_300.0) as i64;
            s.style = attr_str(a, b"style");
            out.insert(name, s);
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Desenho da aba
// ---------------------------------------------------------------------------

/// Retângulo em EMU (x, y, largura, altura).
type R = [f64; 4];

struct Walk<'a> {
    pkg: &'a Package,
    drawing: &'a str,
    rels: &'a [(String, String, String)],
    slicers: &'a HashMap<String, Slicer>,
    anchor: Anchor,
    /// Retângulo do elemento de cima (frações dos filhos são em relação a ele).
    top: Option<R>,
    out: Vec<Object>,
}

impl Walk<'_> {
    fn push(&mut self, rect: Option<R>, kind: ObjectKind) {
        let frac = match (rect, self.top) {
            (Some(r), Some(t)) if t[2] > 0.0 && t[3] > 0.0 => [
                ((r[0] - t[0]) / t[2]) as f32,
                ((r[1] - t[1]) / t[3]) as f32,
                (r[2] / t[2]) as f32,
                (r[3] / t[3]) as f32,
            ],
            _ => [0.0, 0.0, 1.0, 1.0],
        };
        self.out.push(Object { anchor: self.anchor, frac, kind });
    }

    /// Visita os elementos de `xml` com `map` levando coordenadas locais ao
    /// espaço do elemento de cima.
    fn walk(&mut self, xml: &[u8], map: &dyn Fn(R) -> R, depth: u32) {
        for el in children(xml) {
            match el.name {
                b"mc:AlternateContent" => {
                    if let Some(ch) = child(el.body, b"mc:Choice") {
                        self.walk(ch.body, map, depth);
                    }
                }
                b"xdr:grpSp" if depth < 16 => {
                    let Some(pr) = child(el.body, b"xdr:grpSpPr") else { continue };
                    let Some((off, ch)) = xfrm(pr.body) else {
                        self.walk(el.body, map, depth + 1);
                        continue;
                    };
                    let me = map(off);
                    if depth == 0 && self.top.is_none() {
                        self.top = Some(me);
                    }
                    let ch = ch.unwrap_or(off);
                    let inner_map = move |r: R| -> R {
                        let sx = if ch[2] != 0.0 { off[2] / ch[2] } else { 1.0 };
                        let sy = if ch[3] != 0.0 { off[3] / ch[3] } else { 1.0 };
                        map([
                            off[0] + (r[0] - ch[0]) * sx,
                            off[1] + (r[1] - ch[1]) * sy,
                            r[2] * sx,
                            r[3] * sy,
                        ])
                    };
                    self.walk(el.body, &inner_map, depth + 1);
                }
                b"xdr:graphicFrame" => {
                    let rect = child(el.body, b"xdr:xfrm").and_then(|x| {
                        let wrapped = [b"<a:xfrm>".as_slice(), x.body, b"</a:xfrm>"].concat();
                        xfrm(&wrapped).map(|(r, _)| map(r))
                    });
                    let rect = rect.filter(|r| r[2] > 0.0 && r[3] > 0.0);
                    if depth == 0 && self.top.is_none() {
                        self.top = rect;
                    }
                    if let Some(c) = find_tag(el.body, 0, b"c:chart") {
                        let rid = attr_str(c.attrs, b"r:id").unwrap_or_default();
                        if let Some((_, _, target)) = self.rels.iter().find(|r| r.0 == rid)
                            && let Ok(x) = self.pkg.read(target)
                            && let Some(chart) = parse_chart(&x)
                        {
                            self.push(rect, ObjectKind::Chart(Box::new(chart)));
                        }
                    } else if let Some(s) = find_tag(el.body, 0, b"sle:slicer") {
                        let name = attr_str(s.attrs, b"name").unwrap_or_default();
                        if let Some(sl) = self.slicers.get(&name) {
                            self.push(rect, ObjectKind::Slicer(Box::new(sl.clone())));
                        }
                    }
                }
                b"xdr:sp" => {
                    let rect = child(el.body, b"xdr:spPr").and_then(|p| xfrm(p.body)).map(|(r, _)| map(r));
                    if depth == 0 && self.top.is_none() {
                        self.top = rect;
                    }
                    let text = child(el.body, b"xdr:txBody").map(|t| text_of(t.body)).unwrap_or_default();
                    let sp = sp_pr(el.body);
                    if !text.trim().is_empty() || sp.fill.is_some() {
                        self.push(
                            rect,
                            ObjectKind::Shape {
                                text,
                                fill: sp.fill,
                                line: if sp.no_line { None } else { sp.line },
                            },
                        );
                    }
                }
                b"xdr:pic" => {
                    let rect = child(el.body, b"xdr:spPr").and_then(|p| xfrm(p.body)).map(|(r, _)| map(r));
                    if depth == 0 && self.top.is_none() {
                        self.top = rect;
                    }
                    if let Some(b) = find_tag(el.body, 0, b"a:blip") {
                        let rid = attr_str(b.attrs, b"r:embed").unwrap_or_default();
                        if let Some((_, _, target)) = self.rels.iter().find(|r| r.0 == rid) {
                            self.push(rect, ObjectKind::Picture { part: target.clone() });
                        }
                    }
                }
                _ => {}
            }
        }
        let _ = self.drawing;
    }
}

/// Objetos desenhados na aba `sheet_part`.
pub fn read_objects(pkg: &Package, sheet_part: &str, caches: &SlicerCaches) -> Vec<Object> {
    let mut out = Vec::new();
    let slicers = read_slicers(pkg, sheet_part, caches);
    for (_, kind, drawing) in parts::rel_targets_typed(pkg, sheet_part) {
        if !kind.ends_with("/drawing") {
            continue;
        }
        let Ok(xml) = pkg.read(&drawing) else { continue };
        let Some(root) = find_tag(&xml, 0, b"xdr:wsDr") else { continue };
        let body = inner(&xml, &root, b"xdr:wsDr");
        let rels = parts::rel_targets_typed(pkg, &drawing);
        for a in children(body) {
            let anchor = match a.name {
                b"xdr:twoCellAnchor" => {
                    let (Some(f), Some(t)) = (child(a.body, b"xdr:from"), child(a.body, b"xdr:to")) else {
                        continue;
                    };
                    Anchor::Two { from: marker(f.body), to: marker(t.body) }
                }
                b"xdr:oneCellAnchor" => {
                    let Some(f) = child(a.body, b"xdr:from") else { continue };
                    let ext = child(a.body, b"xdr:ext");
                    let n = |k: &[u8]| ext.as_ref().and_then(|e| num_attr(e.attrs, k)).unwrap_or(0.0) as i64;
                    Anchor::One { from: marker(f.body), cx: n(b"cx"), cy: n(b"cy") }
                }
                b"xdr:absoluteAnchor" => {
                    let pos = child(a.body, b"xdr:pos");
                    let ext = child(a.body, b"xdr:ext");
                    let n = |e: &Option<El>, k: &[u8]| {
                        e.as_ref().and_then(|e| num_attr(e.attrs, k)).unwrap_or(0.0) as i64
                    };
                    Anchor::Abs { x: n(&pos, b"x"), y: n(&pos, b"y"), cx: n(&ext, b"cx"), cy: n(&ext, b"cy") }
                }
                _ => continue,
            };
            let mut w = Walk {
                pkg,
                drawing: &drawing,
                rels: &rels,
                slicers: &slicers,
                anchor,
                top: None,
                out: Vec::new(),
            };
            w.walk(a.body, &|r| r, 0);
            out.extend(w.out);
        }
    }
    out
}
