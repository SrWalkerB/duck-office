//! `duck-core`: o modelo de dados do workbook (ADR-004).
//!
//! - **Colunar em blocos**: cada coluna é uma lista de blocos de
//!   [`BLOCK_ROWS`] linhas. Blocos vazios não alocam.
//! - Na montagem, cada célula ocupa 11 bytes dentro do bloco: tipo (`u8`),
//!   estilo (`u16`) e um payload de 64 bits (número `f64`, índice de string,
//!   booleano ou erro). Assim que um bloco não muda mais, ele é compactado:
//!   estilo único vira um valor só, payloads que cabem em 32 bits (inteiros,
//!   índices) usam 4 bytes, e blocos só com estilo não guardam payload. Isso
//!   acontece já durante a leitura ([`SheetPart::pack_inner`] e
//!   [`Sheet::merge_part`]); [`Sheet::finish`] compacta o que sobrou.
//! - Strings ficam em tabelas contíguas ([`StringTable`]); células guardam
//!   só o índice.
//! - Partes da mesma aba podem ser montadas em paralelo ([`SheetPart`]) e
//!   juntadas depois ([`Sheet::merge_part`]) em ordem fixa, então o resultado
//!   é idêntico com qualquer número de threads (ADR-012).

pub mod cond;
pub mod cube;
pub mod drawing;
pub mod edit;
pub mod input;
mod layout;
pub mod numfmt;
pub mod outline;
pub mod pivot;
pub mod shape;
pub mod shift;
mod styles;

use std::collections::HashMap;

pub use cond::{
    Bar, BarAxis, BarLook, CfApply, CfCtx, CfKind, CfOp, CfRule, CfState, Cfvo, CfvoKind, ExtId, Period,
    TextOp,
};
pub use edit::{CellChange, CellValue, Command, Edits, OpsHandle, RawCell};
pub use layout::{
    AutoFilter, CellRange, ColSpec, ControlRef, FilterColumn, FilterKind, Hyperlink, Layout, Pane, RowSpec,
    SheetView, Table,
};
pub use numfmt::{Formatted, NumFmt};
pub use shift::{Axis, FormulaOps, Shift};
pub use styles::{
    Alignment, Border, BorderSide, BorderStyle, CellXf, Color, Dxf, FileCounts, Fill, Font, HAlign, RichRun,
    RunFont, Styles, Theme, VAlign, apply_tint, rotation_angle,
};

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
    /// Trechos das strings com texto rico (várias fontes), por índice, em ordem.
    rich: Vec<(u32, Box<[RichRun]>)>,
}

impl StringTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(strings: usize, bytes: usize) -> Self {
        Self { data: String::with_capacity(bytes), ends: Vec::with_capacity(strings), rich: Vec::new() }
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
        self.rich.extend(other.rich.iter().map(|(i, r)| (i + base, r.clone())));
        base
    }

    /// Guarda os trechos de texto rico da string `i`.
    pub fn set_rich(&mut self, i: u32, runs: Vec<RichRun>) {
        let at = self.rich.partition_point(|r| r.0 < i);
        if self.rich.get(at).is_some_and(|r| r.0 == i) {
            self.rich[at].1 = runs.into();
        } else {
            self.rich.insert(at, (i, runs.into()));
        }
    }

    /// Trechos de texto rico da string `i`, se ela tiver.
    pub fn rich(&self, i: u32) -> Option<&[RichRun]> {
        let k = self.rich.binary_search_by_key(&i, |r| r.0).ok()?;
        Some(&self.rich[k].1)
    }

    /// Quantas strings têm texto rico.
    pub fn rich_count(&self) -> usize {
        self.rich.len()
    }

    /// Índice de um `&str` devolvido por [`StringTable::get`] desta tabela.
    pub fn index_of(&self, s: &str) -> Option<u32> {
        let (lo, hi) = self.byte_range();
        let p = s.as_ptr() as usize;
        if p < lo || p > hi {
            return None;
        }
        let off = (p - lo) as u64;
        let i = self.ends.partition_point(|&e| e <= off);
        (self.get(i as u32)?.as_ptr() == s.as_ptr()).then_some(i as u32)
    }

    /// Trechos de texto rico de um `&str` devolvido por esta tabela.
    #[inline]
    pub fn rich_of(&self, s: &str) -> Option<&[RichRun]> {
        if self.rich.is_empty() || s.is_empty() {
            return None;
        }
        self.rich(self.index_of(s)?)
    }

    /// Endereços [início, fim) do texto da tabela. Um `&str` devolvido por
    /// [`StringTable::get`] cai dentro desse intervalo: serve para saber de que
    /// tabela veio o texto de uma célula sem guardar o índice.
    pub fn byte_range(&self) -> (usize, usize) {
        let p = self.data.as_ptr() as usize;
        (p, p + self.data.len())
    }

    pub fn heap_bytes(&self) -> usize {
        self.data.capacity()
            + self.ends.capacity() * 8
            + self.rich.iter().map(|r| r.1.len() * std::mem::size_of::<RichRun>() + 16).sum::<usize>()
    }

    /// Devolve a folga dos buffers (depois de montar).
    pub fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
        self.ends.shrink_to_fit();
        self.rich.shrink_to_fit();
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

/// 1024 linhas de uma coluna durante a montagem: 11 KB, escrita direta.
/// Alocado só se a coluna tiver algo nessas linhas.
pub struct Block {
    pub kind: [u8; BLOCK_ROWS],
    pub style: [u16; BLOCK_ROWS],
    pub payload: [u64; BLOCK_ROWS],
}

impl Block {
    fn new() -> Box<Self> {
        // Memória já zerada direto do alocador, sem montar 11 KB na pilha e copiar.
        // SAFETY: `Block` só tem inteiros; todos os bits zero é um valor válido.
        unsafe { Box::<Self>::new_zeroed().assume_init() }
    }

    #[inline]
    fn present(&self, i: usize) -> bool {
        self.kind[i] != kind::EMPTY || self.style[i] != 0
    }
}

/// Array de inteiros zerado direto no heap.
fn zeroed_lane<T: Copy>() -> Box<[T; BLOCK_ROWS]> {
    // SAFETY: só é usado com inteiros, para os quais todos os bits zero é válido.
    unsafe { Box::<[T; BLOCK_ROWS]>::new_zeroed().assume_init() }
}

/// Payload em 32 bits, se couber sem perda: números inteiros na faixa de
/// `i32` (sem o -0), strings locais com tabela < 2^12 e índice < 2^20 (uma
/// tabela por parte de 8 MB de XML: a aba de 1,6 GB tem centenas), e o resto
/// (índices, booleanos, erros) se for < 2^32.
#[inline]
fn narrow(k: u8, p: u64) -> Option<u32> {
    match k & kind::TYPE_MASK {
        kind::NUMBER => {
            let i = f64::from_bits(p) as i32;
            ((i as f64).to_bits() == p).then_some(i as u32)
        }
        kind::LOCAL_STR => {
            let (table, index) = (p >> 32, p & 0xffff_ffff);
            (table < 1 << TABLE_BITS && index < 1 << INDEX_BITS)
                .then_some(((table << INDEX_BITS) | index) as u32)
        }
        _ => u32::try_from(p).ok(),
    }
}

/// Bits de tabela e de índice de uma string local compactada ([`narrow`]).
const TABLE_BITS: u32 = 12;
const INDEX_BITS: u32 = 20;

/// Inverso de [`narrow`].
#[inline]
fn widen(k: u8, v: u32) -> u64 {
    match k & kind::TYPE_MASK {
        kind::NUMBER => (v as i32 as f64).to_bits(),
        kind::LOCAL_STR => (((v >> INDEX_BITS) as u64) << 32) | (v & ((1 << INDEX_BITS) - 1)) as u64,
        _ => v as u64,
    }
}

/// Estilos de um bloco compactado.
enum StyleLane {
    Same(u16),
    Each(Box<[u16; BLOCK_ROWS]>),
}

/// Payloads de um bloco compactado.
enum PayloadLane {
    /// Todos zero (blocos só com estilo, ou só zeros e índices 0).
    Zero,
    /// Todos cabem em 32 bits ([`narrow`]).
    Narrow(Box<[u32; BLOCK_ROWS]>),
    Wide(Box<[u64; BLOCK_ROWS]>),
}

/// Bloco depois da montagem: 1 KB de tipos mais o que não se repete.
struct Packed {
    kind: [u8; BLOCK_ROWS],
    style: StyleLane,
    payload: PayloadLane,
}

impl Packed {
    fn from_block(b: &Block) -> Self {
        let s0 = b.style[0];
        let style = if b.style.iter().all(|&s| s == s0) {
            StyleLane::Same(s0)
        } else {
            let mut each = zeroed_lane::<u16>();
            each.copy_from_slice(&b.style);
            StyleLane::Each(each)
        };
        let payload = if b.payload.iter().all(|&p| p == 0) {
            PayloadLane::Zero
        } else {
            let mut small = zeroed_lane::<u32>();
            let fits = (0..BLOCK_ROWS).all(|i| match narrow(b.kind[i], b.payload[i]) {
                Some(v) => {
                    small[i] = v;
                    true
                }
                None => false,
            });
            if fits {
                PayloadLane::Narrow(small)
            } else {
                let mut wide = zeroed_lane::<u64>();
                wide.copy_from_slice(&b.payload);
                PayloadLane::Wide(wide)
            }
        };
        Self { kind: b.kind, style, payload }
    }

    fn to_block(&self) -> Box<Block> {
        let mut b = Block::new();
        b.kind = self.kind;
        for i in 0..BLOCK_ROWS {
            b.style[i] = self.style(i);
            b.payload[i] = self.payload(i);
        }
        b
    }

    #[inline]
    fn style(&self, i: usize) -> u16 {
        match &self.style {
            StyleLane::Same(s) => *s,
            StyleLane::Each(a) => a[i],
        }
    }

    #[inline]
    fn payload(&self, i: usize) -> u64 {
        match &self.payload {
            PayloadLane::Zero => 0,
            PayloadLane::Narrow(a) => widen(self.kind[i], a[i]),
            PayloadLane::Wide(a) => a[i],
        }
    }

    fn heap_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self.style {
                StyleLane::Same(_) => 0,
                StyleLane::Each(_) => BLOCK_ROWS * 2,
            }
            + match self.payload {
                PayloadLane::Zero => 0,
                PayloadLane::Narrow(_) => BLOCK_ROWS * 4,
                PayloadLane::Wide(_) => BLOCK_ROWS * 8,
            }
    }
}

/// Um bloco em montagem ou já compactado.
enum Slot {
    Wide(Box<Block>),
    Packed(Box<Packed>),
}

impl Slot {
    #[inline]
    fn kinds(&self) -> &[u8; BLOCK_ROWS] {
        match self {
            Slot::Wide(b) => &b.kind,
            Slot::Packed(p) => &p.kind,
        }
    }

    #[inline]
    fn kind(&self, i: usize) -> u8 {
        self.kinds()[i]
    }

    #[inline]
    fn style(&self, i: usize) -> u16 {
        match self {
            Slot::Wide(b) => b.style[i],
            Slot::Packed(p) => p.style(i),
        }
    }

    #[inline]
    fn payload(&self, i: usize) -> u64 {
        match self {
            Slot::Wide(b) => b.payload[i],
            Slot::Packed(p) => p.payload(i),
        }
    }

    /// O bloco para escrita (descompacta, se preciso).
    fn wide_mut(&mut self) -> &mut Block {
        if let Slot::Packed(p) = self {
            *self = Slot::Wide(p.to_block());
        }
        match self {
            Slot::Wide(b) => b,
            Slot::Packed(_) => unreachable!(),
        }
    }

    fn pack(&mut self) {
        if let Slot::Wide(b) = self {
            *self = Slot::Packed(Box::new(Packed::from_block(b)));
        }
    }

    /// Aponta as strings locais do bloco para a tabela `table` da aba (a parte
    /// só conhece a própria tabela, a 0). Um bloco compactado continua assim.
    fn retag(&mut self, table: u64) {
        let local = |k: &u8| k & kind::TYPE_MASK == kind::LOCAL_STR;
        if table == 0 || !self.kinds().iter().any(local) {
            return;
        }
        if let Slot::Packed(p) = self
            && let Packed { kind: kinds, payload: PayloadLane::Narrow(a), .. } = &mut **p
            && table < 1 << TABLE_BITS
        {
            // Os índices da parte já cabem em INDEX_BITS (senão o lane seria `Wide`).
            for (v, k) in a.iter_mut().zip(kinds.iter()) {
                if local(k) {
                    *v |= (table as u32) << INDEX_BITS;
                }
            }
            return;
        }
        let packed = matches!(self, Slot::Packed(_));
        let b = self.wide_mut();
        for i in 0..BLOCK_ROWS {
            if local(&b.kind[i]) {
                b.payload[i] |= table << 32;
            }
        }
        if packed {
            self.pack();
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Slot::Wide(_) => std::mem::size_of::<Block>(),
            Slot::Packed(p) => p.heap_bytes(),
        }
    }
}

/// Id de fórmula de uma célula: 0 = sem fórmula; com o bit alto, membro do
/// grupo compartilhado `si`; senão, índice + 1 em `Sheet::formula_defs`.
const SHARED_BIT: u32 = 0x8000_0000;

/// Ids de fórmula de um bloco. Fórmulas copiadas para baixo costumam ter o
/// mesmo id no bloco inteiro (modelo deduplicado ou grupo compartilhado).
enum Ids {
    Same(u32),
    Each(Box<[u32; BLOCK_ROWS]>),
}

impl Ids {
    #[inline]
    fn get(&self, i: usize) -> u32 {
        match self {
            Ids::Same(id) => *id,
            Ids::Each(a) => a[i],
        }
    }

    fn each_mut(&mut self) -> &mut [u32; BLOCK_ROWS] {
        if let Ids::Same(id) = *self {
            let mut a = zeroed_lane::<u32>();
            a.fill(id);
            *self = Ids::Each(a);
        }
        match self {
            Ids::Each(a) => a,
            Ids::Same(_) => unreachable!(),
        }
    }

    /// Troca cada id por `f(id)`, sem descompactar.
    fn map(&mut self, f: impl Fn(u32) -> u32) {
        match self {
            Ids::Same(id) => *id = f(*id),
            Ids::Each(a) => a.iter_mut().for_each(|id| *id = f(*id)),
        }
    }

    fn pack(&mut self) {
        if let Ids::Each(a) = self {
            let id0 = a[0];
            if a.iter().all(|&id| id == id0) {
                *self = Ids::Same(id0);
            }
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Ids::Same(_) => 0,
            Ids::Each(_) => BLOCK_ROWS * 4,
        }
    }
}

#[derive(Default)]
pub struct Column {
    blocks: Vec<Option<Slot>>,
    /// Ids de fórmula, alocados só nos blocos que têm fórmula.
    formulas: Vec<Option<Ids>>,
}

impl Column {
    #[inline]
    fn block_mut(&mut self, bi: usize) -> &mut Block {
        if bi >= self.blocks.len() {
            self.blocks.resize_with(bi + 1, || None);
        }
        self.blocks[bi].get_or_insert_with(|| Slot::Wide(Block::new())).wide_mut()
    }

    #[inline]
    fn formula_block_mut(&mut self, bi: usize) -> &mut [u32; BLOCK_ROWS] {
        if bi >= self.formulas.len() {
            self.formulas.resize_with(bi + 1, || None);
        }
        self.formulas[bi].get_or_insert_with(|| Ids::Each(zeroed_lane::<u32>())).each_mut()
    }

    #[inline]
    fn get(&self, row: usize) -> Option<(&Slot, usize)> {
        let b = self.blocks.get(row / BLOCK_ROWS)?.as_ref()?;
        Some((b, row % BLOCK_ROWS))
    }

    #[inline]
    fn formula_id(&self, row: usize) -> u32 {
        match self.formulas.get(row / BLOCK_ROWS) {
            Some(Some(f)) => f.get(row % BLOCK_ROWS),
            _ => 0,
        }
    }

    pub fn allocated_blocks(&self) -> usize {
        self.blocks.iter().filter(|b| b.is_some()).count()
    }

    /// Compacta os blocos de `range` que existirem.
    fn pack_range(&mut self, range: std::ops::Range<usize>) {
        let n = self.blocks.len();
        for b in self.blocks[range.start.min(n)..range.end.min(n)].iter_mut().flatten() {
            b.pack();
        }
        let n = self.formulas.len();
        for f in self.formulas[range.start.min(n)..range.end.min(n)].iter_mut().flatten() {
            f.pack();
        }
    }

    /// A linha `r0 + i` recebe o que estava em `r0 + perm[i]` (classificar).
    fn permute(&mut self, r0: usize, perm: &[u32]) {
        let n = perm.len();
        let get = |c: &Column, r: usize| -> (u8, u16, u64, u32) {
            let f = c.formula_id(r);
            match c.get(r) {
                Some((b, i)) => (b.kind(i), b.style(i), b.payload(i), f),
                None => (0, 0, 0, f),
            }
        };
        let old: Vec<(u8, u16, u64, u32)> = (0..n).map(|i| get(self, r0 + i)).collect();
        if old.iter().all(|x| *x == (0, 0, 0, 0)) {
            return;
        }
        let has_ids = old.iter().any(|x| x.3 != 0);
        for (i, &p) in perm.iter().enumerate() {
            let (k, s, pl, f) = old[p as usize];
            let r = r0 + i;
            let (bi, j) = (r / BLOCK_ROWS, r % BLOCK_ROWS);
            let exists = self.blocks.get(bi).is_some_and(Option::is_some);
            if exists || k != 0 || s != 0 {
                let b = self.block_mut(bi);
                b.kind[j] = k;
                b.style[j] = s;
                b.payload[j] = pl;
            }
            if has_ids && (f != 0 || matches!(self.formulas.get(bi), Some(Some(_)))) {
                self.formula_block_mut(bi)[j] = f;
            }
        }
        self.pack_range(r0 / BLOCK_ROWS..(r0 + n).div_ceil(BLOCK_ROWS));
    }

    /// Compacta os blocos (fim da montagem; ver [`Sheet::finish_with`]).
    pub fn pack(&mut self) {
        for b in self.blocks.iter_mut().flatten() {
            b.pack();
        }
        for f in self.formulas.iter_mut().flatten() {
            f.pack();
        }
        self.blocks.shrink_to_fit();
        self.formulas.shrink_to_fit();
    }

    fn heap_bytes(&self) -> usize {
        self.blocks.capacity() * std::mem::size_of::<Option<Slot>>()
            + self.formulas.capacity() * std::mem::size_of::<Option<Ids>>()
            + self.blocks.iter().flatten().map(Slot::heap_bytes).sum::<usize>()
            + self.formulas.iter().flatten().map(Ids::heap_bytes).sum::<usize>()
    }
}

// ---------------------------------------------------------------------------
// Fórmulas
// ---------------------------------------------------------------------------

/// Como a fórmula foi gravada no arquivo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormulaKind {
    Normal,
    /// Fórmula de matriz que ocupa `range`: matricial antiga (CSE, área
    /// fixa) ou dinâmica (`dynamic`, `cm` no arquivo: a área acompanha o
    /// tamanho do resultado e o resultado "se espalha" pelas vizinhas).
    Array {
        range: CellRange,
        dynamic: bool,
    },
    /// Célula-mestre de uma fórmula compartilhada (`si`); as outras células do
    /// grupo reaproveitam o texto deslocando as referências relativas.
    Shared {
        si: u32,
    },
    /// Tabela de dados (Análise de Hipóteses).
    DataTable,
}

#[derive(Clone, Copy, Debug)]
struct FormulaDef {
    text: u32,
    kind: FormulaKind,
    row: u32,
    col: u32,
    /// `text` é um modelo relativo, compartilhado por várias células.
    template: bool,
}

/// Fórmula de uma célula como está no arquivo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormulaRef<'a> {
    /// Texto da fórmula (sem o `=`). Para membros de grupo compartilhado, é o
    /// texto da célula-mestre: as referências relativas precisam ser
    /// deslocadas de (`anchor_row`, `anchor_col`) até a célula
    /// (`duck_calc::cell_formula` faz isso).
    pub text: &'a str,
    pub kind: FormulaKind,
    pub anchor_row: u32,
    pub anchor_col: u32,
    /// `text` é um modelo relativo (`duck_calc::from_template`), não texto A1.
    pub template: bool,
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
    formula_texts: StringTable,
    formula_defs: Vec<FormulaDef>,
    /// Modelo relativo → índice em `formula_defs` (deduplicação dentro da parte).
    templates: HashMap<Box<str>, u32>,
    /// Larguras, alturas, mesclagens... vistas neste pedaço.
    pub layout: Layout,
    /// Maior linha/coluna + 1 vistas nesta parte.
    pub rows: u32,
    pub cols: u32,
    /// Menor linha vista nesta parte (`None` = parte sem células).
    first_row: Option<u32>,
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
        self.first_row = Some(self.first_row.map_or(row, |f| f.min(row)));
    }

    /// Compacta os blocos que só esta parte pode ter: os que ficam inteiros
    /// entre a menor e a maior linha dela (as partes de uma aba cobrem faixas
    /// de linhas que não se cruzam). Os das pontas ficam para o merge.
    ///
    /// Feito na thread que leu a parte, em paralelo: a aba é montada já
    /// compactada, em vez de ocupar o dobro até o fim da leitura.
    pub fn pack_inner(&mut self) {
        let Some(first) = self.first_row else { return };
        let lo = (first as usize).div_ceil(BLOCK_ROWS);
        let hi = self.rows as usize / BLOCK_ROWS;
        if lo < hi {
            for col in &mut self.columns {
                col.pack_range(lo..hi);
            }
        }
    }

    #[inline]
    fn set_formula_id(&mut self, row: u32, col: u32, id: u32) {
        let c = col as usize;
        if c >= self.columns.len() {
            self.columns.resize_with(c + 1, Column::default);
        }
        let r = row as usize;
        self.columns[c].formula_block_mut(r / BLOCK_ROWS)[r % BLOCK_ROWS] = id;
        self.rows = self.rows.max(row + 1);
        self.first_row = Some(self.first_row.map_or(row, |f| f.min(row)));
    }

    /// Fórmula com texto próprio (normal, matricial, tabela de dados ou mestre
    /// de grupo compartilhado).
    pub fn set_formula(&mut self, row: u32, col: u32, text: &str, kind: FormulaKind) {
        let text = self.formula_texts.push(text);
        self.formula_defs.push(FormulaDef { text, kind, row, col, template: false });
        let id = self.formula_defs.len() as u32;
        self.set_formula_id(row, col, id);
    }

    /// Fórmula normal guardada como modelo relativo (deduplicado): fórmulas
    /// copiadas para baixo viram uma entrada só.
    /// Retorna o id da fórmula, que pode ser repassado a [`Self::reuse_formula`].
    pub fn set_formula_template(&mut self, row: u32, col: u32, template: &str) -> u32 {
        let idx = match self.templates.get(template) {
            Some(&i) => i,
            None => {
                let text = self.formula_texts.push(template);
                let i = self.formula_defs.len() as u32;
                self.formula_defs.push(FormulaDef {
                    text,
                    kind: FormulaKind::Normal,
                    row: 0,
                    col: 0,
                    template: true,
                });
                self.templates.insert(template.into(), i);
                i
            }
        };
        self.set_formula_id(row, col, idx + 1);
        idx + 1
    }

    /// Dá à célula a mesma fórmula (id de [`Self::set_formula_template`]) de
    /// outra célula desta parte. Só vale para modelos sem referência, que não
    /// dependem da posição.
    pub fn reuse_formula(&mut self, row: u32, col: u32, id: u32) {
        self.set_formula_id(row, col, id);
    }

    /// Célula que reaproveita a fórmula compartilhada `si`.
    pub fn set_shared_formula(&mut self, row: u32, col: u32, si: u32) {
        self.set_formula_id(row, col, SHARED_BIT | (si & !SHARED_BIT));
    }
}

// ---------------------------------------------------------------------------
// Aba
// ---------------------------------------------------------------------------

pub struct Sheet {
    pub name: String,
    pub hidden: bool,
    /// `sheetId` no arquivo (as segmentações apontam por ele).
    pub tab_id: u32,
    columns: Vec<Column>,
    /// Strings que não estão na tabela compartilhada (inline, resultado de
    /// fórmula): uma tabela por parte, movida sem cópia no merge. A célula
    /// guarda `(tabela << 32) | índice`.
    strings: Vec<StringTable>,
    formula_texts: StringTable,
    formula_defs: Vec<FormulaDef>,
    /// `si` → índice da célula-mestre em `formula_defs`.
    shared_groups: HashMap<u32, u32>,
    /// Modelo relativo → índice em `formula_defs` (deduplicação entre partes).
    templates: HashMap<Box<str>, u32>,
    /// Larguras, alturas, mesclagens, painéis congelados.
    pub layout: Layout,
    rows: u32,
    cols: u32,
    /// Os blocos abaixo deste índice já foram compactados no merge.
    sealed: usize,
    /// O que mudou desde o último salvamento (ver `edit.rs`).
    edits: Edits,
    /// Índice em `strings` da tabela do texto digitado.
    edit_table: Option<usize>,
    /// Blocos (coluna, bloco) escritos desde o último [`Sheet::settle`].
    touched: Vec<(u32, u32)>,
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            hidden: false,
            tab_id: 0,
            columns: Vec::new(),
            strings: Vec::new(),
            formula_texts: StringTable::new(),
            formula_defs: Vec::new(),
            shared_groups: HashMap::new(),
            templates: HashMap::new(),
            layout: Layout::default(),
            rows: 0,
            cols: 0,
            sealed: 0,
            edits: Edits::default(),
            edit_table: None,
            touched: Vec::new(),
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
        // As strings da parte viram mais uma tabela da aba (sem copiar).
        let table = if part.strings.is_empty() { 0 } else { self.strings.len() as u64 };
        if !part.strings.is_empty() {
            self.strings.push(std::mem::take(&mut part.strings));
        }
        // Fórmulas da parte: modelos já conhecidos reaproveitam a entrada da
        // aba; o resto é anexado. `remap[local] = global`.
        let mut remap = Vec::with_capacity(part.formula_defs.len());
        for d in &part.formula_defs {
            let text = part.formula_texts.get(d.text).unwrap_or("");
            if d.template
                && let Some(&g) = self.templates.get(text)
            {
                remap.push(g);
                continue;
            }
            let g = self.formula_defs.len() as u32;
            let t = self.formula_texts.push(text);
            self.formula_defs.push(FormulaDef { text: t, ..*d });
            if d.template {
                self.templates.insert(text.into(), g);
            }
            if let FormulaKind::Shared { si } = d.kind {
                self.shared_groups.insert(si, g);
            }
            remap.push(g);
        }
        let identity = remap.iter().enumerate().all(|(i, &g)| g as usize == i);
        if self.columns.len() < part.columns.len() {
            self.columns.resize_with(part.columns.len(), Column::default);
        }
        for (c, pcol) in part.columns.iter_mut().enumerate() {
            let col = &mut self.columns[c];
            if col.blocks.len() < pcol.blocks.len() {
                col.blocks.resize_with(pcol.blocks.len(), || None);
            }
            for (bi, slot) in pcol.blocks.iter_mut().enumerate() {
                let Some(mut pslot) = slot.take() else { continue };
                pslot.retag(table);
                match &mut col.blocks[bi] {
                    None => col.blocks[bi] = Some(pslot),
                    Some(existing) => {
                        let pb = pslot.wide_mut();
                        let existing = existing.wide_mut();
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
            if col.formulas.len() < pcol.formulas.len() {
                col.formulas.resize_with(pcol.formulas.len(), || None);
            }
            for (bi, slot) in pcol.formulas.iter_mut().enumerate() {
                let Some(mut pids) = slot.take() else { continue };
                if !identity {
                    pids.map(
                        |id| if id != 0 && id & SHARED_BIT == 0 { remap[id as usize - 1] + 1 } else { id },
                    );
                }
                match &mut col.formulas[bi] {
                    None => col.formulas[bi] = Some(pids),
                    Some(existing) => {
                        for (i, e) in existing.each_mut().iter_mut().enumerate() {
                            let n = pids.get(i);
                            if n != 0 {
                                *e = n;
                            }
                        }
                    }
                }
            }
        }
        self.layout.absorb(std::mem::take(&mut part.layout));
        self.rows = self.rows.max(part.rows);
        self.cols = self.cols.max(part.cols);
        // As partes chegam na ordem do arquivo, e as linhas de uma aba vêm em
        // ordem crescente: os blocos inteiros antes da primeira linha desta
        // parte não mudam mais. Se um arquivo fugir disso, escrever num bloco
        // compactado o descompacta: fica mais lento, nunca errado.
        if let Some(first) = part.first_row {
            let upto = first as usize / BLOCK_ROWS;
            if upto > self.sealed {
                for col in &mut self.columns {
                    col.pack_range(self.sealed..upto);
                }
                self.sealed = upto;
            }
        }
    }

    /// Termina a montagem: ordena o layout e compacta os blocos (a aba fica
    /// com cerca de metade da memória). Chamado depois do último merge; um
    /// merge depois disso ainda funciona (descompacta os blocos que tocar).
    pub fn finish(&mut self) {
        self.finish_with(|cols| cols.iter_mut().for_each(Column::pack));
    }

    /// Como [`Sheet::finish`], com a compactação das colunas feita por
    /// `pack` (ex.: em paralelo, uma coluna por tarefa).
    pub fn finish_with(&mut self, pack: impl FnOnce(&mut [Column])) {
        self.layout.finish();
        pack(&mut self.columns);
        self.columns.shrink_to_fit();
        for t in &mut self.strings {
            t.shrink_to_fit();
        }
        self.strings.shrink_to_fit();
        self.formula_texts.shrink_to_fit();
        self.formula_defs.shrink_to_fit();
        // Só serve para deduplicar modelos entre partes.
        self.templates = HashMap::new();
    }

    /// Célula em (linha, coluna), a partir de 0.
    pub fn cell<'a>(&'a self, row: u32, col: u32, shared: &'a StringTable) -> Cell<'a> {
        let empty = Cell { value: Value::Empty, style: 0, formula: false };
        let Some(column) = self.columns.get(col as usize) else { return empty };
        let Some((b, i)) = column.get(row as usize) else { return empty };
        self.cell_in(b, i, shared)
    }

    /// Célula `i` de um bloco já encontrado.
    #[inline]
    fn cell_in<'a>(&'a self, b: &Slot, i: usize, shared: &'a StringTable) -> Cell<'a> {
        let k = b.kind(i);
        let p = b.payload(i);
        let value = match k & kind::TYPE_MASK {
            kind::NUMBER => Value::Number(f64::from_bits(p)),
            kind::SHARED_STR => Value::Text(shared.get(p as u32).unwrap_or("")),
            kind::LOCAL_STR => {
                Value::Text(self.strings.get((p >> 32) as usize).and_then(|t| t.get(p as u32)).unwrap_or(""))
            }
            kind::BOOL => Value::Bool(p != 0),
            kind::ERROR => Value::Error(CellError::from_code(p as u8)),
            _ => Value::Empty,
        };
        Cell { value, style: b.style(i), formula: k & kind::FORMULA != 0 }
    }

    /// Visita, em ordem, as células com valor da coluna `col` nas linhas
    /// `rows`, pulando blocos vazios. `f` devolve `false` para parar.
    pub fn for_each_value<'a>(
        &'a self,
        col: u32,
        rows: std::ops::Range<u32>,
        shared: &'a StringTable,
        mut f: impl FnMut(u32, Cell<'a>) -> bool,
    ) {
        let Some(column) = self.columns.get(col as usize) else { return };
        let end = (rows.end as usize).min(column.blocks.len() * BLOCK_ROWS);
        let mut r = rows.start as usize;
        while r < end {
            let bi = r / BLOCK_ROWS;
            let Some(b) = column.blocks[bi].as_ref() else {
                r = (bi + 1) * BLOCK_ROWS;
                continue;
            };
            let stop = ((bi + 1) * BLOCK_ROWS).min(end);
            for row in r..stop {
                if b.kind(row % BLOCK_ROWS) & kind::TYPE_MASK != kind::EMPTY
                    && !f(row as u32, self.cell_in(b, row % BLOCK_ROWS, shared))
                {
                    return;
                }
            }
            r = stop;
        }
    }

    /// Próxima linha com valor na coluna depois de `row` (exclusive).
    pub fn next_value_row(&self, col: u32, row: u32) -> Option<u32> {
        let column = self.columns.get(col as usize)?;
        let mut r = row as usize + 1;
        while r < column.blocks.len() * BLOCK_ROWS {
            let bi = r / BLOCK_ROWS;
            match column.blocks[bi].as_ref() {
                None => r = (bi + 1) * BLOCK_ROWS,
                Some(b) => {
                    if b.kind(r % BLOCK_ROWS) & kind::TYPE_MASK != kind::EMPTY {
                        return Some(r as u32);
                    }
                    r += 1;
                }
            }
        }
        None
    }

    /// Linha com valor mais próxima antes de `row` (exclusive) na coluna.
    pub fn prev_value_row(&self, col: u32, row: u32) -> Option<u32> {
        let column = self.columns.get(col as usize)?;
        let mut r = (row as usize).min(column.blocks.len() * BLOCK_ROWS);
        while r > 0 {
            let bi = (r - 1) / BLOCK_ROWS;
            match column.blocks[bi].as_ref() {
                None => r = bi * BLOCK_ROWS,
                Some(b) => {
                    if b.kind((r - 1) % BLOCK_ROWS) & kind::TYPE_MASK != kind::EMPTY {
                        return Some(r as u32 - 1);
                    }
                    r -= 1;
                }
            }
        }
        None
    }

    /// A célula tem valor (não só estilo).
    pub fn has_value(&self, row: u32, col: u32) -> bool {
        self.columns
            .get(col as usize)
            .and_then(|c| c.get(row as usize))
            .is_some_and(|(b, i)| b.kind(i) & kind::TYPE_MASK != kind::EMPTY)
    }

    /// Fórmula da célula como está no arquivo (ver [`FormulaRef`]).
    pub fn formula(&self, row: u32, col: u32) -> Option<FormulaRef<'_>> {
        let id = self.columns.get(col as usize)?.formula_id(row as usize);
        if id == 0 {
            return None;
        }
        let def = if id & SHARED_BIT != 0 {
            let idx = *self.shared_groups.get(&(id & !SHARED_BIT))?;
            self.formula_defs.get(idx as usize)?
        } else {
            self.formula_defs.get(id as usize - 1)?
        };
        Some(FormulaRef {
            text: self.formula_texts.get(def.text)?,
            kind: def.kind,
            anchor_row: def.row,
            anchor_col: def.col,
            template: def.template,
        })
    }

    /// Como [`Sheet::formula`], com o índice da definição: células de um mesmo
    /// modelo relativo ou grupo compartilhado têm o mesmo índice (o motor de
    /// cálculo compila cada definição uma vez). Para modelos, a âncora é a
    /// própria célula.
    pub fn formula_entry(&self, row: u32, col: u32) -> Option<(u32, FormulaRef<'_>)> {
        let id = self.columns.get(col as usize)?.formula_id(row as usize);
        if id == 0 {
            return None;
        }
        let idx = if id & SHARED_BIT != 0 { *self.shared_groups.get(&(id & !SHARED_BIT))? } else { id - 1 };
        let def = self.formula_defs.get(idx as usize)?;
        let (anchor_row, anchor_col) = if def.template { (row, col) } else { (def.row, def.col) };
        Some((
            idx,
            FormulaRef {
                text: self.formula_texts.get(def.text)?,
                kind: def.kind,
                anchor_row,
                anchor_col,
                template: def.template,
            },
        ))
    }

    /// A célula está num grupo de fórmula compartilhada que continua inteiro
    /// (a célula-mestre ainda tem a fórmula do grupo)?
    pub fn shared_intact(&self, row: u32, col: u32) -> bool {
        let Some(f) = self.formula(row, col) else { return false };
        let FormulaKind::Shared { si } = f.kind else { return false };
        let id = self.formula_id(f.anchor_row, f.anchor_col);
        if id == 0 || id & SHARED_BIT != 0 {
            return false;
        }
        self.formula_defs.get(id as usize - 1).is_some_and(|d| {
            d.kind == FormulaKind::Shared { si } && (d.row, d.col) == (f.anchor_row, f.anchor_col)
        })
    }

    /// Id bruto de fórmula da célula (0 = sem fórmula). Células seguidas com o
    /// mesmo id têm a mesma fórmula (modelo relativo ou grupo compartilhado).
    pub fn formula_id(&self, row: u32, col: u32) -> u32 {
        self.columns.get(col as usize).map_or(0, |c| c.formula_id(row as usize))
    }

    /// Visita as células com fórmula da coluna, em ordem, pulando blocos sem
    /// fórmula: `f(linha, id bruto)`.
    pub fn for_each_formula(&self, col: u32, mut f: impl FnMut(u32, u32)) {
        let Some(column) = self.columns.get(col as usize) else { return };
        for (bi, b) in column.formulas.iter().enumerate() {
            let Some(ids) = b else { continue };
            for i in 0..BLOCK_ROWS {
                let id = ids.get(i);
                if id != 0 {
                    f((bi * BLOCK_ROWS + i) as u32, id);
                }
            }
        }
    }

    /// Entradas de fórmula guardadas (modelos deduplicados, mestres de grupos
    /// compartilhados e fórmulas guardadas como texto).
    pub fn formula_texts_stored(&self) -> usize {
        self.formula_defs.len()
    }

    /// Células com fórmula registrada.
    pub fn formula_cells(&self) -> u64 {
        self.columns
            .iter()
            .flat_map(|c| c.formulas.iter().flatten())
            .map(|ids| (0..BLOCK_ROWS).filter(|&i| ids.get(i) != 0).count() as u64)
            .sum()
    }

    /// Posições (linha, coluna) das células com fórmula, coluna a coluna.
    pub fn formula_positions(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.columns.iter().enumerate().flat_map(|(c, col)| {
            col.formulas.iter().enumerate().flat_map(move |(bi, b)| {
                b.iter().flat_map(move |ids| {
                    (0..BLOCK_ROWS)
                        .filter(move |&i| ids.get(i) != 0)
                        .map(move |i| ((bi * BLOCK_ROWS + i) as u32, c as u32))
                })
            })
        })
    }

    /// Conta as células por tipo.
    pub fn counts(&self) -> CellCounts {
        let mut c = CellCounts::default();
        for col in &self.columns {
            for b in col.blocks.iter().flatten() {
                for i in 0..BLOCK_ROWS {
                    let k = b.kind(i);
                    if k & kind::FORMULA != 0 {
                        c.formulas += 1;
                    }
                    match k & kind::TYPE_MASK {
                        kind::NUMBER => c.numbers += 1,
                        kind::SHARED_STR => c.shared_strings += 1,
                        kind::LOCAL_STR => c.local_strings += 1,
                        kind::BOOL => c.bools += 1,
                        kind::ERROR => c.errors += 1,
                        _ if b.style(i) != 0 => c.styled_empty += 1,
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
        self.columns.iter().map(Column::heap_bytes).sum::<usize>()
            + self.strings.iter().map(StringTable::heap_bytes).sum::<usize>()
            + self.formula_texts.heap_bytes()
            + self.formula_defs.capacity() * std::mem::size_of::<FormulaDef>()
            + self.layout.heap_bytes()
    }
}

// ---------------------------------------------------------------------------
// Workbook
// ---------------------------------------------------------------------------

/// Nome definido do workbook (`<definedName>`).
#[derive(Clone, Debug, PartialEq)]
pub struct DefinedName {
    pub name: String,
    /// Aba em que o nome vale (`localSheetId`); `None` = o arquivo todo.
    pub sheet: Option<u32>,
    /// Fórmula do nome, sem o `=` (ex.: `Dados!$A$1:$C$10`).
    pub formula: String,
    pub hidden: bool,
}

/// Outro arquivo referenciado pelas fórmulas (`[1]Aba!A1`), com os valores
/// que o Excel guardou dele (`xl/externalLinks/externalLinkN.xml`).
#[derive(Default)]
pub struct ExternalBook {
    /// Caminho ou nome do arquivo, como está nos relacionamentos.
    pub target: String,
    /// Abas, com os valores guardados (texto como string local).
    pub sheets: Vec<Sheet>,
    /// Nomes definidos do outro arquivo (nome, referência).
    pub names: Vec<(String, String)>,
}

/// O que o motor de cálculo precisa além das abas: nomes definidos e
/// arquivos externos.
#[derive(Default)]
pub struct BookExtras {
    pub names: Vec<DefinedName>,
    pub externals: Vec<ExternalBook>,
    /// Caches das tabelas dinâmicas (sem os registros, lidos sob demanda).
    pub pivot_caches: Vec<pivot::PivotCache>,
    /// Data e hora (serial) em que o arquivo foi salvo, se o arquivo disser
    /// (`docProps/core.xml`): é o "hoje" dos valores guardados.
    pub saved_at: Option<f64>,
}

#[derive(Default)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    /// Strings compartilhadas (`sharedStrings.xml`).
    pub shared_strings: StringTable,
    pub styles: Styles,
    /// Datas contadas a partir de 1904 (Mac antigo) em vez de 1900.
    pub date1904: bool,
    /// Nomes definidos e arquivos externos.
    pub extras: BookExtras,
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
    fn column_scans_skip_empty_blocks() {
        let mut p = SheetPart::new();
        p.set(3, 0, kind::NUMBER, 0, 1.0f64.to_bits());
        p.set(5000, 0, kind::NUMBER, 0, 2.0f64.to_bits());
        p.set(5001, 0, kind::EMPTY, 7, 0); // só estilo: não conta
        p.set(9000, 0, kind::NUMBER, 0, 3.0f64.to_bits());
        let shared = StringTable::new();
        let mut sheet = Sheet::new("s");
        sheet.merge_part(p);
        let mut seen = Vec::new();
        sheet.for_each_value(0, 0..9000, &shared, |r, _| {
            seen.push(r);
            true
        });
        assert_eq!(seen, vec![3, 5000]);
        assert_eq!(sheet.next_value_row(0, 3), Some(5000));
        assert_eq!(sheet.next_value_row(0, 5000), Some(9000));
        assert_eq!(sheet.next_value_row(0, 9000), None);
        assert_eq!(sheet.prev_value_row(0, 9000), Some(5000));
        assert_eq!(sheet.prev_value_row(0, 3), None);
        assert_eq!(sheet.prev_value_row(0, 1_000_000), Some(9000));
        assert!(sheet.has_value(5000, 0));
        assert!(!sheet.has_value(5001, 0));
        assert!(!sheet.has_value(5000, 9));
    }

    #[test]
    fn narrow_roundtrip() {
        let nums: [f64; 6] = [0.0, 1.0, -1.0, 42.0, 2147483647.0, -2147483648.0];
        for v in nums {
            let p = v.to_bits();
            assert_eq!(widen(kind::NUMBER, narrow(kind::NUMBER, p).unwrap()), p, "{v}");
        }
        // Não cabem: fração, -0, fora de i32, NaN, infinito.
        for v in [0.5, -0.0, 2147483648.0, -2147483649.0, f64::NAN, f64::INFINITY, 1e300] {
            assert_eq!(narrow(kind::NUMBER, v.to_bits()), None, "{v}");
        }
        let local = (7u64 << 32) | 123_456;
        assert_eq!(widen(kind::LOCAL_STR, narrow(kind::LOCAL_STR, local).unwrap()), local);
        let many = (4095u64 << 32) | ((1 << 20) - 1);
        assert_eq!(widen(kind::LOCAL_STR, narrow(kind::LOCAL_STR, many).unwrap()), many);
        assert_eq!(narrow(kind::LOCAL_STR, (4096u64 << 32) | 1), None);
        assert_eq!(narrow(kind::LOCAL_STR, 1 << 20), None);
        assert_eq!(widen(kind::SHARED_STR | kind::FORMULA, narrow(kind::SHARED_STR, 99).unwrap()), 99);
    }

    #[test]
    fn finish_packs_without_changing_cells() {
        let mut p = SheetPart::new();
        let a = p.strings.push("a");
        // Coluna 0: estilo único, inteiros e strings (payload de 32 bits).
        for r in 0..1500u32 {
            if r % 3 == 0 {
                p.set(r, 0, kind::NUMBER, 5, (r as f64).to_bits());
            } else {
                p.set(r, 0, kind::LOCAL_STR, 5, a as u64);
            }
        }
        // Coluna 1: números com fração e estilos variados (64 bits).
        for r in 0..1500u32 {
            p.set(r, 1, kind::NUMBER | kind::FORMULA, (r % 4) as u16, (r as f64 + 0.25).to_bits());
            p.set_formula_template(r, 1, "R[0]C[-1]*2");
        }
        // Coluna 2: só estilo (sem payload), booleanos e erros.
        p.set(10, 2, kind::EMPTY, 9, 0);
        p.set(11, 2, kind::BOOL, 0, 1);
        p.set(12, 2, kind::ERROR, 0, CellError::NA as u64);
        p.set(13, 2, kind::NUMBER, 0, (-0.0f64).to_bits());
        let shared = StringTable::new();
        let mut sheet = Sheet::new("s");
        sheet.merge_part(p);
        let snapshot = |s: &Sheet| {
            let mut v = Vec::new();
            for c in 0..3 {
                for r in 0..1600 {
                    v.push((
                        format!("{:?}", s.cell(r, c, &shared)),
                        s.formula(r, c).map(|f| f.text.to_string()),
                    ));
                }
            }
            v
        };
        let before = snapshot(&sheet);
        let bytes_before = sheet.heap_bytes();
        sheet.finish();
        assert_eq!(snapshot(&sheet), before);
        // A coluna 1 (frações, estilos variados) não encolhe; as outras, sim.
        assert!(sheet.heap_bytes() * 5 < bytes_before * 4, "{} → {}", bytes_before, sheet.heap_bytes());
        assert_eq!(sheet.cell(13, 2, &shared).value, Value::Number(-0.0));
        assert!(sheet.cell(13, 2, &shared).value.eq(&Value::Number(0.0)));
        if let Value::Number(z) = sheet.cell(13, 2, &shared).value {
            assert!(z.is_sign_negative());
        }
        // Um merge depois de compactar ainda funciona.
        let mut late = SheetPart::new();
        late.set(1, 0, kind::NUMBER, 5, 7.5f64.to_bits());
        sheet.merge_part(late);
        assert_eq!(sheet.cell(1, 0, &shared).value, Value::Number(7.5));
        assert_eq!(sheet.cell(2, 0, &shared).value, Value::Text("a"));
    }

    /// Aba de teste em partes: `cuts` são as linhas onde cada parte começa.
    /// Cada parte tem a própria tabela de strings locais e modelos de fórmula.
    fn parts(rows: u32, cuts: &[u32]) -> Vec<SheetPart> {
        let mut bounds: Vec<u32> = cuts.to_vec();
        bounds.push(rows);
        bounds
            .windows(2)
            .map(|w| {
                let mut p = SheetPart::new();
                for r in w[0]..w[1] {
                    let s = p.strings.push(&format!("s{r}"));
                    p.set(r, 0, kind::LOCAL_STR, 3, s as u64);
                    p.set(r, 1, kind::NUMBER | kind::FORMULA, (r % 7 == 0) as u16, (r as f64).to_bits());
                    p.set_formula_template(r, 1, "R[0]C[-1]+1");
                    if r % 5 == 0 {
                        p.set(r, 2, kind::NUMBER, 0, (r as f64 + 0.5).to_bits());
                    }
                }
                p
            })
            .collect()
    }

    fn snapshot(s: &Sheet, rows: u32) -> Vec<(String, Option<String>)> {
        let shared = StringTable::new();
        (0..3)
            .flat_map(|c| (0..rows + 10).map(move |r| (r, c)))
            .map(|(r, c)| {
                (format!("{:?}", s.cell(r, c, &shared)), s.formula(r, c).map(|f| f.text.to_string()))
            })
            .collect()
    }

    #[test]
    fn packing_while_merging_keeps_cells() {
        // 300 partes pequenas (300 tabelas de strings: passa das 256 de antes)
        // e depois partes grandes, com blocos inteiros no meio de cada uma.
        let rows = 9000;
        let mut cuts: Vec<u32> = (0..300).map(|i| i * 10).collect();
        cuts.extend([3000, 3500, 5100, 7777]);
        let mut whole = Sheet::new("s");
        whole.merge_part(parts(rows, &[0]).pop().unwrap());
        let expected = snapshot(&whole, rows);

        let mut sheet = Sheet::new("s");
        for mut p in parts(rows, &cuts) {
            p.pack_inner();
            sheet.merge_part(p);
        }
        assert_eq!(snapshot(&sheet, rows), expected);
        // Antes do `finish`, a aba já está compactada: só ficam abertos, em
        // cada coluna, o bloco da fronteira com a última parte e o último.
        let wide = |s: &Sheet| {
            s.columns
                .iter()
                .flat_map(|c| c.blocks.iter().flatten())
                .filter(|b| matches!(b, Slot::Wide(_)))
                .count()
        };
        assert_eq!(wide(&sheet), 6);
        sheet.finish();
        assert_eq!(snapshot(&sheet, rows), expected);
        assert_eq!(wide(&sheet), 0);
        whole.finish();
        assert!(sheet.heap_bytes() < whole.heap_bytes() + 64 * 1024);

        // Fora de ordem (arquivo com linhas fora de ordem): mais lento, mesmo resultado.
        let mut shuffled = Sheet::new("s");
        let mut ps = parts(rows, &cuts);
        ps.reverse();
        for mut p in ps {
            p.pack_inner();
            shuffled.merge_part(p);
        }
        shuffled.finish();
        assert_eq!(snapshot(&shuffled, rows), expected);
    }

    #[test]
    fn block_is_11_bytes_per_cell() {
        assert_eq!(std::mem::size_of::<Block>(), BLOCK_ROWS * 11);
    }
}
