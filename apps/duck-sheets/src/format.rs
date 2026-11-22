//! Formatação (F3.4): barra de ferramentas, paleta de cores, atalhos e o
//! comando que troca os estilos da seleção.
//!
//! Cada estilo novo deriva do estilo da célula (`Styles::derive_xf`); fontes,
//! preenchimentos e bordas iguais são reaproveitados. Tudo vira um
//! `Command::SetStyles` (desfazer de uma vez).

use std::collections::HashMap;
use std::sync::Arc;

use duck_core::{Border, BorderSide, BorderStyle, Color, Command, Fill, HAlign, Styles};
use duck_text::TextStyle;
use duck_ui::{Align, Painter, Rect, Rgba};

use crate::app::{Geom, SheetsApp};
use crate::look::Looks;

/// Uma mudança de formato na seleção.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fmt {
    Bold,
    Italic,
    Underline,
    Strike,
    Align(HAlign),
    Wrap,
    /// Formato de número interno do `styles.xml`.
    Number(u16),
    /// RGB; `None` = automática.
    FontColor(Option<u32>),
    /// RGB; `None` = sem preenchimento.
    FillColor(Option<u32>),
    /// Borda fina em volta da seleção.
    Outline,
    /// Borda fina em todas as células da seleção.
    AllBorders,
    NoBorders,
}

/// Botão da barra.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tool {
    Fmt(Fmt),
    TextColor,
    FillColor,
    /// Inserir/apagar linhas/colunas (`true` = inserir; `true` = colunas).
    Shift(bool, bool),
}

/// (ferramenta, rótulo, dica, largura em pixels lógicos; 0 = medida pelo rótulo).
/// `None` = separador.
const BUTTONS: &[Option<(Tool, &str, &str)>] = &[
    Some((Tool::Fmt(Fmt::Bold), "N", "Negrito (Ctrl+N ou Ctrl+B)")),
    Some((Tool::Fmt(Fmt::Italic), "I", "Itálico (Ctrl+I)")),
    Some((Tool::Fmt(Fmt::Underline), "S", "Sublinhado (Ctrl+U)")),
    Some((Tool::Fmt(Fmt::Strike), "abc", "Tachado (Ctrl+5)")),
    None,
    Some((Tool::TextColor, "A", "Cor do texto")),
    Some((Tool::FillColor, "Fundo", "Cor de preenchimento")),
    None,
    Some((Tool::Fmt(Fmt::Align(HAlign::Left)), "", "Alinhar à esquerda (Ctrl+Shift+L)")),
    Some((Tool::Fmt(Fmt::Align(HAlign::Center)), "", "Centralizar (Ctrl+Shift+E)")),
    Some((Tool::Fmt(Fmt::Align(HAlign::Right)), "", "Alinhar à direita (Ctrl+Shift+R)")),
    Some((Tool::Fmt(Fmt::Wrap), "Quebrar", "Quebrar texto")),
    None,
    Some((Tool::Fmt(Fmt::Number(7)), "R$", "Moeda (Ctrl+Shift+4)")),
    Some((Tool::Fmt(Fmt::Number(10)), "%", "Percentual (Ctrl+Shift+5)")),
    Some((Tool::Fmt(Fmt::Number(4)), "000", "Número com milhar (Ctrl+Shift+1)")),
    Some((Tool::Fmt(Fmt::Number(14)), "Data", "Data (Ctrl+Shift+3)")),
    Some((Tool::Fmt(Fmt::Number(0)), "Geral", "Formato geral (Ctrl+Shift+6)")),
    None,
    Some((Tool::Fmt(Fmt::AllBorders), "Bordas", "Bordas em todas as células")),
    Some((Tool::Fmt(Fmt::Outline), "Contorno", "Borda em volta (Ctrl+Shift+7)")),
    Some((Tool::Fmt(Fmt::NoBorders), "Sem bordas", "Tirar as bordas (Ctrl+Shift+-)")),
    None,
    Some((Tool::Shift(true, false), "+ Linha", "Inserir linhas acima da seleção (Ctrl+Alt+=)")),
    Some((Tool::Shift(false, false), "- Linha", "Apagar as linhas da seleção (Ctrl+Alt+-)")),
    Some((Tool::Shift(true, true), "+ Coluna", "Inserir colunas à esquerda da seleção")),
    Some((Tool::Shift(false, true), "- Coluna", "Apagar as colunas da seleção")),
];

/// Cores da paleta (RGB): uma linha viva, uma clara, uma escura.
pub const PALETTE: [u32; 30] = [
    0x000000, 0x434343, 0x666666, 0x999999, 0xB7B7B7, 0xCCCCCC, 0xD9D9D9, 0xEFEFEF, 0xF3F3F3, 0xFFFFFF,
    0x980000, 0xFF0000, 0xFF9900, 0xFFFF00, 0x00FF00, 0x00FFFF, 0x4A86E8, 0x0000FF, 0x9900FF, 0xFF00FF,
    0xE6B8AF, 0xF4CCCC, 0xFCE5CD, 0xFFF2CC, 0xD9EAD3, 0xD0E0E3, 0xC9DAF8, 0xCFE2F3, 0xD9D2E9, 0xEAD1DC,
];

/// Formato pelo nome (opção `--edit formatar:NOME:A1:B2`): `negrito`,
/// `italico`, `sublinhado`, `tachado`, `esquerda`, `centro`, `direita`,
/// `quebrar`, `moeda`, `percentual`, `milhar`, `data`, `geral`, `bordas`,
/// `contorno`, `sembordas`, `cor=RRGGBB`, `fundo=RRGGBB`.
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
        "quebrar" => Fmt::Wrap,
        "moeda" => Fmt::Number(7),
        "percentual" => Fmt::Number(10),
        "milhar" => Fmt::Number(4),
        "data" => Fmt::Number(14),
        "geral" => Fmt::Number(0),
        "bordas" => Fmt::AllBorders,
        "contorno" => Fmt::Outline,
        "sembordas" => Fmt::NoBorders,
        _ => match name.split_once('=') {
            Some(("cor", v)) => Fmt::FontColor(Some(hex(v)?)),
            Some(("fundo", v)) => Fmt::FillColor(Some(hex(v)?)),
            _ => return None,
        },
    })
}

/// Paleta aberta (cor do texto ou do fundo), embaixo do botão.
#[derive(Clone, Copy, Debug)]
pub struct Menu {
    pub tool: Tool,
    pub rect: Rect,
}

const SWATCH: f32 = 20.0;
const GAP: f32 = 4.0;

/// Botões da barra e os retângulos deles.
pub fn layout(
    g: &Geom,
    text: &mut duck_text::TextSystem,
    style: TextStyle,
) -> Vec<(Tool, Rect, &'static str, &'static str)> {
    let s = g.s;
    let r = g.toolbar;
    let mut x = r.x0 + (8.0 * s).round();
    let (y0, y1) = (r.y0 + (4.0 * s).round(), r.y1 - (4.0 * s).round());
    let mut out = Vec::new();
    for b in BUTTONS {
        match b {
            None => x += (10.0 * s).round(),
            Some((tool, label, tip)) => {
                let w = if label.chars().count() <= 1 {
                    (28.0 * s).round()
                } else {
                    (text.measure_styled(label, style) + 16.0 * s).round()
                };
                out.push((*tool, Rect::new(x, y0, x + w, y1), *label, *tip));
                x += w + (2.0 * s).round();
            }
        }
    }
    out
}

/// Retângulo da paleta embaixo do botão `b`.
pub fn menu_rect(g: &Geom, b: Rect) -> Rect {
    let s = g.s;
    let w = (10.0 * SWATCH + 11.0 * GAP) * s;
    let h = (3.0 * SWATCH + 4.0 * GAP + 26.0) * s;
    let x0 = b.x0.min(g.w - w - 4.0 * s).max(0.0);
    Rect::new(x0, b.y1 + 2.0 * s, x0 + w, b.y1 + 2.0 * s + h)
}

/// O que há no ponto da paleta: `Some(Some(rgb))` = cor, `Some(None)` =
/// automática / sem preenchimento, `None` = nada.
pub fn menu_hit(g: &Geom, m: &Menu, x: f32, y: f32) -> Option<Option<u32>> {
    let s = g.s;
    if !m.rect.contains(x, y) {
        return None;
    }
    let (lx, ly) = ((x - m.rect.x0) / s, (y - m.rect.y0) / s);
    if ly < 26.0 {
        return Some(None);
    }
    let col = ((lx - GAP) / (SWATCH + GAP)).floor();
    let row = ((ly - 26.0 - GAP) / (SWATCH + GAP)).floor();
    if !(0.0..10.0).contains(&col) || !(0.0..3.0).contains(&row) {
        return None;
    }
    Some(Some(PALETTE[(row as usize) * 10 + col as usize]))
}

fn rgba(rgb: u32) -> Rgba {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
}

/// Estado (ligado/desligado) dos botões para a célula ativa.
pub struct Active {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub align: HAlign,
    pub wrap: bool,
}

impl Active {
    fn on(&self, t: Tool) -> bool {
        match t {
            Tool::Fmt(Fmt::Bold) => self.bold,
            Tool::Fmt(Fmt::Italic) => self.italic,
            Tool::Fmt(Fmt::Underline) => self.underline,
            Tool::Fmt(Fmt::Strike) => self.strike,
            Tool::Fmt(Fmt::Align(h)) => self.align == h,
            Tool::Fmt(Fmt::Wrap) => self.wrap,
            _ => false,
        }
    }
}

/// Desenha a barra (e a paleta aberta, se houver).
#[allow(clippy::too_many_arguments)]
pub fn draw(
    p: &mut Painter,
    g: &Geom,
    ui: TextStyle,
    active: Option<&Active>,
    hover: Option<Tool>,
    menu: Option<&Menu>,
    colors: (u32, u32),
    theme: (Rgba, Rgba, Rgba, Rgba, Rgba),
) {
    let (bg, line, text_c, hover_bg, on_bg) = theme;
    let s = g.s;
    let px = s.round().max(1.0);
    let r = g.toolbar;
    p.fill(r, &r, bg);
    p.fill(Rect::new(r.x0, r.y1 - px, r.x1, r.y1), &r, line);
    let buttons = layout(g, p.text, ui);
    for (tool, b, label, _) in &buttons {
        let on = active.is_some_and(|a| a.on(*tool));
        if on {
            p.fill(*b, &r, on_bg);
        } else if hover == Some(*tool) {
            p.fill(*b, &r, hover_bg);
        }
        match tool {
            Tool::Fmt(Fmt::Align(h)) => {
                // Ícone: quatro linhas alinhadas.
                let (w, cx, cy) = (14.0 * s, (b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
                for (i, frac) in [1.0f32, 0.6, 1.0, 0.6].iter().enumerate() {
                    let lw = (w * frac).round();
                    let y = (cy - 6.0 * s + i as f32 * 4.0 * s).round();
                    let x0 = match h {
                        HAlign::Left => cx - w / 2.0,
                        HAlign::Right => cx + w / 2.0 - lw,
                        _ => cx - lw / 2.0,
                    }
                    .round();
                    p.fill(Rect::new(x0, y, x0 + lw, y + px.max(2.0 * s).round()), &r, text_c);
                }
            }
            _ => {
                let mut st = ui;
                if matches!(tool, Tool::Fmt(Fmt::Bold)) {
                    st.bold = true;
                }
                if matches!(tool, Tool::Fmt(Fmt::Italic)) {
                    st.italic = true;
                }
                p.label(label, st, *b, Align::Center, 0.0, text_c);
                let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
                let tw = p.text.measure_styled(label, st);
                match tool {
                    Tool::Fmt(Fmt::Underline) => {
                        let y = (cy + 7.0 * s).round();
                        p.fill(
                            Rect::new((cx - tw / 2.0).round(), y, (cx + tw / 2.0).round(), y + px),
                            &r,
                            text_c,
                        );
                    }
                    Tool::Fmt(Fmt::Strike) => {
                        let y = (cy + 1.0 * s).round();
                        p.fill(
                            Rect::new((cx - tw / 2.0).round(), y, (cx + tw / 2.0).round(), y + px),
                            &r,
                            text_c,
                        );
                    }
                    Tool::TextColor | Tool::FillColor => {
                        let c = if *tool == Tool::TextColor { colors.0 } else { colors.1 };
                        let y = (b.y1 - 5.0 * s).round();
                        p.fill(Rect::new(b.x0 + 5.0 * s, y, b.x1 - 5.0 * s, y + 3.0 * s), &r, rgba(c));
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(m) = menu {
        let mr = m.rect;
        p.fill(mr.inset(-px), &Rect::everything(), line);
        p.fill(mr, &Rect::everything(), [255, 255, 255, 255]);
        let label = if m.tool == Tool::TextColor { "Automática" } else { "Sem preenchimento" };
        let top = Rect::new(mr.x0, mr.y0, mr.x1, mr.y0 + 26.0 * s);
        p.label(label, ui, top, Align::Left, 8.0 * s, text_c);
        for (i, &c) in PALETTE.iter().enumerate() {
            let (row, col) = ((i / 10) as f32, (i % 10) as f32);
            let x0 = (mr.x0 + (GAP + col * (SWATCH + GAP)) * s).round();
            let y0 = (mr.y0 + (26.0 + GAP + row * (SWATCH + GAP)) * s).round();
            let sw = Rect::new(x0, y0, x0 + (SWATCH * s).round(), y0 + (SWATCH * s).round());
            p.fill(sw.inset(-px), &Rect::everything(), line);
            p.fill(sw, &Rect::everything(), rgba(c));
        }
    }
}

/// Novo estilo para uma célula de estilo `base` (`edges`: bordas da seleção
/// em que a célula está: esquerda, direita, cima, baixo).
fn restyle(st: &mut Styles, base: u16, f: Fmt, on: bool, edges: [bool; 4]) -> u16 {
    let mut xf = st.xfs.get(base as usize).copied().unwrap_or_default();
    match f {
        Fmt::Bold | Fmt::Italic | Fmt::Underline | Fmt::Strike | Fmt::FontColor(_) => {
            let mut ft = st.fonts.get(xf.font as usize).cloned().unwrap_or_default();
            match f {
                Fmt::Bold => ft.bold = on,
                Fmt::Italic => ft.italic = on,
                Fmt::Underline => ft.underline = on,
                Fmt::Strike => ft.strike = on,
                Fmt::FontColor(c) => ft.color = c.map_or(Color::Auto, |rgb| Color::Rgb(0xFF00_0000 | rgb)),
                _ => {}
            }
            if ft.size == 0.0 {
                ft.size = 11.0;
            }
            if ft.name.is_empty() {
                ft.name = "Calibri".into();
            }
            xf.font = st.intern_font(ft);
        }
        Fmt::Align(h) => xf.alignment.horizontal = if on { h } else { HAlign::General },
        Fmt::Wrap => xf.alignment.wrap = on,
        Fmt::Number(id) => xf.num_fmt = id,
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
        Fmt::Outline | Fmt::AllBorders => {
            let mut b = st.borders.get(xf.border as usize).copied().unwrap_or_default();
            let thin = BorderSide { style: BorderStyle::Thin, color: Color::Auto };
            let all = f == Fmt::AllBorders;
            if all || edges[0] {
                b.left = thin;
            }
            if all || edges[1] {
                b.right = thin;
            }
            if all || edges[2] {
                b.top = thin;
            }
            if all || edges[3] {
                b.bottom = thin;
            }
            xf.border = st.intern_border(b);
        }
        Fmt::NoBorders => xf.border = st.intern_border(Border::default()),
    }
    st.derive_xf(base, |x| *x = xf)
}

impl SheetsApp {
    /// Estado dos botões para a célula ativa.
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
            wrap: xf.alignment.wrap,
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
        let Some(doc) = &mut self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r0, c0, r1, c1) = v.sel();
        // Linha ou coluna inteira: só até onde há dados.
        let r1 = if r1 - r0 + 1 == v.rows.len() { r1.min(sheet.rows().max(r0 + 1) - 1) } else { r1 };
        let c1 = if c1 - c0 + 1 == v.cols.len() { c1.min(sheet.cols().max(c0 + 1) - 1) } else { c1 };
        let n = (r1 - r0 + 1) as u64 * (c1 - c0 + 1) as u64;
        if n > 2_000_000 {
            self.message = "Seleção grande demais para formatar de uma vez".into();
            return;
        }
        let mut cells = Vec::with_capacity(n as usize);
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
        self.run_command(Command::SetStyles(list));
    }

    /// Clique na barra ou na paleta. `true` = o clique foi tratado.
    pub fn toolbar_click(&mut self, g: &Geom, x: f32, y: f32) -> bool {
        if let Some(m) = self.menu.take() {
            if let Some(color) = menu_hit(g, &m, x, y) {
                match m.tool {
                    Tool::TextColor => {
                        self.apply_fmt(Fmt::FontColor(color));
                        self.colors.0 = color.unwrap_or(0);
                    }
                    _ => {
                        self.apply_fmt(Fmt::FillColor(color));
                        self.colors.1 = color.unwrap_or(0xFFFFFF);
                    }
                }
                return true;
            }
            if m.rect.contains(x, y) {
                self.menu = Some(m);
                return true;
            }
        }
        if !g.toolbar.contains(x, y) {
            return false;
        }
        let ui = self.text.as_ref().map(|t| t.default_style());
        let (Some(ui), Some(text)) = (ui, self.text.as_mut()) else { return true };
        let hit = layout(g, text, ui).into_iter().find(|b| b.1.contains(x, y));
        match hit {
            Some((Tool::Fmt(f), ..)) => self.apply_fmt(f),
            Some((Tool::Shift(insert, cols), ..)) => {
                let axis = if cols { duck_core::Axis::Cols } else { duck_core::Axis::Rows };
                self.shift_selection(insert, Some(axis));
            }
            Some((tool, b, ..)) => self.menu = Some(Menu { tool, rect: menu_rect(g, b) }),
            None => {}
        }
        true
    }

    /// Botão sob o mouse (para destacar e mostrar a dica).
    pub fn toolbar_hover(&mut self, g: &Geom, x: f32, y: f32) -> Option<(Tool, &'static str)> {
        if !g.toolbar.contains(x, y) {
            return None;
        }
        let ui = self.text.as_ref()?.default_style();
        let text = self.text.as_mut()?;
        layout(g, text, ui).into_iter().find(|b| b.1.contains(x, y)).map(|b| (b.0, b.3))
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
            ("&" | "7", true) => Fmt::Outline,
            ("_" | "-", true) => Fmt::NoBorders,
            ("l", true) => Fmt::Align(HAlign::Left),
            ("e", true) => Fmt::Align(HAlign::Center),
            ("r", true) => Fmt::Align(HAlign::Right),
            _ => return false,
        };
        self.apply_fmt(f);
        true
    }
}
