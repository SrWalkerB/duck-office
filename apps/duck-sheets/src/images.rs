//! Imagens da aba (F5): PNG e JPEG das partes `xl/media/` são decodificadas
//! numa thread depois de abrir, reduzidas se forem grandes e guardadas num
//! atlas RGBA na GPU; o desenho é um retângulo com a região do atlas.
//! Metarquivos EMF (vetoriais do Windows) são rasterizados pelo `duck-emf`;
//! WMF ainda não.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc::{Receiver, channel};

use duck_core::drawing::ObjectKind;
use duck_gfx::IMAGE_ATLAS;

/// Maior lado guardado no atlas (imagens maiores são reduzidas).
const MAX_SIDE: u32 = 1024;

pub struct Decoded {
    pub part: String,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct Images {
    /// Parte → região (u0, v0, u1, v1) no atlas.
    pub uv: HashMap<String, [f32; 4]>,
    rx: Option<Receiver<Vec<Decoded>>>,
    /// Empacotamento em prateleiras: (x, y, altura da prateleira).
    shelf: (u32, u32, u32),
}

/// Glifos do `duck-text` para o texto dos EMF.
struct EmfText(duck_text::CpuGlyphs);

impl duck_emf::Text for EmfText {
    fn glyphs(&mut self, text: &str, f: &duck_emf::Font, x: f32, y: f32, out: &mut Vec<(i32, i32, f32)>) {
        let st = self.0.style(&f.face, f.px, f.bold, f.italic);
        self.0.coverage(text, st, x, y, out);
    }
    fn measure(&mut self, text: &str, f: &duck_emf::Font) -> f32 {
        let st = self.0.style(&f.face, f.px, f.bold, f.italic);
        self.0.measure(text, st)
    }
    fn ascent(&mut self, f: &duck_emf::Font) -> f32 {
        let st = self.0.style(&f.face, f.px, f.bold, f.italic);
        self.0.ascent(st)
    }
}

// Glifos para os EMF: criados na primeira imagem com texto (carregam as
// fontes do sistema), na thread das imagens.
thread_local! {
    static GLYPHS: std::cell::RefCell<Option<EmfText>> = const { std::cell::RefCell::new(None) };
}

fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if bytes.starts_with(b"\x89PNG") {
        let mut d = png::Decoder::new(Cursor::new(bytes));
        d.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = d.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()?];
        let info = r.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width, info.height);
        let px = (w * h) as usize;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..px * 4].to_vec(),
            png::ColorType::Rgb => buf[..px * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::GrayscaleAlpha => {
                buf[..px * 2].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect()
            }
            png::ColorType::Indexed => return None,
        };
        return Some((w, h, rgba));
    }
    if duck_emf::is_emf(bytes) {
        let img = GLYPHS.with(|g| {
            let mut g = g.borrow_mut();
            let t = g.get_or_insert_with(|| EmfText(duck_text::CpuGlyphs::new()));
            duck_emf::render_with(bytes, MAX_SIDE, Some(t))
        })?;
        return Some((img.w, img.h, img.rgba));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        use zune_jpeg::zune_core::colorspace::ColorSpace;
        use zune_jpeg::zune_core::options::DecoderOptions;
        let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
        let mut d = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), opts);
        let px = d.decode().ok()?;
        let (w, h) = d.dimensions()?;
        return Some((w as u32, h as u32, px));
    }
    None
}

/// Reduz por média de blocos até caber em `MAX_SIDE`.
fn shrink(w: u32, h: u32, rgba: Vec<u8>) -> (u32, u32, Vec<u8>) {
    let f = w.max(h).div_ceil(MAX_SIDE).max(1);
    if f == 1 {
        return (w, h, rgba);
    }
    let (nw, nh) = (w / f, h / f);
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            for dy in 0..f {
                for dx in 0..f {
                    let i = (((y * f + dy) * w + (x * f + dx)) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += rgba[i + c] as u32;
                    }
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / (f * f)) as u8;
            }
        }
    }
    (nw.max(1), nh.max(1), out)
}

impl crate::app::SheetsApp {
    /// Começa a decodificar as imagens das abas (depois de carregar).
    pub fn load_images(&mut self) {
        let Some(doc) = &self.doc else { return };
        let mut parts: Vec<String> = Vec::new();
        for s in doc.sheets.iter().filter_map(|s| s.data.as_ref()) {
            for o in &s.layout.objects {
                if let ObjectKind::Picture { part } = &o.kind
                    && !parts.contains(part)
                {
                    parts.push(part.clone());
                }
            }
        }
        if parts.is_empty() {
            return;
        }
        let path = doc.path.clone();
        let Some(waker) = self.waker.clone() else { return };
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("duck-imagens".into())
            .spawn(move || {
                let Ok(book) = duck_xlsx::Book::open(&path) else { return };
                let out: Vec<Decoded> = parts
                    .into_iter()
                    .filter_map(|part| {
                        let bytes = book.read_part(&part)?;
                        let (w, h, rgba) = decode(&bytes)?;
                        let (w, h, rgba) = shrink(w, h, rgba);
                        Some(Decoded { part, w, h, rgba })
                    })
                    .collect();
                let _ = tx.send(out);
                waker.wake();
            })
            .expect("thread de imagens");
        self.images.rx = Some(rx);
        // Captura de tela de teste: espera as imagens.
        if self.opts.screenshot.is_some() {
            self.take_images(true);
        }
    }

    /// Manda as imagens decodificadas para a GPU. `true` = redesenhar.
    pub fn poll_images(&mut self) -> bool {
        self.take_images(false)
    }

    fn take_images(&mut self, block: bool) -> bool {
        let Some(rx) = &self.images.rx else { return false };
        let list = if block { rx.recv().ok() } else { rx.try_recv().ok() };
        let Some(list) = list else { return false };
        self.images.rx = None;
        let Some(gfx) = &self.gfx else { return false };
        let size = IMAGE_ATLAS as f32;
        for d in list {
            let (mut x, mut y, mut row) = self.images.shelf;
            if x + d.w > IMAGE_ATLAS {
                x = 0;
                y += row;
                row = 0;
            }
            if y + d.h > IMAGE_ATLAS {
                continue;
            }
            gfx.upload_image(x, y, d.w, d.h, &d.rgba);
            // Meio texel para dentro: o filtro não puxa o vizinho.
            let uv = [
                (x as f32 + 0.5) / size,
                (y as f32 + 0.5) / size,
                (x + d.w) as f32 / size - 0.5 / size,
                (y + d.h) as f32 / size - 0.5 / size,
            ];
            self.images.uv.insert(d.part, uv);
            self.images.shelf = (x + d.w, y, row.max(d.h));
        }
        true
    }
}
