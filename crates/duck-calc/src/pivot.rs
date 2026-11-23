//! Motor das tabelas dinâmicas (F5).
//!
//! A partir do cache (registros) e da definição (campos nas linhas, colunas
//! e filtros; itens escondidos; layout), calcula:
//!
//! - as linhas e colunas do resultado no mesmo formato que o Excel grava
//!   (`rowItems`/`colItems`: `r`, `x`, `t`) — o que permite conferir o
//!   layout com o arquivo;
//! - os valores (contagem, soma, média, mínimo, máximo...) de cada
//!   cruzamento, em uma passada pelos registros: cada registro soma em todos
//!   os prefixos dos caminhos de linha e coluna;
//! - as células a escrever na aba (rótulos e valores).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use duck_core::CellValue;
use duck_core::pivot::{
    AxisItem, BASE_NEXT, BASE_PREVIOUS, PItem, PivotCache, PivotFilter, PivotTable, Records, SortType,
};

use crate::value::text_cmp;

/// Acumulador de um campo de dados.
#[derive(Clone, Copy, Debug, Default)]
pub struct Acc {
    pub count: u64,
    pub nums: u64,
    pub sum: f64,
    pub sumsq: f64,
    pub min: f64,
    pub max: f64,
    pub product: f64,
}

impl Acc {
    fn add(&mut self, v: &PItem) {
        if matches!(v, PItem::Missing) {
            return;
        }
        self.count += 1;
        let x = match v {
            PItem::Num(x) | PItem::Date(x) => *x,
            _ => return,
        };
        if self.nums == 0 {
            self.min = x;
            self.max = x;
            self.product = 1.0;
        }
        self.nums += 1;
        self.sum += x;
        self.sumsq += x * x;
        self.min = self.min.min(x);
        self.max = self.max.max(x);
        self.product *= x;
    }

    /// Valor pela função do campo de dados. `None` = célula vazia.
    pub fn value(&self, func: &str) -> Option<f64> {
        let n = self.nums as f64;
        match func {
            "count" => (self.count > 0).then_some(self.count as f64),
            "countNums" => (self.nums > 0).then_some(n),
            _ if self.count == 0 => None,
            "average" => (self.nums > 0).then(|| self.sum / n),
            "max" => (self.nums > 0).then_some(self.max),
            "min" => (self.nums > 0).then_some(self.min),
            "product" => (self.nums > 0).then_some(self.product),
            "stdDev" | "var" if self.nums > 1 => {
                let v = (self.sumsq - self.sum * self.sum / n) / (n - 1.0);
                Some(if func == "var" { v } else { v.max(0.0).sqrt() })
            }
            "stdDevp" | "varp" if self.nums > 0 => {
                let v = self.sumsq / n - (self.sum / n).powi(2);
                Some(if func == "varp" { v } else { v.max(0.0).sqrt() })
            }
            "stdDev" | "var" | "stdDevp" | "varp" => None,
            _ => Some(self.sum),
        }
    }
}

/// Resultado do cálculo.
/// Valores prontos por (linha, coluna, campo de dados).
pub type CubeValues = HashMap<(Vec<u32>, Vec<u32>, u32), CellValue>;

#[derive(Default)]
pub struct PivotResult {
    pub row_items: Vec<AxisItem>,
    pub col_items: Vec<AxisItem>,
    /// Caminho (posições nos itens dos campos de linha) de cada linha, e o
    /// mesmo para as colunas: para achar os valores.
    pub row_paths: Vec<Vec<u32>>,
    pub col_paths: Vec<Vec<u32>>,
    /// Acumuladores por (caminho da linha, caminho da coluna).
    pub cells: HashMap<(Vec<u32>, Vec<u32>), Vec<Acc>>,
    /// Registros que passaram nos filtros.
    pub records: usize,
    /// Registros barrados pelos filtros e sem item no campo de um eixo.
    pub filtered: usize,
    pub no_item: usize,
    /// Campos calculados: fórmula compilada de cada campo de dados (se for
    /// calculado) e os campos somados à parte para ela (depois dos campos
    /// de dados em cada acumulador).
    pub calc: Vec<Option<Expr>>,
    pub aux: Vec<u32>,
    /// Ordem de exibição dos itens por (eixo: 0 linhas, 1 colunas; caminho do pai).
    pub order: HashMap<(u8, Vec<u32>), Vec<u32>>,
    /// Modelo de Dados: valores prontos por (linha, coluna, campo de dados).
    pub cube: Option<CubeValues>,
}

/// Fórmula de campo calculado: aritmética sobre nomes de campos (que valem
/// a soma do campo na célula, como no Excel).
#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64),
    /// Campo (índice em `PivotResult::aux`).
    Field(usize),
    Neg(Box<Expr>),
    Bin(u8, Box<Expr>, Box<Expr>),
}

impl Expr {
    fn eval(&self, sums: &[f64]) -> Result<f64, ()> {
        Ok(match self {
            Expr::Num(x) => *x,
            Expr::Field(i) => sums.get(*i).copied().unwrap_or(0.0),
            Expr::Neg(e) => -e.eval(sums)?,
            Expr::Bin(op, a, b) => {
                let (x, y) = (a.eval(sums)?, b.eval(sums)?);
                match op {
                    b'+' => x + y,
                    b'-' => x - y,
                    b'*' => x * y,
                    b'/' => {
                        if y == 0.0 {
                            return Err(());
                        }
                        x / y
                    }
                    _ => x.powf(y),
                }
            }
        })
    }
}

/// Compila a fórmula de um campo calculado; nomes viram índices em `aux`.
fn parse_calc(f: &str, cache: &PivotCache, aux: &mut Vec<u32>) -> Option<Expr> {
    struct P<'a> {
        s: &'a [char],
        i: usize,
    }
    fn ws(p: &mut P) {
        while p.i < p.s.len() && p.s[p.i].is_whitespace() {
            p.i += 1;
        }
    }
    fn expr(p: &mut P, cache: &PivotCache, aux: &mut Vec<u32>) -> Option<Expr> {
        let mut l = term(p, cache, aux)?;
        loop {
            ws(p);
            let Some(&c) = p.s.get(p.i) else { return Some(l) };
            if c != '+' && c != '-' {
                return Some(l);
            }
            p.i += 1;
            let r = term(p, cache, aux)?;
            l = Expr::Bin(c as u8, Box::new(l), Box::new(r));
        }
    }
    fn term(p: &mut P, cache: &PivotCache, aux: &mut Vec<u32>) -> Option<Expr> {
        let mut l = pow(p, cache, aux)?;
        loop {
            ws(p);
            let Some(&c) = p.s.get(p.i) else { return Some(l) };
            if c != '*' && c != '/' {
                return Some(l);
            }
            p.i += 1;
            let r = pow(p, cache, aux)?;
            l = Expr::Bin(c as u8, Box::new(l), Box::new(r));
        }
    }
    fn pow(p: &mut P, cache: &PivotCache, aux: &mut Vec<u32>) -> Option<Expr> {
        let mut l = unary(p, cache, aux)?;
        loop {
            ws(p);
            if p.s.get(p.i) != Some(&'^') {
                return Some(l);
            }
            p.i += 1;
            let r = unary(p, cache, aux)?;
            l = Expr::Bin(b'^', Box::new(l), Box::new(r));
        }
    }
    fn unary(p: &mut P, cache: &PivotCache, aux: &mut Vec<u32>) -> Option<Expr> {
        ws(p);
        match p.s.get(p.i)? {
            '-' => {
                p.i += 1;
                Some(Expr::Neg(Box::new(unary(p, cache, aux)?)))
            }
            '+' => {
                p.i += 1;
                unary(p, cache, aux)
            }
            '(' => {
                p.i += 1;
                let e = expr(p, cache, aux)?;
                ws(p);
                if p.s.get(p.i) != Some(&')') {
                    return None;
                }
                p.i += 1;
                Some(e)
            }
            c if c.is_ascii_digit() || *c == '.' => {
                let st = p.i;
                while p.i < p.s.len()
                    && (p.s[p.i].is_ascii_digit() || p.s[p.i] == '.' || p.s[p.i] == 'E' || p.s[p.i] == 'e')
                {
                    p.i += 1;
                }
                p.s[st..p.i].iter().collect::<String>().parse().ok().map(Expr::Num)
            }
            _ => {
                // Nome entre apóstrofos ('Campo A', com '' de escape) ou sem.
                let mut name = String::new();
                if p.s[p.i] == '\'' {
                    p.i += 1;
                    while p.i < p.s.len() {
                        if p.s[p.i] == '\'' {
                            if p.s.get(p.i + 1) == Some(&'\'') {
                                name.push('\'');
                                p.i += 2;
                                continue;
                            }
                            p.i += 1;
                            break;
                        }
                        name.push(p.s[p.i]);
                        p.i += 1;
                    }
                } else {
                    while p.i < p.s.len() && (p.s[p.i].is_alphanumeric() || matches!(p.s[p.i], '_' | '.')) {
                        name.push(p.s[p.i]);
                        p.i += 1;
                    }
                }
                let f = cache.fields.iter().position(|f| crate::value::text_eq(f.name.trim(), name.trim()))?
                    as u32;
                // Outro campo calculado: entra a fórmula dele.
                if let Some(inner) = cache.fields[f as usize].formula.as_deref()
                    && p.i < 10_000
                {
                    let chars: Vec<char> = inner.trim().trim_start_matches('=').chars().collect();
                    let mut q = P { s: &chars, i: 0 };
                    let e = expr(&mut q, cache, aux)?;
                    return Some(e);
                }
                let k = match aux.iter().position(|&a| a == f) {
                    Some(k) => k,
                    None => {
                        aux.push(f);
                        aux.len() - 1
                    }
                };
                Some(Expr::Field(k))
            }
        }
    }
    let chars: Vec<char> = f.trim().trim_start_matches('=').chars().collect();
    let mut p = P { s: &chars, i: 0 };
    let e = expr(&mut p, cache, aux)?;
    ws(&mut p);
    (p.i == chars.len()).then_some(e)
}

/// Comparação de itens como o Excel ordena (números, textos, lógicos, erros, vazios).
pub fn cmp_items(a: &PItem, b: &PItem) -> Ordering {
    fn rank(p: &PItem) -> u8 {
        match p {
            PItem::Num(_) | PItem::Date(_) => 0,
            PItem::Str(_) => 1,
            PItem::Bool(_) => 2,
            PItem::Err(_) => 3,
            PItem::Missing => 4,
        }
    }
    match (a, b) {
        (PItem::Num(x) | PItem::Date(x), PItem::Num(y) | PItem::Date(y)) => x.total_cmp(y),
        (PItem::Str(x), PItem::Str(y)) => text_cmp(x, y),
        (PItem::Bool(x), PItem::Bool(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// Um campo num eixo: índice do campo (-2 = valores).
#[derive(Clone, Copy)]
struct Level {
    field: i32,
}

/// Calcula a tabela dinâmica.
/// Ordem dos itens de um campo novo num eixo: números, textos (sem diferenciar
/// maiúsculas), lógicos, erros e vazio.
fn item_order(a: &PItem, b: &PItem) -> std::cmp::Ordering {
    let rank = |x: &PItem| match x {
        PItem::Num(_) | PItem::Date(_) => 0,
        PItem::Str(_) => 1,
        PItem::Bool(_) => 2,
        PItem::Err(_) => 3,
        PItem::Missing => 4,
    };
    match (a, b) {
        (PItem::Num(x) | PItem::Date(x), PItem::Num(y) | PItem::Date(y)) => x.total_cmp(y),
        (PItem::Str(x), PItem::Str(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (PItem::Bool(x), PItem::Bool(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// Muda o eixo de um campo (`Axis::None` = tira da tabela). Um campo que
/// nunca esteve num eixo ganha os itens do cache, em ordem, e o subtotal.
pub fn set_axis(t: &mut PivotTable, cache: &PivotCache, field: usize, axis: duck_core::pivot::Axis) {
    use duck_core::pivot::{Axis, FieldItem};
    let id = field as i32;
    t.row_fields.retain(|&f| f != id);
    t.col_fields.retain(|&f| f != id);
    t.page_fields.retain(|p| p.0 != field as u32);
    let Some(f) = t.fields.get_mut(field) else { return };
    f.axis = axis;
    if axis != Axis::None
        && f.items.is_empty()
        && let Some(cf) = cache.fields.get(field)
    {
        let mut order: Vec<u32> = (0..cf.items.len() as u32).collect();
        order.sort_by(|&a, &b| item_order(&cf.items[a as usize], &cf.items[b as usize]));
        f.items = order.into_iter().map(|x| FieldItem { x: Some(x), ..Default::default() }).collect();
        if f.default_subtotal {
            f.items.push(FieldItem { t: Some("default".into()), ..Default::default() });
        }
    }
    match axis {
        Axis::Row => t.row_fields.push(id),
        Axis::Col => t.col_fields.push(id),
        Axis::Page => t.page_fields.push((field as u32, None)),
        _ => {}
    }
}

/// Colunas de rótulos de linha: uma só na forma compacta; nas outras, uma por
/// campo que não é compacto.
pub fn label_cols(t: &PivotTable) -> u32 {
    let n = t.row_fields.len();
    let extra = t
        .row_fields
        .iter()
        .take(n.saturating_sub(1))
        .filter(|&&f| f >= 0 && t.fields.get(f as usize).is_some_and(|f| !(f.compact && f.outline)));
    1 + extra.count() as u32
}

pub fn compute(t: &PivotTable, cache: &PivotCache, rec: &Records) -> PivotResult {
    let nf = t.fields.len();
    let n_data = t.data_fields.len().max(1);
    // Posição de cada item do cache na lista de itens do campo.
    let pos_of: Vec<HashMap<u32, u32>> = t
        .fields
        .iter()
        .map(|f| {
            f.items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.t.is_none())
                .filter_map(|(p, it)| it.x.map(|x| (x, p as u32)))
                .collect()
        })
        .collect();
    let item_pos = |f: usize, r: usize| -> Option<u32> {
        let x = *rec.cols.get(f)?.get(r)?;
        pos_of.get(f)?.get(&x).copied()
    };
    // Filtros: itens escondidos em qualquer campo e o item escolhido dos filtros de página.
    let mut filters: Vec<(usize, Option<u32>)> = Vec::new();
    for f in 0..nf.min(rec.cols.len()) {
        if rec.cols[f].is_empty() {
            continue;
        }
        let page_item = t.page_fields.iter().find(|p| p.0 as usize == f).and_then(|p| p.1);
        let any_hidden = t.fields[f].items.iter().any(|i| i.hidden);
        if any_hidden || page_item.is_some() {
            filters.push((f, page_item));
        }
    }
    // Filtros de rótulo e de data: decididos por item do cache, aplicados a
    // cada registro (o campo nem precisa estar num eixo).
    let item_filters: Vec<(&[u32], Vec<bool>)> = t
        .filters
        .iter()
        .filter(|f| !f.by_value())
        .filter_map(|f| item_filter(t, cache, rec, f).map(|(c, p, _)| (c, p)))
        .collect();
    // Posições barradas por esses filtros nos campos dos eixos (não aparecem
    // nem com "mostrar itens sem dados").
    let mut blocked: HashMap<usize, HashSet<u32>> = HashMap::new();
    for f in t.filters.iter().filter(|f| !f.by_value()) {
        if let Some((_, pass, false)) = item_filter(t, cache, rec, f) {
            let fld = f.fld as usize;
            for (&x, &p) in pos_of.get(fld).into_iter().flatten() {
                if !pass.get(x as usize).copied().unwrap_or(true) {
                    blocked.entry(fld).or_default().insert(p);
                }
            }
        }
    }
    let rows: Vec<Level> = t.row_fields.iter().map(|&f| Level { field: f }).collect();
    let cols: Vec<Level> = t.col_fields.iter().map(|&f| Level { field: f }).collect();
    // Caminho de um registro num eixo (o campo de valores não entra no caminho).
    let path = |levels: &[Level], r: usize| -> Option<Vec<u32>> {
        let mut p = Vec::with_capacity(levels.len());
        for l in levels {
            if l.field < 0 {
                continue;
            }
            p.push(item_pos(l.field as usize, r)?);
        }
        Some(p)
    };
    // Valor de cada campo de dados num registro.
    let data_vals = |r: usize| -> Vec<PItem> {
        t.data_fields
            .iter()
            .map(|d| {
                let f = d.fld as usize;
                match (rec.cols.get(f).and_then(|c| c.get(r)), cache.fields.get(f)) {
                    (Some(&x), Some(cf)) => cf
                        .raw_items()
                        .get(x as usize)
                        .or(cf.items.get(x as usize))
                        .cloned()
                        .unwrap_or(PItem::Missing),
                    _ => PItem::Missing,
                }
            })
            .collect()
    };
    // Campos calculados: fórmulas e os campos que elas somam.
    let mut aux: Vec<u32> = Vec::new();
    let calc: Vec<Option<Expr>> = t
        .data_fields
        .iter()
        .map(|d| {
            cache
                .fields
                .get(d.fld as usize)
                .and_then(|f| f.formula.as_deref())
                .and_then(|f| parse_calc(f, cache, &mut aux))
        })
        .collect();
    let width = n_data + aux.len();
    let aux_vals = |r: usize| -> Vec<PItem> {
        aux.iter()
            .map(|&f| {
                let f = f as usize;
                match (rec.cols.get(f).and_then(|c| c.get(r)), cache.fields.get(f)) {
                    (Some(&x), Some(cf)) => cf.raw_items().get(x as usize).cloned().unwrap_or(PItem::Missing),
                    _ => PItem::Missing,
                }
            })
            .collect()
    };
    // Uma passada pelos registros; `excl` = caminhos tirados pelos filtros de valor.
    let run = |excl: &[HashSet<Vec<u32>>; 2]| -> (Cells, usize, usize, usize) {
        let mut cells: Cells = HashMap::new();
        let (mut passed, mut filtered, mut no_item) = (0, 0, 0);
        'rec: for r in 0..rec.count {
            for &(f, page) in &filters {
                let Some(p) = item_pos(f, r) else { continue };
                if t.fields[f].items[p as usize].hidden || page.is_some_and(|i| i != p) {
                    filtered += 1;
                    continue 'rec;
                }
            }
            for (col, pass) in &item_filters {
                if col.get(r).is_some_and(|&x| !pass.get(x as usize).copied().unwrap_or(true)) {
                    filtered += 1;
                    continue 'rec;
                }
            }
            let (Some(rp), Some(cp)) = (path(&rows, r), path(&cols, r)) else {
                no_item += 1;
                continue;
            };
            let cut = |set: &HashSet<Vec<u32>>, p: &[u32]| (1..=p.len()).any(|l| set.contains(&p[..l]));
            if !excl[0].is_empty() && cut(&excl[0], &rp) || !excl[1].is_empty() && cut(&excl[1], &cp) {
                filtered += 1;
                continue;
            }
            passed += 1;
            let mut vals = data_vals(r);
            vals.extend(aux_vals(r));
            for i in 0..=rp.len() {
                for j in 0..=cp.len() {
                    let e = cells
                        .entry((rp[..i].to_vec(), cp[..j].to_vec()))
                        .or_insert_with(|| vec![Acc::default(); width]);
                    for (k, v) in vals.iter().enumerate() {
                        e[k].add(v);
                    }
                }
            }
        }
        (cells, passed, filtered, no_item)
    };
    let mut excl: [HashSet<Vec<u32>>; 2] = [HashSet::new(), HashSet::new()];
    let (mut cells, mut passed, mut filtered, mut no_item) = run(&excl);
    // Filtros de valor, na ordem do arquivo: cada um olha o resultado dos anteriores.
    for vf in t.filters.iter().filter(|f| f.by_value()) {
        let real = |ls: &[Level]| ls.iter().filter(|l| l.field >= 0).position(|l| l.field == vf.fld as i32);
        let (axis, level) = match (real(&rows), real(&cols)) {
            (Some(l), _) => (0, l),
            (None, Some(l)) => (1, l),
            _ => continue,
        };
        let before = excl[axis].len();
        value_filter(t, &calc, &cells, vf, axis, level, &mut excl[axis]);
        if excl[axis].len() != before {
            (cells, passed, filtered, no_item) = run(&excl);
        }
    }

    // Sem campo de valores e sem campos nos eixos: o Excel não mostra nada.
    if t.data_fields.is_empty() && t.row_fields.is_empty() && t.col_fields.is_empty() {
        return PivotResult {
            cube: None,
            row_items: Vec::new(),
            col_items: Vec::new(),
            row_paths: Vec::new(),
            col_paths: Vec::new(),
            cells,
            records: passed,
            filtered,
            no_item,
            calc,
            aux,
            order: HashMap::new(),
        };
    }
    let (row_items, row_paths) = axis(t, cache, &rows, &cells, true, &excl[0], &blocked);
    let (col_items, col_paths) = axis(t, cache, &cols, &cells, false, &excl[1], &blocked);
    // Ordem de exibição dos irmãos (mostrar valores como: anterior, acumulado, posição).
    let mut order: HashMap<(u8, Vec<u32>), Vec<u32>> = HashMap::new();
    for (ax, paths, items) in [(0u8, &row_paths, &row_items), (1, &col_paths, &col_items)] {
        for (p, it) in paths.iter().zip(items.iter()) {
            if it.t.is_some() {
                continue;
            }
            for l in 0..p.len() {
                let v = order.entry((ax, p[..l].to_vec())).or_default();
                if !v.contains(&p[l]) {
                    v.push(p[l]);
                }
            }
        }
    }
    PivotResult {
        cube: None,
        row_items,
        col_items,
        row_paths,
        col_paths,
        cells,
        records: passed,
        filtered,
        no_item,
        calc,
        aux,
        order,
    }
}

type Cells = HashMap<(Vec<u32>, Vec<u32>), Vec<Acc>>;

/// Tipos de filtro de data relativos a hoje: [início, fim).
fn dynamic_range(kind: &str) -> Option<(f64, f64)> {
    let (y, m, d) = crate::value::today();
    let s = duck_core::input::serial;
    let t = s(y, m, d);
    let month = |y: i32, m: i32| {
        let (yy, mm) = (y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) as u32 + 1);
        let (ny, nm) = if mm == 12 { (yy + 1, 1) } else { (yy, mm + 1) };
        (s(yy, mm, 1), s(ny, nm, 1))
    };
    let quarter = |k: i32| {
        let q0 = (m as i32 - 1) / 3 * 3 + 1 + 3 * k;
        (month(y, q0).0, month(y, q0 + 2).1)
    };
    // Semana de domingo a sábado (o serial 1 foi um domingo).
    let week = |k: f64| {
        let dow = (t - 1.0).rem_euclid(7.0);
        let a = t - dow + 7.0 * k;
        (a, a + 7.0)
    };
    Some(match kind {
        "today" => (t, t + 1.0),
        "yesterday" => (t - 1.0, t),
        "tomorrow" => (t + 1.0, t + 2.0),
        "thisWeek" => week(0.0),
        "lastWeek" => week(-1.0),
        "nextWeek" => week(1.0),
        "thisMonth" => month(y, m as i32),
        "lastMonth" => month(y, m as i32 - 1),
        "nextMonth" => month(y, m as i32 + 1),
        "thisQuarter" => quarter(0),
        "lastQuarter" => quarter(-1),
        "nextQuarter" => quarter(1),
        "thisYear" => (s(y, 1, 1), s(y + 1, 1, 1)),
        "lastYear" => (s(y - 1, 1, 1), s(y, 1, 1)),
        "nextYear" => (s(y + 1, 1, 1), s(y + 2, 1, 1)),
        "yearToDate" => (s(y, 1, 1), t + 1.0),
        _ => return None,
    })
}

/// Compara pelo operador do `customFilter`. Texto: igual/diferente aceitam curingas.
fn custom_pass(op: &str, val: &str, num: Option<f64>, text: &str) -> bool {
    let target = val.trim().parse::<f64>().ok();
    let ord = match (num, target) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        _ => text_cmp(text, val),
    };
    match op {
        "notEqual" => !eq_wild(val, num, target, text),
        "greaterThan" => ord.is_gt(),
        "greaterThanOrEqual" => ord.is_ge(),
        "lessThan" => ord.is_lt(),
        "lessThanOrEqual" => ord.is_le(),
        _ => eq_wild(val, num, target, text),
    }
}

fn eq_wild(val: &str, num: Option<f64>, target: Option<f64>, text: &str) -> bool {
    if let (Some(a), Some(b)) = (num, target) {
        return a == b;
    }
    if val.contains(['*', '?']) {
        return crate::index::Wild::new(val).matches(text);
    }
    crate::value::text_eq(text, val)
}

fn customs_pass(f: &PivotFilter, num: Option<f64>, text: &str) -> bool {
    if f.custom.is_empty() {
        return true;
    }
    let mut it = f.custom.iter().map(|(op, v)| custom_pass(op, v, num, text));
    if f.and { it.all(|b| b) } else { it.any(|b| b) }
}

fn month_of(serial: f64) -> Option<u32> {
    duck_core::numfmt::serial_to_datetime(serial, false, 0).map(|d| d.month)
}

/// Filtro de rótulo/data: qual coluna de índices dos registros olhar, se
/// cada índice passa e se os índices são os valores brutos (campo agrupado).
fn item_filter<'a>(
    t: &PivotTable,
    cache: &PivotCache,
    rec: &'a Records,
    f: &PivotFilter,
) -> Option<(&'a [u32], Vec<bool>, bool)> {
    let fld = f.fld as usize;
    let cf = cache.fields.get(fld)?;
    let quarter = f.kind.strip_prefix('Q').and_then(|q| q.parse::<u32>().ok());
    let month = f.kind.strip_prefix('M').and_then(|q| q.parse::<u32>().ok());
    let is_date = f.kind.starts_with("date")
        || dynamic_range(&f.kind).is_some()
        || quarter.is_some()
        || month.is_some();
    // Datas: o valor bruto (antes de agrupar), se o campo for agrupado.
    let (col, items, raw): (&[u32], &[PItem], bool) =
        match rec.grouped_raw.iter().find(|g| g.0 as usize == fld) {
            Some((_, raw)) if is_date => (raw.as_slice(), cf.raw.as_slice(), true),
            _ => (rec.cols.get(fld)?.as_slice(), cf.items.as_slice(), false),
        };
    if col.is_empty() {
        return None;
    }
    let names: HashMap<u32, &str> = t
        .fields
        .get(fld)
        .map(|pf| pf.items.iter().filter_map(|i| Some((i.x?, i.name.as_deref()?))).collect())
        .unwrap_or_default();
    let pass = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            if is_date {
                let (PItem::Date(d) | PItem::Num(d)) = *it else { return false };
                let d = if f.whole_day { d.floor() } else { d };
                if let Some((a, b)) = dynamic_range(&f.kind) {
                    return d >= a && d < b;
                }
                if let Some(q) = quarter {
                    return month_of(d).is_some_and(|m| (m - 1) / 3 + 1 == q);
                }
                if let Some(mm) = month {
                    return month_of(d) == Some(mm);
                }
                return customs_pass(f, Some(d), &item_text(it, false));
            }
            let text = names.get(&(i as u32)).map_or_else(|| item_text(it, false), |n| n.to_string());
            let num = match it {
                PItem::Num(x) | PItem::Date(x) => Some(*x),
                _ => None,
            };
            customs_pass(f, num, &text)
        })
        .collect();
    Some((col, pass, raw))
}

/// Valor de um campo de dados num cruzamento: `None` = vazio, `Err` = erro.
fn acc_value(t: &PivotTable, calc: &[Option<Expr>], accs: &[Acc], k: usize) -> Option<Result<f64, ()>> {
    if let Some(Some(e)) = calc.get(k) {
        let n_data = t.data_fields.len().max(1);
        let sums: Vec<f64> = accs.get(n_data..)?.iter().map(|a| a.sum).collect();
        return Some(e.eval(&sums).and_then(|x| if x.is_finite() { Ok(x) } else { Err(()) }));
    }
    let func = t.data_fields.get(k).map_or("sum", |d| d.subtotal.as_str());
    accs.get(k)?.value(func).map(Ok)
}

/// Filtro de valor (inclusive os 10 maiores) num nível de um eixo: em cada
/// grupo de irmãos, os itens que não passam vão para `excl`.
fn value_filter(
    t: &PivotTable,
    calc: &[Option<Expr>],
    cells: &Cells,
    f: &PivotFilter,
    axis: usize,
    level: usize,
    excl: &mut HashSet<Vec<u32>>,
) {
    let k = f.measure as usize;
    type Sib<'a> = (&'a [u32], Option<f64>);
    let mut groups: HashMap<&[u32], Vec<Sib>> = HashMap::new();
    for ((rp, cp), accs) in cells {
        let (p, other) = if axis == 0 { (rp, cp) } else { (cp, rp) };
        if !other.is_empty() || p.len() != level + 1 {
            continue;
        }
        let v = acc_value(t, calc, accs, k).and_then(Result::ok);
        groups.entry(&p[..level]).or_default().push((p.as_slice(), v));
    }
    for (_, mut sibs) in groups {
        let keep: Vec<bool> = match (f.kind.as_str(), f.top) {
            ("count" | "percent" | "sum", Some((top, n))) => {
                sibs.sort_by(|a, b| {
                    let (x, y) = (a.1.unwrap_or(f64::NAN), b.1.unwrap_or(f64::NAN));
                    let o = x.total_cmp(&y);
                    if top { o.reverse() } else { o }
                });
                let vals: Vec<f64> = sibs.iter().filter_map(|s| s.1).collect();
                if f.kind == "count" {
                    let n = n.max(0.0) as usize;
                    // Empates com o último entram.
                    let cut = if n == 0 { None } else { vals.get(n - 1).or(vals.last()).copied() };
                    sibs.iter()
                        .map(|s| match (s.1, cut) {
                            (Some(v), Some(c)) => {
                                if top {
                                    v >= c
                                } else {
                                    v <= c
                                }
                            }
                            _ => false,
                        })
                        .collect()
                } else {
                    let total: f64 = vals.iter().sum();
                    let goal = if f.kind == "percent" { total * n / 100.0 } else { n };
                    let mut acc = 0.0;
                    sibs.iter()
                        .map(|s| match s.1 {
                            Some(v) => {
                                let take = acc < goal;
                                acc += v;
                                take
                            }
                            None => false,
                        })
                        .collect()
                }
            }
            _ => sibs.iter().map(|s| s.1.is_some_and(|v| customs_pass(f, Some(v), ""))).collect(),
        };
        for (s, ok) in sibs.iter().zip(keep) {
            if !ok {
                excl.insert(s.0.to_vec());
            }
        }
    }
}

/// Linhas (ou colunas) do resultado.
fn axis(
    t: &PivotTable,
    cache: &PivotCache,
    levels: &[Level],
    cells: &HashMap<(Vec<u32>, Vec<u32>), Vec<Acc>>,
    rows: bool,
    excl: &HashSet<Vec<u32>>,
    blocked: &HashMap<usize, HashSet<u32>>,
) -> (Vec<AxisItem>, Vec<Vec<u32>>) {
    let real: Vec<usize> = levels.iter().filter(|l| l.field >= 0).map(|l| l.field as usize).collect();
    let values_at = levels.iter().position(|l| l.field < 0);
    let n_data = t.data_fields.len().max(1) as u32;
    let mut items = Vec::new();
    let mut paths = Vec::new();
    if real.is_empty() {
        // Sem campos: uma linha só (o total), com os campos de dados se estiverem neste eixo.
        if values_at.is_some() {
            for k in 0..n_data {
                items.push(AxisItem { t: None, r: 0, i: k, x: vec![k] });
                paths.push(Vec::new());
            }
        } else {
            items.push(AxisItem::default());
            paths.push(Vec::new());
        }
        return (items, paths);
    }
    // Filhos de um caminho num nível, na ordem de exibição.
    let has_data = |p: &[u32]| {
        if rows {
            cells.contains_key(&(p.to_vec(), Vec::new()))
        } else {
            cells.contains_key(&(Vec::new(), p.to_vec()))
        }
    };
    // Total de um caminho pelo campo de dados `k`, na linha/coluna do outro
    // eixo escolhida para a ordenação (`refs`; sem referências = o total).
    let total = |p: &[u32], k: usize, refs: &[(u32, u32)]| -> f64 {
        let other: &[i32] = if rows { &t.col_fields } else { &t.row_fields };
        let mut q = Vec::new();
        for &f in other.iter().filter(|&&f| f >= 0) {
            match refs.iter().find(|r| r.0 as i32 == f) {
                Some(r) => q.push(r.1),
                None => break,
            }
        }
        let key = if rows { (p.to_vec(), q) } else { (q, p.to_vec()) };
        cells
            .get(&key)
            .and_then(|v| v.get(k))
            .and_then(|a| a.value(t.data_fields.get(k).map_or("sum", |d| d.subtotal.as_str())))
            .unwrap_or(0.0)
    };
    // Ordem em que os itens de cada campo apareceram na última exibição do
    // Excel (desempate das ordens por valor: o Excel reordena a partir dela).
    // Por caminho completo (no grupo do pai) e, para itens novos, por campo.
    type Shown = (HashMap<Vec<u32>, usize>, Vec<HashMap<u32, usize>>);
    let (shown_path, shown_order): Shown = {
        let stored = if rows { &t.row_items } else { &t.col_items };
        let mut by_path = HashMap::new();
        let mut orders: Vec<HashMap<u32, usize>> = vec![HashMap::new(); real.len()];
        let mut path: Vec<u32> = Vec::new();
        for (row, it) in stored.iter().enumerate() {
            if it.t.is_some() {
                continue;
            }
            path.truncate(it.r as usize);
            path.extend(&it.x);
            for (lvl, &p) in path.iter().enumerate().take(real.len()) {
                let n = orders[lvl].len();
                orders[lvl].entry(p).or_insert(n);
                by_path.entry(path[..=lvl].to_vec()).or_insert(row);
            }
        }
        (by_path, orders)
    };
    let children = |level: usize, prefix: &[u32]| -> Vec<u32> {
        let f = &t.fields[real[level]];
        let cf = cache.fields.get(real[level]);
        let mut v: Vec<u32> = f
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.t.is_none() && !it.hidden)
            .map(|(p, _)| p as u32)
            .filter(|p| !blocked.get(&real[level]).is_some_and(|b| b.contains(p)))
            .filter(|&p| {
                let mut q = prefix.to_vec();
                q.push(p);
                has_data(&q) || (f.show_all && !f.items[p as usize].missing && !excl.contains(&q))
            })
            .collect();
        match (f.sort, f.sort_by_data) {
            (SortType::Manual, _) => {}
            (s, Some(k)) => {
                // Ponto de partida: a ordem da última exibição; a ordenação é estável.
                let rank = |p: &u32| {
                    let mut q = prefix.to_vec();
                    q.push(*p);
                    (
                        shown_path.get(&q).copied().unwrap_or(usize::MAX),
                        shown_order[level].get(p).copied().unwrap_or(usize::MAX),
                    )
                };
                v.sort_by_key(rank);
                v.sort_by(|a, b| {
                    let (mut pa, mut pb) = (prefix.to_vec(), prefix.to_vec());
                    pa.push(*a);
                    pb.push(*b);
                    let o =
                        total(&pa, k as usize, &f.sort_refs).total_cmp(&total(&pb, k as usize, &f.sort_refs));
                    if s == SortType::Descending { o.reverse() } else { o }
                });
            }
            (s, None) => {
                let item = |p: u32| -> PItem {
                    f.items[p as usize]
                        .x
                        .and_then(|x| cf.and_then(|c| c.items.get(x as usize)).cloned())
                        .unwrap_or(PItem::Missing)
                };
                v.sort_by(|a, b| cmp_items(&item(*a), &item(*b)));
                if s == SortType::Descending {
                    v.reverse();
                }
            }
        }
        v
    };
    // Linhas como (tipo, caminho).
    let mut out: Vec<(Option<&'static str>, Vec<u32>)> = Vec::new();
    fn walk(
        t: &PivotTable,
        real: &[usize],
        level: usize,
        prefix: &mut Vec<u32>,
        children: &dyn Fn(usize, &[u32]) -> Vec<u32>,
        out: &mut Vec<(Option<&'static str>, Vec<u32>)>,
        rows: bool,
    ) {
        let f = &t.fields[real[level]];
        let last = level + 1 == real.len();
        // Forma tabular: o primeiro filho fica na mesma linha do pai. Nas
        // colunas é sempre assim (o pai não ganha coluna própria).
        let tabular = !rows || !f.outline || !t.outline && !t.compact;
        for c in children(level, prefix) {
            prefix.push(c);
            let collapsed = f.items[c as usize].collapsed;
            if last || collapsed {
                out.push((None, prefix.clone()));
            } else {
                if !tabular {
                    out.push((None, prefix.clone()));
                }
                walk(t, real, level + 1, prefix, children, out, rows);
                let bottom = tabular || !f.subtotal_top;
                if f.default_subtotal && bottom {
                    out.push((Some("default"), prefix.clone()));
                }
            }
            prefix.pop();
        }
    }
    let mut prefix = Vec::new();
    walk(t, &real, 0, &mut prefix, &children, &mut out, rows);

    // Para AxisItem: `r` = campos repetidos da linha anterior.
    let mut prev: Vec<u32> = Vec::new();
    let data_split = values_at.is_some() && n_data > 1;
    for (kind, p) in out {
        match kind {
            None => {
                let common = prev.iter().zip(&p).take_while(|(a, b)| a == b).count();
                let r = common.min(p.len().saturating_sub(1));
                let ks: Vec<u32> = if data_split { (0..n_data).collect() } else { vec![0] };
                for k in ks {
                    // Do segundo campo de valores em diante, o item se repete.
                    let (rr, mut x) = if k == 0 { (r, p[r..].to_vec()) } else { (p.len(), Vec::new()) };
                    if data_split {
                        x.push(k);
                    }
                    items.push(AxisItem { t: None, r: rr as u32, i: k, x });
                    paths.push(p.clone());
                }
                prev = p;
            }
            Some(ty) => {
                let r = p.len() - 1;
                items.push(AxisItem { t: Some(ty.to_string()), r: r as u32, i: 0, x: vec![p[r]] });
                paths.push(p.clone());
                prev = p;
            }
        }
    }
    let grand = if rows { t.row_grand } else { t.col_grand };
    if grand {
        let ks: Vec<u32> = if data_split { (0..n_data).collect() } else { vec![0] };
        for k in ks {
            items.push(AxisItem { t: Some("grand".into()), r: 0, i: k, x: vec![0] });
            paths.push(Vec::new());
        }
    }
    (items, paths)
}

/// Texto de um item como o Excel mostra.
pub fn item_text(it: &PItem, date1904: bool) -> String {
    match it {
        PItem::Str(s) => s.clone(),
        PItem::Num(x) => {
            let mut s = String::new();
            crate::value::num_to_text(*x, &mut s);
            s
        }
        PItem::Date(x) => {
            let mut s = String::new();
            duck_core::NumFmt::parse("dd/mm/yyyy").format_number(*x, date1904, &mut s);
            s
        }
        PItem::Bool(b) => crate::value::bool_text(*b).to_string(),
        PItem::Err(e) => e.clone(),
        PItem::Missing => "(vazio)".into(),
    }
}

/// Valor da célula de dados na linha `ri` e coluna `ci` do resultado.
pub fn data_value(t: &PivotTable, r: &PivotResult, ri: usize, ci: usize) -> Option<CellValue> {
    let ritem = r.row_items.get(ri)?;
    let citem = r.col_items.get(ci)?;
    let k = if t.row_fields.contains(&-2) { ritem.i } else { citem.i } as usize;
    let (rp, cp) = (r.row_paths.get(ri)?, r.col_paths.get(ci)?);
    if let Some(cube) = &r.cube {
        // Célula vazia com legenda: o Excel guarda "-" como zero.
        return cube.get(&(rp.clone(), cp.clone(), k as u32)).cloned().or_else(|| {
            t.missing_caption.as_ref().map(|m| match m.trim() {
                "-" => CellValue::Number(0.0),
                m => CellValue::Text(m.to_string()),
            })
        });
    }
    let raw = |p: &[u32], q: &[u32]| -> Option<Result<f64, ()>> {
        let accs = r.cells.get(&(p.to_vec(), q.to_vec()))?;
        acc_value(t, &r.calc, accs, k)
    };
    // Erro com `showError`: o texto escolhido (vazio = célula vazia).
    let err = |e: duck_core::CellError| {
        if t.show_error && t.error_caption.is_empty() {
            None
        } else if t.show_error {
            Some(CellValue::Text(t.error_caption.clone()))
        } else {
            Some(CellValue::Error(e))
        }
    };
    let v = match raw(rp, cp)? {
        Ok(v) => v,
        Err(()) => return err(duck_core::CellError::Div0),
    };
    let Some(d) = t.data_fields.get(k) else { return Some(CellValue::Number(v)) };
    let Some(show) = d.show_as.as_deref() else { return Some(CellValue::Number(v)) };
    let num = |p: &[u32], q: &[u32]| raw(p, q).and_then(Result::ok);
    let div = |a: f64, b: Option<f64>| match b {
        Some(b) if b != 0.0 => Some(CellValue::Number(a / b)),
        _ => err(duck_core::CellError::Div0),
    };
    // Eixo (0 linhas, 1 colunas) e nível do campo base.
    let base = || -> Option<(usize, usize)> {
        let lvl = |fs: &[i32]| fs.iter().filter(|&&f| f >= 0).position(|&f| f == d.base_field);
        lvl(&t.row_fields).map(|l| (0, l)).or_else(|| lvl(&t.col_fields).map(|l| (1, l)))
    };
    // O par de caminhos trocando o do eixo `ax`.
    let with = |ax: usize, p: &[u32]| -> (Vec<u32>, Vec<u32>) {
        if ax == 0 { (p.to_vec(), cp.clone()) } else { (rp.clone(), p.to_vec()) }
    };
    let path_of = |ax: usize| if ax == 0 { rp } else { cp };
    match show {
        "percentOfTotal" => div(v, num(&[], &[])),
        "percentOfRow" => div(v, num(rp, &[])),
        "percentOfCol" => div(v, num(&[], cp)),
        "index" => match (num(&[], &[]), num(rp, &[]), num(&[], cp)) {
            (Some(g), Some(a), Some(b)) if a * b != 0.0 => Some(CellValue::Number(v * g / (a * b))),
            _ => err(duck_core::CellError::Div0),
        },
        "percentOfParentRow" | "percentOfParentCol" => {
            let ax = usize::from(show == "percentOfParentCol");
            let p = path_of(ax);
            if p.is_empty() {
                return Some(CellValue::Number(1.0));
            }
            let (a, b) = with(ax, &p[..p.len() - 1]);
            div(v, num(&a, &b))
        }
        "percentOfParent" => {
            let (ax, l) = base()?;
            let p = path_of(ax);
            if p.len() <= l {
                return None;
            }
            let (a, b) = with(ax, &p[..=l]);
            div(v, num(&a, &b))
        }
        "percent" | "difference" | "percentDiff" => {
            let (ax, l) = base()?;
            let p = path_of(ax);
            if p.len() <= l {
                return None;
            }
            let sibs = r.order.get(&(ax as u8, p[..l].to_vec()))?;
            let at = sibs.iter().position(|&x| x == p[l]);
            let bpos = match d.base_item {
                BASE_PREVIOUS => at.and_then(|i| i.checked_sub(1)).map_or(p[l], |i| sibs[i]),
                BASE_NEXT => at.and_then(|i| sibs.get(i + 1).copied()).unwrap_or(p[l]),
                b => b,
            };
            // O próprio item base: 100% em "% de"; vazio nas diferenças.
            if bpos == p[l] && show != "percent" {
                return None;
            }
            let mut q = p.clone();
            q[l] = bpos;
            let (a, b) = with(ax, &q);
            let bv = num(&a, &b);
            match show {
                "percent" => div(v, bv),
                "difference" => Some(CellValue::Number(v - bv.unwrap_or(0.0))),
                _ => div(v - bv.unwrap_or(0.0), bv),
            }
        }
        "runTotal" | "percentOfRunningTotal" | "rankAscending" | "rankDescending" => {
            let (ax, l) = base()?;
            let p = path_of(ax);
            if p.len() <= l {
                return None;
            }
            let sibs = r.order.get(&(ax as u8, p[..l].to_vec()))?;
            let vals: Vec<(u32, Option<f64>)> = sibs
                .iter()
                .map(|&s| {
                    let mut q = p.clone();
                    q[l] = s;
                    let (a, b) = with(ax, &q);
                    (s, num(&a, &b))
                })
                .collect();
            if show.starts_with("rank") {
                let asc = show == "rankAscending";
                let n = vals.iter().filter_map(|x| x.1).filter(|&x| if asc { x < v } else { x > v }).count();
                return Some(CellValue::Number(n as f64 + 1.0));
            }
            let upto = vals.iter().position(|x| x.0 == p[l]).unwrap_or(0);
            let run: f64 = vals[..=upto].iter().filter_map(|x| x.1).sum();
            if show == "runTotal" {
                Some(CellValue::Number(run))
            } else {
                div(run, Some(vals.iter().filter_map(|x| x.1).sum()))
            }
        }
        _ => Some(CellValue::Number(v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::CellRange;
    use duck_core::pivot::{Axis, CacheField, DataField, FieldItem, PivotField};

    fn sample() -> (PivotTable, PivotCache, Records) {
        let cache = PivotCache {
            fields: vec![
                CacheField {
                    name: "Loja".into(),
                    items: vec![PItem::Str("Norte".into()), PItem::Str("Sul".into())],
                    ..Default::default()
                },
                CacheField {
                    name: "Valor".into(),
                    items: vec![PItem::Num(10.0), PItem::Num(5.0)],
                    ..Default::default()
                },
                CacheField {
                    name: "Dobro".into(),
                    virtual_field: true,
                    formula: Some("'Valor'*2".into()),
                    ..Default::default()
                },
            ],
            record_count: 3,
            ..Default::default()
        };
        let rec = Records {
            count: 3,
            cols: vec![vec![0, 1, 0], vec![0, 1, 1], Vec::new()],
            grouped_raw: Vec::new(),
        };
        let item =
            |x: Option<u32>, t: Option<&str>| FieldItem { x, t: t.map(Into::into), ..Default::default() };
        let t = PivotTable {
            part: String::new(),
            name: "T".into(),
            cache_id: 0,
            range: CellRange { first_row: 0, first_col: 0, last_row: 3, last_col: 1 },
            first_header_row: 0,
            first_data_row: 1,
            first_data_col: 1,
            fields: vec![
                PivotField {
                    axis: Axis::Row,
                    items: vec![item(Some(0), None), item(Some(1), None), item(None, Some("default"))],
                    ..Default::default()
                },
                PivotField { data_field: true, ..Default::default() },
                PivotField { data_field: true, ..Default::default() },
            ],
            row_fields: vec![0],
            col_fields: Vec::new(),
            page_fields: Vec::new(),
            data_fields: vec![
                DataField { name: "Soma".into(), fld: 1, subtotal: "sum".into(), ..Default::default() },
                DataField { name: "Dobro".into(), fld: 2, subtotal: "sum".into(), ..Default::default() },
            ],
            row_items: Vec::new(),
            col_items: Vec::new(),
            compact: true,
            outline: true,
            row_grand: true,
            col_grand: true,
            data_on_rows: false,
            data_caption: "Valores".into(),
            grand_caption: None,
            row_header_caption: None,
            col_header_caption: None,
            style: None,
            show_headers: true,
            show_error: false,
            error_caption: String::new(),
            filters: Vec::new(),
            missing_caption: None,
            cube: None,
        };
        (t, cache, rec)
    }

    #[test]
    fn sums_and_layout() {
        let (mut t, cache, rec) = sample();
        t.col_fields = vec![-2];
        let r = compute(&t, &cache, &rec);
        // Linhas: Norte, Sul, total geral.
        assert_eq!(r.row_items.len(), 3);
        assert_eq!(r.row_items[2].t.as_deref(), Some("grand"));
        // Norte: 10 + 5; Sul: 5; total: 20. Campo calculado: o dobro da soma.
        assert_eq!(data_value(&t, &r, 0, 0), Some(CellValue::Number(15.0)));
        assert_eq!(data_value(&t, &r, 1, 0), Some(CellValue::Number(5.0)));
        assert_eq!(data_value(&t, &r, 2, 0), Some(CellValue::Number(20.0)));
        assert_eq!(data_value(&t, &r, 2, 1), Some(CellValue::Number(40.0)));
        // Escondendo "Sul": só Norte e o total (que passa a ser 15).
        t.fields[0].items[1].hidden = true;
        let r = compute(&t, &cache, &rec);
        assert_eq!(r.row_items.len(), 2);
        assert_eq!(data_value(&t, &r, 1, 0), Some(CellValue::Number(15.0)));
    }

    #[test]
    fn filters() {
        let (mut t, cache, rec) = sample();
        t.col_fields = vec![-2];
        // 10 maiores (1 maior): só Norte; o total geral acompanha.
        t.filters =
            vec![PivotFilter { fld: 0, kind: "count".into(), top: Some((true, 1.0)), ..Default::default() }];
        let r = compute(&t, &cache, &rec);
        assert_eq!(r.row_items.len(), 2);
        assert_eq!(data_value(&t, &r, 1, 0), Some(CellValue::Number(15.0)));
        // Valor maior que 10 (não passa ninguém além de Norte).
        t.filters[0] = PivotFilter {
            fld: 0,
            kind: "valueGreaterThan".into(),
            custom: vec![("greaterThan".into(), "10".into())],
            ..Default::default()
        };
        assert_eq!(compute(&t, &cache, &rec).row_items.len(), 2);
        // Rótulo contém "ul": só Sul.
        t.filters[0] = PivotFilter {
            fld: 0,
            kind: "captionContains".into(),
            custom: vec![("equal".into(), "*UL*".into())],
            ..Default::default()
        };
        let r = compute(&t, &cache, &rec);
        assert_eq!(r.row_items.len(), 2);
        assert_eq!(data_value(&t, &r, 1, 0), Some(CellValue::Number(5.0)));
    }

    #[test]
    fn show_values_as() {
        let (mut t, cache, rec) = sample();
        t.col_fields = vec![-2];
        let set = |t: &mut PivotTable, s: &str, item: u32| {
            t.data_fields[0].show_as = Some(s.into());
            t.data_fields[0].base_field = 0;
            t.data_fields[0].base_item = item;
        };
        let at = |t: &PivotTable, ri: usize| {
            let r = compute(t, &cache, &rec);
            data_value(t, &r, ri, 0)
        };
        set(&mut t, "percentOfTotal", 0);
        assert_eq!(at(&t, 0), Some(CellValue::Number(0.75)));
        assert_eq!(at(&t, 2), Some(CellValue::Number(1.0)));
        set(&mut t, "runTotal", 0);
        assert_eq!(at(&t, 1), Some(CellValue::Number(20.0)));
        set(&mut t, "difference", BASE_PREVIOUS);
        assert_eq!(at(&t, 0), None);
        assert_eq!(at(&t, 1), Some(CellValue::Number(-10.0)));
        set(&mut t, "percent", 0);
        assert_eq!(at(&t, 1), Some(CellValue::Number(5.0 / 15.0)));
        set(&mut t, "rankDescending", 0);
        assert_eq!(at(&t, 1), Some(CellValue::Number(2.0)));
    }
}
