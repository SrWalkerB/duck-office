//! Objetos desenhados sobre a aba (F5): gráficos, segmentações de dados
//! (slicers), formas com texto. Ficam presos às células por uma âncora,
//! como no Excel (`xl/drawings/drawingN.xml`).

/// Um canto da âncora: célula + deslocamento dentro dela (EMU: 914400 por
/// polegada, 9525 por pixel a 96 DPI).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marker {
    pub col: u32,
    pub col_off: i64,
    pub row: u32,
    pub row_off: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Dois cantos nas células (o objeto acompanha larguras e alturas).
    Two { from: Marker, to: Marker },
    /// Um canto e o tamanho em EMU.
    One { from: Marker, cx: i64, cy: i64 },
    /// Posição e tamanho absolutos em EMU.
    Abs { x: i64, y: i64, cx: i64, cy: i64 },
}

impl Marker {
    /// O canto depois de inserir/apagar linhas ou colunas: anda com a célula;
    /// se a célula foi apagada, fica no começo do trecho apagado.
    pub fn shifted(self, s: crate::Shift) -> Marker {
        let (pos, off) = match s.axis {
            crate::Axis::Rows => (self.row, self.row_off),
            crate::Axis::Cols => (self.col, self.col_off),
        };
        let (pos, off) = if s.insert {
            if pos >= s.at { (pos + s.n, off) } else { (pos, off) }
        } else if pos >= s.at + s.n {
            (pos - s.n, off)
        } else if pos >= s.at {
            (s.at, 0)
        } else {
            (pos, off)
        };
        match s.axis {
            crate::Axis::Rows => Marker { row: pos, row_off: off, ..self },
            crate::Axis::Cols => Marker { col: pos, col_off: off, ..self },
        }
    }
}

impl Anchor {
    /// A âncora depois de inserir/apagar linhas ou colunas (mover e
    /// redimensionar com as células, como o Excel faz por padrão).
    pub fn shifted(self, s: crate::Shift) -> Anchor {
        match self {
            Anchor::Two { from, to } => Anchor::Two { from: from.shifted(s), to: to.shifted(s) },
            Anchor::One { from, cx, cy } => Anchor::One { from: from.shifted(s), cx, cy },
            a => a,
        }
    }
}

/// Uma âncora de desenho (`xdr:twoCellAnchor`...) na ordem da parte.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawingAnchor {
    /// Parte do desenho (`xl/drawings/drawingN.xml`).
    pub part: String,
    pub anchor: Anchor,
}

/// Cor de gráfico: direta ou do tema, com os ajustes do DrawingML.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChartColor {
    Rgb(u32),
    /// `idx` como em `Theme::color` (1 = dk1, 0 = lt1, 4..9 = destaque 1..6).
    Scheme {
        idx: u8,
        lum_mod: f32,
        lum_off: f32,
        shade: f32,
        tint: f32,
    },
}

/// Aplica os ajustes do DrawingML (`lumMod`, `lumOff`, `shade`, `tint`) a uma cor RGB.
pub fn adjust_color(v: u32, lum_mod: f32, lum_off: f32, shade: f32, tint: f32) -> u32 {
    let mut v = v;
    if lum_mod != 1.0 || lum_off != 0.0 {
        let (h, l, s) = rgb_to_hsl(v);
        v = hsl_to_rgb(h, (l * lum_mod as f64 + lum_off as f64).clamp(0.0, 1.0), s);
    }
    if shade != 1.0 {
        v = map_rgb(v, |c| c * shade as f64);
    }
    if tint != 1.0 {
        v = map_rgb(v, |c| c + (255.0 - c) * (1.0 - tint as f64));
    }
    v
}

pub fn map_rgb(v: u32, f: impl Fn(f64) -> f64) -> u32 {
    let ch = |s: u32| f(((v >> s) & 0xFF) as f64).round().clamp(0.0, 255.0) as u32;
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

fn rgb_to_hsl(v: u32) -> (f64, f64, f64) {
    let r = ((v >> 16) & 0xFF) as f64 / 255.0;
    let g = ((v >> 8) & 0xFF) as f64 / 255.0;
    let b = (v & 0xFF) as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, l, 0.0);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, l, s)
}

fn hsl_to_rgb(h: f64, l: f64, s: f64) -> u32 {
    let f = |p: f64, q: f64, mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let (r, g, b) = if s == 0.0 {
        (l, l, l)
    } else {
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        (f(p, q, h + 1.0 / 3.0), f(p, q, h), f(p, q, h - 1.0 / 3.0))
    };
    let c = |x: f64| (x * 255.0).round().clamp(0.0, 255.0) as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

/// Texto de um elemento do gráfico (`c:txPr`): tamanho em pontos, negrito, cor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextProps {
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub color: Option<ChartColor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PlotKind {
    #[default]
    Bar,
    /// Barras horizontais.
    BarH,
    Line,
    Area,
    Pie,
    Doughnut,
    Scatter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Grouping {
    #[default]
    Clustered,
    Stacked,
    Percent,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub name: String,
    /// Fórmulas das referências (`'Aba'!$B$2:$B$13`): os valores são
    /// relidos das células quando elas mudam.
    pub name_ref: Option<String>,
    pub cat_ref: Option<String>,
    pub val_ref: Option<String>,
    pub cats: Vec<String>,
    pub vals: Vec<Option<f64>>,
    /// Formato dos valores (rótulos).
    pub fmt: Option<String>,
    pub fill: Option<ChartColor>,
    pub line: Option<ChartColor>,
    pub no_fill: bool,
    pub no_line: bool,
    pub line_width: Option<f32>,
    pub dash: bool,
    pub marker: bool,
    pub labels: Option<bool>,
    pub label_text: Option<TextProps>,
    /// Fundo da caixa dos rótulos.
    pub label_fill: Option<ChartColor>,
    /// Cores de pontos individuais (`c:dPt`): (índice, cor).
    pub points: Vec<(u32, ChartColor)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plot {
    pub kind: PlotKind,
    pub grouping: Grouping,
    pub series: Vec<Series>,
    /// No eixo secundário.
    pub secondary: bool,
    pub gap: u32,
    pub overlap: i32,
    pub labels: bool,
    pub label_text: Option<TextProps>,
    pub label_fill: Option<ChartColor>,
    pub vary_colors: bool,
    /// Furo da rosca (%).
    pub hole: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartAxis {
    pub deleted: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub fmt: Option<String>,
    pub gridlines: bool,
    pub text: Option<TextProps>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LegendPos {
    #[default]
    Right,
    Bottom,
    Top,
    Left,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chart {
    pub title: Option<String>,
    pub title_text: Option<TextProps>,
    pub legend_text: Option<TextProps>,
    pub plots: Vec<Plot>,
    pub legend: Option<LegendPos>,
    /// Área do gráfico em frações (x, y, largura, altura) do quadro.
    pub plot_layout: Option<[f32; 4]>,
    pub cat_axis: ChartAxis,
    pub val_axis: ChartAxis,
    pub sec_axis: Option<ChartAxis>,
    pub background: Option<ChartColor>,
    pub no_background: bool,
    pub border: Option<ChartColor>,
    /// Gráfico de tabela dinâmica (os valores vêm da tabela).
    pub pivot: bool,
    /// Parte do gráfico (`xl/charts/chartN.xml`).
    pub part: String,
}

impl Chart {
    /// As referências das séries (nome, categorias, valores), na ordem.
    pub fn refs_mut(&mut self) -> impl Iterator<Item = &mut String> {
        self.plots
            .iter_mut()
            .flat_map(|p| p.series.iter_mut())
            .flat_map(|s| [&mut s.name_ref, &mut s.cat_ref, &mut s.val_ref])
            .flatten()
    }
}

/// Segmentação de dados: botões com os valores de um campo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Slicer {
    pub name: String,
    pub caption: String,
    pub show_caption: bool,
    pub columns: u32,
    /// Altura dos botões (EMU).
    pub row_height: i64,
    pub style: Option<String>,
    /// (texto, selecionado, tem dados).
    pub items: Vec<(String, bool, bool)>,
    /// Segmentação de tabela do Excel: (tabela, coluna) para filtrar.
    pub table: Option<(String, String)>,
    /// Segmentação de tabela dinâmica: o que ela filtra.
    pub pivot: Option<SlicerPivot>,
}

/// Ligação de uma segmentação com as tabelas dinâmicas.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlicerPivot {
    /// Parte do cache da segmentação (`xl/slicerCaches/slicerCacheN.xml`).
    pub part: String,
    /// Campo do cache que ela filtra (nome).
    pub source: String,
    /// `pivotCacheId` (x14) do cache da tabela dinâmica.
    pub cache_id: String,
    /// Tabelas dinâmicas ligadas: (`tabId` da aba, nome).
    pub tables: Vec<(u32, String)>,
    /// Índice no cache de cada item (na ordem de `Slicer::items`).
    pub item_x: Vec<u32>,
}

/// Linha do tempo: filtro de datas das tabelas dinâmicas (um período ou
/// um intervalo de períodos).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    pub name: String,
    pub caption: String,
    pub show_header: bool,
    pub show_selection_label: bool,
    pub show_time_level: bool,
    pub show_scrollbar: bool,
    /// Nível mostrado: 0 anos, 1 trimestres, 2 meses, 3 dias.
    pub level: u32,
    /// Primeira data visível (serial).
    pub scroll: f64,
    pub style: Option<String>,
    /// Parte do cache (`xl/timelineCaches/timelineCacheN.xml`).
    pub part: String,
    /// Campo de data (nome no cache da tabela dinâmica).
    pub source: String,
    /// `pivotCacheId` (x14) do cache da tabela dinâmica.
    pub cache_id: String,
    /// Tabelas dinâmicas ligadas: (`tabId` da aba, nome).
    pub tables: Vec<(u32, String)>,
    /// Primeiro e último dia escolhidos (seriais); `None` = todos os períodos.
    pub selection: Option<(f64, f64)>,
    /// Período dos dados: [início, fim).
    pub bounds: (f64, f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ObjectKind {
    Chart(Box<Chart>),
    Slicer(Box<Slicer>),
    Timeline(Box<Timeline>),
    /// Forma com texto (caixa de texto, botão).
    Shape {
        text: String,
        fill: Option<ChartColor>,
        line: Option<ChartColor>,
    },
    /// Imagem (caminho da parte no pacote).
    Picture {
        part: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub anchor: Anchor,
    /// Parte da âncora que o objeto ocupa (x, y, largura, altura em frações):
    /// objetos dentro de grupos.
    pub frac: [f32; 4],
    pub kind: ObjectKind,
}

impl Object {
    pub fn new(anchor: Anchor, kind: ObjectKind) -> Self {
        Self { anchor, frac: [0.0, 0.0, 1.0, 1.0], kind }
    }
}
