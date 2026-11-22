//! Aparência das tabelas do Excel a partir do nome do estilo
//! (`TableStyleLight1`…`TableStyleDark11`), com as cores do tema do arquivo.
//!
//! Os estilos internos do Excel não vêm no arquivo: aqui fica uma
//! aproximação das famílias (cabeçalho, faixas alternadas, linha do
//! cabeçalho), que é o que dá a cara das tabelas na tela. A formatação
//! própria da célula vence o estilo da tabela, como no Excel.

use duck_core::{Styles, apply_tint};

/// O que o estilo de uma tabela pinta.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TableLook {
    pub header_fill: Option<u32>,
    pub header_text: Option<u32>,
    pub header_bold: bool,
    /// Linha embaixo do cabeçalho.
    pub header_line: Option<u32>,
    /// Faixas alternadas (linhas pares de dados).
    pub band_fill: Option<u32>,
    /// Texto do corpo (estilos escuros).
    pub body_text: Option<u32>,
    /// Fundo das linhas sem faixa (estilos escuros).
    pub body_fill: Option<u32>,
}

/// Cor do tema: 0 = escura (dk1), 1..6 = destaques 1..6.
fn color(st: &Styles, k: u32) -> u32 {
    // Índices de `Theme::color`: 1 = dk1, 4..9 = accent1..6.
    let idx = if k == 0 { 1 } else { 3 + k as u8 };
    st.theme.color(idx)
}

pub fn table_look(name: Option<&str>, st: &Styles) -> TableLook {
    let name = name.unwrap_or("TableStyleMedium2");
    let (family, n) = match name.strip_prefix("TableStyle") {
        Some(rest) => {
            let digits = rest.trim_start_matches(|c: char| c.is_ascii_alphabetic());
            let fam = &rest[..rest.len() - digits.len()];
            (fam.to_string(), digits.parse::<u32>().unwrap_or(1).max(1))
        }
        None => ("Medium".into(), 2),
    };
    let k = (n - 1) % 7;
    let base = color(st, k);
    let tint = |t: f32| apply_tint(base, t);
    let white = 0xFFFFFF;
    let black = color(st, 0);
    match family.as_str() {
        "Light" => match (n - 1) / 7 {
            0 => TableLook {
                header_bold: true,
                header_line: Some(base),
                band_fill: Some(if k == 0 { 0xD9D9D9 } else { tint(0.8) }),
                ..Default::default()
            },
            1 => TableLook {
                header_fill: Some(base),
                header_text: Some(white),
                header_bold: true,
                header_line: Some(base),
                ..Default::default()
            },
            _ => TableLook {
                header_bold: true,
                header_line: Some(base),
                band_fill: Some(if k == 0 { 0xD9D9D9 } else { tint(0.8) }),
                ..Default::default()
            },
        },
        "Dark" => TableLook {
            header_fill: Some(black),
            header_text: Some(white),
            header_bold: true,
            body_fill: Some(if k == 0 { 0x737373 } else { apply_tint(base, -0.25) }),
            band_fill: Some(if k == 0 { 0x404040 } else { apply_tint(base, -0.5) }),
            body_text: Some(white),
            ..Default::default()
        },
        // Medium (o padrão do Excel é Medium2).
        _ => match (n - 1) / 7 {
            0 => TableLook {
                header_fill: Some(base),
                header_text: Some(white),
                header_bold: true,
                band_fill: Some(if k == 0 { 0xD9D9D9 } else { tint(0.8) }),
                ..Default::default()
            },
            1 => TableLook {
                header_fill: Some(black),
                header_text: Some(white),
                header_bold: true,
                band_fill: Some(if k == 0 { 0xA6A6A6 } else { tint(0.6) }),
                body_fill: Some(if k == 0 { 0xD9D9D9 } else { tint(0.8) }),
                ..Default::default()
            },
            2 => TableLook {
                header_fill: Some(black),
                header_text: Some(white),
                header_bold: true,
                band_fill: Some(if k == 0 { 0xD9D9D9 } else { tint(0.8) }),
                ..Default::default()
            },
            _ => TableLook {
                header_fill: Some(if k == 0 { 0xA6A6A6 } else { tint(0.4) }),
                header_bold: true,
                band_fill: Some(if k == 0 { 0xD9D9D9 } else { tint(0.6) }),
                body_fill: Some(if k == 0 { 0xF2F2F2 } else { tint(0.8) }),
                ..Default::default()
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        let st = Styles::default();
        let m2 = table_look(Some("TableStyleMedium2"), &st);
        assert!(m2.header_fill.is_some() && m2.header_text == Some(0xFFFFFF) && m2.band_fill.is_some());
        let l1 = table_look(Some("TableStyleLight1"), &st);
        assert_eq!(l1.header_fill, None);
        assert_eq!(l1.band_fill, Some(0xD9D9D9));
        assert!(l1.header_bold);
        let d = table_look(Some("TableStyleDark3"), &st);
        assert_eq!(d.body_text, Some(0xFFFFFF));
    }
}
