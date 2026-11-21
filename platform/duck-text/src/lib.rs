//! `duck-text`: texto para UI e células.
//!
//! - Shaping completo (`cosmic-text`: bidi, ligaduras, fallback de fontes).
//! - Rasterização (`swash`) num **atlas de alfa** empacotado em prateleiras.
//! - **Estilos** ([`TextStyle`]): família, tamanho, negrito e itálico. Fontes
//!   que não existem no sistema caem para equivalentes métricos (Calibri →
//!   Carlito no Linux, Arial → Liberation Sans...) ou para a sans-serif padrão.
//! - **Cache de texto moldado por (estilo, string)**: a mesma string repetida
//!   100 mil vezes é moldada uma vez.
//! - **Caminho rápido para números**: strings só com dígitos e pontuação
//!   numérica são montadas por caractere, sem shaping (dígitos não têm
//!   kerning nem ligaduras nas fontes de UI). É o caso mais comum numa
//!   planilha e o que mais muda durante o scroll.
//! - **Quebra de linha** ([`TextSystem::wrap`]) para células com texto quebrado.
//! - **Início rápido** ([`TextSystem::new_fast`]): carrega só as fontes que a
//!   primeira tela quase sempre usa; o resto vem de [`scan_system_fonts`] numa
//!   thread e entra com [`TextSystem::add_fonts`]. Varrer todas as fontes do
//!   Windows (~350 arquivos) leva de 0,2 a 0,6 s com o cache de disco frio.
//!
//! O crate não depende de GPU: produz [`GlyphQuad`]s e uma fila de
//! [`AtlasUpload`]s que o renderer copia para a textura.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cosmic_text::fontdb::{self, Database};
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent, Weight,
    Wrap,
};

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

/// Estilo de um texto. Barato de copiar e de comparar (chave dos caches).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextStyle {
    /// Família registrada com [`TextSystem::family`]; 0 = sans-serif padrão.
    pub family: u16,
    /// Tamanho em 1/8 de pixel físico.
    size_q: u32,
    pub bold: bool,
    pub italic: bool,
}

impl TextStyle {
    /// Tamanho em pixels físicos.
    pub fn px(&self) -> f32 {
        self.size_q as f32 / 8.0
    }

    /// Mesmo estilo com outro tamanho.
    pub fn with_px(mut self, px: f32) -> Self {
        self.size_q = quantize(px);
        self
    }
}

fn quantize(px: f32) -> u32 {
    (px.max(1.0) * 8.0).round() as u32
}

/// Altura de linha de um tamanho de fonte (em pixels).
fn line_height_for(px: f32) -> f32 {
    (px * 1.3).ceil()
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

#[derive(Clone)]
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

/// Texto quebrado em linhas para uma largura.
pub struct Wrapped {
    lines: Vec<ShapedRun>,
    line_height: f32,
}

impl Wrapped {
    pub fn lines(&self) -> usize {
        self.lines.len()
    }

    pub fn line_width(&self, i: usize) -> f32 {
        self.lines.get(i).map_or(0.0, |l| l.width)
    }

    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    pub fn height(&self) -> f32 {
        self.line_height * self.lines.len() as f32
    }
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

/// Uma família pedida e a que de fato existe no sistema (`None` = sans-serif).
struct FamilyEntry {
    requested: String,
    resolved: Option<String>,
}

/// Equivalentes métricos (mesmas larguras) e parecidos, na ordem de preferência.
fn fallbacks(name: &str) -> &'static [&'static str] {
    match name.to_ascii_lowercase().as_str() {
        "calibri" | "calibri light" => &["Carlito", "Liberation Sans", "Arial"],
        "cambria" => &["Caladea", "Liberation Serif", "Times New Roman"],
        "arial" | "helvetica" => &["Liberation Sans", "Arimo", "Helvetica", "Arial"],
        "arial narrow" | "aptos narrow" => &["Arial Narrow", "Liberation Sans Narrow", "Carlito", "Calibri"],
        "aptos" | "aptos display" => &["Calibri", "Carlito", "Liberation Sans"],
        "times new roman" | "times" => &["Liberation Serif", "Tinos", "Times New Roman"],
        "courier new" | "courier" => &["Liberation Mono", "Cousine", "Courier New"],
        "segoe ui" => &["Noto Sans", "DejaVu Sans", "Liberation Sans"],
        "verdana" | "tahoma" => &["DejaVu Sans", "Liberation Sans"],
        _ => &[],
    }
}

/// A família (ou um equivalente de [`fallbacks`]) com o nome exato do banco
/// de fontes; `None` = não existe, usa a sans-serif padrão.
fn resolve_family(db: &Database, name: &str) -> Option<String> {
    std::iter::once(name).chain(fallbacks(name).iter().copied()).find_map(|n| {
        db.faces()
            .flat_map(|f| f.families.iter())
            .find(|(fam, _)| fam.eq_ignore_ascii_case(n))
            .map(|(fam, _)| fam.clone())
    })
}

/// Fontes do sistema lidas por [`scan_system_fonts`], prontas para
/// [`TextSystem::add_fonts`].
pub struct FontSet {
    db: Database,
}

impl FontSet {
    /// Número de faces (cada estilo de cada fonte conta uma).
    pub fn len(&self) -> usize {
        self.db.len()
    }

    pub fn is_empty(&self) -> bool {
        self.db.is_empty()
    }
}

/// Lê todas as fontes instaladas. Lento (centenas de arquivos): rode numa thread.
pub fn scan_system_fonts() -> FontSet {
    let mut db = Database::new();
    db.load_system_fonts();
    FontSet { db }
}

/// Fontes que a primeira tela quase sempre usa: a da interface e as padrões
/// do Office. Vazio = a plataforma não tem uma lista; carrega tudo.
fn essential_fonts() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        const FILES: [&str; 12] = [
            "segoeui.ttf",
            "segoeuib.ttf",
            "segoeuii.ttf",
            "segoeuiz.ttf",
            "calibri.ttf",
            "calibrib.ttf",
            "calibrii.ttf",
            "calibriz.ttf",
            "arial.ttf",
            "arialbd.ttf",
            "ariali.ttf",
            "arialbi.ttf",
        ];
        // Mesma pasta que `load_system_fonts` usa, para reconhecer as repetidas.
        let root = std::env::var_os("SYSTEMROOT").map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from);
        let dir = root.join("Fonts");
        FILES.iter().map(|f| dir.join(f)).collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Arquivo de onde veio uma face (para não repetir faces ao juntar).
fn face_key(face: &fontdb::FaceInfo) -> Option<(String, u32)> {
    let path: &Path = match &face.source {
        fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => p,
        fontdb::Source::Binary(_) => return None,
    };
    Some((path.to_string_lossy().to_lowercase(), face.index))
}

fn font_system(mut db: Database) -> FontSystem {
    // Os padrões do cosmic-text, exceto a sans-serif no Windows: lá a "Open
    // Sans" não existe e o texto caía na Segoe UI pela lista de reserva.
    db.set_monospace_family("Noto Sans Mono");
    db.set_serif_family("DejaVu Serif");
    db.set_sans_serif_family(if cfg!(windows) { "Segoe UI" } else { "Open Sans" });
    let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".into());
    FontSystem::new_with_locale_and_db(locale, db)
}

/// Sistema de texto: fontes, shaping, atlas e caches.
pub struct TextSystem {
    fonts: FontSystem,
    /// Todas as fontes do sistema já estão no banco.
    fonts_complete: bool,
    swash: SwashCache,
    buffer: Buffer,
    /// Estilo padrão (UI): sans-serif no tamanho de [`TextSystem::font_size`].
    default: TextStyle,
    families: Vec<FamilyEntry>,
    packer: ShelfPacker,
    glyphs: HashMap<CacheKey, Option<GlyphEntry>>,
    runs: HashMap<TextStyle, HashMap<Box<str>, Arc<ShapedRun>>>,
    cached_runs: usize,
    wraps: HashMap<(TextStyle, u32), HashMap<Box<str>, Arc<Wrapped>>>,
    fast_chars: HashMap<(TextStyle, char), FastChar>,
    baselines: HashMap<TextStyle, i32>,
    uploads: Vec<AtlasUpload>,
    stats: TextStats,
    atlas_reset: bool,
}

/// Limite de strings moldadas em cache antes de esvaziar (LRU fica para depois).
const MAX_CACHED_RUNS: usize = 200_000;

impl TextSystem {
    /// Com todas as fontes do sistema (lento; veja [`TextSystem::new_fast`]).
    /// `font_size` em pixels físicos; `atlas_size` em texels (atlas quadrado).
    pub fn new(font_size: f32, atlas_size: u32) -> Self {
        Self::with_fonts(font_system(scan_system_fonts().db), true, font_size, atlas_size)
    }

    /// Só com as fontes essenciais (dezenas de ms). As outras entram depois
    /// com [`TextSystem::add_fonts`]; até lá, famílias que faltam caem na
    /// sans-serif. Em plataformas sem lista de essenciais, carrega tudo
    /// ([`TextSystem::fonts_complete`] = `true`).
    pub fn new_fast(font_size: f32, atlas_size: u32) -> Self {
        let mut db = Database::new();
        for path in essential_fonts() {
            let _ = db.load_font_file(path);
        }
        let complete = db.is_empty();
        if complete {
            db.load_system_fonts();
        }
        Self::with_fonts(font_system(db), complete, font_size, atlas_size)
    }

    fn with_fonts(mut fonts: FontSystem, fonts_complete: bool, font_size: f32, atlas_size: u32) -> Self {
        let buffer = Buffer::new(&mut fonts, Metrics::new(font_size, line_height_for(font_size)));
        Self {
            fonts,
            fonts_complete,
            swash: SwashCache::new(),
            buffer,
            default: TextStyle { family: 0, size_q: quantize(font_size), bold: false, italic: false },
            families: vec![FamilyEntry { requested: String::new(), resolved: None }],
            packer: ShelfPacker::new(atlas_size),
            glyphs: HashMap::new(),
            runs: HashMap::new(),
            cached_runs: 0,
            wraps: HashMap::new(),
            fast_chars: HashMap::new(),
            baselines: HashMap::new(),
            uploads: Vec::new(),
            stats: TextStats::default(),
            atlas_reset: false,
        }
    }

    pub fn font_size(&self) -> f32 {
        self.default.px()
    }

    /// Altura de linha do estilo padrão.
    pub fn line_height(&self) -> f32 {
        line_height_for(self.default.px())
    }

    pub fn stats(&self) -> TextStats {
        self.stats
    }

    /// Estilo padrão (UI).
    pub fn default_style(&self) -> TextStyle {
        self.default
    }

    /// Registra (ou reaproveita) uma família pelo nome e diz qual fonte será
    /// usada de fato. Nome vazio = sans-serif padrão.
    pub fn family(&mut self, name: &str) -> u16 {
        if name.is_empty() {
            return 0;
        }
        if let Some(i) = self.families.iter().position(|f| f.requested.eq_ignore_ascii_case(name)) {
            return i as u16;
        }
        let resolved = resolve_family(self.fonts.db(), name);
        self.families.push(FamilyEntry { requested: name.to_string(), resolved });
        (self.families.len() - 1) as u16
    }

    /// `false` enquanto faltar juntar o resto das fontes ([`TextSystem::add_fonts`]).
    pub fn fonts_complete(&self) -> bool {
        self.fonts_complete
    }

    /// Junta as fontes de [`scan_system_fonts`] às já carregadas. Glifos no
    /// atlas continuam valendo (as faces antigas mantêm o id); o texto moldado
    /// é refeito, porque a reserva para caracteres que faltavam pode ter
    /// mudado. Retorna `true` se alguma família passou a usar outra fonte
    /// (larguras medidas antes podem ter mudado).
    pub fn add_fonts(&mut self, set: FontSet) -> bool {
        let known: HashSet<(String, u32)> = self.fonts.db().faces().filter_map(face_key).collect();
        let db = self.fonts.db_mut();
        for face in set.db.faces() {
            if face_key(face).is_none_or(|k| !known.contains(&k)) {
                db.push_face_info(face.clone());
            }
        }
        self.fonts_complete = true;
        let mut changed = false;
        for i in 1..self.families.len() {
            let resolved = resolve_family(self.fonts.db(), &self.families[i].requested);
            if resolved != self.families[i].resolved {
                self.families[i].resolved = resolved;
                changed = true;
            }
        }
        self.clear_text_caches();
        changed
    }

    /// Nome da fonte usada de fato pela família (`None` = sans-serif padrão).
    pub fn resolved_family(&self, family: u16) -> Option<&str> {
        self.families.get(family as usize).and_then(|f| f.resolved.as_deref())
    }

    /// Monta um estilo. `px` em pixels físicos.
    pub fn style(&mut self, family: &str, px: f32, bold: bool, italic: bool) -> TextStyle {
        TextStyle { family: self.family(family), size_q: quantize(px), bold, italic }
    }

    /// Altura de linha de um estilo, em pixels.
    pub fn line_height_of(&self, style: TextStyle) -> f32 {
        line_height_for(style.px())
    }

    /// Muda o tamanho do estilo padrão (ex.: mudança de DPI).
    pub fn set_font_size(&mut self, font_size: f32) {
        let q = quantize(font_size);
        if q != self.default.size_q {
            self.default.size_q = q;
        }
    }

    /// Esvazia os caches de texto moldado (ex.: depois de mudar o zoom, os
    /// tamanhos antigos não voltam tão cedo). Glifos no atlas ficam.
    pub fn clear_text_caches(&mut self) {
        self.runs.clear();
        self.cached_runs = 0;
        self.wraps.clear();
        self.fast_chars.clear();
        self.baselines.clear();
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

    /// Largura do texto em pixels (estilo padrão).
    pub fn measure(&mut self, text: &str) -> f32 {
        self.measure_styled(text, self.default)
    }

    /// Largura do texto em pixels.
    pub fn measure_styled(&mut self, text: &str, style: TextStyle) -> f32 {
        if is_numeric_like(text) {
            return text.chars().map(|c| self.fast_char(style, c).advance).sum();
        }
        self.shaped(text, style).width
    }

    /// Emite os glifos de `text` (estilo padrão) com o topo-esquerdo da linha
    /// em (x, y), recortados por `clip` (x0, y0, x1, y1). Retorna a largura.
    pub fn draw(&mut self, text: &str, x: f32, y: f32, clip: [f32; 4], sink: impl FnMut(GlyphQuad)) -> f32 {
        self.draw_styled(text, self.default, x, y, clip, sink)
    }

    /// Como [`TextSystem::draw`], com estilo.
    pub fn draw_styled(
        &mut self,
        text: &str,
        style: TextStyle,
        x: f32,
        y: f32,
        clip: [f32; 4],
        mut sink: impl FnMut(GlyphQuad),
    ) -> f32 {
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        if is_numeric_like(text) {
            self.stats.fast_path_runs += 1;
            let baseline = self.baseline(style);
            let mut pen = 0.0f32;
            for c in text.chars() {
                let fc = self.fast_char(style, c);
                if let Some(key) = fc.key {
                    self.emit(key, ox + pen.round() as i32, oy + baseline, clip, &mut sink);
                }
                pen += fc.advance;
            }
            return pen;
        }
        let run = self.shaped(text, style);
        for g in &run.glyphs {
            self.emit(g.key, ox + g.x, oy + g.y, clip, &mut sink);
        }
        run.width
    }

    /// Quebra `text` em linhas de no máximo `width` pixels (palavras inteiras;
    /// palavras maiores que a largura são cortadas por caractere).
    pub fn wrap(&mut self, text: &str, style: TextStyle, width: f32) -> Arc<Wrapped> {
        let wq = width.max(1.0).round() as u32;
        if let Some(w) = self.wraps.get(&(style, wq)).and_then(|m| m.get(text)) {
            self.stats.cache_hits += 1;
            return w.clone();
        }
        self.stats.shaped_runs += 1;
        let lh = line_height_for(style.px());
        let fam = self.families.get(style.family as usize).and_then(|f| f.resolved.clone());
        let attrs = attrs_for(&fam, style);
        self.buffer.set_metrics(Metrics::new(style.px(), lh));
        self.buffer.set_wrap(Wrap::WordOrGlyph);
        self.buffer.set_size(Some(wq as f32), None);
        self.buffer.set_text(text, &attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.fonts, false);
        let mut lines = Vec::new();
        for (i, run) in self.buffer.layout_runs().enumerate() {
            let top = i as f32 * lh;
            let glyphs = run
                .glyphs
                .iter()
                .map(|g| {
                    let p = g.physical((0.0, run.line_y - top), 1.0);
                    ShapedGlyph { key: p.cache_key, x: p.x, y: p.y }
                })
                .collect();
            lines.push(ShapedRun { glyphs, width: run.line_w });
        }
        self.buffer.set_wrap(Wrap::None);
        let w = Arc::new(Wrapped { lines, line_height: lh });
        self.wraps.entry((style, wq)).or_default().insert(text.into(), w.clone());
        w
    }

    /// Desenha a linha `line` de um texto quebrado com o topo-esquerdo em (x, y).
    pub fn draw_wrapped_line(
        &mut self,
        w: &Wrapped,
        line: usize,
        x: f32,
        y: f32,
        clip: [f32; 4],
        mut sink: impl FnMut(GlyphQuad),
    ) {
        let Some(run) = w.lines.get(line) else { return };
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        for g in &run.glyphs {
            self.emit(g.key, ox + g.x, oy + g.y, clip, &mut sink);
        }
    }

    fn baseline(&mut self, style: TextStyle) -> i32 {
        if let Some(&b) = self.baselines.get(&style) {
            return b;
        }
        // Baseline do caminho rápido = baseline do shaping de "0".
        let run = self.shaped("0", style);
        let b = run.glyphs.first().map(|g| g.y).unwrap_or(style.px() as i32);
        self.baselines.insert(style, b);
        b
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

    fn shaped(&mut self, text: &str, style: TextStyle) -> Arc<ShapedRun> {
        if let Some(run) = self.runs.get(&style).and_then(|m| m.get(text)) {
            self.stats.cache_hits += 1;
            return run.clone();
        }
        self.stats.shaped_runs += 1;
        let lh = line_height_for(style.px());
        let fam = self.families.get(style.family as usize).and_then(|f| f.resolved.clone());
        let attrs = attrs_for(&fam, style);
        self.buffer.set_metrics(Metrics::new(style.px(), lh));
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
        if self.cached_runs >= MAX_CACHED_RUNS {
            self.runs.clear();
            self.cached_runs = 0;
        }
        self.cached_runs += 1;
        self.runs.entry(style).or_default().insert(text.into(), run.clone());
        run
    }

    fn fast_char(&mut self, style: TextStyle, c: char) -> FastChar {
        if let Some(fc) = self.fast_chars.get(&(style, c)) {
            return fc.clone();
        }
        let mut tmp = [0u8; 4];
        let run = self.shaped(c.encode_utf8(&mut tmp), style);
        let fc = FastChar { key: run.glyphs.first().map(|g| g.key), advance: run.width };
        self.fast_chars.insert((style, c), fc.clone());
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

fn attrs_for(family: &Option<String>, style: TextStyle) -> Attrs<'_> {
    let fam = match family {
        Some(name) => Family::Name(name),
        None => Family::SansSerif,
    };
    Attrs::new()
        .family(fam)
        .weight(if style.bold { Weight::BOLD } else { Weight::NORMAL })
        .style(if style.italic { Style::Italic } else { Style::Normal })
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

    #[test]
    fn styles_change_metrics() {
        let mut ts = TextSystem::new(16.0, 1024);
        let regular = ts.default_style();
        let big = regular.with_px(32.0);
        let bold = TextStyle { bold: true, ..regular };
        let w = ts.measure_styled("Planilha", regular);
        assert!(w > 0.0);
        let wb = ts.measure_styled("Planilha", big);
        assert!(wb > w * 1.8, "{wb} vs {w}");
        // Negrito é mais largo (se o sistema tiver a variante; senão, igual).
        assert!(ts.measure_styled("Planilha", bold) >= w);
        // Família inexistente cai na padrão.
        let f = ts.family("Fonte Que Não Existe");
        assert!(f > 0);
        assert_eq!(ts.resolved_family(f), None);
    }

    #[test]
    fn fast_start_then_all_fonts() {
        let mut ts = TextSystem::new_fast(16.0, 1024);
        let st = ts.default_style();
        let w = ts.measure_styled("Planilha", st);
        assert!(w > 0.0, "nenhuma fonte no início rápido");
        let f = ts.family("Times New Roman");
        let _ = ts.add_fonts(scan_system_fonts());
        assert!(ts.fonts_complete());
        // Igual a carregar tudo de uma vez: mesmas faces (sem repetidas) e famílias.
        let mut full = TextSystem::new(16.0, 1024);
        assert_eq!(ts.fonts.db().len(), full.fonts.db().len());
        let g = full.family("Times New Roman");
        assert_eq!(ts.resolved_family(f), full.resolved_family(g));
        assert_eq!(ts.measure_styled("Planilha", st), full.measure_styled("Planilha", full.default_style()));
    }

    #[test]
    fn wrapping_breaks_lines() {
        let mut ts = TextSystem::new(16.0, 1024);
        let st = ts.default_style();
        let one = ts.measure_styled("palavra", st);
        let w = ts.wrap("palavra palavra palavra palavra", st, one * 2.5);
        assert!(w.lines() >= 2, "{} linhas", w.lines());
        assert!((0..w.lines()).all(|i| w.line_width(i) <= one * 2.5 + 1.0));
        let w2 = ts.wrap("uma\nduas", st, 1000.0);
        assert_eq!(w2.lines(), 2);
    }
}
