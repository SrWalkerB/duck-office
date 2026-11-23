//! Abas da vitrine com validação de dados, notas e proteção: "Regras" (uma
//! regra de cada tipo, notas e uma conversa) e "Protegida" (senha `pato`).
//! Tudo inventado.

use std::fmt::Write as _;

const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const THREADED: &str = "http://schemas.microsoft.com/office/spreadsheetml/2018/threadedcomments";

/// Estilos que estas abas usam, no fim do `cellXfs` da vitrine.
pub const XF_INPUT: u32 = 32;
pub const XF_HIDDEN: u32 = 33;
pub const XFS: &str = r#"<xf numFmtId="0" fontId="0" fillId="5" borderId="1" xfId="0" applyProtection="1"><protection locked="0"/></xf>
<xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0" applyProtection="1"><protection hidden="1"/></xf>
"#;

pub const NAMES: &str = r#"<definedNames><definedName name="Frutas">Regras!$E$4:$E$7</definedName><definedName name="Legumes">Regras!$F$4:$F$6</definedName></definedNames>"#;

/// (caminho, conteúdo) de todas as partes.
pub fn parts() -> Vec<(&'static str, String)> {
    vec![
        ("xl/worksheets/sheet5.xml", rules()),
        ("xl/worksheets/_rels/sheet5.xml.rels", rules_rels()),
        ("xl/comments1.xml", comments()),
        ("xl/drawings/vmlDrawing1.vml", vml()),
        ("xl/threadedComments/threadedComment1.xml", threads()),
        ("xl/persons/person.xml", persons()),
        ("xl/worksheets/sheet6.xml", protected()),
    ]
}

/// Tipos de conteúdo, abas e relacionamentos a acrescentar nas partes comuns.
pub fn content_types() -> String {
    let mut x = String::from(
        r#"<Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/>"#,
    );
    for (p, t) in [
        ("xl/worksheets/sheet5.xml", "openxmlformats-officedocument.spreadsheetml.worksheet"),
        ("xl/worksheets/sheet6.xml", "openxmlformats-officedocument.spreadsheetml.worksheet"),
        ("xl/comments1.xml", "openxmlformats-officedocument.spreadsheetml.comments"),
        ("xl/threadedComments/threadedComment1.xml", "ms-excel.threadedcomments"),
        ("xl/persons/person.xml", "ms-excel.person"),
    ] {
        let _ = write!(x, r#"<Override PartName="/{p}" ContentType="application/vnd.{t}+xml"/>"#);
    }
    x
}

pub const SHEETS: &str =
    r#"<sheet name="Regras" sheetId="5" r:id="rId7"/><sheet name="Protegida" sheetId="6" r:id="rId8"/>"#;

pub fn workbook_rels() -> String {
    format!(
        r#"<Relationship Id="rId7" Type="{REL}/worksheet" Target="worksheets/sheet5.xml"/><Relationship Id="rId8" Type="{REL}/worksheet" Target="worksheets/sheet6.xml"/><Relationship Id="rId9" Type="http://schemas.microsoft.com/office/2017/10/relationships/person" Target="persons/person.xml"/>"#
    )
}

fn text(r: &str, style: u32, t: &str) -> String {
    format!(r#"<c r="{r}" s="{style}" t="inlineStr"><is><t>{t}</t></is></c>"#)
}

fn num(r: &str, style: u32, v: f64) -> String {
    format!(r#"<c r="{r}" s="{style}"><v>{v}</v></c>"#)
}

/// (linha, o que a regra aceita, valor de exemplo).
const ROWS: [(u32, &str, &str); 14] = [
    (4, "Lista escrita na regra", "Sim"),
    (5, "Lista de um intervalo", "Banana"),
    (6, "Lista de outra aba (extensão x14)", ""),
    (7, "Lista de um nome definido", "Uva"),
    (8, "Categoria", "Legumes"),
    (9, "Lista que depende da categoria (INDIRETO)", "Batata"),
    (10, "Inteiro de 1 a 10", "7"),
    (11, "Decimal até 99,5 (aviso)", "12.5"),
    (12, "Data em 2026 (informação)", "46100"),
    (13, "Texto com até 5 letras", "curto"),
    (14, "Personalizada: sem repetir", "101"),
    (15, "Personalizada: sem repetir", "102"),
    (16, "Só a mensagem de entrada", ""),
    (17, "Lista com vírgula nos itens, sem seta", ""),
];

fn rules() -> String {
    let mut x = format!(
        r#"{HEAD}<worksheet xmlns="{MAIN}" xmlns:r="{REL}"><sheetPr><tabColor rgb="FFE37400"/></sheetPr><dimension ref="A1:H17"/><sheetViews><sheetView workbookViewId="0"><selection activeCell="B4" sqref="B4"/></sheetView></sheetViews><sheetFormatPr defaultRowHeight="15"/><cols><col min="1" max="1" width="40" customWidth="1"/><col min="2" max="2" width="22" customWidth="1"/></cols><sheetData><row r="1">{}</row><row r="3">{}{}{}{}</row>"#,
        text("A1", 12, "Validação de dados, notas e conversas"),
        text("A3", 2, "O que a célula aceita"),
        text("B3", 2, "Experimente"),
        text("E3", 2, "Frutas"),
        text("F3", 2, "Legumes"),
    );
    for (r, what, value) in ROWS {
        let _ = write!(x, r#"<row r="{r}">{}"#, text(&format!("A{r}"), 3, what));
        x.push_str(&match value.parse::<f64>() {
            Ok(v) => num(&format!("B{r}"), if r == 12 { 4 } else { 11 }, v),
            Err(_) if value.is_empty() => format!(r#"<c r="B{r}" s="11"/>"#),
            Err(_) => text(&format!("B{r}"), 11, value),
        });
        let k = (r - 4) as usize;
        if let Some(f) = ["Banana", "Maçã", "Uva", "Pera"].get(k) {
            x.push_str(&text(&format!("E{r}"), 3, f));
        }
        if let Some(f) = ["Batata", "Cenoura", "Chuchu"].get(k) {
            x.push_str(&text(&format!("F{r}"), 3, f));
        }
        x.push_str("</row>");
    }
    x.push_str("</sheetData>");
    let base = r#"allowBlank="1" showInputMessage="1" showErrorMessage="1""#;
    let rules = [
        format!(r#"type="list" {base} promptTitle="Resposta" prompt="Escolha na lista ou digite Sim, Não ou Talvez." sqref="B4"><formula1>"Sim,Não,Talvez"</formula1>"#),
        format!(r#"type="list" {base} sqref="B5"><formula1>$E$4:$E$7</formula1>"#),
        format!(r#"type="list" {base} sqref="B7"><formula1>Frutas</formula1>"#),
        format!(r#"type="list" {base} sqref="B8"><formula1>"Frutas,Legumes"</formula1>"#),
        format!(r#"type="list" {base} sqref="B9"><formula1>INDIRECT($B$8)</formula1>"#),
        format!(r#"type="whole" {base} errorTitle="Nota inválida" error="Digite um número inteiro de 1 a 10." sqref="B10"><formula1>1</formula1><formula2>10</formula2>"#),
        format!(r#"type="decimal" errorStyle="warning" operator="lessThanOrEqual" {base} error="O valor passa de 99,5." sqref="B11"><formula1>99.5</formula1>"#),
        format!(r#"type="date" errorStyle="information" {base} error="A data não é de 2026." sqref="B12"><formula1>DATE(2026,1,1)</formula1><formula2>DATE(2026,12,31)</formula2>"#),
        format!(r#"type="textLength" operator="lessThanOrEqual" {base} sqref="B13"><formula1>5</formula1>"#),
        format!(r#"type="custom" {base} error="Esse número já existe." sqref="B14:B15"><formula1>COUNTIF($B$14:$B$15,B14)=1</formula1>"#),
        r#"allowBlank="1" showInputMessage="1" promptTitle="Observação" prompt="Aqui vale qualquer texto." sqref="B16">"#.to_string(),
        format!(r#"type="list" showDropDown="1" {base} sqref="B17"><mc:AlternateContent xmlns:x12ac="http://schemas.microsoft.com/office/spreadsheetml/2011/1/ac" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="x12ac"><x12ac:list>"1,5","2,5"</x12ac:list></mc:Choice><mc:Fallback><formula1>"1,5,2,5"</formula1></mc:Fallback></mc:AlternateContent>"#),
    ];
    let _ = write!(x, r#"<dataValidations count="{}">"#, rules.len());
    for r in rules {
        let _ = write!(x, "<dataValidation {r}</dataValidation>");
    }
    let _ = write!(
        x,
        r#"</dataValidations><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><legacyDrawing r:id="rId2"/><extLst><ext uri="{{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:dataValidations xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main" count="1"><x14:dataValidation type="list" allowBlank="1" showInputMessage="1" showErrorMessage="1"><x14:formula1><xm:f>Vitrine!$A$3:$A$10</xm:f></x14:formula1><xm:sqref>B6</xm:sqref></x14:dataValidation></x14:dataValidations></ext></extLst></worksheet>"#
    );
    x
}

fn rules_rels() -> String {
    format!(
        r#"{HEAD}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/comments" Target="../comments1.xml"/><Relationship Id="rId2" Type="{REL}/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/><Relationship Id="rId3" Type="http://schemas.microsoft.com/office/2017/10/relationships/threadedComment" Target="../threadedComments/threadedComment1.xml"/></Relationships>"#
    )
}

const ROOT: &str = "{11111111-2222-4333-8444-555555555555}";

fn comments() -> String {
    format!(
        r#"{HEAD}<comments xmlns="{MAIN}" xmlns:xr="http://schemas.microsoft.com/office/spreadsheetml/2014/revision"><authors><author>Ana</author><author>tc={ROOT}</author></authors><commentList><comment ref="A3" authorId="0" shapeId="0"><text><r><rPr><b/><sz val="9"/><color indexed="81"/><rFont val="Tahoma"/><family val="2"/></rPr><t>Ana:</t></r><r><rPr><sz val="9"/><color indexed="81"/><rFont val="Tahoma"/><family val="2"/></rPr><t xml:space="preserve">
Cada linha tem uma regra diferente.</t></r></text></comment><comment ref="B10" authorId="1" shapeId="0" xr:uid="{ROOT}"><text><t>[Threaded comment] Your version of Excel allows you to read this threaded comment.</t></text></comment><comment ref="E3" authorId="0" shapeId="0"><text><r><rPr><sz val="9"/><color indexed="81"/><rFont val="Tahoma"/><family val="2"/></rPr><t>Esta nota fica sempre à vista.</t></r></text></comment></commentList></comments>"#
    )
}

fn note_shape(id: u32, row: u32, col: u32, anchor: &str, visible: bool) -> String {
    let vis = if visible { "visible" } else { "hidden" };
    let mark = if visible { "<x:Visible/>" } else { "" };
    format!(
        r##"<v:shape id="_x0000_s{id}" type="#_x0000_t202" style='position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;z-index:1;visibility:{vis}' fillcolor="#ffffe1" o:insetmode="auto"><v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/><v:path o:connecttype="none"/><v:textbox style='mso-direction-alt:auto'><div style='text-align:left'></div></v:textbox><x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>
    {anchor}</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>{row}</x:Row><x:Column>{col}</x:Column>{mark}</x:ClientData></v:shape>"##
    )
}

fn vml() -> String {
    format!(
        r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="5"/></o:shapelayout><v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype>{}{}{}</xml>"#,
        note_shape(5121, 2, 0, "1, 15, 1, 10, 3, 15, 5, 4", false),
        note_shape(5122, 9, 1, "2, 15, 8, 10, 4, 15, 12, 4", false),
        note_shape(5123, 2, 4, "6, 20, 7, 4, 8, 40, 10, 4", true),
    )
}

fn threads() -> String {
    format!(
        r#"{HEAD}<ThreadedComments xmlns="{THREADED}" xmlns:x="{MAIN}"><threadedComment ref="B10" dT="2026-11-20T09:15:00.00" personId="{{AAAAAAAA-0000-4000-8000-000000000001}}" id="{ROOT}"><text>Esta nota vai de 1 a 10 mesmo?</text></threadedComment><threadedComment ref="B10" dT="2026-11-20T10:02:00.00" personId="{{AAAAAAAA-0000-4000-8000-000000000002}}" id="{{11111111-2222-4333-8444-555555555556}}" parentId="{ROOT}"><text>Vai sim, só números inteiros.</text></threadedComment></ThreadedComments>"#
    )
}

fn persons() -> String {
    format!(
        r#"{HEAD}<personList xmlns="{THREADED}" xmlns:x="{MAIN}"><person displayName="Ana" id="{{AAAAAAAA-0000-4000-8000-000000000001}}" userId="ana" providerId="None"/><person displayName="Beto" id="{{AAAAAAAA-0000-4000-8000-000000000002}}" userId="beto" providerId="None"/></personList>"#
    )
}

/// Senha "pato": SHA-512 com 100 mil voltas, como o Excel grava.
const HASH: &str = "oKMJqivBE7WJQWmobdfk3KRhYvW/Ln3k3t8jMySACBX0Cip9MyFLw6fw4gbLLKfL+9UdOSBVJhUr2Zr5bmSXvg==";
const SALT: &str = "AAECAwQFBgcICQoLDA0ODw==";

fn protected() -> String {
    format!(
        r#"{HEAD}<worksheet xmlns="{MAIN}" xmlns:r="{REL}"><sheetPr><tabColor rgb="FF5F6368"/></sheetPr><dimension ref="A1:C7"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><cols><col min="1" max="1" width="34" customWidth="1"/><col min="2" max="3" width="16" customWidth="1"/></cols><sheetData><row r="1">{}</row><row r="2">{}</row><row r="4">{}{}</row><row r="5">{}{}</row><row r="6">{}<c r="B6" s="{XF_HIDDEN}"><f>B4*B5</f><v>250</v></c></row><row r="7">{}{}</row></sheetData><sheetProtection algorithmName="SHA-512" hashValue="{HASH}" saltValue="{SALT}" spinCount="100000" sheet="1" objects="1" scenarios="1" formatColumns="0"/><protectedRanges><protectedRange sqref="C4:C7" name="Observações"/></protectedRanges><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#,
        text("A1", 12, "Planilha protegida (a senha é pato)"),
        text("A2", 16, "As células amarelas e C4:C7 aceitam edição; larguras também."),
        text("A4", 23, "Quantidade"),
        num("B4", XF_INPUT, 10.0),
        text("A5", 23, "Preço"),
        num("B5", XF_INPUT, 25.0),
        text("A6", 23, "Total (fórmula oculta)"),
        text("A7", 23, "Bloqueada"),
        text("B7", 3, "não muda"),
    )
}
