//! `duck-core`: o modelo de dados do workbook (ADR-004).
//!
//! - **Colunar em blocos**: cada coluna é uma lista de blocos de
//!   [`BLOCK_ROWS`] linhas. Blocos vazios não alocam.
//! - Cada célula ocupa 11 bytes dentro do bloco: tipo (`u8`), estilo (`u16`)
//!   e um payload de 64 bits (número `f64`, índice de string, booleano ou erro).
//! - Strings ficam em tabelas contíguas ([`StringTable`]); células guardam
//!   só o índice.
//! - Partes da mesma aba podem ser montadas em paralelo ([`SheetPart`]) e
//!   juntadas depois ([`Sheet::merge_part`]) em ordem fixa, então o resultado
//!   é idêntico com qualquer número de threads (ADR-012).

mod styles;

pub use styles::{Alignment, CellXf, Color, Fill, Font, HAlign, Styles, VAlign};

/// Linhas por bloco.
pub const BLOCK_ROWS: usize = 1024;

/// Tipo da célula (os 7 bits de baixo de `kind`).
pub mod kind {
    pub const EMPTY: u8 = 0;
    pub const NUMBER: u8 = 1;
    /// Índice na tabela de strings compartilhadas do workbook.
    pub const SHARED_STR: u8 = 2;
    /// Índice na tabela de strings da própria aba (texto inline, resultado de fórmula).
    pub const LOCAL_STR: u8 = 3;
    pub const BOOL: u8 = 4;
    pub const ERROR: u8 = 5;
    /// Bit alto: a célula tem fórmula (o valor é o resultado em cache).
    pub const FORMULA: u8 = 0x80;
    pub const TYPE_MASK: u8 = 0x7f;
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// Tabela de strings contígua: um buffer só + posições de fim.
#[derive(Clone, Default)]
pub struct StringTable {
    data: String,
    ends: Vec<u64>,
}

impl StringTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(strings: usize, bytes: usize) -> Self {
        Self { data: String::with_capacity(bytes), ends: Vec::with_capacity(strings) }
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    pub fn push(&mut self, s: &str) -> u32 {
        self.data.push_str(s);
        self.ends.push(self.data.len() as u64);
        (self.ends.len() - 1) as u32
    }

    pub fn get(&self, i: u32) -> Option<&str> {
        let i = i as usize;
        let end = *self.ends.get(i)? as usize;
        let start = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        Some(&self.data[start..end])
    }

    /// Anexa outra tabela; retorna o deslocamento dos índices dela.
    pub fn append(&mut self, other: &StringTable) -> u32 {
        let base = self.ends.len() as u32;
        let byte_base = self.data.len() as u64;
        self.data.push_str(&other.data);
        self.ends.extend(other.ends.iter().map(|e| e + byte_base));
        base
    }

    pub fn heap_bytes(&self) -> usize {
        self.data.capacity() + self.ends.capacity() * 8
    }
}

// ---------------------------------------------------------------------------
// Valores
// ---------------------------------------------------------------------------

/// Erros de fórmula do Excel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CellError {
    Null = 1,
    Div0,
    Value,
    Ref,
    Name,
    Num,
    NA,
    GettingData,
    Spill,
    Calc,
    Other,
}

impl CellError {
    pub fn parse(s: &[u8]) -> Self {
        match s {
            b"#NULL!" => Self::Null,
            b"#DIV/0!" => Self::Div0,
            b"#VALUE!" => Self::Value,
            b"#REF!" => Self::Ref,
            b"#NAME?" => Self::Name,
            b"#NUM!" => Self::Num,
            b"#N/A" => Self::NA,
            b"#GETTING_DATA" => Self::GettingData,
            b"#SPILL!" => Self::Spill,
            b"#CALC!" => Self::Calc,
            _ => Self::Other,
        }
    }

    pub fn from_code(c: u8) -> Self {
        match c {
            1 => Self::Null,
            2 => Self::Div0,
            3 => Self::Value,
            4 => Self::Ref,
            5 => Self::Name,
            6 => Self::Num,
            7 => Self::NA,
            8 => Self::GettingData,
            9 => Self::Spill,
            10 => Self::Calc,
            _ => Self::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Null => "#NULL!",
            Self::Div0 => "#DIV/0!",
            Self::Value => "#VALUE!",
            Self::Ref => "#REF!",
            Self::Name => "#NAME?",
            Self::Num => "#NUM!",
            Self::NA => "#N/A",
            Self::GettingData => "#GETTING_DATA",
            Self::Spill => "#SPILL!",
            Self::Calc => "#CALC!",
            Self::Other => "#ERR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    Empty,
    Number(f64),
    Text(&'a str),
    Bool(bool),
    Error(CellError),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell<'a> {
    pub value: Value<'a>,
    /// Índice em [`Styles::xfs`].
    pub style: u16,
    pub formula: bool,
}

// ---------------------------------------------------------------------------
// Blocos e colunas
// ---------------------------------------------------------------------------

/// 1024 linhas de uma coluna. 11 KB, alocado só se a coluna tiver algo nessas linhas.
pub struct Block {
    pub kind: [u8; BLOCK_ROWS],
    pub style: [u16; BLOCK_ROWS],
    pub payload: [u64; BLOCK_ROWS],
}

impl Block {
    fn new() -> Box<Self> {
        Box::new(Self { kind: [0; BLOCK_ROWS], style: [0; BLOCK_ROWS], payload: [0; BLOCK_ROWS] })
    }

    #[inline]
    fn present(&self, i: usize) -> bool {
        self.kind[i] != kind::EMPTY || self.style[i] != 0
    }
}

#[derive(Default)]
pub struct Column {
    blocks: Vec<Option<Box<Block>>>,
}

impl Column {
    #[inline]
    fn block_mut(&mut self, bi: usize) -> &mut Block {
        if bi >= self.blocks.len() {
            self.blocks.resize_with(bi + 1, || None);
        }
        self.blocks[bi].get_or_insert_with(Block::new)
    }

    #[inline]
    fn get(&self, row: usize) -> Option<(&Block, usize)> {
        let b = self.blocks.get(row / BLOCK_ROWS)?.as_deref()?;
        Some((b, row % BLOCK_ROWS))
    }

    pub fn allocated_blocks(&self) -> usize {
        self.blocks.iter().filter(|b| b.is_some()).count()
    }
}

/// Contagem de células por tipo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellCounts {
    pub numbers: u64,
    pub shared_strings: u64,
    pub local_strings: u64,
    pub bools: u64,
    pub errors: u64,
    /// Células sem valor mas com estilo (bordas, fundo...).
    pub styled_empty: u64,
    pub formulas: u64,
}

impl CellCounts {
    pub fn values(&self) -> u64 {
        self.numbers + self.shared_strings + self.local_strings + self.bools + self.errors
    }

    pub fn add(&mut self, o: &CellCounts) {
        self.numbers += o.numbers;
        self.shared_strings += o.shared_strings;
        self.local_strings += o.local_strings;
        self.bools += o.bools;
        self.errors += o.errors;
        self.styled_empty += o.styled_empty;
        self.formulas += o.formulas;
    }
}

// ---------------------------------------------------------------------------
// Partes de aba (montadas em paralelo)
// ---------------------------------------------------------------------------

/// Pedaço de uma aba montado por uma thread (ex.: um intervalo de linhas).
#[derive(Default)]
pub struct SheetPart {
    columns: Vec<Column>,
    /// Strings locais desta parte; índices rebaseados no merge.
    pub strings: StringTable,
    /// Maior linha/coluna + 1 vistas nesta parte.
    pub rows: u32,
    pub cols: u32,
}

impl SheetPart {
    pub fn new() -> Self {
        Self::default()
    }

    /// Grava uma célula (linha e coluna a partir de 0).
    #[inline]
    pub fn set(&mut self, row: u32, col: u32, kind: u8, style: u16, payload: u64) {
        let c = col as usize;
        if c >= self.columns.len() {
            self.columns.resize_with(c + 1, Column::default);
        }
        let r = row as usize;
        let b = self.columns[c].block_mut(r / BLOCK_ROWS);
        let i = r % BLOCK_ROWS;
        b.kind[i] = kind;
        b.style[i] = style;
        b.payload[i] = payload;
        self.rows = self.rows.max(row + 1);
        self.cols = self.cols.max(col + 1);
    }
}

// ---------------------------------------------------------------------------
// Aba
// ---------------------------------------------------------------------------

pub struct Sheet {
    pub name: String,
    pub hidden: bool,
    columns: Vec<Column>,
    /// Strings que não estão na tabela compartilhada (inline, resultado de fórmula).
    pub strings: StringTable,
    rows: u32,
    cols: u32,
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            hidden: false,
            columns: Vec::new(),
            strings: StringTable::new(),
            rows: 0,
            cols: 0,
        }
    }

    /// Número de linhas usadas (maior linha com conteúdo + 1).
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Número de colunas usadas.
    pub fn cols(&self) -> u32 {
        self.cols
    }

    /// Junta uma parte a esta aba. Partes podem se sobrepor nos blocos de
    /// fronteira; as células presentes na parte sobrescrevem as da aba.
    pub fn merge_part(&mut self, mut part: SheetPart) {
        let base = self.strings.append(&part.strings) as u64;
        if self.columns.len() < part.columns.len() {
            self.columns.resize_with(part.columns.len(), Column::default);
        }
        for (c, pcol) in part.columns.iter_mut().enumerate() {
            let col = &mut self.columns[c];
            if col.blocks.len() < pcol.blocks.len() {
                col.blocks.resize_with(pcol.blocks.len(), || None);
            }
            for (bi, slot) in pcol.blocks.iter_mut().enumerate() {
                let Some(mut pb) = slot.take() else { continue };
                if base != 0 {
                    for i in 0..BLOCK_ROWS {
                        if pb.kind[i] & kind::TYPE_MASK == kind::LOCAL_STR {
                            pb.payload[i] += base;
                        }
                    }
                }
                match &mut col.blocks[bi] {
                    None => col.blocks[bi] = Some(pb),
                    Some(existing) => {
                        for i in 0..BLOCK_ROWS {
                            if pb.present(i) {
                                existing.kind[i] = pb.kind[i];
                                existing.style[i] = pb.style[i];
                                existing.payload[i] = pb.payload[i];
                            }
                        }
                    }
                }
            }
        }
        self.rows = self.rows.max(part.rows);
        self.cols = self.cols.max(part.cols);
    }

    /// Célula em (linha, coluna), a partir de 0.
    pub fn cell<'a>(&'a self, row: u32, col: u32, shared: &'a StringTable) -> Cell<'a> {
        let empty = Cell { value: Value::Empty, style: 0, formula: false };
        let Some(column) = self.columns.get(col as usize) else { return empty };
        let Some((b, i)) = column.get(row as usize) else { return empty };
        let k = b.kind[i];
        let p = b.payload[i];
        let value = match k & kind::TYPE_MASK {
            kind::NUMBER => Value::Number(f64::from_bits(p)),
            kind::SHARED_STR => Value::Text(shared.get(p as u32).unwrap_or("")),
            kind::LOCAL_STR => Value::Text(self.strings.get(p as u32).unwrap_or("")),
            kind::BOOL => Value::Bool(p != 0),
            kind::ERROR => Value::Error(CellError::from_code(p as u8)),
            _ => Value::Empty,
        };
        Cell { value, style: b.style[i], formula: k & kind::FORMULA != 0 }
    }

    /// Conta as células por tipo.
    pub fn counts(&self) -> CellCounts {
        let mut c = CellCounts::default();
        for col in &self.columns {
            for b in col.blocks.iter().flatten() {
                for i in 0..BLOCK_ROWS {
                    let k = b.kind[i];
                    if k & kind::FORMULA != 0 {
                        c.formulas += 1;
                    }
                    match k & kind::TYPE_MASK {
                        kind::NUMBER => c.numbers += 1,
                        kind::SHARED_STR => c.shared_strings += 1,
                        kind::LOCAL_STR => c.local_strings += 1,
                        kind::BOOL => c.bools += 1,
                        kind::ERROR => c.errors += 1,
                        _ if b.style[i] != 0 => c.styled_empty += 1,
                        _ => {}
                    }
                }
            }
        }
        c
    }

    pub fn allocated_blocks(&self) -> usize {
        self.columns.iter().map(Column::allocated_blocks).sum()
    }

    /// Memória ocupada pelos dados desta aba.
    pub fn heap_bytes(&self) -> usize {
        self.allocated_blocks() * std::mem::size_of::<Block>()
            + self.columns.iter().map(|c| c.blocks.capacity() * 8).sum::<usize>()
            + self.strings.heap_bytes()
    }
}

// ---------------------------------------------------------------------------
// Workbook
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    /// Strings compartilhadas (`sharedStrings.xml`).
    pub shared_strings: StringTable,
    pub styles: Styles,
    /// Datas contadas a partir de 1904 (Mac antigo) em vez de 1900.
    pub date1904: bool,
}

impl Workbook {
    pub fn cell(&self, sheet: usize, row: u32, col: u32) -> Cell<'_> {
        self.sheets[sheet].cell(row, col, &self.shared_strings)
    }

    pub fn heap_bytes(&self) -> usize {
        self.sheets.iter().map(Sheet::heap_bytes).sum::<usize>() + self.shared_strings.heap_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_table_roundtrip_and_append() {
        let mut a = StringTable::new();
        assert_eq!(a.push("olá"), 0);
        assert_eq!(a.push(""), 1);
        assert_eq!(a.push("mundo"), 2);
        let mut b = StringTable::new();
        b.push("x");
        let base = a.append(&b);
        assert_eq!(base, 3);
        assert_eq!(a.get(0), Some("olá"));
        assert_eq!(a.get(1), Some(""));
        assert_eq!(a.get(2), Some("mundo"));
        assert_eq!(a.get(3), Some("x"));
        assert_eq!(a.get(4), None);
    }

    #[test]
    fn parts_merge_in_order_with_boundary_blocks() {
        // Duas partes que dividem o mesmo bloco (linhas 1000..1100).
        let mut p1 = SheetPart::new();
        let s = p1.strings.push("a");
        p1.set(1000, 0, kind::LOCAL_STR, 3, s as u64);
        p1.set(1023, 1, kind::NUMBER, 0, 1.5f64.to_bits());
        let mut p2 = SheetPart::new();
        let s = p2.strings.push("b");
        p2.set(1024, 0, kind::LOCAL_STR, 0, s as u64);
        p2.set(1010, 2, kind::BOOL | kind::FORMULA, 0, 1);

        let shared = StringTable::new();
        let mut sheet = Sheet::new("s");
        sheet.merge_part(p1);
        sheet.merge_part(p2);
        assert_eq!(sheet.cell(1000, 0, &shared).value, Value::Text("a"));
        assert_eq!(sheet.cell(1000, 0, &shared).style, 3);
        assert_eq!(sheet.cell(1024, 0, &shared).value, Value::Text("b"));
        assert_eq!(sheet.cell(1023, 1, &shared).value, Value::Number(1.5));
        let c = sheet.cell(1010, 2, &shared);
        assert_eq!(c.value, Value::Bool(true));
        assert!(c.formula);
        assert_eq!(sheet.cell(5, 5, &shared).value, Value::Empty);
        assert_eq!((sheet.rows(), sheet.cols()), (1025, 3));
        let counts = sheet.counts();
        assert_eq!(counts.local_strings, 2);
        assert_eq!(counts.formulas, 1);
    }

    #[test]
    fn block_is_11_bytes_per_cell() {
        assert_eq!(std::mem::size_of::<Block>(), BLOCK_ROWS * 11);
    }
}
