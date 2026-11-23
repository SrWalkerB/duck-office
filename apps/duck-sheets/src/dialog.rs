//! Caixas de diálogo com vários campos (validação, nota, proteção, avisos),
//! desenhadas pelo próprio app. Enquanto uma está aberta, só ela responde.

use duck_platform::{Clipboard, Key, Modifiers};
use duck_text::{TextStyle, TextSystem};
use duck_ui::{Align, Painter, Rect, TextInput};

use crate::app::{Geom, SheetsApp};
use crate::theme::Theme;

/// Para que serve a caixa aberta (o que os botões fazem).
#[derive(Clone, Debug)]
pub enum Kind {
    /// Só um aviso.
    Message,
    /// Regra de validação da seleção.
    Validation,
    /// Valor recusado (ou com aviso) pela validação.
    Invalid(Box<crate::valid::Pending>),
    /// Texto da nota da célula.
    Note {
        row: u32,
        col: u32,
    },
    ProtectSheet,
    UnprotectSheet,
    ProtectBook,
    UnprotectBook,
}

pub enum Control {
    Text {
        input: TextInput,
        secret: bool,
    },
    /// Texto de várias linhas.
    Area(TextInput),
    Choice {
        options: Vec<String>,
        sel: usize,
    },
    Check(bool),
    /// Texto fixo (quebrado na largura da caixa).
    Info,
    /// Título de uma seção.
    Heading,
}

pub struct Field {
    pub label: String,
    pub control: Control,
    pub show: bool,
}

impl Field {
    fn new(label: &str, control: Control) -> Self {
        Self { label: label.into(), control, show: true }
    }

    pub fn text(label: &str, value: &str) -> Self {
        let mut input = TextInput::new();
        input.set_text(value);
        Self::new(label, Control::Text { input, secret: false })
    }

    pub fn secret(label: &str) -> Self {
        Self::new(label, Control::Text { input: TextInput::new(), secret: true })
    }

    pub fn area(label: &str, value: &str) -> Self {
        let mut input = TextInput::new();
        input.set_text(value);
        Self::new(label, Control::Area(input))
    }

    pub fn choice(label: &str, options: &[&str], sel: usize) -> Self {
        let options = options.iter().map(|s| s.to_string()).collect();
        Self::new(label, Control::Choice { options, sel })
    }

    pub fn check(label: &str, on: bool) -> Self {
        Self::new(label, Control::Check(on))
    }

    pub fn info(text: &str) -> Self {
        Self::new(text, Control::Info)
    }

    pub fn heading(text: &str) -> Self {
        Self::new(text, Control::Heading)
    }

    fn focusable(&self) -> bool {
        self.show && !matches!(self.control, Control::Info | Control::Heading)
    }
}

/// O que a caixa pede depois de um clique ou tecla.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// Um campo mudou (a caixa pode esconder ou mostrar outros).
    Changed,
    /// Um botão (índice em `buttons`).
    Button(usize),
    Cancel,
}

pub struct Dialog {
    pub kind: Kind,
    pub title: String,
    pub fields: Vec<Field>,
    /// Os botões; o primeiro é o do Enter, o último o do Esc.
    pub buttons: Vec<&'static str>,
    pub focus: usize,
    /// O que está errado nos campos.
    pub error: String,
    /// Lista aberta de um campo de escolha.
    open: Option<usize>,
    width: f32,
}

const ROW: f32 = 32.0;
const AREA: f32 = 132.0;
const LABEL_W: f32 = 150.0;
const LINE: f32 = 18.0;

pub struct Laid {
    pub bx: Rect,
    /// (rótulo, controle) de cada campo visível.
    fields: Vec<Option<(Rect, Rect)>>,
    buttons: Vec<Rect>,
    error: Rect,
    /// Opções da lista aberta.
    options: Vec<Rect>,
}

impl Dialog {
    pub fn new(kind: Kind, title: &str, fields: Vec<Field>, buttons: &[&'static str]) -> Self {
        let focus = fields.iter().position(Field::focusable).unwrap_or(usize::MAX);
        let mut d = Self {
            kind,
            title: title.into(),
            fields,
            buttons: buttons.to_vec(),
            focus,
            error: String::new(),
            open: None,
            width: 470.0,
        };
        // Começa com o texto do campo selecionado, como o resto do app.
        if let Some(Field { control: Control::Text { input, .. }, .. }) = d.fields.get_mut(focus) {
            input.select_all();
        }
        d
    }

    pub fn message(title: &str, text: &str) -> Self {
        Self::new(Kind::Message, title, vec![Field::info(text)], &["OK"])
    }

    // --- valores ----------------------------------------------------------

    pub fn text_of(&self, i: usize) -> &str {
        match self.fields.get(i).map(|f| &f.control) {
            Some(Control::Text { input, .. } | Control::Area(input)) => &input.text,
            _ => "",
        }
    }

    pub fn choice_of(&self, i: usize) -> usize {
        match self.fields.get(i).map(|f| &f.control) {
            Some(Control::Choice { sel, .. }) => *sel,
            _ => 0,
        }
    }

    pub fn is_on(&self, i: usize) -> bool {
        matches!(self.fields.get(i).map(|f| &f.control), Some(Control::Check(true)))
    }

    // --- geometria --------------------------------------------------------

    pub fn layout(&self, g: &Geom, text: &mut TextSystem, ui: TextStyle) -> Laid {
        let s = g.s;
        let w = (self.width * s).round().min(g.w - 16.0 * s);
        let pad = 16.0 * s;
        let mut y = 46.0 * s;
        let mut rel = Vec::with_capacity(self.fields.len());
        for f in &self.fields {
            if !f.show {
                rel.push(None);
                continue;
            }
            let (h, label, control) = match &f.control {
                Control::Info => {
                    let n = wrap(&f.label, ui, w - 2.0 * pad, text).len().max(1);
                    let h = (n as f32 * LINE + 8.0) * s;
                    (h, Rect::new(pad, y, w - pad, y + h), Rect::new(pad, y, pad, y))
                }
                Control::Heading => {
                    let h = 28.0 * s;
                    (h, Rect::new(pad, y, w - pad, y + h), Rect::new(pad, y, pad, y))
                }
                Control::Check(_) => {
                    let h = 26.0 * s;
                    let b = 14.0 * s;
                    let m = (h - b) / 2.0;
                    (
                        h,
                        Rect::new(pad + b + 8.0 * s, y, w - pad, y + h),
                        Rect::new(pad, y + m, pad + b, y + m + b),
                    )
                }
                Control::Area(_) if f.label.is_empty() => {
                    let h = AREA * s;
                    (h + 6.0 * s, Rect::new(pad, y, pad, y), Rect::new(pad, y, w - pad, y + h))
                }
                Control::Area(_) => {
                    let h = 58.0 * s;
                    let x = pad + LABEL_W * s;
                    (h + 6.0 * s, Rect::new(pad, y, x, y + 26.0 * s), Rect::new(x, y, w - pad, y + h))
                }
                _ => {
                    let h = ROW * s;
                    let x = pad + LABEL_W * s;
                    (h, Rect::new(pad, y, x, y + 26.0 * s), Rect::new(x, y, w - pad, y + 26.0 * s))
                }
            };
            rel.push(Some((label, control)));
            y += h;
        }
        let err_h = if self.error.is_empty() { 4.0 * s } else { 24.0 * s };
        let error = Rect::new(pad, y, w - pad, y + err_h);
        y += err_h;
        let h = y + 46.0 * s;
        let x0 = ((g.w - w) / 2.0).max(0.0).round();
        let y0 = ((g.status.y1 - h) / 2.0).max(0.0).round();
        let mv = |r: Rect| Rect::new(r.x0 + x0, r.y0 + y0, r.x1 + x0, r.y1 + y0);
        let bx = Rect::new(x0, y0, x0 + w, y0 + h.round());
        // Botões da direita para a esquerda.
        let mut x = bx.x1 - pad;
        let mut buttons = vec![Rect::new(0.0, 0.0, 0.0, 0.0); self.buttons.len()];
        for (i, label) in self.buttons.iter().enumerate().rev() {
            let bw = (text.measure_styled(label, ui) + 28.0 * s).max(90.0 * s).round();
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
            let top = if c.y1 + total > g.status.y1 { (c.y0 - total).max(0.0) } else { c.y1 };
            for k in 0..list.len() {
                options.push(Rect::new(c.x0, top + k as f32 * oh, c.x1, top + (k + 1) as f32 * oh));
            }
        }
        Laid { bx, fields, buttons, error: mv(error), options }
    }

    // --- eventos ----------------------------------------------------------

    pub fn click(&mut self, l: &Laid, x: f32, y: f32) -> Outcome {
        if let Some(i) = self.open.take() {
            if let Some(k) = l.options.iter().position(|r| r.contains(x, y))
                && let Control::Choice { sel, .. } = &mut self.fields[i].control
            {
                *sel = k;
                return Outcome::Changed;
            }
            return Outcome::None;
        }
        if let Some(i) = l.buttons.iter().position(|r| r.contains(x, y)) {
            return Outcome::Button(i);
        }
        for (i, r) in l.fields.iter().enumerate() {
            let Some((label, control)) = r else { continue };
            let hit = control.contains(x, y) || label.contains(x, y);
            if !hit || !self.fields[i].focusable() {
                continue;
            }
            self.focus = i;
            match &mut self.fields[i].control {
                Control::Check(on) => {
                    *on = !*on;
                    return Outcome::Changed;
                }
                Control::Choice { .. } => self.open = Some(i),
                Control::Text { input, .. } => {
                    let end = input.text.len();
                    input.set_cursor(end);
                }
                Control::Area(input) => {
                    let end = input.text.len();
                    input.set_cursor(end);
                }
                _ => {}
            }
            return Outcome::None;
        }
        Outcome::None
    }

    fn move_focus(&mut self, back: bool) {
        let n = self.fields.len();
        if n == 0 || !self.fields.iter().any(Field::focusable) {
            return;
        }
        let mut i = self.focus.min(n - 1);
        loop {
            i = if back { (i + n - 1) % n } else { (i + 1) % n };
            if self.fields[i].focusable() {
                break;
            }
        }
        self.focus = i;
        if let Control::Text { input, .. } = &mut self.fields[i].control {
            input.select_all();
        }
    }

    pub fn key(&mut self, key: &Key, mods: Modifiers, text: Option<&str>, clip: &mut Clipboard) -> Outcome {
        let ctrl = mods.ctrl || mods.logo;
        if let Some(i) = self.open {
            if let Control::Choice { options, sel } = &mut self.fields[i].control {
                match key {
                    Key::ArrowDown => *sel = (*sel + 1).min(options.len().saturating_sub(1)),
                    Key::ArrowUp => *sel = sel.saturating_sub(1),
                    Key::Enter | Key::Escape | Key::Tab | Key::Space => self.open = None,
                    _ => return Outcome::None,
                }
                return Outcome::Changed;
            }
            self.open = None;
        }
        match key {
            Key::Escape => return Outcome::Cancel,
            Key::Tab => {
                self.move_focus(mods.shift);
                return Outcome::None;
            }
            _ => {}
        }
        let enter = matches!(key, Key::Enter);
        let Some(f) = self.fields.get_mut(self.focus).filter(|f| f.show) else {
            return if enter { Outcome::Button(0) } else { Outcome::None };
        };
        match &mut f.control {
            // Ctrl+Enter confirma; Enter quebra a linha.
            Control::Area(input) => {
                match key {
                    Key::Enter if ctrl => return Outcome::Button(0),
                    Key::Enter => insert(input, "\n"),
                    Key::ArrowUp | Key::ArrowDown => line_move(input, matches!(key, Key::ArrowDown)),
                    Key::Char(c) if ctrl && c.eq_ignore_ascii_case("v") => {
                        if let Some(t) = clip.get_text() {
                            insert(input, &t.replace("\r\n", "\n"));
                        }
                    }
                    _ => {
                        input.key(key, mods, text, clip);
                    }
                }
                Outcome::Changed
            }
            _ if enter => Outcome::Button(0),
            Control::Text { input, .. } => {
                if input.key(key, mods, text, clip).changed {
                    Outcome::Changed
                } else {
                    Outcome::None
                }
            }
            Control::Check(on) if matches!(key, Key::Space) => {
                *on = !*on;
                Outcome::Changed
            }
            Control::Choice { options, sel } => {
                match key {
                    Key::ArrowDown if mods.alt => {
                        self.open = Some(self.focus);
                        return Outcome::None;
                    }
                    Key::ArrowDown => *sel = (*sel + 1).min(options.len().saturating_sub(1)),
                    Key::ArrowUp => *sel = sel.saturating_sub(1),
                    Key::Space => {
                        self.open = Some(self.focus);
                        return Outcome::None;
                    }
                    _ => return Outcome::None,
                }
                Outcome::Changed
            }
            _ => Outcome::None,
        }
    }

    // --- desenho ----------------------------------------------------------

    pub fn draw(&self, p: &mut Painter, g: &Geom, ui: TextStyle, mouse: (f32, f32), th: &Theme) {
        let s = g.s;
        let px = s.round().max(1.0);
        let all = Rect::everything();
        let l = self.layout(g, p.text, ui);
        p.fill(Rect::new(0.0, 0.0, g.w, g.status.y1), &all, th.veil);
        p.fill(l.bx.inset(-px), &all, th.line);
        p.fill(l.bx, &all, th.menu);
        let mut bold = ui;
        bold.bold = true;
        let title = Rect::new(l.bx.x0, l.bx.y0 + 8.0 * s, l.bx.x1, l.bx.y0 + 38.0 * s);
        p.label(&self.title, bold.with_px((15.0 * s).round()), title, Align::Left, 16.0 * s, th.text);
        for (i, f) in self.fields.iter().enumerate() {
            let Some((label, control)) = l.fields[i] else { continue };
            let focused = i == self.focus;
            match &f.control {
                Control::Info => {
                    let mut y = label.y0;
                    for (a, b) in wrap(&f.label, ui, label.w(), p.text) {
                        let r = Rect::new(label.x0, y, label.x1, y + LINE * s);
                        p.label(f.label[a..b].trim_end(), ui, r, Align::Left, 0.0, th.text);
                        y += LINE * s;
                    }
                }
                Control::Heading => {
                    p.label(&f.label, bold, label, Align::Left, 0.0, th.muted);
                    let y = (label.y1 - 2.0 * s).round();
                    p.fill(Rect::new(label.x0, y, label.x1, y + px), &all, th.line);
                }
                Control::Text { input, secret } => {
                    p.label(&f.label, ui, label, Align::Left, 0.0, th.text);
                    if *secret {
                        let mut dots = TextInput::new();
                        dots.set_text(&"•".repeat(input.text.chars().count()));
                        dots.draw(p, control, ui, 6.0 * s, focused, &th.input, "");
                    } else {
                        input.draw(p, control, ui, 6.0 * s, focused, &th.input, "");
                    }
                }
                Control::Area(input) => {
                    p.label(&f.label, ui, label, Align::Left, 0.0, th.text);
                    draw_area(p, input, control, ui, s, focused, th);
                }
                Control::Choice { options, sel } => {
                    p.label(&f.label, ui, label, Align::Left, 0.0, th.text);
                    p.fill(control, &all, th.field);
                    p.stroke(control, &all, px, if focused { th.input.focus } else { th.input.border });
                    let text = options.get(*sel).map_or("", String::as_str);
                    p.label(text, ui, control, Align::Left, 6.0 * s, th.text);
                    let (cx, cy) = (control.x1 - 13.0 * s, (control.y0 + control.y1) / 2.0);
                    p.triangle(
                        [cx - 4.0 * s, cy - 2.0 * s],
                        [cx + 4.0 * s, cy - 2.0 * s],
                        [cx, cy + 3.0 * s],
                        &all,
                        th.muted,
                    );
                }
                Control::Check(on) => {
                    p.fill(control, &all, if *on { th.accent } else { th.field });
                    let edge = if *on || focused { th.accent } else { th.muted };
                    p.stroke(control, &all, px, edge);
                    if *on {
                        let at = |x: f32, y: f32| [control.x0 + x * s, control.y0 + y * s];
                        let m = th.on_accent;
                        p.polygon(&[at(2.6, 7.2), at(3.8, 6.0), at(6.0, 8.2), at(6.0, 10.6)], &all, m);
                        p.polygon(&[at(6.0, 8.2), at(10.4, 3.8), at(11.6, 5.0), at(6.0, 10.6)], &all, m);
                    }
                    p.label(&f.label, ui, label, Align::Left, 0.0, th.text);
                }
            }
        }
        if !self.error.is_empty() {
            p.label(&self.error, ui, l.error, Align::Left, 0.0, th.danger);
        }
        for (i, (r, label)) in l.buttons.iter().zip(&self.buttons).enumerate() {
            let primary = i == 0;
            p.fill(*r, &all, if primary { th.accent } else { th.menu });
            if !primary && r.contains(mouse.0, mouse.1) {
                p.fill(*r, &all, th.hover);
            }
            p.stroke(*r, &all, px, if primary { th.accent } else { th.line });
            p.label(label, ui, *r, Align::Center, 0.0, if primary { th.on_accent } else { th.text });
        }
        // Lista aberta por cima de tudo.
        if let (Some(i), Some(first)) = (self.open, l.options.first())
            && let Control::Choice { options, sel } = &self.fields[i].control
        {
            let last = l.options.last().unwrap_or(first);
            let area = Rect::new(first.x0, first.y0, first.x1, last.y1);
            p.fill(area.inset(-px), &all, th.line);
            p.fill(area, &all, th.menu);
            for (k, r) in l.options.iter().enumerate() {
                if r.contains(mouse.0, mouse.1) {
                    p.fill(*r, &all, th.hover);
                } else if k == *sel {
                    p.fill(*r, &all, th.on);
                }
                p.label(&options[k], ui, *r, Align::Left, 8.0 * s, th.text);
            }
        }
    }
}

impl SheetsApp {
    /// Depois de um clique ou tecla na caixa aberta.
    pub fn dialog_event(&mut self, o: Outcome) {
        let Some(mut d) = self.dialog.take() else { return };
        let keep = match o {
            Outcome::None => true,
            Outcome::Changed => {
                if matches!(d.kind, Kind::Validation) {
                    crate::valid::refresh(&mut d);
                }
                true
            }
            Outcome::Cancel => {
                if let Kind::Invalid(p) = d.kind.clone() {
                    self.invalid_answer(*p, None);
                }
                false
            }
            Outcome::Button(i) => {
                let last = d.buttons.len() > 1 && i + 1 == d.buttons.len();
                match d.kind.clone() {
                    Kind::Message => false,
                    Kind::Invalid(p) => {
                        self.invalid_answer(*p, Some(i));
                        false
                    }
                    _ if last => false,
                    Kind::Validation => !self.apply_validation(&mut d, i == 1),
                    Kind::Note { row, col } => {
                        let text = d.text_of(0).to_string();
                        self.set_note(row, col, &text, Some(d.is_on(1)));
                        false
                    }
                    _ => !self.apply_protection(&mut d),
                }
            }
        };
        // A resposta pode ter aberto outra caixa (um aviso): ela fica.
        if keep && self.dialog.is_none() {
            self.dialog = Some(d);
        }
    }
}

fn insert(input: &mut TextInput, s: &str) {
    let at = input.cursor();
    input.replace(at..at, s);
}

/// Sobe ou desce uma linha (as quebras escritas, não as da largura).
fn line_move(input: &mut TextInput, down: bool) {
    let (t, at) = (input.text.clone(), input.cursor());
    let start = t[..at].rfind('\n').map_or(0, |i| i + 1);
    let col = t[start..at].chars().count();
    let target = if down {
        t[at..].find('\n').map(|i| at + i + 1)
    } else if start > 0 {
        Some(t[..start - 1].rfind('\n').map_or(0, |i| i + 1))
    } else {
        None
    };
    let Some(from) = target else {
        input.set_cursor(if down { t.len() } else { 0 });
        return;
    };
    let line = t[from..].split('\n').next().unwrap_or("");
    let off = line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
    input.set_cursor(from + off);
}

fn draw_area(p: &mut Painter, input: &TextInput, r: Rect, ui: TextStyle, s: f32, focused: bool, th: &Theme) {
    let px = s.round().max(1.0);
    let all = Rect::everything();
    p.fill(r, &all, th.input.background);
    p.stroke(r, &all, px, if focused { th.input.focus } else { th.input.border });
    let inner = r.inset(6.0 * s);
    let lines = wrap(&input.text, ui, inner.w(), p.text);
    let at = input.cursor();
    // Linha do cursor: a última que começa antes (ou nele).
    let cur = lines.iter().rposition(|&(a, _)| a <= at).unwrap_or(0);
    let fit = ((inner.h() / (LINE * s)).floor() as usize).max(1);
    let first = (cur + 1).saturating_sub(fit);
    let clip = r.inset(px);
    for (k, &(a, b)) in lines.iter().enumerate().skip(first).take(fit) {
        let y = inner.y0 + (k - first) as f32 * LINE * s;
        let row = Rect::new(inner.x0, y, inner.x1, y + LINE * s).intersect(&clip);
        p.label(input.text[a..b].trim_end_matches('\n'), ui, row, Align::Left, 0.0, th.input.text);
        if focused && k == cur {
            let x = inner.x0 + p.text.measure_styled(&input.text[a..at.clamp(a, b)], ui);
            p.fill(Rect::new(x, y + 2.0 * s, x + px, y + LINE * s - 2.0 * s), &clip, th.input.text);
        }
    }
}

/// Quebra `s` em linhas de até `max` pixels (nas quebras escritas, nos
/// espaços e, se uma palavra não couber, no meio dela): trechos em bytes.
pub fn wrap(s: &str, style: TextStyle, max: f32, text: &mut TextSystem) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut base = 0;
    for para in s.split('\n') {
        let mut start = 0; // início da linha, dentro do parágrafo
        let mut end = 0; // fim do que já coube
        for (i, word) in pieces(para) {
            let stop = i + word.len();
            if text.measure_styled(para[start..stop].trim_end(), style) <= max {
                end = stop;
                continue;
            }
            if end > start {
                out.push((base + start, base + end));
                start = end;
            }
            // Palavra maior que a linha: corta nos caracteres.
            while text.measure_styled(para[start..stop].trim_end(), style) > max {
                let mut cut = start;
                for (k, c) in para[start..stop].char_indices() {
                    let e = start + k + c.len_utf8();
                    if e > cut && text.measure_styled(&para[start..e], style) > max {
                        break;
                    }
                    cut = e;
                }
                if cut <= start || cut >= stop {
                    break;
                }
                out.push((base + start, base + cut));
                start = cut;
            }
            end = stop;
        }
        out.push((base + start, base + para.len()));
        base += para.len() + 1;
    }
    out
}

/// Palavras com os espaços que vêm depois: (início, trecho).
fn pieces(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = false;
    for (i, c) in s.char_indices() {
        if space && !c.is_whitespace() {
            out.push((start, &s[start..i]));
            start = i;
        }
        space = c.is_whitespace();
    }
    if start < s.len() {
        out.push((start, &s[start..]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(pieces("um  dois três"), [(0, "um  "), (4, "dois "), (9, "três")]);
        assert_eq!(pieces(""), []);
    }

    #[test]
    fn lines_up_and_down() {
        let mut i = TextInput::new();
        i.set_text("abc\nçd\nefgh");
        i.set_cursor(2);
        line_move(&mut i, true);
        assert_eq!(&i.text[..i.cursor()], "abc\nçd");
        line_move(&mut i, true);
        assert_eq!(&i.text[..i.cursor()], "abc\nçd\nef");
        line_move(&mut i, false);
        line_move(&mut i, false);
        assert_eq!(&i.text[..i.cursor()], "ab");
        insert(&mut i, "X\n");
        assert_eq!(i.text, "abX\nc\nçd\nefgh");
    }

    #[test]
    fn hidden_fields_do_not_take_focus() {
        let mut d = Dialog::new(
            Kind::Message,
            "t",
            vec![
                Field::info("x"),
                Field::text("a", "1"),
                Field::check("b", false),
                Field::choice("c", &["x"], 0),
            ],
            &["OK", "Cancelar"],
        );
        assert_eq!(d.focus, 1);
        d.fields[2].show = false;
        d.move_focus(false);
        assert_eq!(d.focus, 3);
        d.move_focus(false);
        assert_eq!(d.focus, 1);
        d.move_focus(true);
        assert_eq!(d.focus, 3);
    }
}
