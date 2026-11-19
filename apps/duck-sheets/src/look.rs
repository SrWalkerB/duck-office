//! Estilos resolvidos para desenhar: cada `cellXfs` vira um [`Look`] com
//! cores já em RGB (tema + tonalidade, paleta indexada), formato de número já
//! interpretado e bordas prontas. Feito uma vez por arquivo (centenas de
//! estilos), não por célula.

use duck_core::numfmt::format_general;
use duck_core::{BorderSide, BorderStyle, CellError, HAlign, NumFmt, Styles, VAlign, Value};
use duck_ui::{Rgba, rgb};

/// Uma borda de célula pronta para desenhar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub style: BorderStyle,
    pub color: Rgba,
}

/// Uma fonte do arquivo.
#[derive(Clone, Debug)]
pub struct FontLook {
    pub name: String,
    pub pt: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// Um estilo de célula resolvido.
#[derive(Clone, Debug)]
pub struct Look {
    pub fill: Option<Rgba>,
    pub font: u16,
    /// Cor do texto (automática = preto).
    pub color: Rgba,
    pub h: HAlign,
    pub v: VAlign,
    pub wrap: bool,
    pub indent: u8,
    /// Índice em [`Looks::fmts`].
    pub fmt: u32,
    /// Esquerda, direita, cima, baixo.
    pub edges: [Option<Edge>; 4],
}

pub struct Looks {
    pub xfs: Vec<Look>,
    pub fonts: Vec<FontLook>,
    pub fmts: Vec<NumFmt>,
    default: Look,
}

const BLACK: Rgba = [0, 0, 0, 255];

impl Looks {
    pub fn new(st: &Styles) -> Self {
        let mut fonts: Vec<FontLook> = st
            .fonts
            .iter()
            .map(|f| FontLook {
                name: if f.name.is_empty() { st.theme.minor_font.clone() } else { f.name.clone() },
                pt: if f.size > 0.0 { f.size } else { 11.0 },
                bold: f.bold,
                italic: f.italic,
                underline: f.underline,
            })
            .collect();
        if fonts.is_empty() {
            fonts.push(FontLook {
                name: st.theme.minor_font.clone(),
                pt: 11.0,
                bold: false,
                italic: false,
                underline: false,
            });
        }
        let mut fmts = vec![NumFmt::general()];
        let mut fmt_ids: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
        let mut fmt_of = |id: u16| -> u32 {
            *fmt_ids.entry(id).or_insert_with(|| {
                fmts.push(NumFmt::parse(st.num_fmt_code(id)));
                (fmts.len() - 1) as u32
            })
        };
        let edge = |s: &BorderSide| -> Option<Edge> {
            (s.style != BorderStyle::None)
                .then(|| Edge { style: s.style, color: st.rgb(s.color).map_or(BLACK, rgb) })
        };
        let xfs: Vec<Look> = st
            .xfs
            .iter()
            .map(|xf| {
                let font = (xf.font as usize).min(fonts.len() - 1);
                let color = st.fonts.get(font).and_then(|f| st.rgb(f.color)).map_or(BLACK, rgb);
                let fill = st.fills.get(xf.fill as usize).and_then(|f| fill_color(st, f));
                let edges = match st.borders.get(xf.border as usize) {
                    Some(b) => [edge(&b.left), edge(&b.right), edge(&b.top), edge(&b.bottom)],
                    None => [None; 4],
                };
                Look {
                    fill,
                    font: font as u16,
                    color,
                    h: xf.alignment.horizontal,
                    v: xf.alignment.vertical,
                    wrap: xf.alignment.wrap,
                    indent: xf.alignment.indent,
                    fmt: if xf.num_fmt == 0 { 0 } else { fmt_of(xf.num_fmt) },
                    edges,
                }
            })
            .collect();
        let default = xfs.first().cloned().unwrap_or(Look {
            fill: None,
            font: 0,
            color: BLACK,
            h: HAlign::General,
            v: VAlign::Bottom,
            wrap: false,
            indent: 0,
            fmt: 0,
            edges: [None; 4],
        });
        Self { xfs, fonts, fmts, default }
    }

    #[inline]
    pub fn get(&self, xf: u16) -> &Look {
        self.xfs.get(xf as usize).unwrap_or(&self.default)
    }

    pub fn fmt(&self, look: &Look) -> &NumFmt {
        &self.fmts[look.fmt as usize]
    }
}

/// Cor de fundo de um preenchimento. Padrões (hachuras) viram a mistura das
/// duas cores na proporção da tinta.
fn fill_color(st: &Styles, f: &duck_core::Fill) -> Option<Rgba> {
    let pattern = f.pattern.as_deref()?;
    let fg = st.rgb(f.fg);
    let bg = st.rgb(f.bg);
    let density = match pattern {
        "solid" => 1.0,
        "darkGray" => 0.75,
        "mediumGray" => 0.5,
        "lightGray" => 0.25,
        "gray125" => 0.125,
        "gray0625" => 0.0625,
        _ => 0.25, // listras e grades
    };
    if density >= 1.0 {
        // Sólido com cor automática: o Excel usa a cor de fundo do sistema.
        return Some(rgb(fg.or(bg).unwrap_or(0xFFFFFF)));
    }
    let a = fg.unwrap_or(0x000000);
    let b = bg.unwrap_or(0xFFFFFF);
    let mix = |sh: u32| {
        let x = ((a >> sh) & 0xFF) as f32;
        let y = ((b >> sh) & 0xFF) as f32;
        (x * density + y * (1.0 - density)).round() as u32
    };
    Some(rgb((mix(16) << 16) | (mix(8) << 8) | mix(0)))
}

/// Tipo do que a célula mostra (define o alinhamento "Geral").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Number,
    Text,
    /// Booleanos e erros: centralizados no alinhamento "Geral".
    Logical,
}

/// O texto de uma célula e como ele deve aparecer.
#[derive(Clone, Copy, Debug)]
pub struct Shown {
    pub kind: Kind,
    /// Cor vinda do formato (`[Red]`...), sobrepõe a da fonte.
    pub color: Option<Rgba>,
    /// Preenchimento contábil (`*`): texto antes vai à esquerda, depois à direita.
    pub fill_at: Option<usize>,
    /// O formato é "Geral" (números podem perder casas para caber).
    pub general: bool,
    /// Não cabe no formato (data negativa): mostrar `###`.
    pub overflow: bool,
}

/// Nome do erro como no Excel em português.
pub fn error_text(e: CellError) -> &'static str {
    match e {
        CellError::Null => "#NULO!",
        CellError::Div0 => "#DIV/0!",
        CellError::Value => "#VALOR!",
        CellError::Ref => "#REF!",
        CellError::Name => "#NOME?",
        CellError::Num => "#NÚM!",
        CellError::NA => "#N/D",
        CellError::GettingData => "#OBTENDO_DADOS",
        CellError::Spill => "#DESPEJAR!",
        CellError::Calc => "#CALC!",
        CellError::Other => "#ERRO",
    }
}

/// Escreve em `out` o texto que a célula mostra.
pub fn show(looks: &Looks, look: &Look, value: Value, date1904: bool, out: &mut String) -> Option<Shown> {
    out.clear();
    let fmt = looks.fmt(look);
    let base = Shown { kind: Kind::Text, color: None, fill_at: None, general: false, overflow: false };
    match value {
        Value::Empty => None,
        Value::Number(v) => {
            let r = fmt.format_number(v, date1904, out);
            Some(Shown {
                kind: Kind::Number,
                color: r.color.map(rgb),
                fill_at: r.fill_at,
                general: fmt.is_general(),
                overflow: r.overflow,
            })
        }
        Value::Text(s) => match fmt.format_text(s, out) {
            Some(r) => Some(Shown { color: r.color.map(rgb), fill_at: r.fill_at, ..base }),
            None => {
                out.push_str(s);
                Some(base)
            }
        },
        Value::Bool(b) => {
            out.push_str(if b { "VERDADEIRO" } else { "FALSO" });
            Some(Shown { kind: Kind::Logical, ..base })
        }
        Value::Error(e) => {
            out.push_str(error_text(e));
            Some(Shown { kind: Kind::Logical, ..base })
        }
    }
}

/// Número "Geral" em no máximo `max_chars` caracteres.
pub fn general_fit(v: f64, max_chars: usize, out: &mut String) {
    out.clear();
    format_general(v, max_chars, out);
}

/// O valor como aparece na barra de fórmulas (o valor guardado, não o formatado).
pub fn edit_text(looks: &Looks, look: &Look, value: Value, date1904: bool, out: &mut String) {
    out.clear();
    match value {
        Value::Empty => {}
        Value::Number(v) => {
            let fmt = looks.fmt(look);
            if fmt.is_date() {
                let code = if v.fract() == 0.0 {
                    "dd/mm/yyyy"
                } else if v < 1.0 {
                    "hh:mm:ss"
                } else {
                    "dd/mm/yyyy hh:mm:ss"
                };
                NumFmt::parse(code).format_number(v, date1904, out);
            } else {
                // Precisão total (15 dígitos), sem separador de milhar.
                format_general(v, 17, out);
            }
        }
        Value::Text(s) => out.push_str(s),
        Value::Bool(b) => out.push_str(if b { "VERDADEIRO" } else { "FALSO" }),
        Value::Error(e) => out.push_str(error_text(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{CellXf, Color, Fill, Font};

    fn styles() -> Styles {
        let mut st = Styles::default();
        st.fonts.push(Font { name: "Calibri".into(), size: 11.0, ..Default::default() });
        st.fonts.push(Font {
            name: "Arial".into(),
            size: 14.0,
            bold: true,
            color: Color::Theme { index: 4, tint: 0.0 },
            ..Default::default()
        });
        st.fills.push(Fill::default());
        st.fills.push(Fill { pattern: Some("gray125".into()), ..Default::default() });
        st.fills.push(Fill {
            pattern: Some("solid".into()),
            fg: Color::Theme { index: 0, tint: -0.149_998_47 },
            bg: Color::Indexed(64),
        });
        st.num_fmts.insert(164, "#,##0.00;[Red]-#,##0.00".into());
        st.xfs.push(CellXf::default());
        st.xfs.push(CellXf { font: 1, fill: 2, num_fmt: 164, ..Default::default() });
        st.xfs.push(CellXf { num_fmt: 14, ..Default::default() });
        st
    }

    #[test]
    fn resolves_colors_and_formats() {
        let st = styles();
        let looks = Looks::new(&st);
        let l = looks.get(1);
        assert_eq!(l.fill, Some(rgb(0xD9D9D9)));
        assert_eq!(l.color, rgb(0x4472C4));
        assert!(looks.fonts[l.font as usize].bold);
        let mut s = String::new();
        let r = show(&looks, l, Value::Number(-1234.5), false, &mut s).unwrap();
        assert_eq!(s, "-1.234,50");
        assert_eq!(r.color, Some(rgb(0xFF0000)));
        assert!(!r.general);
        let r = show(&looks, looks.get(2), Value::Number(45000.0), false, &mut s).unwrap();
        assert_eq!(s, "15/03/2023");
        assert_eq!(r.kind, Kind::Number);
        show(&looks, looks.get(0), Value::Bool(true), false, &mut s);
        assert_eq!(s, "VERDADEIRO");
        show(&looks, looks.get(0), Value::Error(CellError::NA), false, &mut s);
        assert_eq!(s, "#N/D");
        // Estilo inexistente: o padrão.
        assert_eq!(looks.get(999).fmt, 0);
    }

    #[test]
    fn formula_bar_text() {
        let st = styles();
        let looks = Looks::new(&st);
        let mut s = String::new();
        edit_text(&looks, looks.get(1), Value::Number(1234.5), false, &mut s);
        assert_eq!(s, "1234,5");
        edit_text(&looks, looks.get(2), Value::Number(45000.0), false, &mut s);
        assert_eq!(s, "15/03/2023");
        edit_text(&looks, looks.get(2), Value::Number(45000.5), false, &mut s);
        assert_eq!(s, "15/03/2023 12:00:00");
    }
}
