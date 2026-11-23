//! Caixas de diálogo da edição (substituir, colar especial, inserir e excluir
//! células, formatar células): campos em uma ou duas colunas, abas, listas,
//! cores e um quadro de bordas. O que cada caixa faz fica no módulo dela;
//! aqui estão o teclado, o mouse e o desenho.

use duck_platform::{Clipboard, Key, Modifiers};
use duck_text::{TextStyle, TextSystem};
use duck_ui::{Align, Painter, Rect, Rgba, TextInput, rgb};

use crate::app::Geom;
use crate::format::PALETTE;
use crate::theme::Theme;

/// Para que serve a caixa aberta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormId {
    Replace,
    PasteSpecial,
    InsertCells,
    DeleteCells,
    FormatCells,
}

/// Coluna em que o campo fica.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Full,
    Left,
    Right,
}

pub enum Control {
    Text(TextInput),
    /// Lista suspensa.
    Choice {
        options: Vec<String>,
        sel: usize,
    },
    /// Opções à vista, uma por linha.
    Radio {
        options: Vec<String>,
        sel: usize,
    },
    Check(bool),
    /// Caixa com lista rolável (linhas à vista).
    List {
        items: Vec<String>,
        sel: usize,
        scroll: usize,
        rows: usize,
    },
    /// Texto fixo.
    Info,
    Heading,
    /// Amostra: texto numa caixa.
    Sample {
        text: String,
        color: Option<u32>,
        fill: Option<u32>,
        bold: bool,
        italic: bool,
    },
    /// Paleta de cores; a primeira opção é "nenhuma" (rótulo).
    Colors {
        none: String,
        sel: Option<u32>,
    },
    /// Quadro das bordas: lados ligados, por [`Side`].
    Borders {
        on: [Option<bool>; 6],
        multi_rows: bool,
        multi_cols: bool,
    },
}

/// Lados no quadro das bordas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
    InsideV,
    InsideH,
}

pub const SIDES: [Side; 6] = [Side::Left, Side::Right, Side::Top, Side::Bottom, Side::InsideV, Side::InsideH];

pub struct Field {
    pub key: &'static str,
    pub label: String,
    pub control: Control,
    pub page: usize,
    pub col: Col,
    pub show: bool,
}

impl Field {
    pub fn new(key: &'static str, label: &str, control: Control) -> Self {
        Self { key, label: label.into(), control, page: 0, col: Col::Full, show: true }
    }

    pub fn text(key: &'static str, label: &str, value: &str) -> Self {
        let mut input = TextInput::new();
        input.set_text(value);
        Self::new(key, label, Control::Text(input))
    }

    pub fn choice(key: &'static str, label: &str, options: &[&str], sel: usize) -> Self {
        Self::new(
            key,
            label,
            Control::Choice { options: options.iter().map(|s| s.to_string()).collect(), sel },
        )
    }

    pub fn radio(key: &'static str, label: &str, options: &[&str], sel: usize) -> Self {
        Self::new(
            key,
            label,
            Control::Radio { options: options.iter().map(|s| s.to_string()).collect(), sel },
        )
    }

    pub fn check(key: &'static str, label: &str, on: bool) -> Self {
        Self::new(key, label, Control::Check(on))
    }

    pub fn list(key: &'static str, label: &str, items: Vec<String>, sel: usize, rows: usize) -> Self {
        let scroll = (sel + 1).saturating_sub(rows);
        Self::new(key, label, Control::List { items, sel, scroll, rows })
    }

    pub fn info(key: &'static str, text: &str) -> Self {
        Self::new(key, text, Control::Info)
    }

    pub fn heading(text: &str) -> Self {
        Self::new("", text, Control::Heading)
    }

    pub fn colors(key: &'static str, label: &str, none: &str, sel: Option<u32>) -> Self {
        Self::new(key, label, Control::Colors { none: none.into(), sel })
    }

    pub fn page(mut self, page: usize) -> Self {
        self.page = page;
        self
    }

    pub fn left(mut self) -> Self {
        self.col = Col::Left;
        self
    }

    pub fn right(mut self) -> Self {
        self.col = Col::Right;
        self
    }

    fn focusable(&self) -> bool {
        self.show && !matches!(self.control, Control::Info | Control::Heading | Control::Sample { .. })
    }
}

/// O que a caixa pede depois de um clique ou tecla.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// O campo (índice) mudou.
    Changed(usize),
    /// Um lado do quadro de bordas foi clicado.
    Border(usize, Side),
    Button(usize),
    Cancel,
}

pub struct Form {
    pub id: FormId,
    pub title: String,
    pub pages: Vec<&'static str>,
    pub page: usize,
    pub fields: Vec<Field>,
    /// Os botões: o primeiro é o do Enter, o último o do Esc.
    pub buttons: Vec<&'static str>,
    pub focus: usize,
    pub error: String,
    /// Deixa ver (e não escurece) a planilha atrás.
    pub light: bool,
    pub width: f32,
    /// Altura mínima do miolo (a caixa não pula ao trocar de aba).
    pub body: f32,
    open: Option<usize>,
}

const ROW: f32 = 32.0;
const LINE: f32 = 18.0;
const ITEM: f32 = 22.0;
const LABEL: f32 = 130.0;
const SWATCH: f32 = 18.0;

pub struct Laid {
    pub bx: Rect,
    pages: Vec<Rect>,
    /// (rótulo, controle) de cada campo à vista.
    fields: Vec<Option<(Rect, Rect)>>,
    buttons: Vec<Rect>,
    error: Rect,
    options: Vec<Rect>,
}

impl Form {
    pub fn new(id: FormId, title: &str, fields: Vec<Field>, buttons: &[&'static str]) -> Self {
        let mut f = Self {
            id,
            title: title.into(),
            pages: Vec::new(),
            page: 0,
            fields,
            buttons: buttons.to_vec(),
            focus: usize::MAX,
            error: String::new(),
            light: false,
            width: 460.0,
            body: 0.0,
            open: None,
        };
        f.focus_first();
        f
    }

    /// Foca o primeiro campo da página.
    pub fn focus_first(&mut self) {
        self.focus = (0..self.fields.len()).find(|&i| self.usable(i)).unwrap_or(usize::MAX);
        if let Some(Field { control: Control::Text(input), .. }) = self.fields.get_mut(self.focus) {
            input.select_all();
        }
    }

    fn usable(&self, i: usize) -> bool {
        self.fields.get(i).is_some_and(|f| f.focusable() && (self.pages.is_empty() || f.page == self.page))
    }

    fn visible(&self, f: &Field) -> bool {
        f.show && (self.pages.is_empty() || f.page == self.page)
    }

    // --- valores -----------------------------------------------------------

    pub fn index(&self, key: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.key == key)
    }

    pub fn field(&mut self, key: &str) -> Option<&mut Field> {
        self.fields.iter_mut().find(|f| f.key == key)
    }

    pub fn text(&self, key: &str) -> &str {
        match self.index(key).map(|i| &self.fields[i].control) {
            Some(Control::Text(t)) => &t.text,
            _ => "",
        }
    }

    pub fn set_text(&mut self, key: &str, v: &str) {
        if let Some(Field { control: Control::Text(t), .. }) = self.field(key) {
            t.set_text(v);
        }
    }

    pub fn sel(&self, key: &str) -> usize {
        match self.index(key).map(|i| &self.fields[i].control) {
            Some(Control::Choice { sel, .. } | Control::Radio { sel, .. } | Control::List { sel, .. }) => {
                *sel
            }
            _ => 0,
        }
    }

    pub fn set_sel(&mut self, key: &str, v: usize) {
        if let Some(f) = self.field(key) {
            match &mut f.control {
                Control::Choice { sel, .. } | Control::Radio { sel, .. } => *sel = v,
                Control::List { sel, scroll, rows, .. } => {
                    *sel = v;
                    *scroll = (*scroll).min(v).max((v + 1).saturating_sub(*rows));
                }
                _ => {}
            }
        }
    }

    pub fn on(&self, key: &str) -> bool {
        matches!(self.index(key).map(|i| &self.fields[i].control), Some(Control::Check(true)))
    }

    pub fn color(&self, key: &str) -> Option<u32> {
        match self.index(key).map(|i| &self.fields[i].control) {
            Some(Control::Colors { sel, .. }) => *sel,
            _ => None,
        }
    }

    pub fn show(&mut self, key: &str, on: bool) {
        if let Some(f) = self.field(key) {
            f.show = on;
        }
    }

    // --- geometria ---------------------------------------------------------

    fn height(&self, f: &Field, w: f32, ui: TextStyle, s: f32, text: &mut TextSystem) -> f32 {
        let label = if f.label.is_empty() { 0.0 } else { 20.0 };
        match &f.control {
            Control::Info => wrap(&f.label, ui, w, text).len().max(1) as f32 * LINE + 8.0,
            Control::Heading => 28.0,
            Control::Check(_) => 26.0,
            Control::Text(_) | Control::Choice { .. } => {
                if f.col == Col::Full {
                    ROW
                } else {
                    label + ROW
                }
            }
            Control::Radio { options, .. } => label + options.len() as f32 * 24.0 + 6.0,
            Control::List { rows, .. } => label + *rows as f32 * ITEM + 10.0,
            Control::Sample { .. } => label + 44.0,
            Control::Colors { .. } => label + 26.0 + 3.0 * (SWATCH + 4.0) + 8.0,
            Control::Borders { .. } => 190.0,
        }
        .max(0.0)
            * s
    }

    pub fn layout(&self, g: &Geom, text: &mut TextSystem, ui: TextStyle) -> Laid {
        let s = g.s;
        let w = (self.width * s).round().min(g.w - 16.0 * s);
        let pad = 16.0 * s;
        let mut top = 44.0 * s;
        let mut pages = Vec::new();
        if !self.pages.is_empty() {
            let mut x = pad;
            for name in &self.pages {
                let pw = text.measure_styled(name, ui) + 24.0 * s;
                pages.push(Rect::new(x, top, x + pw, top + 30.0 * s));
                x += pw;
            }
            top += 40.0 * s;
        }
        let half = (w - 2.0 * pad - 16.0 * s) / 2.0;
        let (mut yl, mut yr) = (top, top);
        let mut rel = Vec::with_capacity(self.fields.len());
        for f in &self.fields {
            if !self.visible(f) {
                rel.push(None);
                continue;
            }
            let (x0, cw, y) = match f.col {
                Col::Full => {
                    let y = yl.max(yr);
                    (pad, w - 2.0 * pad, y)
                }
                Col::Left => (pad, half, yl),
                Col::Right => (pad + half + 16.0 * s, half, yr),
            };
            let h = self.height(f, cw, ui, s, text);
            let lab = if f.label.is_empty() { 0.0 } else { 20.0 * s };
            let x1 = x0 + cw;
            let (label, control) = match &f.control {
                Control::Info | Control::Heading => (Rect::new(x0, y, x1, y + h), Rect::new(x0, y, x0, y)),
                Control::Check(_) => {
                    let b = 14.0 * s;
                    let m = (h - b) / 2.0;
                    (Rect::new(x0 + b + 8.0 * s, y, x1, y + h), Rect::new(x0, y + m, x0 + b, y + m + b))
                }
                Control::Text(_) | Control::Choice { .. } if f.col == Col::Full => {
                    let x = x0 + if f.label.is_empty() { 0.0 } else { LABEL * s };
                    (Rect::new(x0, y, x, y + 26.0 * s), Rect::new(x, y, x1, y + 26.0 * s))
                }
                Control::Text(_) | Control::Choice { .. } => {
                    (Rect::new(x0, y, x1, y + lab), Rect::new(x0, y + lab, x1, y + lab + 26.0 * s))
                }
                _ => (Rect::new(x0, y, x1, y + lab), Rect::new(x0, y + lab, x1, y + h - 4.0 * s)),
            };
            rel.push(Some((label, control)));
            match f.col {
                Col::Full => {
                    yl = y + h;
                    yr = y + h;
                }
                Col::Left => yl = y + h,
                Col::Right => yr = y + h,
            }
        }
        let mut y = yl.max(yr).max(top + self.body * s);
        let err_h = if self.error.is_empty() { 4.0 * s } else { 24.0 * s };
        let error = Rect::new(pad, y, w - pad, y + err_h);
        y += err_h;
        let h = (y + 50.0 * s).round();
        // As caixas leves ficam embaixo, à direita, para deixar ver a grade.
        let (x0, y0) = if self.light {
            (g.grid.x1 - w - 24.0 * s, g.grid.y1 - h - 24.0 * s)
        } else {
            ((g.w - w) / 2.0, (g.status.y1 - h) / 2.0)
        };
        let (x0, y0) = (x0.max(0.0).round(), y0.max(0.0).round());
        let mv = |r: Rect| Rect::new(r.x0 + x0, r.y0 + y0, r.x1 + x0, r.y1 + y0);
        let bx = Rect::new(x0, y0, x0 + w, y0 + h);
        let mut x = bx.x1 - pad;
        let mut buttons = vec![Rect::default(); self.buttons.len()];
        for (i, label) in self.buttons.iter().enumerate().rev() {
            let bw = (text.measure_styled(label, ui) + 28.0 * s).max(84.0 * s).round();
            buttons[i] = Rect::new(x - bw, bx.y1 - 44.0 * s, x, bx.y1 - 14.0 * s);
            x -= bw + 8.0 * s;
        }
        let fields: Vec<Option<(Rect, Rect)>> =
            rel.into_iter().map(|r| r.map(|(a, b)| (mv(a), mv(b)))).collect();
        let mut options = Vec::new();
        if let Some(i) = self.open
            && let Some(Some((_, c))) = fields.get(i)
            && let Control::Choice { options: list, .. } = &self.fields[i].control
        {
            let oh = 26.0 * s;
            let total = oh * list.len() as f32;
            let y0 = if c.y1 + total > g.status.y1 { (c.y0 - total).max(0.0) } else { c.y1 };
            for k in 0..list.len() {
                options.push(Rect::new(c.x0, y0 + k as f32 * oh, c.x1, y0 + (k + 1) as f32 * oh));
            }
        }
        Laid { bx, pages: pages.into_iter().map(mv).collect(), fields, buttons, error: mv(error), options }
    }

    /// Cor (índice na paleta; `None` = "nenhuma") sob o ponto.
    fn swatch(c: &Rect, s: f32, x: f32, y: f32) -> Option<Option<usize>> {
        if y < c.y0 + 24.0 * s {
            return c.contains(x, y).then_some(None);
        }
        let step = (SWATCH + 4.0) * s;
        let (col, row) = (((x - c.x0) / step).floor(), ((y - c.y0 - 26.0 * s) / step).floor());
        ((0.0..10.0).contains(&col) && (0.0..3.0).contains(&row))
            .then(|| Some(row as usize * 10 + col as usize))
    }

    /// Lado do quadro de bordas sob o ponto.
    fn side(c: &Rect, s: f32, x: f32, y: f32) -> Option<Side> {
        let r = border_box(c, s);
        let near = 14.0 * s;
        if !r.inset(-near).contains(x, y) {
            return None;
        }
        let (cx, cy) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
        let cand = [
            (Side::Left, (x - r.x0).abs()),
            (Side::Right, (x - r.x1).abs()),
            (Side::Top, (y - r.y0).abs()),
            (Side::Bottom, (y - r.y1).abs()),
            (Side::InsideV, (x - cx).abs()),
            (Side::InsideH, (y - cy).abs()),
        ];
        cand.into_iter().filter(|c| c.1 <= near).min_by(|a, b| a.1.total_cmp(&b.1)).map(|c| c.0)
    }

    // --- eventos -----------------------------------------------------------

    pub fn click(&mut self, l: &Laid, s: f32, x: f32, y: f32) -> Outcome {
        if let Some(i) = self.open.take() {
            if let Some(k) = l.options.iter().position(|r| r.contains(x, y))
                && let Control::Choice { sel, .. } = &mut self.fields[i].control
            {
                *sel = k;
                return Outcome::Changed(i);
            }
            return Outcome::None;
        }
        if let Some(i) = l.pages.iter().position(|r| r.contains(x, y)) {
            self.page = i;
            self.focus_first();
            return Outcome::None;
        }
        if let Some(i) = l.buttons.iter().position(|r| r.contains(x, y)) {
            return Outcome::Button(i);
        }
        for (i, r) in l.fields.iter().enumerate() {
            let Some((label, control)) = r else { continue };
            if !(control.contains(x, y) || label.contains(x, y)) || !self.fields[i].focusable() {
                continue;
            }
            self.focus = i;
            match &mut self.fields[i].control {
                Control::Check(on) => {
                    *on = !*on;
                    return Outcome::Changed(i);
                }
                Control::Choice { .. } => self.open = Some(i),
                Control::Text(input) => {
                    let end = input.text.len();
                    input.set_cursor(end);
                }
                Control::Radio { options, sel } => {
                    let k = ((y - control.y0) / (24.0 * s)).floor();
                    if k >= 0.0 && (k as usize) < options.len() {
                        *sel = k as usize;
                        return Outcome::Changed(i);
                    }
                }
                Control::List { items, sel, scroll, .. } => {
                    let k = ((y - control.y0 - 2.0 * s) / (ITEM * s)).floor().max(0.0) as usize + *scroll;
                    if k < items.len() {
                        *sel = k;
                        return Outcome::Changed(i);
                    }
                }
                Control::Colors { sel, .. } => {
                    if let Some(k) = Self::swatch(control, s, x, y) {
                        *sel = k.map(|k| PALETTE[k]);
                        return Outcome::Changed(i);
                    }
                }
                Control::Borders { .. } => {
                    if let Some(side) = Self::side(control, s, x, y) {
                        return Outcome::Border(i, side);
                    }
                }
                _ => {}
            }
            return Outcome::None;
        }
        Outcome::None
    }

    /// Roda do mouse: rola a lista sob o ponto.
    pub fn wheel(&mut self, l: &Laid, x: f32, y: f32, up: bool) {
        for (i, r) in l.fields.iter().enumerate() {
            let Some((_, control)) = r else { continue };
            if control.contains(x, y)
                && let Control::List { items, scroll, rows, .. } = &mut self.fields[i].control
            {
                let max = items.len().saturating_sub(*rows);
                *scroll = if up { scroll.saturating_sub(3) } else { (*scroll + 3).min(max) };
            }
        }
    }

    fn move_focus(&mut self, back: bool) {
        let n = self.fields.len();
        if !(0..n).any(|i| self.usable(i)) {
            return;
        }
        let mut i = self.focus.min(n - 1);
        loop {
            i = if back { (i + n - 1) % n } else { (i + 1) % n };
            if self.usable(i) {
                break;
            }
        }
        self.focus = i;
        if let Control::Text(input) = &mut self.fields[i].control {
            input.select_all();
        }
    }

    pub fn key(&mut self, key: &Key, mods: Modifiers, text: Option<&str>, clip: &mut Clipboard) -> Outcome {
        let ctrl = mods.ctrl || mods.logo;
        if let Some(i) = self.open {
            let Control::Choice { options, sel } = &mut self.fields[i].control else {
                self.open = None;
                return Outcome::None;
            };
            match key {
                Key::ArrowDown => *sel = (*sel + 1).min(options.len().saturating_sub(1)),
                Key::ArrowUp => *sel = sel.saturating_sub(1),
                Key::Enter | Key::Escape | Key::Tab | Key::Space => self.open = None,
                _ => return Outcome::None,
            }
            return Outcome::Changed(i);
        }
        match key {
            Key::Escape => return Outcome::Cancel,
            Key::Enter => return Outcome::Button(0),
            Key::Tab if ctrl && !self.pages.is_empty() => {
                let n = self.pages.len();
                self.page = if mods.shift { (self.page + n - 1) % n } else { (self.page + 1) % n };
                self.focus_first();
                return Outcome::None;
            }
            Key::Tab => {
                self.move_focus(mods.shift);
                return Outcome::None;
            }
            _ => {}
        }
        let i = self.focus;
        if !self.usable(i) {
            return Outcome::None;
        }
        let step = |sel: &mut usize, n: usize, down: bool| {
            *sel = if down { (*sel + 1).min(n.saturating_sub(1)) } else { sel.saturating_sub(1) };
        };
        match &mut self.fields[i].control {
            Control::Text(input) => {
                if input.key(key, mods, text, clip).changed {
                    return Outcome::Changed(i);
                }
            }
            Control::Check(on) if matches!(key, Key::Space) => {
                *on = !*on;
                return Outcome::Changed(i);
            }
            Control::Choice { .. }
                if matches!(key, Key::Space) || mods.alt && matches!(key, Key::ArrowDown) =>
            {
                self.open = Some(i);
            }
            Control::Choice { options, sel } | Control::Radio { options, sel } => {
                let n = options.len();
                match key {
                    Key::ArrowDown => step(sel, n, true),
                    Key::ArrowUp => step(sel, n, false),
                    _ => return Outcome::None,
                }
                return Outcome::Changed(i);
            }
            Control::List { items, sel, scroll, rows } => {
                match key {
                    Key::ArrowDown => step(sel, items.len(), true),
                    Key::ArrowUp => step(sel, items.len(), false),
                    _ => return Outcome::None,
                }
                *scroll = (*scroll).min(*sel).max((*sel + 1).saturating_sub(*rows));
                return Outcome::Changed(i);
            }
            Control::Colors { sel, .. } => {
                let at = sel.and_then(|c| PALETTE.iter().position(|p| *p == c));
                let last = PALETTE.len() - 1;
                let new = match (key, at) {
                    (Key::ArrowRight, None) => Some(0),
                    (Key::ArrowRight, Some(k)) => Some((k + 1).min(last)),
                    (Key::ArrowLeft, Some(0)) => None,
                    (Key::ArrowLeft, Some(k)) => Some(k - 1),
                    (Key::ArrowDown, None) => Some(0),
                    (Key::ArrowDown, Some(k)) => Some((k + 10).min(last)),
                    (Key::ArrowUp, Some(k)) if k >= 10 => Some(k - 10),
                    (Key::ArrowUp, _) => None,
                    _ => return Outcome::None,
                };
                *sel = new.map(|k| PALETTE[k]);
                return Outcome::Changed(i);
            }
            _ => {}
        }
        Outcome::None
    }

    // --- desenho -----------------------------------------------------------

    pub fn draw(&self, p: &mut Painter, g: &Geom, ui: TextStyle, mouse: (f32, f32), th: &Theme) {
        let s = g.s;
        let px = s.round().max(1.0);
        let all = Rect::everything();
        let l = self.layout(g, p.text, ui);
        if !self.light {
            p.fill(Rect::new(0.0, 0.0, g.w, g.status.y1), &all, th.veil);
        }
        let r = l.bx;
        p.fill(Rect::new(r.x0 + 2.0 * s, r.y0 + 2.0 * s, r.x1 + 3.0 * s, r.y1 + 3.0 * s), &all, th.shadow);
        p.fill(r.inset(-px), &all, th.line);
        p.fill(r, &all, th.menu);
        let mut bold = ui;
        bold.bold = true;
        let title = Rect::new(r.x0, r.y0 + 8.0 * s, r.x1, r.y0 + 36.0 * s);
        p.label(&self.title, bold.with_px((15.0 * s).round()), title, Align::Left, 16.0 * s, th.text);
        if let (Some(first), Some(_)) = (l.pages.first(), l.pages.last()) {
            let y = first.y1 - px;
            p.fill(Rect::new(r.x0 + 16.0 * s, y, r.x1 - 16.0 * s, y + px), &all, th.line);
            for (k, (pr, name)) in l.pages.iter().zip(&self.pages).enumerate() {
                let on = k == self.page;
                if !on && pr.contains(mouse.0, mouse.1) {
                    p.fill(*pr, &all, th.hover);
                }
                p.label(name, ui, *pr, Align::Center, 0.0, if on { th.accent } else { th.text });
                if on {
                    p.fill(Rect::new(pr.x0, pr.y1 - 3.0 * px, pr.x1, pr.y1), &all, th.accent);
                }
            }
        }
        for (i, f) in self.fields.iter().enumerate() {
            let Some((label, control)) = l.fields[i] else { continue };
            let focused = i == self.focus;
            let edge = if focused { th.input.focus } else { th.input.border };
            match &f.control {
                Control::Info => {
                    let mut y = label.y0;
                    for (a, b) in wrap(&f.label, ui, label.w(), p.text) {
                        let row = Rect::new(label.x0, y, label.x1, y + LINE * s);
                        p.label(f.label[a..b].trim_end(), ui, row, Align::Left, 0.0, th.muted);
                        y += LINE * s;
                    }
                    continue;
                }
                Control::Heading => {
                    p.label(&f.label, bold, label, Align::Left, 0.0, th.muted);
                    let y = (label.y1 - 2.0 * s).round();
                    p.fill(Rect::new(label.x0, y, label.x1, y + px), &all, th.line);
                    continue;
                }
                _ => {}
            }
            p.label(&f.label, ui, label, Align::Left, 0.0, th.text);
            match &f.control {
                Control::Text(input) => input.draw(p, control, ui, 6.0 * s, focused, &th.input, ""),
                Control::Choice { options, sel } => {
                    p.fill(control, &all, th.field);
                    p.stroke(control, &all, px, edge);
                    let t = options.get(*sel).map_or("", String::as_str);
                    p.label(t, ui, control, Align::Left, 6.0 * s, th.text);
                    let (cx, cy) = (control.x1 - 13.0 * s, (control.y0 + control.y1) / 2.0);
                    p.triangle(
                        [cx - 4.0 * s, cy - 2.0 * s],
                        [cx + 4.0 * s, cy - 2.0 * s],
                        [cx, cy + 3.0 * s],
                        &all,
                        th.muted,
                    );
                }
                Control::Radio { options, sel } => {
                    for (k, o) in options.iter().enumerate() {
                        let y = control.y0 + k as f32 * 24.0 * s;
                        let c = [control.x0 + 8.0 * s, y + 12.0 * s];
                        let ring = if k == *sel { th.accent } else { th.muted };
                        p.disc(c, 7.0 * s, &all, ring);
                        p.disc(c, 5.6 * s, &all, th.menu);
                        if k == *sel {
                            p.disc(c, 3.8 * s, &all, th.accent);
                        }
                        let row = Rect::new(control.x0 + 22.0 * s, y, control.x1, y + 24.0 * s);
                        p.label(o, ui, row, Align::Left, 0.0, th.text);
                        if focused && k == *sel {
                            let w = p.text.measure_styled(o, ui);
                            let u = Rect::new(row.x0, row.y1 - 4.0 * s, row.x0 + w, row.y1 - 4.0 * s + px);
                            p.fill(u, &all, th.accent);
                        }
                    }
                }
                Control::Check(on) => {
                    p.fill(control, &all, if *on { th.accent } else { th.field });
                    p.stroke(control, &all, px, if *on || focused { th.accent } else { th.muted });
                    if *on {
                        let at = |x: f32, y: f32| [control.x0 + x * s, control.y0 + y * s];
                        let m = th.on_accent;
                        p.polygon(&[at(2.6, 7.2), at(3.8, 6.0), at(6.0, 8.2), at(6.0, 10.6)], &all, m);
                        p.polygon(&[at(6.0, 8.2), at(10.4, 3.8), at(11.6, 5.0), at(6.0, 10.6)], &all, m);
                    }
                }
                Control::List { items, sel, scroll, rows } => {
                    p.fill(control, &all, th.field);
                    p.stroke(control, &all, px, edge);
                    let clip = control.inset(px);
                    for (k, t) in items.iter().enumerate().skip(*scroll).take(*rows) {
                        let y = control.y0 + 2.0 * s + (k - scroll) as f32 * ITEM * s;
                        let row = Rect::new(control.x0, y, control.x1, y + ITEM * s).intersect(&clip);
                        if k == *sel {
                            p.fill(row, &all, th.on);
                        } else if row.contains(mouse.0, mouse.1) {
                            p.fill(row, &all, th.hover);
                        }
                        p.label(t, ui, row, Align::Left, 8.0 * s, th.text);
                    }
                    if items.len() > *rows {
                        let h = clip.h();
                        let th_h = (h * *rows as f32 / items.len() as f32).max(12.0 * s);
                        let max = (items.len() - rows) as f32;
                        let y = clip.y0 + (h - th_h) * (*scroll as f32 / max);
                        p.fill(Rect::new(clip.x1 - 6.0 * s, y, clip.x1 - 2.0 * s, y + th_h), &all, th.thumb);
                    }
                }
                Control::Sample { text, color, fill, bold, italic } => {
                    let bg = fill.map_or(th.paper, rgb);
                    p.fill(control, &all, bg);
                    p.stroke(control, &all, px, th.line);
                    let mut st = ui;
                    st.bold = *bold;
                    st.italic = *italic;
                    let ink = color.map_or(if fill.is_some() { [0, 0, 0, 255] } else { th.text }, rgb);
                    p.label(text, st, control, Align::Left, 8.0 * s, ink);
                }
                Control::Colors { none, sel } => {
                    let top = Rect::new(control.x0, control.y0, control.x1, control.y0 + 22.0 * s);
                    if sel.is_none() {
                        p.fill(top, &all, th.on);
                    } else if top.contains(mouse.0, mouse.1) {
                        p.fill(top, &all, th.hover);
                    }
                    p.label(none, ui, top, Align::Left, 6.0 * s, th.text);
                    let custom = sel.filter(|c| !PALETTE.contains(c));
                    if let Some(c) = custom {
                        let m = Rect::new(
                            top.x1 - 60.0 * s,
                            top.y0 + 3.0 * s,
                            top.x1 - 6.0 * s,
                            top.y1 - 3.0 * s,
                        );
                        p.fill(m.inset(-px), &all, th.accent);
                        p.fill(m, &all, rgb(c));
                    }
                    for (k, &c) in PALETTE.iter().enumerate() {
                        let step = (SWATCH + 4.0) * s;
                        let x = (control.x0 + (k % 10) as f32 * step).round();
                        let y = (control.y0 + 26.0 * s + (k / 10) as f32 * step).round();
                        let sw = Rect::new(x, y, x + (SWATCH * s).round(), y + (SWATCH * s).round());
                        let hot = *sel == Some(c);
                        p.fill(
                            sw.inset(if hot { -2.0 * px } else { -px }),
                            &all,
                            if hot { th.accent } else { th.line },
                        );
                        p.fill(sw, &all, rgb(c));
                    }
                    if focused {
                        p.stroke(control.inset(-3.0 * s), &all, px, th.input.focus);
                    }
                }
                Control::Borders { on, multi_rows, multi_cols } => {
                    let b = border_box(&control, s);
                    p.fill(b, &all, th.paper);
                    let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
                    let mut lines: Vec<(Side, [f32; 2], [f32; 2])> = vec![
                        (Side::Left, [b.x0, b.y0], [b.x0, b.y1]),
                        (Side::Right, [b.x1, b.y0], [b.x1, b.y1]),
                        (Side::Top, [b.x0, b.y0], [b.x1, b.y0]),
                        (Side::Bottom, [b.x0, b.y1], [b.x1, b.y1]),
                    ];
                    if *multi_cols {
                        lines.push((Side::InsideV, [cx, b.y0], [cx, b.y1]));
                    }
                    if *multi_rows {
                        lines.push((Side::InsideH, [b.x0, cy], [b.x1, cy]));
                    }
                    for (side, a, z) in lines {
                        let k = SIDES.iter().position(|x| *x == side).unwrap_or(0);
                        let (w, color): (f32, Rgba) = match on[k] {
                            Some(true) => (2.0 * px, th.text),
                            Some(false) => (px, th.line),
                            // Misturado na seleção.
                            None => (2.0 * px, th.muted),
                        };
                        p.line(a, z, w, &all, color);
                    }
                    for (x, y) in [
                        (b.x0 + b.w() * 0.25, b.y0 + b.h() * 0.25),
                        (b.x0 + b.w() * 0.75, b.y0 + b.h() * 0.75),
                    ] {
                        let t = Rect::new(x - 30.0 * s, y - 9.0 * s, x + 30.0 * s, y + 9.0 * s);
                        if *multi_rows || *multi_cols || x < cx {
                            p.label("Texto", ui, t, Align::Center, 0.0, th.muted);
                        }
                    }
                }
                Control::Info | Control::Heading => {}
            }
        }
        if !self.error.is_empty() {
            p.label(
                &self.error,
                ui,
                l.error,
                Align::Left,
                0.0,
                if th.dark { rgb(0xF28B82) } else { rgb(0xC5221F) },
            );
        }
        for (i, (b, label)) in l.buttons.iter().zip(&self.buttons).enumerate() {
            let primary = i == 0;
            p.fill(*b, &all, if primary { th.accent } else { th.menu });
            if !primary && b.contains(mouse.0, mouse.1) {
                p.fill(*b, &all, th.hover);
            }
            p.stroke(*b, &all, px, if primary { th.accent } else { th.line });
            p.label(label, ui, *b, Align::Center, 0.0, if primary { th.on_accent } else { th.text });
        }
        if let (Some(i), Some(first), Some(last)) = (self.open, l.options.first(), l.options.last())
            && let Control::Choice { options, sel } = &self.fields[i].control
        {
            let area = Rect::new(first.x0, first.y0, first.x1, last.y1);
            p.fill(area.inset(-px), &all, th.line);
            p.fill(area, &all, th.menu);
            for (k, o) in l.options.iter().enumerate() {
                if o.contains(mouse.0, mouse.1) {
                    p.fill(*o, &all, th.hover);
                } else if k == *sel {
                    p.fill(*o, &all, th.on);
                }
                p.label(&options[k], ui, *o, Align::Left, 8.0 * s, th.text);
            }
        }
    }
}

impl crate::app::SheetsApp {
    /// Depois de um clique ou tecla na caixa aberta.
    pub fn form_event(&mut self, o: Outcome) {
        let Some(mut f) = self.form.take() else { return };
        let keep = match o {
            Outcome::None => true,
            Outcome::Cancel => false,
            Outcome::Changed(i) => {
                f.error.clear();
                if f.id == FormId::FormatCells {
                    self.format_form_changed(&mut f, i);
                }
                true
            }
            Outcome::Border(i, side) => {
                self.format_form_border(&mut f, i, side);
                true
            }
            // O último botão fecha.
            Outcome::Button(b) if b + 1 == f.buttons.len() && b > 0 => false,
            Outcome::Button(b) => match f.id {
                FormId::Replace => {
                    f.error = self.replace_form(&f, b);
                    true
                }
                FormId::PasteSpecial => {
                    self.paste_form(&f);
                    false
                }
                FormId::InsertCells | FormId::DeleteCells => {
                    self.shift_cells(f.id == FormId::InsertCells, f.sel("to"));
                    false
                }
                FormId::FormatCells => match self.format_form_apply(&f) {
                    Ok(()) => false,
                    Err(e) => {
                        f.error = e;
                        true
                    }
                },
            },
        };
        if keep && self.form.is_none() {
            self.form = Some(f);
        }
    }
}

/// O quadro das bordas dentro do controle.
fn border_box(c: &Rect, s: f32) -> Rect {
    let (w, h) = (200.0 * s, 120.0 * s);
    let x0 = ((c.x0 + c.x1 - w) / 2.0).round();
    let y0 = (c.y0 + 18.0 * s).round();
    Rect::new(x0, y0, x0 + w.round(), y0 + h.round())
}

/// Quebra `s` em linhas de até `max` pixels, nos espaços: trechos em bytes.
pub fn wrap(s: &str, style: TextStyle, max: f32, text: &mut TextSystem) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut base = 0;
    for para in s.split('\n') {
        let mut start = 0;
        let mut end = 0;
        let mut k = 0;
        while k < para.len() {
            let stop = para[k..].find(' ').map_or(para.len(), |i| k + i + 1);
            if text.measure_styled(para[start..stop].trim_end(), style) > max && end > start {
                out.push((base + start, base + end));
                start = end;
            }
            end = stop;
            k = stop;
        }
        out.push((base + start, base + para.len()));
        base += para.len() + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Form {
        let mut f = Form::new(
            FormId::PasteSpecial,
            "t",
            vec![
                Field::info("", "x"),
                Field::text("a", "A", "1"),
                Field::radio("b", "B", &["x", "y", "z"], 0),
                Field::check("c", "C", false).page(1),
            ],
            &["OK", "Cancelar"],
        );
        f.pages = vec!["Um", "Dois"];
        f
    }

    fn press(f: &mut Form, k: Key, ctrl: bool) -> Outcome {
        let m = Modifiers { ctrl, ..Default::default() };
        f.key(&k, m, None, &mut Clipboard::default())
    }

    #[test]
    fn focus_pages_and_values() {
        let mut f = form();
        assert_eq!(f.focus, 1);
        press(&mut f, Key::Tab, false);
        assert_eq!(f.focus, 2);
        assert_eq!(press(&mut f, Key::ArrowDown, false), Outcome::Changed(2));
        assert_eq!(f.sel("b"), 1);
        press(&mut f, Key::ArrowUp, false);
        press(&mut f, Key::ArrowUp, false);
        assert_eq!(f.sel("b"), 0);
        // O campo da outra página só recebe o foco nela.
        press(&mut f, Key::Tab, false);
        assert_eq!(f.focus, 1);
        press(&mut f, Key::Tab, true);
        assert_eq!((f.page, f.focus), (1, 3));
        press(&mut f, Key::Space, false);
        assert!(f.on("c"));
        assert_eq!(press(&mut f, Key::Enter, false), Outcome::Button(0));
        assert_eq!(press(&mut f, Key::Escape, false), Outcome::Cancel);
        f.set_text("a", "2");
        assert_eq!(f.text("a"), "2");
        assert_eq!(f.text("nada"), "");
    }

    #[test]
    fn list_scrolls_with_the_selection() {
        let items = (0..20).map(|i| i.to_string()).collect();
        let mut f = Form::new(FormId::FormatCells, "t", vec![Field::list("l", "", items, 0, 5)], &["OK"]);
        for _ in 0..7 {
            press(&mut f, Key::ArrowDown, false);
        }
        assert_eq!(f.sel("l"), 7);
        let Control::List { scroll, .. } = f.fields[0].control else { panic!() };
        assert_eq!(scroll, 3);
        f.set_sel("l", 0);
        let Control::List { scroll, .. } = f.fields[0].control else { panic!() };
        assert_eq!(scroll, 0);
    }
}
