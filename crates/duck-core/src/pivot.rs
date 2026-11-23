//! Tabelas dinâmicas (F5): o cache (campos, itens e registros) e a definição
//! de cada tabela (campos nas linhas, colunas, filtros e valores; itens
//! escondidos; layout). O cálculo fica no `duck-calc::pivot`.

/// Um item do cache (valor distinto de um campo).
#[derive(Clone, Debug, PartialEq)]
pub enum PItem {
    Str(String),
    Num(f64),
    /// Data (serial).
    Date(f64),
    Bool(bool),
    Err(String),
    /// Vazio.
    Missing,
}

/// Agrupamento de um campo (`fieldGroup`).
#[derive(Clone, Debug, PartialEq)]
pub enum Group {
    /// Datas por período: `by` = years, quarters, months, days, hours,
    /// minutes, seconds; início e fim (seriais).
    Dates { by: String, start: f64, end: f64 },
    /// Números em faixas.
    Numbers { start: f64, end: f64, step: f64 },
    /// Itens do campo base agrupados à mão: índice do grupo de cada item base.
    Discrete(Vec<u32>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CacheField {
    pub name: String,
    /// Itens compartilhados (`sharedItems`), na ordem do arquivo. Os
    /// registros com valor próprio acrescentam itens aqui na leitura.
    pub items: Vec<PItem>,
    /// Formato de número do campo.
    pub num_fmt: u32,
    /// Campo calculado ou de grupo (sem coluna nos registros).
    pub virtual_field: bool,
    /// Agrupamento: campo base e regra; os itens do grupo ficam em `items`.
    pub group: Option<(u32, Group)>,
    /// Fórmula de campo calculado (não suportado: valores guardados ficam).
    pub formula: Option<String>,
    /// Campo agrupado: os valores brutos (`sharedItems`) que os registros
    /// indexam; `items` são os itens do grupo.
    pub raw: Vec<PItem>,
}

impl CacheField {
    /// Itens que os registros indexam.
    pub fn raw_items(&self) -> &[PItem] {
        if self.group.is_some() { &self.raw } else { &self.items }
    }
}

/// Registros do cache, por campo (coluna): índice do item em `items`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Records {
    pub count: usize,
    /// `cols[campo][registro]`; campos virtuais ficam vazios.
    pub cols: Vec<Vec<u32>>,
    /// Campos agrupados: os índices brutos (antes do grupo).
    pub grouped_raw: Vec<(u32, Vec<u32>)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PivotCache {
    /// `cacheId` do workbook.
    pub id: u32,
    /// Parte da definição (`xl/pivotCache/pivotCacheDefinitionN.xml`).
    pub part: String,
    /// Parte dos registros (se houver).
    pub records_part: Option<String>,
    /// Id usado pelas segmentações (`x14:pivotCacheDefinition pivotCacheId`).
    pub x14_id: Option<String>,
    pub fields: Vec<CacheField>,
    pub record_count: usize,
    /// Fonte externa (Modelo de Dados, conexão): sem registros, não recalcula.
    pub external: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Axis {
    #[default]
    None,
    Row,
    Col,
    Page,
    Values,
}

/// Item de um campo da tabela (ordem = ordem de exibição).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldItem {
    /// Índice no cache (`x`); `None` para itens de subtotal/total.
    pub x: Option<u32>,
    /// Tipo (`t`): `None` = item de dados; `default`, `sum`, `grand`...
    pub t: Option<String>,
    pub hidden: bool,
    /// Detalhe recolhido (`sd="0"`).
    pub collapsed: bool,
    /// Item que não existe mais nos dados (`m="1"`).
    pub missing: bool,
    /// Nome próprio (`n`).
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortType {
    #[default]
    Manual,
    Ascending,
    Descending,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PivotField {
    pub axis: Axis,
    pub data_field: bool,
    pub show_all: bool,
    pub compact: bool,
    pub outline: bool,
    pub subtotal_top: bool,
    pub default_subtotal: bool,
    pub sort: SortType,
    /// Ordem pelo valor de um campo de dados (`autoSortScope`): índice em `data_fields`.
    pub sort_by_data: Option<u32>,
    pub items: Vec<FieldItem>,
    pub name: Option<String>,
    pub num_fmt: Option<u32>,
    pub insert_blank_row: bool,
}

impl Default for PivotField {
    fn default() -> Self {
        Self {
            axis: Axis::None,
            data_field: false,
            show_all: true,
            compact: true,
            outline: true,
            subtotal_top: true,
            default_subtotal: true,
            sort: SortType::Manual,
            sort_by_data: None,
            items: Vec::new(),
            name: None,
            num_fmt: None,
            insert_blank_row: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataField {
    pub name: String,
    pub fld: u32,
    /// sum, count, average, max, min, product, countNums, stdDev, stdDevp, var, varp.
    pub subtotal: String,
    pub show_as: Option<String>,
    pub num_fmt: Option<u32>,
}

/// Uma linha (ou coluna) do resultado como o Excel guarda (`rowItems/i`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AxisItem {
    /// `t`: `None` = dados; `default`, `grand`, `blank`...
    pub t: Option<String>,
    /// Quantos campos repetem a linha anterior.
    pub r: u32,
    /// Índice do campo de dados (`i`).
    pub i: u32,
    /// Índices nos itens dos campos (`x v`).
    pub x: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PivotTable {
    pub part: String,
    pub name: String,
    pub cache_id: u32,
    /// Área do corpo (`location ref`), sem os filtros.
    pub range: crate::CellRange,
    pub first_header_row: u32,
    pub first_data_row: u32,
    pub first_data_col: u32,
    pub fields: Vec<PivotField>,
    /// Campos nas linhas e colunas (-2 = "Valores").
    pub row_fields: Vec<i32>,
    pub col_fields: Vec<i32>,
    /// Filtros: (campo, item escolhido).
    pub page_fields: Vec<(u32, Option<u32>)>,
    pub data_fields: Vec<DataField>,
    /// Estrutura que o Excel gravou (conferência e desenho).
    pub row_items: Vec<AxisItem>,
    pub col_items: Vec<AxisItem>,
    pub compact: bool,
    pub outline: bool,
    pub row_grand: bool,
    pub col_grand: bool,
    pub data_on_rows: bool,
    pub data_caption: String,
    pub grand_caption: Option<String>,
    pub row_header_caption: Option<String>,
    pub col_header_caption: Option<String>,
    pub style: Option<String>,
    pub show_headers: bool,
    /// Erros aparecem como `error_caption` (`showError`).
    pub show_error: bool,
    pub error_caption: String,
}
