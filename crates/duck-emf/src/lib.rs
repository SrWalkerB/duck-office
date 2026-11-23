//! Rasterizador de metarquivos EMF (F5): as imagens vetoriais que o Office
//! guarda nas planilhas (logotipos, ícones, figuras coladas).
//!
//! Lê os registros GDI do EMF (os metarquivos "duplos" do Office trazem
//! também o EMF+, em comentários, que fica de fora) e desenha numa imagem
//! RGBA na CPU:
//!
//! - caminhos e formas (linhas, polígonos, curvas de Bézier, retângulos,
//!   elipses), preenchidos com regra par-ímpar ou não-zero, contornos com a
//!   largura da caneta; cobertura com 4 sub-linhas por pixel (antisserrilhado);
//! - bitmaps (`StretchDIBits`, `BitBlt`, `StretchBlt`, `AlphaBlend`) de 1, 4,
//!   8, 16, 24 e 32 bits;
//! - transformações (mundo, janela e viewport, modos de mapeamento),
//!   `SaveDC`/`RestoreDC` e recortes retangulares.
//!
//! - texto (`ExtTextOutW`), posicionado caractere a caractere como o GDI,
//!   com os glifos de quem chama ([`Text`]).

use std::collections::HashMap;

/// Fonte de um texto do EMF (já em pixels da imagem).
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub face: String,
    pub px: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Quem desenha os glifos (o app usa o `duck-text`).
pub trait Text {
    /// Pixels cobertos por `text` com o topo-esquerdo da linha em (x, y):
    /// (x, y, cobertura 0..1).
    fn glyphs(&mut self, text: &str, font: &Font, x: f32, y: f32, out: &mut Vec<(i32, i32, f32)>);
    /// Largura do texto.
    fn measure(&mut self, text: &str, font: &Font) -> f32;
    /// Do topo da linha até a linha de base.
    fn ascent(&mut self, font: &Font) -> f32;
}

/// Imagem RGBA (não pré-multiplicada), linhas contíguas.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// É um EMF? (registro de cabeçalho com a assinatura " EMF").
pub fn is_emf(b: &[u8]) -> bool {
    b.len() >= 88 && u32le(b, 0) == 1 && &b[40..44] == b" EMF"
}

/// Desenha o EMF numa imagem com o maior lado até `max_side` pixels (sem texto).
pub fn render(b: &[u8], max_side: u32) -> Option<Image> {
    render_with(b, max_side, None)
}

/// Como [`render`], com o texto desenhado por `text`.
pub fn render_with(b: &[u8], max_side: u32, text: Option<&mut dyn Text>) -> Option<Image> {
    if !is_emf(b) {
        return None;
    }
    let bounds = rect(b, 8);
    let frame = rect(b, 24);
    let dev = (i32le(b, 72) as f64, i32le(b, 76) as f64);
    let mm = (i32le(b, 80) as f64, i32le(b, 84) as f64);
    // Moldura (0,01 mm) em pixels do dispositivo de referência.
    let px_per_mm =
        if mm.0 > 0.0 && mm.1 > 0.0 && dev.0 > 0.0 { (dev.0 / mm.0, dev.1 / mm.1) } else { (3.78, 3.78) };
    let mut area = [
        frame[0] as f64 / 100.0 * px_per_mm.0,
        frame[1] as f64 / 100.0 * px_per_mm.1,
        frame[2] as f64 / 100.0 * px_per_mm.0,
        frame[3] as f64 / 100.0 * px_per_mm.1,
    ];
    if area[2] - area[0] < 1.0 || area[3] - area[1] < 1.0 {
        area = [bounds[0] as f64, bounds[1] as f64, bounds[2] as f64 + 1.0, bounds[3] as f64 + 1.0];
    }
    let (aw, ah) = (area[2] - area[0], area[3] - area[1]);
    if aw < 1.0 || ah < 1.0 {
        return None;
    }
    // Resolução: o dobro da moldura (nítido com zoom), até `max_side`.
    let scale = (2.0f64).min(max_side as f64 / aw.max(ah));
    let (w, h) = ((aw * scale).round().max(1.0) as u32, (ah * scale).round().max(1.0) as u32);
    // Desenha numa escala inteira do dispositivo (como o GDI, em pixels
    // inteiros: faixas vizinhas não deixam frestas) e reduz depois.
    let draw_scale = if scale >= 1.5 { 2.0 } else { 1.0 };
    let (dw, dh) = ((aw * draw_scale).round().max(1.0) as u32, (ah * draw_scale).round().max(1.0) as u32);
    if (dw as u64) * (dh as u64) > 64_000_000 {
        return None;
    }
    let origin = [area[0].round(), area[1].round()];
    let mut p = Player::new(dw, dh, origin, draw_scale, px_per_mm);
    p.text = text;
    let mut i = 0usize;
    while i + 8 <= b.len() {
        let ty = u32le(b, i);
        let size = u32le(b, i + 4) as usize;
        if size < 8 || i + size > b.len() {
            break;
        }
        p.record(ty, &b[i..i + size]);
        if ty == 14 {
            break; // EMR_EOF
        }
        i += size;
    }
    let canvas = if (dw, dh) == (w, h) { p.canvas } else { p.canvas.resample(w, h) };
    Some(Image { w, h, rgba: canvas.into_rgba() })
}

// ---------------------------------------------------------------------------
// Leitura dos bytes
// ---------------------------------------------------------------------------

fn u32le(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn i32le(b: &[u8], i: usize) -> i32 {
    u32le(b, i) as i32
}
fn u16le(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}
fn i16le(b: &[u8], i: usize) -> i16 {
    u16le(b, i) as i16
}
fn f32le(b: &[u8], i: usize) -> f32 {
    f32::from_bits(u32le(b, i))
}
fn rect(b: &[u8], i: usize) -> [i32; 4] {
    [i32le(b, i), i32le(b, i + 4), i32le(b, i + 8), i32le(b, i + 12)]
}
/// COLORREF (0x00BBGGRR) → RGBA.
fn colorref(v: u32) -> [u8; 4] {
    [(v & 0xFF) as u8, (v >> 8 & 0xFF) as u8, (v >> 16 & 0xFF) as u8, 255]
}

// ---------------------------------------------------------------------------
// Tela (CPU)
// ---------------------------------------------------------------------------

struct Canvas {
    w: u32,
    h: u32,
    /// RGBA em ponto flutuante (0..1), cor não pré-multiplicada.
    px: Vec<[f32; 4]>,
}

type Pt = (f64, f64);

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self { w, h, px: vec![[0.0; 4]; (w * h) as usize] }
    }

    fn into_rgba(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in self.px {
            for c in p {
                out.push((c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        out
    }

    /// Mistura `c` (com cobertura `a` 0..1) no pixel.
    fn blend(&mut self, x: u32, y: u32, c: [f32; 4], a: f32) {
        let i = (y * self.w + x) as usize;
        let sa = c[3] * a;
        if sa <= 0.0 {
            return;
        }
        let d = &mut self.px[i];
        let da = d[3];
        let oa = sa + da * (1.0 - sa);
        if oa <= 0.0 {
            return;
        }
        for k in 0..3 {
            d[k] = (c[k] * sa + d[k] * da * (1.0 - sa)) / oa;
        }
        d[3] = oa;
    }

    /// Reduz para (w, h) pela média das áreas (em cor pré-multiplicada).
    fn resample(&self, w: u32, h: u32) -> Canvas {
        let mut out = Canvas::new(w, h);
        let (sx, sy) = (self.w as f64 / w as f64, self.h as f64 / h as f64);
        for y in 0..h {
            let (y0, y1) = (y as f64 * sy, (y + 1) as f64 * sy);
            for x in 0..w {
                let (x0, x1) = (x as f64 * sx, (x + 1) as f64 * sx);
                let mut acc = [0f64; 4];
                let mut area = 0.0;
                let mut yy = y0.floor() as u32;
                while (yy as f64) < y1 && yy < self.h {
                    let fy = (y1.min(yy as f64 + 1.0) - y0.max(yy as f64)).max(0.0);
                    let mut xx = x0.floor() as u32;
                    while (xx as f64) < x1 && xx < self.w {
                        let fx = (x1.min(xx as f64 + 1.0) - x0.max(xx as f64)).max(0.0);
                        let a = fx * fy;
                        let p = self.px[(yy * self.w + xx) as usize];
                        let pa = p[3] as f64;
                        for k in 0..3 {
                            acc[k] += p[k] as f64 * pa * a;
                        }
                        acc[3] += pa * a;
                        area += a;
                        xx += 1;
                    }
                    yy += 1;
                }
                if area > 0.0 && acc[3] > 0.0 {
                    let alpha = acc[3] / area;
                    out.px[(y * w + x) as usize] = [
                        (acc[0] / acc[3]) as f32,
                        (acc[1] / acc[3]) as f32,
                        (acc[2] / acc[3]) as f32,
                        alpha as f32,
                    ];
                }
            }
        }
        out
    }

    /// Operação raster do GDI (ROP3) num pixel: `p` = pincel, `s` = fonte.
    /// Fundo ainda vazio conta como branco; se continuar branco, fica vazio.
    fn rop(&mut self, x: u32, y: u32, rop: u32, p: [u8; 3], s: [u8; 3]) {
        let i = (y * self.w + x) as usize;
        let d = self.px[i];
        let empty = d[3] <= 0.0;
        let dv: [u8; 3] =
            if empty { [255; 3] } else { [0, 1, 2].map(|k| (d[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8) };
        let out = [0, 1, 2].map(|k| rop3(rop, p[k], s[k], dv[k]));
        if empty && out == [255; 3] {
            return;
        }
        self.px[i] = [out[0] as f32 / 255.0, out[1] as f32 / 255.0, out[2] as f32 / 255.0, 1.0];
    }

    /// Preenche polígonos (regra não-zero ou par-ímpar) com antisserrilhado
    /// vertical de 4 sub-linhas e cobertura horizontal exata nas pontas.
    fn fill(&mut self, polys: &[Vec<Pt>], color: [f32; 4], nonzero: bool, clip: [f64; 4]) {
        let mut edges: Vec<(f64, f64, f64, f64, i32)> = Vec::new();
        let (mut y0, mut y1) = (f64::MAX, f64::MIN);
        for poly in polys {
            if poly.len() < 2 {
                continue;
            }
            for k in 0..poly.len() {
                let a = poly[k];
                let b = poly[(k + 1) % poly.len()];
                if (a.1 - b.1).abs() < 1e-12 || !a.0.is_finite() || !b.0.is_finite() {
                    continue;
                }
                let dir = if b.1 > a.1 { 1 } else { -1 };
                let (top, bot) = if a.1 < b.1 { (a, b) } else { (b, a) };
                edges.push((top.0, top.1, bot.0, bot.1, dir));
                y0 = y0.min(top.1);
                y1 = y1.max(bot.1);
            }
        }
        if edges.is_empty() {
            return;
        }
        let cx0 = clip[0].max(0.0);
        let cx1 = clip[2].min(self.w as f64);
        let ys = (y0.max(clip[1]).max(0.0).floor()) as i64;
        let ye = (y1.min(clip[3]).min(self.h as f64).ceil()) as i64;
        if cx1 <= cx0 || ye <= ys {
            return;
        }
        let width = self.w as usize;
        let mut cov = vec![0f32; width + 1];
        let mut xs: Vec<(f64, i32)> = Vec::new();
        const SUB: usize = 4;
        for y in ys..ye {
            cov.iter_mut().for_each(|c| *c = 0.0);
            let (mut lo, mut hi) = (usize::MAX, 0usize);
            for s in 0..SUB {
                let sy = y as f64 + (s as f64 + 0.5) / SUB as f64;
                if sy < clip[1] || sy >= clip[3] {
                    continue;
                }
                xs.clear();
                for &(ax, ay, bx, by, dir) in &edges {
                    if sy >= ay && sy < by {
                        xs.push((ax + (sy - ay) / (by - ay) * (bx - ax), dir));
                    }
                }
                xs.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut wind = 0;
                for k in 0..xs.len() {
                    let before = if nonzero { wind != 0 } else { wind % 2 != 0 };
                    wind += if nonzero { xs[k].1 } else { 1 };
                    let after = if nonzero { wind != 0 } else { wind % 2 != 0 };
                    if !before && after {
                        // Começo de um trecho: acha o fim.
                        let start = xs[k].0;
                        let mut w2 = wind;
                        let mut end = start;
                        for x in xs.iter().skip(k + 1) {
                            w2 += if nonzero { x.1 } else { 1 };
                            let inside = if nonzero { w2 != 0 } else { w2 % 2 != 0 };
                            if !inside {
                                end = x.0;
                                break;
                            }
                        }
                        let (a, b) = (start.max(cx0), end.min(cx1));
                        if b > a {
                            span(&mut cov, a, b, 1.0 / SUB as f32);
                            lo = lo.min(a.floor() as usize);
                            hi = hi.max(b.ceil() as usize);
                        }
                    }
                }
            }
            if lo == usize::MAX {
                continue;
            }
            for (x, c) in cov.iter().enumerate().take(hi.min(width)).skip(lo) {
                let a = c.min(1.0);
                if a > 0.0 {
                    self.blend(x as u32, y as u32, color, a);
                }
            }
        }
    }
}

/// Resultado de uma operação raster ternária (índice no byte alto do código),
/// bit a bit: cada bit do resultado vem da tabela pelo trio (P, S, D).
fn rop3(rop: u32, p: u8, s: u8, d: u8) -> u8 {
    let idx = (rop >> 16) & 0xFF;
    let mut r = 0u8;
    for bit in 0..8 {
        let k = ((p >> bit) & 1) << 2 | ((s >> bit) & 1) << 1 | ((d >> bit) & 1);
        r |= (((idx >> k) & 1) as u8) << bit;
    }
    r
}

/// Códigos de operação raster usados aqui.
const SRCCOPY: u32 = 0x00CC_0020;
const PATCOPY: u32 = 0x00F0_0021;
const DSTCOPY: u32 = 0x00AA_0029;

/// Soma `w` de cobertura no trecho [a, b) (pontas fracionárias).
fn span(cov: &mut [f32], a: f64, b: f64, w: f32) {
    let (ia, ib) = (a.floor() as usize, b.floor() as usize);
    if ia >= cov.len() {
        return;
    }
    if ia == ib {
        cov[ia] += w * (b - a) as f32;
        return;
    }
    cov[ia] += w * (1.0 - (a - ia as f64)) as f32;
    let end = ib.min(cov.len());
    for c in cov.iter_mut().take(end).skip(ia + 1) {
        *c += w;
    }
    if ib < cov.len() {
        cov[ib] += w * (b - ib as f64) as f32;
    }
}

/// Contorno de uma linha poligonal como polígonos (segmentos + juntas), todos
/// com a mesma orientação (a união com a regra não-zero não abre buracos).
fn stroke(line: &[Pt], closed: bool, hw: f64) -> Vec<Vec<Pt>> {
    let mut out = Vec::new();
    let n = line.len();
    if n == 0 {
        return out;
    }
    let segs = if closed { n } else { n.saturating_sub(1) };
    let orient = |mut p: Vec<Pt>| {
        let area: f64 = (0..p.len())
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                a.0 * b.1 - b.0 * a.1
            })
            .sum();
        if area < 0.0 {
            p.reverse();
        }
        p
    };
    for i in 0..segs {
        let a = line[i];
        let b = line[(i + 1) % n];
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            continue;
        }
        let (nx, ny) = (-dy / len * hw, dx / len * hw);
        out.push(orient(vec![
            (a.0 + nx, a.1 + ny),
            (b.0 + nx, b.1 + ny),
            (b.0 - nx, b.1 - ny),
            (a.0 - nx, a.1 - ny),
        ]));
    }
    // Juntas redondas (octógonos) nos vértices, quando a linha é grossa.
    if hw > 0.75 {
        for &(x, y) in line {
            let oct: Vec<Pt> = (0..8)
                .map(|k| {
                    let t = k as f64 * std::f64::consts::FRAC_PI_4;
                    (x + hw * t.cos(), y + hw * t.sin())
                })
                .collect();
            out.push(orient(oct));
        }
    }
    out
}

/// Curva de Bézier cúbica em segmentos.
fn bezier(out: &mut Vec<Pt>, p0: Pt, p1: Pt, p2: Pt, p3: Pt) {
    const N: usize = 16;
    for k in 1..=N {
        let t = k as f64 / N as f64;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1));
    }
}

// ---------------------------------------------------------------------------
// Estado do GDI e os registros
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Obj {
    Pen {
        color: [u8; 4],
        width: f64,
        null: bool,
    },
    Brush {
        color: [u8; 4],
        null: bool,
    },
    /// Altura (lógica; negativa = altura do caractere), peso, itálico, sublinhado, tachado.
    /// `escapement`: ângulo do texto em décimos de grau (anti-horário).
    Font {
        face: String,
        height: i32,
        escapement: i32,
        weight: u32,
        italic: bool,
        underline: bool,
        strike: bool,
    },
    Other,
}

#[derive(Clone)]
struct State {
    /// Transformação do mundo: (m11, m12, m21, m22, dx, dy).
    xf: [f64; 6],
    map_mode: u32,
    win_org: Pt,
    win_ext: Pt,
    vp_org: Pt,
    vp_ext: Pt,
    pen: Obj,
    brush: Obj,
    font: Obj,
    text_color: [u8; 4],
    bk_color: [u8; 4],
    text_align: u32,
    nonzero: bool,
    /// Recorte em pixels da imagem.
    clip: [f64; 4],
    cur: Pt,
}

struct Player<'t> {
    canvas: Canvas,
    text: Option<&'t mut dyn Text>,
    st: State,
    stack: Vec<State>,
    objs: HashMap<u32, Obj>,
    /// Origem (pixels do dispositivo) e escala dispositivo → imagem.
    origin: [f64; 2],
    scale: f64,
    px_per_mm: (f64, f64),
    /// Caminho em construção (`BeginPath`): figuras em coordenadas da imagem.
    path: Option<Vec<(Vec<Pt>, bool)>>,
    /// Figura atual (em coordenadas da imagem).
    fig: Vec<Pt>,
}

const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn stock(id: u32) -> Option<Obj> {
    Some(match id & 0x7FFF_FFFF {
        0 => Obj::Brush { color: WHITE, null: false },
        1 => Obj::Brush { color: [192, 192, 192, 255], null: false },
        2 => Obj::Brush { color: [128, 128, 128, 255], null: false },
        3 => Obj::Brush { color: [64, 64, 64, 255], null: false },
        4 => Obj::Brush { color: BLACK, null: false },
        5 => Obj::Brush { color: BLACK, null: true },
        6 => Obj::Pen { color: WHITE, width: 0.0, null: false },
        7 => Obj::Pen { color: BLACK, width: 0.0, null: false },
        8 => Obj::Pen { color: BLACK, width: 0.0, null: true },
        18 => Obj::Brush { color: WHITE, null: false },
        19 => Obj::Pen { color: BLACK, width: 0.0, null: false },
        _ => return None,
    })
}

impl<'t> Player<'t> {
    fn new(w: u32, h: u32, origin: [f64; 2], scale: f64, px_per_mm: (f64, f64)) -> Self {
        Self {
            canvas: Canvas::new(w, h),
            text: None,
            st: State {
                xf: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                map_mode: 1,
                win_org: (0.0, 0.0),
                win_ext: (1.0, 1.0),
                vp_org: (0.0, 0.0),
                vp_ext: (1.0, 1.0),
                pen: Obj::Pen { color: BLACK, width: 0.0, null: false },
                brush: Obj::Brush { color: WHITE, null: false },
                font: Obj::Other,
                text_color: BLACK,
                bk_color: WHITE,
                text_align: 0,
                nonzero: false,
                clip: [0.0, 0.0, w as f64, h as f64],
                cur: (0.0, 0.0),
            },
            stack: Vec::new(),
            objs: HashMap::new(),
            origin,
            scale,
            px_per_mm,
            path: None,
            fig: Vec::new(),
        }
    }

    /// Unidades da página → pixels do dispositivo (modo de mapeamento).
    fn page_to_dev(&self, p: Pt) -> Pt {
        let s = &self.st;
        let (sx, sy) = match s.map_mode {
            // Modos fixos: unidades físicas (o y cresce para cima).
            2 => (self.px_per_mm.0 * 0.1, -self.px_per_mm.1 * 0.1),
            3 => (self.px_per_mm.0 * 0.01, -self.px_per_mm.1 * 0.01),
            4 => (self.px_per_mm.0 * 0.254, -self.px_per_mm.1 * 0.254),
            5 => (self.px_per_mm.0 * 0.0254, -self.px_per_mm.1 * 0.0254),
            6 => (self.px_per_mm.0 * 25.4 / 1440.0, -self.px_per_mm.1 * 25.4 / 1440.0),
            7 | 8 if s.win_ext.0 != 0.0 && s.win_ext.1 != 0.0 => {
                let (mut x, mut y) = (s.vp_ext.0 / s.win_ext.0, s.vp_ext.1 / s.win_ext.1);
                if s.map_mode == 7 {
                    // Isotrópico: a mesma escala nos dois eixos (a menor).
                    let m = x.abs().min(y.abs());
                    x = m * x.signum();
                    y = m * y.signum();
                }
                (x, y)
            }
            _ => (1.0, 1.0),
        };
        ((p.0 - s.win_org.0) * sx + s.vp_org.0, (p.1 - s.win_org.1) * sy + s.vp_org.1)
    }

    /// Coordenada lógica → pixel da imagem.
    fn pt(&self, x: f64, y: f64) -> Pt {
        let m = &self.st.xf;
        let wx = x * m[0] + y * m[2] + m[4];
        let wy = x * m[1] + y * m[3] + m[5];
        // Pixels inteiros do dispositivo, como o GDI.
        let d = self.page_to_dev((wx, wy));
        ((d.0.round() - self.origin[0]) * self.scale, (d.1.round() - self.origin[1]) * self.scale)
    }

    /// Fator de escala lógico → imagem (larguras de caneta).
    fn len_scale(&self) -> f64 {
        let a = self.pt(0.0, 0.0);
        let b = self.pt(1.0, 0.0);
        let c = self.pt(0.0, 1.0);
        let sx = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let sy = ((c.0 - a.0).powi(2) + (c.1 - a.1).powi(2)).sqrt();
        (sx * sy).sqrt()
    }

    fn color(c: [u8; 4]) -> [f32; 4] {
        [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
    }

    /// Preenche e/ou contorna figuras (fora de um caminho) com o pincel e a caneta.
    fn draw(&mut self, figs: &[(Vec<Pt>, bool)], fill: bool, line: bool) {
        if let Some(path) = &mut self.path {
            path.extend(figs.iter().cloned());
            return;
        }
        if fill && let Obj::Brush { color, null: false } = self.st.brush.clone() {
            let polys: Vec<Vec<Pt>> = figs.iter().map(|f| f.0.clone()).collect();
            self.canvas.fill(&polys, Self::color(color), self.st.nonzero, self.st.clip);
        }
        if line {
            self.stroke_figs(figs);
        }
    }

    fn stroke_figs(&mut self, figs: &[(Vec<Pt>, bool)]) {
        let Obj::Pen { color, width, null: false } = self.st.pen.clone() else { return };
        // Caneta "cosmética" (largura 0): um pixel do dispositivo.
        let wpx = if width <= 0.0 { self.scale } else { width * self.len_scale() };
        let hw = wpx.max(1.0) / 2.0;
        let mut polys = Vec::new();
        for (f, closed) in figs {
            // A linha de 1 pixel do GDI ocupa o pixel à direita/abaixo do ponto.
            let half = self.scale / 2.0;
            let f: Vec<Pt> = f.iter().map(|p| (p.0 + half, p.1 + half)).collect();
            polys.extend(stroke(&f, *closed, hw));
        }
        self.canvas.fill(&polys, Self::color(color), true, self.st.clip);
    }

    fn points16(&self, r: &[u8], off: usize, n: usize) -> Vec<Pt> {
        (0..n).map(|k| self.pt(i16le(r, off + k * 4) as f64, i16le(r, off + k * 4 + 2) as f64)).collect()
    }

    fn points32(&self, r: &[u8], off: usize, n: usize) -> Vec<Pt> {
        (0..n).map(|k| self.pt(i32le(r, off + k * 8) as f64, i32le(r, off + k * 8 + 4) as f64)).collect()
    }

    /// Ponto lógico atual em pixels.
    fn cur_px(&self) -> Pt {
        self.pt(self.st.cur.0, self.st.cur.1)
    }

    /// Guarda a posição lógica a partir de um ponto da imagem (inverte só
    /// para os registros "To", que continuam do último ponto).
    fn set_cur_logical(&mut self, x: f64, y: f64) {
        self.st.cur = (x, y);
    }

    fn record(&mut self, ty: u32, r: &[u8]) {
        match ty {
            // Polígonos e linhas (16 e 32 bits).
            2 | 3 | 4 | 5 | 6 | 85 | 86 | 87 | 88 | 89 => {
                let n = u32le(r, 24) as usize;
                let small = ty >= 85;
                let need = 28 + n * if small { 4 } else { 8 };
                if r.len() < need {
                    return;
                }
                let pts = if small { self.points16(r, 28, n) } else { self.points32(r, 28, n) };
                let last_logical = |k: usize| -> Pt {
                    if small {
                        (i16le(r, 28 + k * 4) as f64, i16le(r, 28 + k * 4 + 2) as f64)
                    } else {
                        (i32le(r, 28 + k * 8) as f64, i32le(r, 28 + k * 8 + 4) as f64)
                    }
                };
                match ty {
                    // PolyBezier
                    2 | 85 => {
                        if pts.is_empty() {
                            return;
                        }
                        let mut line = vec![pts[0]];
                        let mut k = 1;
                        while k + 2 < pts.len() {
                            let p0 = *line.last().unwrap_or(&pts[0]);
                            bezier(&mut line, p0, pts[k], pts[k + 1], pts[k + 2]);
                            k += 3;
                        }
                        self.draw(&[(line, false)], false, true);
                    }
                    // Polygon
                    3 | 86 => self.draw(&[(pts, true)], true, true),
                    // Polyline
                    4 | 87 => self.draw(&[(pts, false)], false, true),
                    // PolyBezierTo
                    5 | 88 => {
                        let start = self.cur_px();
                        if self.fig.is_empty() {
                            self.fig.push(start);
                        }
                        let mut k = 0;
                        while k + 2 < pts.len() {
                            let p0 = *self.fig.last().unwrap_or(&start);
                            let mut seg = Vec::new();
                            bezier(&mut seg, p0, pts[k], pts[k + 1], pts[k + 2]);
                            self.fig.extend(seg);
                            k += 3;
                        }
                        if n > 0 {
                            let l = last_logical(n - 1);
                            self.set_cur_logical(l.0, l.1);
                        }
                        self.flush_open_fig();
                    }
                    // PolylineTo
                    _ => {
                        let start = self.cur_px();
                        if self.fig.is_empty() {
                            self.fig.push(start);
                        }
                        self.fig.extend(pts);
                        if n > 0 {
                            let l = last_logical(n - 1);
                            self.set_cur_logical(l.0, l.1);
                        }
                        self.flush_open_fig();
                    }
                }
            }
            // PolyPolyline / PolyPolygon (32 e 16 bits).
            7 | 8 | 90 | 91 => {
                let np = u32le(r, 24) as usize;
                let total = u32le(r, 28) as usize;
                let small = ty >= 90;
                let pts_at = 32 + np * 4;
                if r.len() < pts_at + total * if small { 4 } else { 8 } || np > 100_000 {
                    return;
                }
                let all =
                    if small { self.points16(r, pts_at, total) } else { self.points32(r, pts_at, total) };
                let mut figs = Vec::with_capacity(np);
                let mut at = 0usize;
                for k in 0..np {
                    let c = u32le(r, 32 + k * 4) as usize;
                    if at + c > all.len() {
                        break;
                    }
                    figs.push((all[at..at + c].to_vec(), ty == 8 || ty == 91));
                    at += c;
                }
                let poly = ty == 8 || ty == 91;
                self.draw(&figs, poly, true);
            }
            // Rectangle, Ellipse, RoundRect.
            42..=44 => {
                let b = rect(r, 8);
                let (l, t, rr, bb) = (b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64);
                let figure: Vec<Pt> = if ty == 43 {
                    vec![self.pt(l, t), self.pt(rr, t), self.pt(rr, bb), self.pt(l, bb)]
                } else {
                    let (cx, cy) = ((l + rr) / 2.0, (t + bb) / 2.0);
                    let (rx, ry) = if ty == 42 {
                        ((rr - l) / 2.0, (bb - t) / 2.0)
                    } else {
                        (i32le(r, 24) as f64 / 2.0, i32le(r, 28) as f64 / 2.0)
                    };
                    if ty == 42 {
                        (0..64)
                            .map(|k| {
                                let a = k as f64 / 64.0 * std::f64::consts::TAU;
                                self.pt(cx + rx * a.cos(), cy + ry * a.sin())
                            })
                            .collect()
                    } else {
                        // Cantos arredondados: quartos de elipse nos quatro cantos.
                        let mut v = Vec::new();
                        let corners = [
                            (rr - rx, t + ry, -90.0),
                            (rr - rx, bb - ry, 0.0),
                            (l + rx, bb - ry, 90.0),
                            (l + rx, t + ry, 180.0),
                        ];
                        for (ccx, ccy, a0) in corners {
                            for k in 0..=8 {
                                let a = (a0 + k as f64 * 90.0 / 8.0f64).to_radians();
                                v.push(self.pt(ccx + rx * a.cos(), ccy + ry * a.sin()));
                            }
                        }
                        v
                    }
                };
                self.draw(&[(figure, true)], true, true);
            }
            // MoveToEx
            27 => {
                if self.path.is_some() {
                    self.end_fig(false);
                } else {
                    self.fig.clear();
                }
                self.st.cur = (i32le(r, 8) as f64, i32le(r, 12) as f64);
            }
            // LineTo
            54 => {
                let p = self.cur_px();
                let (x, y) = (i32le(r, 8) as f64, i32le(r, 12) as f64);
                let q = self.pt(x, y);
                if self.fig.is_empty() {
                    self.fig.push(p);
                }
                self.fig.push(q);
                self.st.cur = (x, y);
                self.flush_open_fig();
            }
            // BeginPath / EndPath / CloseFigure / AbortPath
            59 => {
                self.fig.clear();
                self.path = Some(Vec::new());
            }
            60 => self.end_fig(false),
            61 => self.end_fig(true),
            68 => {
                self.path = None;
                self.fig.clear();
            }
            // FillPath / StrokeAndFillPath / StrokePath
            62..=64 => {
                self.end_fig(false);
                let figs = self.path.take().unwrap_or_default();
                // Preencher fecha as figuras.
                if ty != 64
                    && let Obj::Brush { color, null: false } = self.st.brush.clone()
                {
                    let polys: Vec<Vec<Pt>> = figs.iter().map(|f| f.0.clone()).collect();
                    self.canvas.fill(&polys, Self::color(color), self.st.nonzero, self.st.clip);
                }
                if ty != 62 {
                    self.stroke_figs(&figs);
                }
            }
            // SelectClipPath: o retângulo do caminho (aproximação).
            67 => {
                self.end_fig(false);
                if let Some(figs) = self.path.take() {
                    let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                    for (f, _) in &figs {
                        for p in f {
                            bb = [bb[0].min(p.0), bb[1].min(p.1), bb[2].max(p.0), bb[3].max(p.1)];
                        }
                    }
                    if bb[0] < bb[2] {
                        let c = &mut self.st.clip;
                        *c = [c[0].max(bb[0]), c[1].max(bb[1]), c[2].min(bb[2]), c[3].min(bb[3])];
                    }
                }
            }
            // Objetos.
            38 => {
                let ih = u32le(r, 8);
                let style = u32le(r, 12);
                let width = i32le(r, 16) as f64;
                let color = colorref(u32le(r, 24));
                self.objs.insert(ih, Obj::Pen { color, width, null: style & 0xF == 5 });
            }
            95 => {
                let ih = u32le(r, 8);
                let style = u32le(r, 28);
                let width = u32le(r, 32) as f64;
                let bstyle = u32le(r, 36);
                let color = colorref(u32le(r, 40));
                let null = style & 0xF == 5 || bstyle == 1;
                self.objs.insert(ih, Obj::Pen { color, width, null });
            }
            39 => {
                let ih = u32le(r, 8);
                let style = u32le(r, 12);
                let color = colorref(u32le(r, 16));
                self.objs.insert(ih, Obj::Brush { color, null: style == 1 });
            }
            // Pincel de padrão (DIB): a cor média do padrão.
            93 | 94 => {
                let ih = u32le(r, 8);
                let (off_bmi, off_bits) = (u32le(r, 16) as usize, u32le(r, 24) as usize);
                let color = dib(r, off_bmi, off_bits)
                    .map(|(w, h, px)| {
                        let n = (w * h).max(1) as u64;
                        let mut s = [0u64; 3];
                        for p in px.chunks(4) {
                            for k in 0..3 {
                                s[k] += p[k] as u64;
                            }
                        }
                        [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8, 255]
                    })
                    .unwrap_or([128, 128, 128, 255]);
                self.objs.insert(ih, Obj::Brush { color, null: false });
            }
            // Fontes, paletas e outros objetos: só ocupam o índice.
            49 => {
                self.objs.insert(u32le(r, 8), Obj::Other);
            }
            // ExtCreateFontIndirectW: LOGFONTW.
            82 => {
                let face: Vec<u16> = (0..32).map(|k| u16le(r, 40 + k * 2)).take_while(|&c| c != 0).collect();
                let f = Obj::Font {
                    face: String::from_utf16_lossy(&face),
                    height: i32le(r, 12),
                    escapement: i32le(r, 20),
                    weight: u32le(r, 28),
                    italic: r.get(32).is_some_and(|&b| b != 0),
                    underline: r.get(33).is_some_and(|&b| b != 0),
                    strike: r.get(34).is_some_and(|&b| b != 0),
                };
                self.objs.insert(u32le(r, 8), f);
            }
            37 => {
                let ih = u32le(r, 8);
                let o = if ih & 0x8000_0000 != 0 { stock(ih) } else { self.objs.get(&ih).cloned() };
                match o {
                    Some(o @ Obj::Pen { .. }) => self.st.pen = o,
                    Some(o @ Obj::Brush { .. }) => self.st.brush = o,
                    Some(o @ Obj::Font { .. }) => self.st.font = o,
                    _ => {}
                }
            }
            // Texto: cor, fundo, alinhamento.
            24 => self.st.text_color = colorref(u32le(r, 8)),
            25 => self.st.bk_color = colorref(u32le(r, 8)),
            22 => self.st.text_align = u32le(r, 8),
            84 => self.text_out(r),
            40 => {
                self.objs.remove(&u32le(r, 8));
            }
            // Modo de preenchimento: 1 = par-ímpar, 2 = não-zero.
            19 => self.st.nonzero = u32le(r, 8) == 2,
            // Mapeamento.
            9 => self.st.win_ext = (i32le(r, 8) as f64, i32le(r, 12) as f64),
            10 => self.st.win_org = (i32le(r, 8) as f64, i32le(r, 12) as f64),
            11 => self.st.vp_ext = (i32le(r, 8) as f64, i32le(r, 12) as f64),
            12 => self.st.vp_org = (i32le(r, 8) as f64, i32le(r, 12) as f64),
            17 => self.st.map_mode = u32le(r, 8),
            // SetWorldTransform / ModifyWorldTransform.
            35 | 36 => {
                let x = [
                    f32le(r, 8) as f64,
                    f32le(r, 12) as f64,
                    f32le(r, 16) as f64,
                    f32le(r, 20) as f64,
                    f32le(r, 24) as f64,
                    f32le(r, 28) as f64,
                ];
                let mode = if ty == 35 { 4 } else { u32le(r, 32) };
                let cur = self.st.xf;
                self.st.xf = match mode {
                    1 => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                    2 => mul(&x, &cur),
                    3 => mul(&cur, &x),
                    _ => x,
                };
            }
            // SaveDC / RestoreDC.
            33 => self.stack.push(self.st.clone()),
            34 => {
                let k = i32le(r, 8);
                let n = if k < 0 {
                    (-k) as usize
                } else {
                    self.stack.len().saturating_sub(k as usize - 1).max(1)
                };
                for _ in 0..n.saturating_sub(1) {
                    self.stack.pop();
                }
                if let Some(s) = self.stack.pop() {
                    self.st = s;
                }
            }
            // IntersectClipRect.
            30 => {
                let b = rect(r, 8);
                let a = self.pt(b[0] as f64, b[1] as f64);
                let z = self.pt(b[2] as f64, b[3] as f64);
                let (x0, x1) = (a.0.min(z.0), a.0.max(z.0));
                let (y0, y1) = (a.1.min(z.1), a.1.max(z.1));
                let c = &mut self.st.clip;
                *c = [c[0].max(x0), c[1].max(y0), c[2].min(x1), c[3].min(y1)];
            }
            // ExtSelectClipRgn: copiar sem região = sem recorte; com região,
            // o retângulo envolvente (unidades do dispositivo).
            75 => {
                let cb = u32le(r, 8) as usize;
                let mode = u32le(r, 12);
                let full = [0.0, 0.0, self.canvas.w as f64, self.canvas.h as f64];
                if cb < 32 {
                    if mode == 5 {
                        self.st.clip = full;
                    }
                    return;
                }
                let bd = rect(r, 16 + 16);
                let tx = |x: f64| (x - self.origin[0]) * self.scale;
                let ty_ = |y: f64| (y - self.origin[1]) * self.scale;
                let rc = [tx(bd[0] as f64), ty_(bd[1] as f64), tx(bd[2] as f64), ty_(bd[3] as f64)];
                let c = &mut self.st.clip;
                *c = match mode {
                    5 => [rc[0].max(0.0), rc[1].max(0.0), rc[2].min(full[2]), rc[3].min(full[3])],
                    1 => [c[0].max(rc[0]), c[1].max(rc[1]), c[2].min(rc[2]), c[3].min(rc[3])],
                    _ => *c,
                };
            }
            // Bitmaps.
            76 | 77 | 81 | 114 => self.bitmap(ty, r),
            _ => {}
        }
    }

    /// Fecha a figura atual (no caminho, ou desenha a linha solta).
    fn end_fig(&mut self, close: bool) {
        if self.fig.len() >= 2 {
            let f = std::mem::take(&mut self.fig);
            if let Some(p) = &mut self.path {
                p.push((f, close));
            }
        } else {
            self.fig.clear();
        }
    }

    /// Fora de um caminho, linhas de MoveTo/LineTo são desenhadas na hora.
    fn flush_open_fig(&mut self) {
        if self.path.is_none() && self.fig.len() >= 2 {
            let f = std::mem::take(&mut self.fig);
            self.stroke_figs(&[(f, false)]);
            // Continua do último ponto.
        } else if self.path.is_none() {
            self.fig.clear();
        }
    }

    /// `ExtTextOutW`: cada caractere na posição do vetor de avanços (como o
    /// GDI posicionou), com a fonte, a cor e o alinhamento selecionados.
    fn text_out(&mut self, r: &[u8]) {
        let (rx, ry) = (i32le(r, 36) as f64, i32le(r, 40) as f64);
        let n = u32le(r, 44) as usize;
        let off_str = u32le(r, 48) as usize;
        let opts = u32le(r, 52);
        let rc = rect(r, 56);
        let off_dx = u32le(r, 72) as usize;
        // Retângulo opaco (ETO_OPAQUE): fundo com a cor de fundo.
        let a = self.pt(rc[0] as f64, rc[1] as f64);
        let z = self.pt(rc[2] as f64, rc[3] as f64);
        let (rx0, rx1, ry0, ry1) = (a.0.min(z.0), a.0.max(z.0), a.1.min(z.1), a.1.max(z.1));
        if opts & 0x2 != 0 && rx1 > rx0 && ry1 > ry0 {
            let poly = vec![(rx0, ry0), (rx1, ry0), (rx1, ry1), (rx0, ry1)];
            self.canvas.fill(&[poly], Self::color(self.st.bk_color), false, self.st.clip);
        }
        // Índices de glifo (ETO_GLYPH_INDEX) não dão para converter em texto.
        if n == 0 || opts & 0x10 != 0 || r.len() < off_str + n * 2 {
            return;
        }
        let Obj::Font { face, height, escapement, weight, italic, underline, strike } = self.st.font.clone()
        else {
            return;
        };
        let chars: Vec<u16> = (0..n).map(|k| u16le(r, off_str + k * 2)).collect();
        let s = String::from_utf16_lossy(&chars);
        // Altura negativa = altura do caractere; positiva = da célula (com o
        // espaço interno, ~1,2× o caractere).
        let em = if height < 0 { -height as f64 } else { height as f64 / 1.2 };
        let px = (em * self.len_scale()) as f32;
        if px < 1.0 {
            return;
        }
        let font = Font { face, px, bold: weight >= 600, italic, underline, strike };
        // Avanços (lógicos) de cada caractere; com ETO_PDY vêm em pares.
        let pdy = opts & 0x2000 != 0;
        let dx: Vec<f64> = if off_dx > 0 && r.len() >= off_dx + n * if pdy { 8 } else { 4 } {
            (0..n).map(|k| i32le(r, off_dx + k * if pdy { 8 } else { 4 }) as f64).collect()
        } else {
            Vec::new()
        };
        let clip = if opts & 0x4 != 0 {
            let c = self.st.clip;
            [c[0].max(rx0), c[1].max(ry0), c[2].min(rx1), c[3].min(ry1)]
        } else {
            self.st.clip
        };
        let color = Self::color(self.st.text_color);
        let (rx, ry) = if self.st.text_align & 1 != 0 { self.st.cur } else { (rx, ry) };
        let ls = self.len_scale().max(1e-9);
        let Some(text) = self.text.as_mut() else { return };
        // Largura total (lógica): soma dos avanços ou medida.
        let total = if dx.is_empty() { text.measure(&s, &font) as f64 / ls } else { dx.iter().sum() };
        let shift = match self.st.text_align & 6 {
            2 => -total,
            6 => -total / 2.0,
            _ => 0.0,
        };
        let ascent = text.ascent(&font) as f64;
        let line = font.px as f64 * 1.3;
        let mut cov = Vec::new();
        let mut x = rx + shift;
        let units: Vec<char> = s.chars().collect();
        // Texto girado: monta na horizontal a partir do ponto de referência e
        // gira os pixels em volta dele.
        if escapement % 3600 != 0 {
            let (sin, cos) = (escapement as f64 / 10.0).to_radians().sin_cos();
            let p0 = self.pt(rx, ry);
            let top = match self.st.text_align & 24 {
                24 => -ascent,
                8 => -line,
                _ => 0.0,
            };
            let ls = self.len_scale();
            let mut adv = shift * ls;
            let mut k = 0usize;
            let mut all = Vec::new();
            for ch in units {
                let Some(text) = self.text.as_mut() else { return };
                cov.clear();
                if dx.is_empty() {
                    text.glyphs(&s, &font, adv as f32, top as f32, &mut cov);
                } else {
                    text.glyphs(&ch.to_string(), &font, adv as f32, top as f32, &mut cov);
                }
                for &(px, py, a) in &cov {
                    let (fx, fy) = (px as f64 + 0.5, py as f64 + 0.5);
                    let rx2 = p0.0 + fx * cos + fy * sin;
                    let ry2 = p0.1 - fx * sin + fy * cos;
                    all.push((rx2.floor() as i32, ry2.floor() as i32, a));
                }
                if dx.is_empty() {
                    break;
                }
                adv += dx.get(k).copied().unwrap_or(0.0) * ls;
                k += ch.len_utf16();
            }
            self.paint(&all, color, clip);
            return;
        }
        // Um caractere por avanço (pares substitutos contam como dois).
        let mut k = 0usize;
        for ch in units {
            let p = {
                let m = &self.st.xf;
                let wx = x * m[0] + ry * m[2] + m[4];
                let wy = x * m[1] + ry * m[3] + m[5];
                let d = self.page_to_dev((wx, wy));
                ((d.0 - self.origin[0]) * self.scale, (d.1 - self.origin[1]) * self.scale)
            };
            let top = match self.st.text_align & 24 {
                24 => p.1 - ascent,
                8 => p.1 - line,
                _ => p.1,
            };
            let Some(text) = self.text.as_mut() else { return };
            if !dx.is_empty() {
                cov.clear();
                text.glyphs(&ch.to_string(), &font, p.0 as f32, top as f32, &mut cov);
                self.paint(&cov, color, clip);
                x += dx.get(k).copied().unwrap_or(0.0);
                k += ch.len_utf16();
            } else {
                cov.clear();
                text.glyphs(&s, &font, p.0 as f32, top as f32, &mut cov);
                self.paint(&cov, color, clip);
                break;
            }
        }
        let _ = (font.underline, font.strike);
    }

    fn paint(&mut self, cov: &[(i32, i32, f32)], color: [f32; 4], clip: [f64; 4]) {
        for &(x, y, a) in cov {
            if x < 0 || y < 0 || (x as u32) >= self.canvas.w || (y as u32) >= self.canvas.h {
                continue;
            }
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            if fx < clip[0] || fx >= clip[2] || fy < clip[1] || fy >= clip[3] {
                continue;
            }
            self.canvas.blend(x as u32, y as u32, color, a);
        }
    }

    fn bitmap(&mut self, ty: u32, r: &[u8]) {
        // Destino e fonte de cada tipo.
        let rop = match ty {
            81 => u32le(r, 68),
            114 => SRCCOPY,
            _ => u32le(r, 40),
        };
        let (xd, yd, cxd, cyd, xs, ys, cxs, cys, off_bmi, cb_bmi, off_bits, alpha) = match ty {
            81 => {
                let (xd, yd) = (i32le(r, 24), i32le(r, 28));
                let (xs, ys, cxs, cys) = (i32le(r, 32), i32le(r, 36), i32le(r, 40), i32le(r, 44));
                let (ob, cb, obits) = (u32le(r, 48) as usize, u32le(r, 52) as usize, u32le(r, 56) as usize);
                let (cxd, cyd) = (i32le(r, 72), i32le(r, 76));
                (xd, yd, cxd, cyd, xs, ys, cxs, cys, ob, cb, obits, None)
            }
            114 => {
                let (xd, yd, cxd, cyd) = (i32le(r, 24), i32le(r, 28), i32le(r, 32), i32le(r, 36));
                let blend = u32le(r, 40);
                let (xs, ys) = (i32le(r, 44), i32le(r, 48));
                let (ob, cb, obits) = (u32le(r, 84) as usize, u32le(r, 88) as usize, u32le(r, 92) as usize);
                let (cxs, cys) = (i32le(r, 100), i32le(r, 104));
                let konst = (blend >> 16 & 0xFF) as f32 / 255.0;
                let src_alpha = blend >> 24 & 0xFF == 1;
                (xd, yd, cxd, cyd, xs, ys, cxs, cys, ob, cb, obits, Some((konst, src_alpha)))
            }
            _ => {
                let (xd, yd, cxd, cyd) = (i32le(r, 24), i32le(r, 28), i32le(r, 32), i32le(r, 36));
                let (xs, ys) = (i32le(r, 44), i32le(r, 48));
                let (ob, cb, obits) = (u32le(r, 84) as usize, u32le(r, 88) as usize, u32le(r, 92) as usize);
                let (cxs, cys) = if ty == 77 { (i32le(r, 100), i32le(r, 104)) } else { (cxd, cyd) };
                (xd, yd, cxd, cyd, xs, ys, cxs, cys, ob, cb, obits, None)
            }
        };
        let a = self.pt(xd as f64, yd as f64);
        let z = self.pt((xd + cxd) as f64, (yd + cyd) as f64);
        if rop == DSTCOPY {
            return;
        }
        let brush = match self.st.brush.clone() {
            Obj::Brush { color, null: false } => Some(color),
            _ => None,
        };
        if cb_bmi == 0 {
            // Sem bitmap: operação só com o pincel e o destino.
            if rop == PATCOPY {
                if let Some(color) = brush {
                    let poly = vec![a, (z.0, a.1), z, (a.0, z.1)];
                    self.canvas.fill(&[poly], Self::color(color), false, self.st.clip);
                }
                return;
            }
            let p = brush.map_or([0; 3], |c| [c[0], c[1], c[2]]);
            let c = self.st.clip;
            let x0 = a.0.min(z.0).max(c[0]).max(0.0).round() as i64;
            let x1 = a.0.max(z.0).min(c[2]).min(self.canvas.w as f64).round() as i64;
            let y0 = a.1.min(z.1).max(c[1]).max(0.0).round() as i64;
            let y1 = a.1.max(z.1).min(c[3]).min(self.canvas.h as f64).round() as i64;
            for y in y0..y1 {
                for x in x0..x1 {
                    self.canvas.rop(x as u32, y as u32, rop, p, [0; 3]);
                }
            }
            return;
        }
        let pat = brush.map_or([0; 3], |c| [c[0], c[1], c[2]]);
        let Some((bw, bh, px)) = dib(r, off_bmi, off_bits) else { return };
        let (x0, x1) = (a.0.min(z.0), a.0.max(z.0));
        let (y0, y1) = (a.1.min(z.1), a.1.max(z.1));
        let flip_x = z.0 < a.0;
        let flip_y = z.1 < a.1;
        let c = self.st.clip;
        let (px0, px1) =
            (x0.max(c[0]).max(0.0).floor() as i64, x1.min(c[2]).min(self.canvas.w as f64).ceil() as i64);
        let (py0, py1) =
            (y0.max(c[1]).max(0.0).floor() as i64, y1.min(c[3]).min(self.canvas.h as f64).ceil() as i64);
        if x1 - x0 < 1e-6 || y1 - y0 < 1e-6 {
            return;
        }
        let (cxs, cys) = (if cxs == 0 { bw as i32 } else { cxs }, if cys == 0 { bh as i32 } else { cys });
        for y in py0..py1 {
            for x in px0..px1 {
                let (fx, fy) = ((x as f64 + 0.5 - x0) / (x1 - x0), (y as f64 + 0.5 - y0) / (y1 - y0));
                if !(0.0..1.0).contains(&fx) || !(0.0..1.0).contains(&fy) {
                    continue;
                }
                let fx = if flip_x { 1.0 - fx } else { fx };
                let fy = if flip_y { 1.0 - fy } else { fy };
                let sx = xs as f64 + fx * cxs as f64;
                let sy = ys as f64 + fy * cys as f64;
                let (sx, sy) = (sx.floor() as i64, sy.floor() as i64);
                if sx < 0 || sy < 0 || sx >= bw as i64 || sy >= bh as i64 {
                    continue;
                }
                let i = ((sy as u32 * bw + sx as u32) * 4) as usize;
                if alpha.is_none() && rop != SRCCOPY {
                    self.canvas.rop(x as u32, y as u32, rop, pat, [px[i], px[i + 1], px[i + 2]]);
                    continue;
                }
                let mut col = [px[i] as f32 / 255.0, px[i + 1] as f32 / 255.0, px[i + 2] as f32 / 255.0, 1.0];
                if let Some((k, src_alpha)) = alpha {
                    let sa = if src_alpha { px[i + 3] as f32 / 255.0 } else { 1.0 };
                    if src_alpha && sa > 0.0 {
                        // Pré-multiplicado → normal.
                        for v in col.iter_mut().take(3) {
                            *v = (*v / sa).min(1.0);
                        }
                    }
                    col[3] = sa * k;
                }
                self.canvas.blend(x as u32, y as u32, col, 1.0);
            }
        }
    }
}

/// Multiplica transformações: aplica `a` e depois `b`.
fn mul(a: &[f64; 6], b: &[f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Bitmap independente de dispositivo (cabeçalho + pixels) → RGBA, de cima
/// para baixo. Alfa vem do bitmap de 32 bits (usado só por `AlphaBlend`).
fn dib(r: &[u8], off_bmi: usize, off_bits: usize) -> Option<(u32, u32, Vec<u8>)> {
    let h = r.get(off_bmi..)?;
    let hsize = u32le(h, 0) as usize;
    let (w, hh, bpp, comp) = if hsize == 12 {
        (u16le(h, 4) as i32, i16le(h, 6) as i32, u16le(h, 10), 0)
    } else {
        (i32le(h, 4), i32le(h, 8), u16le(h, 14), u32le(h, 16))
    };
    if w <= 0 || hh == 0 || w > 16384 || hh.abs() > 16384 {
        return None;
    }
    let (w, top_down, ht) = (w as u32, hh < 0, hh.unsigned_abs());
    let used = if hsize >= 36 { u32le(h, 32) } else { 0 };
    let pal_n = if bpp <= 8 { if used > 0 { used } else { 1 << bpp } } else { 0 } as usize;
    let mut pal = Vec::with_capacity(pal_n);
    let pal_at = hsize + if comp == 3 && hsize == 40 { 12 } else { 0 };
    for k in 0..pal_n {
        let p = pal_at + k * if hsize == 12 { 3 } else { 4 };
        pal.push([*h.get(p + 2)?, *h.get(p + 1)?, *h.get(p)?]);
    }
    // Máscaras de BI_BITFIELDS (16 e 32 bits).
    let masks = if comp == 3 {
        Some((u32le(h, hsize.min(40)), u32le(h, hsize.min(40) + 4), u32le(h, hsize.min(40) + 8)))
    } else {
        None
    };
    if comp != 0 && comp != 3 {
        return None; // RLE/JPEG/PNG dentro do DIB: fora.
    }
    let bits = r.get(off_bits..)?;
    let stride = (w as usize * bpp as usize).div_ceil(32) * 4;
    if bits.len() < stride * ht as usize {
        return None;
    }
    let mut out = vec![0u8; (w * ht * 4) as usize];
    let ch = |v: u32, m: u32| -> u8 {
        if m == 0 {
            return 0;
        }
        let shift = m.trailing_zeros();
        let max = m >> shift;
        (((v & m) >> shift) * 255 / max.max(1)) as u8
    };
    for y in 0..ht {
        let src_y = if top_down { y } else { ht - 1 - y };
        let row = &bits[src_y as usize * stride..];
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            let rgba: [u8; 4] = match bpp {
                1 | 4 | 8 => {
                    let bit = x as usize * bpp as usize;
                    let byte = row[bit / 8];
                    let idx = (byte >> (8 - bpp as usize - bit % 8)) & ((1u16 << bpp) - 1) as u8;
                    let c = pal.get(idx as usize).copied().unwrap_or([0, 0, 0]);
                    [c[0], c[1], c[2], 255]
                }
                16 => {
                    let v = u16le(row, x as usize * 2) as u32;
                    let (rm, gm, bm) = masks.unwrap_or((0x7C00, 0x03E0, 0x001F));
                    [ch(v, rm), ch(v, gm), ch(v, bm), 255]
                }
                24 => {
                    let i = x as usize * 3;
                    [row[i + 2], row[i + 1], row[i], 255]
                }
                32 => {
                    let i = x as usize * 4;
                    match masks {
                        Some((rm, gm, bm)) => {
                            let v = u32le(row, i);
                            [ch(v, rm), ch(v, gm), ch(v, bm), row[i + 3]]
                        }
                        None => [row[i + 2], row[i + 1], row[i], row[i + 3]],
                    }
                }
                _ => return None,
            };
            out[o..o + 4].copy_from_slice(&rgba);
        }
    }
    Some((w, ht, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta um EMF: cabeçalho + registros + EOF.
    fn emf(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (ty, params) in records {
            body.extend_from_slice(&ty.to_le_bytes());
            body.extend_from_slice(&((8 + params.len()) as u32).to_le_bytes());
            body.extend_from_slice(params);
        }
        let mut h = vec![0u8; 88];
        h[0..4].copy_from_slice(&1u32.to_le_bytes());
        h[4..8].copy_from_slice(&88u32.to_le_bytes());
        // Limites 0..99 e moldura de 100 px (dispositivo 1000 px = 100 mm → 10 px/mm).
        for (i, v) in [0i32, 0, 99, 99].iter().enumerate() {
            h[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [0i32, 0, 1000, 1000].iter().enumerate() {
            h[24 + i * 4..28 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        h[40..44].copy_from_slice(b" EMF");
        for (i, v) in [1000i32, 1000, 100, 100].iter().enumerate() {
            h[72 + i * 4..76 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut out = h;
        out.extend(body);
        out.extend_from_slice(&14u32.to_le_bytes());
        out.extend_from_slice(&20u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
        out
    }

    fn le(v: &[i32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn red_rectangle() {
        let b = emf(&[
            (39, le(&[1, 0, 0x0000FF, 0])),     // pincel vermelho
            (37, le(&[1])),                     // seleciona
            (37, le(&[0x8000_0008u32 as i32])), // caneta nula
            (43, le(&[10, 10, 50, 50])),        // retângulo
        ]);
        let img = render(&b, 1024).unwrap();
        assert_eq!((img.w, img.h), (200, 200));
        let at = |x: u32, y: u32| &img.rgba[((y * img.w + x) * 4) as usize..][..4];
        assert_eq!(at(60, 60), &[255, 0, 0, 255]);
        assert_eq!(at(5, 5)[3], 0, "fora do retângulo fica transparente");
        assert_eq!(at(150, 150)[3], 0);
    }

    #[test]
    fn raster_ops() {
        // SRCAND, SRCPAINT, PATINVERT e "destino" pela tabela de cada código.
        assert_eq!(rop3(0x8800C6, 0, 0b1100, 0b1010), 0b1000);
        assert_eq!(rop3(0xEE0086, 0, 0b1100, 0b1010), 0b1110);
        assert_eq!(rop3(0x5A0049, 0b1100, 0, 0b1010), 0b0110);
        assert_eq!(rop3(DSTCOPY, 0xFF, 0xFF, 0x5A), 0x5A);
        assert_eq!(rop3(SRCCOPY, 0, 0x33, 0xFF), 0x33);
    }

    #[test]
    fn bitmap_24() {
        // 2×1, de baixo para cima: azul, verde.
        let mut bmi = Vec::new();
        bmi.extend_from_slice(&40u32.to_le_bytes());
        bmi.extend_from_slice(&2i32.to_le_bytes());
        bmi.extend_from_slice(&1i32.to_le_bytes());
        bmi.extend_from_slice(&1u16.to_le_bytes());
        bmi.extend_from_slice(&24u16.to_le_bytes());
        bmi.extend_from_slice(&[0u8; 24]);
        let bits = [255u8, 0, 0, 0, 255, 0, 0, 0];
        let mut p = Vec::new();
        p.extend(le(&[0, 0, 99, 99])); // limites
        p.extend(le(&[0, 0, 0, 0, 2, 1])); // destino e fonte
        let off_bmi = 8 + p.len() as i32 + 32;
        p.extend(le(&[off_bmi, 40, off_bmi + 40, 8, 0, 0xCC0020u32 as i32, 100, 100]));
        p.extend(&bmi);
        p.extend(&bits);
        let b = emf(&[(81, p)]);
        let img = render(&b, 1024).unwrap();
        let at = |x: u32, y: u32| &img.rgba[((y * img.w + x) * 4) as usize..][..4];
        assert_eq!(at(20, 100), &[0, 0, 255, 255]);
        assert_eq!(at(180, 100), &[0, 255, 0, 255]);
    }
}
