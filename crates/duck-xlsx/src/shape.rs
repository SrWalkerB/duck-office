//! Formas do DrawingML (`xdr:sp`, `xdr:cxnSp`, bordas de `xdr:pic`): a
//! geometria, o preenchimento, a linha e o texto com a formatação.

use duck_core::drawing::ChartColor;
use duck_core::shape::{Fill, Geom, Line, Paint, Para, Run, Shadow, Shape, TextAlign, TextAnchor, TextBody};

use crate::drawing::{child, children, color, num_attr};
use crate::xml::{attr, attr_str, find_tag, inner, unescape_into};

const EMU_PER_PX: f64 = 9525.0;

/// Cor com a opacidade (`a:alpha`).
fn paint(xml: &[u8]) -> Option<Paint> {
    let alpha =
        find_tag(xml, 0, b"a:alpha").and_then(|t| num_attr(t.attrs, b"val")).map_or(1.0, |v| v / 100_000.0);
    Some(Paint { color: color(xml)?, alpha: alpha as f32 })
}

/// Preenchimento de um `spPr`; `None` = não diz (vale o do estilo).
/// `rels`: `r:embed` → parte da imagem.
fn fill(sp_pr: &[u8], rels: &[(String, String, String)]) -> Option<Fill> {
    for el in children(sp_pr) {
        match el.name {
            b"a:noFill" => return Some(Fill::None),
            b"a:solidFill" => return Some(paint(el.body).map_or(Fill::None, Fill::Solid)),
            b"a:pattFill" => {
                let fg = child(el.body, b"a:fgClr").and_then(|c| paint(c.body));
                return Some(fg.map_or(Fill::None, Fill::Solid));
            }
            b"a:gradFill" => {
                let mut stops = Vec::new();
                if let Some(l) = child(el.body, b"a:gsLst") {
                    for g in children(l.body) {
                        let pos = num_attr(g.attrs, b"pos").unwrap_or(0.0) / 100_000.0;
                        if let Some(p) = paint(g.body) {
                            stops.push((pos as f32, p));
                        }
                    }
                }
                stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                let ang = child(el.body, b"a:lin").and_then(|l| num_attr(l.attrs, b"ang")).unwrap_or(0.0);
                return Some(match stops.len() {
                    0 => Fill::None,
                    1 => Fill::Solid(stops[0].1),
                    _ => Fill::Gradient(stops, (ang / 60_000.0) as f32),
                });
            }
            b"a:blipFill" => {
                let rid = find_tag(el.body, 0, b"a:blip").and_then(|t| attr_str(t.attrs, b"r:embed"))?;
                let part = rels.iter().find(|r| r.0 == rid).map(|r| r.2.clone());
                return Some(part.map_or(Fill::None, Fill::Picture));
            }
            _ => {}
        }
    }
    None
}

/// Linha de um `spPr`: `None` = não diz; `Some(None)` = sem linha.
fn line(sp_pr: &[u8], style: Option<Line>) -> Option<Option<Line>> {
    let ln = child(sp_pr, b"a:ln")?;
    let mut paint_ = style.map(|l| l.paint);
    let mut dash = false;
    let (mut head, mut tail) = (false, false);
    for el in children(ln.body) {
        match el.name {
            b"a:noFill" => return Some(None),
            b"a:solidFill" => paint_ = paint(el.body),
            b"a:gradFill" => {
                paint_ = find_tag(el.body, 0, b"a:gs").and_then(|t| paint(inner(el.body, &t, b"a:gs")))
            }
            b"a:prstDash" => dash = attr(el.attrs, b"val").is_some_and(|v| v != b"solid"),
            b"a:headEnd" => head = attr(el.attrs, b"type").is_some_and(|v| v != b"none"),
            b"a:tailEnd" => tail = attr(el.attrs, b"type").is_some_and(|v| v != b"none"),
            _ => {}
        }
    }
    let width = num_attr(ln.attrs, b"w").map_or(style.map_or(0.75, |l| l.width), |w| (w / 12_700.0) as f32);
    Some(paint_.map(|paint| Line { paint, width, dash, head, tail }))
}

fn geom(sp_pr: &[u8]) -> Geom {
    if let Some(p) = child(sp_pr, b"a:prstGeom") {
        let name = attr_str(p.attrs, b"prst").unwrap_or_else(|| "rect".into());
        let mut adj = Vec::new();
        if let Some(av) = child(p.body, b"a:avLst") {
            for gd in children(av.body) {
                let val = attr_str(gd.attrs, b"fmla")
                    .and_then(|f| f.strip_prefix("val ").and_then(|v| v.trim().parse::<f64>().ok()));
                if let (Some(n), Some(v)) = (attr_str(gd.attrs, b"name"), val) {
                    adj.push((n, v));
                }
            }
        }
        return Geom::Preset(name, adj);
    }
    if let Some(c) = child(sp_pr, b"a:custGeom") {
        let paths = custom(c.body);
        if !paths.is_empty() {
            return Geom::Custom(paths);
        }
    }
    Geom::default()
}

/// Contornos de um `a:custGeom`, em frações do retângulo. Pontos com
/// fórmulas (guias) ficam de fora: só coordenadas numéricas.
fn custom(xml: &[u8]) -> Vec<Vec<[f32; 2]>> {
    let mut out = Vec::new();
    let Some(list) = child(xml, b"a:pathLst") else { return out };
    for path in children(list.body) {
        let (w, h) = (
            num_attr(path.attrs, b"w").unwrap_or(0.0).max(1.0),
            num_attr(path.attrs, b"h").unwrap_or(0.0).max(1.0),
        );
        let pts = |xml: &[u8]| -> Vec<[f64; 2]> {
            children(xml)
                .iter()
                .filter(|e| e.name == b"a:pt")
                .filter_map(|e| Some([num_attr(e.attrs, b"x")? / w, num_attr(e.attrs, b"y")? / h]))
                .collect()
        };
        let mut cur: Vec<[f32; 2]> = Vec::new();
        let f = |p: [f64; 2]| [p[0] as f32, p[1] as f32];
        let last = |c: &Vec<[f32; 2]>| c.last().map_or([0.0, 0.0], |p| [p[0] as f64, p[1] as f64]);
        for el in children(path.body) {
            let p = pts(el.body);
            match el.name {
                b"a:moveTo" => {
                    if cur.len() > 1 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    cur.extend(p.first().map(|&p| f(p)));
                }
                b"a:lnTo" => cur.extend(p.first().map(|&p| f(p))),
                b"a:cubicBezTo" if p.len() == 3 => {
                    let a = last(&cur);
                    for i in 1..=16 {
                        let t = i as f64 / 16.0;
                        let u = 1.0 - t;
                        let c = |k: usize| {
                            u * u * u * a[k]
                                + 3.0 * u * u * t * p[0][k]
                                + 3.0 * u * t * t * p[1][k]
                                + t * t * t * p[2][k]
                        };
                        cur.push(f([c(0), c(1)]));
                    }
                }
                b"a:quadBezTo" if p.len() == 2 => {
                    let a = last(&cur);
                    for i in 1..=16 {
                        let t = i as f64 / 16.0;
                        let u = 1.0 - t;
                        let c = |k: usize| u * u * a[k] + 2.0 * u * t * p[0][k] + t * t * p[1][k];
                        cur.push(f([c(0), c(1)]));
                    }
                }
                b"a:arcTo" => {
                    let n = |k: &[u8]| num_attr(el.attrs, k).unwrap_or(0.0);
                    let (rx, ry) = (n(b"wR") / w, n(b"hR") / h);
                    let (st, sw) =
                        ((n(b"stAng") / 60_000.0).to_radians(), (n(b"swAng") / 60_000.0).to_radians());
                    let a = last(&cur);
                    let (cx, cy) = (a[0] - rx * st.cos(), a[1] - ry * st.sin());
                    for i in 1..=24 {
                        let t = st + sw * i as f64 / 24.0;
                        cur.push(f([cx + rx * t.cos(), cy + ry * t.sin()]));
                    }
                }
                _ => {}
            }
        }
        if cur.len() > 1 {
            // Fechado repetindo o primeiro ponto: tira a repetição.
            if cur.len() > 2 && cur.first() == cur.last() {
                cur.pop();
            }
            out.push(cur);
        }
    }
    out
}

fn shadow(sp_pr: &[u8]) -> Option<Shadow> {
    let eff = child(sp_pr, b"a:effectLst")?;
    let s = child(eff.body, b"a:outerShdw")?;
    let n = |k: &[u8]| num_attr(s.attrs, k).unwrap_or(0.0);
    let (dist, dir) = (n(b"dist") / EMU_PER_PX, (n(b"dir") / 60_000.0).to_radians());
    Some(Shadow {
        dx: (dist * dir.cos()) as f32,
        dy: (dist * dir.sin()) as f32,
        blur: (n(b"blurRad") / EMU_PER_PX) as f32,
        paint: paint(s.body)?,
    })
}

fn run_props(xml: &[u8], attrs: &[u8], run: &mut Run) {
    run.size = num_attr(attrs, b"sz").map(|v| (v / 100.0) as f32);
    run.bold = attr(attrs, b"b").is_some_and(|v| v == b"1" || v == b"true");
    run.italic = attr(attrs, b"i").is_some_and(|v| v == b"1" || v == b"true");
    run.underline = attr(attrs, b"u").is_some_and(|v| v != b"none");
    run.strike = attr(attrs, b"strike").is_some_and(|v| v != b"noStrike");
    for el in children(xml) {
        match el.name {
            b"a:solidFill" => run.color = color(el.body),
            b"a:gradFill" => {
                run.color = find_tag(el.body, 0, b"a:gs").and_then(|t| color(inner(el.body, &t, b"a:gs")))
            }
            b"a:latin" => {
                // `+mn-lt` / `+mj-lt`: fontes do tema.
                run.font = attr_str(el.attrs, b"typeface").filter(|f| !f.is_empty());
            }
            _ => {}
        }
    }
}

/// `xdr:txBody`.
pub(crate) fn text_body(xml: &[u8]) -> TextBody {
    let mut t = TextBody::default();
    for el in children(xml) {
        match el.name {
            b"a:bodyPr" => {
                let px = |k: &[u8], d: f32| num_attr(el.attrs, k).map_or(d, |v| (v / EMU_PER_PX) as f32);
                t.insets = [px(b"lIns", 9.6), px(b"tIns", 4.8), px(b"rIns", 9.6), px(b"bIns", 4.8)];
                t.wrap = attr(el.attrs, b"wrap") != Some(b"none");
                t.anchor = match attr(el.attrs, b"anchor") {
                    Some(b"ctr") => TextAnchor::Middle,
                    Some(b"b") => TextAnchor::Bottom,
                    _ => TextAnchor::Top,
                };
                if let Some(fit) = child(el.body, b"a:normAutofit") {
                    t.scale = num_attr(fit.attrs, b"fontScale").map_or(1.0, |v| (v / 100_000.0) as f32);
                }
            }
            b"a:p" => {
                let mut p = Para::default();
                let mut base = Run::default();
                for r in children(el.body) {
                    match r.name {
                        b"a:pPr" => {
                            p.align = match attr(r.attrs, b"algn") {
                                Some(b"ctr") => TextAlign::Center,
                                Some(b"r") => TextAlign::Right,
                                _ => TextAlign::Left,
                            };
                            if let Some(d) = child(r.body, b"a:defRPr") {
                                run_props(d.body, d.attrs, &mut base);
                            }
                        }
                        b"a:r" | b"a:fld" => {
                            let mut run = base.clone();
                            if let Some(pr) = child(r.body, b"a:rPr") {
                                run_props(pr.body, pr.attrs, &mut run);
                            }
                            if let Some(tx) = child(r.body, b"a:t") {
                                unescape_into(tx.body, &mut run.text);
                            }
                            p.runs.push(run);
                        }
                        b"a:br" => p.runs.push(Run { text: "\n".into(), ..base.clone() }),
                        b"a:endParaRPr" => p.end_size = num_attr(r.attrs, b"sz").map(|v| (v / 100.0) as f32),
                        _ => {}
                    }
                }
                t.paras.push(p);
            }
            _ => {}
        }
    }
    t
}

/// Cor de um `a:lnRef`/`a:fillRef`/`a:fontRef` com `idx` diferente de zero.
fn style_ref(style: &[u8], name: &[u8]) -> Option<(u32, ChartColor)> {
    let r = child(style, name)?;
    let idx = match attr(r.attrs, b"idx")? {
        b"none" => 0,
        b"minor" | b"major" => 1,
        v => std::str::from_utf8(v).ok()?.parse().ok()?,
    };
    (idx > 0).then_some(())?;
    Some((idx, color(r.body)?))
}

/// Uma forma ou conector (`xml` = conteúdo do `xdr:sp`/`xdr:cxnSp`).
pub(crate) fn parse(xml: &[u8], rels: &[(String, String, String)]) -> Shape {
    let mut s = Shape::default();
    let pr = child(xml, b"xdr:spPr");
    let pr_body: &[u8] = pr.as_ref().map_or(&[], |p| p.body);
    let style = child(xml, b"xdr:style");
    let style_body: &[u8] = style.as_ref().map_or(&[], |p| p.body);
    s.geom = geom(pr_body);
    if let Some(x) = child(pr_body, b"a:xfrm") {
        s.rot = num_attr(x.attrs, b"rot").map_or(0.0, |v| (v / 60_000.0) as f32);
        s.flip_h = attr(x.attrs, b"flipH") == Some(b"1");
        s.flip_v = attr(x.attrs, b"flipV") == Some(b"1");
    }
    s.fill = fill(pr_body, rels).unwrap_or_else(|| {
        style_ref(style_body, b"a:fillRef")
            .map_or(Fill::None, |(_, color)| Fill::Solid(Paint { color, alpha: 1.0 }))
    });
    let style_line = style_ref(style_body, b"a:lnRef").map(|(idx, color)| Line {
        paint: Paint { color, alpha: 1.0 },
        width: [0.5, 1.0, 1.5][(idx as usize).clamp(1, 3) - 1],
        dash: false,
        head: false,
        tail: false,
    });
    s.line = line(pr_body, style_line).unwrap_or(style_line);
    s.shadow = shadow(pr_body);
    if let Some(t) = child(xml, b"xdr:txBody") {
        s.text = text_body(t.body);
    }
    s.text.color = style_ref(style_body, b"a:fontRef").map(|r| r.1);
    s
}

/// Recorte de uma imagem (`a:srcRect`): frações tiradas de cada lado
/// (esquerda, cima, direita, baixo).
pub(crate) fn crop(blip_fill: &[u8]) -> [f32; 4] {
    let Some(r) = child(blip_fill, b"a:srcRect") else { return [0.0; 4] };
    let n = |k: &[u8]| num_attr(r.attrs, k).map_or(0.0, |v| (v / 100_000.0) as f32);
    [n(b"l"), n(b"t"), n(b"r"), n(b"b")]
}

/// Borda de uma imagem.
pub(crate) fn pic_line(xml: &[u8]) -> Option<Line> {
    let pr = child(xml, b"xdr:spPr")?;
    line(pr.body, None).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &[u8] = br#"<xdr:nvSpPr/><xdr:spPr><a:xfrm flipH="1"><a:off x="0" y="0"/><a:ext cx="9525" cy="9525"/></a:xfrm><a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val 50000"/></a:avLst></a:prstGeom><a:gradFill><a:gsLst><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs><a:gs pos="0"><a:srgbClr val="FF0000"><a:alpha val="50000"/></a:srgbClr></a:gs></a:gsLst><a:lin ang="5400000"/></a:gradFill><a:ln w="25400"><a:prstDash val="dash"/><a:tailEnd type="triangle"/></a:ln><a:effectLst><a:outerShdw blurRad="95250" dist="95250" dir="5400000"><a:srgbClr val="000000"><a:alpha val="40000"/></a:srgbClr></a:outerShdw></a:effectLst></xdr:spPr><xdr:style><a:lnRef idx="2"><a:srgbClr val="00FF00"/></a:lnRef><a:fillRef idx="1"><a:srgbClr val="112233"/></a:fillRef><a:fontRef idx="minor"><a:srgbClr val="ABCDEF"/></a:fontRef></xdr:style><xdr:txBody><a:bodyPr lIns="0" anchor="ctr" wrap="none"><a:normAutofit fontScale="90000"/></a:bodyPr><a:p><a:pPr algn="ctr"/><a:r><a:rPr sz="1800" b="1"><a:solidFill><a:srgbClr val="010203"/></a:solidFill><a:latin typeface="Arial"/></a:rPr><a:t>Ol&amp;</a:t></a:r><a:br/><a:r><a:rPr i="1" u="sng"/><a:t>dois</a:t></a:r></a:p></xdr:txBody>"#;

    #[test]
    fn full_shape() {
        let s = parse(XML, &[]);
        assert_eq!(s.geom, Geom::Preset("roundRect".into(), vec![("adj".into(), 50_000.0)]));
        assert!(s.flip_h && !s.flip_v);
        let Fill::Gradient(stops, ang) = &s.fill else { panic!("{:?}", s.fill) };
        assert_eq!((stops.len(), stops[0].0, stops[0].1.alpha, *ang), (2, 0.0, 0.5, 90.0));
        // A linha não diz a cor: vem do estilo.
        let l = s.line.unwrap();
        assert_eq!((l.paint.color, l.width, l.dash, l.tail), (ChartColor::Rgb(0x00FF00), 2.0, true, true));
        let sh = s.shadow.unwrap();
        assert!((sh.dy - 10.0).abs() < 1e-3 && sh.dx.abs() < 1e-3 && sh.blur == 10.0);
        let t = &s.text;
        assert_eq!((t.anchor, t.wrap, t.scale, t.insets[0]), (TextAnchor::Middle, false, 0.9, 0.0));
        assert_eq!(t.color, Some(ChartColor::Rgb(0xABCDEF)));
        let p = &t.paras[0];
        assert_eq!(p.align, TextAlign::Center);
        assert_eq!(t.plain(), "Ol&\ndois");
        assert_eq!(
            (p.runs[0].size, p.runs[0].bold, p.runs[0].font.as_deref()),
            (Some(18.0), true, Some("Arial"))
        );
        assert!(p.runs[2].italic && p.runs[2].underline && !p.runs[2].bold);
    }

    #[test]
    fn style_fill_and_custom_path() {
        let s = parse(
            br#"<xdr:spPr><a:custGeom><a:pathLst><a:path w="100" h="50"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="100" y="0"/></a:lnTo><a:cubicBezTo><a:pt x="100" y="50"/><a:pt x="50" y="50"/><a:pt x="0" y="50"/></a:cubicBezTo><a:close/></a:path></a:pathLst></a:custGeom><a:ln><a:noFill/></a:ln></xdr:spPr><xdr:style><a:lnRef idx="1"><a:srgbClr val="00FF00"/></a:lnRef><a:fillRef idx="1"><a:srgbClr val="112233"/></a:fillRef></xdr:style>"#,
            &[],
        );
        assert_eq!(s.fill, Fill::Solid(Paint { color: ChartColor::Rgb(0x112233), alpha: 1.0 }));
        assert_eq!(s.line, None);
        let Geom::Custom(p) = &s.geom else { panic!() };
        assert_eq!((p[0].len(), p[0][1], *p[0].last().unwrap()), (18, [1.0, 0.0], [0.0, 1.0]));
        assert_eq!(crop(br#"<a:srcRect l="10000" b="25000"/>"#), [0.1, 0.0, 0.0, 0.25]);
    }
}
