//! Strings compartilhadas (`sharedStrings.bin`).

use duck_core::{RichRun, RunFont, StringTable};
use rayon::prelude::*;

use super::rec::{self, Cur, Head, id, push_utf16};
use super::styles;
use crate::Error;
use crate::package::Package;

/// Lê a tabela. Como no XML, o fluxo é cortado em pedaços (aqui, em fronteiras
/// de registro) e cada um vira uma tabela numa thread.
pub fn read(pkg: &Package, real: &str) -> Result<StringTable, Error> {
    const PIECE: usize = 4 << 20;
    let buf = pkg.read_raw(real)?;
    let mut cuts = vec![0usize];
    let (mut pos, mut rich) = (0, false);
    while let Head::Record(kind, s, e) = rec::head(&buf, pos) {
        rich |= kind == id::SST_ITEM && buf.get(s).is_some_and(|f| f & 1 != 0);
        pos = e;
        if pos - cuts[cuts.len() - 1] >= PIECE {
            cuts.push(pos);
        }
    }
    cuts.push(pos);
    let fonts = if rich { run_fonts(pkg) } else { Vec::new() };
    let tables: Vec<StringTable> = cuts
        .par_windows(2)
        .map(|w| {
            let piece = &buf[w[0]..w[1]];
            let mut t = StringTable::with_capacity(piece.len() / 32, piece.len() / 2);
            let mut s = String::new();
            for (_, b) in rec::Records::new(piece).filter(|r| r.0 == id::SST_ITEM) {
                s.clear();
                let runs = rich_text(b, &fonts, &mut s);
                let i = t.push(&s);
                if let Some(runs) = runs.filter(|r| r.iter().any(|x| x.font.is_some())) {
                    t.set_rich(i, runs);
                }
            }
            t
        })
        .collect();
    let mut out = StringTable::new();
    for t in &tables {
        out.append(t);
    }
    Ok(out)
}

/// As fontes dos estilos, como fontes de trecho.
fn run_fonts(pkg: &Package) -> Vec<RunFont> {
    let Some(x) = pkg.xlsb() else { return Vec::new() };
    let Some(real) = x.parts.values().find(|p| p.1 == super::Kind::Styles).map(|p| p.0.as_str()) else {
        return Vec::new();
    };
    let Ok(bin) = pkg.read_raw(real) else { return Vec::new() };
    styles::run_fonts(&bin).iter().map(|x| crate::parts::run_font(x.as_bytes())).collect()
}

/// `RichStr`: marcas, texto e, se houver, os trechos com outra fonte.
pub fn rich_text(b: &[u8], fonts: &[RunFont], out: &mut String) -> Option<Vec<RichRun>> {
    let mut c = Cur::new(b);
    let flags = c.u8()?;
    let wide = c.wide()?;
    push_utf16(wide, out);
    if flags & 1 == 0 || fonts.is_empty() {
        return None;
    }
    let mut runs = Vec::new();
    // O começo, até o primeiro trecho, fica com a fonte da célula.
    let (mut unit, mut byte) = (0usize, 0usize);
    let mut chars = out.chars();
    for k in 0..c.u32()? {
        let (at, font) = (c.u16()? as usize, c.u16()? as usize);
        // Posição em unidades UTF-16 → em bytes.
        while unit < at {
            let Some(ch) = chars.next() else { break };
            unit += ch.len_utf16();
            byte += ch.len_utf8();
        }
        if k == 0 && at > 0 {
            runs.push(RichRun { start: 0, font: None });
        }
        runs.push(RichRun { start: byte as u32, font: fonts.get(font).cloned() });
    }
    Some(runs)
}

/// O `sharedStrings.xml` equivalente.
pub fn to_xml(t: &StringTable) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
    );
    for i in 0..t.len() as u32 {
        out.extend_from_slice(b"<si>");
        crate::write::cellxml::rich_xml(t.get(i).unwrap_or_default(), t.rich(i), &mut out);
        out.extend_from_slice(b"</si>");
    }
    out.extend_from_slice(b"</sst>");
    out
}
