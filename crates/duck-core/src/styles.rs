//! Estilos de célula (subconjunto de `styles.xml` usado pela renderização).

use std::collections::HashMap;

/// Cor como no OOXML: RGB direto, índice de tema (com tonalidade) ou índice legado.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Color {
    #[default]
    Auto,
    /// ARGB
    Rgb(u32),
    Theme {
        index: u8,
        tint: f32,
    },
    Indexed(u8),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Font {
    pub name: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Color,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fill {
    /// `None` = sem preenchimento.
    pub pattern: Option<String>,
    pub fg: Color,
    pub bg: Color,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HAlign {
    #[default]
    General,
    Left,
    Center,
    Right,
    Fill,
    Justify,
    CenterContinuous,
    Distributed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Center,
    #[default]
    Bottom,
    Justify,
    Distributed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Alignment {
    pub horizontal: HAlign,
    pub vertical: VAlign,
    pub wrap: bool,
    pub indent: u8,
}

/// Uma entrada de `cellXfs`: o que o índice de estilo de uma célula aponta.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellXf {
    pub num_fmt: u16,
    pub font: u16,
    pub fill: u16,
    pub border: u16,
    pub alignment: Alignment,
}

#[derive(Clone, Debug, Default)]
pub struct Styles {
    /// Formatos de número personalizados (id ≥ 164 em geral).
    pub num_fmts: HashMap<u16, String>,
    pub fonts: Vec<Font>,
    pub fills: Vec<Fill>,
    /// Índice = valor de `s` na célula.
    pub xfs: Vec<CellXf>,
    /// Quantidade de bordas definidas (detalhes ficam para a F2).
    pub border_count: usize,
}
