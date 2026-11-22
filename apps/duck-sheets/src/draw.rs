//! Montagem do frame: só o que está na tela (O(células visíveis)).
//!
//! Ordem de pintura em cada painel: linhas de grade → preenchimentos →
//! resultados da busca → bordas → texto → seleção. Depois os cabeçalhos, a
//! barra de fórmulas, as abas e a barra de status.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;

use duck_core::{
    BorderStyle, CellRange, CfApply, CfCtx, CfKind, HAlign, RichRun, Sheet, Styles, VAlign, Value,
};
use duck_text::TextStyle;
use duck_ui::{Align, InputColors, Painter, Rect, Rgba, Tab, TabColors, rgb};

use crate::app::{Geom, Quad, SheetsApp, View, cell_name, col_name, fmt_int};
use crate::look::{Edge, Kind, Look, Looks, general_fit, show};

pub const WHITE: Rgba = [255, 255, 255, 255];
const GRID: Rgba = rgb(0xE1E1E1);
const HEADER_BG: Rgba = rgb(0xF8F9FA);
const HEADER_TEXT: Rgba = rgb(0x5F6368);
const HEADER_LINE: Rgba = rgb(0xD5D7DA);
const HEADER_SEL: Rgba = rgb(0xD3E3FD);
const HEADER_SEL_FULL: Rgba = rgb(0x0B57D0);
const ACCENT: Rgba = rgb(0x0B57D0);
const EDIT_BORDER: Rgba = ACCENT;
const SEL_FILL: Rgba = [14, 101, 235, 26];
const HIT_FILL: Rgba = [255, 213, 0, 110];
const FROZEN_LINE: Rgba = rgb(0xBDC1C6);
const TRACK: Rgba = rgb(0xF1F3F4);
const THUMB: Rgba = rgb(0xBDC1C6);
const THUMB_ACTIVE: Rgba = rgb(0x80868B);
const UI_TEXT: Rgba = rgb(0x202124);
const UI_MUTED: Rgba = rgb(0x5F6368);
const BAR_LINE: Rgba = rgb(0xDADCE0);

/// Colunas olhadas além da borda da tela para texto que transborda.
const OVERFLOW_LOOK: u32 = 24;

/// Tudo o que o desenho das células precisa, emprestado do app.
struct Ctx<'a> {
    sheet: &'a Sheet,
    view: &'a View,
    looks: &'a Looks,
    styles: &'a Styles,
    strings: &'a duck_core::StringTable,
    date1904: bool,
    fonts: &'a [TextStyle],
    g: f32,
    /// Espessura de uma linha fina (1 px físico por unidade de escala da UI).
    px: f32,
    pad: f32,
    hits: &'a [(u32, u32)],
    cf: CfCtx<'a>,
    /// Tabelas do Excel da aba: (intervalo, linhas de cabeçalho, de totais,
    /// faixas ligadas, aparência).
    tables: Vec<(CellRange, u32, u32, bool, crate::tablestyle::TableLook)>,
}

/// O que a formatação condicional muda numa célula (o que não vier fica do estilo).
#[derive(Clone, Copy, Default)]
struct Fx {
    fill: Option<Rgba>,
    color: Option<Rgba>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    edges: [Option<Edge>; 4],
    /// Barra de dados: fração da largura e cor.
    bar: Option<(f32, Rgba)>,
    /// Ícone: caractere e cor.
    icon: Option<(char, Rgba)>,
    /// Barra ou ícone sem o valor (`showValue="0"`).
    hide: bool,
    /// Preenchimento do estilo da tabela: só vale se a célula não tiver um.
    under: Option<Rgba>,
}

/// Junta a formatação condicional (por cima) com o estilo da tabela (por baixo).
fn merge_fx(cf: Option<Fx>, t: Option<Fx>) -> Option<Fx> {
    match (cf, t) {
        (None, t) => t,
        (c, None) => c,
        (Some(mut c), Some(t)) => {
            c.under = t.under;
            c.color = c.color.or(t.color);
            c.bold = c.bold.or(t.bold);
            for (e, te) in c.edges.iter_mut().zip(t.edges) {
                *e = e.or(te);
            }
            Some(c)
        }
    }
}

impl Ctx<'_> {
    /// Formatação condicional da célula, considerando só as regras `active`
    /// (as que tocam a área visível), em ordem de prioridade.
    fn fx(&self, active: &[usize], r: u32, c: u32, value: Value) -> Option<Fx> {
        if active.is_empty() {
            return None;
        }
        let rules = &self.sheet.layout.cond;
        let mut fx = Fx::default();
        let mut any = false;
        for &k in active {
            let rule = &rules[k];
            if !rule.covers(r, c) {
                continue;
            }
            let Some(a) = self.cf.apply(rule, &self.view.cf[k], r, c, value) else { continue };
            any = true;
            match a {
                CfApply::Dxf(i) => {
                    if let Some(d) = self.styles.dxfs.get(i as usize) {
                        let color = |c| self.styles.rgb(c).map(rgb);
                        fx.fill = fx.fill.or(d.fill.and_then(color));
                        fx.color = fx.color.or(d.color.and_then(color));
                        fx.bold = fx.bold.or(d.bold);
                        fx.italic = fx.italic.or(d.italic);
                        fx.underline = fx.underline.or(d.underline);
                        fx.strike = fx.strike.or(d.strike);
                        for (e, s) in fx.edges.iter_mut().zip(d.border) {
                            if e.is_none()
                                && let Some(s) = s
                            {
                                *e = Some(Edge {
                                    style: s.style,
                                    color: self.styles.rgb(s.color).map_or([0, 0, 0, 255], rgb),
                                });
                            }
                        }
                    }
                }
                CfApply::Scale { lo, hi, t } => {
                    if fx.fill.is_none() {
                        let (a, b) = (
                            self.styles.rgb(lo).unwrap_or(0xFFFFFF),
                            self.styles.rgb(hi).unwrap_or(0xFFFFFF),
                        );
                        let mix = |sh: u32| {
                            let (x, y) = (((a >> sh) & 0xFF) as f32, ((b >> sh) & 0xFF) as f32);
                            (x + (y - x) * t).round() as u32
                        };
                        fx.fill = Some(rgb((mix(16) << 16) | (mix(8) << 8) | mix(0)));
                    }
                }
                CfApply::Bar { frac, color, show_value } => {
                    if fx.bar.is_none() {
                        fx.bar = Some((frac, self.styles.rgb(color).map_or(rgb(0x638EC6), rgb)));
                        fx.hide |= !show_value;
                    }
                }
                CfApply::Icon { set, idx, n, show_value } => {
                    if fx.icon.is_none() {
                        fx.icon = Some(icon(set, idx, n));
                        fx.hide |= !show_value;
                    }
                }
            }
            if rule.stop {
                break;
            }
        }
        any.then_some(fx)
    }

    /// O que o estilo da tabela que contém a célula pinta nela.
    fn table_fx(&self, r: u32, c: u32) -> Option<Fx> {
        let &(range, header, totals, stripes, tl) = self.tables.iter().find(|t| t.0.contains(r, c))?;
        let rgb_o = |c: Option<u32>| c.map(rgb);
        let mut fx = Fx::default();
        if header > 0 && r < range.first_row + header {
            fx.under = rgb_o(tl.header_fill);
            fx.color = rgb_o(tl.header_text);
            fx.bold = tl.header_bold.then_some(true);
            if let Some(line) = tl.header_line {
                fx.edges[3] = Some(Edge { style: BorderStyle::Thin, color: rgb(line) });
            }
        } else if totals > 0 && r > range.last_row - totals {
            fx.bold = Some(true);
            fx.edges[2] = tl.header_line.map(|l| Edge { style: BorderStyle::Double, color: rgb(l) });
        } else {
            let k = r - (range.first_row + header);
            fx.under = if stripes && k.is_multiple_of(2) { rgb_o(tl.band_fill) } else { rgb_o(tl.body_fill) };
            fx.color = rgb_o(tl.body_text);
        }
        Some(fx)
    }

    fn look_of(&self, style: u16, r: u32, c: u32) -> &Look {
        let s = if style == 0 { self.view.blank_style(r, c) } else { style };
        self.looks.get(s)
    }

    fn font(&self, look: &Look) -> TextStyle {
        self.fonts.get(look.font as usize).copied().unwrap_or(self.fonts[0])
    }
}

/// Uma linha ou coluna visível: índice e posição na tela.
#[derive(Clone, Copy)]
struct Span {
    i: u32,
    a: f32,
    b: f32,
}

fn spans(axis: &crate::axis::Axis, origin: f64, start: u32, end: u32, lo: f32, hi: f32, g: f32) -> Vec<Span> {
    let mut out = Vec::with_capacity(64);
    let mut i = axis.index_at(origin as i64).max(start);
    while i < end {
        let a = lo + ((axis.offset(i) as f64 - origin) * g as f64).round() as f32;
        if a >= hi {
            break;
        }
        let b = lo + ((axis.offset(i + 1) as f64 - origin) * g as f64).round() as f32;
        if b > a {
            out.push(Span { i, a, b });
        }
        i += 1;
    }
    out
}

/// Posição na tela do início do item `i` num painel.
fn pos(axis: &crate::axis::Axis, origin: f64, i: u32, lo: f32, g: f32) -> f32 {
    lo + ((axis.offset(i) as f64 - origin) * g as f64).round() as f32
}

impl SheetsApp {
    pub fn build_frame(&mut self) {
        let geom = self.geom();
        self.ensure_font_styles(geom.g);
        let header_style = self.header_style(geom.g);
        let mut out = std::mem::take(&mut self.instances);
        out.clear();
        let mut buf = std::mem::take(&mut self.buf);
        let mut buf2 = std::mem::take(&mut self.buf2);
        let stats = self.sel_stats();
        let mut formula = String::new();
        self.formula_text(&mut formula);
        let active = self.active_format();
        let filter_buttons = self.filter_buttons(&geom);

        let (vthumb, hthumb) = (self.vthumb(&geom), self.hthumb(&geom));
        let text = self.text.as_mut().expect("texto");
        let ui = text.default_style();
        let mut p = Painter::new(&mut out, text);
        let s = geom.s;
        let px = s.round().max(1.0);

        match self.doc.as_ref().and_then(|d| d.current().map(|c| (d, c))) {
            Some((doc, (sheet, view))) => {
                let hits: &[(u32, u32)] =
                    self.find.as_ref().and_then(|f| f.result.as_ref()).map_or(&[], |r| r.hits.as_slice());
                let cx = Ctx {
                    sheet,
                    view,
                    looks: &doc.looks,
                    styles: &doc.styles,
                    strings: &doc.strings,
                    date1904: doc.date1904,
                    fonts: &self.font_styles,
                    g: geom.g,
                    px,
                    pad: (3.0 * geom.g).round().max(1.0),
                    hits,
                    cf: CfCtx { sheet, shared: &doc.strings, today: self.today },
                    tables: sheet
                        .layout
                        .tables
                        .iter()
                        .map(|t| {
                            let look = crate::tablestyle::table_look(t.style.as_deref(), &doc.styles);
                            (t.range, t.header_rows, t.totals_rows, t.row_stripes, look)
                        })
                        .collect(),
                };
                // Fundo da grade.
                p.fill(geom.grid, &geom.grid, WHITE);
                for q in geom.quads.iter().rev() {
                    if !q.rect.is_empty() {
                        draw_cells(&mut p, &cx, q, &mut buf, &mut buf2);
                    }
                }
                // Linhas dos painéis congelados.
                if view.frozen_rows > 0 {
                    let y = geom.grid.y0 + geom.fh;
                    p.fill(
                        Rect::new(geom.rowhdr.x0, y - px, geom.grid.x1, y),
                        &Rect::everything(),
                        FROZEN_LINE,
                    );
                }
                if view.frozen_cols > 0 {
                    let x = geom.grid.x0 + geom.fw;
                    p.fill(
                        Rect::new(x - px, geom.colhdr.y0, x, geom.grid.y1),
                        &Rect::everything(),
                        FROZEN_LINE,
                    );
                }
                if view.headers {
                    draw_headers(&mut p, &cx, &geom, header_style, &mut buf);
                }
                // Editor sobre a célula.
                if let Some(ed) = &self.editor
                    && let Some(cell) = cell_screen_rect(&geom, view, ed.row, ed.col)
                {
                    let style = ui.with_px((11.0 * 96.0 / 72.0 * geom.g).round().max(6.0));
                    let pad = (3.0 * geom.g).round().max(1.0);
                    let need = p.text.measure_styled(&ed.input.text, style) + 2.0 * pad + 8.0 * geom.g;
                    let r =
                        Rect::new(cell.x0, cell.y0, (cell.x0 + need).max(cell.x1).min(geom.grid.x1), cell.y1);
                    let colors =
                        InputColors { focus: EDIT_BORDER, border: EDIT_BORDER, ..InputColors::default() };
                    ed.input.draw(&mut p, r, style, pad, !ed.in_bar, &colors, "");
                    p.stroke(r.inset(-px), &Rect::everything(), 2.0 * px, EDIT_BORDER);
                }
                draw_scrollbars(&mut p, vthumb, &geom, self.drag);
            }
            None => {
                p.fill(
                    geom.grid.intersect(&Rect::new(0.0, geom.fbar.y1, geom.w, geom.tabs.y0)),
                    &Rect::everything(),
                    WHITE,
                );
                let msg = if self.loader.is_some() || self.doc.is_some() {
                    if self.message.is_empty() { "Carregando…".to_string() } else { self.message.clone() }
                } else {
                    "Arraste um arquivo .xlsx, .xlsm ou .csv para cá, ou pressione Ctrl+O para abrir"
                        .to_string()
                };
                let area = Rect::new(0.0, geom.fbar.y1, geom.w, geom.tabs.y0);
                let big = ui.with_px(15.0 * s);
                p.label(&msg, big, area, Align::Center, 0.0, UI_MUTED);
            }
        }

        // Barra de fórmulas.
        let fbar = geom.fbar;
        p.fill(fbar, &fbar, WHITE);
        p.fill(Rect::new(0.0, fbar.y1 - px, geom.w, fbar.y1), &fbar, BAR_LINE);
        let nb = geom.namebox.inset((4.0 * s).round());
        if self.namebox_focus {
            self.namebox.draw(&mut p, nb, ui, 6.0 * s, true, &InputColors::default(), "");
        } else {
            buf.clear();
            if let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) {
                let (r0, c0, r1, c1) = v.sel();
                if (r0, c0) != (r1, c1) && !(r1 - r0 + 1 == v.rows.len() || c1 - c0 + 1 == v.cols.len()) {
                    let _ = write!(buf, "{}L × {}C", r1 - r0 + 1, c1 - c0 + 1);
                } else {
                    cell_name(v.cur.0, v.cur.1, &mut buf);
                }
            }
            p.fill(nb, &nb, WHITE);
            p.stroke(nb, &nb, px, BAR_LINE);
            p.label(&buf, ui, nb, Align::Left, 6.0 * s, UI_TEXT);
        }
        p.fill(
            Rect::new(geom.namebox.x1, fbar.y0 + 6.0 * s, geom.namebox.x1 + px, fbar.y1 - 6.0 * s),
            &fbar,
            BAR_LINE,
        );
        let fx = Rect::new(geom.namebox.x1, fbar.y0, geom.formula.x0, fbar.y1);
        let mut italic = ui;
        italic.italic = true;
        p.label("fx", italic, fx, Align::Center, 0.0, UI_MUTED);
        match &self.editor {
            Some(ed) => {
                let colors = InputColors { border: WHITE, focus: WHITE, ..InputColors::default() };
                ed.input.draw(&mut p, geom.formula.inset(2.0 * s), ui, 2.0 * s, ed.in_bar, &colors, "");
            }
            None => {
                // Uma linha só: quebras viram espaço.
                let formula = formula.replace(['\n', '\r'], " ");
                p.label(&formula, ui, geom.formula, Align::Left, 4.0 * s, UI_TEXT);
            }
        }

        // Busca.
        if let Some(f) = &self.find {
            let r = find_rect(&geom);
            p.fill(r.inset(-px), &Rect::everything(), BAR_LINE);
            p.fill(r, &r, WHITE);
            let input = Rect::new(r.x0 + 6.0 * s, r.y0 + 5.0 * s, r.x1 - 120.0 * s, r.y1 - 5.0 * s);
            f.input.draw(&mut p, input, ui, 6.0 * s, f.focused, &InputColors::default(), "Buscar na aba");
            buf.clear();
            if f.job.is_some() {
                buf.push_str("buscando…");
            } else if let Some(res) = &f.result {
                if res.hits.is_empty() {
                    buf.push_str("0 de 0");
                } else {
                    let _ = write!(buf, "{} de {}", fmt_int(f.current as u64 + 1), fmt_int(res.total as u64));
                }
            }
            let info = Rect::new(input.x1, r.y0, r.x1, r.y1);
            p.label(&buf, ui, info, Align::Center, 4.0 * s, UI_MUTED);
        }

        // Abas.
        let mut tabs = Vec::new();
        let mut active_tab = 0;
        if let Some(doc) = &self.doc {
            for (i, slot) in doc.sheets.iter().enumerate() {
                if slot.info.hidden {
                    continue;
                }
                if i == doc.active {
                    active_tab = tabs.len();
                }
                let color = slot
                    .data
                    .as_ref()
                    .and_then(|d| d.layout.view)
                    .and_then(|v| v.tab_color)
                    .and_then(|c| doc.styles.rgb(c))
                    .map(rgb);
                let mut label = slot.info.name.clone();
                if slot.data.is_none() || slot.preview {
                    label.push_str(" …");
                }
                tabs.push(Tab { label, color });
            }
        }
        let tabs_rect = geom.tabs;
        self.tabbar.draw(&mut p, tabs_rect, &tabs, active_tab, ui, 14.0 * s, &TabColors::default());
        // Aba ativa nova: traz para a vista (depois o usuário rola à vontade).
        let active_sheet = self.doc.as_ref().map(|d| d.active);
        if tabs.len() > 1 && self.tab_revealed != active_sheet {
            self.tabbar.reveal(active_tab, tabs_rect.w());
            self.tab_revealed = active_sheet;
        }
        p.fill(Rect::new(tabs_rect.x1, tabs_rect.y0, geom.w, tabs_rect.y1), &Rect::everything(), HEADER_BG);
        p.fill(
            Rect::new(tabs_rect.x1, tabs_rect.y0, geom.w, tabs_rect.y0 + px),
            &Rect::everything(),
            BAR_LINE,
        );
        if self.doc.as_ref().and_then(|d| d.current()).is_some() {
            draw_hscroll(&mut p, hthumb, &geom, self.drag);
        }

        // Barra de status.
        let st = geom.status;
        p.fill(st, &st, HEADER_BG);
        p.fill(Rect::new(0.0, st.y0, geom.w, st.y0 + px), &st, BAR_LINE);
        let small = ui.with_px(12.0 * s);
        let left = match (&self.hover_link, self.hover_act) {
            (_, Some((_, tip))) => tip.to_string(),
            (Some(l), _) => format!("Ctrl+clique para abrir: {l}"),
            _ => self.message.clone(),
        };
        p.label(&left, small, st, Align::Left, 10.0 * s, UI_MUTED);
        buf.clear();
        if let Some(stats) = stats
            && stats.count > 1
        {
            if stats.numbers > 0 {
                buf2.clear();
                duck_core::numfmt::format_general(stats.sum, 15, &mut buf2);
                let _ = write!(buf, "Soma: {buf2}   ");
                buf2.clear();
                duck_core::numfmt::format_general(stats.sum / stats.numbers as f64, 11, &mut buf2);
                let _ = write!(buf, "Média: {buf2}   ");
            }
            let _ = write!(buf, "Contagem: {}   ", fmt_int(stats.count));
        }
        if let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) {
            let _ = write!(buf, "{}%", v.zoom);
        }
        if !self.recent_cpu.is_empty() {
            let avg = self.recent_cpu.iter().sum::<f64>() / self.recent_cpu.len() as f64;
            let _ = write!(buf, "   ·   {avg:.2} ms/frame");
        }
        p.label(&buf, small, st, Align::Right, 10.0 * s, UI_MUTED);

        // Faixa de opções, menus e caixa de diálogo por último: ficam por cima.
        let theme = crate::ribbon::Theme {
            bg: HEADER_BG,
            body: WHITE,
            line: BAR_LINE,
            text: UI_TEXT,
            muted: UI_MUTED,
            hover: rgb(0xE8EAED),
            on: HEADER_SEL,
            accent: ACCENT,
        };
        let view = self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| (v.grid, v.headers));
        let num_fmt_name = match (active.as_ref(), self.doc.as_ref()) {
            (Some(a), Some(d)) => crate::format::format_name(&d.styles, a.num_fmt),
            _ => "",
        };
        let state = crate::ribbon::State {
            active: active.as_ref(),
            grid: view.is_none_or(|v| v.0),
            headers: view.is_none_or(|v| v.1),
            colors: self.colors,
            num_fmt_name,
        };
        let file_open = self
            .popup
            .as_ref()
            .is_some_and(|m| m.owner == Some(crate::ribbon::Act::Drop(crate::ribbon::Drop::File)));
        crate::filter::draw_buttons(&mut p, &geom, &filter_buttons, &theme);
        crate::ribbon::draw(
            &mut p,
            &geom,
            self.ribbon_tab,
            ui,
            &state,
            self.hover_act.map(|t| t.0),
            file_open,
            &theme,
        );
        if let Some(m) = &self.popup {
            m.draw(&mut p, &geom, ui, self.mouse, &theme);
        }
        if let Some(fp) = &self.filter_panel {
            fp.draw(&mut p, &geom, ui, self.mouse, &theme);
        }
        if let Some(pr) = &self.prompt {
            pr.draw(&mut p, &geom, ui, &theme);
        }

        self.instances = out;
        self.buf = buf;
        self.buf2 = buf2;
    }
}

/// Retângulo na tela da célula (ou da mesclagem que começa nela), recortado
/// ao painel em que ela aparece. `None` se não estiver visível.
pub fn cell_screen_rect(g: &Geom, v: &crate::app::View, r: u32, c: u32) -> Option<Rect> {
    let q = g.quads.iter().find(|q| {
        !q.rect.is_empty() && (q.row_start..q.row_end).contains(&r) && (q.col_start..q.col_end).contains(&c)
    })?;
    let (r1, c1) = v.merges.at(r, c).map_or((r, c), |m| (m.last_row, m.last_col));
    let x0 = pos(&v.cols, q.ox, c, q.rect.x0, g.g);
    let x1 = pos(&v.cols, q.ox, c1 + 1, q.rect.x0, g.g);
    let y0 = pos(&v.rows, q.oy, r, q.rect.y0, g.g);
    let y1 = pos(&v.rows, q.oy, r1 + 1, q.rect.y0, g.g);
    let rect = Rect::new(x0, y0, x1.max(x0 + 1.0), y1.max(y0 + 1.0)).intersect(&q.rect);
    (!rect.is_empty()).then_some(rect)
}

/// Caixa de busca (canto superior direito da grade).
pub fn find_rect(g: &Geom) -> Rect {
    let w = (420.0 * g.s).round().min(g.grid.w());
    let h = (38.0 * g.s).round();
    let x1 = g.grid.x1 - (16.0 * g.s).round();
    let y0 = g.grid.y0 + (8.0 * g.s).round();
    Rect::new(x1 - w, y0, x1, y0 + h)
}

// ---------------------------------------------------------------------------
// Células
// ---------------------------------------------------------------------------

fn draw_cells(p: &mut Painter, cx: &Ctx, q: &Quad, buf: &mut String, buf2: &mut String) {
    let v = cx.view;
    let g = cx.g;
    let px = cx.px;
    let clip = q.rect;
    let rows = spans(&v.rows, q.oy, q.row_start, q.row_end, clip.y0, clip.y1, g);
    let cols = spans(&v.cols, q.ox, q.col_start, q.col_end, clip.x0, clip.x1, g);
    if rows.is_empty() || cols.is_empty() {
        return;
    }
    let (r_first, r_last) = (rows[0].i, rows[rows.len() - 1].i);
    let (c_first, c_last) = (cols[0].i, cols[cols.len() - 1].i);
    let nc = cols.len();
    let ci_of = |c: u32| cols.binary_search_by_key(&c, |s| s.i).ok();
    let ri_of = |r: u32| rows.binary_search_by_key(&r, |s| s.i).ok();
    let x_of = |c: u32| pos(&v.cols, q.ox, c, clip.x0, g);
    let y_of = |r: u32| pos(&v.rows, q.oy, r, clip.y0, g);

    // Mesclagens visíveis e a máscara de células cobertas.
    let merges: Vec<CellRange> = v
        .merges
        .in_rows(r_first, r_last)
        .filter(|m| m.last_col >= c_first && m.first_col <= c_last)
        .copied()
        .collect();
    let mut covered = vec![0u16; rows.len() * nc];
    for (k, m) in merges.iter().enumerate().take(u16::MAX as usize - 1) {
        for (ri, r) in rows.iter().enumerate() {
            if r.i < m.first_row || r.i > m.last_row {
                continue;
            }
            for (ci, c) in cols.iter().enumerate() {
                if c.i >= m.first_col && c.i <= m.last_col {
                    covered[ri * nc + ci] = k as u16 + 1;
                }
            }
        }
    }
    // Células visíveis (um acesso por célula no frame).
    let mut cells = Vec::with_capacity(rows.len() * nc);
    for r in &rows {
        for c in &cols {
            cells.push(cx.sheet.cell(r.i, c.i, cx.strings));
        }
    }
    let grid_bottom = rows[rows.len() - 1].b;
    let grid_right = cols[nc - 1].b;
    // Formatação condicional: regras que tocam a área visível e o efeito em cada célula.
    let active: Vec<usize> = cx
        .sheet
        .layout
        .cond
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            !matches!(r.kind, CfKind::Unsupported) && r.touches(r_first, r_last, c_first, c_last)
        })
        .map(|(k, _)| k)
        .collect();
    let view_rect = CellRange { first_row: r_first, first_col: c_first, last_row: r_last, last_col: c_last };
    let tables_here = cx.tables.iter().any(|t| {
        t.0.first_row <= view_rect.last_row
            && view_rect.first_row <= t.0.last_row
            && t.0.first_col <= view_rect.last_col
            && view_rect.first_col <= t.0.last_col
    });
    let fxs: Vec<Option<Fx>> = if active.is_empty() && !tables_here {
        Vec::new()
    } else {
        let mut v = Vec::with_capacity(cells.len());
        for (ri, r) in rows.iter().enumerate() {
            for (ci, c) in cols.iter().enumerate() {
                let t = if tables_here { cx.table_fx(r.i, c.i) } else { None };
                v.push(merge_fx(cx.fx(&active, r.i, c.i, cells[ri * nc + ci].value), t));
            }
        }
        v
    };
    let fx_at = |ri: usize, ci: usize| fxs.get(ri * nc + ci).copied().flatten();
    let merge_fx: Vec<Option<Fx>> = merges
        .iter()
        .map(|m| {
            merge_fx(
                cx.fx(
                    &active,
                    m.first_row,
                    m.first_col,
                    cx.sheet.cell(m.first_row, m.first_col, cx.strings).value,
                ),
                cx.table_fx(m.first_row, m.first_col),
            )
        })
        .collect();

    // 1. Linhas de grade.
    if v.grid {
        for c in &cols {
            p.fill(Rect::new(c.b - px, clip.y0, c.b, grid_bottom), &clip, GRID);
        }
        for r in &rows {
            p.fill(Rect::new(clip.x0, r.b - px, grid_right, r.b), &clip, GRID);
        }
    }

    // 2. Preenchimentos.
    for (ri, r) in rows.iter().enumerate() {
        for (ci, c) in cols.iter().enumerate() {
            if covered[ri * nc + ci] != 0 {
                continue;
            }
            let cell = &cells[ri * nc + ci];
            let fx = fx_at(ri, ci);
            if let Some(fill) =
                fx.and_then(|f| f.fill).or(cx.look_of(cell.style, r.i, c.i).fill).or(fx.and_then(|f| f.under))
            {
                p.fill(Rect::new(c.a - px, r.a - px, c.b, r.b), &clip, fill);
            }
            if let Some((frac, color)) = fx.and_then(|f| f.bar) {
                data_bar(p, Rect::new(c.a, r.a, c.b - px, r.b - px), frac, color, px, &clip);
            }
        }
    }
    for (mi, m) in merges.iter().enumerate() {
        let rect =
            Rect::new(x_of(m.first_col), y_of(m.first_row), x_of(m.last_col + 1), y_of(m.last_row + 1));
        let anchor = cx.sheet.cell(m.first_row, m.first_col, cx.strings);
        let fx = merge_fx[mi];
        if let Some((frac, color)) = fx.and_then(|f| f.bar) {
            data_bar(p, Rect::new(rect.x0, rect.y0, rect.x1 - px, rect.y1 - px), frac, color, px, &clip);
        }
        match fx
            .and_then(|f| f.fill)
            .or(cx.look_of(anchor.style, m.first_row, m.first_col).fill)
            .or(fx.and_then(|f| f.under))
        {
            Some(fill) => p.fill(Rect::new(rect.x0 - px, rect.y0 - px, rect.x1, rect.y1), &clip, fill),
            // Sem preenchimento: apaga as linhas de grade de dentro.
            None => p.fill(Rect::new(rect.x0, rect.y0, rect.x1 - px, rect.y1 - px), &clip, WHITE),
        }
    }

    // 3. Resultados da busca visíveis.
    if !cx.hits.is_empty() {
        let a = cx.hits.partition_point(|&h| h < (r_first, 0));
        for &(r, c) in cx.hits[a..].iter().take_while(|h| h.0 <= r_last) {
            if let (Some(ri), Some(ci)) = (ri_of(r), ci_of(c)) {
                let (rs, cs) = (rows[ri], cols[ci]);
                p.fill(Rect::new(cs.a, rs.a, cs.b - px, rs.b - px), &clip, HIT_FILL);
            }
        }
    }

    // 4. Bordas.
    for (ri, r) in rows.iter().enumerate() {
        for (ci, c) in cols.iter().enumerate() {
            let cell = &cells[ri * nc + ci];
            let look = cx.look_of(cell.style, r.i, c.i);
            let fx_edges = fx_at(ri, ci).map_or([None; 4], |f| f.edges);
            let edges = [0, 1, 2, 3].map(|k| fx_edges[k].or(look.edges[k]));
            if edges.iter().all(Option::is_none) {
                continue;
            }
            // Dentro de uma mesclagem, só as bordas do contorno dela.
            let m = match covered[ri * nc + ci] {
                0 => None,
                k => Some(merges[k as usize - 1]),
            };
            let on = |side: usize| match m {
                None => true,
                Some(m) => match side {
                    0 => c.i == m.first_col,
                    1 => c.i == m.last_col,
                    2 => r.i == m.first_row,
                    _ => r.i == m.last_row,
                },
            };
            let [left, right, top, bottom] = edges;
            if let Some(e) = left.filter(|_| on(0)) {
                edge(p, &clip, false, c.a - px, r.a - px, r.b, e, px, g);
            }
            if let Some(e) = right.filter(|_| on(1)) {
                edge(p, &clip, false, c.b - px, r.a - px, r.b, e, px, g);
            }
            if let Some(e) = top.filter(|_| on(2)) {
                edge(p, &clip, true, r.a - px, c.a - px, c.b, e, px, g);
            }
            if let Some(e) = bottom.filter(|_| on(3)) {
                edge(p, &clip, true, r.b - px, c.a - px, c.b, e, px, g);
            }
        }
    }

    // 5. Texto. Colunas além da tela entram para o texto que transborda.
    let ext_lo = {
        let mut c = c_first;
        let mut n = 0;
        while c > q.col_start && n < OVERFLOW_LOOK {
            c -= 1;
            n += 1;
        }
        c
    };
    let ext_hi = (c_last + OVERFLOW_LOOK).min(q.col_end - 1);
    for (ri, r) in rows.iter().enumerate() {
        let mut c = ext_lo;
        while c <= ext_hi {
            let this = c;
            c += 1;
            let ci = ci_of(this);
            let fx = match ci {
                Some(ci) => fx_at(ri, ci),
                None if !active.is_empty() => {
                    cx.fx(&active, r.i, this, cx.sheet.cell(r.i, this, cx.strings).value)
                }
                None => None,
            };
            let cell = match ci {
                Some(ci) => {
                    if covered[ri * nc + ci] != 0 {
                        continue;
                    }
                    cells[ri * nc + ci]
                }
                None => {
                    if !cx.sheet.has_value(r.i, this) || v.cols.size(this) == 0 {
                        continue;
                    }
                    if v.merges.at(r.i, this).is_some() {
                        continue;
                    }
                    cx.sheet.cell(r.i, this, cx.strings)
                }
            };
            if matches!(cell.value, Value::Empty) {
                continue;
            }
            let look = cx.look_of(cell.style, r.i, this);
            let (x0, x1) = match ci {
                Some(ci) => (cols[ci].a, cols[ci].b),
                None => (x_of(this), x_of(this + 1)),
            };
            let Some(shown) = show(cx.looks, look, cell.value, cx.date1904, buf) else { continue };
            if buf.is_empty() {
                continue;
            }
            let cell_rect = Rect::new(x0, r.a, x1, r.b);
            draw_cell_text(
                p,
                cx,
                q,
                ri,
                &rows,
                &cols,
                this,
                cell_rect,
                look,
                shown,
                cell.value,
                buf,
                buf2,
                true,
                fx.as_ref(),
            );
        }
    }
    for (mi, m) in merges.iter().enumerate() {
        let rect =
            Rect::new(x_of(m.first_col), y_of(m.first_row), x_of(m.last_col + 1), y_of(m.last_row + 1));
        let cell = cx.sheet.cell(m.first_row, m.first_col, cx.strings);
        let look = cx.look_of(cell.style, m.first_row, m.first_col);
        if let Some(shown) = show(cx.looks, look, cell.value, cx.date1904, buf)
            && !buf.is_empty()
        {
            draw_cell_text(
                p,
                cx,
                q,
                usize::MAX,
                &rows,
                &cols,
                m.first_col,
                rect,
                look,
                shown,
                cell.value,
                buf,
                buf2,
                false,
                merge_fx[mi].as_ref(),
            );
        }
    }

    // 6. Seleção.
    let (r0, c0, r1, c1) = v.sel();
    if r1 >= r_first && r0 <= r_last && c1 >= c_first && c0 <= c_last {
        let sel = Rect::new(x_of(c0), y_of(r0), x_of(c1 + 1), y_of(r1 + 1));
        let cur = v.merges.at(v.cur.0, v.cur.1).unwrap_or(CellRange {
            first_row: v.cur.0,
            first_col: v.cur.1,
            last_row: v.cur.0,
            last_col: v.cur.1,
        });
        let active = Rect::new(
            x_of(cur.first_col),
            y_of(cur.first_row),
            x_of(cur.last_col + 1),
            y_of(cur.last_row + 1),
        );
        if (r0, c0) != (r1, c1) {
            // Área selecionada menos a célula ativa (que fica branca, como no Excel).
            let parts = [
                Rect::new(sel.x0, sel.y0, sel.x1, active.y0),
                Rect::new(sel.x0, active.y1, sel.x1, sel.y1),
                Rect::new(sel.x0, active.y0, active.x0, active.y1),
                Rect::new(active.x1, active.y0, sel.x1, active.y1),
            ];
            for part in parts {
                p.fill(part.intersect(&sel), &clip, SEL_FILL);
            }
            p.stroke(sel.inset(-px), &clip, px, ACCENT);
        }
        let b = (2.0 * cx.px).max(2.0);
        p.stroke(Rect::new(active.x0 - px, active.y0 - px, active.x1, active.y1), &clip, b, ACCENT);
    }
}

/// Desenha uma borda. `at` = coordenada da linha de grade (esquerda/topo do
/// pixel); `from..to` = extensão ao longo da linha.
#[allow(clippy::too_many_arguments)]
fn edge(
    p: &mut Painter,
    clip: &Rect,
    horizontal: bool,
    at: f32,
    from: f32,
    to: f32,
    e: Edge,
    px: f32,
    g: f32,
) {
    let w = (e.style.width() as f32 * px).max(px);
    let start = at - ((w - px) / 2.0).floor();
    let rect = |a: f32, b: f32| {
        if horizontal { Rect::new(from, a, to + px, b) } else { Rect::new(a, from, b, to + px) }
    };
    match e.style {
        BorderStyle::Double => {
            p.fill(rect(start, start + px), clip, e.color);
            p.fill(rect(start + 2.0 * px, start + 3.0 * px), clip, e.color);
        }
        s => match s.dash() {
            Some((period, on)) => {
                let k = (g / px).max(1.0) * px;
                p.dashed(rect(start, start + w), clip, e.color, period * k, on * k);
            }
            None => p.fill(rect(start, start + w), clip, e.color),
        },
    }
}

/// Texto de uma célula (ou mesclagem, `ri == usize::MAX`), com alinhamento,
/// quebra de linha, transbordo para vizinhas vazias e `###`.
#[allow(clippy::too_many_arguments)]
fn draw_cell_text(
    p: &mut Painter,
    cx: &Ctx,
    q: &Quad,
    ri: usize,
    rows: &[Span],
    cols: &[Span],
    col: u32,
    cell: Rect,
    look: &Look,
    shown: crate::look::Shown,
    value: Value,
    buf: &mut String,
    buf2: &mut String,
    can_overflow: bool,
    fx: Option<&Fx>,
) {
    let mut style = cx.font(look);
    let pad = cx.pad;
    let px = cx.px;
    let mut color = shown.color.unwrap_or(look.color);
    let mut cell = cell;
    let font = cx.looks.fonts.get(look.font as usize);
    let (mut underline, mut strike) = (font.is_some_and(|f| f.underline), font.is_some_and(|f| f.strike));
    if let Some(fx) = fx {
        color = fx.color.unwrap_or(color);
        underline = fx.underline.unwrap_or(underline);
        strike = fx.strike.unwrap_or(strike);
        if let Some(f) = font
            && (fx.bold.is_some_and(|b| b != f.bold) || fx.italic.is_some_and(|i| i != f.italic))
        {
            style = p.text.style(
                &f.name,
                f.pt * 96.0 / 72.0 * cx.g,
                fx.bold.unwrap_or(f.bold),
                fx.italic.unwrap_or(f.italic),
            );
        }
        if let Some((ch, ic)) = fx.icon {
            // Ícone à esquerda; o texto fica no resto da célula.
            let lh = p.text.line_height_of(style);
            let mut tmp = [0u8; 4];
            let s = ch.encode_utf8(&mut tmp);
            let w = p.text.measure_styled(s, style);
            let y = (cell.y1 - px - lh).max(cell.y0 + px).floor();
            let clip = Rect::new(cell.x0, cell.y0, cell.x1 - px, cell.y1 - px).intersect(&q.rect);
            p.text(s, style, cell.x0 + pad, y, &clip, ic);
            cell.x0 += w + pad;
        }
        if fx.hide {
            return;
        }
    }
    let lh = p.text.line_height_of(style);
    let avail = (cell.w() - 2.0 * pad).max(0.0);
    let indent = look.indent as f32 * 3.0 * (style.px() * 0.55).round();
    let align = match look.h {
        HAlign::General => match shown.kind {
            Kind::Number => Align::Right,
            Kind::Text => Align::Left,
            Kind::Logical => Align::Center,
        },
        HAlign::Left | HAlign::Fill | HAlign::Justify | HAlign::Distributed => Align::Left,
        HAlign::Center | HAlign::CenterContinuous => Align::Center,
        HAlign::Right => Align::Right,
    };
    let vpos = |block_h: f32| match look.v {
        VAlign::Top => cell.y0 + px,
        VAlign::Center | VAlign::Justify | VAlign::Distributed => {
            ((cell.y0 + cell.y1 - px - block_h) / 2.0).floor()
        }
        VAlign::Bottom => (cell.y1 - px - block_h).max(cell.y0 + px).floor(),
    };
    let clip_cell = Rect::new(cell.x0, cell.y0, cell.x1 - px, cell.y1 - px).intersect(&q.rect);

    // Texto girado ou empilhado: sempre dentro da célula, uma linha.
    if look.rotation != 0 {
        one_line(buf);
        let inner = Rect::new(cell.x0 + pad + indent, cell.y0 + px, cell.x1 - pad, cell.y1 - px);
        let place = |w: f32, h: f32| {
            let x = match align {
                Align::Left => inner.x0,
                Align::Right => inner.x1 - w,
                Align::Center => ((cell.x0 + cell.x1 - w) / 2.0).floor(),
            };
            (x, vpos(h))
        };
        match duck_core::rotation_angle(look.rotation) {
            Some(deg) => {
                let tw = p.text.measure_styled(buf, style);
                let (sin, cos) = deg.to_radians().sin_cos();
                // Caixa da linha girada: cantos de (0,0)-(tw,lh) depois de girar.
                let pts = [
                    (0.0, 0.0),
                    (tw * cos, -tw * sin),
                    (lh * sin, lh * cos),
                    (tw * cos + lh * sin, lh * cos - tw * sin),
                ];
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for (x, y) in pts {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
                let (bx, by) = place(x1 - x0, y1 - y0);
                p.text_rotated(buf, style, (bx - x0).round(), (by - y0).round(), deg, &clip_cell, color);
            }
            None => {
                // Empilhado: uma letra por linha, cada uma centrada na coluna.
                let n = buf.chars().count();
                let wmax = buf
                    .chars()
                    .map(|ch| p.text.measure_styled(ch.encode_utf8(&mut [0; 4]), style))
                    .fold(0.0, f32::max);
                let (bx, by) = place(wmax, n as f32 * lh);
                for (i, ch) in buf.chars().enumerate() {
                    let s = ch.encode_utf8(&mut [0; 4]).to_string();
                    let y = by + i as f32 * lh;
                    if y > clip_cell.y1 {
                        break;
                    }
                    p.text_aligned(&s, style, bx, bx + wmax, y, Align::Center, &clip_cell, color);
                }
            }
        }
        return;
    }

    // Trechos do texto: vários (texto rico) ou um só, com a fonte da célula.
    let base = Seg { range: 0..buf.len(), style, color, underline, strike, rise: 0.0 };
    let rich = match value {
        Value::Text(s) if shown.kind == Kind::Text && s == buf.as_str() => cx.strings.rich_of(s),
        _ => None,
    };
    let segs = match rich {
        Some(runs) => rich_segs(p, cx, look, &base, runs, buf.len()),
        None => vec![base.clone()],
    };

    // Texto rico quebrado: quebra própria (por palavra, entre trechos).
    if look.wrap && rich.is_some() {
        let lines = wrap_segs(p, &segs, buf, (avail - indent).max(1.0));
        let line_h = segs.iter().map(|s| p.text.line_height_of(s.style)).fold(lh, f32::max);
        let y0 = vpos((lines.len() as f32 * line_h).min(cell.h()));
        for (k, line) in lines.iter().enumerate() {
            let y = y0 + k as f32 * line_h;
            if y > clip_cell.y1 {
                break;
            }
            let lw: f32 = line.iter().map(|(_, r)| r.2).sum();
            let x = match align {
                Align::Left => cell.x0 + pad + indent,
                Align::Right => cell.x1 - pad - lw - indent,
                Align::Center => ((cell.x0 + cell.x1 - lw) / 2.0).floor(),
            };
            let mut xx = x;
            for &(si, (a, b, w)) in line {
                draw_seg(p, cx, &segs[si], &buf[a..b], xx, y, line_h, w, &clip_cell);
                xx += w;
            }
        }
        return;
    }

    // Texto quebrado: só dentro da célula.
    if look.wrap && shown.kind == Kind::Text {
        let w = p.text.wrap(buf, style, (avail - indent).max(1.0));
        let y0 = vpos(w.height().min(cell.h()));
        for line in 0..w.lines() {
            let y = y0 + line as f32 * w.line_height();
            if y > clip_cell.y1 {
                break;
            }
            let lw = w.line_width(line);
            let x = match align {
                Align::Left => cell.x0 + pad + indent,
                Align::Right => cell.x1 - pad - lw - indent,
                Align::Center => ((cell.x0 + cell.x1 - lw) / 2.0).floor(),
            };
            let out = &mut *p.out;
            p.text.draw_wrapped_line(&w, line, x, y, clip_cell.as_clip(), |gq| {
                out.push(duck_gfx::Instance::glyph(gq.rect, gq.uv, color))
            });
            decorate(p, cx, &base, x, y, lw, w.line_height(), &clip_cell);
        }
        return;
    }
    one_line(buf);
    let y = vpos(lh);

    // Números: precisam caber (sem transbordo).
    if shown.kind == Kind::Number {
        let mut tw = p.text.measure_styled(buf, style);
        if shown.overflow || tw > avail + 0.5 {
            let mut fitted = false;
            if shown.general
                && !shown.overflow
                && let Value::Number(n) = value
            {
                // "Geral" perde casas decimais para caber, como no Excel.
                let mut chars = buf.chars().count();
                while chars > 1 {
                    chars -= 1;
                    general_fit(n, chars, buf2);
                    if buf2.contains('E') && !buf.contains('E') && chars < 6 {
                        break;
                    }
                    let w2 = p.text.measure_styled(buf2, style);
                    if w2 <= avail + 0.5 {
                        std::mem::swap(buf, buf2);
                        tw = w2;
                        fitted = true;
                        break;
                    }
                }
            }
            if !fitted {
                let hw = p.text.measure_styled("#", style).max(1.0);
                let n = ((avail / hw).floor() as usize).max(1);
                buf.clear();
                buf.extend(std::iter::repeat_n('#', n));
                tw = hw * n as f32;
            }
        }
        if let Some(k) = shown.fill_at.filter(|&k| k <= buf.len() && buf.is_char_boundary(k)) {
            // Contábil: o que vem antes do `*` à esquerda, o resto à direita.
            let (a, b) = buf.split_at(k);
            let wb = p.text.measure_styled(b, style);
            p.text(a, style, cell.x0 + pad, y, &clip_cell, color);
            p.text(b, style, cell.x1 - pad - wb, y, &clip_cell, color);
            return;
        }
        let x = match align {
            Align::Left => cell.x0 + pad + indent,
            Align::Right => cell.x1 - pad - tw - indent,
            Align::Center => ((cell.x0 + cell.x1 - tw) / 2.0).floor(),
        };
        p.text(buf, style, x, y, &clip_cell, color);
        decorate(p, cx, &base, x, y, tw, lh, &clip_cell);
        return;
    }

    let widths: Vec<f32> =
        segs.iter().map(|s| p.text.measure_styled(&buf[s.range.clone()], s.style)).collect();
    let tw: f32 = widths.iter().sum();
    let line_h = segs.iter().map(|s| p.text.line_height_of(s.style)).fold(lh, f32::max);
    let y = if line_h > lh { vpos(line_h) } else { y };
    let mut span = cell;
    if can_overflow && ri != usize::MAX && tw + 2.0 * pad + indent > cell.w() && look.h != HAlign::Fill {
        // Transborda para as vizinhas vazias (e não mescladas) do lado do alinhamento.
        let r = rows[ri].i;
        let v = cx.view;
        let free = |c: u32| !cx.sheet.has_value(r, c) && v.merges.at(r, c).is_none();
        let need = tw + 2.0 * pad + indent;
        let (grow_right, grow_left) = match align {
            Align::Left => (need - cell.w(), 0.0),
            Align::Right => (0.0, need - cell.w()),
            Align::Center => ((need - cell.w()) / 2.0, (need - cell.w()) / 2.0),
        };
        let mut c = col;
        let mut gained = 0.0;
        while gained < grow_right && c + 1 < v.cols.len() && free(c + 1) && c < col + OVERFLOW_LOOK * 2 {
            c += 1;
            gained += v.cols.size(c) as f32 * cx.g;
        }
        span.x1 = cell.x1 + gained;
        let mut c = col;
        let mut gained = 0.0;
        while gained < grow_left && c > q.col_start && free(c - 1) && col - (c - 1) <= OVERFLOW_LOOK * 2 {
            c -= 1;
            gained += v.cols.size(c) as f32 * cx.g;
        }
        span.x0 = cell.x0 - gained;
        // Apaga as linhas de grade verticais por baixo do texto.
        if v.grid && (span.x0 < cell.x0 || span.x1 > cell.x1) {
            let bg = look.fill.unwrap_or(WHITE);
            for s in cols.iter() {
                let line = s.b;
                if line > span.x0 + 0.5
                    && line < span.x1 - 0.5
                    && (line < cell.x0 + 0.5 || line > cell.x1 - 0.5)
                {
                    p.fill(Rect::new(line - px, cell.y0, line, cell.y1 - px), &q.rect, bg);
                }
            }
            if cell.x1 < span.x1 {
                p.fill(Rect::new(cell.x1 - px, cell.y0, cell.x1, cell.y1 - px), &q.rect, bg);
            }
            if span.x0 < cell.x0 {
                p.fill(Rect::new(cell.x0 - px, cell.y0, cell.x0, cell.y1 - px), &q.rect, bg);
            }
        }
    }
    let x = match align {
        Align::Left => cell.x0 + pad + indent,
        Align::Right => cell.x1 - pad - tw - indent,
        Align::Center => ((cell.x0 + cell.x1 - tw) / 2.0).floor(),
    };
    let clip = Rect::new(span.x0, cell.y0, span.x1 - px, cell.y1 - px).intersect(&q.rect);
    let mut xx = x;
    for (seg, w) in segs.iter().zip(widths) {
        draw_seg(p, cx, seg, &buf[seg.range.clone()], xx, y, line_h, w, &clip);
        xx += w;
    }
}

/// Barra de dados: da esquerda até `frac` da largura, com contorno na cor.
fn data_bar(p: &mut Painter, cell: Rect, frac: f32, color: Rgba, px: f32, clip: &Rect) {
    let inset = (2.0 * px).max(1.0);
    let r = Rect::new(
        cell.x0 + px,
        cell.y0 + inset,
        cell.x0 + px + (cell.w() - 2.0 * px) * frac,
        cell.y1 - inset,
    );
    let light = [color[0], color[1], color[2], 150];
    p.fill(r, clip, light);
    p.stroke(r, clip, px, color);
}

/// Ícone de um conjunto (`iconSet`): caractere e cor. `idx` 0 = o mais baixo.
fn icon(set: &str, idx: usize, n: usize) -> (char, Rgba) {
    const RED: Rgba = rgb(0xE0443E);
    const YELLOW: Rgba = rgb(0xF1C232);
    const GREEN: Rgba = rgb(0x3E9B3E);
    const GRAY: Rgba = rgb(0x808080);
    const BLACK: Rgba = rgb(0x202124);
    let ramp = |colors: &[Rgba]| colors[(idx * colors.len() / n.max(1)).min(colors.len() - 1)];
    let traffic = ramp(&[RED, YELLOW, GREEN]);
    let arrows = |gray: bool| {
        let ch = match (n, idx) {
            (_, 0) => '↓',
            (3, 1) | (5, 2) => '→',
            (4, 1) | (5, 1) => '↘',
            (4, 2) | (5, 3) => '↗',
            _ => '↑',
        };
        (ch, if gray { GRAY } else { traffic })
    };
    match set {
        "3Arrows" | "4Arrows" | "5Arrows" => arrows(false),
        "3ArrowsGray" | "4ArrowsGray" | "5ArrowsGray" => arrows(true),
        "3Flags" => ('⚑', traffic),
        "3Symbols" | "3Symbols2" => (['✖', '!', '✔'][idx.min(2)], traffic),
        "3Signs" => (['◆', '▲', '●'][idx.min(2)], traffic),
        "3Stars" => (['☆', '⯪', '★'][idx.min(2)], YELLOW),
        "3Triangles" => (['▼', '▬', '▲'][idx.min(2)], traffic),
        "4RedToBlack" => ('●', [BLACK, GRAY, rgb(0xF4A6A6), RED][idx.min(3)]),
        "4Rating" | "5Rating" | "5Boxes" => {
            (['▁', '▃', '▅', '▇', '█'][(idx * 5 / n.max(1)).min(4)], rgb(0x4472C4))
        }
        "5Quarters" => (['○', '◔', '◑', '◕', '●'][idx.min(4)], BLACK),
        // Semáforos e o padrão.
        _ => ('●', if n >= 4 { ramp(&[BLACK, RED, YELLOW, GREEN]) } else { traffic }),
    }
}

/// Um trecho de texto com fonte própria (texto rico) ou o texto inteiro.
#[derive(Clone)]
struct Seg {
    range: Range<usize>,
    style: TextStyle,
    color: Rgba,
    underline: bool,
    strike: bool,
    /// Deslocamento vertical (sobrescrito < 0, subscrito > 0).
    rise: f32,
}

/// Trechos de um texto rico, com a fonte de cada um resolvida a partir da
/// fonte da célula.
fn rich_segs(p: &mut Painter, cx: &Ctx, look: &Look, base: &Seg, runs: &[RichRun], len: usize) -> Vec<Seg> {
    let cell_font = cx.looks.fonts.get(look.font as usize);
    let mut out = Vec::with_capacity(runs.len());
    for (k, run) in runs.iter().enumerate() {
        let a = run.start as usize;
        let b = runs.get(k + 1).map_or(len, |r| r.start as usize).min(len);
        if a >= b {
            continue;
        }
        let Some(f) = &run.font else {
            out.push(Seg { range: a..b, ..base.clone() });
            continue;
        };
        let name = f.name.as_deref().or(cell_font.map(|c| c.name.as_str())).unwrap_or("Calibri");
        let mut pt = f.size.or(cell_font.map(|c| c.pt)).unwrap_or(11.0);
        if f.script != 0 {
            pt *= 0.75;
        }
        let style = p.text.style(name, pt * 96.0 / 72.0 * cx.g, f.bold, f.italic);
        let lh = p.text.line_height_of(style);
        out.push(Seg {
            range: a..b,
            style,
            color: f.color.and_then(|c| cx.styles.rgb(c)).map_or(base.color, duck_ui::rgb),
            underline: f.underline,
            strike: f.strike,
            rise: match f.script {
                1 => -(lh * 0.35).round(),
                -1 => (lh * 0.2).round(),
                _ => 0.0,
            },
        });
    }
    if out.is_empty() {
        out.push(base.clone());
    }
    out
}

/// Quebra trechos em linhas de até `width` pixels, por palavra (os espaços
/// ficam no fim da palavra) e em `\n`. Cada linha: (trecho, (início, fim, largura)).
#[allow(clippy::type_complexity)]
fn wrap_segs(
    p: &mut Painter,
    segs: &[Seg],
    text: &str,
    width: f32,
) -> Vec<Vec<(usize, (usize, usize, f32))>> {
    let mut lines = vec![Vec::new()];
    let mut x = 0.0f32;
    for (si, seg) in segs.iter().enumerate() {
        let s = &text[seg.range.clone()];
        let mut start = 0;
        let bytes = s.as_bytes();
        let mut i = 0;
        while start < s.len() {
            // Próximo pedaço: até depois dos espaços que seguem a palavra, ou até `\n`.
            while i < s.len() && bytes[i] != b' ' && bytes[i] != b'\n' {
                i += 1;
            }
            let newline = i < s.len() && bytes[i] == b'\n';
            while !newline && i < s.len() && bytes[i] == b' ' {
                i += 1;
            }
            let (a, b) = (seg.range.start + start, seg.range.start + i);
            let w = p.text.measure_styled(&text[a..b], seg.style);
            let line = lines.last_mut().unwrap();
            if x + w.min(width) > width + 0.5 && !line.is_empty() {
                lines.push(Vec::new());
                x = 0.0;
            }
            lines.last_mut().unwrap().push((si, (a, b, w)));
            x += w;
            if newline {
                i += 1;
                lines.push(Vec::new());
                x = 0.0;
            }
            start = i;
        }
    }
    if lines.len() > 1 && lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    lines
}

/// Desenha um trecho com a base alinhada às dos outros (linha de altura `line_h`).
#[allow(clippy::too_many_arguments)]
fn draw_seg(p: &mut Painter, cx: &Ctx, seg: &Seg, s: &str, x: f32, y: f32, line_h: f32, w: f32, clip: &Rect) {
    let lh = p.text.line_height_of(seg.style);
    let yy = y + (line_h - lh) + seg.rise;
    p.text(s, seg.style, x, yy, clip, seg.color);
    decorate(p, cx, seg, x, yy, w, lh, clip);
}

/// Quebras de linha viram espaço (texto de uma linha só).
fn one_line(buf: &mut String) {
    if buf.contains(['\n', '\r']) {
        let s = buf.replace(['\n', '\r'], " ");
        buf.clear();
        buf.push_str(&s);
    }
}

/// Sublinhado e tachado de um trecho.
#[allow(clippy::too_many_arguments)]
fn decorate(p: &mut Painter, cx: &Ctx, seg: &Seg, x: f32, y: f32, w: f32, lh: f32, clip: &Rect) {
    if seg.underline {
        let uy = (y + lh * 0.86).round();
        p.fill(Rect::new(x, uy, x + w, uy + cx.px), clip, seg.color);
    }
    if seg.strike {
        let sy = (y + lh * 0.55).round();
        p.fill(Rect::new(x, sy, x + w, sy + cx.px), clip, seg.color);
    }
}

// ---------------------------------------------------------------------------
// Cabeçalhos e barras de rolagem
// ---------------------------------------------------------------------------

fn draw_headers(p: &mut Painter, cx: &Ctx, geom: &Geom, style: TextStyle, buf: &mut String) {
    let v = cx.view;
    let px = cx.px;
    let (r0, c0, r1, c1) = v.sel();
    let all_rows = r0 == 0 && r1 == v.rows.len() - 1;
    let all_cols = c0 == 0 && c1 == v.cols.len() - 1;
    let lh = p.text.line_height_of(style);

    // Colunas: painel congelado e painel que rola.
    let ch = geom.colhdr;
    p.fill(ch, &ch, HEADER_BG);
    for q in [&geom.quads[0], &geom.quads[1]] {
        let strip = Rect::new(q.rect.x0, ch.y0, q.rect.x1, ch.y1);
        if strip.is_empty() {
            continue;
        }
        for s in spans(&v.cols, q.ox, q.col_start, q.col_end, strip.x0, strip.x1, cx.g) {
            let r = Rect::new(s.a, ch.y0, s.b, ch.y1);
            let selected = s.i >= c0 && s.i <= c1;
            if selected {
                p.fill(r, &strip, if all_rows { HEADER_SEL_FULL } else { HEADER_SEL });
            }
            buf.clear();
            col_name(s.i, buf);
            let color = if selected && all_rows { WHITE } else { HEADER_TEXT };
            let y = (ch.y0 + (ch.h() - lh) / 2.0).floor();
            p.text_aligned(buf, style, s.a, s.b, y, Align::Center, &strip.intersect(&r), color);
            p.fill(Rect::new(s.b - px, ch.y0, s.b, ch.y1), &strip, HEADER_LINE);
        }
    }
    p.fill(Rect::new(ch.x0, ch.y1 - px, ch.x1, ch.y1), &ch, HEADER_LINE);

    // Linhas.
    let rh = geom.rowhdr;
    p.fill(rh, &rh, HEADER_BG);
    for q in [&geom.quads[0], &geom.quads[2]] {
        let strip = Rect::new(rh.x0, q.rect.y0, rh.x1, q.rect.y1);
        if strip.is_empty() {
            continue;
        }
        for s in spans(&v.rows, q.oy, q.row_start, q.row_end, strip.y0, strip.y1, cx.g) {
            let r = Rect::new(rh.x0, s.a, rh.x1, s.b);
            let selected = s.i >= r0 && s.i <= r1;
            if selected {
                p.fill(r, &strip, if all_cols { HEADER_SEL_FULL } else { HEADER_SEL });
            }
            buf.clear();
            let _ = write!(buf, "{}", s.i + 1);
            let color = if selected && all_cols { WHITE } else { HEADER_TEXT };
            let y = (s.a + (s.b - s.a - lh) / 2.0).floor();
            p.text_aligned(buf, style, rh.x0, rh.x1, y, Align::Center, &strip.intersect(&r), color);
            p.fill(Rect::new(rh.x0, s.b - px, rh.x1, s.b), &strip, HEADER_LINE);
        }
    }
    p.fill(Rect::new(rh.x1 - px, rh.y0, rh.x1, rh.y1), &rh, HEADER_LINE);
    // Canto.
    let corner = geom.corner;
    p.fill(corner, &corner, HEADER_BG);
    p.fill(Rect::new(corner.x1 - px, corner.y0, corner.x1, corner.y1), &corner, HEADER_LINE);
    p.fill(Rect::new(corner.x0, corner.y1 - px, corner.x1, corner.y1), &corner, HEADER_LINE);
}

fn draw_scrollbars(p: &mut Painter, vthumb: (f32, f32), g: &Geom, drag: Option<crate::app::Drag>) {
    let vs = g.vscroll;
    p.fill(vs, &vs, TRACK);
    let inset = (3.0 * g.s).round();
    let active = matches!(drag, Some(crate::app::Drag::VThumb { .. }));
    p.fill(
        Rect::new(vs.x0 + inset, vthumb.0, vs.x1 - inset, vthumb.0 + vthumb.1),
        &vs,
        if active { THUMB_ACTIVE } else { THUMB },
    );
}

fn draw_hscroll(p: &mut Painter, hthumb: (f32, f32), g: &Geom, drag: Option<crate::app::Drag>) {
    let hs = g.hscroll;
    p.fill(hs, &hs, TRACK);
    let inset = (3.0 * g.s).round();
    let active = matches!(drag, Some(crate::app::Drag::HThumb { .. }));
    p.fill(
        Rect::new(hthumb.0, hs.y0 + inset, hthumb.0 + hthumb.1, hs.y1 - inset),
        &hs,
        if active { THUMB_ACTIVE } else { THUMB },
    );
}

// ---------------------------------------------------------------------------
// Utilidades do processo
// ---------------------------------------------------------------------------

/// Memória do processo (atual, pico), em bytes.
#[cfg(windows)]
pub fn process_memory() -> (u64, u64) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: chamada simples da API do Windows com um struct zerado do tamanho declarado.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            return (c.WorkingSetSize as u64, c.PeakWorkingSetSize as u64);
        }
    }
    (0, 0)
}

#[cfg(target_os = "macos")]
pub fn process_memory() -> (u64, u64) {
    let current = mac_rusage().map_or(0, |r| r.ri_resident_size);
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `getrusage` só escreve no struct passado. No macOS, `ru_maxrss`
    // vem em bytes (no Linux seria em KB).
    let peak =
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } == 0 { ru.ru_maxrss as u64 } else { 0 };
    (current, peak)
}

#[cfg(target_os = "macos")]
fn mac_rusage() -> Option<libc::rusage_info_v4> {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: com RUSAGE_INFO_V4, `proc_pid_rusage` escreve um `rusage_info_v4`.
    let rc = unsafe {
        libc::proc_pid_rusage(std::process::id() as i32, libc::RUSAGE_INFO_V4, (&raw mut info).cast())
    };
    (rc == 0).then_some(info)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn process_memory() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| {
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    (kb("VmRSS:"), kb("VmHWM:"))
}

/// Memória **privada** em uso (o que o Gerenciador de Tarefas mostra como
/// "Memória"): sem as páginas de DLLs e arquivos mapeados, que são
/// compartilhadas e podem ser descartadas pelo sistema a qualquer momento.
#[cfg(windows)]
pub fn private_memory() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX2};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: como em `process_memory`; a versão EX2 existe desde o Windows 10 1809.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX2 = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut c).cast(), c.cb) != 0 {
            return c.PrivateWorkingSetSize as u64;
        }
    }
    0
}

/// No macOS, a "pegada física" (phys_footprint): é o que o Monitor de
/// Atividade mostra como "Memória".
#[cfg(target_os = "macos")]
pub fn private_memory() -> u64 {
    mac_rusage().map_or(0, |r| r.ri_phys_footprint)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn private_memory() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with("RssAnon:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}

pub fn save_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}
