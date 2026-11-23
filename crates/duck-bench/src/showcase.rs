//! Planilha de vitrine para conferir a fidelidade visual (F2) sem dados reais:
//! painéis congelados, mesclagens, bordas de todos os tipos, formatos de número
//! (moeda, contábil, %, datas, cores), fontes, preenchimentos, texto quebrado e
//! transbordando, abas coloridas e uma aba oculta. A aba "Recursos" tem texto
//! girado, estilo de linha inteira, texto rico, formatação condicional e
//! hiperlinks. Tudo inventado.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const STYLES: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<numFmts count="2"><numFmt numFmtId="164" formatCode="[Color10]#,##0.00;[Red]\-#,##0.00"/><numFmt numFmtId="165" formatCode="dd/mm/yyyy hh:mm"/></numFmts>
<fonts count="9">
<font><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/><scheme val="minor"/></font>
<font><b/><sz val="16"/><color rgb="FFFFFFFF"/><name val="Calibri"/><family val="2"/></font>
<font><b/><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/></font>
<font><i/><sz val="11"/><color theme="4"/><name val="Calibri"/><family val="2"/></font>
<font><u/><sz val="12"/><color rgb="FFC00000"/><name val="Arial"/><family val="2"/></font>
<font><sz val="10"/><color theme="1"/><name val="Courier New"/><family val="3"/></font>
<font><b/><sz val="11"/><color rgb="FF0B57D0"/><name val="Calibri"/><family val="2"/></font>
<font><u/><sz val="11"/><color theme="10"/><name val="Calibri"/><family val="2"/></font>
<font><strike/><sz val="11"/><color rgb="FF808080"/><name val="Calibri"/><family val="2"/></font>
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
<cellXfs count="31">
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
<xf numFmtId="0" fontId="2" fillId="3" borderId="1" xfId="0" applyAlignment="1"><alignment horizontal="center" vertical="center" textRotation="90"/></xf>
<xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0" applyAlignment="1"><alignment horizontal="center" vertical="center" textRotation="45"/></xf>
<xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0" applyAlignment="1"><alignment horizontal="center" vertical="center" textRotation="135"/></xf>
<xf numFmtId="0" fontId="2" fillId="0" borderId="1" xfId="0" applyAlignment="1"><alignment horizontal="center" vertical="top" textRotation="255"/></xf>
<xf numFmtId="0" fontId="0" fillId="4" borderId="0" xfId="0"/>
<xf numFmtId="0" fontId="7" fillId="0" borderId="0" xfId="0"/>
<xf numFmtId="0" fontId="8" fillId="0" borderId="0" xfId="0"/>
</cellXfs>
<dxfs count="3">
<dxf><font><color rgb="FF9C0006"/></font><fill><patternFill><bgColor rgb="FFFFC7CE"/></patternFill></fill></dxf>
<dxf><fill><patternFill><bgColor rgb="FFE2EFDA"/></patternFill></fill></dxf>
<dxf><font><b/><i/><color rgb="FF0B57D0"/></font><border><bottom style="medium"><color rgb="FF0B57D0"/></bottom></border></dxf>
</dxfs>
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

/// Marca de string com texto rico (o resto é o XML dos `<r>`).
const RICH: &str = "\u{1}";

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
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet3.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet4.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/tables/table1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/><Override PartName="/xl/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/><Override PartName="/xl/charts/chart2.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/></Types>"#,
    )?;
    put(
        &mut z,
        "_rels/.rels",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
    )?;
    put(
        &mut z,
        "xl/workbook.xml",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><bookViews><workbookView activeTab="0"/></bookViews><sheets><sheet name="Vitrine" sheetId="1" r:id="rId1"/><sheet name="Oculta" sheetId="2" state="hidden" r:id="rId2"/><sheet name="Resumo" sheetId="3" r:id="rId3"/><sheet name="Recursos" sheetId="4" r:id="rId6"/></sheets></workbook>"#,
    )?;
    put(
        &mut z,
        "xl/_rels/workbook.xml.rels",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet4.xml"/></Relationships>"#,
    )?;
    put(&mut z, "xl/styles.xml", STYLES)?;

    let mut st = Strings { list: Vec::new() };
    let s1 = vitrine(&mut st);
    let s2 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>42</v></c></row></sheetData></worksheet>"#;
    let s3 = resumo(&mut st);
    let s4 = recursos(&mut st);
    put(&mut z, "xl/worksheets/sheet1.xml", &s1)?;
    put(&mut z, "xl/worksheets/sheet2.xml", s2)?;
    put(&mut z, "xl/worksheets/sheet3.xml", &s3)?;
    put(&mut z, "xl/worksheets/sheet4.xml", &s4)?;
    put(
        &mut z,
        "xl/worksheets/_rels/sheet4.xml.rels",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#,
    )?;
    // Gráficos (F5): barras + linha no eixo secundário e rosca, da tabela de estoque.
    put(
        &mut z,
        "xl/drawings/drawing1.xml",
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:twoCellAnchor><xdr:from><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>20</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>11</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>36</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Gráfico 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor><xdr:twoCellAnchor><xdr:from><xdr:col>11</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>20</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>16</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>36</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="3" name="Gráfico 2"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId2"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"##,
    )?;
    put(
        &mut z,
        "xl/drawings/_rels/drawing1.xml.rels",
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart2.xml"/></Relationships>"##,
    )?;
    put(
        &mut z,
        "xl/charts/chart1.xml",
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Estoque e preço</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:autoTitleDeleted val="0"/><c:plotArea><c:layout/><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>Recursos!$C$21</c:f></c:strRef></c:tx><c:spPr><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></c:spPr><c:cat><c:strRef><c:f>Recursos!$A$22:$A$27</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Recursos!$C$22:$C$27</c:f></c:numRef></c:val></c:ser><c:dLbls><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="0"/><c:showBubbleSize val="0"/></c:dLbls><c:gapWidth val="80"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:lineChart><c:grouping val="standard"/><c:varyColors val="0"/><c:ser><c:idx val="1"/><c:order val="1"/><c:tx><c:strRef><c:f>Recursos!$D$21</c:f></c:strRef></c:tx><c:spPr><a:ln w="28575"><a:solidFill><a:schemeClr val="accent1"><a:lumMod val="75000"/></a:schemeClr></a:solidFill><a:prstDash val="dash"/></a:ln></c:spPr><c:marker><c:symbol val="square"/></c:marker><c:cat><c:strRef><c:f>Recursos!$A$22:$A$27</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Recursos!$D$22:$D$27</c:f></c:numRef></c:val></c:ser><c:marker val="1"/><c:axId val="3"/><c:axId val="4"/></c:lineChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:numFmt formatCode="General" sourceLinked="1"/><c:crossAx val="1"/></c:valAx><c:catAx><c:axId val="3"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="1"/><c:axPos val="b"/><c:crossAx val="4"/></c:catAx><c:valAx><c:axId val="4"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="r"/><c:numFmt formatCode="#,##0.00" sourceLinked="0"/><c:crossAx val="3"/><c:crosses val="max"/></c:valAx></c:plotArea><c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend><c:plotVisOnly val="1"/></c:chart></c:chartSpace>"##,
    )?;
    put(
        &mut z,
        "xl/charts/chart2.xml",
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Quantidade por produto</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:autoTitleDeleted val="0"/><c:plotArea><c:layout/><c:doughnutChart><c:varyColors val="1"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>Recursos!$C$21</c:f></c:strRef></c:tx><c:cat><c:strRef><c:f>Recursos!$A$22:$A$27</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Recursos!$C$22:$C$27</c:f></c:numRef></c:val></c:ser><c:dLbls><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="0"/><c:showBubbleSize val="0"/></c:dLbls><c:firstSliceAng val="0"/><c:holeSize val="55"/></c:doughnutChart></c:plotArea><c:legend><c:legendPos val="r"/><c:overlay val="0"/></c:legend><c:plotVisOnly val="1"/></c:chart></c:chartSpace>"##,
    )?;
    put(
        &mut z,
        "xl/tables/table1.xml",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Tabela1" displayName="Estoque" ref="A21:D27" totalsRowShown="0"><autoFilter ref="A21:D27"/><tableColumns count="4"><tableColumn id="1" name="Produto"/><tableColumn id="2" name="Categoria"/><tableColumn id="3" name="Qtde"/><tableColumn id="4" name="Preço"/></tableColumns><tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/></table>"#,
    )?;
    let mut ss = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{0}" uniqueCount="{0}">"#,
        st.list.len()
    );
    for s in &st.list {
        match s.strip_prefix(RICH) {
            // Texto rico: o XML dos trechos vai como está.
            Some(xml) => {
                let _ = write!(ss, "<si>{xml}</si>");
            }
            None => {
                let _ = write!(ss, "<si><t xml:space=\"preserve\">{}</t></si>", esc(s));
            }
        }
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

fn recursos(st: &mut Strings) -> String {
    let mut x = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetPr><tabColor rgb="FF7B1FA2"/></sheetPr><sheetFormatPr defaultRowHeight="15"/><cols><col min="1" max="1" width="22.7109375" customWidth="1"/><col min="2" max="7" width="13.7109375" customWidth="1"/></cols><sheetData>"#,
    );
    x.push_str(r#"<row r="1">"#);
    s(&mut x, st, "A1", 12, "Texto girado, texto rico, formatação condicional e links");
    x.push_str(r#"</row><row r="2" ht="90" customHeight="1">"#);
    s(&mut x, st, "A2", 24, "Girado 90°");
    s(&mut x, st, "B2", 25, "Girado 45°");
    s(&mut x, st, "C2", 26, "Girado -45°");
    s(&mut x, st, "D2", 27, "Pilha");
    x.push_str(r#"</row><row r="4" s="28" customFormat="1">"#);
    s(&mut x, st, "A4", 28, "Linha inteira com estilo (as vazias também)");
    x.push_str("</row>");
    let heads = ["Escala", "Barra", "Setas", "Maior que 5", "Linhas pares", "Repetidos", "3 maiores"];
    x.push_str(r#"<row r="5">"#);
    for (i, h) in heads.iter().enumerate() {
        s(&mut x, st, &format!("{}5", (b'A' + i as u8) as char), 2, h);
    }
    x.push_str("</row>");
    let names = ["Ana", "Bruno", "Carla", "Ana", "Davi", "Eva", "Bruno", "Ana", "Fábio", "Gil"];
    for k in 0..10u32 {
        let r = 6 + k;
        let v = ((k * 7) % 10 + 1) as f64;
        let _ = write!(x, r#"<row r="{r}">"#);
        for col in ["A", "B", "C", "D"] {
            n(&mut x, &format!("{col}{r}"), 3, v);
        }
        s(&mut x, st, &format!("E{r}"), 3, &format!("linha {r}"));
        s(&mut x, st, &format!("F{r}"), 3, names[k as usize]);
        n(&mut x, &format!("G{r}"), 3, v * 11.0);
        x.push_str("</row>");
    }
    x.push_str(r#"<row r="17" ht="48" customHeight="1">"#);
    let rich = format!(
        "{RICH}<r><t xml:space=\"preserve\">Texto rico: </t></r><r><rPr><b/><sz val=\"11\"/><color rgb=\"FFC00000\"/><rFont val=\"Calibri\"/></rPr><t xml:space=\"preserve\">negrito vermelho, </t></r><r><rPr><i/><sz val=\"14\"/><color theme=\"4\"/><rFont val=\"Calibri\"/></rPr><t xml:space=\"preserve\">itálico maior, </t></r><r><rPr><strike/><sz val=\"11\"/><rFont val=\"Calibri\"/></rPr><t xml:space=\"preserve\">tachado</t></r><r><rPr><sz val=\"11\"/><rFont val=\"Calibri\"/></rPr><t xml:space=\"preserve\"> e m</t></r><r><rPr><vertAlign val=\"superscript\"/><sz val=\"11\"/><rFont val=\"Calibri\"/></rPr><t>2</t></r>"
    );
    s(&mut x, st, "A17", 14, &rich);
    let rich2 = format!(
        "{RICH}<r><rPr><b/><sz val=\"11\"/><rFont val=\"Calibri\"/></rPr><t xml:space=\"preserve\">Numa linha: </t></r><r><rPr><sz val=\"11\"/><color rgb=\"FF188038\"/><rFont val=\"Courier New\"/></rPr><t>código</t></r>"
    );
    s(&mut x, st, "C17", 10, &rich2);
    x.push_str(r#"</row><row r="18">"#);
    s(&mut x, st, "A18", 29, "Ir para Vitrine!D5");
    s(&mut x, st, "C18", 29, "https://example.com/");
    x.push_str(r#"</row><row r="19">"#);
    s(&mut x, st, "A19", 30, "Fonte tachada");
    x.push_str("</row>");
    // Tabela do Excel com filtro (estilo Médio 2, faixas alternadas).
    x.push_str(r#"<row r="21">"#);
    for (col, h) in ["A", "B", "C", "D"].iter().zip(["Produto", "Categoria", "Qtde", "Preço"]) {
        s(&mut x, st, &format!("{col}21"), 0, h);
    }
    x.push_str("</row>");
    let itens = [
        ("Caneta", "Papelaria", 120.0, 2.5),
        ("Caderno", "Papelaria", 45.0, 18.9),
        ("Café", "Copa", 12.0, 24.0),
        ("Açúcar", "Copa", 8.0, 6.75),
        ("Grampeador", "Papelaria", 6.0, 32.0),
        ("Copo", "Copa", 300.0, 0.35),
    ];
    for (k, (p, c, q, v)) in itens.iter().enumerate() {
        let r = 22 + k;
        let _ = write!(x, r#"<row r="{r}">"#);
        s(&mut x, st, &format!("A{r}"), 0, p);
        s(&mut x, st, &format!("B{r}"), 0, c);
        n(&mut x, &format!("C{r}"), 0, *q);
        n(&mut x, &format!("D{r}"), 0, *v);
        x.push_str("</row>");
    }
    x.push_str("</sheetData>");
    x.push_str(r#"<conditionalFormatting sqref="A6:A15"><cfRule type="colorScale" priority="1"><colorScale><cfvo type="min"/><cfvo type="percentile" val="50"/><cfvo type="max"/><color rgb="FFF8696B"/><color rgb="FFFFEB84"/><color rgb="FF63BE7B"/></colorScale></cfRule></conditionalFormatting>"#);
    x.push_str(r#"<conditionalFormatting sqref="B6:B15"><cfRule type="dataBar" priority="2"><dataBar><cfvo type="min"/><cfvo type="max"/><color rgb="FF638EC6"/></dataBar></cfRule></conditionalFormatting>"#);
    x.push_str(r#"<conditionalFormatting sqref="C6:C15"><cfRule type="iconSet" priority="3"><iconSet iconSet="3Arrows"><cfvo type="percent" val="0"/><cfvo type="percent" val="33"/><cfvo type="percent" val="67"/></iconSet></cfRule></conditionalFormatting>"#);
    x.push_str(r#"<conditionalFormatting sqref="D6:D15"><cfRule type="cellIs" dxfId="0" priority="4" operator="greaterThan"><formula>5</formula></cfRule></conditionalFormatting>"#);
    x.push_str(r#"<conditionalFormatting sqref="E6:E15"><cfRule type="expression" dxfId="1" priority="5"><formula>MOD(ROW(),2)=0</formula></cfRule></conditionalFormatting>"#);
    x.push_str(r#"<conditionalFormatting sqref="F6:F15"><cfRule type="duplicateValues" dxfId="0" priority="6"/></conditionalFormatting>"#);
    x.push_str(r#"<conditionalFormatting sqref="G6:G15"><cfRule type="top10" dxfId="2" priority="7" rank="3"/></conditionalFormatting>"#);
    x.push_str(r#"<hyperlinks><hyperlink ref="A18" location="Vitrine!D5" display="Ir para Vitrine!D5"/><hyperlink ref="C18" r:id="rId1" tooltip="Site de exemplo"/></hyperlinks><drawing r:id="rId3"/><tableParts count="1"><tablePart r:id="rId2"/></tableParts></worksheet>"#);
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
