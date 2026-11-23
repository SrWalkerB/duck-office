//! `duck-bench emf ARQUIVO [--png PASTA]`: rasteriza cada imagem EMF do
//! arquivo com o `duck-emf` e mostra só métricas (tamanho, pixels
//! desenhados, tempo). `--png` grava as imagens para conferir a olho (use
//! uma pasta temporária: as imagens podem ter conteúdo da planilha).

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use crate::Res;

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

pub fn cmd_emf(args: &[String]) -> Res {
    let mut glyphs: Option<EmfText> = None;
    let path = crate::file_arg(args)?;
    let out = args.iter().position(|a| a == "--png").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let book = duck_xlsx::Book::open(&path).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    for i in 0..zip.len() {
        let f = zip.by_index_raw(i).map_err(|e| e.to_string())?;
        let n = f.name().map_err(|e| e.to_string())?.to_string();
        if n.to_lowercase().ends_with(".emf") {
            names.push(n);
        }
    }
    for (k, name) in names.iter().enumerate() {
        let Some(bytes) = book.read_part(name) else { continue };
        let t = Instant::now();
        let t_text = glyphs.get_or_insert_with(|| EmfText(duck_text::CpuGlyphs::new()));
        match duck_emf::render_with(&bytes, 1024, Some(t_text)) {
            Some(img) => {
                let drawn = img.rgba.chunks(4).filter(|p| p[3] > 0).count();
                let total = (img.w * img.h) as usize;
                println!(
                    "emf {k}: {} KB → {}×{}, {:.0}% desenhado, {:.1} ms",
                    bytes.len() / 1024,
                    img.w,
                    img.h,
                    drawn as f64 * 100.0 / total.max(1) as f64,
                    t.elapsed().as_secs_f64() * 1000.0
                );
                if let Some(dir) = &out {
                    let p = dir.join(format!("emf{k}.png"));
                    write_png(&p, img.w, img.h, &img.rgba).map_err(|e| e.to_string())?;
                }
            }
            None => println!("emf {k}: {} KB → não desenhado", bytes.len() / 1024),
        }
    }
    Ok(())
}

/// PNG RGBA simples (filtro 0, zlib).
fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = ty.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    fn crc32(b: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &x in b {
            c ^= x as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    let mut raw = Vec::with_capacity((w * h * 4 + h) as usize);
    for row in rgba.chunks((w * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&raw)?;
    let data = z.finish()?;
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &data);
    chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out)
}
