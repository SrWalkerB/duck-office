//! Planilha nova em branco (F3.5): o pacote mínimo que o Excel abre sem
//! reclamar (uma aba "Planilha1", estilo Calibri 11). O duck-sheets abre
//! esse arquivo como qualquer outro, e o "salvar" pede onde guardar.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use super::deflate::compress_chunk;
use super::zipw::{DEFLATED, ZipWriter};
use crate::Error;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;

const WORKBOOK: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><bookViews><workbookView activeTab="0"/></bookViews><sheets><sheet name="Planilha1" sheetId="1" r:id="rId1"/></sheets><calcPr calcId="191029"/></workbook>"#;

const WORKBOOK_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/><scheme val="minor"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#;

const SHEET: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><dimension ref="A1"/><sheetViews><sheetView tabSelected="1" workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><sheetData/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#;

/// Grava uma planilha em branco em `path`.
pub fn write_blank(path: &Path) -> Result<(), Error> {
    let file = File::create(path)?;
    let mut zip = ZipWriter::new(BufWriter::new(file));
    for (name, xml) in [
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", ROOT_RELS),
        ("xl/workbook.xml", WORKBOOK),
        ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS),
        ("xl/styles.xml", STYLES),
        ("xl/worksheets/sheet1.xml", SHEET),
    ] {
        let mut h = crc32fast::Hasher::new();
        h.update(xml.as_bytes());
        zip.copy_raw(
            name,
            DEFLATED,
            h.finalize(),
            xml.len() as u64,
            &compress_chunk(xml.as_bytes(), 6, true),
        )?;
    }
    let (w, len) = zip.finish()?;
    let f = w.into_inner().map_err(|e| Error::Io(e.into_error()))?;
    f.set_len(len)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_opens_and_saves() {
        let dir = std::env::temp_dir().join(format!("duck-blank-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("novo.xlsx");
        write_blank(&p).unwrap();
        let (mut wb, _) = crate::open(&p, &crate::Options::default()).unwrap();
        assert_eq!(wb.sheets.len(), 1);
        assert_eq!(wb.sheets[0].name, "Planilha1");
        wb.sheets[0].set_cell(0, 0, duck_core::CellValue::Text("olá".into()), None, None);
        let sheets: Vec<Option<&duck_core::Sheet>> = wb.sheets.iter().map(Some).collect();
        let q = dir.join("salvo.xlsx");
        crate::save_file(&p, &q, &sheets, &wb.shared_strings, &wb.styles, &crate::SaveOptions::default())
            .unwrap();
        let (back, _) = crate::open(&q, &crate::Options::default()).unwrap();
        assert_eq!(back.cell(0, 0, 0).value, duck_core::Value::Text("olá"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
