//! `duck-text`: texto de uma linha para UI e células.
//!
//! - Shaping completo (`cosmic-text`: bidi, ligaduras, fallback de fontes).
//! - Rasterização (`swash`) num **atlas de alfa** empacotado em prateleiras.
//! - **Cache de texto moldado por string**: a mesma string repetida 100 mil
//!   vezes é moldada uma vez.
//! - **Caminho rápido para números**: strings só com dígitos e pontuação
//!   numérica são montadas por caractere, sem shaping (dígitos não têm
//!   kerning nem ligaduras nas fontes de UI). É o caso mais comum numa
//!   planilha e o que mais muda durante o scroll.
//!
//! O crate não depende de GPU: produz [`GlyphQuad`]s e uma fila de
//! [`AtlasUpload`]s que o renderer copia para a textura.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent};

/// Um glifo pronto para desenhar: retângulo na tela e região no atlas,
/// ambos em pixels (1 texel = 1 pixel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphQuad {
    /// x, y, largura, altura
    pub rect: [f32; 4],
    /// u0, v0, u1, v1
    pub uv: [f32; 4],
}

/// Região do atlas que mudou e precisa ir para a GPU.
#[derive(Clone, Debug)]
pub struct AtlasUpload {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Alfa, 1 byte por pixel, `w * h` bytes.
    pub data: Vec<u8>,
}

/// Contadores para benchmark e diagnóstico.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextStats {
    pub shaped_runs: u64,
    pub fast_path_runs: u64,
    pub cache_hits: u64,
    pub glyphs_rasterized: u64,
    pub atlas_resets: u64,
}

#[derive(Clone, Copy)]
struct GlyphEntry {
    /// Deslocamento do bitmap em relação à origem do glifo (baseline).
    left: i32,
    top: i32,
    w: u32,
    h: u32,
    ax: u32,
    ay: u32,
}

struct ShapedGlyph {
    key: CacheKey,
    /// Origem do glifo relativa ao topo-esquerdo da caixa de texto.
    x: i32,
    y: i32,
}

struct ShapedRun {
    glyphs: Vec<ShapedGlyph>,
    width: f32,
}

/// Glifo pré-moldado de um caractere, usado no caminho rápido.
#[derive(Clone)]
struct FastChar {
    key: Option<CacheKey>,
    advance: f32,
}

/// Empacotador de prateleiras: simples, rápido e bom para glifos de alturas parecidas.
struct ShelfPacker {
    size: u32,
    shelves: Vec<Shelf>,
    next_y: u32,
}

struct Shelf {
    y: u32,
    h: u32,
    x: u32,
}

impl ShelfPacker {
    fn new(size: u32) -> Self {
        Self { size, shelves: Vec::new(), next_y: 0 }
    }

    fn clear(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        const PAD: u32 = 1;
        let (pw, ph) = (w + PAD, h + PAD);
        if pw > self.size || ph > self.size {
            return None;
        }
        // Melhor prateleira: a mais baixa que comporta, sem desperdiçar mais de 50%.
        let mut best: Option<usize> = None;
        for (i, s) in self.shelves.iter().enumerate() {
            if s.h >= ph
                && s.h <= ph + ph / 2
                && s.x + pw <= self.size
                && best.is_none_or(|b| self.shelves[b].h > s.h)
            {
                best = Some(i);
            }
        }
        if let Some(i) = best {
            let s = &mut self.shelves[i];
            let pos = (s.x, s.y);
            s.x += pw;
            return Some(pos);
        }
        if self.next_y + ph > self.size {
            return None;
        }
        let y = self.next_y;
        self.next_y += ph;
        self.shelves.push(Shelf { y, h: ph, x: pw });
        Some((0, y))
    }
}

/// Sistema de texto: fontes, shaping, atlas e caches.
pub struct TextSystem {
    fonts: FontSystem,
    swash: SwashCache,
    buffer: Buffer,
    font_size: f32,
    line_height: f32,
    family: Family<'static>,
    packer: ShelfPacker,
    glyphs: HashMap<CacheKey, Option<GlyphEntry>>,
    runs: HashMap<Box<str>, Arc<ShapedRun>>,
    fast_chars: HashMap<char, FastChar>,
    uploads: Vec<AtlasUpload>,
    stats: TextStats,
    atlas_reset: bool,
}

/// Limite de strings moldadas em cache antes de esvaziar (LRU fica para depois).
const MAX_CACHED_RUNS: usize = 200_000;

impl TextSystem {
    /// `font_size` em pixels físicos; `atlas_size` em texels (atlas quadrado).
    pub fn new(font_size: f32, atlas_size: u32) -> Self {
        let mut fonts = FontSystem::new();
        let line_height = (font_size * 1.3).ceil();
        let buffer = Buffer::new(&mut fonts, Metrics::new(font_size, line_height));
        Self {
            fonts,
            swash: SwashCache::new(),
            buffer,
            font_size,
            line_height,
            family: Family::SansSerif,
            packer: ShelfPacker::new(atlas_size),
            glyphs: HashMap::new(),
            runs: HashMap::new(),
            fast_chars: HashMap::new(),
            uploads: Vec::new(),
            stats: TextStats::default(),
            atlas_reset: false,
        }
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    pub fn stats(&self) -> TextStats {
        self.stats
    }

    /// Muda o tamanho da fonte (ex.: mudança de DPI). Esvazia os caches.
    pub fn set_font_size(&mut self, font_size: f32) {
        if (font_size - self.font_size).abs() < f32::EPSILON {
            return;
        }
        self.font_size = font_size;
        self.line_height = (font_size * 1.3).ceil();
        self.buffer.set_metrics(Metrics::new(self.font_size, self.line_height));
        self.runs.clear();
        self.fast_chars.clear();
        self.reset_atlas();
    }

    /// Muda o tamanho do atlas (ex.: a GPU suporta menos que o pedido).
    pub fn set_atlas_size(&mut self, size: u32) {
        if size != self.packer.size {
            self.packer = ShelfPacker::new(size);
            self.reset_atlas();
        }
    }

    /// Uploads pendentes para a textura do atlas (esvazia a fila).
    pub fn take_uploads(&mut self) -> Vec<AtlasUpload> {
        std::mem::take(&mut self.uploads)
    }

    /// `true` se o atlas foi esvaziado desde a última chamada: quads emitidos
    /// antes disso no mesmo frame podem apontar para lixo, então redesenhe.
    pub fn take_atlas_reset(&mut self) -> bool {
        std::mem::take(&mut self.atlas_reset)
    }

    /// Largura do texto em pixels.
    pub fn measure(&mut self, text: &str) -> f32 {
        if is_numeric_like(text) {
            return text.chars().map(|c| self.fast_char(c).advance).sum();
        }
        self.shaped(text).width
    }

    /// Emite os glifos de `text` com o topo-esquerdo da linha em (x, y),
    /// recortados por `clip` (x0, y0, x1, y1). Retorna a largura do texto.
    pub fn draw(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        clip: [f32; 4],
        mut sink: impl FnMut(GlyphQuad),
    ) -> f32 {
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        if is_numeric_like(text) {
            self.stats.fast_path_runs += 1;
            let baseline = self.baseline();
            let mut pen = 0.0f32;
            for c in text.chars() {
                let fc = self.fast_char(c);
                if let Some(key) = fc.key {
                    self.emit(key, ox + pen.round() as i32, oy + baseline, clip, &mut sink);
                }
                pen += fc.advance;
            }
            return pen;
        }
        let run = self.shaped(text);
        for g in &run.glyphs {
            self.emit(g.key, ox + g.x, oy + g.y, clip, &mut sink);
        }
        run.width
    }

    fn baseline(&mut self) -> i32 {
        // Baseline do caminho rápido = baseline do shaping de "0".
        let run = self.shaped("0");
        run.glyphs.first().map(|g| g.y).unwrap_or(self.font_size as i32)
    }

    fn emit(&mut self, key: CacheKey, gx: i32, gy: i32, clip: [f32; 4], sink: &mut impl FnMut(GlyphQuad)) {
        let Some(e) = self.glyph(key) else { return };
        let x0 = (gx + e.left) as f32;
        let y0 = (gy - e.top) as f32;
        let (x1, y1) = (x0 + e.w as f32, y0 + e.h as f32);
        let cx0 = x0.max(clip[0]);
        let cy0 = y0.max(clip[1]);
        let cx1 = x1.min(clip[2]);
        let cy1 = y1.min(clip[3]);
        if cx0 >= cx1 || cy0 >= cy1 {
            return;
        }
        let (u, v) = (e.ax as f32, e.ay as f32);
        sink(GlyphQuad {
            rect: [cx0, cy0, cx1 - cx0, cy1 - cy0],
            uv: [u + (cx0 - x0), v + (cy0 - y0), u + (cx1 - x0), v + (cy1 - y0)],
        });
    }

    fn shaped(&mut self, text: &str) -> Arc<ShapedRun> {
        if let Some(run) = self.runs.get(text) {
            self.stats.cache_hits += 1;
            return run.clone();
        }
        self.stats.shaped_runs += 1;
        let attrs = Attrs::new().family(self.family);
        self.buffer.set_size(None, None);
        self.buffer.set_text(text, &attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.fonts, false);

        let mut glyphs = Vec::new();
        let mut width = 0.0f32;
        if let Some(line) = self.buffer.layout_runs().next() {
            width = line.line_w;
            for g in line.glyphs {
                let p = g.physical((0.0, line.line_y), 1.0);
                glyphs.push(ShapedGlyph { key: p.cache_key, x: p.x, y: p.y });
            }
        }
        let run = Arc::new(ShapedRun { glyphs, width });
        if self.runs.len() >= MAX_CACHED_RUNS {
            self.runs.clear();
        }
        self.runs.insert(text.into(), run.clone());
        run
    }

    fn fast_char(&mut self, c: char) -> FastChar {
        if let Some(fc) = self.fast_chars.get(&c) {
            return fc.clone();
        }
        let mut tmp = [0u8; 4];
        let run = self.shaped(c.encode_utf8(&mut tmp));
        let fc = FastChar { key: run.glyphs.first().map(|g| g.key), advance: run.width };
        self.fast_chars.insert(c, fc.clone());
        fc
    }

    fn glyph(&mut self, key: CacheKey) -> Option<GlyphEntry> {
        if let Some(e) = self.glyphs.get(&key) {
            return *e;
        }
        let entry = self.rasterize(key);
        self.glyphs.insert(key, entry);
        entry
    }

    fn rasterize(&mut self, key: CacheKey) -> Option<GlyphEntry> {
        let image = self.swash.get_image_uncached(&mut self.fonts, key)?;
        let (w, h) = (image.placement.width, image.placement.height);
        if w == 0 || h == 0 {
            return None;
        }
        let alpha: Vec<u8> = match image.content {
            SwashContent::Mask => image.data,
            // Emoji colorido: por enquanto só a silhueta (alfa).
            SwashContent::Color => image.data.as_chunks::<4>().0.iter().map(|p| p[3]).collect(),
            SwashContent::SubpixelMask => image
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| ((p[0] as u16 + p[1] as u16 + p[2] as u16) / 3) as u8)
                .collect(),
        };
        let (ax, ay) = match self.packer.alloc(w, h) {
            Some(pos) => pos,
            None => {
                // Atlas cheio: recomeça do zero. Os glifos visíveis voltam no próximo frame.
                self.reset_atlas();
                self.packer.alloc(w, h)?
            }
        };
        self.stats.glyphs_rasterized += 1;
        self.uploads.push(AtlasUpload { x: ax, y: ay, w, h, data: alpha });
        Some(GlyphEntry { left: image.placement.left, top: image.placement.top, w, h, ax, ay })
    }

    fn reset_atlas(&mut self) {
        self.packer.clear();
        self.glyphs.clear();
        self.uploads.clear();
        self.atlas_reset = true;
        self.stats.atlas_resets += 1;
    }
}

/// Strings que podem ir pelo caminho rápido: dígitos e pontuação numérica.
fn is_numeric_like(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b',' | b'-' | b'+' | b'%' | b'/' | b':' | b' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_packer_fills_and_rejects() {
        let mut p = ShelfPacker::new(64);
        let a = p.alloc(10, 10).unwrap();
        let b = p.alloc(10, 10).unwrap();
        assert_eq!(a, (0, 0));
        assert_eq!(b, (11, 0));
        assert!(p.alloc(100, 5).is_none());
        let mut n = 2;
        while p.alloc(10, 10).is_some() {
            n += 1;
        }
        // 64 / 11 = 5 por prateleira, 5 prateleiras.
        assert_eq!(n, 25);
    }

    #[test]
    fn text_system_is_send() {
        // Fontes podem ser carregadas numa thread enquanto a janela e a GPU sobem.
        fn assert_send<T: Send>() {}
        assert_send::<TextSystem>();
    }

    #[test]
    fn numeric_detection() {
        assert!(is_numeric_like("1.234,56"));
        assert!(is_numeric_like("-12%"));
        assert!(is_numeric_like("2026-11-18"));
        assert!(!is_numeric_like("Loja 12"));
        assert!(!is_numeric_like(""));
    }

    #[test]
    fn clip_adjusts_uv() {
        let mut ts = TextSystem::new(16.0, 512);
        let mut quads = Vec::new();
        ts.draw("8888", 0.0, 0.0, [0.0, 0.0, 10_000.0, 10_000.0], |q| quads.push(q));
        assert!(!quads.is_empty(), "nenhum glifo: fonte do sistema não encontrada?");
        let full = quads[0];
        let mut clipped = Vec::new();
        let cut = full.rect[0] + 2.0;
        ts.draw("8888", 0.0, 0.0, [cut, 0.0, 10_000.0, 10_000.0], |q| clipped.push(q));
        let c = clipped[0];
        assert_eq!(c.rect[0], cut);
        assert_eq!(c.uv[0], full.uv[0] + 2.0);
        assert_eq!(c.rect[2], full.rect[2] - 2.0);
        // Uma única rasterização para os quatro "8".
        assert_eq!(ts.stats().glyphs_rasterized, 1);
    }
}
