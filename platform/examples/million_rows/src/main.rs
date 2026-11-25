//! `million_rows`: prova de conceito da F0.
//!
//! Uma grade estilo planilha com 1.000.000 de linhas × 40 colunas, dados
//! gerados na hora, rodando só sobre `platform/` (nenhum crate de office).
//!
//! Uso:
//!   cargo run --release -p million_rows            # interativo
//!   cargo run --release -p million_rows -- --bench # benchmark e sai
//!
//! Teclas: setas, PageUp/PageDown, Home, Ctrl+setas, Ctrl+Home/End,
//! Ctrl+G (vai para a linha do meio), Ctrl+C (copia a célula), F2 (vsync).
//! `--rows N` muda o tamanho da grade (padrão: 1 milhão).

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use duck_gfx::{FrameStatus, Gfx, Instance};
use duck_platform::{App, Context, Event, Key, MouseButton, WheelDelta, Window, WindowConfig};
use duck_text::TextSystem;

// ---------------------------------------------------------------------------
// Parâmetros (pixels lógicos)
// ---------------------------------------------------------------------------

/// Linhas da grade quando `--rows` não é informado.
const DEFAULT_ROWS: usize = 1_000_000;
const COLS: usize = 40;
const ROW_H: u32 = 21;
const TALL_ROW_H: u32 = 34;
const HEADER_H: f64 = 24.0;
const STATUS_H: f64 = 24.0;
const SCROLLBAR: f64 = 14.0;
const MIN_THUMB: f64 = 28.0;
const FONT_PX: f32 = 13.0;
const PAD: f64 = 5.0;
const ATLAS: u32 = 2048;

const WHITE: [u8; 4] = [255, 255, 255, 255];
const GRID: [u8; 4] = [226, 227, 227, 255];
const TEXT: [u8; 4] = [32, 33, 36, 255];
const HEADER_BG: [u8; 4] = [248, 249, 250, 255];
const HEADER_TEXT: [u8; 4] = [95, 99, 104, 255];
const HEADER_LINE: [u8; 4] = [196, 199, 197, 255];
const HEADER_ACTIVE: [u8; 4] = [211, 227, 253, 255];
const ACCENT: [u8; 4] = [26, 115, 232, 255];
const TALL_BG: [u8; 4] = [254, 247, 224, 255];
const TRACK: [u8; 4] = [241, 243, 244, 255];
const THUMB: [u8; 4] = [189, 193, 198, 255];
const THUMB_ACTIVE: [u8; 4] = [128, 134, 139, 255];

// ---------------------------------------------------------------------------
// Eixo com tamanhos variáveis: árvore de Fenwick (prefix sums)
// ---------------------------------------------------------------------------

/// Linhas ou colunas. `offset(i)` e `index_at(pos)` em O(log n), então
/// "qual linha está no pixel Y" e "ir para a linha 500000" são instantâneos
/// mesmo com tamanhos diferentes por linha.
struct Axis {
    sizes: Vec<u32>,
    tree: Vec<i64>,
    top_bit: usize,
}

impl Axis {
    // `isolate_lowest_one()` só é estável em Rust mais novo que o mínimo do projeto.
    #[allow(unknown_lints, clippy::manual_isolate_lowest_one)]
    fn new(sizes: Vec<u32>) -> Self {
        let n = sizes.len();
        let mut tree = vec![0i64; n + 1];
        for i in 1..=n {
            tree[i] += sizes[i - 1] as i64;
            let j = i + (i & i.wrapping_neg());
            if j <= n {
                tree[j] += tree[i];
            }
        }
        let top_bit = if n == 0 { 0 } else { 1 << (usize::BITS - 1 - n.leading_zeros()) };
        Self { sizes, tree, top_bit }
    }

    fn len(&self) -> usize {
        self.sizes.len()
    }

    fn size(&self, i: usize) -> u32 {
        self.sizes[i]
    }

    /// Soma dos tamanhos de `[0, i)`.
    fn offset(&self, i: usize) -> i64 {
        let mut i = i.min(self.len());
        let mut s = 0;
        while i > 0 {
            s += self.tree[i];
            i &= i - 1;
        }
        s
    }

    fn total(&self) -> i64 {
        self.offset(self.len())
    }

    /// Índice do item que contém a posição `pos`.
    fn index_at(&self, pos: i64) -> usize {
        if pos <= 0 {
            return 0;
        }
        let (mut idx, mut rem, mut step) = (0usize, pos, self.top_bit);
        while step > 0 {
            let next = idx + step;
            if next <= self.len() && self.tree[next] <= rem {
                idx = next;
                rem -= self.tree[next];
            }
            step >>= 1;
        }
        idx.min(self.len().saturating_sub(1))
    }
}

// ---------------------------------------------------------------------------
// Dados sintéticos
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

fn mix(mut x: u64) -> u64 {
    // splitmix64
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn is_tall(row: usize) -> bool {
    row % 1000 == 999
}

const PRODUCTS: [&str; 10] = [
    "Lombo Bife",
    "Filé Medalhão",
    "Costelinha",
    "Pernil",
    "Panceta",
    "Picanha",
    "Fraldinha",
    "Alcatra",
    "Contrafilé",
    "Maminha",
];
const STATUS: [&str; 4] = ["Sim", "Não", "N/A", "Parcial"];

/// Escreve o conteúdo de uma célula em `out` (reaproveitado entre células).
fn cell_text(row: usize, col: usize, out: &mut String) -> Align {
    out.clear();
    let h = mix((row as u64) << 8 | col as u64);
    if is_tall(row) {
        if col == 0 {
            out.push_str("Subtotal");
        } else if col >= 4 && col % 11 != 10 {
            fmt_br(out, (h % 10_000_000) as f64 / 10.0, 2);
            return Align::Right;
        }
        return Align::Left;
    }
    match col {
        0 => {
            let _ = write!(out, "Loja {:03}", (row * 7 + 3) % 500);
            Align::Left
        }
        1 => {
            out.push_str(PRODUCTS[(h % PRODUCTS.len() as u64) as usize]);
            Align::Left
        }
        2 => {
            let _ = write!(out, "2026-{:02}-{:02}", 1 + h % 12, 1 + (h >> 8) % 28);
            Align::Right
        }
        3 => {
            out.push_str(STATUS[(h % STATUS.len() as u64) as usize]);
            Align::Center
        }
        c if c % 11 == 10 => Align::Left, // coluna vazia
        c if c % 3 == 0 => {
            let _ = write!(out, "{}", h % 1000);
            Align::Right
        }
        _ => {
            fmt_br(out, (h % 100_000_000) as f64 / 100.0, 2);
            Align::Right
        }
    }
}

/// Número no formato brasileiro: 1.234.567,89
fn fmt_br(out: &mut String, v: f64, decimals: u32) {
    let pow = 10u64.pow(decimals);
    let scaled = (v.abs() * pow as f64).round() as u64;
    let (int, frac) = (scaled / pow, scaled % pow);
    if v < 0.0 {
        out.push('-');
    }
    let mut digits = [0u8; 20];
    let (mut n, mut x) = (0, int);
    loop {
        digits[n] = (x % 10) as u8;
        n += 1;
        x /= 10;
        if x == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        out.push((b'0' + digits[i]) as char);
        if i > 0 && i % 3 == 0 {
            out.push('.');
        }
    }
    if decimals > 0 {
        let _ = write!(out, ",{:0width$}", frac, width = decimals as usize);
    }
}

fn col_name(mut c: usize, out: &mut String) {
    out.clear();
    let mut buf = [0u8; 8];
    let mut n = 0;
    loop {
        buf[n] = b'A' + (c % 26) as u8;
        n += 1;
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    for i in (0..n).rev() {
        out.push(buf[i] as char);
    }
}

// ---------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------

const WARMUP_FRAMES: usize = 30;
const PHASES: [(&str, usize); 3] = [
    ("scroll vertical (40 px/frame)", 2000),
    ("saltos aleatórios (pior caso)", 400),
    ("scroll horizontal (30 px/frame)", 600),
];

struct Bench {
    phase: usize,
    frame: usize,
    warmup: usize,
    rng: u64,
    cpu_ms: Vec<f64>,
    instances: usize,
    phase_start: Instant,
    report: String,
}

impl Bench {
    fn new() -> Self {
        Self {
            phase: 0,
            frame: 0,
            warmup: WARMUP_FRAMES,
            rng: 42,
            cpu_ms: Vec::new(),
            instances: 0,
            phase_start: Instant::now(),
            report: String::new(),
        }
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

// ---------------------------------------------------------------------------
// O app
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Drag {
    VThumb { grab: f64 },
    HThumb { grab: f64 },
}

/// Geometria do frame em pixels físicos.
#[derive(Clone, Copy)]
struct Layout {
    s: f64,
    w: f64,
    h: f64,
    hh: f64,
    rw: f64,
    sh: f64,
    sb: f64,
    /// Área das células: x0, y0, x1, y1
    vx0: f64,
    vy0: f64,
    vx1: f64,
    vy1: f64,
}

impl Layout {
    fn view_w(&self) -> f64 {
        (self.vx1 - self.vx0) / self.s
    }
    fn view_h(&self) -> f64 {
        (self.vy1 - self.vy0) / self.s
    }
}

struct Sheet {
    start: Instant,
    window: Option<Window>,
    gfx: Option<Gfx>,
    text: Option<TextSystem>,
    scale: f64,
    rownum_w: f64,
    rows: Axis,
    cols: Axis,
    scroll_x: f64,
    scroll_y: f64,
    active: (usize, usize),
    mouse: (f64, f64),
    drag: Option<Drag>,
    vsync: bool,
    instances: Vec<Instance>,
    cellbuf: String,
    labelbuf: String,
    recent_cpu: Vec<f64>,
    startup: Vec<(&'static str, Duration)>,
    gpu_steps: Vec<(&'static str, Duration)>,
    first_frame: Option<Duration>,
    bench: Option<Bench>,
    error: Option<String>,
    /// `--screenshot <arquivo.png>`: renderiza um frame fora da tela e sai.
    screenshot: Option<String>,
    /// Fontes carregando em paralelo com a janela e a GPU.
    fonts_loading: Option<std::thread::JoinHandle<(TextSystem, Duration)>>,
}

impl Sheet {
    fn new(bench: bool, nrows: usize) -> Self {
        let start = Instant::now();
        let fonts_loading = std::thread::spawn(|| {
            let t = Instant::now();
            let ts = TextSystem::new(FONT_PX, ATLAS);
            (ts, t.elapsed())
        });
        let t = Instant::now();
        let rows = Axis::new((0..nrows).map(|r| if is_tall(r) { TALL_ROW_H } else { ROW_H }).collect());
        let cols = Axis::new(
            (0..COLS)
                .map(|c| match c {
                    0 => 90,
                    1 => 130,
                    2 => 96,
                    3 => 70,
                    c if c % 11 == 10 => 60,
                    c if c % 3 == 0 => 70,
                    _ => 110,
                })
                .collect(),
        );
        let axis_time = t.elapsed();
        Self {
            start,
            window: None,
            gfx: None,
            text: None,
            scale: 1.0,
            rownum_w: 64.0,
            rows,
            cols,
            scroll_x: 0.0,
            scroll_y: 0.0,
            active: (0, 0),
            mouse: (0.0, 0.0),
            drag: None,
            vsync: !bench,
            instances: Vec::with_capacity(32 * 1024),
            cellbuf: String::with_capacity(64),
            labelbuf: String::with_capacity(64),
            recent_cpu: Vec::with_capacity(120),
            startup: vec![("índice das linhas (Fenwick)", axis_time)],
            gpu_steps: Vec::new(),
            first_frame: None,
            bench: bench.then(Bench::new),
            error: None,
            screenshot: None,
            fonts_loading: Some(fonts_loading),
        }
    }

    fn layout(&self) -> Layout {
        let (w, h) = self.gfx.as_ref().map(|g| g.size()).unwrap_or((1, 1));
        let s = self.scale;
        let (w, h) = (w as f64, h as f64);
        let hh = (HEADER_H * s).round();
        let rw = self.rownum_w;
        let sh = (STATUS_H * s).round();
        let sb = (SCROLLBAR * s).round();
        Layout {
            s,
            w,
            h,
            hh,
            rw,
            sh,
            sb,
            vx0: rw,
            vy0: hh,
            vx1: (w - sb).max(rw + 1.0),
            vy1: (h - sh - sb).max(hh + 1.0),
        }
    }

    fn update_metrics(&mut self) {
        if let Some(text) = &mut self.text {
            text.set_font_size(FONT_PX * self.scale as f32);
            // Largura do maior número de linha que existe na grade.
            let w = text.measure(&self.rows.len().to_string()) as f64;
            self.rownum_w = (w + 2.0 * PAD * self.scale).ceil().max(40.0 * self.scale);
        }
    }

    fn clamp_scroll(&mut self) {
        let l = self.layout();
        let max_x = (self.cols.total() as f64 - l.view_w()).max(0.0);
        let max_y = (self.rows.total() as f64 - l.view_h()).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, max_x);
        self.scroll_y = self.scroll_y.clamp(0.0, max_y);
    }

    fn ensure_visible(&mut self) {
        let l = self.layout();
        let (r, c) = self.active;
        let (top, bottom) = (self.rows.offset(r) as f64, self.rows.offset(r + 1) as f64);
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + l.view_h() {
            self.scroll_y = bottom - l.view_h();
        }
        let (left, right) = (self.cols.offset(c) as f64, self.cols.offset(c + 1) as f64);
        if left < self.scroll_x {
            self.scroll_x = left;
        } else if right > self.scroll_x + l.view_w() {
            self.scroll_x = right - l.view_w();
        }
        self.clamp_scroll();
    }

    fn visible_rows(&self) -> usize {
        let l = self.layout();
        ((l.view_h() / ROW_H as f64) as usize).max(1)
    }

    fn redraw_later(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    // --- scrollbars -------------------------------------------------------

    /// (trilho início, trilho fim, thumb início, thumb tamanho), físicos.
    fn vthumb(&self, l: &Layout) -> (f64, f64, f64, f64) {
        thumb(l.vy0, l.vy1, self.rows.total() as f64, l.view_h(), self.scroll_y, MIN_THUMB * l.s)
    }

    fn hthumb(&self, l: &Layout) -> (f64, f64, f64, f64) {
        thumb(l.vx0, l.vx1, self.cols.total() as f64, l.view_w(), self.scroll_x, MIN_THUMB * l.s)
    }

    // --- desenho ----------------------------------------------------------

    fn build_frame(&mut self) {
        let l = self.layout();
        let s = l.s;
        let px = s.round().max(1.0);
        let out = &mut self.instances;
        out.clear();
        let Some(text) = self.text.as_mut() else { return };
        let mut buf = std::mem::take(&mut self.cellbuf);
        let mut label = std::mem::take(&mut self.labelbuf);
        let view = [l.vx0 as f32, l.vy0 as f32, l.vx1 as f32, l.vy1 as f32];

        // Linhas e colunas visíveis: O(tela), nunca O(planilha).
        let mut vrows = Vec::with_capacity(64);
        let mut r = self.rows.index_at(self.scroll_y as i64);
        while r < self.rows.len() {
            let y0 = l.vy0 + ((self.rows.offset(r) as f64 - self.scroll_y) * s).round();
            if y0 >= l.vy1 {
                break;
            }
            let y1 = y0 + (self.rows.size(r) as f64 * s).round();
            vrows.push((r, y0, y1));
            r += 1;
        }
        let mut vcols = Vec::with_capacity(32);
        let mut c = self.cols.index_at(self.scroll_x as i64);
        while c < self.cols.len() {
            let x0 = l.vx0 + ((self.cols.offset(c) as f64 - self.scroll_x) * s).round();
            if x0 >= l.vx1 {
                break;
            }
            let x1 = x0 + (self.cols.size(c) as f64 * s).round();
            vcols.push((c, x0, x1));
            c += 1;
        }

        // Fundos de linhas de subtotal.
        for &(r, y0, y1) in &vrows {
            if is_tall(r) {
                rect_clipped(out, l.vx0, y0, l.vx1, y1, view, TALL_BG);
            }
        }
        // Linhas de grade.
        for &(_, _, x1) in &vcols {
            rect_clipped(out, x1 - px, l.vy0, x1, l.vy1, view, GRID);
        }
        for &(_, _, y1) in &vrows {
            rect_clipped(out, l.vx0, y1 - px, l.vx1, y1, view, GRID);
        }
        // Texto das células.
        let pad = (PAD * s).round();
        let lh = text.line_height() as f64;
        for &(r, y0, y1) in &vrows {
            let ty = y0 + ((y1 - y0 - lh) / 2.0).floor();
            for &(c, x0, x1) in &vcols {
                let align = cell_text(r, c, &mut buf);
                if buf.is_empty() {
                    continue;
                }
                let clip = [
                    (x0 + pad).max(l.vx0) as f32,
                    y0.max(l.vy0) as f32,
                    (x1 - pad).min(l.vx1) as f32,
                    y1.min(l.vy1) as f32,
                ];
                if clip[0] >= clip[2] || clip[1] >= clip[3] {
                    continue;
                }
                // Número que não cabe vira "###", como no Excel e no Sheets:
                // cortado, ele mostraria um valor errado.
                if matches!(align, Align::Right) && text.measure(&buf) as f64 > x1 - x0 - 2.0 * pad {
                    let hash_w = text.measure("#") as f64;
                    let n = (((x1 - x0 - 2.0 * pad) / hash_w).floor() as usize).max(1);
                    buf.clear();
                    buf.extend(std::iter::repeat_n('#', n));
                }
                draw_text(text, out, &buf, x0 + pad, x1 - pad, ty, align, clip, TEXT);
            }
        }
        // Célula ativa.
        let (ar, ac) = self.active;
        if let (Some(&(_, y0, y1)), Some(&(_, x0, x1))) =
            (vrows.iter().find(|v| v.0 == ar), vcols.iter().find(|v| v.0 == ac))
        {
            let b = (2.0 * s).round().max(2.0);
            let (x0, y0, x1, y1) = (x0 - px, y0 - px, x1, y1);
            rect_clipped(out, x0, y0, x1, y0 + b, view, ACCENT);
            rect_clipped(out, x0, y1 - b, x1, y1, view, ACCENT);
            rect_clipped(out, x0, y0, x0 + b, y1, view, ACCENT);
            rect_clipped(out, x1 - b, y0, x1, y1, view, ACCENT);
        }

        // Cabeçalho de colunas.
        let header_clip = [l.vx0 as f32, 0.0, l.vx1 as f32, l.hh as f32];
        out.push(Instance::solid(l.vx0 as f32, 0.0, (l.vx1 - l.vx0) as f32, l.hh as f32, HEADER_BG));
        let hy = ((l.hh - lh) / 2.0).floor();
        for &(c, x0, x1) in &vcols {
            if c == ac {
                rect_clipped(out, x0, 0.0, x1, l.hh, header_clip, HEADER_ACTIVE);
            }
            col_name(c, &mut label);
            let clip = [x0.max(l.vx0) as f32, 0.0, x1.min(l.vx1) as f32, l.hh as f32];
            draw_text(text, out, &label, x0, x1, hy, Align::Center, clip, HEADER_TEXT);
            rect_clipped(out, x1 - px, 0.0, x1, l.hh, header_clip, HEADER_LINE);
        }
        out.push(Instance::solid(0.0, (l.hh - px) as f32, l.vx1 as f32, px as f32, HEADER_LINE));

        // Cabeçalho de linhas.
        let rowhdr_clip = [0.0, l.vy0 as f32, l.rw as f32, l.vy1 as f32];
        out.push(Instance::solid(0.0, l.vy0 as f32, l.rw as f32, (l.vy1 - l.vy0) as f32, HEADER_BG));
        for &(r, y0, y1) in &vrows {
            if r == ar {
                rect_clipped(out, 0.0, y0, l.rw, y1, rowhdr_clip, HEADER_ACTIVE);
            }
            label.clear();
            let _ = write!(label, "{}", r + 1);
            let ty = y0 + ((y1 - y0 - lh) / 2.0).floor();
            let clip = [0.0, y0.max(l.vy0) as f32, l.rw as f32, y1.min(l.vy1) as f32];
            draw_text(text, out, &label, 0.0, l.rw, ty, Align::Center, clip, HEADER_TEXT);
            rect_clipped(out, 0.0, y1 - px, l.rw, y1, rowhdr_clip, HEADER_LINE);
        }
        out.push(Instance::solid((l.rw - px) as f32, 0.0, px as f32, l.vy1 as f32, HEADER_LINE));
        // Canto.
        out.push(Instance::solid(0.0, 0.0, (l.rw - px) as f32, (l.hh - px) as f32, HEADER_BG));

        // Scrollbars.
        let dragging_v = matches!(self.drag, Some(Drag::VThumb { .. }));
        let dragging_h = matches!(self.drag, Some(Drag::HThumb { .. }));
        let (_, _, ty0, tlen) =
            thumb(l.vy0, l.vy1, self.rows.total() as f64, l.view_h(), self.scroll_y, MIN_THUMB * s);
        out.push(Instance::solid(l.vx1 as f32, l.vy0 as f32, l.sb as f32, (l.vy1 - l.vy0) as f32, TRACK));
        let inset = (3.0 * s).round();
        out.push(Instance::solid(
            (l.vx1 + inset) as f32,
            ty0 as f32,
            (l.sb - 2.0 * inset) as f32,
            tlen as f32,
            if dragging_v { THUMB_ACTIVE } else { THUMB },
        ));
        let (_, _, tx0, tlen) =
            thumb(l.vx0, l.vx1, self.cols.total() as f64, l.view_w(), self.scroll_x, MIN_THUMB * s);
        out.push(Instance::solid(l.vx0 as f32, l.vy1 as f32, (l.vx1 - l.vx0) as f32, l.sb as f32, TRACK));
        out.push(Instance::solid(
            tx0 as f32,
            (l.vy1 + inset) as f32,
            tlen as f32,
            (l.sb - 2.0 * inset) as f32,
            if dragging_h { THUMB_ACTIVE } else { THUMB },
        ));
        out.push(Instance::solid(l.vx1 as f32, l.vy1 as f32, l.sb as f32, l.sb as f32, TRACK));

        // Barra de status.
        let sy = l.h - l.sh;
        out.push(Instance::solid(0.0, sy as f32, l.w as f32, l.sh as f32, HEADER_BG));
        out.push(Instance::solid(0.0, sy as f32, l.w as f32, px as f32, HEADER_LINE));
        label.clear();
        label.push_str("Linha ");
        fmt_br(&mut label, (ar + 1) as f64, 0);
        label.push_str(" de ");
        fmt_br(&mut label, self.rows.len() as f64, 0);
        let mut colname = String::new();
        col_name(ac, &mut colname);
        let _ = write!(label, " · Coluna {colname} · {} instâncias", out.len());
        if !self.recent_cpu.is_empty() {
            let avg = self.recent_cpu.iter().sum::<f64>() / self.recent_cpu.len() as f64;
            let _ = write!(label, " · CPU {:.2} ms/frame", avg);
        }
        if let Some(ff) = self.first_frame {
            let _ = write!(label, " · 1º frame {} ms", ff.as_millis());
        }
        if let Some(g) = &self.gfx {
            let info = g.adapter_info();
            let _ = write!(label, " · {} ({})", info.name, info.backend);
        }
        let _ = write!(label, " · vsync {} (F2)", if self.vsync { "on" } else { "off" });
        let ty = sy + ((l.sh - lh) / 2.0).floor();
        let clip = [0.0, sy as f32, l.w as f32, l.h as f32];
        draw_text(text, out, &label, 8.0 * s, l.w, ty, Align::Left, clip, HEADER_TEXT);

        self.cellbuf = buf;
        self.labelbuf = label;
    }

    // --- entrada ----------------------------------------------------------

    fn hit_cell(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        let l = self.layout();
        let in_x = x >= l.vx0 && x < l.vx1;
        let in_y = y >= l.vy0 && y < l.vy1;
        let row = || self.rows.index_at(((y - l.vy0) / l.s + self.scroll_y) as i64);
        let col = || self.cols.index_at(((x - l.vx0) / l.s + self.scroll_x) as i64);
        match (in_x, in_y) {
            (true, true) => Some((row(), col())),
            (true, false) if y < l.hh => Some((self.active.0, col())), // cabeçalho de coluna
            (false, true) if x < l.rw => Some((row(), self.active.1)), // cabeçalho de linha
            _ => None,
        }
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        let l = self.layout();
        // Scrollbar vertical.
        if x >= l.vx1 && y >= l.vy0 && y < l.vy1 {
            let (_, _, t0, tlen) = self.vthumb(&l);
            if y >= t0 && y < t0 + tlen {
                self.drag = Some(Drag::VThumb { grab: y - t0 });
            } else {
                let page = l.view_h() * 0.9;
                self.scroll_y += if y < t0 { -page } else { page };
            }
            return;
        }
        // Scrollbar horizontal.
        if y >= l.vy1 && y < l.h - l.sh && x >= l.vx0 && x < l.vx1 {
            let (_, _, t0, tlen) = self.hthumb(&l);
            if x >= t0 && x < t0 + tlen {
                self.drag = Some(Drag::HThumb { grab: x - t0 });
            } else {
                let page = l.view_w() * 0.9;
                self.scroll_x += if x < t0 { -page } else { page };
            }
            return;
        }
        if let Some(cell) = self.hit_cell(x, y) {
            self.active = cell;
        }
    }

    fn mouse_move(&mut self, x: f64, y: f64) -> bool {
        self.mouse = (x, y);
        let l = self.layout();
        match self.drag {
            Some(Drag::VThumb { grab }) => {
                let (a, b, _, tlen) = self.vthumb(&l);
                let max = (self.rows.total() as f64 - l.view_h()).max(0.0);
                let t = ((y - grab - a) / (b - a - tlen).max(1.0)).clamp(0.0, 1.0);
                self.scroll_y = t * max;
                true
            }
            Some(Drag::HThumb { grab }) => {
                let (a, b, _, tlen) = self.hthumb(&l);
                let max = (self.cols.total() as f64 - l.view_w()).max(0.0);
                let t = ((x - grab - a) / (b - a - tlen).max(1.0)).clamp(0.0, 1.0);
                self.scroll_x = t * max;
                true
            }
            None => false,
        }
    }

    fn key(&mut self, cx: &mut Context, key: Key, ctrl: bool) -> bool {
        let (mut r, mut c) = self.active;
        let last_r = self.rows.len() - 1;
        let last_c = COLS - 1;
        match key {
            Key::ArrowUp if ctrl => r = 0,
            Key::ArrowDown if ctrl => r = last_r,
            Key::ArrowLeft if ctrl => c = 0,
            Key::ArrowRight if ctrl => c = last_c,
            Key::ArrowUp => r = r.saturating_sub(1),
            Key::ArrowDown => r = (r + 1).min(last_r),
            Key::ArrowLeft => c = c.saturating_sub(1),
            Key::ArrowRight | Key::Tab => c = (c + 1).min(last_c),
            Key::Enter => r = (r + 1).min(last_r),
            Key::PageUp => r = r.saturating_sub(self.visible_rows()),
            Key::PageDown => r = (r + self.visible_rows()).min(last_r),
            Key::Home if ctrl => (r, c) = (0, 0),
            Key::End if ctrl => (r, c) = (last_r, last_c),
            Key::Home => c = 0,
            Key::F(2) => {
                self.vsync = !self.vsync;
                if let Some(g) = &mut self.gfx {
                    g.set_vsync(self.vsync);
                }
                return true;
            }
            Key::Char(ref k) if ctrl && k.eq_ignore_ascii_case("g") => {
                // Vai para a linha do meio da grade, centralizada.
                r = self.rows.len() / 2 - 1;
                self.active = (r, c);
                let l = self.layout();
                self.scroll_y = self.rows.offset(r) as f64 - l.view_h() / 2.0;
                self.clamp_scroll();
                return true;
            }
            Key::Char(ref k) if ctrl && k.eq_ignore_ascii_case("c") => {
                let mut buf = String::new();
                cell_text(r, c, &mut buf);
                cx.clipboard().set_text(&buf);
                return false;
            }
            _ => return false,
        }
        self.active = (r, c);
        self.ensure_visible();
        true
    }

    // --- benchmark --------------------------------------------------------

    /// Aplica o movimento do benchmark antes do frame. `false` = terminou.
    fn bench_before_frame(&mut self) {
        let nrows = self.rows.len();
        let Some(b) = &mut self.bench else { return };
        if b.warmup > 0 {
            return;
        }
        match b.phase {
            0 => self.scroll_y += 40.0,
            1 => {
                b.rng = mix(b.rng);
                let row = (b.rng % nrows as u64) as usize;
                let col = ((b.rng >> 32) % COLS as u64) as usize;
                self.scroll_y = self.rows.offset(row) as f64;
                self.scroll_x = self.cols.offset(col) as f64;
            }
            _ => {
                self.scroll_x += 30.0;
                let l = self.layout();
                if self.scroll_x > self.cols.total() as f64 - l.view_w() {
                    self.scroll_x = 0.0;
                    self.scroll_y += 400.0;
                }
            }
        }
    }

    fn bench_after_frame(&mut self, cpu_ms: f64) -> bool {
        let n_inst = self.instances.len();
        let Some(b) = &mut self.bench else { return false };
        if b.warmup > 0 {
            b.warmup -= 1;
            if b.warmup == 0 {
                b.phase_start = Instant::now();
            }
            return false;
        }
        b.cpu_ms.push(cpu_ms);
        b.instances = b.instances.max(n_inst);
        b.frame += 1;
        let (name, frames) = PHASES[b.phase];
        if b.frame < frames {
            return false;
        }
        let wall = b.phase_start.elapsed().as_secs_f64();
        let mut v = std::mem::take(&mut b.cpu_ms);
        v.sort_by(|a, b| a.total_cmp(b));
        let _ = writeln!(
            b.report,
            "  {name:<34} {frames:>5} frames | CPU/frame p50 {:>6.3} ms  p95 {:>6.3}  p99 {:>6.3}  máx {:>7.3} | {:>6.0} fps",
            percentile(&v, 0.5),
            percentile(&v, 0.95),
            percentile(&v, 0.99),
            v.last().copied().unwrap_or(0.0),
            frames as f64 / wall,
        );
        b.phase += 1;
        b.frame = 0;
        b.phase_start = Instant::now();
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        b.phase >= PHASES.len()
    }

    fn print_bench(&self) {
        let Some(b) = &self.bench else { return };
        println!("\nduck-office · F0 · million_rows --bench");
        println!("  grade: {} linhas × {} colunas (dados sintéticos)", fmt_int(self.rows.len()), COLS);
        if let Some(g) = &self.gfx {
            let i = g.adapter_info();
            println!("  GPU: {} ({}){}", i.name, i.backend, if i.software { " [software]" } else { "" });
            let (w, h) = g.size();
            println!("  janela: {w}×{h} px, escala {:.2}, vsync off", self.scale);
        }
        println!("\nInicialização:");
        for (name, d) in &self.startup {
            println!("  {name:<34} {:>8.1} ms", d.as_secs_f64() * 1000.0);
            if *name == "GPU + pipeline prontos" {
                for (step, d) in &self.gpu_steps {
                    println!("    · {step:<30} {:>8.1} ms", d.as_secs_f64() * 1000.0);
                }
            }
        }
        if let Some(ff) = self.first_frame {
            println!("  {:<34} {:>8.1} ms", "primeiro frame na tela (total)", ff.as_secs_f64() * 1000.0);
        }
        let (cur, peak) = process_memory();
        println!(
            "\nMemória do processo: {:.0} MB agora, {:.0} MB de pico",
            cur as f64 / 1048576.0,
            peak as f64 / 1048576.0
        );
        println!("\nFrames:");
        print!("{}", b.report);
        println!("  máximo de instâncias num frame: {}", b.instances);
        if let Some(t) = &self.text {
            let st = t.stats();
            println!(
                "\nTexto: {} strings moldadas, {} pelo caminho rápido, {} hits de cache, {} glifos rasterizados, {} resets de atlas",
                st.shaped_runs, st.fast_path_runs, st.cache_hits, st.glyphs_rasterized, st.atlas_resets
            );
        }
    }
}

fn fmt_int(n: usize) -> String {
    let mut s = String::new();
    fmt_br(&mut s, n as f64, 0);
    s
}

fn thumb(a: f64, b: f64, total: f64, view: f64, scroll: f64, min: f64) -> (f64, f64, f64, f64) {
    let track = (b - a).max(1.0);
    let len = if total <= view { track } else { (track * view / total).max(min).min(track) };
    let max = (total - view).max(0.0);
    let pos = if max > 0.0 { (track - len) * scroll / max } else { 0.0 };
    (a, b, (a + pos).round(), len.round())
}

fn rect_clipped(out: &mut Vec<Instance>, x0: f64, y0: f64, x1: f64, y1: f64, clip: [f32; 4], color: [u8; 4]) {
    let x0 = (x0 as f32).max(clip[0]);
    let y0 = (y0 as f32).max(clip[1]);
    let x1 = (x1 as f32).min(clip[2]);
    let y1 = (y1 as f32).min(clip[3]);
    if x0 < x1 && y0 < y1 {
        out.push(Instance::solid(x0, y0, x1 - x0, y1 - y0, color));
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    text: &mut TextSystem,
    out: &mut Vec<Instance>,
    s: &str,
    x0: f64,
    x1: f64,
    y: f64,
    align: Align,
    clip: [f32; 4],
    color: [u8; 4],
) {
    let x = match align {
        Align::Left => x0,
        Align::Right => x1 - text.measure(s) as f64,
        Align::Center => ((x0 + x1 - text.measure(s) as f64) / 2.0).floor(),
    };
    text.draw(s, x as f32, y as f32, clip, |q| out.push(Instance::glyph(q.rect, q.uv, color)));
}

impl App for Sheet {
    fn init(&mut self, cx: &mut Context, window: Window) {
        self.startup.push(("janela nativa criada", self.start.elapsed()));
        self.scale = window.scale_factor();
        let (w, h) = window.inner_size();
        let t = Instant::now();
        match Gfx::new(window.clone(), w, h, self.vsync, ATLAS) {
            Ok(g) => self.gfx = Some(g),
            Err(e) => {
                self.error = Some(e.to_string());
                cx.exit();
                return;
            }
        }
        self.startup.push(("GPU + pipeline prontos", t.elapsed()));
        if let Some(g) = &self.gfx {
            for &(name, d) in g.init_timings() {
                self.gpu_steps.push((name, d));
            }
        }
        let t = Instant::now();
        let handle = self.fonts_loading.take().expect("fontes carregadas uma vez");
        let (mut text, load_time) = handle.join().expect("thread de fontes");
        self.startup.push(("fontes do sistema (em paralelo)", load_time));
        self.startup.push(("espera pelas fontes", t.elapsed()));
        if let Some(g) = &self.gfx {
            text.set_atlas_size(g.atlas_size());
        }
        self.text = Some(text);
        self.update_metrics();
        self.window = Some(window);
    }

    fn event(&mut self, cx: &mut Context, event: Event) {
        let redraw = match event {
            Event::Resized { width, height } => {
                if let Some(g) = &mut self.gfx {
                    g.resize(width, height);
                }
                self.clamp_scroll();
                true
            }
            Event::ScaleFactorChanged(f) => {
                self.scale = f;
                self.update_metrics();
                true
            }
            Event::Wheel { delta, modifiers } => {
                let (mut dx, mut dy) = match delta {
                    WheelDelta::Lines { x, y } => {
                        (x as f64 * 3.0 * ROW_H as f64 * 2.0, y as f64 * 3.0 * ROW_H as f64)
                    }
                    WheelDelta::Pixels { x, y } => (x / self.scale, y / self.scale),
                };
                if modifiers.shift {
                    std::mem::swap(&mut dx, &mut dy);
                }
                self.scroll_x -= dx;
                self.scroll_y -= dy;
                true
            }
            Event::MouseDown { button: MouseButton::Left, x, y, .. } => {
                self.mouse_down(x, y);
                true
            }
            Event::MouseUp { button: MouseButton::Left, .. } => self.drag.take().is_some(),
            Event::MouseMove { x, y } => self.mouse_move(x, y),
            Event::Key { key, pressed: true, modifiers, .. } => self.key(cx, key, modifiers.ctrl),
            _ => false,
        };
        if redraw {
            self.clamp_scroll();
            self.redraw_later();
        }
    }

    fn redraw(&mut self, cx: &mut Context) {
        let t0 = Instant::now();
        self.bench_before_frame();
        self.clamp_scroll();
        self.build_frame();

        let (Some(gfx), Some(text)) = (self.gfx.as_mut(), self.text.as_mut()) else { return };
        for u in text.take_uploads() {
            gfx.upload_atlas(u.x, u.y, u.w, u.h, &u.data);
        }
        let atlas_reset = text.take_atlas_reset();
        if let Some(path) = self.screenshot.take() {
            // Primeiro frame já tem todos os glifos no atlas: salva e sai.
            let (w, h, rgba) = gfx.render_to_rgba(&self.instances, WHITE);
            if let Err(e) = save_png(&path, w, h, &rgba) {
                self.error = Some(format!("não foi possível salvar {path}: {e}"));
            } else {
                println!("screenshot salvo em {path} ({w}×{h})");
            }
            cx.exit();
            return;
        }
        let status = gfx.render(&self.instances, WHITE);
        let cpu_ms = t0.elapsed().as_secs_f64() * 1000.0;

        if status == FrameStatus::Presented && self.first_frame.is_none() {
            self.first_frame = Some(self.start.elapsed());
        }
        if self.recent_cpu.len() == 120 {
            self.recent_cpu.remove(0);
        }
        self.recent_cpu.push(cpu_ms);

        if self.bench.is_some() {
            if self.bench_after_frame(cpu_ms) {
                self.print_bench();
                cx.exit();
                return;
            }
            self.redraw_later();
        } else if atlas_reset || status == FrameStatus::Skipped {
            self.redraw_later();
        }
    }
}

/// Memória do processo (atual, pico), em bytes.
#[cfg(windows)]
fn process_memory() -> (u64, u64) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: chamada simples da API do Windows com um struct zerado do tamanho declarado.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            return (c.WorkingSetSize as u64, c.PeakWorkingSetSize as u64);
        }
    }
    (0, 0)
}

#[cfg(not(windows))]
fn process_memory() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| {
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    (kb("VmRSS:"), kb("VmHWM:"))
}

fn save_png(path: &str, w: u32, h: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let bench = args.iter().any(|a| a == "--bench");
    // `--rows N`: tamanho da grade (padrão: 1 milhão). Aceita 10_000_000 ou 10.000.000.
    let nrows = value("--rows")
        .and_then(|v| v.replace(['_', '.'], "").parse::<usize>().ok())
        .unwrap_or(DEFAULT_ROWS)
        .max(2);
    let mut sheet = Sheet::new(bench, nrows);
    sheet.screenshot = value("--screenshot");
    // `--row N`: começa com a linha N (1-based) selecionada e centralizada.
    if let Some(row) = value("--row").and_then(|r| r.parse::<usize>().ok()) {
        let r = row.clamp(1, sheet.rows.len()) - 1;
        sheet.active = (r, 1);
        sheet.scroll_y = sheet.rows.offset(r) as f64 - 300.0;
    }
    let config = WindowConfig {
        title: "duck-sheets · million_rows (F0)".into(),
        width: 1440.0,
        height: 900.0,
        ..Default::default()
    };
    if let Err(e) = duck_platform::run(config, &mut sheet) {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
    if let Some(e) = sheet.error {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_offsets_and_lookup() {
        let sizes: Vec<u32> = (0..10_000).map(|i| if i % 7 == 0 { 30 } else { 20 }).collect();
        let axis = Axis::new(sizes.clone());
        let mut acc = 0i64;
        for (i, &s) in sizes.iter().enumerate() {
            assert_eq!(axis.offset(i), acc);
            assert_eq!(axis.index_at(acc), i);
            assert_eq!(axis.index_at(acc + s as i64 - 1), i);
            acc += s as i64;
        }
        assert_eq!(axis.total(), acc);
        assert_eq!(axis.index_at(acc + 1000), sizes.len() - 1);
        assert_eq!(axis.index_at(-5), 0);
    }

    #[test]
    fn br_number_format() {
        let mut s = String::new();
        fmt_br(&mut s, 1234567.891, 2);
        assert_eq!(s, "1.234.567,89");
        s.clear();
        fmt_br(&mut s, -5.0, 2);
        assert_eq!(s, "-5,00");
        s.clear();
        fmt_br(&mut s, 1_000_000.0, 0);
        assert_eq!(s, "1.000.000");
    }

    #[test]
    fn column_names() {
        let mut s = String::new();
        for (c, want) in [(0, "A"), (25, "Z"), (26, "AA"), (27, "AB"), (701, "ZZ"), (702, "AAA")] {
            col_name(c, &mut s);
            assert_eq!(s, want);
        }
    }
}
