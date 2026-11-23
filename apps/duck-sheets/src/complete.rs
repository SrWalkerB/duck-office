//! Autocompletar fórmulas, na célula e na barra: a lista de funções, nomes,
//! tabelas e colunas que começam com o que está sendo digitado, e a
//! assinatura da função em volta do cursor.
//!
//! - Os nomes, os argumentos e as descrições vêm da tabela do motor
//!   (`duck_calc::funcs`), a mesma que calcula: uma função nova aparece aqui
//!   sozinha. As que o motor não calcula ficam de fora.
//! - [`analyze`] só lê o texto até o cursor (sem montar a fórmula): texto
//!   entre aspas, nomes de aba entre apóstrofos e `Tabela[...]` são pulados.
//! - Com só o `=`, como no Excel, não há lista (as setas continuam confirmando
//!   e andando): aparece uma dica de uma linha.

use std::ops::Range;
use std::sync::OnceLock;

use duck_calc::funcs::{self, Func};
use duck_platform::{Key, Modifiers};
use duck_text::{TextStyle, TextSystem};
use duck_ui::{Align, Painter, Rect, Rgba};

use crate::app::{Geom, SheetsApp, View};
use crate::edit::Editor;
use crate::search::fold;
use crate::theme::Theme;

/// Linhas visíveis da lista.
pub const ROWS: usize = 8;
/// Itens no máximo (a lista rola; mais que isso é melhor digitar).
const MAX_ITEMS: usize = 400;

// ---------------------------------------------------------------------------
// Análise do texto
// ---------------------------------------------------------------------------

/// O que está sendo digitado.
#[derive(Clone, Debug, PartialEq)]
pub enum What {
    /// Função, nome definido ou tabela.
    Name,
    /// Coluna de uma tabela (vazio = a tabela da célula, `[@Coluna]`).
    Column {
        table: String,
        /// Depois de `@`.
        row: bool,
        /// Dentro de colchetes próprios (`T[[Coluna`, `T[@[Coluna`).
        nested: bool,
    },
}

/// O trecho que a sugestão aceita substitui.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub range: Range<usize>,
    pub what: What,
}

/// A função em volta do cursor e o argumento em que ele está.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub func: Func,
    pub arg: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub token: Option<Token>,
    pub call: Option<Call>,
    /// Só o `=` foi digitado.
    pub start: bool,
    /// Cabe uma referência no cursor (depois de `=`, operador, `(` ou `;`).
    pub accepts: bool,
    /// Parênteses abertos até o cursor: a função e quantos `;` já vieram.
    pub open: Vec<(Option<Func>, usize)>,
    /// Nada pela metade além dos parênteses (texto, aba, colchete, chave).
    pub sound: bool,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '\\' | '?')
}

enum Open {
    Paren(Option<Func>, usize),
    Brace,
}

/// Lê `text` até o cursor (posição em bytes).
pub fn analyze(text: &str, cursor: usize) -> Analysis {
    let mut out = Analysis::default();
    let Some(body) = text.get(..cursor) else { return out };
    if !body.starts_with('=') {
        return out;
    }
    out.start = text.trim_end() == "=";
    let s: Vec<(usize, char)> = body.char_indices().collect();
    let n = s.len();
    let pos = |i: usize| s.get(i).map_or(body.len(), |x| x.0);
    let mut stack: Vec<Open> = Vec::new();
    let mut i = 1;
    let mut inside = false;
    let mut stray = false;
    while i < n {
        let c = s[i].1;
        match c {
            '"' | '\'' => {
                // Até a aspa que fecha (dobrada = escape).
                let mut j = i + 1;
                loop {
                    if j >= n {
                        inside = true;
                        break;
                    }
                    if s[j].1 == c {
                        if s.get(j + 1).is_some_and(|x| x.1 == c) {
                            j += 2;
                            continue;
                        }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                i = j;
            }
            '[' => match close_bracket(&s, i) {
                Some(j) => i = j,
                None => {
                    inside = true;
                    out.token = column_token(&s, i, body, text);
                    i = n;
                }
            },
            '(' => {
                stack.push(Open::Paren(None, 0));
                i += 1;
            }
            '{' => {
                stack.push(Open::Brace);
                i += 1;
            }
            ')' | '}' => {
                stray |= stack.pop().is_none();
                i += 1;
            }
            ';' => {
                if let Some(Open::Paren(_, k)) = stack.last_mut() {
                    *k += 1;
                }
                i += 1;
            }
            // Erro (`#N/D`); depois de um nome é o operador de espalhar (`A1#`).
            '#' if !is_ident(s[i - 1].1) => {
                i += 1;
                while i < n && (s[i].1.is_alphanumeric() || matches!(s[i].1, '/' | '!' | '?' | '_')) {
                    i += 1;
                }
            }
            _ if is_ident(c) || c == '$' => {
                let st = i;
                while i < n && (is_ident(s[i].1) || s[i].1 == '$') {
                    i += 1;
                }
                let word = &body[pos(st)..pos(i)];
                let mut j = i;
                while j < n && s[j].1 == ' ' {
                    j += 1;
                }
                if j < n && s[j].1 == '(' {
                    let f = funcs::lookup_pt(word).or_else(|| funcs::lookup(word));
                    stack.push(Open::Paren(f, 0));
                    i = j + 1;
                } else if i == n {
                    let named = c.is_alphabetic() || c == '_';
                    let joined = matches!(s[st - 1].1, ':' | '!' | ']' | ')' | '"' | '\'' | '%' | '}' | '#');
                    // No meio de uma palavra, ou um nome de aba: nada a sugerir.
                    let next = text[cursor..].chars().next();
                    let mid = next.is_some_and(|c| is_ident(c) || matches!(c, '$' | '!' | '[' | ':'));
                    if named && !word.contains('$') && !joined && !mid {
                        out.token = Some(Token { range: pos(st)..cursor, what: What::Name });
                    }
                }
            }
            _ => i += 1,
        }
    }
    // A chamada mais de dentro: parênteses só de agrupamento não contam.
    out.call = stack.iter().rev().find_map(|o| match o {
        Open::Paren(Some(func), arg) => Some(Call { func: *func, arg: *arg }),
        _ => None,
    });
    let brace = stack.iter().any(|o| matches!(o, Open::Brace));
    out.sound = !inside && !brace && !stray;
    let mut back = body.chars().rev().filter(|c| *c != ' ');
    let fits = match back.next() {
        Some(':') => back.next().is_some_and(|c| is_ident(c) || c == ')'),
        Some(c) => matches!(c, '=' | '+' | '-' | '*' | '/' | '^' | '&' | '<' | '>' | '(' | ';' | '@'),
        None => false,
    };
    let glued = text[cursor..].chars().next().is_some_and(|c| {
        is_ident(c) || matches!(c, '$' | '"' | '\'' | '[' | '(' | '{' | '@' | '#' | '!' | ':')
    });
    out.accepts = !inside && !brace && fits && !glued;
    out.open = stack
        .iter()
        .filter_map(|o| match o {
            Open::Paren(f, k) => Some((*f, *k)),
            Open::Brace => None,
        })
        .collect();
    out
}

/// O fim do `[...]` que começa em `i` (aninhado; `'` escapa).
fn close_bracket(s: &[(usize, char)], i: usize) -> Option<usize> {
    let mut depth = 0;
    let mut j = i;
    while j < s.len() {
        match s[j].1 {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            '\'' => j += 1,
            _ => {}
        }
        j += 1;
    }
    None
}

/// A coluna sendo digitada no `[` aberto em `i`.
fn column_token(s: &[(usize, char)], i: usize, body: &str, text: &str) -> Option<Token> {
    // O nome da tabela, colado no `[`.
    let mut st = i;
    while st > 1 && is_ident(s[st - 1].1) {
        st -= 1;
    }
    let table = body[s[st].0..s[i].0].to_string();
    let mut depth = 1;
    let mut row = false;
    let mut from = i + 1;
    let mut j = i + 1;
    while j < s.len() {
        match s[j].1 {
            '[' => {
                depth += 1;
                from = j + 1;
            }
            ']' => {
                depth -= 1;
                // Fechou um item: falta abrir o próximo.
                from = usize::MAX;
            }
            '@' if depth == 1 => {
                row = true;
                from = j + 1;
            }
            '\'' => j += 1,
            _ => {}
        }
        j += 1;
    }
    if from == usize::MAX {
        return None;
    }
    let start = s.get(from).map_or(body.len(), |x| x.0);
    let prefix = &body[start..];
    // `#Dados`, `#Totais`... não são colunas; espaço antes do `[` de dentro também não.
    if prefix.starts_with('#') {
        return None;
    }
    let next = text[body.len()..].chars().next();
    if next.is_some_and(|c| !matches!(c, ']' | ')' | ';' | ' ' | ',') && is_ident(c)) {
        return None;
    }
    Some(Token { range: start..body.len(), what: What::Column { table, row, nested: depth == 2 } })
}

// ---------------------------------------------------------------------------
// Candidatos
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Func(Func),
    Name,
    Table,
    Column,
}

/// Uma sugestão.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub kind: Kind,
    /// O que é, numa linha.
    pub help: String,
    folded: String,
    /// Nome em inglês (também acha).
    en: &'static str,
}

/// Funções que o motor calcula, em ordem alfabética.
fn functions() -> &'static [Item] {
    static F: OnceLock<Vec<Item>> = OnceLock::new();
    F.get_or_init(|| {
        let mut v: Vec<Item> = funcs::all()
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.opaque)
            .map(|(i, d)| Item {
                name: d.pt.to_string(),
                kind: Kind::Func(i as Func),
                help: d.help.to_string(),
                folded: fold(d.pt),
                en: d.name,
            })
            .collect();
        v.sort_by(|a, b| a.folded.cmp(&b.folded));
        v
    })
}

/// O que o arquivo aberto tem para sugerir (montado uma vez por edição).
#[derive(Clone, Debug, Default)]
pub struct Source {
    /// Nomes definidos e tabelas.
    names: Vec<Item>,
    /// Tabelas: (nome dobrado, colunas).
    tables: Vec<(String, Vec<String>)>,
    /// A tabela que contém a célula editada.
    here: Option<usize>,
}

impl Source {
    /// `names`: (nome, fórmula em português); `tables`: (nome, colunas).
    pub fn new(
        names: impl IntoIterator<Item = (String, String)>,
        tables: impl IntoIterator<Item = (String, Vec<String>)>,
        here: Option<usize>,
    ) -> Self {
        let mut src = Source { here, ..Default::default() };
        for (name, formula) in names {
            let folded = fold(&name);
            src.names.push(Item { name, kind: Kind::Name, help: formula, folded, en: "" });
        }
        for (name, cols) in tables {
            let help = match cols.len() {
                1 => "Tabela com 1 coluna".to_string(),
                n => format!("Tabela com {n} colunas"),
            };
            let folded = fold(&name);
            src.tables.push((folded.clone(), cols));
            src.names.push(Item { name, kind: Kind::Table, help, folded, en: "" });
        }
        src
    }

    /// Sugestões para o trecho `typed`, as melhores primeiro.
    pub fn find(&self, what: &What, typed: &str) -> Vec<Item> {
        match what {
            What::Name => self.find_names(typed),
            What::Column { table, .. } => self.find_columns(table, typed),
        }
    }

    fn find_names(&self, typed: &str) -> Vec<Item> {
        let t = fold(typed);
        if t.is_empty() {
            return Vec::new();
        }
        // Uma referência (`B2`, `AB`) só vira sugestão se um nome começa assim.
        let loose = t.chars().count() >= 2 && crate::app::parse_cell(typed).is_none();
        let up = typed.to_ascii_uppercase();
        let rank = |it: &Item| -> Option<u8> {
            if it.folded.starts_with(&t) {
                return Some(0);
            }
            if !loose {
                return None;
            }
            // `vazio` acha CONTAR.VAZIO; `contse`, CONT.SE; `vlook`, PROCV.
            if it.folded.split('.').skip(1).any(|p| p.starts_with(&t)) {
                return Some(1);
            }
            if it.folded.contains('.') && it.folded.replace('.', "").starts_with(&t) {
                return Some(1);
            }
            if t.len() >= 3 && !it.en.is_empty() && it.en.starts_with(&up) {
                return Some(2);
            }
            (t.chars().count() >= 3 && it.folded.contains(&t)).then_some(3)
        };
        let mut v: Vec<(u8, &Item)> =
            self.names.iter().chain(functions()).filter_map(|it| Some((rank(it)?, it))).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.folded.cmp(&b.1.folded)));
        // Um nome ou tabela já escrito por inteiro: nada a completar.
        if let [(_, only)] = v.as_slice()
            && !matches!(only.kind, Kind::Func(_))
            && only.folded == t
        {
            return Vec::new();
        }
        v.into_iter().take(MAX_ITEMS).map(|x| x.1.clone()).collect()
    }

    fn find_columns(&self, table: &str, typed: &str) -> Vec<Item> {
        let cols = if table.is_empty() {
            self.here.and_then(|i| self.tables.get(i))
        } else {
            let t = fold(table);
            self.tables.iter().find(|x| x.0 == t)
        };
        let Some((_, cols)) = cols else { return Vec::new() };
        let t = fold(&typed.replace('\'', ""));
        let mut v: Vec<(bool, Item)> = cols
            .iter()
            .filter_map(|c| {
                let folded = fold(c);
                let first = folded.starts_with(&t);
                (first || folded.contains(&t)).then(|| {
                    // Um título com quebra de linha aparece numa linha só.
                    (
                        !first,
                        Item {
                            name: c.clone(),
                            kind: Kind::Column,
                            help: "Coluna da tabela".into(),
                            folded,
                            en: "",
                        },
                    )
                })
            })
            .collect();
        // As que começam primeiro; dentro de cada grupo, a ordem da tabela.
        v.sort_by_key(|x| x.0);
        v.into_iter().take(MAX_ITEMS).map(|x| x.1).collect()
    }
}

/// Nome de coluna como se escreve numa referência de tabela.
fn escape_column(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '[' | ']' | '#' | '\'' | '@') {
            out.push('\'');
        }
        out.push(c);
    }
    out
}

/// Texto que entra no lugar do trecho ao aceitar `item`. `rest` = o que vem
/// depois do cursor (um `(` ou `]` que já está lá não se repete).
pub fn insertion(item: &Item, what: &What, rest: &str) -> String {
    match item.kind {
        // VERDADEIRO e FALSO não levam parênteses.
        Kind::Func(f) if matches!(funcs::def(f).name, "TRUE" | "FALSE") => item.name.clone(),
        Kind::Func(_) if rest.starts_with('(') => item.name.clone(),
        Kind::Func(_) => format!("{}(", item.name),
        Kind::Name | Kind::Table => item.name.clone(),
        Kind::Column => {
            let name = escape_column(&item.name);
            let plain = item.name.chars().all(|c| c.is_alphanumeric() || c == '_');
            let (open, close) = match what {
                What::Column { row: true, nested: true, .. } => ("", "]]"),
                // `[@Valor total]` precisa de colchetes próprios.
                What::Column { row: true, nested: false, .. } if !plain => ("[", "]]"),
                _ => ("", "]"),
            };
            let there = rest.chars().take_while(|c| *c == ']').count().min(close.len());
            format!("{open}{name}{}", &close[there..])
        }
    }
}

// ---------------------------------------------------------------------------
// Estado
// ---------------------------------------------------------------------------

/// Lista aberta.
#[derive(Clone, Debug)]
pub struct List {
    pub items: Vec<Item>,
    pub token: Token,
    pub sel: usize,
    /// Primeira linha à vista.
    pub top: usize,
}

impl List {
    fn select(&mut self, i: usize) {
        self.sel = i.min(self.items.len().saturating_sub(1));
        self.top = self.top.min(self.sel).max((self.sel + 1).saturating_sub(ROWS));
    }

    fn scroll(&mut self, d: i32) {
        let max = self.items.len().saturating_sub(ROWS) as i32;
        self.top = (self.top as i32 + d).clamp(0, max) as usize;
    }
}

/// Estado do autocompletar de uma edição.
#[derive(Clone, Debug, Default)]
pub struct State {
    source: Option<Source>,
    pub list: Option<List>,
    pub call: Option<Call>,
    /// Só o `=`: mostra a dica.
    pub start: bool,
    /// Início do trecho em que o Esc fechou a lista (fica fechada nele).
    dismissed: Option<usize>,
}

impl State {
    /// Refaz lista e assinatura. `typed` = o texto mudou (mover o cursor só
    /// atualiza a assinatura e fecha a lista, como no Excel).
    pub fn update(&mut self, text: &str, cursor: usize, typed: bool, source: impl FnOnce() -> Source) {
        let a = analyze(text, cursor);
        self.call = a.call;
        self.start = a.start;
        self.list = None;
        let Some(token) = a.token else {
            self.dismissed = None;
            return;
        };
        if self.dismissed.is_some_and(|d| d != token.range.start) {
            self.dismissed = None;
        }
        if !typed || self.dismissed.is_some() {
            return;
        }
        let src = self.source.get_or_insert_with(source);
        let items = src.find(&token.what, &text[token.range.clone()]);
        if !items.is_empty() {
            self.list = Some(List { items, token, sel: 0, top: 0 });
        }
    }

    /// Esc com a lista aberta.
    fn dismiss(&mut self) {
        if let Some(l) = self.list.take() {
            self.dismissed = Some(l.token.range.start);
        }
    }
}

/// Aceita o item `i` da lista aberta no texto do editor. `false` = não há.
pub fn accept(ed: &mut Editor, i: usize) -> bool {
    let Some(l) = ed.complete.list.take() else { return false };
    let Some(item) = l.items.get(i) else { return false };
    let text = insertion(item, &l.token.what, &ed.input.text[l.token.range.end..]);
    let paren = text.ends_with(|c: char| c != '(') && matches!(item.kind, Kind::Func(_));
    ed.point = None;
    ed.input.replace(l.token.range.clone(), &text);
    // O `(` que já estava lá: o cursor passa por ele.
    if paren && ed.input.text[ed.input.cursor()..].starts_with('(') {
        let to = ed.input.cursor() + 1;
        ed.input.set_cursor(to);
    }
    true
}

impl SheetsApp {
    /// Depois de mudar o texto (`typed`) ou só o cursor do editor.
    pub fn refresh_complete(&mut self, typed: bool) {
        let Some(mut ed) = self.editor.take() else { return };
        let at = (ed.sheet, ed.row, ed.col);
        ed.complete.update(&ed.input.text, ed.input.cursor(), typed, || self.complete_source(at));
        self.editor = Some(ed);
    }

    /// Nomes e tabelas para a célula (aba, linha, coluna) em edição.
    fn complete_source(&self, (home, row, col): (usize, u32, u32)) -> Source {
        let Some(doc) = &self.doc else { return Source::default() };
        let names = doc
            .calc
            .extras
            .names
            .iter()
            .filter(|n| !n.hidden && !n.name.starts_with("_xl"))
            .filter(|n| n.sheet.is_none_or(|s| s as usize == home))
            .map(|n| (n.name.clone(), duck_calc::locale::to_pt(&n.formula)));
        let mut here = None;
        let mut tables = Vec::new();
        for (i, slot) in doc.sheets.iter().enumerate() {
            let Some(sheet) = &slot.data else { continue };
            for t in &sheet.layout.tables {
                if i == home && t.range.contains(row, col) {
                    here = Some(tables.len());
                }
                tables.push((t.name.clone(), t.columns.clone()));
            }
        }
        Source::new(names, tables, here)
    }

    /// Teclas da lista aberta. `false` = segue o fluxo normal do editor.
    pub fn complete_key(&mut self, key: &Key, mods: Modifiers) -> bool {
        let Some(ed) = &mut self.editor else { return false };
        let Some(l) = &mut ed.complete.list else { return false };
        if mods.ctrl || mods.alt || mods.logo || mods.shift {
            return false;
        }
        match key {
            Key::ArrowDown => l.select(if l.sel + 1 == l.items.len() { 0 } else { l.sel + 1 }),
            Key::ArrowUp => l.select(if l.sel == 0 { l.items.len() - 1 } else { l.sel - 1 }),
            Key::PageDown => l.select(l.sel + ROWS),
            Key::PageUp => l.select(l.sel.saturating_sub(ROWS)),
            Key::Escape => ed.complete.dismiss(),
            Key::Tab => {
                let i = l.sel;
                accept(ed, i);
                self.refresh_complete(false);
            }
            _ => return false,
        }
        true
    }

    /// Clique sobre a lista, a assinatura ou a dica. `true` = era nelas (o
    /// clique não chega à grade, que confirmaria a edição).
    pub fn complete_click(&mut self, g: &Geom, x: f32, y: f32) -> bool {
        let Some(b) = self.complete_boxes(g) else { return false };
        if let Some(i) = b.row_at(x, y) {
            if let Some(ed) = &mut self.editor {
                accept(ed, i);
            }
            self.refresh_complete(false);
            return true;
        }
        b.contains(x, y)
    }

    /// Roda do mouse sobre a lista.
    pub fn complete_wheel(&mut self, g: &Geom, x: f32, y: f32, up: bool) -> bool {
        let Some(b) = self.complete_boxes(g) else { return false };
        if !b.list.is_some_and(|r| r.contains(x, y)) {
            return false;
        }
        if let Some(l) = self.editor.as_mut().and_then(|e| e.complete.list.as_mut()) {
            l.scroll(if up { -3 } else { 3 });
        }
        true
    }

    pub fn complete_boxes(&mut self, g: &Geom) -> Option<Boxes> {
        let ed = self.editor.as_ref()?;
        let view = self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| v)?;
        let text = self.text.as_mut()?;
        let ui = text.default_style();
        boxes(ed, g, view, text, ui)
    }
}

// ---------------------------------------------------------------------------
// Lugar na tela
// ---------------------------------------------------------------------------

/// Onde ficam as caixas do autocompletar.
#[derive(Clone, Debug, Default)]
pub struct Boxes {
    /// Assinatura ou dica.
    pub sig: Option<Rect>,
    pub list: Option<Rect>,
    /// Linhas visíveis da lista: (item, retângulo).
    pub rows: Vec<(usize, Rect)>,
    /// Pedaços da assinatura: (linha, x, texto, é o argumento atual).
    sig_text: Vec<(usize, f32, String, bool)>,
}

impl Boxes {
    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        self.rows.iter().find(|r| r.1.contains(x, y)).map(|r| r.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.sig.is_some_and(|r| r.contains(x, y)) || self.list.is_some_and(|r| r.contains(x, y))
    }
}

/// Retângulo do editor (na célula ou na barra), a fonte e a margem dele.
pub fn editor_rect(
    ed: &Editor,
    g: &Geom,
    view: &View,
    text: &mut TextSystem,
    ui: TextStyle,
) -> (Rect, TextStyle, f32) {
    let cell = if ed.in_bar { None } else { cell_editor(ed, g, view, text, ui) };
    cell.unwrap_or((g.formula.inset(2.0 * g.s), ui, 2.0 * g.s))
}

/// O editor sobre a célula, se ela está à vista.
pub fn cell_editor(
    ed: &Editor,
    g: &Geom,
    view: &View,
    text: &mut TextSystem,
    ui: TextStyle,
) -> Option<(Rect, TextStyle, f32)> {
    if ed.away {
        return None;
    }
    let cell = crate::draw::cell_screen_rect(g, view, ed.row, ed.col)?;
    let style = ui.with_px((11.0 * 96.0 / 72.0 * g.g).round().max(6.0));
    let pad = (3.0 * g.g).round().max(1.0);
    let need = text.measure_styled(&ed.input.text, style) + 2.0 * pad + 8.0 * g.g;
    let x1 = (cell.x0 + need).max(cell.x1).min(g.grid.x1);
    Some((Rect::new(cell.x0, cell.y0, x1, cell.y1), style, pad))
}

const HINT: &str = "Digite o nome de uma função (como SOMA), uma célula (como A1) ou um valor";
const KEYS: &str = "↑↓ escolhe · Tab insere · Esc fecha";
const ROW_H: f32 = 24.0;
const LIST_W: f32 = 400.0;
const FOOT_H: f32 = 44.0;

/// Pedaços da assinatura: nome, argumentos e separadores, com o atual marcado.
pub fn signature(call: Call) -> Vec<(String, bool)> {
    let d = funcs::def(call.func);
    let args = d.arg_names();
    let mut out = vec![(format!("{}(", d.pt), false)];
    for (i, a) in args.iter().enumerate() {
        // Além dos que aparecem, o cursor está no "…".
        let on = i == call.arg || (*a == "…" && call.arg > i);
        if i > 0 {
            out.push(("; ".into(), false));
        }
        out.push((a.to_string(), on));
    }
    out.push((")".into(), false));
    out
}

fn bold(mut style: TextStyle, on: bool) -> TextStyle {
    style.bold = on;
    style
}

/// Calcula as caixas. Nunca saem da janela.
pub fn boxes(ed: &Editor, g: &Geom, view: &View, text: &mut TextSystem, ui: TextStyle) -> Option<Boxes> {
    let st = &ed.complete;
    // Funções que o motor não calcula não têm assinatura.
    let call = st.call.filter(|c| !funcs::def(c.func).opaque);
    if st.list.is_none() && call.is_none() && !st.start {
        return None;
    }
    let s = g.s;
    let (rect, style, pad) = editor_rect(ed, g, view, text, ui);
    // x do começo do trecho (o campo rola para o cursor ficar à vista).
    let at = st.list.as_ref().map_or(0, |l| l.token.range.start);
    let caret = text.measure_styled(&ed.input.text[..ed.input.cursor()], style);
    let shift = (caret - (rect.w() - 2.0 * pad)).max(0.0);
    let x = rect.x0 + pad - shift + text.measure_styled(&ed.input.text[..at], style);
    let win = Rect::new(0.0, 0.0, g.w, g.status.y1);

    let mut b = Boxes::default();
    let small = ui.with_px((12.0 * s).round());
    let lh = (text.line_height_of(small) + 2.0 * s).round();
    let max_w = (win.w() - 16.0 * s).max(40.0);
    let mut sig_size = None;
    if call.is_some() || st.start {
        let parts = match call {
            Some(c) => signature(c),
            None => vec![(HINT.to_string(), false)],
        };
        let inner = (max_w - 16.0 * s).max(20.0);
        let (mut line, mut lx, mut widest) = (0usize, 0.0f32, 0.0f32);
        for (t, on) in parts {
            let w = text.measure_styled(&t, bold(small, on));
            if lx > 0.0 && lx + w > inner && t != "; " && t != ")" {
                line += 1;
                lx = 0.0;
            }
            b.sig_text.push((line, lx, t, on));
            lx += w;
            widest = widest.max(lx);
        }
        sig_size = Some(((widest + 16.0 * s).min(max_w).ceil(), (line + 1) as f32 * lh + 8.0 * s));
    }
    let list = st.list.as_ref().map(|l| (l.items.len(), (LIST_W * s).min(max_w), ROW_H * s, FOOT_H * s));
    let (sig, lr, rows) = place(rect, x - 8.0 * s, sig_size, list, win, (3.0 * s).round());
    b.sig = sig;
    b.list = lr;
    if let (Some(l), Some(r)) = (&st.list, lr) {
        // Com menos linhas que o normal (janela baixa), o item escolhido fica à vista.
        let top = l.top.max((l.sel + 1).saturating_sub(rows)).min(l.items.len() - rows.min(l.items.len()));
        let top = if rows < ROWS { top } else { l.top };
        for k in 0..rows {
            let y = r.y0 + 4.0 * s + k as f32 * ROW_H * s;
            b.rows.push((top + k, Rect::new(r.x0, y, r.x1, y + ROW_H * s)));
        }
    }
    Some(b)
}

/// Coloca assinatura e lista perto de `anchor`, dentro de `win`: embaixo se
/// couber (ou onde houver mais espaço), a assinatura junto do editor.
/// `list` = (itens, largura, altura da linha, altura do rodapé). Devolve
/// também quantas linhas a lista mostra.
pub fn place(
    anchor: Rect,
    x: f32,
    sig: Option<(f32, f32)>,
    list: Option<(usize, f32, f32, f32)>,
    win: Rect,
    gap: f32,
) -> (Option<Rect>, Option<Rect>, usize) {
    let sig_h = sig.map_or(0.0, |s| s.1 + gap);
    let frame = gap * 8.0 / 3.0;
    let list_h = |rows: usize| list.map_or(0.0, |l| rows as f32 * l.2 + l.3 + frame + gap);
    let want = list.map_or(0, |l| l.0.min(ROWS));
    let below = (win.y1 - anchor.y1).max(0.0);
    let above = (anchor.y0 - win.y0).max(0.0);
    let down = sig_h + list_h(want) <= below || below >= above;
    let room = if down { below } else { above };
    let mut rows = want;
    while rows > 1 && sig_h + list_h(rows) > room {
        rows -= 1;
    }
    let fit = |w: f32, h: f32, y: f32| {
        let w = w.min(win.w());
        let h = h.min(win.h());
        let x0 = x.min(win.x1 - w).max(win.x0).round();
        let y0 = y.min(win.y1 - h).max(win.y0).round();
        Rect::new(x0, y0, (x0 + w).min(win.x1), (y0 + h).min(win.y1))
    };
    let sig_r = sig.map(|(w, h)| fit(w, h, if down { anchor.y1 + gap } else { anchor.y0 - gap - h }));
    let list_r = list.map(|l| {
        let h = list_h(rows) - gap;
        let y = if down { anchor.y1 + sig_h + gap } else { anchor.y0 - sig_h - gap - h };
        fit(l.1, h, y)
    });
    (sig_r, list_r, rows)
}

// ---------------------------------------------------------------------------
// Desenho
// ---------------------------------------------------------------------------

fn frame(p: &mut Painter, r: Rect, s: f32, th: &Theme) {
    let all = Rect::everything();
    let px = s.round().max(1.0);
    p.fill(Rect::new(r.x0 + 2.0 * s, r.y0 + 2.0 * s, r.x1 + 3.0 * s, r.y1 + 3.0 * s), &all, th.shadow);
    p.fill(r, &all, th.line);
    p.fill(r.inset(px), &all, th.menu);
}

/// Símbolo do tipo da sugestão.
fn symbol(p: &mut Painter, kind: Kind, r: Rect, ui: TextStyle, s: f32, th: &Theme) {
    let all = Rect::everything();
    let px = s.round().max(1.0);
    let (cx, cy) = (((r.x0 + r.x1) / 2.0).round(), ((r.y0 + r.y1) / 2.0).round());
    let cell = |x: f32, y: f32, w: f32, h: f32| {
        Rect::new(cx + x * s, cy + y * s, cx + (x + w) * s, cy + (y + h) * s)
    };
    match kind {
        Kind::Func(_) => {
            let mut it = ui;
            it.italic = true;
            p.label("fx", it, r, Align::Center, 0.0, th.accent);
        }
        Kind::Name => {
            p.stroke(cell(-7.0, -5.0, 14.0, 10.0), &all, px, th.muted);
            p.fill(cell(-4.0, -1.0, 8.0, 2.0), &all, th.muted);
        }
        Kind::Table | Kind::Column => {
            let b = cell(-7.0, -6.0, 14.0, 12.0);
            let head =
                if kind == Kind::Table { cell(-7.0, -6.0, 14.0, 4.0) } else { cell(-2.0, -6.0, 5.0, 12.0) };
            p.fill(head, &all, th.accent);
            p.stroke(b, &all, px, th.muted);
            p.fill(cell(-2.0, -6.0, 1.0, 12.0), &all, th.muted);
            p.fill(cell(3.0, -6.0, 1.0, 12.0), &all, th.muted);
        }
    }
}

/// Desenha a assinatura (ou a dica) e a lista.
pub fn draw(p: &mut Painter, ed: &Editor, b: &Boxes, s: f32, ui: TextStyle, mouse: (f32, f32), th: &Theme) {
    let all = Rect::everything();
    let px = s.round().max(1.0);
    let small = ui.with_px((12.0 * s).round());
    if let Some(r) = b.sig {
        frame(p, r, s, th);
        let lh = (p.text.line_height_of(small) + 2.0 * s).round();
        let hint = ed.complete.call.is_none();
        for (line, x, t, on) in &b.sig_text {
            let color: Rgba = match (hint, on) {
                (true, _) => th.muted,
                (_, true) => th.accent,
                _ => th.text,
            };
            let y = r.y0 + 5.0 * s + *line as f32 * lh;
            p.text(t, bold(small, *on), r.x0 + 8.0 * s + x, y, &r, color);
        }
    }
    let (Some(r), Some(l)) = (b.list, &ed.complete.list) else { return };
    frame(p, r, s, th);
    for &(i, row) in &b.rows {
        let Some(it) = l.items.get(i) else { continue };
        let clip = Rect::new(row.x0 + px, row.y0, row.x1 - px, row.y1);
        if i == l.sel {
            p.fill(clip, &r, th.on);
        } else if row.contains(mouse.0, mouse.1) {
            p.fill(clip, &r, th.hover);
        }
        let icon = Rect::new(row.x0 + 4.0 * s, row.y0, row.x0 + 30.0 * s, row.y1);
        symbol(p, it.kind, icon, ui, s, th);
        let name = it.name.replace(['\n', '\r'], " ");
        let nr = Rect::new(icon.x1 + 4.0 * s, row.y0, row.x1 - 12.0 * s, row.y1);
        p.label(&name, ui, nr, Align::Left, 0.0, th.text);
    }
    // Barra de rolagem da lista.
    let n = b.rows.len();
    if n > 0 && l.items.len() > n {
        let (y0, y1) = (b.rows[0].1.y0, b.rows[n - 1].1.y1);
        let h = ((y1 - y0) * n as f32 / l.items.len() as f32).max(16.0 * s).min(y1 - y0);
        let k = b.rows[0].0 as f32 / (l.items.len() - n) as f32;
        let y = y0 + (y1 - y0 - h) * k.clamp(0.0, 1.0);
        p.fill(Rect::new(r.x1 - 7.0 * s, y, r.x1 - 3.0 * s, y + h), &all, th.thumb);
    }
    // Rodapé: o que é o item escolhido e as teclas.
    let y = b.rows.last().map_or(r.y0, |x| x.1.y1) + 4.0 * s;
    p.fill(Rect::new(r.x0 + 8.0 * s, y, r.x1 - 8.0 * s, y + px), &all, th.line);
    let line = |k: f32| Rect::new(r.x0, y + 2.0 * s + k * 20.0 * s, r.x1, y + 2.0 * s + (k + 1.0) * 20.0 * s);
    if let Some(it) = l.items.get(l.sel) {
        p.label(&it.help, small, line(0.0), Align::Left, 10.0 * s, th.text);
    }
    p.label(KEYS, ui.with_px((11.0 * s).round()), line(1.0), Align::Left, 10.0 * s, th.muted);
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_ui::TextInput;

    fn at_end(t: &str) -> Analysis {
        analyze(t, t.len())
    }

    fn token(t: &str) -> Option<&str> {
        at_end(t).token.filter(|k| k.what == What::Name).map(|k| &t[k.range])
    }

    fn call(t: &str) -> Option<(&'static str, usize)> {
        at_end(t).call.map(|c| (funcs::def(c.func).pt, c.arg))
    }

    #[test]
    fn names_after_operators() {
        assert_eq!(token("=so"), Some("so"));
        assert_eq!(token("=A1+so"), Some("so"));
        assert_eq!(token("=A1 + so"), Some("so"));
        assert_eq!(token("=SOMA(me"), Some("me"));
        assert_eq!(token("=SOMA(A1;cont.s"), Some("cont.s"));
        assert_eq!(token("=-@un"), Some("un"));
        assert_eq!(token("=A1>=má"), Some("má"));
        assert_eq!(token("=\"a;b\"&so"), Some("so"));
        assert_eq!(token("='Aba (1)'!A1+so"), Some("so"));
        assert_eq!(token("=A1#/so"), Some("so"));
    }

    #[test]
    fn nothing_to_suggest() {
        // Sem `=`, só o `=`, depois de operador, números e referências.
        for t in ["so", "", "=", "=A1+", "=SOMA(", "=1", "=1,5", "=1E", "=$A", "=A$1", "=SOMA(A1;"] {
            assert_eq!(token(t), None, "{t}");
        }
        // Texto entre aspas, nome de aba, intervalo, erro, fim de referência.
        for t in ["=\"so", "=\"a\"\"so", "='Aba so", "=A1:B", "=Aba!so", "=#N/D", "=T[Col]so", "=(A1)so"] {
            assert_eq!(token(t), None, "{t}");
        }
        // No meio de uma palavra.
        assert_eq!(analyze("=soma(A1)", 3).token, None);
        assert!(analyze("=so+A1", 3).token.is_some());
        assert!(at_end("=").start && !at_end("=s").start && !at_end("").start);
    }

    #[test]
    fn call_and_argument() {
        assert_eq!(call("=SOMA("), Some(("SOMA", 0)));
        assert_eq!(call("=se(A1>0;"), Some(("SE", 1)));
        assert_eq!(call("=SE(A1>0;SOMA(A1;A2"), Some(("SOMA", 1)));
        assert_eq!(call("=SE(A1>0;SOMA(A1;A2);"), Some(("SE", 2)));
        // Parênteses de agrupamento, matriz, texto e tabela não contam.
        assert_eq!(call("=SE((A1+1"), Some(("SE", 0)));
        assert_eq!(call("=SOMA({1;2;3};"), Some(("SOMA", 1)));
        assert_eq!(call("=SE(\"a;(b\";"), Some(("SE", 1)));
        assert_eq!(call("=SE(T[a;b];"), Some(("SE", 1)));
        assert_eq!(call("=SE(\"dentro"), Some(("SE", 0)));
        assert_eq!(call("=SOMA(A1)"), None);
        // Em inglês também, e nome desconhecido não tem assinatura.
        assert_eq!(call("=VLOOKUP(A1;"), Some(("PROCV", 1)));
        assert_eq!(call("=XPTO(A1;"), None);
        // O cursor manda, não o fim do texto.
        let t = "=SE(A1;B1;C1)";
        assert_eq!(analyze(t, 8).call.map(|c| c.arg), Some(1));
        assert_eq!(analyze(t, t.len()).call, None);
    }

    #[test]
    fn signature_marks_current_argument() {
        let sig = |name: &str, arg: usize| {
            let call = Call { func: funcs::lookup_pt(name).unwrap(), arg };
            let parts = signature(call);
            let on: Vec<String> = parts.iter().filter(|p| p.1).map(|p| p.0.clone()).collect();
            (parts.iter().map(|p| p.0.as_str()).collect::<String>(), on.join(""))
        };
        assert_eq!(
            sig("SE", 1),
            (
                "SE(teste_lógico; [valor_se_verdadeiro]; [valor_se_falso])".into(),
                "[valor_se_verdadeiro]".into()
            )
        );
        assert_eq!(sig("SOMA", 0).1, "núm1");
        // Depois dos que aparecem: o "…".
        assert_eq!(sig("SOMA", 9), ("SOMA(núm1; [núm2]; …)".into(), "…".into()));
        // Argumentos demais: nenhum marcado.
        assert_eq!(sig("ABS", 3).1, "");
        assert_eq!(sig("HOJE", 0).0, "HOJE()");
    }

    fn source() -> Source {
        Source::new(
            [("Taxa".to_string(), "Dados!$A$1".to_string()), ("Índice_Anual".into(), "1".into())],
            [(
                "Vendas".to_string(),
                vec!["Região".into(), "Valor".into(), "Valor total".into(), "Nº #".into()],
            )],
            Some(0),
        )
    }

    fn found(typed: &str) -> Vec<String> {
        source().find(&What::Name, typed).into_iter().map(|i| i.name).collect()
    }

    #[test]
    fn finds_without_case_or_accents() {
        assert_eq!(found("unico")[0], "ÚNICO");
        assert_eq!(found("ÚNIco")[0], "ÚNICO");
        assert_eq!(found("media")[..3], ["MÉDIA", "MÉDIAA", "MÉDIASE"]);
        assert!(found("so").starts_with(&["SOMA".into(), "SOMARPRODUTO".into(), "SOMASE".into()]));
        // Nome definido, tabela e nome com acento.
        assert_eq!(found("tax"), ["Taxa"]);
        assert_eq!(found("vend"), ["Vendas"]);
        assert_eq!(found("ind")[..3], ["ÍNDICE", "Índice_Anual", "INDIRETO"]);
        assert!(found("indice").contains(&"Índice_Anual".to_string()));
        // Parte depois do ponto, sem o ponto e o nome em inglês.
        assert!(found("vazio").contains(&"CONTAR.VAZIO".to_string()));
        assert_eq!(found("contse")[0], "CONT.SE");
        assert_eq!(found("vlook"), ["PROCV"]);
        // Já escrito por inteiro: o Tab volta a confirmar.
        assert!(found("taxa").is_empty() && found("VENDAS").is_empty());
        // Uma referência não vira sugestão frouxa.
        assert!(found("B2").is_empty());
        assert!(found("").is_empty());
    }

    #[test]
    fn only_computed_functions() {
        let names: Vec<&str> = functions().iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names.len(), funcs::all().iter().filter(|d| !d.opaque).count());
        assert!(names.contains(&"FILTRO") && !names.contains(&"LAMBDA") && !names.contains(&"PGTO"));
    }

    #[test]
    fn table_columns() {
        let cols = |t: &str| -> Vec<String> {
            let k = at_end(t).token.unwrap();
            source().find(&k.what, &t[k.range]).into_iter().map(|i| i.name).collect()
        };
        assert_eq!(cols("=SOMA(Vendas["), ["Região", "Valor", "Valor total", "Nº #"]);
        assert_eq!(cols("=SOMA(vendas[val"), ["Valor", "Valor total"]);
        assert_eq!(cols("=Vendas[regiao"), ["Região"]);
        assert_eq!(cols("=Vendas[[#Data],[to"), ["Valor total"]);
        assert_eq!(cols("=Vendas[@va"), ["Valor", "Valor total"]);
        // Sem o nome: a tabela da célula.
        assert_eq!(cols("=[@re"), ["Região"]);
        assert!(
            source().find(&What::Column { table: "Outra".into(), row: false, nested: false }, "").is_empty()
        );
        assert_eq!(at_end("=Vendas[#To").token, None);
        assert_eq!(at_end("=Vendas[[#Data],").token, None);
    }

    /// Digita `t`, aceita o item `name` da lista e devolve o texto e o cursor.
    fn accepted(t: &str, cursor: usize, name: &str) -> (String, usize) {
        let mut input = TextInput::new();
        input.set_text(t);
        input.set_cursor(cursor);
        let mut ed = Editor {
            row: 0,
            col: 0,
            input,
            enter_mode: true,
            in_bar: false,
            sheet: 0,
            away: false,
            point: None,
            complete: State::default(),
        };
        ed.complete.update(t, cursor, true, source);
        let i = ed.complete.list.as_ref().unwrap().items.iter().position(|x| x.name == name).unwrap();
        assert!(accept(&mut ed, i));
        assert!(ed.complete.list.is_none());
        (ed.input.text.clone(), ed.input.cursor())
    }

    #[test]
    fn accepting() {
        let end = |t: &str, name: &str| {
            let (text, cur) = accepted(t, t.len(), name);
            assert_eq!(cur, text.len());
            text
        };
        assert_eq!(end("=so", "SOMA"), "=SOMA(");
        assert_eq!(end("=A1+unico", "ÚNICO"), "=A1+ÚNICO(");
        assert_eq!(end("=SE(ver", "VERDADEIRO"), "=SE(VERDADEIRO");
        assert_eq!(end("=2*tax", "Taxa"), "=2*Taxa");
        assert_eq!(end("=SOMA(ven", "Vendas"), "=SOMA(Vendas");
        assert_eq!(end("=SOMA(Vendas[val", "Valor"), "=SOMA(Vendas[Valor]");
        assert_eq!(end("=SOMA(Vendas[", "Nº #"), "=SOMA(Vendas[Nº '#]");
        assert_eq!(end("=Vendas[@val", "Valor"), "=Vendas[@Valor]");
        assert_eq!(end("=Vendas[@val", "Valor total"), "=Vendas[@[Valor total]]");
        assert_eq!(end("=Vendas[@[val", "Valor total"), "=Vendas[@[Valor total]]");
        assert_eq!(end("=Vendas[[#Data],[val", "Valor"), "=Vendas[[#Data],[Valor]");
        // No meio do texto: o resto fica, e parêntese e colchete não se repetem.
        assert_eq!(accepted("=so+A1", 3, "SOMA"), ("=SOMA(+A1".into(), 6));
        assert_eq!(accepted("=so(A1)", 3, "SOMA"), ("=SOMA(A1)".into(), 6));
        assert_eq!(accepted("=SOMA(Vendas[val])", 16, "Valor"), ("=SOMA(Vendas[Valor])".into(), 18));
    }

    #[test]
    fn list_follows_typing() {
        let mut st = State::default();
        st.update("=s", 2, true, source);
        assert!(st.list.is_some());
        // Esc fecha e a lista fica fechada até outra palavra.
        st.dismiss();
        st.update("=so", 3, true, source);
        assert!(st.list.is_none());
        st.update("=so+m", 5, true, source);
        assert!(st.list.is_some());
        // Mover o cursor fecha a lista mas mantém a assinatura.
        st.update("=SOMA(m", 7, false, source);
        assert!(st.list.is_none() && st.call.is_some());
        // Só o `=`: a dica, sem lista.
        st.update("=", 1, true, source);
        assert!(st.start && st.list.is_none());
    }

    #[test]
    fn selection_scrolls() {
        let mut st = State::default();
        st.update("=c", 2, true, source);
        let l = st.list.as_mut().unwrap();
        assert!(l.items.len() > ROWS);
        l.select(ROWS);
        assert_eq!((l.sel, l.top), (ROWS, 1));
        l.select(0);
        assert_eq!((l.sel, l.top), (0, 0));
        l.select(usize::MAX - 1);
        assert_eq!(l.sel, l.items.len() - 1);
        l.scroll(-1000);
        assert_eq!(l.top, 0);
    }

    #[test]
    fn boxes_stay_inside_the_window() {
        let win = Rect::new(0.0, 0.0, 800.0, 600.0);
        let inside = |r: Rect| r.x0 >= win.x0 && r.y0 >= win.y0 && r.x1 <= win.x1 && r.y1 <= win.y1;
        for (x, y) in
            [(10.0, 100.0), (790.0, 100.0), (10.0, 590.0), (790.0, 590.0), (400.0, 300.0), (-50.0, 0.0)]
        {
            let anchor = Rect::new(x, y, x + 64.0, y + 20.0);
            let (sig, list, rows) =
                place(anchor, x, Some((300.0, 30.0)), Some((40, 400.0, 24.0, 44.0)), win, 3.0);
            let (sig, list) = (sig.unwrap(), list.unwrap());
            assert!(inside(sig) && inside(list), "{x},{y}: {sig:?} {list:?}");
            assert!(rows >= 1);
            assert!(sig.intersect(&list).is_empty());
            // Não cobrem a célula editada.
            assert!(anchor.intersect(&list).is_empty() && anchor.intersect(&sig).is_empty());
        }
        // Janela pequena demais: ainda assim dentro dela.
        let tiny = Rect::new(0.0, 0.0, 200.0, 150.0);
        let (_, list, rows) =
            place(Rect::new(50.0, 70.0, 110.0, 90.0), 50.0, None, Some((40, 400.0, 24.0, 44.0)), tiny, 3.0);
        let r = list.unwrap();
        assert!(rows == 1 && r.x1 <= 200.0 && r.y1 <= 150.0 && r.y0 >= 0.0);
    }
}
