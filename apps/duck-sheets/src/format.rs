//! Formatação (F3.4, F3.5): o comando que troca os estilos da seleção, os
//! atalhos e as listas (fontes, tamanhos, formatos de número, cores) que a
//! faixa de opções mostra.
//!
//! Cada estilo novo deriva do estilo da célula (`Styles::derive_xf`); fontes,
//! preenchimentos, bordas e formatos de número iguais são reaproveitados.
//! Tudo vira um `Command::SetStyles` (desfazer de uma vez).

use std::collections::HashMap;
use std::sync::Arc;

use duck_core::{Border, BorderSide, BorderStyle, Color, Command, Fill, HAlign, Styles, VAlign};

use crate::app::SheetsApp;
use crate::look::Looks;

/// Uma mudança de formato na seleção.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fmt {
    Bold,
    Italic,
    Underline,
    Strike,
    Align(HAlign),
    VAlign(VAlign),
    Wrap,
    /// Mais (+1) ou menos (−1) recuo.
    Indent(i8),
    /// Fonte da lista [`FONTS`].
    Font(u8),
    /// Tamanho em décimos de ponto.
    Size(u16),
    /// Um tamanho da lista [`SIZES`] acima (+1) ou abaixo (−1).
    Grow(i8),
    /// Formato de número interno do `styles.xml`.
    Number(u16),
    /// Mais (+1) ou menos (−1) casas decimais.
    Decimals(i8),
    /// RGB; `None` = automática.
    FontColor(Option<u32>),
    /// RGB; `None` = sem preenchimento.
    FillColor(Option<u32>),
    Borders(Borders),
}

/// Bordas aplicadas pela lista de bordas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Borders {
    All,
    Outline,
    ThickOutline,
    Bottom,
    Top,
    Left,
    Right,
    None,
}

/// Fontes oferecidas na lista (as comuns no Windows; no Linux o desenho cai
/// numa parecida se faltar).
pub const FONTS: [&str; 14] = [
    "Aptos",
    "Arial",
    "Calibri",
    "Cambria",
    "Candara",
    "Consolas",
    "Courier New",
    "Georgia",
    "Segoe UI",
    "Tahoma",
    "Times New Roman",
    "Trebuchet MS",
    "Verdana",
    "Liberation Sans",
];

/// Tamanhos da lista, em pontos.
pub const SIZES: [u16; 16] = [8, 9, 10, 11, 12, 14, 16, 18, 20, 22, 24, 26, 28, 36, 48, 72];

/// Formatos de número da lista: (nome, id interno).
pub const NUMBER_FORMATS: [(&str, u16); 11] = [
    ("Geral", 0),
    ("Número", 4),
    ("Moeda", 7),
    ("Contábil", 44),
    ("Data", 14),
    ("Data e hora", 22),
    ("Hora", 21),
    ("Porcentagem", 10),
    ("Fração", 12),
    ("Científico", 11),
    ("Texto", 49),
];

/// Nome do tipo de formato (para a caixa "Formato de número").
pub fn format_name(styles: &Styles, id: u16) -> &'static str {
    if let Some(f) = NUMBER_FORMATS.iter().find(|f| f.1 == id) {
        return f.0;
    }
    let code = styles.num_fmt_code(id);
    let f = duck_core::NumFmt::parse(code);
    let lower = code.to_lowercase();
    if f.is_general() {
        "Geral"
    } else if f.is_date() {
        if lower.contains('h') && (lower.contains('d') || lower.contains('y')) {
            "Data e hora"
        } else if lower.contains('d') || lower.contains('y') || lower.contains("mmm") {
            "Data"
        } else {
            "Hora"
        }
    } else if f.is_percent() {
        "Porcentagem"
    } else if code.contains('@') && !code.contains('0') {
        "Texto"
    } else if code.contains("R$") || code.contains('$') || code.contains('€') {
        if code.contains("* ") || code.contains("_-") { "Contábil" } else { "Moeda" }
    } else if code.contains(['E', 'e']) && code.contains("0E") {
        "Científico"
    } else if code.contains('/') && code.contains('?') {
        "Fração"
    } else if code.contains(['0', '#']) {
        "Número"
    } else {
        "Personalizado"
    }
}

/// Cores da paleta (RGB): cinzas, uma linha viva e uma clara.
pub const PALETTE: [u32; 30] = [
    0x000000, 0x434343, 0x666666, 0x999999, 0xB7B7B7, 0xCCCCCC, 0xD9D9D9, 0xEFEFEF, 0xF3F3F3, 0xFFFFFF,
    0x980000, 0xFF0000, 0xFF9900, 0xFFFF00, 0x00FF00, 0x00FFFF, 0x4A86E8, 0x0000FF, 0x9900FF, 0xFF00FF,
    0xE6B8AF, 0xF4CCCC, 0xFCE5CD, 0xFFF2CC, 0xD9EAD3, 0xD0E0E3, 0xC9DAF8, 0xCFE2F3, 0xD9D2E9, 0xEAD1DC,
];

/// Formato pelo nome (opção `--edit formatar:NOME:A1:B2`): `negrito`,
/// `italico`, `sublinhado`, `tachado`, `esquerda`, `centro`, `direita`,
/// `topo`, `meio`, `base`, `quebrar`, `recuo+`, `recuo-`, `moeda`,
/// `percentual`, `milhar`, `data`, `geral`, `decimais+`, `decimais-`,
/// `maior`, `menor`, `bordas`, `contorno`, `sembordas`, `cor=RRGGBB`,
/// `fundo=RRGGBB`, `fonte=Nome`, `tamanho=12`.
pub fn by_name(name: &str) -> Option<Fmt> {
    let hex = |v: &str| u32::from_str_radix(v, 16).ok();
    Some(match name {
        "negrito" => Fmt::Bold,
        "italico" => Fmt::Italic,
        "sublinhado" => Fmt::Underline,
        "tachado" => Fmt::Strike,
        "esquerda" => Fmt::Align(HAlign::Left),
        "centro" => Fmt::Align(HAlign::Center),
        "direita" => Fmt::Align(HAlign::Right),
        "topo" => Fmt::VAlign(VAlign::Top),
        "meio" => Fmt::VAlign(VAlign::Center),
        "base" => Fmt::VAlign(VAlign::Bottom),
        "quebrar" => Fmt::Wrap,
        "recuo+" => Fmt::Indent(1),
        "recuo-" => Fmt::Indent(-1),
        "moeda" => Fmt::Number(7),
        "percentual" => Fmt::Number(10),
        "milhar" => Fmt::Number(4),
        "data" => Fmt::Number(14),
        "geral" => Fmt::Number(0),
        "decimais+" => Fmt::Decimals(1),
        "decimais-" => Fmt::Decimals(-1),
        "maior" => Fmt::Grow(1),
        "menor" => Fmt::Grow(-1),
        "bordas" => Fmt::Borders(Borders::All),
        "contorno" => Fmt::Borders(Borders::Outline),
        "sembordas" => Fmt::Borders(Borders::None),
        _ => match name.split_once('=') {
            Some(("cor", v)) => Fmt::FontColor(Some(hex(v)?)),
            Some(("fundo", v)) => Fmt::FillColor(Some(hex(v)?)),
            Some(("fonte", v)) => Fmt::Font(FONTS.iter().position(|f| f.eq_ignore_ascii_case(v))? as u8),
            Some(("tamanho", v)) => Fmt::Size((v.replace(',', ".").parse::<f32>().ok()? * 10.0) as u16),
            _ => return None,
        },
    })
}

/// O formato com uma casa decimal a mais (`delta > 0`) ou a menos, em cada
/// seção. `None` se o formato não tem número (texto, data).
pub fn adjust_decimals(code: &str, delta: i8) -> Option<String> {
    if code.eq_ignore_ascii_case("general") {
        return Some(if delta > 0 { "0.0".into() } else { "0".into() });
    }
    let mut out = String::with_capacity(code.len() + 2);
    let mut changed = false;
    for (k, section) in split_sections(code).into_iter().enumerate() {
        if k > 0 {
            out.push(';');
        }
        match adjust_section(section, delta) {
            Some(s) => {
                changed = true;
                out.push_str(&s);
            }
            None => out.push_str(section),
        }
    }
    (changed && out != code).then_some(out)
}

/// Seções separadas por `;` fora de aspas e colchetes.
fn split_sections(code: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut quoted, mut bracket, mut start) = (false, false, 0);
    for (i, c) in code.char_indices() {
        match c {
            '"' if !bracket => quoted = !quoted,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            ';' if !quoted && !bracket => {
                out.push(&code[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&code[start..]);
    out
}

fn adjust_section(s: &str, delta: i8) -> Option<String> {
    // Posições (fora de aspas, colchetes e escapes) dos dígitos e do ponto.
    let b = s.as_bytes();
    let (mut quoted, mut bracket) = (false, false);
    let mut digits = Vec::new();
    let mut dot = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' if !bracket => quoted = !quoted,
            b'[' if !quoted => bracket = true,
            b']' if !quoted => bracket = false,
            b'\\' | b'_' | b'*' if !quoted && !bracket => i += 1,
            b'0' | b'#' | b'?' if !quoted && !bracket => digits.push(i),
            b'.' if !quoted && !bracket && dot.is_none() => dot = Some(i),
            b'y' | b'm' | b'd' | b'h' | b's' | b'@' if !quoted && !bracket => return None,
            _ => {}
        }
        i += 1;
    }
    let last = *digits.last()?;
    // Seção sem `0` (ex.: o zero do contábil, `"-"??`): fica como está.
    if !digits.iter().any(|&p| b[p] == b'0') {
        return None;
    }
    let mut out = s.to_string();
    match (dot, delta > 0) {
        (Some(d), true) => {
            let after = digits.iter().filter(|&&p| p > d).max().copied().unwrap_or(d);
            out.insert(after + 1, '0');
        }
        (None, true) => out.insert_str(last + 1, ".0"),
        (Some(d), false) => {
            let decimals: Vec<usize> = digits.iter().copied().filter(|&p| p > d).collect();
            match decimals.as_slice() {
                [] => return None,
                [only] => {
                    // Tira o último dígito decimal e o ponto.
                    out.remove(*only);
                    out.remove(d);
                }
                [.., last] => {
                    out.remove(*last);
                }
            }
        }
        (None, false) => return None,
    }
    Some(out)
}

/// Estado (ligado/desligado e valores) da célula ativa, para a faixa de opções.
#[derive(Clone, Debug, Default)]
pub struct Active {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub align: HAlign,
    pub valign: VAlign,
    pub wrap: bool,
    pub font: String,
    pub size: f32,
    pub num_fmt: u16,
    /// A célula ativa está numa mesclagem.
    pub merged: bool,
}

/// Novo estilo para uma célula de estilo `base` (`edges`: bordas da seleção
/// em que a célula está: esquerda, direita, cima, baixo).
fn restyle(st: &mut Styles, base: u16, f: Fmt, on: bool, edges: [bool; 4]) -> u16 {
    let mut xf = st.xfs.get(base as usize).copied().unwrap_or_default();
    match f {
        Fmt::Bold
        | Fmt::Italic
        | Fmt::Underline
        | Fmt::Strike
        | Fmt::FontColor(_)
        | Fmt::Font(_)
        | Fmt::Size(_)
        | Fmt::Grow(_) => {
            let mut ft = st.fonts.get(xf.font as usize).cloned().unwrap_or_default();
            if ft.size == 0.0 {
                ft.size = 11.0;
            }
            if ft.name.is_empty() {
                ft.name = "Calibri".into();
            }
            match f {
                Fmt::Bold => ft.bold = on,
                Fmt::Italic => ft.italic = on,
                Fmt::Underline => ft.underline = on,
                Fmt::Strike => ft.strike = on,
                Fmt::FontColor(c) => ft.color = c.map_or(Color::Auto, |rgb| Color::Rgb(0xFF00_0000 | rgb)),
                Fmt::Font(i) => ft.name = FONTS.get(i as usize).copied().unwrap_or("Calibri").into(),
                Fmt::Size(t) => ft.size = (t as f32 / 10.0).clamp(1.0, 409.0),
                Fmt::Grow(d) => {
                    let cur = ft.size;
                    ft.size = if d > 0 {
                        SIZES.iter().map(|&s| s as f32).find(|&s| s > cur + 0.01).unwrap_or(cur)
                    } else {
                        SIZES.iter().rev().map(|&s| s as f32).find(|&s| s < cur - 0.01).unwrap_or(cur)
                    };
                }
                _ => {}
            }
            xf.font = st.intern_font(ft);
        }
        Fmt::Align(h) => xf.alignment.horizontal = if on { h } else { HAlign::General },
        Fmt::VAlign(v) => xf.alignment.vertical = v,
        Fmt::Wrap => xf.alignment.wrap = on,
        Fmt::Indent(d) => {
            let a = &mut xf.alignment;
            a.indent = (a.indent as i16 + d as i16).clamp(0, 250) as u8;
            if a.indent > 0 && !matches!(a.horizontal, HAlign::Left | HAlign::Right | HAlign::Distributed) {
                a.horizontal = HAlign::Left;
            }
        }
        Fmt::Number(id) => xf.num_fmt = id,
        Fmt::Decimals(d) => {
            let code = st.num_fmt_code(xf.num_fmt).to_string();
            if let Some(new) = adjust_decimals(&code, d) {
                xf.num_fmt = st.intern_num_fmt(&new);
            }
        }
        Fmt::FillColor(c) => {
            let fill = match c {
                None => Fill::default(),
                Some(rgb) => Fill {
                    pattern: Some("solid".into()),
                    fg: Color::Rgb(0xFF00_0000 | rgb),
                    bg: Color::Indexed(64),
                },
            };
            xf.fill = st.intern_fill(fill);
        }
        Fmt::Borders(kind) => {
            let mut b = st.borders.get(xf.border as usize).copied().unwrap_or_default();
            let thin = BorderSide { style: BorderStyle::Thin, color: Color::Auto };
            let thick = BorderSide { style: BorderStyle::Medium, color: Color::Auto };
            let [l, r, t, bo] = edges;
            match kind {
                Borders::All => {
                    b.left = thin;
                    b.right = thin;
                    b.top = thin;
                    b.bottom = thin;
                }
                Borders::Outline | Borders::ThickOutline => {
                    let side = if kind == Borders::Outline { thin } else { thick };
                    if l {
                        b.left = side;
                    }
                    if r {
                        b.right = side;
                    }
                    if t {
                        b.top = side;
                    }
                    if bo {
                        b.bottom = side;
                    }
                }
                Borders::Bottom if bo => b.bottom = thin,
                Borders::Top if t => b.top = thin,
                Borders::Left if l => b.left = thin,
                Borders::Right if r => b.right = thin,
                Borders::None => b = Border::default(),
                _ => {}
            }
            xf.border = st.intern_border(b);
        }
    }
    st.derive_xf(base, |x| *x = xf)
}

impl SheetsApp {
    /// Estado da célula ativa (para a faixa de opções).
    pub fn active_format(&self) -> Option<Active> {
        let doc = self.doc.as_ref()?;
        let (sheet, v) = doc.current()?;
        let (r, c) = v.cur;
        let s = sheet.cell(r, c, &doc.strings).style;
        let s = if s == 0 { v.blank_style(r, c) } else { s };
        let xf = doc.styles.xfs.get(s as usize).copied().unwrap_or_default();
        let f = doc.styles.fonts.get(xf.font as usize).cloned().unwrap_or_default();
        Some(Active {
            bold: f.bold,
            italic: f.italic,
            underline: f.underline,
            strike: f.strike,
            align: xf.alignment.horizontal,
            valign: xf.alignment.vertical,
            wrap: xf.alignment.wrap,
            font: if f.name.is_empty() { "Calibri".into() } else { f.name },
            size: if f.size > 0.0 { f.size } else { 11.0 },
            num_fmt: xf.num_fmt,
            merged: v.merges.at(r, c).is_some(),
        })
    }

    /// Aplica um formato na seleção (um comando só).
    pub fn apply_fmt(&mut self, f: Fmt) {
        self.commit_edit();
        let Some(active) = self.active_format() else { return };
        // Botões de ligar/desligar seguem o oposto da célula ativa.
        let on = match f {
            Fmt::Bold => !active.bold,
            Fmt::Italic => !active.italic,
            Fmt::Underline => !active.underline,
            Fmt::Strike => !active.strike,
            Fmt::Align(h) => active.align != h,
            Fmt::Wrap => !active.wrap,
            _ => true,
        };
        let Some(areas) = self.sel_areas(2_000_000) else { return };
        let mut all = Vec::new();
        for range in areas {
            let Some(list) = self.restyled(range, f, on) else { return };
            all.extend(list);
        }
        self.run_command(Command::SetStyles(all));
    }

    /// Os estilos novos das células de `range` com o formato `f` (sem aplicar).
    pub fn restyled(
        &mut self,
        (r0, c0, r1, c1): (u32, u32, u32, u32),
        f: Fmt,
        on: bool,
    ) -> Option<Vec<(u32, u32, u16)>> {
        let doc = self.doc.as_mut()?;
        let (sheet, v) = doc.current()?;
        let mut cells = Vec::with_capacity(((r1 - r0 + 1) * (c1 - c0 + 1)) as usize);
        for r in r0..=r1 {
            for c in c0..=c1 {
                let s = sheet.cell(r, c, &doc.strings).style;
                let base = if s == 0 { v.blank_style(r, c) } else { s };
                cells.push((r, c, base));
            }
        }
        let st = Arc::make_mut(&mut doc.styles);
        let mut cache: HashMap<(u16, u8), u16> = HashMap::new();
        let list: Vec<(u32, u32, u16)> = cells
            .into_iter()
            .map(|(r, c, base)| {
                let edges = [c == c0, c == c1, r == r0, r == r1];
                let key = (base, edges.iter().enumerate().fold(0u8, |m, (i, &e)| m | ((e as u8) << i)));
                let new = *cache.entry(key).or_insert_with(|| restyle(st, base, f, on, edges));
                (r, c, new)
            })
            .collect();
        doc.looks = Arc::new(Looks::new(&doc.styles));
        self.font_key = (0, 0);
        Some(list)
    }

    /// A seleção para formatar ou limpar: linha ou coluna inteira só até
    /// onde há dados. `None` (com aviso) se passar de `max` células.
    pub fn sel_cells(&mut self, max: u64) -> Option<(u32, u32, u32, u32)> {
        let doc = self.doc.as_ref()?;
        let (sheet, v) = doc.current()?;
        let (r0, c0, r1, c1) = v.sel();
        let r1 = if r1 - r0 + 1 == v.rows.len() { r1.min(sheet.rows().max(r0 + 1) - 1) } else { r1 };
        let c1 = if c1 - c0 + 1 == v.cols.len() { c1.min(sheet.cols().max(c0 + 1) - 1) } else { c1 };
        let n = (r1 - r0 + 1) as u64 * (c1 - c0 + 1) as u64;
        if n > max {
            self.message = "Seleção grande demais para fazer de uma vez".into();
            return None;
        }
        Some((r0, c0, r1, c1))
    }

    /// Atalhos de formato (Ctrl + tecla). `true` = tratado.
    pub fn format_shortcut(&mut self, k: &str, shift: bool) -> bool {
        let f = match (k, shift) {
            ("b" | "n", false) => Fmt::Bold,
            ("i", false) => Fmt::Italic,
            ("u", false) => Fmt::Underline,
            ("5", false) => Fmt::Strike,
            ("$" | "4", true) => Fmt::Number(7),
            ("%" | "5", true) => Fmt::Number(10),
            ("!" | "1", true) => Fmt::Number(4),
            ("#" | "3", true) => Fmt::Number(14),
            ("@" | "2", true) => Fmt::Number(20),
            ("^" | "6" | "~" | "`", true) => Fmt::Number(0),
            ("&" | "7", true) => Fmt::Borders(Borders::Outline),
            ("_" | "-", true) => Fmt::Borders(Borders::None),
            ("l", true) => Fmt::Align(HAlign::Left),
            ("e", true) => Fmt::Align(HAlign::Center),
            ("r", true) => Fmt::Align(HAlign::Right),
            (">" | ".", true) => Fmt::Grow(1),
            ("<" | ",", true) => Fmt::Grow(-1),
            _ => return false,
        };
        self.apply_fmt(f);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals() {
        assert_eq!(adjust_decimals("General", 1).as_deref(), Some("0.0"));
        assert_eq!(adjust_decimals("0", 1).as_deref(), Some("0.0"));
        assert_eq!(adjust_decimals("0.00", 1).as_deref(), Some("0.000"));
        assert_eq!(adjust_decimals("0.0", -1).as_deref(), Some("0"));
        assert_eq!(adjust_decimals("#,##0.00", -1).as_deref(), Some("#,##0.0"));
        assert_eq!(adjust_decimals("0%", 1).as_deref(), Some("0.0%"));
        assert_eq!(
            adjust_decimals("\"R$ \"#,##0.00;[Red]\\-\"R$ \"#,##0.00", 1).as_deref(),
            Some("\"R$ \"#,##0.000;[Red]\\-\"R$ \"#,##0.000")
        );
        assert_eq!(adjust_decimals("dd/mm/yyyy", 1), None);
        assert_eq!(adjust_decimals("0", -1), None);
        assert_eq!(adjust_decimals("@", 1), None);
        assert_eq!(
            adjust_decimals("_-* #,##0.00_-;-* #,##0.00_-;_-* \"-\"??_-;_-@_-", 1).as_deref(),
            Some("_-* #,##0.000_-;-* #,##0.000_-;_-* \"-\"??_-;_-@_-")
        );
    }
}
