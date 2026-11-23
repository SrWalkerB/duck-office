//! Gerador de pastas binárias (`.xlsb`) sintéticas, com o mesmo perfil das
//! `.xlsx` de [`crate::generate`]: para medir e conferir sem dados reais.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::generate::{SHARED, Spec, WORDS, mix};

/// Um fluxo de registros (tipo, tamanho, dados).
#[derive(Default)]
struct Bin {
    out: Vec<u8>,
    /// Dados do registro em montagem.
    d: Vec<u8>,
}

impl Bin {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.d.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.d.extend(v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.d.extend(v.to_le_bytes());
        self
    }
    fn f64(&mut self, v: f64) -> &mut Self {
        self.d.extend(v.to_le_bytes());
        self
    }
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.d.resize(self.d.len() + n, 0);
        self
    }
    fn str(&mut self, s: &str) -> &mut Self {
        self.u32(s.encode_utf16().count() as u32);
        for u in s.encode_utf16() {
            self.u16(u);
        }
        self
    }
    fn range(&mut self, r0: u32, r1: u32, c0: u32, c1: u32) -> &mut Self {
        self.u32(r0).u32(r1).u32(c0).u32(c1)
    }
    /// Referência a uma célula (`PtgRef` ou `PtgRefN`).
    fn cell_ref(&mut self, ptg: u8, row: u32, col: u16) -> &mut Self {
        self.u8(ptg).u32(row).u16(col)
    }
    /// Fecha o registro `id` com os dados montados.
    fn end(&mut self, id: u16) -> &mut Self {
        if id < 0x80 {
            self.out.push(id as u8);
        } else {
            self.out.extend([(id & 0x7F) as u8 | 0x80, (id >> 7) as u8]);
        }
        let mut n = self.d.len();
        loop {
            self.out.push((n & 0x7F) as u8 | if n > 0x7F { 0x80 } else { 0 });
            n >>= 7;
            if n == 0 {
                break;
            }
        }
        self.out.append(&mut self.d);
        self
    }
}

/// Linha e coluna relativas.
const REL: u16 = 0xC000;

fn workbook(sheets: u32) -> Vec<u8> {
    let mut b = Bin::default();
    b.end(131).zeros(8).u32(0).end(153);
    b.zeros(24).u32(0).u8(0).end(158);
    b.end(143);
    for i in 1..=sheets {
        let name = if i == 1 { "BASE".to_string() } else { format!("RESUMO {i}") };
        b.u32(0).u32(i).str(&format!("rId{i}")).str(&name).end(156);
    }
    b.end(144).u32(191029).u32(1).u32(100).f64(0.001).u32(8).u16(0).end(157).end(132);
    b.out
}

fn styles() -> Vec<u8> {
    let mut b = Bin::default();
    b.u16(164).str("#,##0.00").end(44);
    for (weight, rgb) in [(400, [0, 0, 0]), (700, [255, 255, 255])] {
        b.u16(220).u16(0).u16(weight).u16(0).zeros(4).u8(5).u8(0).u16(0);
        b.d.extend(rgb);
        b.u8(255).u8(0).str("Calibri").end(43);
    }
    for (pattern, rgb) in [(0, [0; 3]), (17, [0; 3]), (1, [0xC6, 0x59, 0x11])] {
        b.u32(pattern).u8(5).u8(0).u16(0);
        b.d.extend(rgb);
        b.u8(255).u8(3).u8(64).zeros(6).zeros(48).end(45);
    }
    b.u8(0).zeros(50).end(46);
    b.u32(4).end(617);
    for (fmt, font, fill, align) in [(0, 0, 0, 0), (14, 0, 0, 0), (164, 0, 0, 0), (0, 1, 2, 2)] {
        // Embaixo, travada.
        b.u16(0).u16(fmt).u16(font).u16(fill).u16(0).zeros(2).u16(align | 2 << 3 | 0x1000).u16(0).end(47);
    }
    b.end(618);
    b.out
}

fn strings() -> Vec<u8> {
    let mut b = Bin::default();
    b.u32(SHARED).u32(SHARED).end(159);
    for i in 0..SHARED {
        let h = mix(i as u64);
        let (x, y) = (WORDS[(h % 16) as usize], WORDS[((h >> 8) % 16) as usize]);
        b.u8(0).str(&format!("{x} {y} & {i}")).end(19);
    }
    b.end(160);
    b.out
}

/// Como a aba do `.xlsx` sintético, coluna a coluna.
fn sheet(w: &mut impl Write, rows: u32, cols: u32) -> std::io::Result<()> {
    let mut b = Bin::default();
    b.end(129).range(0, rows, 0, cols - 1).end(148);
    b.u32(u32::MAX).u16(8).u16(300).u32(0).end(485).end(145);
    for r in 0..=rows {
        b.u32(r).u32(0).u16(300).zeros(3).u32(1).u32(0).u32(cols - 1).end(0);
        for c in 0..cols {
            if r == 0 {
                b.u32(c).u32(3).str(&format!("Coluna {c}")).end(6);
                continue;
            }
            let h = mix((((r + 1) as u64) << 16) | c as u64);
            let money = |c: u32| (mix((((r + 1) as u64) << 16) | c as u64) % 10_000_000) as f64 / 100.0;
            match c % 8 {
                0 => b.u32(c).u32(1).u32(((45000 + h % 1000) << 2) as u32 | 2).end(2),
                1..=3 => b.u32(c).u32(0).u32((h % SHARED as u64) as u32).end(7),
                4 => b.u32(c).u32(2).f64(money(c)).end(5),
                5 => {
                    // Compartilhada com valor em cache: a célula da esquerda × 2.
                    b.u32(c).u32(2).f64(money(c - 1) * 2.0).u16(0);
                    b.u32(5).u8(1).u32(1).u32(4).u32(c).end(9);
                    if r == 1 {
                        b.range(1, rows, c, c).u32(11).cell_ref(0x4C, 0, REL | 0x3FFF);
                        b.u8(0x1E).u16(2).u8(5).u32(0).end(427);
                    }
                    &mut b
                }
                6 if (r + 1) % 2 == 0 => {
                    // Comum, com referência relativa e absoluta.
                    b.u32(c).u32(0).f64((h % 1000) as f64).u16(0);
                    b.u32(15).cell_ref(0x44, r, REL | (c - 1) as u16).cell_ref(0x44, 0, 0);
                    b.u8(3).u32(0).end(9)
                }
                6 if !h.is_multiple_of(4) => b.u32(c).u32(0).u32(((h % 1000) << 2) as u32 | 2).end(2),
                6 => &mut b,
                _ if h.is_multiple_of(3) => b.u32(c).u32(0).u8((h & 1) as u8).end(4),
                _ => b.u32(c).u32(1).end(1),
            };
        }
        if b.out.len() > 1 << 20 {
            w.write_all(&b.out)?;
            b.out.clear();
        }
    }
    b.end(146).u32(1).end(177).range(0, 0, 0, 1).end(176).end(178).end(130);
    w.write_all(&b.out)
}

pub fn write(path: &Path, spec: &Spec) -> std::io::Result<()> {
    const CT: &str = "application/vnd.ms-excel.";
    const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let file = BufWriter::with_capacity(1 << 20, File::create(path)?);
    let mut z = ZipWriter::new(file);
    let opt = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).large_file(true);
    let err = |e: zip::result::ZipError| std::io::Error::other(e.to_string());
    let sheets = 1 + spec.small_sheets;

    let rels = |list: &[(String, &str, String)]| {
        let body: String = list
            .iter()
            .map(|(id, kind, target)| {
                format!(r#"<Relationship Id="{id}" Type="{REL_NS}/{kind}" Target="{target}"/>"#)
            })
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>"#
        )
    };
    let mut ct = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="bin" ContentType="{CT}sheet.binary.macroEnabled.main"/><Override PartName="/xl/styles.bin" ContentType="{CT}styles"/><Override PartName="/xl/sharedStrings.bin" ContentType="{CT}sharedStrings"/>"#
    );
    let mut list = Vec::new();
    for i in 1..=sheets {
        ct.push_str(&format!(
            r#"<Override PartName="/xl/worksheets/sheet{i}.bin" ContentType="{CT}worksheet"/>"#
        ));
        list.push((format!("rId{i}"), "worksheet", format!("worksheets/sheet{i}.bin")));
    }
    ct.push_str("</Types>");
    list.push(("rIdS".into(), "styles", "styles.bin".into()));
    list.push(("rIdT".into(), "sharedStrings", "sharedStrings.bin".into()));
    let root = rels(&[("rId1".to_string(), "officeDocument", "xl/workbook.bin".into())]);
    for (name, body) in [
        ("[Content_Types].xml", ct.into_bytes()),
        ("_rels/.rels", root.into_bytes()),
        ("xl/workbook.bin", workbook(sheets)),
        ("xl/_rels/workbook.bin.rels", rels(&list).into_bytes()),
        ("xl/styles.bin", styles()),
        ("xl/sharedStrings.bin", strings()),
    ] {
        z.start_file(name, opt).map_err(err)?;
        z.write_all(&body)?;
    }
    for i in 1..=sheets {
        z.start_file(format!("xl/worksheets/sheet{i}.bin"), opt).map_err(err)?;
        let (rows, cols) = if i == 1 { (spec.rows, spec.cols) } else { (2000, 12) };
        sheet(&mut z, rows, cols)?;
    }
    z.finish().map_err(err)?;
    Ok(())
}
