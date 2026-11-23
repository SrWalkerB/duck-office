//! Avaliação de fórmulas compiladas.
//!
//! - [`Book`]: o arquivo visto pelo motor (abas, strings, nomes, arquivos
//!   externos, "agora") mais os resultados novos ainda não gravados
//!   ([`Overlay`]): o recálculo roda sem mexer nas abas, que a janela segue
//!   mostrando, e grava tudo no fim.
//! - [`Engine`]: fórmulas compiladas (uma por definição: um modelo copiado
//!   para um milhão de linhas é compilado uma vez) e os índices de busca.
//! - [`Eval`]: avalia uma célula. Uma thread por vez; várias células em
//!   paralelo, cada uma com o seu `Eval`.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::{OnceLock, RwLock};

use duck_core::{BookExtras, CellError, CellValue, FormulaKind, Sheet, StringTable, Value};

use crate::funcs;
use crate::index::Cache;
use crate::parse::{self, Area, BinOp, Formula, MAX_COL, MAX_ROW, Node, Resolve, SheetSel, TableInfo};
use crate::value::{Arr, Rng, Sid, V, bool_text, compare, num_to_text, parse_number};

/// Resultados novos de fórmulas, por (aba, coluna) e linha.
#[derive(Default, Debug)]
pub struct Overlay {
    cols: HashMap<(u32, u32), BTreeMap<u32, CellValue>>,
}

impl Overlay {
    pub fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cols.values().map(BTreeMap::len).sum()
    }

    pub fn get(&self, sheet: u32, row: u32, col: u32) -> Option<&CellValue> {
        self.cols.get(&(sheet, col))?.get(&row)
    }

    pub fn set(&mut self, sheet: u32, row: u32, col: u32, v: CellValue) {
        self.cols.entry((sheet, col)).or_default().insert(row, v);
    }

    /// (aba, coluna) com valores novos.
    pub fn columns(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.cols.keys().copied()
    }

    pub fn col(&self, sheet: u32, col: u32) -> Option<&BTreeMap<u32, CellValue>> {
        self.cols.get(&(sheet, col))
    }

    /// Tudo, em ordem de aba, coluna e linha.
    pub fn drain(self) -> impl Iterator<Item = (u32, u32, u32, CellValue)> {
        let mut v: Vec<_> = self.cols.into_iter().collect();
        v.sort_unstable_by_key(|(k, _)| *k);
        v.into_iter().flat_map(|((s, c), m)| m.into_iter().map(move |(r, x)| (s, r, c, x)))
    }

    pub fn merge(&mut self, other: Overlay) {
        for (k, m) in other.cols {
            self.cols.entry(k).or_default().extend(m);
        }
    }
}

/// O arquivo como o motor vê.
#[derive(Clone)]
pub struct Book<'a> {
    pub sheets: Vec<&'a Sheet>,
    pub shared: &'a StringTable,
    pub extras: &'a BookExtras,
    pub date1904: bool,
    /// Data e hora atuais (serial) para `HOJE()` e `AGORA()`.
    pub now: f64,
    pub overlay: Option<&'a Overlay>,
    /// Resultados de um bloco calculado de cima para baixo (saldo acumulado),
    /// visíveis às linhas seguintes do mesmo bloco.
    pub local: Option<&'a RwLock<Overlay>>,
    /// Maior número de linhas entre as abas (colunas inteiras em contas de
    /// matriz param aqui).
    pub max_rows: u32,
}

impl<'a> Book<'a> {
    pub fn new(
        sheets: Vec<&'a Sheet>,
        shared: &'a StringTable,
        extras: &'a BookExtras,
        date1904: bool,
    ) -> Self {
        let max_rows = sheets.iter().map(|s| s.rows()).max().unwrap_or(0);
        let mut b = Self { sheets, shared, extras, date1904, now: 0.0, overlay: None, local: None, max_rows };
        b.set_now(now_serial());
        b
    }

    /// Muda o "agora" (serial, sem a época de 1904) de HOJE() e AGORA().
    pub fn set_now(&mut self, now: f64) {
        self.now = now;
        if let Some(dt) = duck_core::numfmt::serial_to_datetime(now.floor(), false, 0) {
            crate::value::set_today(dt.year, dt.month, dt.day);
        }
    }

    pub fn sheet(&self, sid: Sid) -> Option<&'a Sheet> {
        match sid {
            Sid::Local(i) => self.sheets.get(i as usize).copied(),
            Sid::Ext(b, s) => self.extras.externals.get(b as usize)?.sheets.get(s as usize),
        }
    }

    /// Linhas usadas da aba.
    pub fn used_rows(&self, sid: Sid) -> u32 {
        self.sheet(sid).map_or(0, |s| s.rows())
    }

    pub fn used_cols(&self, sid: Sid) -> u32 {
        self.sheet(sid).map_or(0, |s| s.cols())
    }

    /// Valor da célula (com os resultados novos por cima).
    #[inline]
    pub fn get(&self, sid: Sid, row: u32, col: u32) -> V<'a> {
        let Some(sheet) = self.sheet(sid) else { return V::Empty };
        let c = sheet.cell(row, col, self.shared);
        if c.formula
            && let Sid::Local(s) = sid
        {
            if let Some(l) = self.local
                && let Some(v) = l.read().unwrap().get(s, row, col)
            {
                return cell_v(v).into_owned();
            }
            if let Some(o) = self.overlay
                && let Some(v) = o.get(s, row, col)
            {
                return cell_v(v);
            }
        }
        value_v(c.value)
    }

    /// Visita as células com valor de `r`, coluna por coluna e em ordem de
    /// linha. `f` devolve `false` para parar.
    pub fn each(&self, r: &Rng, mut f: impl FnMut(u32, u32, V<'a>) -> bool) {
        let Some(sheet) = self.sheet(r.sheet) else { return };
        let last_row = r.r1.min(sheet.rows().saturating_sub(1));
        if sheet.rows() == 0 || r.r0 > last_row {
            return;
        }
        let last_col = r.c1.min(sheet.cols().saturating_sub(1));
        let local = self.local.map(|l| l.read().unwrap());
        for col in r.c0..=last_col {
            let over = match (self.overlay, r.sheet) {
                (Some(o), Sid::Local(s)) => o.col(s, col),
                _ => None,
            };
            let local_col = match (&local, r.sheet) {
                (Some(l), Sid::Local(s)) => l.col(s, col).filter(|m| !m.is_empty()),
                _ => None,
            };
            let mut stop = false;
            if over.is_none() && local_col.is_none() {
                sheet.for_each_value(col, r.r0..last_row + 1, self.shared, |row, c| {
                    stop = !f(row, col, value_v(c.value));
                    !stop
                });
            } else {
                // Valores novos por cima dos guardados; os locais (bloco de
                // cima para baixo) por cima de todos.
                let mut extra: Vec<(u32, V<'a>)> = Vec::new();
                if let Some(m) = over {
                    extra.extend(m.range(r.r0..=last_row).map(|(k, v)| (*k, cell_v(v))));
                }
                if let Some(lc) = local_col {
                    let mut add: Vec<(u32, V<'a>)> =
                        lc.range(r.r0..=last_row).map(|(k, v)| (*k, cell_v(v).into_owned())).collect();
                    extra.retain(|(k, _)| !lc.contains_key(k));
                    extra.append(&mut add);
                    extra.sort_by_key(|e| e.0);
                }
                let mut extra = extra.into_iter().peekable();
                sheet.for_each_value(col, r.r0..last_row + 1, self.shared, |row, c| {
                    while let Some((er, _)) = extra.peek() {
                        if *er >= row {
                            break;
                        }
                        let (er, ev) = extra.next().unwrap();
                        if !matches!(ev, V::Empty) && !f(er, col, ev) {
                            stop = true;
                            return false;
                        }
                    }
                    let v = match extra.peek() {
                        Some((er, _)) if *er == row => extra.next().unwrap().1,
                        _ => value_v(c.value),
                    };
                    if matches!(v, V::Empty) {
                        return true;
                    }
                    stop = !f(row, col, v);
                    !stop
                });
                if !stop {
                    for (er, ev) in extra {
                        if !matches!(ev, V::Empty) && !f(er, col, ev) {
                            stop = true;
                            break;
                        }
                    }
                }
            }
            if stop {
                return;
            }
        }
    }
}

fn value_v(v: Value<'_>) -> V<'_> {
    match v {
        Value::Empty => V::Empty,
        Value::Number(x) => V::Num(x),
        Value::Text(s) => V::Str(Cow::Borrowed(s)),
        Value::Bool(b) => V::Bool(b),
        Value::Error(e) => V::Err(e),
    }
}

fn cell_v(v: &CellValue) -> V<'_> {
    match v {
        CellValue::Empty => V::Empty,
        CellValue::Number(x) => V::Num(*x),
        CellValue::Text(s) => V::Str(Cow::Borrowed(s)),
        CellValue::Bool(b) => V::Bool(*b),
        CellValue::Error(e) => V::Err(*e),
    }
}

/// Serial de agora (hora local não é conhecida aqui: UTC).
pub fn now_serial() -> f64 {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    25569.0 + secs / 86400.0
}

impl Resolve for Book<'_> {
    fn sheet(&self, name: &str) -> Option<u32> {
        self.sheets.iter().position(|s| crate::value::text_eq(&s.name, name)).map(|i| i as u32)
    }

    fn ext_sheet(&self, book: u16, name: &str) -> Option<u16> {
        let b = self.extras.externals.get(book as usize)?;
        b.sheets.iter().position(|s| crate::value::text_eq(&s.name, name)).map(|i| i as u16)
    }

    fn name(&self, name: &str, sheet: u32) -> Option<u32> {
        let names = &self.extras.names;
        names
            .iter()
            .position(|n| n.sheet == Some(sheet) && crate::value::text_eq(&n.name, name))
            .or_else(|| names.iter().position(|n| n.sheet.is_none() && crate::value::text_eq(&n.name, name)))
            .map(|i| i as u32)
    }

    fn table(&self, name: &str) -> Option<TableInfo<'_>> {
        for (i, s) in self.sheets.iter().enumerate() {
            for t in &s.layout.tables {
                if crate::value::text_eq(&t.name, name) {
                    return Some(table_info(i as u32, t));
                }
            }
        }
        None
    }

    fn table_at(&self, sheet: u32, row: u32, col: u32) -> Option<TableInfo<'_>> {
        let s = self.sheets.get(sheet as usize)?;
        s.layout.tables.iter().find(|t| t.range.contains(row, col)).map(|t| table_info(sheet, t))
    }
}

fn table_info(sheet: u32, t: &duck_core::Table) -> TableInfo<'_> {
    TableInfo {
        sheet,
        range: t.range,
        header_rows: t.header_rows,
        totals_rows: t.totals_rows,
        columns: &t.columns,
    }
}

// ---------------------------------------------------------------------------
// Motor
// ---------------------------------------------------------------------------

/// Resultado de uma célula: um valor, ou a matriz que ela espalha.
pub enum Full {
    One(CellValue),
    Many { rows: u32, cols: u32, v: Vec<CellValue> },
}

/// Fórmulas compiladas e índices de busca de um arquivo. Feito de novo
/// quando a estrutura muda (linhas/colunas inseridas, abas renomeadas).
pub struct Engine {
    defs: Vec<Vec<OnceLock<Formula>>>,
    names: Vec<OnceLock<Formula>>,
    pub index: Cache,
}

impl Engine {
    pub fn new(book: &Book) -> Self {
        Self {
            defs: book
                .sheets
                .iter()
                .map(|s| (0..s.formula_texts_stored()).map(|_| OnceLock::new()).collect())
                .collect(),
            names: (0..book.extras.names.len()).map(|_| OnceLock::new()).collect(),
            index: Cache::default(),
        }
    }

    /// Fórmula compilada da célula. `None` se a célula não tem fórmula. Uma
    /// definição criada depois do motor (edição) é compilada sem cache.
    pub fn formula<'e>(&'e self, book: &Book, sheet: u32, row: u32, col: u32) -> Option<Cow<'e, Formula>> {
        let s = book.sheets.get(sheet as usize)?;
        let (idx, f) = s.formula_entry(row, col)?;
        let compile = || {
            let text: Cow<str> = if f.template {
                Cow::Owned(crate::from_template(f.text, row, col))
            } else {
                Cow::Borrowed(f.text)
            };
            parse::compile(&text, sheet, f.anchor_row, f.anchor_col, book)
        };
        match self.defs.get(sheet as usize).and_then(|d| d.get(idx as usize)) {
            Some(slot) => Some(Cow::Borrowed(slot.get_or_init(compile))),
            None => Some(Cow::Owned(compile())),
        }
    }

    pub fn name_formula<'e>(&'e self, book: &Book, i: u32) -> Option<&'e Formula> {
        let slot = self.names.get(i as usize)?;
        Some(slot.get_or_init(|| {
            let n = &book.extras.names[i as usize];
            parse::compile(&n.formula, n.sheet.unwrap_or(0), 0, 0, book)
        }))
    }

    /// Como [`Engine::eval_cell`], com a matriz inteira quando o resultado é
    /// uma matriz (fórmula que espalha, ou fórmula de matriz). `spill`: a
    /// fórmula é avaliada como no Excel atual (referências viram matrizes);
    /// sem ele, fórmulas comuns de arquivos antigos dão o primeiro valor.
    pub fn eval_full(&self, book: &Book, sheet: u32, row: u32, col: u32, spill: bool) -> Option<Full> {
        let f = self.formula(book, sheet, row, col)?;
        if f.flags & parse::flag::OPAQUE != 0 {
            return None;
        }
        let array = spill
            || matches!(
                book.sheets[sheet as usize].formula(row, col).map(|f| f.kind),
                Some(FormulaKind::Array { .. })
            );
        let mut e = Eval {
            book,
            eng: self,
            f: &f,
            sheet,
            row,
            col,
            array,
            opaque: false,
            depth: 0,
            locals: Vec::new(),
            local_base: 0,
        };
        let v = e.eval(f.root);
        let arr = match &v {
            V::Arr(a) if a.rows * a.cols > 1 => Some(a.clone()),
            V::Ref(r) if array && !r.is_cell() => Some(e.to_arr(v.clone())),
            _ => None,
        };
        if let Some(a) = arr.filter(|_| array) {
            let vals: Vec<CellValue> = a.v.iter().map(|x| e.final_value(x.clone()).to_cell()).collect();
            if e.opaque {
                return None;
            }
            return Some(Full::Many { rows: a.rows, cols: a.cols, v: vals });
        }
        let v = e.final_value(v);
        if e.opaque {
            return None;
        }
        Some(Full::One(v.to_cell()))
    }

    /// Calcula a célula. `None` = sem fórmula, ou com algo que o motor não
    /// sabe calcular (o valor guardado no arquivo fica).
    pub fn eval_cell(&self, book: &Book, sheet: u32, row: u32, col: u32) -> Option<CellValue> {
        let f = self.formula(book, sheet, row, col)?;
        if f.flags & parse::flag::OPAQUE != 0 {
            return None;
        }
        let array = matches!(
            book.sheets[sheet as usize].formula(row, col).map(|f| f.kind),
            Some(FormulaKind::Array { .. })
        );
        let mut e = Eval {
            book,
            eng: self,
            f: &f,
            sheet,
            row,
            col,
            array,
            opaque: false,
            depth: 0,
            locals: Vec::new(),
            local_base: 0,
        };
        let v = e.eval(f.root);
        let v = e.final_value(v);
        if e.opaque {
            return None;
        }
        Some(v.to_cell())
    }
}

// ---------------------------------------------------------------------------
// Avaliação
// ---------------------------------------------------------------------------

pub type Res<T> = Result<T, CellError>;

pub struct Eval<'e> {
    pub book: &'e Book<'e>,
    pub eng: &'e Engine,
    pub f: &'e Formula,
    pub sheet: u32,
    pub row: u32,
    pub col: u32,
    /// Fórmula matricial (ou dentro de SOMARPRODUTO): operações elemento a elemento.
    pub array: bool,
    /// Passou por algo que o motor não calcula: o valor guardado fica.
    pub opaque: bool,
    depth: u32,
    /// Variáveis de LET (pilha) e o início das da fórmula atual (nomes
    /// definidos têm as próprias).
    pub locals: Vec<V<'e>>,
    local_base: usize,
}

impl<'e> Eval<'e> {
    pub fn new(book: &'e Book<'e>, eng: &'e Engine, f: &'e Formula, sheet: u32, row: u32, col: u32) -> Self {
        Self {
            book,
            eng,
            f,
            sheet,
            row,
            col,
            array: false,
            opaque: false,
            depth: 0,
            locals: Vec::new(),
            local_base: 0,
        }
    }

    pub fn eval(&mut self, n: u32) -> V<'e> {
        let f = self.f;
        match f.node(n) {
            Node::Num(x) => V::Num(*x),
            Node::Str(s) => V::Str(Cow::Borrowed(s)),
            Node::Bool(b) => V::Bool(*b),
            Node::Err(e) => V::Err(*e),
            Node::Missing => V::Empty,
            Node::Area(a) => match self.area(a) {
                Some(r) => V::Ref(r),
                None => V::Err(CellError::Ref),
            },
            Node::Neg(x) => {
                let v = self.eval(*x);
                self.unary(v, |x| -x)
            }
            Node::Pct(x) => {
                let v = self.eval(*x);
                self.unary(v, |x| x / 100.0)
            }
            Node::Bin(op, a, b) => {
                let (op, a, b) = (*op, *a, *b);
                let x = self.eval(a);
                let y = self.eval(b);
                self.binary(op, x, y)
            }
            Node::Call { f: func, start, len } => {
                let def = funcs::def(*func);
                let args = f.call_args(*start, *len);
                if (args.len() as u32) < def.min as u32 || args.len() as u32 > def.max as u32 {
                    return V::Err(CellError::Value);
                }
                if def.opaque {
                    self.opaque = true;
                }
                match (def.imp)(self, args) {
                    Ok(v) => v,
                    Err(e) => V::Err(e),
                }
            }
            Node::Name(i) => {
                if self.depth > 32 {
                    return V::Err(CellError::Ref);
                }
                let Some(nf) = self.eng.name_formula(self.book, *i) else { return V::Err(CellError::Name) };
                let saved = std::mem::replace(&mut self.f, nf);
                let base = std::mem::replace(&mut self.local_base, self.locals.len());
                self.depth += 1;
                let v = self.eval(nf.root);
                self.depth -= 1;
                self.local_base = base;
                self.f = saved;
                v
            }
            Node::Array { rows, cols, start } => {
                let ids = f.call_args(*start, rows * cols);
                let v = ids.iter().map(|&i| self.eval(i)).collect();
                V::Arr(Rc::new(Arr { rows: *rows, cols: *cols, v }))
            }
            Node::Local(k) => {
                self.locals.get(self.local_base + *k as usize).cloned().unwrap_or(V::Err(CellError::Name))
            }
            Node::Range(a, b) => {
                let (a, b) = (*a, *b);
                match (self.eval(a), self.eval(b)) {
                    (V::Ref(x), V::Ref(y)) if x.sheet == y.sheet => V::Ref(Rng {
                        sheet: x.sheet,
                        r0: x.r0.min(y.r0),
                        c0: x.c0.min(y.c0),
                        r1: x.r1.max(y.r1),
                        c1: x.c1.max(y.c1),
                    }),
                    (V::Err(e), _) | (_, V::Err(e)) => V::Err(e),
                    _ => V::Err(CellError::Value),
                }
            }
        }
    }

    /// Área compilada → intervalo na célula avaliada.
    pub fn area(&mut self, a: &Area) -> Option<Rng> {
        let sheet = match a.sheet {
            SheetSel::This => Sid::Local(self.sheet),
            SheetSel::Local(u32::MAX) => return None,
            SheetSel::Local(s) => Sid::Local(s),
            SheetSel::Ext { sheet: u16::MAX, .. } => {
                self.opaque = true;
                return None;
            }
            SheetSel::Ext { book, sheet } => Sid::Ext(book, sheet),
        };
        let (r0, r1) = (a.r0.at(self.row), a.r1.at(self.row));
        let (c0, c1) = (a.c0.at(self.col), a.c1.at(self.col));
        let ok = |v: i64, max: u32| (0..=max as i64).contains(&v);
        if !(ok(r0, MAX_ROW) && ok(r1, MAX_ROW) && ok(c0, MAX_COL) && ok(c1, MAX_COL)) {
            return None;
        }
        Some(Rng {
            sheet,
            r0: r0.min(r1) as u32,
            r1: r0.max(r1) as u32,
            c0: c0.min(c1) as u32,
            c1: c0.max(c1) as u32,
        })
    }

    /// Valor final da célula: referência vira o valor (interseção),
    /// matriz vira o primeiro elemento.
    pub fn final_value(&mut self, v: V<'e>) -> V<'e> {
        match v {
            V::Ref(r) => self.deref(r),
            V::Arr(a) => a.v.first().cloned().unwrap_or(V::Err(CellError::Value)),
            V::Empty => V::Num(0.0),
            v => v,
        }
    }

    /// Referência num lugar que pede um valor: a célula, ou a interseção
    /// implícita com a linha/coluna da fórmula.
    pub fn deref(&self, r: Rng) -> V<'e> {
        if r.is_cell() {
            return self.book.get(r.sheet, r.r0, r.c0);
        }
        if self.array {
            return self.book.get(r.sheet, r.r0, r.c0);
        }
        if r.c0 == r.c1 && (r.r0..=r.r1).contains(&self.row) {
            return self.book.get(r.sheet, self.row, r.c0);
        }
        if r.r0 == r.r1 && (r.c0..=r.c1).contains(&self.col) {
            return self.book.get(r.sheet, r.r0, self.col);
        }
        V::Err(CellError::Value)
    }

    /// Avalia e reduz a um valor simples.
    pub fn scalar(&mut self, n: u32) -> V<'e> {
        let v = self.eval(n);
        self.to_scalar(v)
    }

    pub fn to_scalar(&self, v: V<'e>) -> V<'e> {
        match v {
            V::Ref(r) => self.deref(r),
            V::Arr(a) => a.v.first().cloned().unwrap_or(V::Err(CellError::Value)),
            v => v,
        }
    }

    pub fn is_missing(&self, n: u32) -> bool {
        matches!(self.f.node(n), Node::Missing)
    }

    pub fn num(&mut self, n: u32) -> Res<f64> {
        let v = self.scalar(n);
        self.to_num(&v)
    }

    pub fn to_num(&self, v: &V) -> Res<f64> {
        match v {
            V::Num(x) => Ok(*x),
            V::Empty => Ok(0.0),
            V::Bool(b) => Ok(*b as u8 as f64),
            V::Err(e) => Err(*e),
            V::Str(s) => parse_number(s, self.book.date1904).ok_or(CellError::Value),
            V::Ref(r) => self.to_num(&self.deref(*r)),
            V::Arr(a) => self.to_num(a.v.first().unwrap_or(&V::Err(CellError::Value))),
        }
    }

    /// Argumento opcional numérico.
    pub fn opt_num(&mut self, args: &[u32], i: usize, default: f64) -> Res<f64> {
        match args.get(i) {
            Some(&n) if !self.is_missing(n) => self.num(n),
            _ => Ok(default),
        }
    }

    pub fn text(&mut self, n: u32) -> Res<Cow<'e, str>> {
        let v = self.scalar(n);
        to_text(v)
    }

    pub fn boolean(&mut self, n: u32) -> Res<bool> {
        let v = self.scalar(n);
        to_bool(&v)
    }

    fn unary(&mut self, v: V<'e>, op: impl Fn(f64) -> f64) -> V<'e> {
        if self.is_arrayish(&v) {
            let a = self.to_arr(v);
            let v = a.v.iter().map(|x| self.to_num(x).map_or_else(V::Err, |x| V::num(op(x)))).collect();
            return V::Arr(Rc::new(Arr { rows: a.rows, cols: a.cols, v }));
        }
        let v = self.to_scalar(v);
        match self.to_num(&v) {
            Ok(x) => V::num(op(x)),
            Err(e) => V::Err(e),
        }
    }

    pub fn is_arrayish(&self, v: &V) -> bool {
        match v {
            V::Arr(_) => true,
            V::Ref(r) => self.array && !r.is_cell(),
            _ => false,
        }
    }

    /// Referência ou matriz → matriz de valores (colunas inteiras param na
    /// última linha usada do arquivo).
    pub fn to_arr(&self, v: V<'e>) -> Rc<Arr<'e>> {
        match v {
            V::Arr(a) => a,
            V::Ref(r) => {
                let r1 = r.r1.min(r.r0.max(self.book.max_rows.saturating_sub(1)));
                let c1 = r.c1.min(r.c0.max(self.book.used_cols(r.sheet).saturating_sub(1)));
                let (rows, cols) = (r1 - r.r0 + 1, c1 - r.c0 + 1);
                let mut out = vec![V::Empty; (rows * cols) as usize];
                let rr = Rng { r1, c1, ..r };
                self.book.each(&rr, |row, col, v| {
                    out[((row - r.r0) * cols + (col - r.c0)) as usize] = v;
                    true
                });
                Rc::new(Arr { rows, cols, v: out })
            }
            v => Rc::new(Arr { rows: 1, cols: 1, v: vec![v] }),
        }
    }

    pub fn binary(&mut self, op: BinOp, x: V<'e>, y: V<'e>) -> V<'e> {
        if self.is_arrayish(&x) || self.is_arrayish(&y) {
            let a = self.to_arr(x);
            let b = self.to_arr(y);
            let rows = if a.rows == 1 {
                b.rows
            } else if b.rows == 1 {
                a.rows
            } else {
                a.rows.min(b.rows)
            };
            let cols = if a.cols == 1 {
                b.cols
            } else if b.cols == 1 {
                a.cols
            } else {
                a.cols.min(b.cols)
            };
            let mut v = Vec::with_capacity((rows * cols) as usize);
            for r in 0..rows {
                for c in 0..cols {
                    v.push(self.scalar_op(op, a.get(r, c).clone(), b.get(r, c).clone()));
                }
            }
            return V::Arr(Rc::new(Arr { rows, cols, v }));
        }
        let x = self.to_scalar(x);
        let y = self.to_scalar(y);
        self.scalar_op(op, x, y)
    }

    pub fn scalar_op(&self, op: BinOp, x: V<'e>, y: V<'e>) -> V<'e> {
        if let V::Err(e) = x {
            return V::Err(e);
        }
        if let V::Err(e) = y {
            return V::Err(e);
        }
        let num2 = |s: &Self| -> Res<(f64, f64)> { Ok((s.to_num(&x)?, s.to_num(&y)?)) };
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => {
                let (a, b) = match num2(self) {
                    Ok(p) => p,
                    Err(e) => return V::Err(e),
                };
                match op {
                    BinOp::Add => V::num(round_add(a, b, a + b)),
                    BinOp::Sub => V::num(round_add(a, -b, a - b)),
                    BinOp::Mul => V::num(a * b),
                    BinOp::Div => {
                        if b == 0.0 {
                            V::Err(CellError::Div0)
                        } else {
                            V::num(a / b)
                        }
                    }
                    _ => {
                        if a == 0.0 && b == 0.0 {
                            V::Err(CellError::Num)
                        } else if a == 0.0 && b < 0.0 {
                            V::Err(CellError::Div0)
                        } else {
                            V::num(a.powf(b))
                        }
                    }
                }
            }
            BinOp::Cat => {
                let a = match to_text(x) {
                    Ok(s) => s,
                    Err(e) => return V::Err(e),
                };
                let b = match to_text(y) {
                    Ok(s) => s,
                    Err(e) => return V::Err(e),
                };
                if a.is_empty() {
                    return V::Str(b);
                }
                if b.is_empty() {
                    return V::Str(a);
                }
                let mut s = a.into_owned();
                s.push_str(&b);
                V::Str(Cow::Owned(s))
            }
            _ => {
                let o = compare(&x, &y);
                V::Bool(match op {
                    BinOp::Eq => o.is_eq(),
                    BinOp::Ne => o.is_ne(),
                    BinOp::Lt => o.is_lt(),
                    BinOp::Le => o.is_le(),
                    BinOp::Gt => o.is_gt(),
                    _ => o.is_ge(),
                })
            }
        }
    }

    /// Valor em `(r, c)` (deslocamento) de uma referência ou matriz.
    pub fn at(&self, v: &V<'e>, r: u32, c: u32) -> V<'e> {
        match v {
            V::Ref(rg) => {
                if r >= rg.rows() || c >= rg.cols() {
                    return V::Err(CellError::Ref);
                }
                self.book.get(rg.sheet, rg.r0 + r, rg.c0 + c)
            }
            V::Arr(a) => {
                if r >= a.rows || c >= a.cols {
                    return V::Err(CellError::Ref);
                }
                a.get(r, c).clone()
            }
            v if r == 0 && c == 0 => v.clone(),
            _ => V::Err(CellError::Ref),
        }
    }
}

/// Soma com o "arredondamento" do Excel: resultados que diferem das parcelas
/// só no 16º algarismo (0,1+0,2-0,3) viram zero exato.
fn round_add(a: f64, b: f64, s: f64) -> f64 {
    if s != 0.0 && s.abs() < a.abs().max(b.abs()) * 1e-15 { 0.0 } else { s }
}

pub fn to_text<'e>(v: V<'e>) -> Res<Cow<'e, str>> {
    match v {
        V::Str(s) => Ok(s),
        V::Empty => Ok(Cow::Borrowed("")),
        V::Num(x) => {
            let mut s = String::new();
            num_to_text(x, &mut s);
            Ok(Cow::Owned(s))
        }
        V::Bool(b) => Ok(Cow::Borrowed(bool_text(b))),
        V::Err(e) => Err(e),
        V::Ref(_) | V::Arr(_) => Err(CellError::Value),
    }
}

pub fn to_bool(v: &V) -> Res<bool> {
    match v {
        V::Bool(b) => Ok(*b),
        V::Num(x) => Ok(*x != 0.0),
        V::Empty => Ok(false),
        V::Err(e) => Err(*e),
        V::Str(s) => {
            if s.eq_ignore_ascii_case("TRUE") || s.eq_ignore_ascii_case("VERDADEIRO") {
                Ok(true)
            } else if s.eq_ignore_ascii_case("FALSE") || s.eq_ignore_ascii_case("FALSO") {
                Ok(false)
            } else {
                Err(CellError::Value)
            }
        }
        _ => Err(CellError::Value),
    }
}
