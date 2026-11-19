//! Planilha de vitrine para conferir a fidelidade visual (F2) sem dados reais:
//! painéis congelados, mesclagens, bordas de todos os tipos, formatos de número
//! (moeda, contábil, %, datas, cores), fontes, preenchimentos, texto quebrado e
//! transbordando, abas coloridas e uma aba oculta. Tudo inventado.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const STYLES: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<numFmts count="2"><numFmt numFmtId="164" formatCode="[Color10]#,##0.00;[Red]\-#,##0.00"/><numFmt numFmtId="165" formatCode="dd/mm/yyyy hh:mm"/></numFmts>
<fonts count="7">
<font><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/><scheme val="minor"/></font>
<font><b/><sz val="16"/><color rgb="FFFFFFFF"/><name val="Calibri"/><family val="2"/></font>
<font><b/><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/></font>
<font><i/><sz val="11"/><color theme="4"/><name val="Calibri"/><family val="2"/></font>
<font><u/><sz val="12"/><color rgb="FFC00000"/><name val="Arial"/><family val="2"/></font>
<font><sz val="10"/><color theme="1"/><name val="Courier New"/><family val="3"/></font>
<font><b/><sz val="11"/><color rgb="FF0B57D0"/><name val="Calibri"/><family val="2"/></font>
</fonts>
<fills count="6">
<fill><patternFill patternType="none"/></fill>
<fill><patternFill patternType="gray125"/></fill>
<fill><patternFill patternType="solid"><fgColor theme="4" tint="-0.249977111117893"/><bgColor indexed="64"/></patternFill></fill>
<fill><patternFill patternType="solid"><fgColor theme="0" tint="-0.1499984740745262"/><bgColor indexed="64"/></patternFill></fill>
<fill><patternFill patternType="solid"><fgColor theme="9" tint="0.79998168889431442"/><bgColor indexed="64"/></patternFill></fill>
<fill><patternFill patternType="solid"><fgColor rgb="FFFFF2CC"/><bgColor indexed="64"/></patternFill></fill>
</fills>
<borders count="7">
<border><left/><right/><top/><bottom/><diagonal/></border>
<border><left style="thin"><color theme="0" tint="-0.34998626667073579"/></left><right style="thin"><color theme="0" tint="-0.34998626667073579"/></right><top style="thin"><color theme="0" tint="-0.34998626667073579"/></top><bottom style="thin"><color theme="0" tint="-0.34998626667073579"/></bottom><diagonal/></border>
<border><left/><right/><top/><bottom style="medium"><color indexed="64"/></bottom><diagonal/></border>
<border><left/><right/><top style="double"><color indexed="64"/></top><bottom style="thick"><color indexed="64"/></bottom><diagonal/></border>
<border><left style="dotted"><color indexed="64"/></left><right style="dotted"><color indexed="64"/></right><top style="dotted"><color indexed="64"/></top><bottom style="dotted"><color indexed="64"/></bottom><diagonal/></border>
<border><left style="dashed"><color rgb="FFC00000"/></left><right style="dashed"><color rgb="FFC00000"/></right><top style="dashed"><color rgb="FFC00000"/></top><bottom style="dashed"><color rgb="FFC00000"/></bottom><diagonal/></border>
<border><left style="thick"><color theme="4"/></left><right style="thick"><color theme="4"/></right><top style="thick"><color theme="4"/></top><bottom style="thick"><color theme="4"/></bottom><diagonal/></border>
</borders>
<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
<cellXfs count="24">
<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
<xf numFmtId="0" fontId="1" fillId="2" borderId="0" xfId="0" applyAlignment="1"><alignment horizontal="center" vertical="center"/></xf>
<xf numFmtId="0" fontId="2" fillId="3" borderId="2" xfId="0" applyAlignment="1"><alignment horizontal="center" vertical="center" wrapText="1"/></xf>
<xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0"/>
<xf numFmtId="14" fontId="0" fillId="0" borderId="1" xfId="0" applyAlignment="1"><alignment horizontal="center"/></xf>
<xf numFmtId="3" fontId="0" fillId="0" borderId="1" xfId="0"/>
<xf numFmtId="44" fontId="0" fillId="0" borderId="1" xfId="0"/>
<xf numFmtId="10" fontId="0" fillId="0" borderId="1" xfId="0"/>
<xf numFmtId="164" fontId="0" fillId="0" borderId="1" xfId="0"/>
<xf numFmtId="20" fontId="0" fillId="0" borderId="1" xfId="0"/>
<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
<xf numFmtId="0" fontId="0" fillId="4" borderId="1" xfId="0"/>
<xf numFmtId="0" fontId="2" fillId="0" borderId="3" xfId="0"/>
<xf numFmtId="4" fontId="2" fillId="0" borderId="3" xfId="0"/>
<xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0" applyAlignment="1"><alignment vertical="top" wrapText="1"/></xf>
<xf numFmtId="0" fontId="6" fillId="5" borderId="6" xfId="0" applyAlignment="1"><alignment horizontal="center" vertical="center" wrapText="1"/></xf>
<xf numFmtId="0" fontId="3" fillId="0" borderId="0" xfId="0"/>
<xf numFmtId="0" fontId="4" fillId="0" borderId="0" xfId="0"/>
<xf numFmtId="0" fontId="5" fillId="0" borderId="0" xfId="0"/>
<xf numFmtId="0" fontId="0" fillId="0" borderId="4" xfId="0" applyAlignment="1"><alignment horizontal="center"/></xf>
<xf numFmtId="0" fontId="0" fillId="0" borderId="5" xfId="0" applyAlignment="1"><alignment horizontal="center"/></xf>
<xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0"/>
<xf numFmtId="165" fontId="0" fillId="0" borderId="1" xfId="0"/>
<xf numFmtId="0" fontId="2" fillId="0" borderId="1" xfId="0" applyAlignment="1"><alignment horizontal="right" indent="1"/></xf>
</cellXfs>
</styleSheet>"##;

const STORES: [&str; 8] = [
    "Mercado Bom Preço",
    "Supermercado Estrela",
    "Atacado Central",
    "Empório da Esquina",
    "Hiper Família",
    "Mercadinho São João",
    "Rede Econômica",
    "Feira Livre Norte",
];

struct Strings {
    list: Vec<String>,
}

impl Strings {
    fn id(&mut self, s: &str) -> usize {
        match self.list.iter().position(|x| x == s) {
            Some(i) => i,
            None => {
                self.list.push(s.to_string());
                self.list.len() - 1
            }
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn write(path: &Path) -> std::io::Result<()> {
    let file = BufWriter::new(File::create(path)?);
    let mut z = ZipWriter::new(file);
    let opt = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let err = |e: zip::result::ZipError| std::io::Error::other(e.to_string());
    let put = |z: &mut ZipWriter<BufWriter<File>>, name: &str, body: &str| -> std::io::Result<()> {
        z.start_file(name, opt).map_err(err)?;
        z.write_all(body.as_bytes())
    };

    put(
        &mut z,
        "[Content_Types].xml",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet3.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
    )?;
    put(
        &mut z,
        "_rels/.rels",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
    )?;
    put(
        &mut z,
        "xl/workbook.xml",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><bookViews><workbookView activeTab="0"/></bookViews><sheets><sheet name="Vitrine" sheetId="1" r:id="rId1"/><sheet name="Oculta" sheetId="2" state="hidden" r:id="rId2"/><sheet name="Resumo" sheetId="3" r:id="rId3"/></sheets></workbook>"#,
    )?;
    put(
        &mut z,
        "xl/_rels/workbook.xml.rels",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#,
    )?;
    put(&mut z, "xl/styles.xml", STYLES)?;

    let mut st = Strings { list: Vec::new() };
    let s1 = vitrine(&mut st);
    let s2 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>42</v></c></row></sheetData></worksheet>"#;
    let s3 = resumo(&mut st);
    put(&mut z, "xl/worksheets/sheet1.xml", &s1)?;
    put(&mut z, "xl/worksheets/sheet2.xml", s2)?;
    put(&mut z, "xl/worksheets/sheet3.xml", &s3)?;
    let mut ss = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{0}" uniqueCount="{0}">"#,
        st.list.len()
    );
    for s in &st.list {
        let _ = write!(ss, "<si><t xml:space=\"preserve\">{}</t></si>", esc(s));
    }
    ss.push_str("</sst>");
    put(&mut z, "xl/sharedStrings.xml", &ss)?;
    z.finish().map_err(err)?;
    Ok(())
}

fn s(out: &mut String, st: &mut Strings, r: &str, style: u32, text: &str) {
    let _ = write!(out, r#"<c r="{r}" s="{style}" t="s"><v>{}</v></c>"#, st.id(text));
}

fn n(out: &mut String, r: &str, style: u32, v: f64) {
    let _ = write!(out, r#"<c r="{r}" s="{style}"><v>{v}</v></c>"#);
}

fn vitrine(st: &mut Strings) -> String {
    let mut x = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetPr><tabColor rgb="FF188038"/></sheetPr><dimension ref="A1:L40"/><sheetViews><sheetView tabSelected="1" workbookViewId="0"><pane xSplit="1" ySplit="2" topLeftCell="B3" activePane="bottomRight" state="frozen"/><selection pane="bottomRight" activeCell="D5" sqref="D5"/></sheetView></sheetViews><sheetFormatPr defaultRowHeight="15"/><cols><col min="1" max="1" width="24.7109375" customWidth="1"/><col min="2" max="2" width="12.7109375" customWidth="1"/><col min="3" max="3" width="10.7109375" customWidth="1"/><col min="4" max="4" width="16.7109375" customWidth="1"/><col min="5" max="5" width="10.7109375" customWidth="1"/><col min="6" max="6" width="13.7109375" customWidth="1"/><col min="7" max="7" width="9.140625" customWidth="1"/><col min="8" max="8" width="17.7109375" customWidth="1"/><col min="9" max="9" width="9.140625" hidden="1" customWidth="1"/><col min="10" max="10" width="8.7109375" customWidth="1"/></cols><sheetData>"#,
    );
    // Título mesclado e cabeçalho.
    x.push_str(r#"<row r="1" ht="30" customHeight="1">"#);
    s(&mut x, st, "A1", 1, "Relatório de vendas — exemplo fictício");
    for c in ["B", "C", "D", "E", "F", "G", "H"] {
        let _ = write!(x, r#"<c r="{c}1" s="1"/>"#);
    }
    x.push_str(r#"</row><row r="2" ht="32" customHeight="1">"#);
    let heads = ["Loja", "Data", "Qtde", "Valor (R$)", "Margem", "Variação", "Hora", "Data e hora"];
    for (i, h) in heads.iter().enumerate() {
        s(&mut x, st, &format!("{}2", (b'A' + i as u8) as char), 2, h);
    }
    x.push_str("</row>");
    let mut seed = 7u64;
    let mut rnd = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as f64 / (1u64 << 31) as f64
    };
    let mut total_qty = 0.0;
    let mut total_val = 0.0;
    for r in 3..=18 {
        let _ = write!(x, r#"<row r="{r}">"#);
        let zebra = r % 2 == 0;
        s(&mut x, st, &format!("A{r}"), if zebra { 11 } else { 3 }, STORES[(r as usize) % STORES.len()]);
        n(&mut x, &format!("B{r}"), 4, 46300.0 + r as f64);
        let q = (rnd() * 5000.0).round();
        total_qty += q;
        n(&mut x, &format!("C{r}"), 5, q);
        let v = (rnd() * 200000.0).round() / 100.0;
        total_val += v;
        n(&mut x, &format!("D{r}"), 6, if r == 7 { 0.0 } else { v });
        n(&mut x, &format!("E{r}"), 7, rnd() * 0.4);
        n(&mut x, &format!("F{r}"), 8, (rnd() - 0.5) * 2000.0);
        n(&mut x, &format!("G{r}"), 9, rnd());
        n(&mut x, &format!("H{r}"), 22, 46300.0 + r as f64 + rnd());
        if r == 4 {
            s(
                &mut x,
                st,
                &format!("J{r}"),
                10,
                "Este texto é longo e transborda para as células vazias à direita, como no Excel.",
            );
        }
        x.push_str("</row>");
    }
    // Totais.
    x.push_str(r#"<row r="19">"#);
    s(&mut x, st, "A19", 12, "Total");
    let _ = write!(x, r#"<c r="C19" s="13"><f>SUM(C3:C18)</f><v>{total_qty}</v></c>"#);
    let _ = write!(x, r#"<c r="D19" s="13"><f>SUM(D3:D18)</f><v>{total_val}</v></c>"#);
    for c in ["B", "E", "F", "G", "H"] {
        let _ = write!(x, r#"<c r="{c}19" s="12"/>"#);
    }
    x.push_str("</row>");
    // Bloco mesclado vertical, texto quebrado, fontes, bordas e erros.
    x.push_str(r#"<row r="21" ht="20" customHeight="1">"#);
    s(&mut x, st, "A21", 15, "Bloco mesclado (A21:A25)");
    s(&mut x, st, "B21", 14, "Texto quebrado em várias linhas dentro da célula, alinhado no topo.");
    s(&mut x, st, "D21", 16, "Itálico na cor do tema");
    x.push_str("</row>");
    for r in 22..=25 {
        let _ = write!(x, r#"<row r="{r}"><c r="A{r}" s="15"/>"#);
        match r {
            22 => s(&mut x, st, "D22", 17, "Arial sublinhado"),
            23 => s(&mut x, st, "D23", 18, "Courier New 10"),
            24 => {
                s(&mut x, st, "D24", 19, "pontilhada");
                s(&mut x, st, "F24", 20, "tracejada");
            }
            _ => {
                x.push_str(r#"<c r="D25" s="23" t="b"><v>1</v></c><c r="E25" s="23" t="e"><v>#DIV/0!</v></c><c r="F25" s="23" t="e"><v>#N/A</v></c>"#);
                n(&mut x, "G25", 21, 1.0 / 3.0);
                n(&mut x, "H25", 13, 123456789012.0);
            }
        }
        x.push_str("</row>");
    }
    x.push_str("</sheetData><mergeCells count=\"2\"><mergeCell ref=\"A1:H1\"/><mergeCell ref=\"A21:A25\"/></mergeCells></worksheet>");
    x
}

fn resumo(st: &mut Strings) -> String {
    let mut x = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetPr><tabColor rgb="FFE8710A"/></sheetPr><sheetViews><sheetView showGridLines="0" zoomScale="125" workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><cols><col min="1" max="1" width="30.7109375" customWidth="1"/><col min="2" max="2" width="16.7109375" customWidth="1"/></cols><sheetData>"#,
    );
    x.push_str(r#"<row r="2">"#);
    s(&mut x, st, "A2", 12, "Resumo sem linhas de grade");
    x.push_str(r#"</row><row r="3">"#);
    s(&mut x, st, "A3", 3, "Faturamento");
    n(&mut x, "B3", 6, 1_234_567.89);
    x.push_str(r#"</row><row r="4">"#);
    s(&mut x, st, "A4", 3, "Margem média");
    n(&mut x, "B4", 7, 0.1834);
    x.push_str("</row></sheetData></worksheet>");
    x
}
