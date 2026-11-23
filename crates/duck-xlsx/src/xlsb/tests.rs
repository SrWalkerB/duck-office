//! Pastas binárias sintéticas, montadas registro a registro.

use std::io::Write;
use std::path::{Path, PathBuf};

use duck_core::{CellError, CellRange, FormulaKind, HAlign, Sheet, Value, Workbook};

use super::rec::id;
use crate::{Options, SaveOptions};

// ---------------------------------------------------------------------------
// Montagem
// ---------------------------------------------------------------------------

/// Dados de um registro.
#[derive(Default, Clone)]
struct D(Vec<u8>);

fn d() -> D {
    D::default()
}

impl D {
    fn b(mut self, b: &[u8]) -> Self {
        self.0.extend_from_slice(b);
        self
    }
    fn u8(self, v: u8) -> Self {
        self.b(&[v])
    }
    fn u16(self, v: u16) -> Self {
        self.b(&v.to_le_bytes())
    }
    fn u32(self, v: u32) -> Self {
        self.b(&v.to_le_bytes())
    }
    fn f64(self, v: f64) -> Self {
        self.b(&v.to_le_bytes())
    }
    fn zeros(self, n: usize) -> Self {
        self.b(&vec![0; n])
    }
    /// `XLWideString`.
    fn s(self, s: &str) -> Self {
        let u: Vec<u16> = s.encode_utf16().collect();
        let b: Vec<u8> = u.iter().flat_map(|c| c.to_le_bytes()).collect();
        self.u32(u.len() as u32).b(&b)
    }
    fn null(self) -> Self {
        self.u32(u32::MAX)
    }
    fn range(self, r0: u32, r1: u32, c0: u32, c1: u32) -> Self {
        self.u32(r0).u32(r1).u32(c0).u32(c1)
    }
    /// Coluna e estilo de uma célula.
    fn cell(self, col: u32, style: u32) -> Self {
        self.u32(col).u32(style)
    }
    /// Fórmula: tamanho, tokens, tamanho dos extras, extras.
    fn f(self, rgce: &[u8], rgcb: &[u8]) -> Self {
        self.u32(rgce.len() as u32).b(rgce).u32(rgcb.len() as u32).b(rgcb)
    }
    fn color(self, kind: u8, index: u8, rgb: [u8; 3]) -> Self {
        self.u8(kind << 1 | 1).u8(index).u16(0).b(&rgb).u8(255)
    }
}

/// Um fluxo de registros.
#[derive(Default, Clone)]
struct Bin(Vec<u8>);

impl Bin {
    fn rec(&mut self, id: u16, data: D) -> &mut Self {
        if id < 0x80 {
            self.0.push(id as u8);
        } else {
            self.0.extend([(id & 0x7F) as u8 | 0x80, (id >> 7) as u8]);
        }
        let mut n = data.0.len();
        loop {
            let more = n > 0x7F;
            self.0.push((n & 0x7F) as u8 | if more { 0x80 } else { 0 });
            n >>= 7;
            if !more {
                break;
            }
        }
        self.0.extend(data.0);
        self
    }

    fn row(&mut self, row: u32, height: u16, bits: u8, style: u32) -> &mut Self {
        self.rec(id::ROW, d().u32(row).u32(style).u16(height).u8(0).u8(bits).u8(0).u32(0))
    }
}

const REL: u16 = 0xC000;

fn ptg_ref(row: u32, col: u16) -> Vec<u8> {
    d().u8(0x44).u32(row).u16(col).0
}

fn ptg_exp(row: u32, col: u32) -> (Vec<u8>, Vec<u8>) {
    (d().u8(1).u32(row).0, d().u32(col).0)
}

fn rels(list: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut s = String::from(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, kind, target) in list {
        let ext = if target.contains("://") { " TargetMode=\"External\"" } else { "" };
        s.push_str(&format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"{ext}/>"#
        ));
    }
    s.push_str("</Relationships>");
    s.into_bytes()
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("duck-xlsb-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn package(path: &Path, parts: &[(&str, Vec<u8>)]) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let ct = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/></Types>"#;
    let root = rels(&[("rId1", "officeDocument", "xl/workbook.bin")]);
    let fixed = [("[Content_Types].xml", ct.as_bytes().to_vec()), ("_rels/.rels", root)];
    for (name, body) in fixed.iter().map(|p| (p.0, &p.1)).chain(parts.iter().map(|p| (p.0, &p.1))) {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
}

fn styles() -> Vec<u8> {
    let mut b = Bin::default();
    let font = |bold: bool, italic: bool, color: D, name: &str| {
        let d0 = d().u16(11 * 20).u16(if italic { 2 } else { 0 }).u16(if bold { 700 } else { 400 }).u16(0);
        d0.u8(0).u8(2).u8(0).u8(0).b(&color.0).u8(0).s(name)
    };
    let black = d().color(3, 1, [0; 3]);
    let red = d().color(2, 0, [0xFF, 0, 0]);
    b.rec(id::FMT, d().u16(164).s("0.00\" kg\""));
    b.rec(id::FONT, font(false, false, black.clone(), "Calibri"));
    b.rec(id::FONT, font(true, true, red.clone(), "Arial"));
    let fill = |p: u32, fg: D, bg: D| d().u32(p).b(&fg.0).b(&bg.0).zeros(48);
    let auto = || d().color(1, 64, [0; 3]);
    b.rec(id::FILL, fill(0, auto(), auto()));
    b.rec(id::FILL, fill(17, auto(), auto()));
    b.rec(id::FILL, fill(1, d().color(2, 0, [1, 2, 3]), auto()));
    let side = |style: u16, c: D| d().u16(style).b(&c.0);
    let none = side(0, d().zeros(8));
    b.rec(id::BORDER, d().u8(0).b(&none.0.repeat(5)));
    // Em cima fina, embaixo dupla vermelha.
    b.rec(id::BORDER, d().u8(0).b(&side(1, auto()).0).b(&side(6, red.clone()).0).b(&none.0.repeat(3)));
    let xf = |fmt: u16, font: u16, fill: u16, border: u16, rot: u8, indent: u8, bits: u16| {
        d().u16(0).u16(fmt).u16(font).u16(fill).u16(border).u8(rot).u8(indent).u16(bits).u16(0)
    };
    // Embaixo (2), travada.
    const BASE: u16 = 2 << 3 | 0x1000;
    b.rec(id::BEGIN_STYLE_XFS, d().u32(1)).rec(id::XF, xf(0, 0, 0, 0, 0, 0, BASE));
    b.rec(id::END_STYLE_XFS, d()).rec(id::BEGIN_CELL_XFS, d().u32(3));
    b.rec(id::XF, xf(0, 0, 0, 0, 0, 0, BASE));
    b.rec(id::XF, xf(164, 1, 2, 1, 90, 1, 2 | 1 << 3 | 0x40 | 0x1000));
    b.rec(id::XF, xf(14, 0, 0, 0, 0, 0, BASE));
    b.rec(id::END_CELL_XFS, d());
    b.rec(id::STYLE, d().u32(0).u16(1).u8(0).u8(0).s("Normal"));
    // Formato diferencial: negrito, cor e fundo.
    let prop = |kind: u16, body: D| d().u16(kind).u16(body.0.len() as u16 + 4).b(&body.0).0;
    let props = [prop(0x19, d().u16(700)), prop(5, red.clone()), prop(2, d().color(2, 0, [9, 8, 7]))];
    b.rec(id::BEGIN_DXFS, d().u32(1));
    b.rec(id::DXF, d().u16(0x8000).u16(0).u16(3).b(&props.concat()));
    b.rec(id::END_DXFS, d());
    b.0
}

fn strings() -> Vec<u8> {
    let mut b = Bin::default();
    b.rec(159, d().u32(3).u32(3));
    for s in ["Loja", "Ação 🦆"] {
        b.rec(id::SST_ITEM, d().u8(0).s(s));
    }
    // "ação AZUL" com a fonte 1 a partir da quinta unidade.
    b.rec(id::SST_ITEM, d().u8(1).s("ação AZUL").u32(1).u16(5).u16(1));
    b.rec(160, d());
    b.0
}

/// `Aba1` (tudo), `Minha aba` (escondida) e uma aba de gráfico no meio.
fn workbook() -> Vec<u8> {
    let mut b = Bin::default();
    b.rec(id::WB_PROP, d().u32(0).u32(0).s("EstaPasta"));
    b.rec(id::BOOK_VIEW, d().zeros(24).u32(0).u8(0));
    let tab = |state: u32, n: u32, rid: &str, name: &str| d().u32(state).u32(n).s(rid).s(name);
    b.rec(143, d());
    b.rec(id::BUNDLE_SH, tab(0, 1, "rId1", "Aba1"));
    b.rec(id::BUNDLE_SH, tab(0, 2, "rId7", "Gráf"));
    b.rec(id::BUNDLE_SH, tab(1, 3, "rId2", "Minha aba"));
    b.rec(144, d());
    b.rec(id::SUP_SELF, d());
    b.rec(id::SUP_BOOK_SRC, d().s("rId5"));
    // ixti: 0 = Aba1, 1 = Minha aba, 2 = Aba1:Minha aba, 3 = externo, 4 = excluída, 5 = o arquivo.
    let xti = |s: u32, a: i32, z: i32| d().u32(s).u32(a as u32).u32(z as u32).0;
    let list =
        [xti(0, 0, 0), xti(0, 2, 2), xti(0, 0, 2), xti(1, 0, 0), xti(0, -1, -1), xti(0, -2, -2)].concat();
    b.rec(id::EXTERN_SHEET, d().u32(6).b(&list));
    let name = |bits: u32, tab: u32, n: &str, f: &[u8]| d().u32(bits).u8(0).u32(tab).s(n).f(f, &[]).null();
    // Taxa = Aba1!$A$1; _xlfn.IFS (função); área de impressão da última aba.
    b.rec(id::NAME, name(0, u32::MAX, "Taxa", &d().u8(0x3A).u16(0).u32(0).u16(0).0));
    b.rec(id::NAME, name(3, u32::MAX, "_xlfn.IFS", &[0x1C, 0x1D]));
    let area = d().u8(0x3B).u16(1).u32(0).u32(9).u16(0).u16(2).0;
    b.rec(id::NAME, name(0x20, 2, "Print_Area", &area));
    b.rec(id::CALC_PROP, d().u32(191029).u32(1).u32(100).f64(0.001).u32(8).u16(0));
    b.0
}

fn table() -> Vec<u8> {
    let mut b = Bin::default();
    let head = d().range(1, 4, 6, 7).u32(0).u32(7).u32(1).u32(0).u32(0).zeros(28);
    b.rec(id::BEGIN_LIST, head.s("Vendas").s("Vendas").null().null().null().null());
    b.rec(id::BEGIN_AFILTER, d().range(1, 4, 6, 7)).rec(id::END_AFILTER, d());
    b.rec(345, d().u32(2));
    for (n, name) in [(1, "Loja"), (2, "Valor R$")] {
        b.rec(id::BEGIN_LIST_COL, d().u32(n).zeros(20).null().s(name).null().null().null().null());
        b.rec(id::END_LIST_COL, d());
    }
    b.rec(346, d());
    b.rec(id::TABLE_STYLE_CLIENT, d().u16(4).s("TableStyleMedium2"));
    b.rec(344, d());
    b.0
}

fn external() -> Vec<u8> {
    let mut b = Bin::default();
    b.rec(id::BEGIN_SUP_BOOK, d().u16(0).s("rId1").null());
    b.rec(id::SUP_TABS, d().u32(1).s("Base X"));
    b.rec(id::EXTERN_TABLE_START, d().u32(0).u8(0));
    b.rec(id::EXTERN_ROW, d().u32(2));
    b.rec(id::EXTERN_REAL, d().u32(0).f64(42.5));
    b.rec(id::EXTERN_STRING, d().u32(1).s("fora"));
    b.rec(id::EXTERN_TABLE_END, d());
    b.rec(588, d());
    b.0
}

/// A aba principal.
fn sheet1() -> Vec<u8> {
    let mut b = Bin::default();
    b.rec(129, d());
    let prop = d().u16(0xC0).u8(0).color(2, 0, [0x10, 0x20, 0x30]).u32(u32::MAX).u32(u32::MAX).s("Plan1");
    b.rec(id::WS_PROP, prop);
    b.rec(id::WS_DIM, d().range(0, 11, 0, 9));
    b.rec(133, d());
    // Sem linhas de grade, zoom 80.
    b.rec(id::BEGIN_WS_VIEW, d().u16(0x3D8).u32(0).u32(0).u32(0).u32(0).u16(80).u16(100).u32(0).u32(0));
    b.rec(id::PANE, d().f64(1.0).f64(2.0).u32(2).u32(1).u32(0).u8(3));
    b.rec(id::SEL, d().u32(0).u32(3).u32(2).u32(0).u32(1).range(3, 3, 2, 2));
    b.rec(id::END_WS_VIEW, d()).rec(134, d());
    b.rec(id::WS_FMT_INFO, d().u32(u32::MAX).u16(8).u16(300).u16(0).u8(1).u8(0));
    b.rec(390, d());
    b.rec(id::COL_INFO, d().u32(0).u32(0).u32(20 * 256 + 128).u32(0).u16(2));
    b.rec(id::COL_INFO, d().u32(2).u32(3).u32(5 * 256).u32(1).u16(1 | 2 << 8));
    b.rec(391, d());
    b.rec(id::BEGIN_SHEET_DATA, d());

    // Linha 1: um valor de cada tipo.
    b.row(0, 300, 0, 0);
    b.rec(id::CELL_RK, d().cell(0, 0).u32((250 << 2) | 3)); // 2,5
    b.rec(id::CELL_REAL, d().cell(1, 1).f64(std::f64::consts::PI));
    b.rec(id::CELL_ISST, d().cell(2, 0).u32(1));
    b.rec(id::CELL_ST, d().cell(3, 0).s("direto\nna célula"));
    b.rec(id::CELL_BOOL, d().cell(4, 0).u8(1));
    b.rec(id::CELL_ERROR, d().cell(5, 0).u8(0x07));
    b.rec(id::CELL_BLANK, d().cell(6, 1));
    b.rec(id::CELL_ISST, d().cell(7, 0).u32(2));
    b.rec(id::CELL_RSTRING, d().cell(8, 0).u8(0).s("rico"));
    // Linha 2: altura própria, escondida, estilo de linha e tópico.
    b.row(1, 600, 0x20 | 0x10 | 0x40 | 1, 1);
    b.rec(id::CELL_ISST, d().cell(6, 0).u32(0));
    b.rec(id::CELL_ST, d().cell(7, 0).s("Valor R$"));
    // Linhas 3 a 5: A = 2, 3, 4; B = fórmula compartilhada A*$A$1; tabela em G:H.
    for r in 2..5u32 {
        b.row(r, 300, 0, 0);
        b.rec(id::CELL_RK, d().cell(0, 0).u32((r << 2) | 2));
        let (e, x) = ptg_exp(2, 1);
        b.rec(id::FMLA_NUM, d().cell(1, 0).f64(r as f64 * 2.5).u16(0).f(&e, &x));
        if r == 2 {
            let body = [d().u8(0x4C).u32(0).u16(REL | 0x3FFF).0, ptg_ref(0, 0), vec![0x05]].concat();
            b.rec(id::SHR_FMLA, d().range(2, 4, 1, 1).f(&body, &[]));
        }
        // Fórmula comum, igual em R1C1: A&"x".
        let f = [ptg_ref(r, REL), d().u8(0x17).u16(1).b(b"x\0").u8(8).0].concat();
        b.rec(id::FMLA_STRING, d().cell(2, 0).s(&format!("{r}x")).u16(0).f(&f, &[]));
        b.rec(id::CELL_ST, d().cell(6, 0).s("loja"));
        b.rec(id::CELL_RK, d().cell(7, 0).u32((10 * r) << 2 | 2));
    }
    // Linha 6: matriz dinâmica em A6:A8 (=A3:A5*2), resultado espalhado.
    b.row(5, 300, 0, 0);
    let (e, x) = ptg_exp(5, 0);
    b.rec(id::CELL_META, d().u32(1));
    b.rec(id::FMLA_NUM, d().cell(0, 0).f64(4.0).u16(0).f(&e, &x));
    let arr = [d().u8(0x65).u32(2).u32(4).u16(REL).u16(REL).0, vec![0x1E, 2, 0, 5]].concat();
    b.rec(id::ARR_FMLA, d().range(5, 7, 0, 0).u8(0).f(&arr, &[]));
    // B6 = SUM(Vendas[Valor R$]) por atributo; C6 = _xlfn.IFS(TRUE,1); D6 = fora.
    let list = d().b(&[0x18, 0x19]).u16(1).u16(1).u32(7).u16(1).u16(1).b(&[0x19, 0x10, 0, 0]).0;
    b.rec(id::FMLA_NUM, d().cell(1, 0).f64(90.0).u16(0).f(&list, &[]));
    let ifs = [d().u8(0x23).u32(2).b(&[0x1D, 1, 0x1E, 1, 0]).0, vec![0x42, 3, 0xFF, 0]].concat();
    b.rec(id::FMLA_NUM, d().cell(2, 0).f64(1.0).u16(0).f(&ifs, &[]));
    let ext = d().u8(0x5A).u16(3).u32(2).u16(0).0;
    b.rec(id::FMLA_NUM, d().cell(3, 0).f64(42.5).u16(0).f(&ext, &[]));
    // E6 = Taxa; F6 = 'Minha aba'!A1; erro de fórmula em G6; token desconhecido em H6.
    b.rec(id::FMLA_NUM, d().cell(4, 0).f64(2.5).u16(0).f(&d().u8(0x43).u32(1).0, &[]));
    let other = d().u8(0x5A).u16(1).u32(0).u16(REL).0;
    b.rec(id::FMLA_BOOL, d().cell(5, 0).u8(1).u16(0).f(&other, &[]));
    let gone = d().u8(0x5A).u16(4).u32(0).u16(0).0;
    b.rec(id::FMLA_ERROR, d().cell(6, 0).u8(0x17).u16(0).f(&gone, &[]));
    b.rec(id::FMLA_NUM, d().cell(7, 0).f64(7.0).u16(0).f(&[0x1A, 1], &[]));
    for r in 6..8u32 {
        b.row(r, 300, 0, 0);
        b.rec(id::FMLA_NUM, d().cell(0, 0).f64((r - 3) as f64 * 2.0).u16(0).f(&e, &x));
    }
    // Linha 9: células curtas (sem coluna) depois de D9.
    b.row(8, 300, 0, 0);
    b.rec(id::CELL_RK, d().cell(3, 0).u32(1 << 2 | 2));
    b.rec(13, d().u32(0).u32(2 << 2 | 2));
    b.rec(16, d().u32(1).f64(3.5));
    b.rec(id::END_SHEET_DATA, d());

    b.rec(id::BEGIN_AFILTER, d().range(8, 11, 3, 5));
    b.rec(id::BEGIN_FILTER_COLUMN, d().u32(1).u16(0)).rec(id::BEGIN_FILTERS, d().u32(1).u32(0));
    b.rec(id::FILTER, d().s("2")).rec(id::END_FILTERS, d()).rec(id::END_FILTER_COLUMN, d());
    b.rec(id::END_AFILTER, d());
    b.rec(177, d().u32(1)).rec(id::MERGE_CELL, d().range(9, 10, 0, 2)).rec(178, d());
    // Formatação condicional: B3:B5 > $A$1, e escala de cores em A3:A5.
    let rule = |kind: u32, sub: u32, dxf: u32, pri: u32, param: u32, n: u32| {
        d().u32(kind).u32(sub).u32(dxf).u32(pri).u32(param).zeros(8).u16(2).u32(n).u32(0).u32(0).null()
    };
    b.rec(id::BEGIN_CF, d().u32(1).u32(0).u32(1).range(2, 4, 1, 1));
    b.rec(id::BEGIN_CF_RULE, rule(1, 0, 0, 1, 5, 7).f(&ptg_ref(0, 0), &[]));
    b.rec(id::END_CF_RULE, d()).rec(id::END_CF, d());
    b.rec(id::BEGIN_CF, d().u32(1).u32(0).u32(1).range(2, 4, 0, 0));
    b.rec(id::BEGIN_CF_RULE, rule(3, 2, u32::MAX, 2, 0, 0)).rec(id::BEGIN_COLOR_SCALE, d());
    let cfvo = |kind: u32, v: f64| d().u32(kind).f64(v).u32(0).u32(1).u32(0);
    b.rec(id::CFVO, cfvo(2, 0.0)).rec(id::CFVO, cfvo(1, 3.0));
    b.rec(id::COLOR, d().color(2, 0, [255, 0, 0])).rec(id::COLOR, d().color(2, 0, [0, 255, 0]));
    b.rec(470, d()).rec(id::END_CF_RULE, d()).rec(id::END_CF, d());
    // Validação: lista $A$3:$A$5 em A10.
    let list = d().u8(0x25).u32(2).u32(4).u16(0).u16(0).0;
    let dv = d().u32(3 | 1 << 8 | 1 << 19).u32(1).range(9, 9, 0, 0).s("Opa").s("Não pode").null().null();
    b.rec(id::DVAL, dv.f(&list, &[]).u32(0).u32(0));
    b.rec(id::HLINK, d().range(0, 0, 2, 2).s("rId3").null().s("dica").null());
    b.rec(id::MARGINS, d().f64(0.5).f64(0.5).f64(1.0).f64(1.0).f64(0.3).f64(0.3));
    b.rec(id::DRAWING, d().s("rId2"));
    b.rec(660, d().u32(1)).rec(id::LIST_PART, d().s("rId1")).rec(662, d());
    b.rec(130, d());
    b.0
}

fn sheet2() -> Vec<u8> {
    let mut b = Bin::default();
    b.rec(129, d()).rec(id::WS_DIM, d().range(0, 0, 0, 0));
    b.rec(id::BEGIN_SHEET_DATA, d());
    b.row(0, 300, 0, 0).rec(id::CELL_BOOL, d().cell(0, 2).u8(1));
    b.rec(id::END_SHEET_DATA, d()).rec(130, d());
    b.0
}

const DRAWING: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><xdr:oneCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="100" cy="100"/><xdr:sp macro=""><xdr:nvSpPr><xdr:cNvPr id="2" name="Forma"/></xdr:nvSpPr><xdr:spPr><a:prstGeom prst="rect"/></xdr:spPr></xdr:sp></xdr:oneCellAnchor><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:sle="http://schemas.microsoft.com/office/drawing/2010/slicer" Requires="sle"><xdr:oneCellAnchor><xdr:graphicFrame/></xdr:oneCellAnchor></mc:Choice></mc:AlternateContent></xdr:wsDr>"#;

fn parts() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("xl/workbook.bin", workbook()),
        (
            "xl/_rels/workbook.bin.rels",
            rels(&[
                ("rId1", "worksheet", "worksheets/sheet1.bin"),
                ("rId2", "worksheet", "worksheets/sheet2.bin"),
                ("rId3", "styles", "styles.bin"),
                ("rId4", "sharedStrings", "sharedStrings.bin"),
                ("rId5", "externalLink", "externalLinks/externalLink1.bin"),
                ("rId6", "calcChain", "calcChain.bin"),
                ("rId7", "chartsheet", "chartsheets/sheet1.bin"),
                ("rId8", "vbaProject", "vbaProject.bin"),
            ]),
        ),
        ("xl/worksheets/sheet1.bin", sheet1()),
        (
            "xl/worksheets/_rels/sheet1.bin.rels",
            rels(&[
                ("rId1", "table", "../tables/table1.bin"),
                ("rId2", "drawing", "../drawings/drawing1.xml"),
                ("rId3", "hyperlink", "https://exemplo.com/a?b=1&amp;c=2"),
                ("rId4", "pivotTable", "../pivotTables/pivotTable1.bin"),
            ]),
        ),
        ("xl/worksheets/sheet2.bin", sheet2()),
        ("xl/tables/table1.bin", table()),
        ("xl/styles.bin", styles()),
        ("xl/sharedStrings.bin", strings()),
        ("xl/externalLinks/externalLink1.bin", external()),
        (
            "xl/externalLinks/_rels/externalLink1.bin.rels",
            rels(&[("rId1", "externalLinkPath", "file:///C:/base.xlsx")]),
        ),
        ("xl/drawings/drawing1.xml", DRAWING.as_bytes().to_vec()),
        ("xl/pivotTables/pivotTable1.bin", vec![1, 0]),
        ("xl/chartsheets/sheet1.bin", vec![1, 0]),
        ("xl/calcChain.bin", vec![1, 0]),
        ("xl/vbaProject.bin", vec![0xD0, 0xCF]),
    ]
}

fn open(path: &Path) -> Workbook {
    crate::open(path, &Options::default()).unwrap().0
}

fn formula(s: &Sheet, row: u32, col: u32) -> Option<String> {
    duck_calc::cell_formula(s, row, col).map(|f| f.into_owned())
}

fn range(a: u32, b: u32, c: u32, e: u32) -> CellRange {
    CellRange { first_row: a, first_col: b, last_row: c, last_col: e }
}

// ---------------------------------------------------------------------------
// Testes
// ---------------------------------------------------------------------------

#[test]
fn values_of_every_kind() {
    let dir = Dir::new("valores");
    let p = dir.file("a.xlsb");
    package(&p, &parts());
    let wb = open(&p);
    let v = |r, c| wb.cell(0, r, c).value;
    assert_eq!(v(0, 0), Value::Number(2.5));
    assert_eq!(v(0, 1), Value::Number(std::f64::consts::PI));
    assert_eq!(v(0, 2), Value::Text("Ação 🦆"));
    assert_eq!(v(0, 3), Value::Text("direto\nna célula"));
    assert_eq!(v(0, 4), Value::Bool(true));
    assert_eq!(v(0, 5), Value::Error(CellError::Div0));
    assert_eq!((v(0, 6), wb.cell(0, 0, 6).style), (Value::Empty, 1));
    assert_eq!(v(0, 8), Value::Text("rico"));
    assert_eq!(v(3, 2), Value::Text("3x"));
    assert_eq!(v(5, 5), Value::Bool(true));
    assert_eq!(v(5, 6), Value::Error(CellError::Ref));
    // Células curtas: a coluna continua da anterior.
    assert_eq!((v(8, 3), v(8, 4), v(8, 5)), (Value::Number(1.0), Value::Number(2.0), Value::Number(3.5)));
    assert_eq!(wb.cell(0, 8, 5).style, 1);
    // Texto rico: a fonte 1 a partir do quinto caractere ("ação " tem 7 bytes).
    let runs = wb.shared_strings.rich(2).unwrap();
    assert_eq!((runs[0].start, runs[0].font.is_none(), runs[1].start), (0, true, 7));
    let f = runs[1].font.as_ref().unwrap();
    assert!(f.bold && f.italic);
    assert_eq!(f.name.as_deref(), Some("Arial"));
}

/// Os mesmos valores que o calamine lê.
#[test]
fn same_values_as_calamine() {
    use calamine::{Data, Reader, Xlsb, open_workbook};
    let dir = Dir::new("calamine");
    let p = dir.file("a.xlsb");
    // O calamine não lê as células curtas nem o texto rico embutido.
    let mut all = parts();
    all[2].1 = big(3000, 6);
    package(&p, &all);
    let wb = open(&p);
    let mut cal: Xlsb<_> = open_workbook(&p).unwrap();
    for (i, s) in wb.sheets.iter().enumerate() {
        let theirs = cal.worksheet_range(&s.name).unwrap();
        let (r0, c0) = theirs.start().unwrap();
        let mut n = 0;
        for (r, c, d) in theirs.used_cells() {
            let ours = wb.cell(i, r0 + r as u32, c0 + c as u32).value;
            let same = match (ours, d) {
                (Value::Number(a), Data::Float(b)) => a == *b,
                (Value::Number(a), Data::Int(b)) => a == *b as f64,
                (Value::Text(a), Data::String(b)) => a == b,
                (Value::Bool(a), Data::Bool(b)) => a == *b,
                (Value::Error(_), Data::Error(_)) => true,
                _ => false,
            };
            assert!(same, "aba {i} ({r}, {c}): {ours:?} × {d:?}");
            n += 1;
        }
        assert_eq!(s.counts().values(), n);
    }
}

#[test]
fn formulas_of_every_kind() {
    let dir = Dir::new("formulas");
    let p = dir.file("a.xlsb");
    package(&p, &parts());
    let book = crate::Book::open(&p).unwrap();
    let wb = open(&p);
    let s = &wb.sheets[0];
    let f = |r, c| formula(s, r, c);
    for r in 2..5 {
        assert_eq!(f(r, 1).unwrap(), format!("A{}*$A$1", r + 1));
        assert_eq!(f(r, 2).unwrap(), format!("A{}&\"x\"", r + 1));
    }
    // Cópias para baixo viram uma definição só.
    assert_eq!(s.formula_entry(2, 1).unwrap().0, s.formula_entry(4, 1).unwrap().0);
    assert_eq!(s.formula_entry(2, 2).unwrap().0, s.formula_entry(4, 2).unwrap().0);
    assert_eq!(f(5, 0).unwrap(), "A3:A5*2");
    let array = FormulaKind::Array { range: range(5, 0, 7, 0), dynamic: true };
    assert_eq!(s.formula(5, 0).unwrap().kind, array);
    // As outras células da matriz só têm o valor.
    assert_eq!((f(6, 0), wb.cell(0, 7, 0).value), (None, Value::Number(8.0)));
    assert_eq!(f(5, 1).unwrap(), "SUM(Vendas[[Valor R$]])");
    assert_eq!(f(5, 2).unwrap(), "_xlfn.IFS(TRUE,1)");
    assert_eq!(f(5, 3).unwrap(), "'[1]Base X'!$A$3");
    assert_eq!(f(5, 4).unwrap(), "Taxa");
    assert_eq!(f(5, 5).unwrap(), "'Minha aba'!A1");
    assert_eq!(f(5, 6).unwrap(), "#REF!");
    // Token desconhecido: fica o valor, e a conta de não lidas sobe.
    assert_eq!((f(5, 7), wb.cell(0, 5, 7).value), (None, Value::Number(7.0)));
    book.load_sheets(&[0], &Options::default(), &|_, r| assert!(r.is_ok()));
    assert_eq!(book.unread_formulas(), 1);

    // Recalculadas, dão o valor gravado.
    let sheets: Vec<&Sheet> = wb.sheets.iter().collect();
    let cb = duck_calc::Book::new(sheets, &wb.shared_strings, &wb.extras, wb.date1904);
    let eng = duck_calc::Engine::new(&cb);
    let mut n = 0;
    for (r, c) in s.formula_positions() {
        let got = eng.eval_cell(&cb, 0, r, c).unwrap();
        let got = format!("{got:?}");
        let want = match wb.cell(0, r, c).value {
            Value::Number(x) => format!("{x:?}"),
            Value::Text(t) => format!("{t:?}"),
            Value::Bool(b) => format!("{b:?}"),
            v => format!("{v:?}").replace("Error(", "").replace(')', ""),
        };
        assert!(got.contains(&want), "({r}, {c}): {got} × {want}");
        n += 1;
    }
    assert_eq!(n, 13);
}

#[test]
fn layout_styles_and_names() {
    let dir = Dir::new("layout");
    let p = dir.file("a.xlsb");
    package(&p, &parts());
    let wb = open(&p);
    // A aba de gráfico não entra; a outra está escondida.
    let tabs: Vec<_> = wb.sheets.iter().map(|s| (s.name.as_str(), s.hidden, s.tab_id)).collect();
    assert_eq!(tabs, [("Aba1", false, 1), ("Minha aba", true, 3)]);
    let l = &wb.sheets[0].layout;
    let v = l.view.unwrap();
    assert_eq!((v.show_grid_lines, v.show_headers, v.zoom), (false, true, 80));
    assert_eq!((v.top_left, v.active_cell), (Some((2, 1)), Some((3, 2))));
    assert_eq!(v.tab_color, Some(duck_core::Color::Rgb(0xFF10_2030)));
    assert_eq!(l.pane.map(|p| (p.frozen_rows, p.frozen_cols)), Some((2, 1)));
    assert_eq!((l.default_row_height, l.default_col_width), (Some(15.0), None));
    assert_eq!((l.cols[0].width, l.cols[0].hidden), (Some(20.5), false));
    assert_eq!((l.cols[1].first, l.cols[1].last, l.cols[1].hidden, l.cols[1].style), (2, 3, true, 1));
    assert_eq!((l.outline.cols.level(2), l.outline.rows.level(1)), (2, 1));
    // Só a linha 2 tem altura, estilo ou está escondida.
    assert_eq!(l.rows.len(), 1);
    let r = l.rows[0];
    assert_eq!((r.row, r.height, r.hidden, r.style), (1, Some(30.0), true, Some(1)));
    assert_eq!(l.merges, [range(9, 0, 10, 2)]);
    let af = l.autofilter.as_ref().unwrap();
    assert_eq!((af.range, af.columns.len()), (range(8, 3, 11, 5), 1));
    assert_eq!(l.cond.len(), 2);
    assert!(
        matches!(&l.cond[0].kind, duck_core::CfKind::CellIs { op: duck_core::CfOp::Gt, f1, .. } if f1 == "$A$1")
    );
    assert!(matches!(&l.cond[1].kind, duck_core::CfKind::ColorScale(v) if v.len() == 2));
    assert_eq!(l.links[0].target.as_deref(), Some("https://exemplo.com/a?b=1&c=2"));
    assert_eq!(l.links[0].tooltip.as_deref(), Some("dica"));
    let t = &l.tables[0];
    assert_eq!((t.name.as_str(), t.id, t.range), ("Vendas", 7, range(1, 6, 4, 7)));
    assert_eq!((t.columns.as_slice(), t.row_stripes), (&["Loja".to_string(), "Valor R$".into()][..], true));
    // A forma aparece; a segmentação (que não é lida) não.
    assert_eq!((l.objects.len(), l.drawing_anchors.len()), (1, 1));

    let st = &wb.styles;
    assert_eq!(st.xfs.len(), 3);
    let x = st.xfs[1];
    assert_eq!((x.num_fmt, x.font, x.fill, x.border), (164, 1, 2, 1));
    let a = x.alignment;
    assert_eq!((a.horizontal, a.vertical), (HAlign::Center, duck_core::VAlign::Center));
    assert_eq!((a.wrap, a.rotation, a.indent), (true, 90, 1));
    assert_eq!(st.num_fmts[&164], "0.00\" kg\"");
    let f = &st.fonts[1];
    assert_eq!((f.name.as_str(), f.size, f.bold, f.italic), ("Arial", 11.0, true, true));
    assert_eq!(f.color, duck_core::Color::Rgb(0xFFFF_0000));
    assert_eq!(st.fills[2].fg, duck_core::Color::Rgb(0xFF01_0203));
    let b = &st.borders[1];
    assert_eq!(b.top.style, duck_core::BorderStyle::Thin);
    assert_eq!(b.bottom.style, duck_core::BorderStyle::Double);
    assert_eq!(st.xfs[0].alignment, Default::default());
    let d = &st.dxfs[0];
    assert_eq!((d.bold, d.fill), (Some(true), Some(duck_core::Color::Rgb(0xFF09_0807))));

    let names: Vec<_> =
        wb.extras.names.iter().map(|n| (n.name.as_str(), n.sheet, n.formula.as_str())).collect();
    assert_eq!(
        names,
        // `Aba1` precisa de aspas: sem elas, é uma célula (coluna ABA).
        [("Taxa", None, "'Aba1'!$A$1"), ("_xlnm.Print_Area", Some(1), "'Minha aba'!$A$1:$C$10")]
    );
    let e = &wb.extras.externals[0];
    assert_eq!(e.sheets[0].name, "Base X");
    assert_eq!(e.sheets[0].cell(2, 0, &wb.shared_strings).value, Value::Number(42.5));
    let book = crate::Book::open(&p).unwrap();
    assert!(book.is_binary());
    assert_eq!(book.left_out(), [("tabelas dinâmicas (só os valores)", 1), ("abas de gráfico", 1)]);
}

/// Linhas com números, textos, uma fórmula compartilhada que atravessa a aba
/// inteira (mestre na primeira linha) e fórmulas comuns.
fn big(rows: u32, cols: u32) -> Vec<u8> {
    let mut b = Bin::default();
    b.rec(129, d()).rec(id::WS_DIM, d().range(0, rows - 1, 0, cols - 1));
    b.rec(id::BEGIN_SHEET_DATA, d());
    for r in 0..rows {
        // Registros que precedem a linha (extensões), como o Excel grava.
        b.rec(37, d().u16(1).u32(0)).rec(1024, d().u16(15)).rec(38, d());
        b.row(r, 300, 0, 0);
        b.rec(id::CELL_RK, d().cell(0, 0).u32(r << 2 | 2));
        let (e, x) = ptg_exp(0, 1);
        b.rec(id::FMLA_NUM, d().cell(1, 0).f64(r as f64 * 3.0).u16(0).f(&e, &x));
        if r == 0 {
            // A1*3, com a coluna relativa à esquerda.
            let body = [d().u8(0x4C).u32(0).u16(REL | 0x3FFF).0, vec![0x1E, 3, 0, 0x05]].concat();
            b.rec(id::SHR_FMLA, d().range(0, rows - 1, 1, 1).f(&body, &[]));
        }
        let f = [ptg_ref(r, REL), ptg_ref(r, REL | 1), vec![3]].concat();
        b.rec(id::FMLA_NUM, d().cell(2, 0).f64(r as f64 * 4.0).u16(0).f(&f, &[]));
        for c in 3..cols {
            match (r + c) % 3 {
                0 => b.rec(id::CELL_ST, d().cell(c, r % 3).s(&format!("t{r}-{c}"))),
                1 => b.rec(id::CELL_REAL, d().cell(c, 0).f64(r as f64 + 0.5)),
                _ => b.rec(id::CELL_ISST, d().cell(c, 0).u32(r % 3)),
            };
        }
    }
    b.rec(id::END_SHEET_DATA, d()).rec(130, d());
    b.0
}

/// Mesmo resultado com qualquer número de threads e tamanho de pedaço.
#[test]
fn chunks_and_threads_do_not_change_the_result() {
    let dir = Dir::new("pedacos");
    let p = dir.file("a.xlsb");
    let mut all = parts();
    const ROWS: u32 = 20_000;
    all[2].1 = big(ROWS, 8);
    package(&p, &all);
    let load = |threads: usize, chunk: usize| {
        let pool = duck_par::pool(threads);
        let opts = Options { chunk_bytes: chunk, ..Default::default() };
        pool.install(|| crate::open(&p, &opts).unwrap())
    };
    let (one, rep) = load(1, 1 << 30);
    assert_eq!(rep.sheets[0].chunks, 2);
    let s = &one.sheets[0];
    assert_eq!((s.rows(), s.cols()), (ROWS, 8));
    for r in [0, 1, 1023, 1024, 9_999, ROWS - 1] {
        assert_eq!(formula(s, r, 1).unwrap(), format!("A{}*3", r + 1));
        assert_eq!(formula(s, r, 2).unwrap(), format!("A{0}+B{0}", r + 1));
        assert_eq!(one.cell(0, r, 2).value, Value::Number(r as f64 * 4.0));
    }
    // Duas definições para 40 mil fórmulas.
    assert_eq!((s.counts().formulas, s.formula_texts_stored()), (2 * ROWS as u64, 2));
    for threads in [1, 2, 8] {
        let (many, rep) = load(threads, 256 * 1024);
        assert!(rep.sheets[0].chunks > 5, "{}", rep.sheets[0].chunks);
        for (a, b) in one.sheets.iter().zip(&many.sheets) {
            crate::write::compare_sheets(a, &one.shared_strings, b, &many.shared_strings).unwrap();
            assert_eq!(a.layout, b.layout);
            assert_eq!(a.formula_texts_stored(), b.formula_texts_stored());
        }
    }
}

/// Aba de mais de 8 MB comprimidos: em XML usaria o inflate paralelo, que
/// piora o binário.
#[test]
fn big_sheets_use_sequential_inflate() {
    let dir = Dir::new("inflate");
    let p = dir.file("a.xlsb");
    let mut b = Bin::default();
    b.rec(129, d()).rec(id::BEGIN_SHEET_DATA, d());
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for r in 0..180_000u32 {
        b.row(r, 300, 0, 0);
        for c in 0..8 {
            // Números que não comprimem.
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            b.rec(id::CELL_REAL, d().cell(c, 0).f64((x >> 12) as f64));
        }
    }
    b.rec(id::END_SHEET_DATA, d()).rec(130, d());
    let mut all = parts();
    all[2].1 = b.0;
    package(&p, &all);
    let load = |threads| duck_par::pool(threads).install(|| crate::open(&p, &Options::default()).unwrap());
    let ((a, ra), (z, rz)) = (load(2), load(8));
    assert!(!ra.sheets[0].parallel_inflate && !rz.sheets[0].parallel_inflate);
    crate::write::compare_sheets(&a.sheets[0], &a.shared_strings, &z.sheets[0], &z.shared_strings).unwrap();
    assert_eq!(a.sheets[0].counts().numbers, 180_000 * 8);
}

#[test]
fn preview_has_the_first_rows() {
    let dir = Dir::new("previa");
    let p = dir.file("a.xlsb");
    let mut all = parts();
    all[2].1 = big(20_000, 6);
    package(&p, &all);
    let book = crate::Book::open(&p).unwrap();
    let s = book.preview_sheet(0, 256 * 1024).unwrap();
    assert!((100..20_000).contains(&s.rows()), "{}", s.rows());
    assert_eq!(formula(&s, 50, 1).unwrap(), "A51*3");
    // Aba pequena: vem inteira, com a visão e as larguras.
    let p = dir.file("b.xlsb");
    package(&p, &parts());
    let s = crate::Book::open(&p).unwrap().preview_sheet(0, 1 << 20).unwrap();
    assert_eq!(formula(&s, 4, 1).unwrap(), "A5*$A$1");
    assert!(s.layout.pane.is_some() && s.layout.cols.len() == 2);
}

/// Salvar como `.xlsx`: tudo o que é lido vai, com as edições.
#[test]
fn saved_as_xlsx() {
    let dir = Dir::new("salvar");
    let p = dir.file("a.xlsb");
    package(&p, &parts());
    let original = std::fs::read(&p).unwrap();
    let mut wb = open(&p);
    wb.sheets[0].set_cell(20, 0, duck_core::CellValue::Text("novo".into()), None, None);
    let save = |wb: &Workbook, name: &str| {
        let sheets: Vec<Option<&Sheet>> = wb.sheets.iter().map(Some).collect();
        let q = dir.file(name);
        crate::save_file(&p, &q, &sheets, &wb.shared_strings, &wb.styles, &SaveOptions::default()).map(|_| q)
    };
    let q = save(&wb, "b.xlsx").unwrap();
    let back = open(&q);
    for (a, b) in wb.sheets.iter().zip(&back.sheets) {
        crate::write::compare_sheets(a, &wb.shared_strings, b, &back.shared_strings).unwrap();
        let (x, y) = (&a.layout, &b.layout);
        assert_eq!((&x.cols, &x.rows, &x.merges, x.pane), (&y.cols, &y.rows, &y.merges, y.pane));
        assert_eq!((&x.cond, &x.links, &x.autofilter), (&y.cond, &y.links, &y.autofilter));
        assert_eq!((&x.tables, &x.objects, x.view), (&y.tables, &y.objects, y.view));
        assert_eq!((&x.outline, a.hidden, a.tab_id), (&y.outline, b.hidden, b.tab_id));
    }
    assert_eq!(back.cell(0, 20, 0).value, Value::Text("novo"));
    assert_eq!(back.shared_strings.rich(2), wb.shared_strings.rich(2));
    assert_eq!(back.sheets[0].formula(5, 0).unwrap().kind, wb.sheets[0].formula(5, 0).unwrap().kind);
    assert_eq!(
        (&back.styles.xfs, &back.styles.fonts, &back.styles.dxfs),
        (&wb.styles.xfs, &wb.styles.fonts, &wb.styles.dxfs)
    );
    assert_eq!(back.styles.num_fmts, wb.styles.num_fmts);
    let names = |w: &Workbook| {
        w.extras.names.iter().map(|n| (n.name.clone(), n.sheet, n.formula.clone())).collect::<Vec<_>>()
    };
    assert_eq!(names(&back), names(&wb));
    assert_eq!(back.extras.externals.len(), 1);

    // O pacote: partes em XML, sem o que não é lido nem as macros.
    let pkg = crate::Package::open(&q).unwrap();
    let mut list: Vec<&str> = pkg.entries().map(|e| e.name.as_str()).collect();
    list.sort_unstable();
    assert!(list.iter().all(|n| !n.ends_with(".bin") && !n.contains("pivot")), "{list:?}");
    for part in ["xl/workbook.xml", "xl/tables/table1.xml", "xl/drawings/drawing1.xml", "xl/metadata.xml"] {
        assert!(list.contains(&part), "{part}: {list:?}");
    }
    let sheet = String::from_utf8(pkg.read("xl/worksheets/sheet1.xml").unwrap()).unwrap();
    assert!(
        sheet.contains("<dataValidation type=\"list\"") && sheet.contains("<formula1>$A$3:$A$5</formula1>")
    );
    assert!(sheet.contains("<pageMargins left=\"0.5\""));
    let ct = String::from_utf8(pkg.read("[Content_Types].xml").unwrap()).unwrap();
    assert!(ct.contains("spreadsheetml.sheet.main+xml") && !ct.contains("macroEnabled"));

    // Com macros, só em `.xlsm`.
    let m = save(&wb, "b.xlsm").unwrap();
    let pkg = crate::Package::open(&m).unwrap();
    assert_eq!(pkg.read("xl/vbaProject.bin").unwrap(), [0xD0, 0xCF]);
    let ct = String::from_utf8(pkg.read("[Content_Types].xml").unwrap()).unwrap();
    assert!(ct.contains("sheet.macroEnabled.main+xml") && ct.contains("vbaProject"));
    // O binário não é regravado, e o original fica como estava.
    let e = save(&wb, "c.xlsb").unwrap_err().to_string();
    assert!(e.contains(".xlsb") && !dir.file("c.xlsb").exists(), "{e}");
    let e = save(&wb, "a.xlsb").unwrap_err().to_string();
    assert!(e.contains(".xlsb"), "{e}");
    assert_eq!(std::fs::read(&p).unwrap(), original);
    // Nenhum temporário ficou para trás.
    let left: Vec<_> = std::fs::read_dir(&dir.0).unwrap().map(|f| f.unwrap().file_name()).collect();
    assert_eq!(left.len(), 3, "{left:?}");
}

/// Inserir linhas e depois salvar.
#[test]
fn structural_edits_are_saved() {
    let dir = Dir::new("estrutura");
    let p = dir.file("a.xlsb");
    package(&p, &parts());
    let mut wb = open(&p);
    let shift = duck_core::Shift { axis: duck_core::Axis::Rows, at: 2, n: 3, insert: true };
    let ops = duck_calc::ShiftOps { shift, sheet: "Aba1".into(), own: true };
    duck_core::Command::Shift(shift, duck_core::OpsHandle(std::sync::Arc::new(ops))).apply(&mut wb.sheets[0]);
    let sheets: Vec<Option<&Sheet>> = wb.sheets.iter().map(Some).collect();
    let q = dir.file("b.xlsx");
    crate::save_file(&p, &q, &sheets, &wb.shared_strings, &wb.styles, &SaveOptions::default()).unwrap();
    let back = open(&q);
    assert_eq!(formula(&back.sheets[0], 5, 1).unwrap(), "A6*$A$1");
    assert_eq!(back.sheets[0].layout.merges, [range(12, 0, 13, 2)]);
}

/// Arquivo truncado ou corrompido: erro ou o que der para ler, nunca um pânico.
#[test]
fn damaged_files_do_not_panic() {
    let dir = Dir::new("danos");
    let p = dir.file("a.xlsb");
    let whole = parts();
    let mut x = 12345u32;
    let mut next = || {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        x >> 8
    };
    // Mais rodadas: DUCK_RODADAS=5000.
    let rounds = std::env::var("DUCK_RODADAS").ok().and_then(|v| v.parse().ok()).unwrap_or(80);
    for round in 0..rounds {
        let mut all = whole.clone();
        let bins: Vec<usize> =
            (0..all.len()).filter(|&i| all[i].0.ends_with(".bin") && all[i].1.len() > 8).collect();
        let part = &mut all[bins[round % bins.len()]].1;
        if round % 4 == 0 {
            part.truncate(next() as usize % part.len());
        } else {
            for _ in 0..1 + next() % 6 {
                let at = next() as usize % part.len();
                part[at] = next() as u8;
            }
        }
        package(&p, &all);
        if let Ok(book) = crate::Book::open(&p) {
            let _ = (book.read_styles(), book.read_shared_strings(), book.read_extras());
            for i in 0..book.sheets().len() {
                let _ = book.preview_sheet(i, 1 << 20);
            }
            if let Ok((wb, _)) = crate::open(&p, &Options::default()) {
                for (r, c) in wb.sheets.iter().flat_map(|s| s.formula_positions()).take(100) {
                    let _ = formula(&wb.sheets[0], r, c);
                }
            }
        }
    }
}

#[test]
fn cut_sheet_is_an_error() {
    let dir = Dir::new("cortada");
    let p = dir.file("a.xlsb");
    let mut all = parts();
    let n = all[2].1.len();
    all[2].1.truncate(n / 3);
    package(&p, &all);
    let e = crate::open(&p, &Options::default()).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains("truncada"), "{e}");
}

#[test]
fn drawings_and_charts_lose_only_what_is_not_read() {
    let out = String::from_utf8(super::drawing(DRAWING.as_bytes())).unwrap();
    assert!(out.contains("name=\"Forma\"") && out.ends_with("</xdr:oneCellAnchor></xdr:wsDr>"), "{out}");
    let plain = r#"<xdr:wsDr><xdr:oneCellAnchor><xdr:pic/></xdr:oneCellAnchor></xdr:wsDr>"#;
    assert_eq!(super::drawing(plain.as_bytes()), plain.as_bytes());
    let user = r#"<c:userShapes><a/></c:userShapes>"#;
    assert_eq!(super::drawing(user.as_bytes()), user.as_bytes());
    let chart = r#"<c:chartSpace><c:pivotSource><c:name>x</c:name></c:pivotSource><c:chart/></c:chartSpace>"#;
    assert_eq!(super::chart(chart.as_bytes()), b"<c:chartSpace><c:chart/></c:chartSpace>");
    // Os intervalos de um gráfico dinâmico perdem a linha de total.
    let refs = |cat: &str, val: &str| {
        format!(
            "<c:cat><c:strRef><c:f>{cat}</c:f><c:strCache><c:ptCount val=\"3\"/></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>{val}</c:f><c:numCache><c:ptCount val=\"3\"/></c:numCache></c:numRef></c:val></c:chartSpace>"
        )
    };
    let tx =
        "<c:tx><c:strRef><c:f>D!$B$4</c:f><c:strCache><c:ptCount val=\"1\"/></c:strCache></c:strRef></c:tx>";
    let pivot = format!(
        "<c:pivotSource><c:name>x</c:name></c:pivotSource>{tx}{}",
        refs("D!$A$5:$A$8", "D!$B$5:$B$8")
    );
    let plain = format!("{tx}{}", refs("D!$A$5:$A$7", "D!$B$5:$B$7"));
    assert_eq!(String::from_utf8(super::chart(pivot.as_bytes())).unwrap(), plain);
    // Um gráfico comum fica como está.
    let common = refs("D!$A$5:$A$8", "D!$B$5:$B$8");
    assert_eq!(super::chart(common.as_bytes()), common.as_bytes());
}
