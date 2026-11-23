//! Validação de dados na tela: a seta com a lista suspensa, a mensagem de
//! entrada, a recusa (ou o aviso) de valores inválidos e a caixa que cria,
//! edita e tira regras. Quem avalia é o `duck-calc` (`valid`).

use duck_calc::valid::ListItem;
use duck_core::{
    CellChange, CellRange, CellValue, Command, DataValidation, DvAlert, DvKind, DvOp, NumFmt, Sheet, Value,
};
use duck_platform::{Key, Modifiers};
use duck_text::TextStyle;
use duck_ui::{Align, Painter, Rect};

use crate::app::{Geom, MAX_COLS, MAX_ROWS, SheetsApp};
use crate::dialog::{Dialog, Field, Kind, wrap};
use crate::theme::Theme;

/// O que foi digitado e a validação não aceitou (à espera da resposta).
#[derive(Clone, Debug)]
pub struct Pending {
    pub sheet: usize,
    /// Célula em que se digitou.
    pub cell: (u32, u32),
    /// A mudança, se a resposta for continuar.
    pub cmd: Command,
    /// Como foi digitado.
    pub text: String,
    pub alert: DvAlert,
    /// Para todas as células selecionadas (Ctrl+Enter).
    pub all: bool,
}

/// Lista suspensa aberta.
pub struct ListBox {
    pub sheet: usize,
    pub row: u32,
    pub col: u32,
    /// (texto mostrado, item).
    items: Vec<(String, ListItem)>,
    sel: Option<usize>,
    scroll: usize,
    pub rect: Rect,
}

const ROWS: usize = 9;
const ROW_H: f32 = 23.0;

impl ListBox {
    fn fit(&self, s: f32) -> usize {
        ((self.rect.h() / (ROW_H * s)).round() as usize).max(1)
    }

    pub fn scroll_by(&mut self, d: i32, s: f32) {
        let max = self.items.len().saturating_sub(self.fit(s)) as i32;
        self.scroll = (self.scroll as i32 + d).clamp(0, max.max(0)) as usize;
    }

    fn select(&mut self, k: usize, s: f32) {
        let k = k.min(self.items.len().saturating_sub(1));
        self.sel = Some(k);
        let fit = self.fit(s);
        if k < self.scroll {
            self.scroll = k;
        } else if k >= self.scroll + fit {
            self.scroll = k + 1 - fit;
        }
    }

    fn item_at(&self, s: f32, y: f32) -> Option<usize> {
        let k = self.scroll + ((y - self.rect.y0) / (ROW_H * s)).floor().max(0.0) as usize;
        (k < self.items.len()).then_some(k)
    }

    pub fn draw(&self, p: &mut Painter, g: &Geom, ui: TextStyle, mouse: (f32, f32), th: &Theme) {
        let s = g.s;
        let px = s.round().max(1.0);
        let all = Rect::everything();
        let r = self.rect;
        p.fill(Rect::new(r.x0 + 2.0 * s, r.y0 + 2.0 * s, r.x1 + 3.0 * s, r.y1 + 3.0 * s), &all, th.shadow);
        p.fill(r.inset(-px), &all, th.line);
        p.fill(r, &all, th.menu);
        let fit = self.fit(s);
        let more = self.items.len() > fit;
        let right = if more { r.x1 - 10.0 * s } else { r.x1 };
        for (k, (text, _)) in self.items.iter().enumerate().skip(self.scroll).take(fit) {
            let y = r.y0 + (k - self.scroll) as f32 * ROW_H * s;
            let row = Rect::new(r.x0, y, right, y + ROW_H * s);
            if Some(k) == self.sel {
                p.fill(row, &all, th.on);
            } else if row.contains(mouse.0, mouse.1) {
                p.fill(row, &all, th.hover);
            }
            p.label(text, ui, row, Align::Left, 8.0 * s, th.text);
        }
        if more {
            let track = Rect::new(r.x1 - 8.0 * s, r.y0, r.x1, r.y1);
            p.fill(track, &all, th.track);
            let n = self.items.len() as f32;
            let len = (r.h() * fit as f32 / n).max(16.0 * s);
            let top = r.y0 + (r.h() - len) * self.scroll as f32 / (n - fit as f32).max(1.0);
            p.fill(Rect::new(track.x0, top, track.x1, top + len), &all, th.thumb);
        }
    }
}

/// O item como aparece na lista: formatado como a célula de origem.
fn shown(doc: &crate::app::Doc, item: &ListItem) -> String {
    let mut out = String::new();
    let v = match &item.value {
        CellValue::Empty => return out,
        CellValue::Number(x) => Value::Number(*x),
        CellValue::Text(s) => Value::Text(s),
        CellValue::Bool(b) => Value::Bool(*b),
        CellValue::Error(e) => Value::Error(*e),
    };
    let look = doc.looks.get(item.style.unwrap_or(0));
    crate::look::show(&doc.looks, look, v, doc.date1904, &mut out);
    out
}

/// A seta da lista na célula ativa.
pub fn draw_arrow(p: &mut Painter, r: Rect, g: &Geom, th: &Theme) {
    let px = g.s.round().max(1.0);
    let clip = g.grid;
    p.fill(r, &clip, th.field);
    p.stroke(r, &clip, px, th.muted);
    let (cx, cy, k) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0, r.w() / 4.0);
    p.triangle([cx - k, cy - k * 0.5], [cx + k, cy - k * 0.5], [cx, cy + k * 0.7], &clip, th.text);
}

/// Caixa com título e texto (mensagem de entrada), perto de `cell`.
pub fn draw_tip(
    p: &mut Painter,
    g: &Geom,
    ui: TextStyle,
    cell: Rect,
    title: &str,
    text: &str,
    th: &Theme,
) -> Rect {
    let s = g.s;
    let px = s.round().max(1.0);
    let all = Rect::everything();
    let small = ui.with_px((12.0 * s).round());
    let mut bold = small;
    bold.bold = true;
    let max = 240.0 * s;
    let line = 17.0 * s;
    let heads = if title.is_empty() { Vec::new() } else { wrap(title, bold, max, p.text) };
    let lines = if text.is_empty() { Vec::new() } else { wrap(text, small, max, p.text) };
    let mut w = 60.0 * s;
    for &(a, b) in &heads {
        w = f32::max(w, p.text.measure_styled(title[a..b].trim_end(), bold));
    }
    for &(a, b) in &lines {
        w = f32::max(w, p.text.measure_styled(text[a..b].trim_end(), small));
    }
    let (w, h) = ((w + 14.0 * s).round(), ((heads.len() + lines.len()) as f32 * line + 10.0 * s).round());
    let x0 = (cell.x0 + 10.0 * s).min(g.grid.x1 - w - 4.0 * s).max(0.0).round();
    let below = cell.y1 + 6.0 * s;
    let y0 = if below + h > g.grid.y1 { cell.y0 - h - 6.0 * s } else { below }.max(0.0).round();
    let r = Rect::new(x0, y0, x0 + w, y0 + h);
    p.fill(Rect::new(r.x0 + 2.0 * s, r.y0 + 2.0 * s, r.x1 + 2.0 * s, r.y1 + 2.0 * s), &all, th.shadow);
    p.fill(r.inset(-px), &all, th.note_line);
    p.fill(r, &all, th.note_bg);
    let mut y = r.y0 + 5.0 * s;
    for (list, src, style) in [(&heads, title, bold), (&lines, text, small)] {
        for &(a, b) in list {
            let row = Rect::new(r.x0, y, r.x1, y + line);
            p.label(src[a..b].trim_end(), style, row, Align::Left, 7.0 * s, th.note_text);
            y += line;
        }
    }
    r
}

const KINDS: [(DvKind, &str); 8] = [
    (DvKind::Any, "Qualquer valor"),
    (DvKind::Whole, "Número inteiro"),
    (DvKind::Decimal, "Decimal"),
    (DvKind::List, "Lista"),
    (DvKind::Date, "Data"),
    (DvKind::Time, "Hora"),
    (DvKind::TextLength, "Comprimento do texto"),
    (DvKind::Custom, "Personalizado"),
];

const OPS: [(DvOp, &str); 8] = [
    (DvOp::Between, "está entre"),
    (DvOp::NotBetween, "não está entre"),
    (DvOp::Equal, "é igual a"),
    (DvOp::NotEqual, "é diferente de"),
    (DvOp::Greater, "é maior do que"),
    (DvOp::Less, "é menor do que"),
    (DvOp::GreaterEqual, "é maior ou igual a"),
    (DvOp::LessEqual, "é menor ou igual a"),
];

const ALERTS: [(DvAlert, &str); 3] = [
    (DvAlert::Stop, "Parar (recusa o valor)"),
    (DvAlert::Warning, "Aviso (pergunta)"),
    (DvAlert::Information, "Informação"),
];

/// Posições dos campos da caixa.
mod f {
    pub const KIND: usize = 0;
    pub const OP: usize = 1;
    pub const F1: usize = 2;
    pub const F2: usize = 3;
    pub const BLANK: usize = 4;
    pub const ARROW: usize = 5;
    pub const SHOW_INPUT: usize = 7;
    pub const PROMPT_TITLE: usize = 8;
    pub const PROMPT: usize = 9;
    pub const SHOW_ERROR: usize = 11;
    pub const ALERT: usize = 12;
    pub const ERROR_TITLE: usize = 13;
    pub const ERROR: usize = 14;
    pub const ALL: usize = 15;
}

/// Um limite ou origem como o usuário vê: `99,5`, `31/12/2026`, `=SOMA(A1)`.
fn show_operand(v: &DataValidation, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    if v.kind == DvKind::List
        && let Some(items) = v.literal_items()
    {
        return items.join(";");
    }
    if let Ok(x) = text.parse::<f64>() {
        let mut out = String::new();
        let code = match v.kind {
            DvKind::Date if x.fract() == 0.0 => "dd/mm/yyyy",
            DvKind::Date => "dd/mm/yyyy hh:mm",
            DvKind::Time => "hh:mm:ss",
            _ => "",
        };
        if code.is_empty() {
            duck_core::numfmt::format_general(x, 15, &mut out);
        } else {
            NumFmt::parse(code).format_number(x, false, &mut out);
        }
        return out;
    }
    format!("={}", duck_calc::locale::to_pt(text))
}

/// Inverso de [`show_operand`], para limites. `Err` = o que está errado.
fn read_operand(kind: DvKind, text: &str, date1904: bool) -> Result<String, String> {
    use duck_core::input::{Input, parse_input, today};
    let t = text.trim();
    if t.is_empty() {
        return Err("Preencha o limite (um valor ou uma fórmula com =).".into());
    }
    if kind == DvKind::Custom {
        return Ok(duck_calc::locale::from_pt(t.trim_start_matches('=')));
    }
    match parse_input(t, date1904, today()) {
        Input::Formula(f) => Ok(duck_calc::locale::from_pt(&f)),
        Input::Value(CellValue::Number(x), _) => Ok(format!("{x}")),
        _ => Err(format!("\"{t}\" não é um valor válido: use um número, data, hora ou fórmula com =.")),
    }
}

fn relocate(f: &str, dr: i64, dc: i64) -> String {
    duck_calc::shift(f, dr, dc)
}

/// Desloca as fórmulas da regra de uma célula base para outra.
fn rebase(v: &mut DataValidation, from: (u32, u32), to: (u32, u32)) {
    let (dr, dc) = (to.0 as i64 - from.0 as i64, to.1 as i64 - from.1 as i64);
    if (dr, dc) == (0, 0) {
        return;
    }
    for f in [&mut v.f1, &mut v.f2] {
        if !f.is_empty() {
            *f = relocate(f, dr, dc);
        }
    }
}

/// As regras da aba com `rule` valendo em `range` (`None` = sem regra lá).
pub fn with_rule(
    sheet: &Sheet,
    range: CellRange,
    rule: Option<DataValidation>,
    cur: (u32, u32),
) -> Vec<DataValidation> {
    let mut list = sheet.layout.validations.clone();
    duck_core::valid::clear(&mut list, range, &relocate);
    if let Some(mut v) = rule {
        v.ranges = vec![range];
        let base = v.base();
        rebase(&mut v, cur, base);
        // A mesma regra em intervalos vizinhos fica numa só.
        if let Some(old) = list.iter_mut().find(|o| {
            let mut a = v.clone();
            rebase(&mut a, v.base(), o.base());
            a.ranges = o.ranges.clone();
            a.extra = o.extra.clone();
            a == **o
        }) {
            let mut ranges = old.ranges.clone();
            ranges.push(range);
            old.set_ranges(ranges, &relocate);
        } else {
            list.push(v);
        }
    }
    list
}

impl SheetsApp {
    /// Avalia algo com o arquivo como o motor vê (índice da aba ativa).
    pub fn with_book<T>(&self, f: impl FnOnce(&duck_calc::Book, u32) -> T) -> Option<T> {
        let doc = self.doc.as_ref()?;
        // Abas que ainda não carregaram entram vazias (os índices contam).
        let missing: Vec<Sheet> = doc.sheets.iter().map(|s| Sheet::new(s.info.name.clone())).collect();
        let refs: Vec<&Sheet> =
            doc.sheets.iter().zip(&missing).map(|(s, m)| s.data.as_deref().unwrap_or(m)).collect();
        let extras = doc.calc.extras.clone();
        let book = duck_calc::Book::new(refs, &doc.strings, &extras, doc.date1904);
        Some(f(&book, doc.active as u32))
    }

    /// Célula ativa (o canto, se mesclada) e a regra dela.
    fn active_rule(&self) -> Option<(u32, u32, &DataValidation)> {
        let (sheet, v) = self.doc.as_ref()?.current()?;
        if sheet.layout.validations.is_empty() {
            return None;
        }
        let (r, c) = v.merges.at(v.cur.0, v.cur.1).map_or(v.cur, |m| (m.first_row, m.first_col));
        duck_core::valid::at(&sheet.layout.validations, r, c).map(|d| (r, c, d))
    }

    /// A seta da lista suspensa da célula ativa, se ela tiver.
    pub fn dv_arrow(&self, g: &Geom) -> Option<Rect> {
        let (r, c, rule) = self.active_rule()?;
        if rule.kind != DvKind::List || !rule.dropdown {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let (sheet, v) = doc.current()?;
        if sheet.protected().is_some() && sheet.cell_locked(&doc.styles, r, c) {
            return None;
        }
        let cell = crate::draw::cell_screen_rect(g, v, r, c)?;
        let w = (18.0 * g.s).round();
        let h = (20.0 * g.g).round().min(cell.h()).max(8.0);
        let x0 = if cell.x1 + w > g.grid.x1 { cell.x1 - w } else { cell.x1 };
        Some(Rect::new(x0, cell.y1 - h, x0 + w, cell.y1))
    }

    /// Mensagem de entrada da célula ativa: (célula, título, texto).
    pub fn dv_prompt(&self, g: &Geom) -> Option<(Rect, String, String)> {
        let (r, c, rule) = self.active_rule()?;
        if !rule.show_input || (rule.prompt.is_empty() && rule.prompt_title.is_empty()) {
            return None;
        }
        let (_, v) = self.doc.as_ref()?.current()?;
        let cell = crate::draw::cell_screen_rect(g, v, r, c)?;
        Some((cell, rule.prompt_title.clone(), rule.prompt.clone()))
    }

    /// Abre a lista da célula ativa. `false` = a célula não tem lista.
    pub fn open_list(&mut self) -> bool {
        let Some((row, col, rule)) = self.active_rule() else { return false };
        if rule.kind != DvKind::List {
            return false;
        }
        let rule = rule.clone();
        if !self.can_edit_cell(row, col) {
            return true;
        }
        let items = self
            .with_book(|b, s| duck_calc::valid::list_items(b, s, &rule, row, col))
            .flatten()
            .unwrap_or_default();
        if items.is_empty() {
            self.message = "A lista desta célula está vazia ou a origem tem erro".into();
            return true;
        }
        let g = self.geom();
        let Some(doc) = self.doc.as_ref() else { return true };
        let Some((sheet, v)) = doc.current() else { return true };
        let Some(cell) = crate::draw::cell_screen_rect(&g, v, row, col) else { return true };
        let items: Vec<(String, ListItem)> = items.into_iter().map(|i| (shown(doc, &i), i)).collect();
        let now = ListItem {
            value: match sheet.cell(row, col, &doc.strings).value {
                Value::Empty => CellValue::Empty,
                Value::Number(x) => CellValue::Number(x),
                Value::Text(t) => CellValue::Text(t.to_string()),
                Value::Bool(b) => CellValue::Bool(b),
                Value::Error(e) => CellValue::Error(e),
            },
            style: Some(sheet.cell(row, col, &doc.strings).style),
        };
        let cur = shown(doc, &now);
        let sel = items.iter().position(|i| i.1.value == now.value || (!cur.is_empty() && i.0 == cur));
        let active = doc.active;
        let s = g.s;
        let Some(text) = self.text.as_mut() else { return true };
        let ui = text.default_style();
        let widest =
            items.iter().take(300).map(|i| text.measure_styled(&i.0, ui)).fold(0.0, f32::max) + 30.0 * s;
        let w = widest.max(cell.w() + 18.0 * s).clamp(80.0 * s, 420.0 * s).round();
        let h = (items.len().min(ROWS) as f32 * ROW_H * s).round();
        let x0 = cell.x0.min(g.grid.x1 - w).max(0.0);
        let y0 = if cell.y1 + h > g.grid.y1 { (cell.y0 - h).max(0.0) } else { cell.y1 };
        let mut list = ListBox {
            sheet: active,
            row,
            col,
            items,
            sel: None,
            scroll: 0,
            rect: Rect::new(x0, y0, x0 + w, y0 + h),
        };
        if let Some(k) = sel {
            list.select(k, s);
        }
        self.dv_list = Some(list);
        true
    }

    /// Clique com a lista aberta.
    pub fn list_click(&mut self, x: f32, y: f32) {
        let Some(list) = self.dv_list.take() else { return };
        if !list.rect.contains(x, y) {
            return;
        }
        if let Some(k) = list.item_at(self.geom().s, y) {
            self.pick(&list, k);
        }
    }

    /// Tecla com a lista aberta.
    pub fn list_key(&mut self, key: &Key, mods: Modifiers, text: Option<&str>) {
        let s = self.geom().s;
        let Some(list) = &mut self.dv_list else { return };
        let last = list.items.len().saturating_sub(1);
        let page = list.fit(s);
        match key {
            Key::Escape => self.dv_list = None,
            Key::ArrowUp if mods.alt => self.dv_list = None,
            Key::ArrowDown => list.select(list.sel.map_or(0, |k| k + 1), s),
            Key::ArrowUp => list.select(list.sel.map_or(last, |k| k.saturating_sub(1)), s),
            Key::PageDown => list.select(list.sel.map_or(0, |k| k + page), s),
            Key::PageUp => list.select(list.sel.map_or(0, |k| k.saturating_sub(page)), s),
            Key::Home => list.select(0, s),
            Key::End => list.select(last, s),
            Key::Enter | Key::Tab => {
                if let Some(list) = self.dv_list.take()
                    && let Some(k) = list.sel
                {
                    self.pick(&list, k);
                }
            }
            // Uma letra vai ao próximo item que começa com ela.
            _ => {
                let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) else {
                    return;
                };
                let t = crate::search::fold(t);
                let from = list.sel.map_or(0, |k| k + 1);
                let n = list.items.len();
                let hit = (0..n)
                    .map(|i| (from + i) % n.max(1))
                    .find(|&k| crate::search::fold(&list.items[k].0).starts_with(&t));
                if let Some(k) = hit {
                    list.select(k, s);
                }
            }
        }
    }

    /// Escolhe o item `n` (a partir de 1) da lista aberta.
    pub fn scripted_pick(&mut self, n: usize) {
        match self.dv_list.take() {
            Some(list) => self.pick(&list, n.saturating_sub(1)),
            None => eprintln!("--edit lista: a célula não tem itens"),
        }
    }

    fn pick(&mut self, list: &ListBox, k: usize) {
        self.cancel_edit();
        if self.doc.as_ref().is_none_or(|d| d.active != list.sheet) {
            return;
        }
        let Some((text, item)) = list.items.get(k) else { return };
        match (&item.value, item.style) {
            // Escrito na regra: como se digitado (vira número, data...).
            (CellValue::Text(_), None) => {
                self.go_to(list.row, list.col);
                self.start_edit(Some(text), false);
                self.commit_edit();
            }
            (v, _) => {
                let change =
                    CellChange { row: list.row, col: list.col, value: v.clone(), formula: None, style: None };
                self.run_command(Command::SetCells(vec![change]));
            }
        }
    }

    /// A regra que não aceita o que foi digitado na aba ativa (entre várias, a
    /// mais severa). `None` = tudo aceito.
    pub fn broken_rule(&self, changes: &[CellChange]) -> Option<DataValidation> {
        let (sheet, _) = self.doc.as_ref()?.current()?;
        let rules = &sheet.layout.validations;
        if !changes.iter().any(|x| duck_core::valid::at(rules, x.row, x.col).is_some()) {
            return None;
        }
        let severity = |a: DvAlert| match a {
            DvAlert::Stop => 2,
            DvAlert::Warning => 1,
            DvAlert::Information => 0,
        };
        let mut worst: Option<&DataValidation> = None;
        self.with_book(|b, s| {
            for x in changes {
                let Some(rule) = duck_core::valid::at(rules, x.row, x.col) else { continue };
                if !rule.show_error || rule.kind == DvKind::Any {
                    continue;
                }
                // Uma tão severa quanto já achada não muda nada.
                if worst.is_some_and(|w| severity(w.alert) >= severity(rule.alert)) {
                    continue;
                }
                let typed = match &x.formula {
                    Some(f) => duck_calc::valid::typed_formula(b, s, x.row, x.col, f),
                    None => Some(x.value.clone()),
                };
                // O que não dá para calcular é aceito.
                let ok = typed.and_then(|v| duck_calc::valid::check(b, s, rule, x.row, x.col, &v));
                if ok == Some(false) {
                    worst = Some(rule);
                    if rule.alert == DvAlert::Stop {
                        break;
                    }
                }
            }
        });
        worst.cloned()
    }

    /// Abre o aviso da regra que não aceitou `cmd` (digitado em `cell` como
    /// `text`; `all` = com Ctrl+Enter, na seleção toda).
    pub fn ask_invalid(
        &mut self,
        rule: &DataValidation,
        cmd: Command,
        cell: (u32, u32),
        text: String,
        all: bool,
    ) {
        let Some(sheet) = self.doc.as_ref().map(|d| d.active) else { return };
        let pending = Pending { sheet, cell, cmd, text, alert: rule.alert, all };
        let default = "Este valor não segue as regras de validação de dados desta célula.";
        let mut body = if rule.error.is_empty() { default.to_string() } else { rule.error.clone() };
        let buttons: &[&'static str] = match rule.alert {
            DvAlert::Stop => &["Tentar de novo", "Cancelar"],
            DvAlert::Warning => {
                body.push_str("\nContinuar mesmo assim?");
                &["Não", "Sim", "Cancelar"]
            }
            DvAlert::Information => &["OK", "Cancelar"],
        };
        let title = if rule.error_title.is_empty() { "Validação de dados" } else { &rule.error_title };
        self.dialog =
            Some(Dialog::new(Kind::Invalid(Box::new(pending)), title, vec![Field::info(&body)], buttons));
    }

    /// Há um valor inválido à espera de resposta?
    pub fn answering(&self) -> bool {
        self.dialog.as_ref().is_some_and(|d| matches!(d.kind, Kind::Invalid(_)))
    }

    /// Resposta ao aviso de valor inválido (`None` = Esc).
    pub fn invalid_answer(&mut self, p: Pending, button: Option<usize>) {
        enum Do {
            Retry,
            Keep,
            Drop,
        }
        let what = match (p.alert, button) {
            (DvAlert::Stop, Some(0)) | (DvAlert::Warning, Some(0)) => Do::Retry,
            (DvAlert::Warning, Some(1)) | (DvAlert::Information, Some(0)) => Do::Keep,
            _ => Do::Drop,
        };
        if self.doc.as_ref().is_none_or(|d| d.active != p.sheet) {
            self.switch_sheet(p.sheet);
        }
        match what {
            Do::Retry => {
                // O que se ia fazer depois de digitar não abre por cima.
                (self.form, self.popup) = (None, None);
                if !p.all {
                    self.go_to(p.cell.0, p.cell.1);
                }
                self.start_edit(Some(&p.text), false);
                if let Some(ed) = &mut self.editor {
                    ed.enter_mode = false;
                    ed.input.select_all();
                }
            }
            Do::Keep => {
                self.run_command(p.cmd);
            }
            Do::Drop => {}
        }
    }

    /// O intervalo da seleção (linhas e colunas inteiras até o fim da planilha).
    fn rule_range(&self) -> Option<CellRange> {
        let (_, v) = self.doc.as_ref()?.current()?;
        let (r0, c0, r1, c1) = v.sel();
        let last_row = if r1 - r0 + 1 == v.rows.len() { MAX_ROWS - 1 } else { r1 };
        let last_col = if c1 - c0 + 1 == v.cols.len() { MAX_COLS - 1 } else { c1 };
        Some(CellRange { first_row: r0, first_col: c0, last_row, last_col })
    }

    /// A caixa "Validação de dados" da seleção.
    pub fn ask_validation(&mut self) {
        self.commit_edit();
        let Some(range) = self.rule_range() else { return };
        let rule = self.active_rule().map(|x| x.2.clone());
        let cur = self.doc.as_ref().and_then(|d| d.current()).map_or((0, 0), |(_, v)| v.cur);
        // A regra continua fora da seleção: dá para mudar tudo de uma vez.
        let beyond = rule
            .as_ref()
            .is_some_and(|v| v.ranges.iter().any(|r| !duck_core::valid::subtract(*r, range).is_empty()));
        let mut v = rule.unwrap_or_default();
        if !v.ranges.is_empty() {
            let base = v.base();
            rebase(&mut v, base, cur);
        }
        let kinds = KINDS.map(|k| k.1);
        let ops = OPS.map(|k| k.1);
        let alerts = ALERTS.map(|k| k.1);
        let fields = vec![
            Field::choice("Permitir", &kinds, KINDS.iter().position(|k| k.0 == v.kind).unwrap_or(0)),
            Field::choice("Dados", &ops, OPS.iter().position(|k| k.0 == v.op).unwrap_or(0)),
            Field::text("", &show_operand(&v, &v.f1)),
            Field::text("", &show_operand(&v, &v.f2)),
            Field::check("Ignorar em branco", v.allow_blank),
            Field::check("Mostrar a seta com a lista na célula", v.dropdown),
            Field::heading("Mensagem de entrada"),
            Field::check("Mostrar ao selecionar a célula", v.show_input),
            Field::text("Título", &v.prompt_title),
            Field::area("Mensagem", &v.prompt),
            Field::heading("Valor inválido"),
            Field::check("Avisar quando o valor for inválido", v.show_error),
            Field::choice("Estilo", &alerts, ALERTS.iter().position(|k| k.0 == v.alert).unwrap_or(0)),
            Field::text("Título", &v.error_title),
            Field::area("Mensagem", &v.error),
            Field::check("Aplicar a todas as células com esta regra", false),
        ];
        let mut d = Dialog::new(
            Kind::Validation,
            "Validação de dados",
            fields,
            &["OK", "Tirar a validação", "Cancelar"],
        );
        d.fields[f::ALL].show = beyond;
        refresh(&mut d);
        self.dialog = Some(d);
    }

    /// OK (ou "Tirar") da caixa de validação. `false` = falta corrigir algo.
    pub fn apply_validation(&mut self, d: &mut Dialog, remove: bool) -> bool {
        let Some(range) = self.rule_range() else { return true };
        let Some(doc) = self.doc.as_ref() else { return true };
        let Some((sheet, view)) = doc.current() else { return true };
        let (cur, date1904) = (view.cur, doc.date1904);
        let old = self.active_rule().map(|x| x.2.clone());
        let everywhere = d.fields[f::ALL].show && d.is_on(f::ALL);
        let rule = if remove {
            None
        } else {
            match read_rule(d, date1904) {
                Ok(v) => v,
                Err(e) => {
                    d.error = e;
                    return false;
                }
            }
        };
        let list = match (&old, everywhere) {
            (Some(o), true) => {
                let mut list = sheet.layout.validations.clone();
                let at = list.iter().position(|x| x == o);
                match (at, rule) {
                    (Some(i), Some(mut v)) => {
                        v.ranges = o.ranges.clone();
                        v.extra = o.extra.clone();
                        v.ext = o.ext;
                        rebase(&mut v, cur, o.base());
                        list[i] = v;
                    }
                    (Some(i), None) => {
                        list.remove(i);
                    }
                    _ => {}
                }
                list
            }
            _ => with_rule(sheet, range, rule, cur),
        };
        if list != sheet.layout.validations && self.run_command(Command::Validations(list)) {
            self.message = if remove { "Validação tirada" } else { "Validação aplicada" }.into();
        }
        true
    }

    /// `--edit validar:B2:B9=lista|Sim;Não` (tipo, limites; `nenhuma` tira).
    pub fn scripted_rule(&mut self, range: CellRange, spec: &str) {
        let mut parts = spec.split('|');
        let kind = match parts.next().unwrap_or_default() {
            "lista" => DvKind::List,
            "inteiro" => DvKind::Whole,
            "decimal" => DvKind::Decimal,
            "data" => DvKind::Date,
            "hora" => DvKind::Time,
            "texto" => DvKind::TextLength,
            "personalizada" => DvKind::Custom,
            _ => DvKind::Any,
        };
        let (a, b) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        let Some(doc) = self.doc.as_ref() else { return };
        let Some((sheet, _)) = doc.current() else { return };
        let date1904 = doc.date1904;
        let mut v = DataValidation { kind, ..Default::default() };
        let rule = match kind {
            DvKind::Any => None,
            DvKind::List => {
                set_source(&mut v, a);
                Some(v)
            }
            _ => {
                if b.is_empty() {
                    v.op = DvOp::GreaterEqual;
                }
                v.f1 = read_operand(kind, a, date1904).unwrap_or_default();
                v.f2 = read_operand(kind, b, date1904).unwrap_or_default();
                Some(v)
            }
        };
        let list = with_rule(sheet, range, rule, (range.first_row, range.first_col));
        self.run_command(Command::Validations(list));
    }
}

/// Origem da lista: itens separados por `;`, ou `=` e um intervalo, nome ou
/// fórmula.
fn set_source(v: &mut DataValidation, text: &str) {
    let t = text.trim();
    match t.strip_prefix('=') {
        Some(f) => {
            v.f1 = duck_calc::locale::from_pt(f.trim());
            v.list12 = None;
        }
        None => {
            let items: Vec<String> =
                t.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            v.set_literal_items(&items);
        }
    }
}

/// A regra que está na caixa. `Ok(None)` = nenhuma (qualquer valor, sem
/// mensagem).
fn read_rule(d: &Dialog, date1904: bool) -> Result<Option<DataValidation>, String> {
    let kind = KINDS[d.choice_of(f::KIND).min(KINDS.len() - 1)].0;
    let mut v = DataValidation {
        kind,
        op: OPS[d.choice_of(f::OP).min(OPS.len() - 1)].0,
        allow_blank: d.is_on(f::BLANK),
        dropdown: d.is_on(f::ARROW),
        show_input: d.is_on(f::SHOW_INPUT),
        show_error: d.is_on(f::SHOW_ERROR),
        alert: ALERTS[d.choice_of(f::ALERT).min(2)].0,
        prompt_title: d.text_of(f::PROMPT_TITLE).to_string(),
        prompt: d.text_of(f::PROMPT).to_string(),
        error_title: d.text_of(f::ERROR_TITLE).to_string(),
        error: d.text_of(f::ERROR).to_string(),
        ..Default::default()
    };
    // Os limites do Excel (passar deles corrompe o arquivo para ele).
    for (s, max, what) in [
        (&v.prompt_title, 32, "O título da mensagem de entrada"),
        (&v.error_title, 32, "O título do aviso"),
        (&v.prompt, 255, "A mensagem de entrada"),
        (&v.error, 255, "A mensagem do aviso"),
    ] {
        if s.encode_utf16().count() > max {
            return Err(format!("{what} passa de {max} letras."));
        }
    }
    let (a, b) = (d.text_of(f::F1).to_string(), d.text_of(f::F2).to_string());
    match kind {
        DvKind::Any => {
            v.op = DvOp::Between;
            if v.prompt.is_empty() && v.prompt_title.is_empty() {
                return Ok(None);
            }
        }
        DvKind::List => {
            v.op = DvOp::Between;
            if a.trim().trim_matches(';').is_empty() {
                return Err("Escreva os itens separados por ; ou = e um intervalo.".into());
            }
            set_source(&mut v, &a);
            if v.literal_items().is_some() && v.f1.encode_utf16().count() > 257 {
                return Err("A lista escrita passa de 255 letras: use um intervalo.".into());
            }
        }
        _ => {
            if kind == DvKind::Custom {
                v.op = DvOp::Between;
            }
            v.f1 = read_operand(kind, &a, date1904)?;
            if kind != DvKind::Custom && v.op.two() {
                v.f2 = read_operand(kind, &b, date1904)?;
            }
        }
    }
    if kind != DvKind::List {
        v.dropdown = true;
    }
    Ok(Some(v))
}

/// Mostra só os campos que o tipo usa, com os rótulos dele.
pub fn refresh(d: &mut Dialog) {
    d.error.clear();
    let kind = KINDS[d.choice_of(f::KIND).min(KINDS.len() - 1)].0;
    let two = OPS[d.choice_of(f::OP).min(OPS.len() - 1)].0.two();
    let limits = !matches!(kind, DvKind::Any | DvKind::List | DvKind::Custom);
    d.fields[f::OP].show = limits;
    d.fields[f::F1].show = kind != DvKind::Any;
    d.fields[f::F2].show = limits && two;
    d.fields[f::BLANK].show = kind != DvKind::Any;
    d.fields[f::ARROW].show = kind == DvKind::List;
    d.fields[f::F1].label = match kind {
        DvKind::List => "Itens ou origem",
        DvKind::Custom => "Fórmula",
        _ if two => "De",
        _ => "Valor",
    }
    .into();
    d.fields[f::F2].label = "Até".into();
    let input = d.is_on(f::SHOW_INPUT);
    d.fields[f::PROMPT_TITLE].show = input;
    d.fields[f::PROMPT].show = input;
    let error = d.is_on(f::SHOW_ERROR) && kind != DvKind::Any;
    d.fields[f::SHOW_ERROR].show = kind != DvKind::Any;
    for i in [f::ALERT, f::ERROR_TITLE, f::ERROR] {
        d.fields[i].show = error;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: u32, b: u32, c: u32, d: u32) -> CellRange {
        CellRange { first_row: a, first_col: b, last_row: c, last_col: d }
    }

    #[test]
    fn operands() {
        let v = |kind, f1: &str| DataValidation { kind, f1: f1.into(), ..Default::default() };
        let show = |kind, f: &str| show_operand(&v(kind, f), f);
        assert_eq!(show(DvKind::Decimal, "99.5"), "99,5");
        assert_eq!(show(DvKind::Date, "46387"), "31/12/2026");
        assert_eq!(show(DvKind::Time, "0.5"), "12:00:00");
        assert_eq!(show(DvKind::List, "\"Sim,Não\""), "Sim;Não");
        assert_eq!(show(DvKind::List, "$A$1:$A$3"), "=$A$1:$A$3");
        assert_eq!(show(DvKind::Custom, "COUNTIF(A:A,A1)=1"), "=CONT.SE(A:A;A1)=1");
        let read = |kind, t: &str| read_operand(kind, t, false);
        assert_eq!(read(DvKind::Decimal, "99,5").unwrap(), "99.5");
        assert_eq!(read(DvKind::Date, "31/12/2026").unwrap(), "46387");
        assert_eq!(read(DvKind::Whole, "=SOMA(A1;2)").unwrap(), "SUM(A1,2)");
        assert_eq!(read(DvKind::Custom, "CONT.SE(A:A;A1)=1").unwrap(), "COUNTIF(A:A,A1)=1");
        assert!(read(DvKind::Whole, "abc").is_err() && read(DvKind::Whole, " ").is_err());
        // As funções novas: com os prefixos no arquivo, sem eles na tela.
        for (kind, pt, file) in [
            (DvKind::Custom, "=LET(x;A1;x>1,5)", "_xlfn.LET(_xlpm.x,A1,_xlpm.x>1.5)"),
            (
                DvKind::Custom,
                "=LAMBDA(a;b;a+b)(A1;1)>2",
                "_xlfn.LAMBDA(_xlpm.a,_xlpm.b,_xlpm.a+_xlpm.b)(A1,1)>2",
            ),
            (DvKind::Decimal, "=PROCX(A1;B:B;C:C;0,5)", "_xlfn.XLOOKUP(A1,B:B,C:C,0.5)"),
            (DvKind::List, "=FILTRO(A1:A9;B1:B9>1,5)", "_xlfn._xlws.FILTER(A1:A9,B1:B9>1.5)"),
        ] {
            assert_eq!(show(kind, file), pt);
            if kind == DvKind::List {
                let mut l = v(kind, "");
                set_source(&mut l, pt);
                assert_eq!(l.f1, file);
            } else {
                assert_eq!(read(kind, pt).unwrap(), file);
            }
        }
        let mut l = v(DvKind::List, "");
        set_source(&mut l, "a; b ;;c");
        assert_eq!(l.f1, "\"a,b,c\"");
        set_source(&mut l, "=Frutas");
        assert_eq!(l.f1, "Frutas");
    }

    #[test]
    fn rules_by_range() {
        let mut s = Sheet::new("P");
        let rule =
            |f: &str| Some(DataValidation { kind: DvKind::Custom, f1: f.into(), ..Default::default() });
        // Digitada em A5 (célula ativa), vale de A3 a A6: a base é A3.
        s.layout.validations = with_rule(&s, r(2, 0, 5, 0), rule("B5>1"), (4, 0));
        assert_eq!(s.layout.validations[0].f1, "B3>1");
        // A mesma regra no intervalo vizinho junta.
        s.layout.validations = with_rule(&s, r(6, 0, 8, 0), rule("B7>1"), (6, 0));
        assert_eq!(s.layout.validations.len(), 1);
        assert_eq!(s.layout.validations[0].ranges, [r(2, 0, 5, 0), r(6, 0, 8, 0)]);
        // Outra por cima de uma parte: a antiga se divide.
        s.layout.validations = with_rule(&s, r(3, 0, 3, 0), rule("C4=2"), (3, 0));
        assert_eq!(s.layout.validations.len(), 2);
        assert_eq!(s.layout.validations[0].ranges, [r(2, 0, 2, 0), r(4, 0, 5, 0), r(6, 0, 8, 0)]);
        s.layout.validations = with_rule(&s, r(0, 0, 99, 9), None, (0, 0));
        assert!(s.layout.validations.is_empty());
    }
}
