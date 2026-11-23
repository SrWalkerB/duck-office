//! Registros do BIFF12 (MS-XLSB 2.1.4): tipo (1 ou 2 bytes), tamanho (1 a 4
//! bytes, 7 bits por byte) e dados, um depois do outro.

use duck_core::{CellError, CellRange};

/// Números dos registros usados (MS-XLSB 2.3.2).
pub mod id {
    pub const ROW: u16 = 0;
    pub const CELL_BLANK: u16 = 1;
    pub const CELL_RK: u16 = 2;
    pub const CELL_ERROR: u16 = 3;
    pub const CELL_BOOL: u16 = 4;
    pub const CELL_REAL: u16 = 5;
    pub const CELL_ST: u16 = 6;
    pub const CELL_ISST: u16 = 7;
    pub const FMLA_STRING: u16 = 8;
    pub const FMLA_NUM: u16 = 9;
    pub const FMLA_BOOL: u16 = 10;
    pub const FMLA_ERROR: u16 = 11;
    /// Células "curtas" (sem a coluna: a seguinte à anterior), de 12 a 18.
    pub const SHORT_BLANK: u16 = 12;
    pub const SHORT_ISST: u16 = 18;
    pub const SST_ITEM: u16 = 19;
    pub const NAME: u16 = 39;
    pub const FONT: u16 = 43;
    pub const FMT: u16 = 44;
    pub const FILL: u16 = 45;
    pub const BORDER: u16 = 46;
    pub const XF: u16 = 47;
    pub const STYLE: u16 = 48;
    pub const CELL_META: u16 = 49;
    pub const COL_INFO: u16 = 60;
    pub const CELL_RSTRING: u16 = 62;
    pub const DVAL: u16 = 64;
    pub const BEGIN_WS_VIEW: u16 = 137;
    pub const END_WS_VIEW: u16 = 138;
    pub const BEGIN_SHEET_DATA: u16 = 145;
    pub const END_SHEET_DATA: u16 = 146;
    pub const WS_PROP: u16 = 147;
    pub const WS_DIM: u16 = 148;
    pub const PANE: u16 = 151;
    pub const SEL: u16 = 152;
    pub const WB_PROP: u16 = 153;
    pub const BUNDLE_SH: u16 = 156;
    pub const CALC_PROP: u16 = 157;
    pub const BOOK_VIEW: u16 = 158;
    pub const BEGIN_AFILTER: u16 = 161;
    pub const END_AFILTER: u16 = 162;
    pub const BEGIN_FILTER_COLUMN: u16 = 163;
    pub const END_FILTER_COLUMN: u16 = 164;
    pub const BEGIN_FILTERS: u16 = 165;
    pub const END_FILTERS: u16 = 166;
    pub const FILTER: u16 = 167;
    pub const TOP10_FILTER: u16 = 170;
    pub const BEGIN_CUSTOM_FILTERS: u16 = 172;
    pub const END_CUSTOM_FILTERS: u16 = 173;
    pub const CUSTOM_FILTER: u16 = 174;
    pub const DATE_GROUP_ITEM: u16 = 175;
    pub const MERGE_CELL: u16 = 176;
    pub const BEGIN_LIST: u16 = 343;
    pub const BEGIN_LIST_COL: u16 = 347;
    pub const END_LIST_COL: u16 = 348;
    pub const LIST_CC_FMLA: u16 = 351;
    pub const LIST_TR_FMLA: u16 = 352;
    pub const SUP_BOOK_SRC: u16 = 355;
    pub const SUP_SELF: u16 = 357;
    pub const SUP_SAME: u16 = 358;
    pub const SUP_TABS: u16 = 359;
    pub const BEGIN_SUP_BOOK: u16 = 360;
    pub const EXTERN_SHEET: u16 = 362;
    pub const EXTERN_TABLE_START: u16 = 363;
    pub const EXTERN_TABLE_END: u16 = 364;
    pub const EXTERN_ROW: u16 = 366;
    pub const EXTERN_BLANK: u16 = 367;
    pub const EXTERN_REAL: u16 = 368;
    pub const EXTERN_BOOL: u16 = 369;
    pub const EXTERN_ERROR: u16 = 370;
    pub const EXTERN_STRING: u16 = 371;
    pub const ARR_FMLA: u16 = 426;
    pub const SHR_FMLA: u16 = 427;
    pub const TABLE: u16 = 428;
    pub const BEGIN_CF: u16 = 461;
    pub const END_CF: u16 = 462;
    pub const BEGIN_CF_RULE: u16 = 463;
    pub const END_CF_RULE: u16 = 464;
    pub const BEGIN_ICON_SET: u16 = 465;
    pub const BEGIN_DATABAR: u16 = 467;
    pub const BEGIN_COLOR_SCALE: u16 = 469;
    pub const CFVO: u16 = 471;
    pub const INDEXED_COLOR: u16 = 475;
    pub const MARGINS: u16 = 476;
    pub const PAGE_SETUP: u16 = 478;
    pub const WS_FMT_INFO: u16 = 485;
    pub const HLINK: u16 = 494;
    pub const BEGIN_DXFS: u16 = 505;
    pub const END_DXFS: u16 = 506;
    pub const DXF: u16 = 507;
    pub const TABLE_STYLE_CLIENT: u16 = 513;
    pub const DRAWING: u16 = 550;
    pub const COLOR: u16 = 564;
    pub const SUP_NAME_START: u16 = 577;
    pub const BEGIN_CELL_XFS: u16 = 617;
    pub const END_CELL_XFS: u16 = 618;
    pub const BEGIN_STYLE_XFS: u16 = 626;
    pub const END_STYLE_XFS: u16 = 627;
    pub const LIST_PART: u16 = 661;
    pub const SUP_ADDIN: u16 = 667;
}

/// Últimas linha e coluna de uma aba (base 0).
pub const LAST_ROW: u32 = 0xF_FFFF;
pub const LAST_COL: u32 = 0x3FFF;

/// O que há em `pos`.
pub enum Head {
    /// Tipo, início e fim dos dados (o registro inteiro está no buffer).
    Record(u16, usize, usize),
    /// O registro continua depois do fim do buffer.
    Partial,
}

/// Cabeçalho do registro em `pos`.
#[inline]
pub fn head(buf: &[u8], pos: usize) -> Head {
    let Some(&b0) = buf.get(pos) else { return Head::Partial };
    let (mut kind, mut p) = (b0 as u16, pos + 1);
    if b0 & 0x80 != 0 {
        let Some(&b1) = buf.get(p) else { return Head::Partial };
        kind = (b0 & 0x7f) as u16 | ((b1 & 0x7f) as u16) << 7;
        p += 1;
    }
    let mut len = 0usize;
    for k in 0..4 {
        let Some(&b) = buf.get(p) else { return Head::Partial };
        p += 1;
        len |= ((b & 0x7f) as usize) << (7 * k);
        if b & 0x80 == 0 {
            break;
        }
    }
    if p + len > buf.len() { Head::Partial } else { Head::Record(kind, p, p + len) }
}

/// Os registros inteiros de um buffer, em ordem.
pub struct Records<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Records<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
}

impl<'a> Iterator for Records<'a> {
    type Item = (u16, &'a [u8]);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match head(self.buf, self.pos) {
            Head::Record(k, s, e) => {
                self.pos = e;
                Some((k, &self.buf[s..e]))
            }
            Head::Partial => None,
        }
    }
}

/// Leitor dos campos de um registro. `None` = o registro acabou antes.
#[derive(Clone)]
pub struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }

    #[inline]
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }

    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    pub fn is_empty(&self) -> bool {
        self.p >= self.b.len()
    }

    #[inline]
    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    #[inline]
    pub fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    #[inline]
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }

    #[inline]
    pub fn f64(&mut self) -> Option<f64> {
        self.take(8).and_then(|b| Some(f64::from_le_bytes(*b.first_chunk()?)))
    }

    /// `XLWideString`: tamanho em unidades UTF-16 (4 bytes) e o texto.
    /// O nulo (`XLNullableWideString`) vem vazio.
    #[inline]
    pub fn wide(&mut self) -> Option<&'a [u8]> {
        match self.u32()? {
            u32::MAX => Some(&[]),
            n => self.take((n as usize).checked_mul(2)?),
        }
    }

    pub fn string(&mut self) -> Option<String> {
        Some(utf16(self.wide()?))
    }

    /// Fórmula: tokens (`rgce`) e os dados extras deles (`rgcb`).
    #[inline]
    pub fn formula(&mut self) -> Option<(&'a [u8], &'a [u8])> {
        let n = self.u32()? as usize;
        let rgce = self.take(n)?;
        let n = self.u32()? as usize;
        Some((rgce, self.take(n)?))
    }

    /// `UncheckedRfX`: intervalo de células.
    pub fn range(&mut self) -> Option<CellRange> {
        let (first_row, last_row) = (self.u32()?, self.u32()?);
        let (first_col, last_col) = (self.u32()?, self.u32()?);
        let ok =
            first_row <= last_row && last_row <= LAST_ROW && first_col <= last_col && last_col <= LAST_COL;
        ok.then_some(CellRange { first_row, first_col, last_row, last_col })
    }

    /// `UncheckedSqRfX`: lista de intervalos.
    pub fn ranges(&mut self) -> Option<Vec<CellRange>> {
        let n = self.u32()? as usize;
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            v.push(self.range()?);
        }
        Some(v)
    }
}

pub fn utf16(b: &[u8]) -> String {
    let mut s = String::new();
    push_utf16(b, &mut s);
    s
}

/// Acrescenta texto UTF-16 LE; metades soltas de par substituto viram U+FFFD.
pub fn push_utf16(b: &[u8], out: &mut String) {
    out.reserve(b.len() / 2);
    let units = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c));
    out.extend(char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)));
}

/// `RkNumber`: inteiro de 30 bits ou a metade de cima de um `f64`, talvez ÷ 100.
#[inline]
pub fn rk(v: u32) -> f64 {
    let x =
        if v & 2 != 0 { ((v as i32) >> 2) as f64 } else { f64::from_bits(((v & 0xFFFF_FFFC) as u64) << 32) };
    if v & 1 != 0 { x / 100.0 } else { x }
}

/// Código de erro gravado no arquivo.
pub fn error(c: u8) -> CellError {
    match c {
        0x00 => CellError::Null,
        0x07 => CellError::Div0,
        0x0F => CellError::Value,
        0x17 => CellError::Ref,
        0x1D => CellError::Name,
        0x24 => CellError::Num,
        0x2A => CellError::NA,
        0x2B => CellError::GettingData,
        _ => CellError::Other,
    }
}

/// Atributos de cor do XML (` rgb="…"`, ` theme="…" tint="…"`...) de um
/// `BrtColor` (8 bytes). `None` = cor não informada.
pub fn color_attrs(c: &[u8]) -> Option<String> {
    let c: &[u8; 8] = c.first_chunk()?;
    let mut s = match c[0] >> 1 {
        0 => " auto=\"1\"".to_string(),
        1 => format!(" indexed=\"{}\"", c[1]),
        2 => format!(" rgb=\"{:02X}{:02X}{:02X}{:02X}\"", c[7], c[4], c[5], c[6]),
        3 => format!(" theme=\"{}\"", c[1]),
        _ => return None,
    };
    let tint = i16::from_le_bytes([c[2], c[3]]);
    if tint != 0 {
        s.push_str(&format!(" tint=\"{}\"", tint as f64 / 32767.0));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_of_one_and_two_bytes() {
        let buf = [0x00, 2, 9, 9, 0x91, 0x01, 0x80, 0x01];
        let Head::Record(k, s, e) = head(&buf, 0) else { panic!() };
        assert_eq!((k, s, e), (0, 2, 4));
        // 0x91 0x01 = 145 (BrtBeginSheetData), com 128 bytes que não estão aqui.
        assert!(matches!(head(&buf, 4), Head::Partial));
        let mut whole = buf.to_vec();
        whole.extend([7; 128]);
        let Head::Record(k, s, e) = head(&whole, 4) else { panic!() };
        assert_eq!((k, s, e), (145, 8, 136));
        for cut in 4..8 {
            assert!(matches!(head(&whole[..cut], 4), Head::Partial));
        }
    }

    #[test]
    fn rk_numbers() {
        assert_eq!(rk((123 << 2) | 2), 123.0);
        assert_eq!(rk((((-5i32) << 2) | 2) as u32), -5.0);
        assert_eq!(rk((12345 << 2) | 3), 123.45);
        assert_eq!(rk((1.5f64.to_bits() >> 32) as u32), 1.5);
    }

    #[test]
    fn text_and_colors() {
        let b: Vec<u8> = "ação 🦆".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(utf16(&b), "ação 🦆");
        assert_eq!(utf16(&[0x00, 0xD8]), "\u{FFFD}");
        assert_eq!(color_attrs(&[5, 0, 0, 0, 1, 2, 3, 255]).unwrap(), " rgb=\"FF010203\"");
        assert_eq!(color_attrs(&[7, 4, 0xFF, 0x7F, 0, 0, 0, 0]).unwrap(), " theme=\"4\" tint=\"1\"");
        assert_eq!(color_attrs(&[2, 64, 0, 0, 0, 0, 0, 0]).unwrap(), " indexed=\"64\"");
        assert!(color_attrs(&[8, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }
}
