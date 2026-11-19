//! Gerador de planilhas sintéticas com o perfil do arquivo de referência:
//! muitas linhas, ~37 colunas, mistura de números, datas, textos repetidos
//! (strings compartilhadas), fórmulas compartilhadas com valor em cache e
//! células estilizadas. Usado no CI e para medir sem depender de dados reais.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub struct Spec {
    pub rows: u32,
    pub cols: u32,
    /// Abas pequenas extras (como as abas de resumo de um painel real).
    pub small_sheets: u32,
}

const WORDS: [&str; 16] = [
    "Estoque",
    "Pedido",
    "Entrega",
    "Lombo",
    "Filé",
    "Costela",
    "Pernil",
    "Panceta",
    "Loja",
    "Gôndola",
    "Balcão",
    "Açougue",
    "Validade",
    "Preço",
    "Exposição",
    "Etiqueta",
];

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

const SHARED: u32 = 5000;

pub fn write(path: &Path, spec: &Spec) -> std::io::Result<()> {
    let file = BufWriter::with_capacity(1 << 20, File::create(path)?);
    let mut z = ZipWriter::new(file);
    let opt = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).large_file(true);
    let err = |e: zip::result::ZipError| std::io::Error::other(e.to_string());

    let sheets = 1 + spec.small_sheets;
    z.start_file("[Content_Types].xml", opt).map_err(err)?;
    let mut ct = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#,
    );
    for i in 1..=sheets {
        let _ = write!(
            ct,
            r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
        );
    }
    ct.push_str("</Types>");
    z.write_all(ct.as_bytes())?;

    z.start_file("_rels/.rels", opt).map_err(err)?;
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#)?;

    z.start_file("xl/workbook.xml", opt).map_err(err)?;
    let mut wb = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#,
    );
    for i in 1..=sheets {
        let name = if i == 1 { "BASE".to_string() } else { format!("RESUMO {i}") };
        let _ = write!(wb, r#"<sheet name="{name}" sheetId="{i}" r:id="rId{i}"/>"#);
    }
    wb.push_str("</sheets></workbook>");
    z.write_all(wb.as_bytes())?;

    z.start_file("xl/_rels/workbook.xml.rels", opt).map_err(err)?;
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for i in 1..=sheets {
        let _ = write!(
            rels,
            r#"<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>"#
        );
    }
    let _ = write!(
        rels,
        r#"<Relationship Id="rIdS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdT" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#
    );
    z.write_all(rels.as_bytes())?;

    z.start_file("xl/styles.xml", opt).map_err(err)?;
    z.write_all(br##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="#,##0.00"/></numFmts><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><color rgb="FFFFFFFF"/><name val="Calibri"/></font></fonts><fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FFC65911"/></patternFill></fill></fills><borders count="1"><border/></borders><cellXfs count="4"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="14" fontId="0" fillId="0" borderId="0" applyNumberFormat="1"/><xf numFmtId="164" fontId="0" fillId="0" borderId="0" applyNumberFormat="1"/><xf numFmtId="0" fontId="1" fillId="2" borderId="0"><alignment horizontal="center"/></xf></cellXfs></styleSheet>"##)?;

    z.start_file("xl/sharedStrings.xml", opt).map_err(err)?;
    let mut ss = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{SHARED}" uniqueCount="{SHARED}">"#
    );
    for i in 0..SHARED {
        let h = mix(i as u64);
        let a = WORDS[(h % 16) as usize];
        let b = WORDS[((h >> 8) % 16) as usize];
        let _ = write!(ss, "<si><t>{a} {b} &amp; {i}</t></si>");
    }
    ss.push_str("</sst>");
    z.write_all(ss.as_bytes())?;

    for i in 1..=sheets {
        z.start_file(format!("xl/worksheets/sheet{i}.xml"), opt).map_err(err)?;
        let rows = if i == 1 { spec.rows } else { 2000 };
        let cols = if i == 1 { spec.cols } else { 12 };
        write_sheet(&mut z, rows, cols)?;
    }
    z.finish().map_err(err)?;
    Ok(())
}

fn write_sheet(w: &mut impl Write, rows: u32, cols: u32) -> std::io::Result<()> {
    let names: Vec<String> = (0..cols).map(col_name).collect();
    let last = names.last().cloned().unwrap_or_default();
    write!(
        w,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:{last}{}"/><sheetData>"#,
        rows + 1
    )?;
    // Cabeçalho estilizado.
    let mut line = String::with_capacity(8192);
    line.push_str(r#"<row r="1">"#);
    for (c, n) in names.iter().enumerate() {
        let _ = write!(line, r#"<c r="{n}1" s="3" t="inlineStr"><is><t>Coluna {c}</t></is></c>"#);
    }
    line.push_str("</row>");
    w.write_all(line.as_bytes())?;

    for r in 2..=rows + 1 {
        line.clear();
        let _ = write!(line, r#"<row r="{r}" spans="1:{cols}">"#);
        for (c, n) in names.iter().enumerate() {
            let h = mix(((r as u64) << 16) | c as u64);
            match c % 8 {
                0 => {
                    let _ = write!(line, r#"<c r="{n}{r}" s="1"><v>{}</v></c>"#, 45000 + (h % 1000));
                }
                1..=3 => {
                    let _ = write!(line, r#"<c r="{n}{r}" t="s"><v>{}</v></c>"#, h % SHARED as u64);
                }
                4 => {
                    let _ =
                        write!(line, r#"<c r="{n}{r}" s="2"><v>{}</v></c>"#, (h % 10_000_000) as f64 / 100.0);
                }
                5 => {
                    // Fórmula compartilhada com valor em cache.
                    let prev = &names[c - 1];
                    let v = ((mix(((r as u64) << 16) | (c - 1) as u64) % 10_000_000) as f64 / 100.0) * 2.0;
                    if r == 2 {
                        let _ = write!(
                            line,
                            r#"<c r="{n}{r}" s="2"><f t="shared" ref="{n}2:{n}{}" si="{c}">{prev}2*2</f><v>{v}</v></c>"#,
                            rows + 1
                        );
                    } else {
                        let _ = write!(line, r#"<c r="{n}{r}" s="2"><f t="shared" si="{c}"/><v>{v}</v></c>"#);
                    }
                }
                6 => {
                    if !h.is_multiple_of(4) {
                        let _ = write!(line, r#"<c r="{n}{r}"><v>{}</v></c>"#, h % 1000);
                    }
                }
                _ => {
                    if h.is_multiple_of(3) {
                        let _ = write!(line, r#"<c r="{n}{r}" t="b"><v>{}</v></c>"#, h & 1);
                    } else {
                        let _ = write!(line, r#"<c r="{n}{r}" s="1"/>"#);
                    }
                }
            }
        }
        line.push_str("</row>");
        w.write_all(line.as_bytes())?;
    }
    w.write_all(b"</sheetData><mergeCells count=\"1\"><mergeCell ref=\"A1:B1\"/></mergeCells></worksheet>")?;
    Ok(())
}
