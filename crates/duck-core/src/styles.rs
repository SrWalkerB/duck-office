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
    pub strike: bool,
    pub color: Color,
}

/// Fonte de um trecho de texto rico (`<r><rPr>`). O que não vier no arquivo
/// (`None`) fica como a fonte da célula.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunFont {
    pub name: Option<String>,
    pub size: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<Color>,
    /// Sobrescrito (1) ou subscrito (-1).
    pub script: i8,
}

/// Um trecho de texto rico: começa no byte `start` do texto e vai até o
/// próximo trecho. `font: None` = a fonte da célula.
#[derive(Clone, Debug, PartialEq)]
pub struct RichRun {
    pub start: u32,
    pub font: Option<RunFont>,
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
    /// `textRotation`: 0–90 = graus no sentido anti-horário, 91–180 = 1–90
    /// graus no sentido horário, 255 = letras empilhadas na vertical.
    pub rotation: u8,
}

impl Alignment {
    /// Ângulo do texto em graus, positivo = anti-horário (subindo). `None`
    /// para texto empilhado (`255`).
    pub fn angle(&self) -> Option<f32> {
        rotation_angle(self.rotation)
    }
}

/// Ver [`Alignment::angle`].
pub fn rotation_angle(rotation: u8) -> Option<f32> {
    match rotation {
        0..=90 => Some(rotation as f32),
        91..=180 => Some(-((rotation - 90) as f32)),
        _ => None,
    }
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
    pub borders: Vec<Border>,
    /// Índice = valor de `s` na célula.
    pub xfs: Vec<CellXf>,
    /// Paleta indexada própria do arquivo (`<colors><indexedColors>`), ARGB.
    pub indexed: Option<Vec<u32>>,
    pub theme: Theme,
    /// Formatos diferenciais (`<dxfs>`), usados pela formatação condicional.
    pub dxfs: Vec<Dxf>,
    /// Quantos `xfs`, `fonts`, `fills` e `borders` vieram do arquivo: os
    /// seguintes foram criados na edição e o gravador os acrescenta ao
    /// `styles.xml` (F3).
    pub file_counts: FileCounts,
    /// Para cada estilo criado na edição (`xfs[file_counts.xfs + k]`), o
    /// estilo do arquivo de que ele derivou (o gravador copia o `<xf>` dele
    /// e troca só o que mudou).
    pub xf_base: Vec<u16>,
    /// Formatos de número criados na edição (ids em `num_fmts`).
    pub new_num_fmts: Vec<u16>,
    /// Estilos de segmentação de dados do arquivo: (nome, [(elemento, dxf)]).
    pub slicer_styles: Vec<(String, Vec<(String, u32)>)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileCounts {
    pub xfs: usize,
    pub fonts: usize,
    pub fills: usize,
    pub borders: usize,
}

/// Formato diferencial: só o que muda em relação ao estilo da célula.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dxf {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub color: Option<Color>,
    pub fill: Option<Color>,
    /// Esquerda, direita, cima, baixo.
    pub border: [Option<BorderSide>; 4],
}

/// Estilo de uma linha de borda.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BorderStyle {
    #[default]
    None,
    Thin,
    Medium,
    Thick,
    Double,
    Hair,
    Dotted,
    Dashed,
    MediumDashed,
    DashDot,
    MediumDashDot,
    DashDotDot,
    MediumDashDotDot,
    SlantDashDot,
}

impl BorderStyle {
    pub fn parse(s: &[u8]) -> Self {
        match s {
            b"thin" => Self::Thin,
            b"medium" => Self::Medium,
            b"thick" => Self::Thick,
            b"double" => Self::Double,
            b"hair" => Self::Hair,
            b"dotted" => Self::Dotted,
            b"dashed" => Self::Dashed,
            b"mediumDashed" => Self::MediumDashed,
            b"dashDot" => Self::DashDot,
            b"mediumDashDot" => Self::MediumDashDot,
            b"dashDotDot" => Self::DashDotDot,
            b"mediumDashDotDot" => Self::MediumDashDotDot,
            b"slantDashDot" => Self::SlantDashDot,
            _ => Self::None,
        }
    }

    /// Nome no `styles.xml` (inverso de [`BorderStyle::parse`]).
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Thin => "thin",
            Self::Medium => "medium",
            Self::Thick => "thick",
            Self::Double => "double",
            Self::Hair => "hair",
            Self::Dotted => "dotted",
            Self::Dashed => "dashed",
            Self::MediumDashed => "mediumDashed",
            Self::DashDot => "dashDot",
            Self::MediumDashDot => "mediumDashDot",
            Self::DashDotDot => "dashDotDot",
            Self::MediumDashDotDot => "mediumDashDotDot",
            Self::SlantDashDot => "slantDashDot",
        }
    }

    /// Espessura em pixels lógicos (a dupla ocupa 3: linha, espaço, linha).
    pub fn width(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Thin | Self::Hair | Self::Dotted | Self::Dashed | Self::DashDot | Self::DashDotDot => 1,
            Self::Medium
            | Self::MediumDashed
            | Self::MediumDashDot
            | Self::MediumDashDotDot
            | Self::SlantDashDot => 2,
            Self::Thick | Self::Double => 3,
        }
    }

    /// Padrão tracejado: (período, parte desenhada), em pixels lógicos.
    pub fn dash(self) -> Option<(f32, f32)> {
        match self {
            Self::Hair | Self::Dotted => Some((2.0, 1.0)),
            Self::Dashed => Some((4.0, 3.0)),
            Self::MediumDashed => Some((12.0, 9.0)),
            Self::DashDot | Self::DashDotDot => Some((6.0, 4.0)),
            Self::MediumDashDot | Self::MediumDashDotDot | Self::SlantDashDot => Some((12.0, 8.0)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BorderSide {
    pub style: BorderStyle,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Border {
    pub left: BorderSide,
    pub right: BorderSide,
    pub top: BorderSide,
    pub bottom: BorderSide,
}

impl Border {
    pub fn is_empty(&self) -> bool {
        [self.left, self.right, self.top, self.bottom].iter().all(|s| s.style == BorderStyle::None)
    }
}

/// Tema do workbook (`theme1.xml`): as 12 cores do esquema e as fontes.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Na ordem do arquivo: dk1, lt1, dk2, lt2, accent1–6, hlink, folHlink. RGB.
    pub colors: [u32; 12],
    /// Fonte do corpo (`minorFont`), usada pelo estilo Normal.
    pub minor_font: String,
    pub major_font: String,
}

impl Default for Theme {
    /// Tema "Office" (2013–2022).
    fn default() -> Self {
        Self {
            colors: [
                0x000000, 0xFFFFFF, 0x44546A, 0xE7E6E6, 0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5,
                0x70AD47, 0x0563C1, 0x954F72,
            ],
            minor_font: "Calibri".into(),
            major_font: "Calibri Light".into(),
        }
    }
}

impl Theme {
    /// Cor do índice usado nas células (`theme="n"`): 0↔1 e 2↔3 vêm trocados
    /// em relação à ordem do arquivo (0 = lt1/fundo, 1 = dk1/texto).
    pub fn color(&self, index: u8) -> u32 {
        let i = match index {
            0 => 1,
            1 => 0,
            2 => 3,
            3 => 2,
            i => i as usize,
        };
        self.colors.get(i).copied().unwrap_or(0)
    }
}

impl Styles {
    /// Resolve uma cor para RGB (sem alfa). `None` = automática (o chamador
    /// escolhe: texto preto, sem preenchimento, borda preta...).
    pub fn rgb(&self, c: Color) -> Option<u32> {
        match c {
            Color::Auto => None,
            Color::Rgb(argb) => Some(argb & 0xFF_FFFF),
            Color::Theme { index, tint } => Some(apply_tint(self.theme.color(index), tint)),
            Color::Indexed(i) => {
                let i = i as usize;
                if i == 64 {
                    return None; // cor do texto do sistema: automática
                }
                let argb = self
                    .indexed
                    .as_ref()
                    .and_then(|p| p.get(i).copied())
                    .or_else(|| crate::numfmt::INDEXED.get(i).copied())?;
                Some(argb & 0xFF_FFFF)
            }
        }
    }

    /// Estilo igual a `base` com as mudanças de `change`: um já existente,
    /// se houver igual, ou um novo no fim de `xfs`.
    pub fn derive_xf(&mut self, base: u16, change: impl FnOnce(&mut CellXf)) -> u16 {
        let mut xf = self.xfs.get(base as usize).copied().unwrap_or_default();
        change(&mut xf);
        if self.xfs.get(base as usize) == Some(&xf) {
            return base;
        }
        if let Some(i) = self.xfs.iter().position(|x| *x == xf) {
            return i as u16;
        }
        // O `<xf>` de origem precisa vir do arquivo (os criados aqui não têm XML).
        let root = if (base as usize) < self.file_counts.xfs {
            base
        } else {
            self.xf_base.get(base as usize - self.file_counts.xfs).copied().unwrap_or(0)
        };
        if self.xfs.is_empty() {
            // Arquivo sem estilos (ex.: CSV): o 0 é o padrão.
            self.xfs.push(CellXf::default());
            self.file_counts.xfs = self.file_counts.xfs.max(1);
        }
        self.xfs.push(xf);
        self.xf_base.push(root);
        (self.xfs.len() - 1) as u16
    }

    /// Id de um formato de número com o código `code`: um interno ou
    /// personalizado igual, ou um novo (a partir de 164, como o Excel).
    pub fn intern_num_fmt(&mut self, code: &str) -> u16 {
        if let Some((&id, _)) = self.num_fmts.iter().find(|(_, c)| c.as_str() == code) {
            return id;
        }
        if let Some(id) = (0..164u16).find(|&i| crate::numfmt::builtin(i) == Some(code)) {
            return id;
        }
        let id = self.num_fmts.keys().copied().max().unwrap_or(163).max(163) + 1;
        self.num_fmts.insert(id, code.to_string());
        self.new_num_fmts.push(id);
        id
    }

    /// Índice de uma fonte igual a `f` (nova no fim da lista, se não houver).
    pub fn intern_font(&mut self, f: Font) -> u16 {
        match self.fonts.iter().position(|x| *x == f) {
            Some(i) => i as u16,
            None => {
                self.fonts.push(f);
                (self.fonts.len() - 1) as u16
            }
        }
    }

    pub fn intern_fill(&mut self, f: Fill) -> u16 {
        match self.fills.iter().position(|x| *x == f) {
            Some(i) => i as u16,
            None => {
                self.fills.push(f);
                (self.fills.len() - 1) as u16
            }
        }
    }

    pub fn intern_border(&mut self, b: Border) -> u16 {
        match self.borders.iter().position(|x| *x == b) {
            Some(i) => i as u16,
            None => {
                self.borders.push(b);
                (self.borders.len() - 1) as u16
            }
        }
    }

    /// Há estilos criados na edição.
    pub fn has_new(&self) -> bool {
        !self.new_num_fmts.is_empty()
            || self.xfs.len() > self.file_counts.xfs
            || self.fonts.len() > self.file_counts.fonts
            || self.fills.len() > self.file_counts.fills
            || self.borders.len() > self.file_counts.borders
    }

    /// Código do formato de número `id` (personalizado ou interno).
    pub fn num_fmt_code(&self, id: u16) -> &str {
        match self.num_fmts.get(&id) {
            Some(s) => s,
            None => crate::numfmt::builtin(id).unwrap_or("General"),
        }
    }
}

/// Aplica a tonalidade do Excel (`tint`, de -1 a 1) na luminosidade HLS.
pub fn apply_tint(rgb: u32, tint: f32) -> u32 {
    if tint == 0.0 {
        return rgb;
    }
    let (h, l, s) = rgb_to_hls(rgb);
    let tint = tint as f64;
    let l = if tint < 0.0 { l * (1.0 + tint) } else { l * (1.0 - tint) + tint };
    hls_to_rgb(h, l.clamp(0.0, 1.0), s)
}

fn rgb_to_hls(rgb: u32) -> (f64, f64, f64) {
    let r = ((rgb >> 16) & 0xFF) as f64 / 255.0;
    let g = ((rgb >> 8) & 0xFF) as f64 / 255.0;
    let b = (rgb & 0xFF) as f64 / 255.0;
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
    };
    (h / 6.0, l, s)
}

fn hls_to_rgb(h: f64, l: f64, s: f64) -> u32 {
    let to = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u32;
    if s == 0.0 {
        let v = to(l);
        return (v << 16) | (v << 8) | v;
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f64| {
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
    (to(hue(h + 1.0 / 3.0)) << 16) | (to(hue(h)) << 8) | to(hue(h - 1.0 / 3.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_matches_excel() {
        // Valores que o Excel mostra para as variações de tema mais comuns.
        assert_eq!(apply_tint(0xFFFFFF, -0.149_998_47), 0xD9D9D9);
        assert_eq!(apply_tint(0x4472C4, 0.799_981_7), 0xDAE3F3);
        assert_eq!(apply_tint(0x4472C4, -0.249_977_11), 0x2F5597);
        assert_eq!(apply_tint(0x000000, 0.499_984_74), 0x7F7F7F);
        assert_eq!(apply_tint(0x123456, 0.0), 0x123456);
    }

    #[test]
    fn theme_index_swap() {
        let t = Theme::default();
        assert_eq!(t.color(0), 0xFFFFFF);
        assert_eq!(t.color(1), 0x000000);
        assert_eq!(t.color(4), 0x4472C4);
    }

    #[test]
    fn resolve_colors() {
        let st = Styles::default();
        assert_eq!(st.rgb(Color::Rgb(0xFF112233)), Some(0x112233));
        assert_eq!(st.rgb(Color::Indexed(10)), Some(0xFF0000));
        assert_eq!(st.rgb(Color::Indexed(64)), None);
        assert_eq!(st.rgb(Color::Auto), None);
        assert_eq!(st.num_fmt_code(4), "#,##0.00");
    }
}
