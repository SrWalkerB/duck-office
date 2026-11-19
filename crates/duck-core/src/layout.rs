//! Geometria da aba: larguras de coluna, alturas de linha, células mescladas,
//! painéis congelados. Tudo em base 0 e com intervalos inclusivos.

/// Intervalo retangular de células (inclusivo, base 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRange {
    pub first_row: u32,
    pub first_col: u32,
    pub last_row: u32,
    pub last_col: u32,
}

impl CellRange {
    pub fn contains(&self, row: u32, col: u32) -> bool {
        row >= self.first_row && row <= self.last_row && col >= self.first_col && col <= self.last_col
    }
}

/// Um `<col>`: propriedades de um intervalo de colunas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColSpec {
    pub first: u32,
    pub last: u32,
    /// Largura em "caracteres" do Excel (largura do dígito 0 da fonte padrão).
    pub width: Option<f32>,
    pub hidden: bool,
    pub style: u16,
}

/// Uma linha com altura própria ou escondida.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowSpec {
    pub row: u32,
    /// Altura em pontos.
    pub height: Option<f32>,
    pub hidden: bool,
}

/// Painéis congelados.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pane {
    pub frozen_rows: u32,
    pub frozen_cols: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// Altura padrão das linhas, em pontos.
    pub default_row_height: Option<f32>,
    /// Largura padrão das colunas, em caracteres.
    pub default_col_width: Option<f32>,
    /// Ordenadas por `first`.
    pub cols: Vec<ColSpec>,
    /// Ordenadas por `row`; só linhas com altura própria ou escondidas.
    pub rows: Vec<RowSpec>,
    pub merges: Vec<CellRange>,
    pub pane: Option<Pane>,
    /// Área usada declarada no arquivo (`<dimension>`).
    pub dimension: Option<CellRange>,
}

impl Layout {
    /// Junta o layout lido por outro pedaço da mesma aba.
    pub fn absorb(&mut self, other: Layout) {
        self.default_row_height = self.default_row_height.or(other.default_row_height);
        self.default_col_width = self.default_col_width.or(other.default_col_width);
        self.pane = self.pane.or(other.pane);
        self.dimension = self.dimension.or(other.dimension);
        self.cols.extend(other.cols);
        self.rows.extend(other.rows);
        self.merges.extend(other.merges);
    }

    /// Ordena depois de juntar todos os pedaços.
    pub fn finish(&mut self) {
        self.cols.sort_by_key(|c| c.first);
        self.rows.sort_by_key(|r| r.row);
        self.merges.sort_by_key(|m| (m.first_row, m.first_col));
    }

    /// Especificação da coluna `col`, se houver (busca binária).
    pub fn col(&self, col: u32) -> Option<&ColSpec> {
        let i = self.cols.partition_point(|c| c.last < col);
        self.cols.get(i).filter(|c| c.first <= col)
    }

    /// Especificação da linha `row`, se houver (busca binária).
    pub fn row(&self, row: u32) -> Option<&RowSpec> {
        self.rows.binary_search_by_key(&row, |r| r.row).ok().map(|i| &self.rows[i])
    }

    pub fn heap_bytes(&self) -> usize {
        self.cols.capacity() * std::mem::size_of::<ColSpec>()
            + self.rows.capacity() * std::mem::size_of::<RowSpec>()
            + self.merges.capacity() * std::mem::size_of::<CellRange>()
    }
}
