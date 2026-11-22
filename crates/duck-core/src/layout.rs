//! Geometria da aba: larguras de coluna, alturas de linha, células mescladas,
//! painéis congelados. Tudo em base 0 e com intervalos inclusivos.

/// Intervalo retangular de células (inclusivo, base 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
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

/// Uma linha com altura própria, escondida ou com estilo de linha inteira.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowSpec {
    pub row: u32,
    /// Altura em pontos.
    pub height: Option<f32>,
    pub hidden: bool,
    /// Estilo das células sem `<c>` da linha (`<row s customFormat="1">`).
    pub style: Option<u16>,
}

/// Painéis congelados.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pane {
    pub frozen_rows: u32,
    pub frozen_cols: u32,
}

/// Como a aba é mostrada (`<sheetView>` e `<sheetPr>`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetView {
    pub show_grid_lines: bool,
    pub show_headers: bool,
    /// Zoom em porcentagem (10–400).
    pub zoom: u16,
    /// Célula do canto superior esquerdo ao abrir.
    pub top_left: Option<(u32, u32)>,
    /// Célula ativa ao abrir.
    pub active_cell: Option<(u32, u32)>,
    /// Cor da guia da aba.
    pub tab_color: Option<crate::Color>,
}

impl Default for SheetView {
    fn default() -> Self {
        Self {
            show_grid_lines: true,
            show_headers: true,
            zoom: 100,
            top_left: None,
            active_cell: None,
            tab_color: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// `None` até o primeiro `<sheetView>` (ou `<sheetPr>`) ser lido.
    pub view: Option<SheetView>,
    /// Altura padrão das linhas, em pontos.
    pub default_row_height: Option<f32>,
    /// Largura padrão das colunas, em caracteres (`defaultColWidth`).
    pub default_col_width: Option<f32>,
    /// Largura base em caracteres, sem a margem (`baseColWidth`, padrão 8).
    pub base_col_width: Option<f32>,
    /// Ordenadas por `first`.
    pub cols: Vec<ColSpec>,
    /// Ordenadas por `row`; só linhas com altura própria ou escondidas.
    pub rows: Vec<RowSpec>,
    pub merges: Vec<CellRange>,
    pub pane: Option<Pane>,
    /// Área usada declarada no arquivo (`<dimension>`).
    pub dimension: Option<CellRange>,
    /// Formatação condicional, em ordem de prioridade depois de [`Layout::finish`].
    pub cond: Vec<crate::CfRule>,
    /// Hiperlinks, em ordem de linha depois de [`Layout::finish`].
    pub links: Vec<Hyperlink>,
    /// Filtro automático da aba (`<autoFilter>`).
    pub autofilter: Option<AutoFilter>,
    /// Tabelas do Excel (`xl/tables/tableN.xml`) desta aba.
    pub tables: Vec<Table>,
}

/// Filtro automático: o intervalo (com a linha de cabeçalho) e os
/// critérios por coluna.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AutoFilter {
    pub range: CellRange,
    pub columns: Vec<FilterColumn>,
}

/// Critério de uma coluna do filtro (`col` = deslocamento dentro do intervalo).
#[derive(Clone, Debug, PartialEq)]
pub struct FilterColumn {
    pub col: u32,
    pub kind: FilterKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FilterKind {
    /// Lista de valores mostrados (como aparecem na célula) e se as vazias passam.
    Values { values: Vec<String>, blank: bool },
    /// Outro critério (personalizado, 10 maiores, por cor, por data...):
    /// guardado como está no arquivo (XML do `<filterColumn>`), sem edição.
    Other(String),
}

/// Uma tabela do Excel.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Table {
    /// Caminho da parte no pacote (`xl/tables/table1.xml`).
    pub part: String,
    pub name: String,
    pub range: CellRange,
    /// Linhas de cabeçalho (0 ou 1) e de totais.
    pub header_rows: u32,
    pub totals_rows: u32,
    /// Nome do estilo (`TableStyleMedium2`...), se houver.
    pub style: Option<String>,
    pub row_stripes: bool,
    pub col_stripes: bool,
    pub first_col: bool,
    pub last_col: bool,
    pub filter: Option<AutoFilter>,
    /// Nomes das colunas.
    pub columns: Vec<String>,
    /// O `<tableColumn>` de cada coluna como está no arquivo (atributos e
    /// fórmulas de coluna calculada ficam intactos ao regravar).
    pub column_xml: Vec<String>,
}

impl Table {
    /// Linha do cabeçalho, se houver.
    pub fn header_row(&self) -> Option<u32> {
        (self.header_rows > 0).then_some(self.range.first_row)
    }

    /// Troca o nome da coluna `k` (texto do cabeçalho), deixando único na tabela.
    pub fn rename_column(&mut self, k: usize, name: &str) {
        let mut name = if name.trim().is_empty() { format!("Coluna{}", k + 1) } else { name.to_string() };
        let taken = |n: &str, cols: &[String]| {
            cols.iter().enumerate().any(|(i, c)| i != k && c.eq_ignore_ascii_case(n))
        };
        let base = name.clone();
        let mut i = 2;
        while taken(&name, &self.columns) {
            name = format!("{base}{i}");
            i += 1;
        }
        if let Some(c) = self.columns.get_mut(k) {
            *c = name.clone();
        }
        if let Some(x) = self.column_xml.get_mut(k) {
            *x = set_xml_attr(x, "name", &name);
        }
    }
}

/// Troca o valor de um atributo numa tag XML simples (`<tableColumn id="1" name="A"/>`).
pub fn set_xml_attr(tag: &str, key: &str, value: &str) -> String {
    let esc = value.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
    let pat = format!(" {key}=\"");
    match tag.find(&pat) {
        Some(i) => {
            let start = i + pat.len();
            let end = tag[start..].find('"').map_or(tag.len(), |e| start + e);
            format!("{}{}{}", &tag[..start], esc, &tag[end..])
        }
        None => {
            let at = tag.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(tag.len());
            format!("{} {key}=\"{esc}\"{}", &tag[..at], &tag[at..])
        }
    }
}

/// Um `<hyperlink>`: endereço externo (já resolvido pelos relacionamentos da
/// aba) ou lugar dentro do arquivo (`Aba!A1`, nome definido).
#[derive(Clone, Debug, PartialEq)]
pub struct Hyperlink {
    pub range: CellRange,
    /// `r:id` até a leitura resolver; depois, o endereço externo.
    pub target: Option<String>,
    pub location: Option<String>,
    pub tooltip: Option<String>,
}

impl Layout {
    /// Junta o layout lido por outro pedaço da mesma aba.
    pub fn absorb(&mut self, other: Layout) {
        self.default_row_height = self.default_row_height.or(other.default_row_height);
        self.default_col_width = self.default_col_width.or(other.default_col_width);
        self.base_col_width = self.base_col_width.or(other.base_col_width);
        self.pane = self.pane.or(other.pane);
        self.view = match (self.view, other.view) {
            (Some(mut a), Some(b)) => {
                a.tab_color = a.tab_color.or(b.tab_color);
                Some(a)
            }
            (a, b) => a.or(b),
        };
        self.dimension = self.dimension.or(other.dimension);
        self.cols.extend(other.cols);
        self.rows.extend(other.rows);
        self.merges.extend(other.merges);
        self.cond.extend(other.cond);
        self.links.extend(other.links);
        self.autofilter = self.autofilter.take().or(other.autofilter);
        self.tables.extend(other.tables);
    }

    /// Ordena depois de juntar todos os pedaços.
    pub fn finish(&mut self) {
        // `<tabColor>` sozinho (sem `<sheetView>`) deixa zoom 0 como marca.
        if let Some(v) = self.view.as_mut()
            && v.zoom == 0
        {
            v.zoom = 100;
        }
        self.cols.sort_by_key(|c| c.first);
        self.rows.sort_by_key(|r| r.row);
        self.merges.sort_by_key(|m| (m.first_row, m.first_col));
        self.cond.sort_by_key(|r| r.priority);
        self.links.sort_by_key(|l| (l.range.first_row, l.range.first_col));
    }

    /// Hiperlink que cobre a célula, se houver.
    pub fn link_at(&self, row: u32, col: u32) -> Option<&Hyperlink> {
        // Poucos links por aba; os que começam depois da linha não servem.
        let end = self.links.partition_point(|l| l.range.first_row <= row);
        self.links[..end].iter().rev().find(|l| l.range.contains(row, col))
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
