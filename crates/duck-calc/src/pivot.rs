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
use std::collections::HashMap;

use duck_core::CellValue;
use duck_core::pivot::{AxisItem, PItem, PivotCache, PivotTable, Records, SortType};

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
    let mut cells: HashMap<(Vec<u32>, Vec<u32>), Vec<Acc>> = HashMap::new();
    let (mut passed, mut filtered, mut no_item) = (0, 0, 0);
    'rec: for r in 0..rec.count {
        for &(f, page) in &filters {
            let Some(p) = item_pos(f, r) else { continue };
            if t.fields[f].items[p as usize].hidden || page.is_some_and(|i| i != p) {
                filtered += 1;
                continue 'rec;
            }
        }
        let (Some(rp), Some(cp)) = (path(&rows, r), path(&cols, r)) else {
            no_item += 1;
            continue;
        };
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

    // Sem campo de valores e sem campos nos eixos: o Excel não mostra nada.
    if t.data_fields.is_empty() && t.row_fields.is_empty() && t.col_fields.is_empty() {
        return PivotResult {
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
        };
    }
    let (row_items, row_paths) = axis(t, cache, &rows, &cells, true);
    let (col_items, col_paths) = axis(t, cache, &cols, &cells, false);
    PivotResult {
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
    }
}

/// Linhas (ou colunas) do resultado.
fn axis(
    t: &PivotTable,
    cache: &PivotCache,
    levels: &[Level],
    cells: &HashMap<(Vec<u32>, Vec<u32>), Vec<Acc>>,
    rows: bool,
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
    let total = |p: &[u32], k: usize| -> f64 {
        let key = if rows { (p.to_vec(), Vec::new()) } else { (Vec::new(), p.to_vec()) };
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
            .filter(|&p| {
                let mut q = prefix.to_vec();
                q.push(p);
                has_data(&q) || (f.show_all && !f.items[p as usize].missing)
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
                    let o = total(&pa, k as usize).total_cmp(&total(&pb, k as usize));
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
    ) {
        let f = &t.fields[real[level]];
        let last = level + 1 == real.len();
        // Forma tabular: o primeiro filho fica na mesma linha do pai.
        let tabular = !f.outline || !t.outline && !t.compact;
        for c in children(level, prefix) {
            prefix.push(c);
            let collapsed = f.items[c as usize].collapsed;
            if last || collapsed {
                out.push((None, prefix.clone()));
            } else {
                if !tabular {
                    out.push((None, prefix.clone()));
                }
                walk(t, real, level + 1, prefix, children, out);
                let bottom = tabular || !f.subtotal_top;
                if f.default_subtotal && bottom {
                    out.push((Some("default"), prefix.clone()));
                }
            }
            prefix.pop();
        }
    }
    let mut prefix = Vec::new();
    walk(t, &real, 0, &mut prefix, &children, &mut out);

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
    let key = (r.row_paths.get(ri)?.clone(), r.col_paths.get(ci)?.clone());
    let accs = r.cells.get(&key)?;
    if let Some(Some(e)) = r.calc.get(k) {
        // Campo calculado: a fórmula sobre as somas dos campos na célula.
        let n_data = t.data_fields.len().max(1);
        let sums: Vec<f64> = accs[n_data..].iter().map(|a| a.sum).collect();
        return match e.eval(&sums) {
            Ok(x) if x.is_finite() => Some(CellValue::Number(x)),
            // Erro com `showError`: o texto escolhido (vazio = célula vazia).
            _ if t.show_error && t.error_caption.is_empty() => None,
            _ if t.show_error => Some(CellValue::Text(t.error_caption.clone())),
            _ => Some(CellValue::Error(duck_core::CellError::Div0)),
        };
    }
    let acc = accs.get(k)?;
    let func = t.data_fields.get(k).map_or("sum", |d| d.subtotal.as_str());
    acc.value(func).map(CellValue::Number)
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
                DataField {
                    name: "Soma".into(),
                    fld: 1,
                    subtotal: "sum".into(),
                    show_as: None,
                    num_fmt: None,
                },
                DataField {
                    name: "Dobro".into(),
                    fld: 2,
                    subtotal: "sum".into(),
                    show_as: None,
                    num_fmt: None,
                },
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
}
