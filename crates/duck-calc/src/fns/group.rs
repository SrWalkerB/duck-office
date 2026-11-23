//! AGRUPARPOR e PIVOTAR: tabelas resumidas por fórmula.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use duck_core::CellError;

use super::lambda::one;
use super::matrix;
use crate::dynarr::{Key, key, sort_cmp};
use crate::eval::{Eval, Res, to_bool};
use crate::funcs::{self, FnDef, n};
use crate::value::{Arr, Fun, V};

pub const DEFS: &[FnDef] = &[
    n(
        "GROUPBY",
        "AGRUPARPOR",
        3,
        8,
        groupby,
        "campos_de_linha; valores; função; [cabeçalhos_de_campo]; [profundidade_total]; [ordem_de_classificação]; [matriz_de_filtro]; [relação_de_campos]",
        "Agrupa linhas e resume os valores",
    ),
    n(
        "PIVOTBY",
        "PIVOTAR",
        4,
        11,
        pivotby,
        "campos_de_linha; campos_de_coluna; valores; função; [cabeçalhos_de_campo]; [profundidade_total_de_linha]; [ordem_de_linha]; [profundidade_total_de_coluna]; [ordem_de_coluna]; [matriz_de_filtro]; [relativo_a]",
        "Agrupa por linhas e colunas e resume os valores",
    ),
    n("PERCENTOF", "PORCENTAGEMDE", 2, 2, percentof, "subconjunto; total", "A parte de um total"),
];

fn percentof<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let sum = |v: Vec<f64>| v.iter().sum::<f64>();
    let part = sum(super::arr_nums(e, a[0])?);
    let all = sum(super::arr_nums(e, a[1])?);
    if all == 0.0 { Err(CellError::Div0) } else { Ok(V::num(part / all)) }
}

/// Valor de uma posição.
fn at<'e>(a: &Arr<'e>, r: usize, c: usize) -> V<'e> {
    a.v.get(r * a.cols as usize + c).cloned().unwrap_or(V::Empty)
}

/// Funções de agregação (uma, ou várias numa matriz de funções).
fn functions<'e>(e: &mut Eval<'e>, n: u32) -> Res<Vec<Rc<Fun<'e>>>> {
    match e.eval(n) {
        V::Fn(f) => Ok(vec![f]),
        V::Err(x) => Err(x),
        V::Arr(a) => {
            a.v.iter()
                .map(|v| match v {
                    V::Fn(f) => Ok(f.clone()),
                    V::Err(x) => Err(*x),
                    _ => Err(CellError::Value),
                })
                .collect()
        }
        _ => Err(CellError::Value),
    }
}

fn fun_name(f: &Fun) -> String {
    match f {
        Fun::Native(f) => funcs::def(*f).pt.to_string(),
        _ => "PERSONALIZADO".to_string(),
    }
}

/// A função recebe também o total (PORCENTAGEMDE, ou LAMBDA de dois parâmetros).
fn wants_total(f: &Fun) -> bool {
    match f {
        Fun::Native(f) => funcs::def(*f).min == 2,
        Fun::Lambda { params, .. } => *params == 2,
        Fun::Omitted => false,
    }
}

/// Argumento opcional inteiro.
fn opt_int(e: &mut Eval, a: &[u32], i: usize) -> Res<Option<i64>> {
    match a.get(i) {
        Some(&n) if !e.is_missing(n) => Ok(Some(e.num(n)?.trunc() as i64)),
        _ => Ok(None),
    }
}

/// Ordens de classificação (colunas com sinal).
fn orders(e: &mut Eval, a: &[u32], i: usize, max: usize) -> Res<Vec<i64>> {
    let Some(&n) = a.get(i) else { return Ok(Vec::new()) };
    if e.is_missing(n) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for v in &e.arr(n)?.v {
        let k = e.to_num(v)?.trunc() as i64;
        if k == 0 || k.unsigned_abs() as usize > max {
            return Err(CellError::Value);
        }
        out.push(k);
    }
    Ok(out)
}

/// Os dados já separados: campos, valores, funções e linhas que entram.
struct Source<'e> {
    values: Rc<Arr<'e>>,
    funs: Vec<Rc<Fun<'e>>>,
    /// Linhas de dados que passam no filtro.
    rows: Vec<usize>,
    /// Tem linha de título.
    headers: bool,
    /// Mostra os títulos.
    show: bool,
}

impl<'e> Source<'e> {
    /// `head` e `filter`: posições dos argumentos opcionais.
    fn read(
        e: &mut Eval<'e>,
        a: &[u32],
        fields: &[&Rc<Arr<'e>>],
        values: u32,
        fun: u32,
        head: usize,
        filter: usize,
    ) -> Res<Self> {
        let values = e.arr(values)?;
        let funs = functions(e, fun)?;
        let len = values.rows as usize;
        if funs.is_empty() || fields.iter().any(|f| f.rows as usize != len) {
            return Err(CellError::Value);
        }
        let levels = fields.iter().map(|f| f.cols).max().unwrap_or(1);
        let (headers, show) = match opt_int(e, a, head)? {
            Some(0) => (false, false),
            Some(1) => (true, false),
            Some(2) => (false, true),
            Some(3) => (true, true),
            Some(_) => return Err(CellError::Value),
            None => {
                let auto = matches!(at(&values, 0, 0), V::Str(_)) && matches!(at(&values, 1, 0), V::Num(_));
                (auto, auto && levels > 1)
            }
        };
        let first = headers as usize;
        let keep: Option<Rc<Arr>> = match a.get(filter) {
            Some(&n) if !e.is_missing(n) => Some(e.arr(n)?),
            _ => None,
        };
        let mut rows = Vec::new();
        match keep {
            None => rows.extend(first..len),
            Some(k) => {
                // O filtro pode vir com ou sem a linha do título.
                let off = match k.v.len() {
                    l if l == len => 0,
                    l if l + first == len => first,
                    _ => return Err(CellError::Value),
                };
                for r in first..len {
                    if to_bool(&k.v[r - off])? {
                        rows.push(r);
                    }
                }
            }
        }
        if rows.is_empty() {
            return Err(CellError::Calc);
        }
        Ok(Self { values, funs, rows, headers, show })
    }

    /// Colunas de resultado: cada coluna de valores com cada função.
    fn outputs(&self) -> usize {
        self.values.cols as usize * self.funs.len()
    }

    fn column(&self, rows: &[usize], c: usize) -> V<'e> {
        let v: Vec<V> = rows.iter().map(|&r| at(&self.values, r, c)).collect();
        V::Arr(Rc::new(Arr { rows: v.len() as u32, cols: 1, v }))
    }

    /// Resultado `k` das linhas `rows` (com `total` para as que comparam).
    fn aggregate(&self, e: &mut Eval<'e>, k: usize, rows: &[usize], total: &[usize]) -> V<'e> {
        let (c, f) = (k / self.funs.len(), &self.funs[k % self.funs.len()]);
        let mut args = vec![self.column(rows, c)];
        if wants_total(f) {
            args.push(self.column(total, c));
        }
        let v = e.call(f, args);
        one(e, v)
    }

    /// Título do resultado `k`.
    fn title(&self, k: usize) -> V<'e> {
        let (c, f) = (k / self.funs.len(), k % self.funs.len());
        let name = if self.headers { at(&self.values, 0, c) } else { V::text(format!("Valor {}", c + 1)) };
        match (self.funs.len() > 1, self.show) {
            (true, false) => V::text(fun_name(&self.funs[f])),
            (true, true) => V::text(format!(
                "{} ({})",
                crate::eval::to_text(name).unwrap_or_default(),
                fun_name(&self.funs[f])
            )),
            _ => name,
        }
    }

    /// Título do campo `c`.
    fn field(&self, f: &Arr<'e>, c: usize, kind: &str) -> V<'e> {
        if self.headers { at(f, 0, c) } else { V::text(format!("Campo de {kind} {}", c + 1)) }
    }
}

/// Linha (ou coluna) do resultado.
struct Line<'e> {
    /// Rótulos, um por campo.
    label: Vec<V<'e>>,
    /// Linhas de dados que ela resume.
    rows: Vec<usize>,
}

/// Como agrupar e somar um eixo.
struct Axis<'a, 'e> {
    fields: &'a Arr<'e>,
    depth: i64,
    order: Vec<i64>,
    /// Campos independentes (cada combinação é um grupo só).
    flat: bool,
}

impl<'e> Axis<'_, 'e> {
    fn levels(&self) -> usize {
        self.fields.cols as usize
    }

    /// Grupos de `rows` pelos campos `[from, to)`, em ordem.
    fn groups(
        &self,
        e: &mut Eval<'e>,
        src: &Source<'e>,
        rows: &[usize],
        from: usize,
        to: usize,
    ) -> Vec<(Vec<V<'e>>, Vec<usize>)> {
        let mut seen: HashMap<Vec<Key>, usize> = HashMap::new();
        let mut out: Vec<(Vec<V<'e>>, Vec<usize>)> = Vec::new();
        for &r in rows {
            let label: Vec<V> = (from..to).map(|c| at(self.fields, r, c)).collect();
            let k: Vec<Key> = label.iter().map(key).collect();
            let i = *seen.entry(k).or_insert_with(|| {
                out.push((label, Vec::new()));
                out.len() - 1
            });
            out[i].1.push(r);
        }
        // Valores que a ordem pede, calculados uma vez por grupo.
        let k = self.levels();
        let by: Vec<(i64, Option<Vec<V>>)> = self
            .order
            .iter()
            .map(|&o| {
                let c = o.unsigned_abs() as usize - 1;
                let vals = (c >= k).then(|| {
                    out.iter().map(|g| src.aggregate(e, c - k, &g.1, &src.rows)).collect::<Vec<V>>()
                });
                (o, vals)
            })
            .collect();
        let mut perm: Vec<usize> = (0..out.len()).collect();
        perm.sort_by(|&x, &y| {
            for (o, vals) in &by {
                let c = o.unsigned_abs() as usize - 1;
                let ord = match vals {
                    Some(v) => sort_cmp(&v[x], &v[y], *o < 0),
                    None if (from..to).contains(&c) => {
                        sort_cmp(&out[x].0[c - from], &out[y].0[c - from], *o < 0)
                    }
                    None => Ordering::Equal,
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            for (p, q) in out[x].0.iter().zip(&out[y].0) {
                let ord = sort_cmp(p, q, false);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });
        let mut slots: Vec<Option<(Vec<V>, Vec<usize>)>> = out.into_iter().map(Some).collect();
        perm.into_iter().filter_map(|i| slots[i].take()).collect()
    }

    /// Linhas do nível `level` para baixo, com os subtotais.
    fn walk(
        &self,
        e: &mut Eval<'e>,
        src: &Source<'e>,
        rows: &[usize],
        prefix: &[V<'e>],
        out: &mut Vec<Line<'e>>,
    ) {
        let level = prefix.len();
        let k = self.levels();
        if self.flat || level + 1 == k {
            for (label, rows) in self.groups(e, src, rows, level, k) {
                let mut l = prefix.to_vec();
                l.extend(label);
                out.push(Line { label: l, rows });
            }
            return;
        }
        for (label, rows) in self.groups(e, src, rows, level, level + 1) {
            let mut l = prefix.to_vec();
            l.extend(label);
            let sub = self.depth.unsigned_abs() as usize > level + 1;
            let total = || {
                let mut t = l.clone();
                t.resize(k, V::text(""));
                Line { label: t, rows: rows.clone() }
            };
            if sub && self.depth < 0 {
                out.push(total());
            }
            self.walk(e, src, &rows, &l, out);
            if sub && self.depth > 0 {
                out.push(total());
            }
        }
    }

    fn lines(&self, e: &mut Eval<'e>, src: &Source<'e>) -> Res<Vec<Line<'e>>> {
        let k = self.levels();
        if self.depth.unsigned_abs() as usize > k || (self.flat && self.depth.abs() > 1) {
            return Err(CellError::Value);
        }
        let mut out = Vec::new();
        self.walk(e, src, &src.rows, &[], &mut out);
        if self.depth != 0 {
            let mut label = vec![V::text(if self.depth.abs() > 1 { "Total Geral" } else { "Total" })];
            label.resize(k, V::text(""));
            let total = Line { label, rows: src.rows.clone() };
            if self.depth > 0 {
                out.push(total);
            } else {
                out.insert(0, total);
            }
        }
        Ok(out)
    }
}

/// AGRUPARPOR(campos; valores; função; [cabeçalhos]; [totais]; [ordem]; [filtro]; [relação]).
fn groupby<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let fields = e.arr(a[0])?;
    let src = Source::read(e, a, &[&fields], a[1], a[2], 3, 6)?;
    let k = fields.cols as usize;
    let depth = opt_int(e, a, 4)?.unwrap_or(1);
    let order = orders(e, a, 5, k + src.outputs())?;
    let flat = opt_int(e, a, 7)?.unwrap_or(0) == 1;
    let axis = Axis { fields: &fields, depth, order, flat };
    let lines = axis.lines(e, &src)?;
    let outs = src.outputs();
    let width = k + outs;
    let mut v = Vec::new();
    if src.show {
        v.extend((0..k).map(|c| src.field(&fields, c, "Linha")));
        v.extend((0..outs).map(|o| src.title(o)));
    } else if src.funs.len() > 1 {
        v.extend((0..k).map(|_| V::text("")));
        v.extend((0..outs).map(|o| src.title(o)));
    }
    for l in lines {
        v.extend(l.label);
        for o in 0..outs {
            v.push(src.aggregate(e, o, &l.rows, &src.rows));
        }
    }
    matrix((v.len() / width) as u32, width as u32, v)
}

/// PIVOTAR(linhas; colunas; valores; função; [cabeçalhos]; [totais de linha];
/// [ordem de linha]; [totais de coluna]; [ordem de coluna]; [filtro]; [relativo a]).
fn pivotby<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let rf = e.arr(a[0])?;
    let cf = e.arr(a[1])?;
    let src = Source::read(e, a, &[&rf, &cf], a[2], a[3], 4, 9)?;
    let (k, m, outs) = (rf.cols as usize, cf.cols as usize, src.outputs());
    let row_axis = Axis {
        fields: &rf,
        depth: opt_int(e, a, 5)?.unwrap_or(1),
        order: orders(e, a, 6, k + outs)?,
        flat: false,
    };
    let col_axis = Axis {
        fields: &cf,
        depth: opt_int(e, a, 7)?.unwrap_or(1),
        order: orders(e, a, 8, m + outs)?,
        flat: false,
    };
    let relative = opt_int(e, a, 10)?.unwrap_or(0);
    if !(0..=2).contains(&relative) {
        return Err(CellError::Value);
    }
    let rows = row_axis.lines(e, &src)?;
    let cols = col_axis.lines(e, &src)?;
    let width = k + cols.len() * outs;
    let mut v = Vec::new();
    // Títulos das colunas, um campo por linha.
    for level in 0..m {
        let last = level + 1 == m && outs == 1;
        for c in 0..k {
            v.push(if last && src.show { src.field(&rf, c, "Linha") } else { V::text("") });
        }
        for l in &cols {
            v.extend((0..outs).map(|_| l.label[level].clone()));
        }
    }
    if outs > 1 {
        v.extend((0..k).map(|c| if src.show { src.field(&rf, c, "Linha") } else { V::text("") }));
        for _ in &cols {
            v.extend((0..outs).map(|o| src.title(o)));
        }
    }
    // Em que linha de dados cada coluna do resultado entra.
    let members: Vec<std::collections::HashSet<usize>> =
        cols.iter().map(|c| c.rows.iter().copied().collect()).collect();
    for l in rows {
        v.extend(l.label);
        for (c, set) in cols.iter().zip(&members) {
            let both: Vec<usize> = l.rows.iter().copied().filter(|r| set.contains(r)).collect();
            let total = match relative {
                0 => &c.rows,
                1 => &l.rows,
                _ => &src.rows,
            };
            for o in 0..outs {
                v.push(if both.is_empty() { V::text("") } else { src.aggregate(e, o, &both, total) });
            }
        }
    }
    matrix((v.len() / width) as u32, width as u32, v)
}
