//! Avaliação das medidas num contexto de filtros.
//!
//! Um filtro é um conjunto de códigos de uma coluna. As linhas que uma
//! agregação vê são as que passam nos filtros da própria tabela e nos que
//! chegam pelas relações. A varredura começa pelo filtro mais seletivo (pelo
//! índice da coluna); a tabela inteira é varrida em paralelo. Agregações
//! repetidas com os mesmos filtros saem da memória.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::model::{Bits, Column, Model, Val, compare, num_text};
use crate::parse::{Node, Op};

type Res<T> = Result<T, String>;

/// Contexto de filtros: no máximo um conjunto por coluna, em ordem.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Ctx {
    f: Vec<((usize, usize), Arc<Bits>)>,
}

impl Ctx {
    pub fn set(&mut self, col: (usize, usize), b: Bits) {
        match self.f.binary_search_by_key(&col, |x| x.0) {
            Ok(i) => self.f[i].1 = Arc::new(b),
            Err(i) => self.f.insert(i, (col, Arc::new(b))),
        }
    }

    /// Restringe o que já estava (filtros de eixos e segmentações somam).
    pub fn restrict(&mut self, col: (usize, usize), mut b: Bits) {
        if let Some(old) = self.get(col) {
            b.and(old);
        }
        self.set(col, b);
    }

    pub fn remove(&mut self, col: (usize, usize)) {
        self.f.retain(|x| x.0 != col);
    }

    pub fn remove_table(&mut self, t: usize) {
        self.f.retain(|x| x.0.0 != t);
    }

    pub fn get(&self, col: (usize, usize)) -> Option<&Bits> {
        self.f.binary_search_by_key(&col, |x| x.0).ok().map(|i| &*self.f[i].1)
    }
}

/// Tabela calculada.
#[derive(Clone, Debug)]
pub struct Tab {
    pub cols: Vec<TabCol>,
    pub rows: Rows,
}

#[derive(Clone, Debug)]
pub struct TabCol {
    pub name: String,
    pub src: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub enum Rows {
    /// Linhas de uma tabela do modelo.
    Base(usize, Arc<Vec<u32>>),
    Values(Arc<Vec<Vec<Val>>>),
}

impl Tab {
    fn len(&self) -> usize {
        match &self.rows {
            Rows::Base(_, r) => r.len(),
            Rows::Values(r) => r.len(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Out {
    Scalar(Val),
    Table(Tab),
}

/// Contexto de linha: de uma tabela do modelo ou calculada.
#[derive(Clone)]
enum RowCtx {
    Base(usize, u32),
    Values(Arc<Vec<TabCol>>, Arc<Vec<Vec<Val>>>, usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Agg {
    Sum,
    Min,
    Max,
    Count,
    CountA,
    CountRows,
    Distinct,
    Average,
}

type MemoKey = (Agg, usize, usize, u64);

/// Avaliador com as memórias de uma rodada (uma dinâmica).
pub struct Eval<'m> {
    pub m: &'m Model,
    memo: Mutex<HashMap<MemoKey, Val>>,
    prop: Mutex<HashMap<(usize, u64), Arc<Bits>>>,
}

struct Scope<'a> {
    ctx: &'a Ctx,
    rows: Vec<RowCtx>,
    vars: Vec<(String, Out)>,
    depth: u32,
}

impl<'m> Eval<'m> {
    pub fn new(m: &'m Model) -> Self {
        Eval { m, memo: Mutex::new(HashMap::new()), prop: Mutex::new(HashMap::new()) }
    }

    /// Valor de uma medida num contexto.
    pub fn measure(&self, name: &str, ctx: &Ctx) -> Res<Val> {
        self.measure_at(name, ctx, 0)
    }

    fn measure_at(&self, name: &str, ctx: &Ctx, depth: u32) -> Res<Val> {
        if depth > 30 {
            return Err("medidas em círculo".into());
        }
        let me = self.m.measures.get(&name.to_lowercase()).ok_or("medida não encontrada")?;
        let body = me.body.as_ref().map_err(|e| e.clone())?;
        self.scalar(body, &mut Scope { ctx, rows: Vec::new(), vars: Vec::new(), depth })
    }

    // --- linhas visíveis -----------------------------------------------------

    /// Filtros que valem nas linhas de `t`: os dela e os que vêm das relações.
    fn effective(&self, t: usize, ctx: &Ctx, depth: u32) -> Res<Vec<(usize, Arc<Bits>)>> {
        let mut out: Vec<(usize, Arc<Bits>)> =
            ctx.f.iter().filter(|x| x.0.0 == t).map(|x| (x.0.1, x.1.clone())).collect();
        if depth > 8 {
            return Ok(out);
        }
        for (ri, rel) in self.m.rels.iter().enumerate() {
            if rel.from.0 != t {
                continue;
            }
            let dim = self.effective(rel.to.0, ctx, depth + 1)?;
            if dim.is_empty() {
                continue;
            }
            let key = (ri, hash(&dim));
            let hit = self.prop.lock().unwrap().get(&key).cloned();
            let bits = match hit {
                Some(b) => b,
                None => {
                    let cols = self.columns(rel.to.0, &dim)?;
                    let join = self.m.join(ri)?;
                    let mut b = Bits::new(join.row_of.len());
                    for (code, row) in join.row_of.iter().enumerate() {
                        if row.is_some_and(|r| pass(&cols, r)) {
                            b.set(code as u32);
                        }
                    }
                    let b = Arc::new(b);
                    self.prop.lock().unwrap().insert(key, b.clone());
                    b
                }
            };
            out.push((rel.from.1, bits));
        }
        Ok(out)
    }

    fn columns(&self, t: usize, eff: &[(usize, Arc<Bits>)]) -> Res<Vec<(Arc<Column>, Arc<Bits>)>> {
        eff.iter().map(|(c, b)| Ok((self.m.column(t, *c)?, b.clone()))).collect()
    }

    /// Dobra as linhas visíveis de `t`: `f` acumula, `join` junta pedaços.
    fn fold<A: Send>(
        &self,
        t: usize,
        eff: &[(usize, Arc<Bits>)],
        new: impl Fn() -> A + Sync + Send,
        f: impl Fn(&mut A, u32) + Sync + Send,
        join: impl Fn(A, A) -> A + Sync + Send,
    ) -> Res<A> {
        const CHUNK: usize = 32 * 1024;
        let n = self.m.rows(t);
        let cols = self.columns(t, eff)?;
        // O filtro que deixa menos linhas.
        let mut best: Option<(usize, usize)> = None;
        if n > 1024 {
            for (i, (c, b)) in cols.iter().enumerate() {
                let ix = c.index();
                let cost: usize =
                    b.iter().map(|k| (ix.start[k as usize + 1] - ix.start[k as usize]) as usize).sum();
                if cost * 2 < n && best.is_none_or(|x| cost < x.1) {
                    best = Some((i, cost));
                }
            }
        }
        if let Some((i, _)) = best {
            let (c, b) = &cols[i];
            let ix = c.index();
            let mut a = new();
            for k in b.iter() {
                for &r in &ix.rows[ix.start[k as usize] as usize..ix.start[k as usize + 1] as usize] {
                    if pass(&cols, r) {
                        f(&mut a, r);
                    }
                }
            }
            return Ok(a);
        }
        let scan = |from: usize| {
            let mut a = new();
            for r in from..(from + CHUNK).min(n) {
                if pass(&cols, r as u32) {
                    f(&mut a, r as u32);
                }
            }
            a
        };
        if n <= CHUNK {
            return Ok(scan(0));
        }
        let parts: Vec<A> = (0..n.div_ceil(CHUNK)).into_par_iter().map(|i| scan(i * CHUNK)).collect();
        Ok(parts.into_iter().reduce(join).unwrap_or_else(new))
    }

    /// Linhas visíveis de uma tabela.
    pub fn visible(&self, t: usize, ctx: &Ctx) -> Res<Vec<u32>> {
        let eff = self.effective(t, ctx, 0)?;
        let mut v = self.fold(
            t,
            &eff,
            Vec::new,
            |a: &mut Vec<u32>, r| a.push(r),
            |mut a, mut b| {
                a.append(&mut b);
                a
            },
        )?;
        v.sort_unstable();
        Ok(v)
    }

    /// Códigos visíveis de uma coluna.
    pub fn visible_codes(&self, t: usize, c: usize, ctx: &Ctx) -> Res<Bits> {
        let col = self.m.column(t, c)?;
        let eff = self.effective(t, ctx, 0)?;
        let n = col.dict.len();
        self.fold(
            t,
            &eff,
            || Bits::new(n),
            |b, r| b.set(col.codes[r as usize]),
            |mut a, b| {
                for i in b.iter() {
                    a.set(i);
                }
                a
            },
        )
    }

    fn aggregate(&self, k: Agg, t: usize, c: usize, ctx: &Ctx) -> Res<Val> {
        let eff = self.effective(t, ctx, 0)?;
        let key = (k, t, c, hash(&eff));
        if let Some(v) = self.memo.lock().unwrap().get(&key) {
            return Ok(v.clone());
        }
        let v = self.aggregate_now(k, t, c, &eff)?;
        self.memo.lock().unwrap().insert(key, v.clone());
        Ok(v)
    }

    fn aggregate_now(&self, k: Agg, t: usize, c: usize, eff: &[(usize, Arc<Bits>)]) -> Res<Val> {
        let count = |n: u64| if n == 0 { Val::Blank } else { Val::Num(n as f64) };
        if k == Agg::CountRows {
            let n = self.fold(t, eff, || 0u64, |a, _| *a += 1, |a, b| a + b)?;
            return Ok(count(n));
        }
        let col = self.m.column(t, c)?;
        match k {
            Agg::Distinct | Agg::Min | Agg::Max => {
                let n = col.dict.len();
                let b = self.fold(
                    t,
                    eff,
                    || Bits::new(n),
                    |b, r| b.set(col.codes[r as usize]),
                    |mut a, b| {
                        for i in b.iter() {
                            a.set(i);
                        }
                        a
                    },
                )?;
                if k == Agg::Distinct {
                    return Ok(count(b.iter().count() as u64));
                }
                let vals = b.iter().map(|i| &col.dict[i as usize]).filter(|v| **v != Val::Blank);
                let best = if k == Agg::Min {
                    vals.min_by(|a, b| compare(a, b))
                } else {
                    vals.max_by(|a, b| compare(a, b))
                };
                Ok(best.cloned().unwrap_or(Val::Blank))
            }
            _ => {
                // Números por código: a varredura só soma e conta.
                let nums: Vec<Option<f64>> = col
                    .dict
                    .iter()
                    .map(|v| match v {
                        Val::Num(n) => Some(*n),
                        Val::Bool(b) if k != Agg::Count => Some(*b as u8 as f64),
                        _ => None,
                    })
                    .collect();
                let filled: Vec<bool> = col.dict.iter().map(|v| *v != Val::Blank).collect();
                let (sum, n, any) = self.fold(
                    t,
                    eff,
                    || (Sum::default(), 0u64, 0u64),
                    |a, r| {
                        let code = col.codes[r as usize] as usize;
                        if let Some(x) = nums[code] {
                            a.0.add(x);
                            a.1 += 1;
                        }
                        a.2 += filled[code] as u64;
                    },
                    |a, b| (a.0.join(b.0), a.1 + b.1, a.2 + b.2),
                )?;
                Ok(match k {
                    Agg::Sum if n == 0 => Val::Blank,
                    Agg::Sum => Val::Num(sum.value()),
                    Agg::Average if n == 0 => Val::Blank,
                    Agg::Average => Val::Num(sum.value() / n as f64),
                    // `COUNT` também conta datas e textos numa coluna.
                    _ => count(any),
                })
            }
        }
    }

    // --- expressões ----------------------------------------------------------

    fn scalar(&self, n: &Node, s: &mut Scope) -> Res<Val> {
        match self.eval(n, s)? {
            Out::Scalar(v) => Ok(v),
            // Uma tabela de uma célula vale como valor.
            Out::Table(t) if t.cols.len() == 1 && t.len() <= 1 => {
                Ok(if t.len() == 0 { Val::Blank } else { self.cell(&t, 0, 0)? })
            }
            Out::Table(_) => Err("uma tabela onde devia haver um valor".into()),
        }
    }

    fn table(&self, n: &Node, s: &mut Scope) -> Res<Tab> {
        match self.eval(n, s)? {
            Out::Table(t) => Ok(t),
            Out::Scalar(_) => Err("um valor onde devia haver uma tabela".into()),
        }
    }

    fn cell(&self, t: &Tab, row: usize, col: usize) -> Res<Val> {
        match &t.rows {
            Rows::Values(v) => Ok(v[row][col].clone()),
            Rows::Base(ti, r) => {
                let c = self.m.column(*ti, col)?;
                Ok(c.dict[c.codes[r[row] as usize] as usize].clone())
            }
        }
    }

    fn row_ctx(t: &Tab, cols: &Arc<Vec<TabCol>>, i: usize) -> RowCtx {
        match &t.rows {
            Rows::Base(ti, r) => RowCtx::Base(*ti, r[i]),
            Rows::Values(v) => RowCtx::Values(cols.clone(), v.clone(), i),
        }
    }

    fn base_tab(&self, t: usize, rows: Vec<u32>) -> Tab {
        let cols = self.m.tables[t]
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| TabCol { name: c.name.clone(), src: Some((t, i)) })
            .collect();
        Tab { cols, rows: Rows::Base(t, Arc::new(rows)) }
    }

    /// Coluna nas linhas atuais (a mais de dentro primeiro).
    fn row_value(&self, s: &Scope, table: Option<usize>, name: &str) -> Res<Option<Val>> {
        for r in s.rows.iter().rev() {
            match r {
                RowCtx::Base(t, row) => {
                    if table.is_some_and(|x| x != *t) {
                        // Do lado "um" da relação (RELATED implícito não existe).
                        continue;
                    }
                    let cols = &self.m.tables[*t].columns;
                    if let Some(c) = cols.iter().position(|c| c.name.to_lowercase() == name.to_lowercase()) {
                        let col = self.m.column(*t, c)?;
                        return Ok(Some(col.dict[col.codes[*row as usize] as usize].clone()));
                    }
                }
                RowCtx::Values(cols, rows, i) => {
                    let hit = cols.iter().position(|c| {
                        c.name.to_lowercase() == name.to_lowercase()
                            && table.is_none_or(|t| c.src.is_none_or(|x| x.0 == t))
                    });
                    if let Some(c) = hit {
                        return Ok(Some(rows[*i][c].clone()));
                    }
                }
            }
        }
        Ok(None)
    }

    fn eval(&self, n: &Node, s: &mut Scope) -> Res<Out> {
        s.depth += 1;
        if s.depth > 400 {
            return Err("fórmula profunda demais".into());
        }
        let r = self.eval_in(n, s);
        s.depth -= 1;
        r
    }

    fn eval_in(&self, n: &Node, s: &mut Scope) -> Res<Out> {
        let val = |v: Val| Ok(Out::Scalar(v));
        match n {
            Node::Num(x) => val(Val::Num(*x)),
            Node::Str(x) => val(Val::str(x)),
            Node::Name(name) => {
                let k = name.to_lowercase();
                if let Some((_, o)) = s.vars.iter().rev().find(|v| v.0 == k) {
                    return Ok(o.clone());
                }
                match self.m.table(name) {
                    Some(t) => Ok(Out::Table(self.base_tab(t, self.visible(t, s.ctx)?))),
                    None => match k.as_str() {
                        "true" => val(Val::Bool(true)),
                        "false" => val(Val::Bool(false)),
                        _ => Err("nome desconhecido na fórmula".into()),
                    },
                }
            }
            Node::Col(t, c) => {
                let ti = self.m.table(t);
                if let Some(v) = self.row_value(s, ti, c)? {
                    return val(v);
                }
                // `Tabela[Medida]`.
                if self.m.measures.contains_key(&c.to_lowercase()) && self.m.find(t, c).is_none() {
                    return val(self.measure_at(c, s.ctx, s.depth)?);
                }
                Err("coluna sem linha atual".into())
            }
            Node::Bracket(c) => {
                if let Some(v) = self.row_value(s, None, c)? {
                    return val(v);
                }
                val(self.measure_at(c, &self.transition(s)?, s.depth)?)
            }
            Node::Neg(x) => match self.scalar(x, s)? {
                Val::Blank => val(Val::Blank),
                v => val(Val::Num(-num(&v)?)),
            },
            Node::Not(x) => val(Val::Bool(!truth(&self.scalar(x, s)?)?)),
            Node::List(l) => {
                let mut rows = Vec::new();
                for x in l {
                    rows.push(vec![self.scalar(x, s)?]);
                }
                Ok(Out::Table(Tab {
                    cols: vec![TabCol { name: "Value".into(), src: None }],
                    rows: Rows::Values(Arc::new(rows)),
                }))
            }
            Node::Vars(list, body) => {
                let mark = s.vars.len();
                for (name, e) in list {
                    let o = self.eval(e, s)?;
                    s.vars.push((name.clone(), o));
                }
                let r = self.eval(body, s);
                s.vars.truncate(mark);
                r
            }
            Node::Bin(op, a, b) => match op {
                Op::And => {
                    let x = truth(&self.scalar(a, s)?)?;
                    val(Val::Bool(x && truth(&self.scalar(b, s)?)?))
                }
                Op::Or => {
                    let x = truth(&self.scalar(a, s)?)?;
                    val(Val::Bool(x || truth(&self.scalar(b, s)?)?))
                }
                Op::In => {
                    let x = self.scalar(a, s)?;
                    let t = self.table(b, s)?;
                    for i in 0..t.len() {
                        if equal(&x, &self.cell(&t, i, 0)?)? {
                            return val(Val::Bool(true));
                        }
                    }
                    val(Val::Bool(false))
                }
                op => {
                    let x = self.scalar(a, s)?;
                    let y = self.scalar(b, s)?;
                    val(binary(*op, &x, &y)?)
                }
            },
            Node::Func(f, args) => self.func(f, args, s),
        }
    }

    /// Contexto com a linha atual virando filtro (medida dentro de linha).
    fn transition(&self, s: &Scope) -> Res<Ctx> {
        let mut ctx = s.ctx.clone();
        for r in &s.rows {
            if let RowCtx::Base(t, row) = r {
                for c in 0..self.m.tables[*t].columns.len() {
                    if self.m.tables[*t].columns[c].expr.is_some() {
                        continue;
                    }
                    let col = self.m.column(*t, c)?;
                    let mut b = Bits::new(col.dict.len());
                    b.set(col.codes[*row as usize]);
                    ctx.restrict((*t, c), b);
                }
            }
        }
        Ok(ctx)
    }

    fn col_of(&self, n: &Node) -> Res<(usize, usize)> {
        match n {
            Node::Col(t, c) => self.m.find(t, c).ok_or_else(|| "coluna não encontrada".into()),
            _ => Err("esperada uma coluna".into()),
        }
    }

    /// Aplica um argumento de `CALCULATE` ao contexto.
    /// (os filtros novos ficam em `new`: valem juntos e trocam os de fora).
    fn apply_filter(&self, n: &Node, ctx: &mut Ctx, new: &mut Ctx, s: &mut Scope) -> Res<()> {
        if let Node::Func(f, args) = n {
            match f.as_str() {
                "ALL" | "REMOVEFILTERS" | "ALLNOBLANKROW" => {
                    if args.is_empty() {
                        *ctx = Ctx::default();
                    }
                    for a in args {
                        match a {
                            Node::Col(..) => ctx.remove(self.col_of(a)?),
                            Node::Name(t) => {
                                let t = self.m.table(t).ok_or("tabela não encontrada")?;
                                ctx.remove_table(t);
                            }
                            _ => return Err("argumento de ALL".into()),
                        }
                    }
                    return Ok(());
                }
                "ALLEXCEPT" => {
                    let Some(Node::Name(t)) = args.first() else { return Err("ALLEXCEPT".into()) };
                    let t = self.m.table(t).ok_or("tabela não encontrada")?;
                    let keep: Vec<_> = args[1..].iter().map(|a| self.col_of(a)).collect::<Res<_>>()?;
                    ctx.f.retain(|x| x.0.0 != t || keep.contains(&x.0));
                    return Ok(());
                }
                "KEEPFILTERS" => {
                    let col = self.filter_col(&args[0]).ok_or("KEEPFILTERS")??;
                    let b = self.predicate(&args[0], col, s)?;
                    ctx.restrict(col, b);
                    return Ok(());
                }
                _ => {}
            }
        }
        // Predicado sobre uma coluna: troca o filtro dela.
        if let Some(col) = self.filter_col(n) {
            let col = col?;
            let b = self.predicate(n, col, s)?;
            new.restrict(col, b);
            return Ok(());
        }
        // Uma tabela (FILTER, VALUES...): os valores das colunas dela.
        let t = self.table(n, s)?;
        match &t.rows {
            Rows::Base(ti, rows) => {
                // As linhas escolhidas, coluna a coluna (as que têm filtro próprio
                // na expressão entram juntas).
                for c in 0..self.m.tables[*ti].columns.len() {
                    let col = self.m.column(*ti, c)?;
                    let mut b = Bits::new(col.dict.len());
                    for &r in rows.iter() {
                        b.set(col.codes[r as usize]);
                    }
                    if b.iter().count() < col.dict.len() {
                        ctx.restrict((*ti, c), b);
                    }
                }
            }
            Rows::Values(rows) => {
                for (k, tc) in t.cols.iter().enumerate() {
                    let Some(src) = tc.src else {
                        return Err("filtro por tabela sem coluna do modelo".into());
                    };
                    let col = self.m.column(src.0, src.1)?;
                    let mut b = Bits::new(col.dict.len());
                    for r in rows.iter() {
                        if let Some(&c) = col.map.get(&r[k].key()) {
                            b.set(c);
                        }
                    }
                    new.restrict(src, b);
                }
            }
        }
        Ok(())
    }

    /// A única coluna de um predicado (`T[c] >= x`, `T[c] = "a" || T[c] = "b"`).
    fn filter_col(&self, n: &Node) -> Option<Res<(usize, usize)>> {
        fn walk<'n>(n: &'n Node, out: &mut Vec<&'n Node>) {
            match n {
                Node::Col(..) => out.push(n),
                Node::Neg(a) | Node::Not(a) => walk(a, out),
                Node::Bin(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                Node::Func(f, args)
                    if matches!(
                        f.as_str(),
                        "NOT"
                            | "ISBLANK"
                            | "LEFT"
                            | "RIGHT"
                            | "UPPER"
                            | "LOWER"
                            | "TRIM"
                            | "YEAR"
                            | "MONTH"
                            | "DAY"
                            | "AND"
                            | "OR"
                            | "LEN"
                            | "INT"
                            | "VALUE"
                    ) =>
                {
                    args.iter().for_each(|a| walk(a, out));
                }
                _ => {}
            }
        }
        let is_pred = matches!(
            n,
            Node::Not(_)
                | Node::Bin(
                    Op::Eq
                        | Op::StrictEq
                        | Op::Ne
                        | Op::Lt
                        | Op::Le
                        | Op::Gt
                        | Op::Ge
                        | Op::And
                        | Op::Or
                        | Op::In,
                    ..
                )
        ) || matches!(n, Node::Func(f, _) if matches!(f.as_str(), "NOT" | "ISBLANK" | "AND" | "OR"));
        if !is_pred {
            return None;
        }
        let mut cols = Vec::new();
        walk(n, &mut cols);
        let first = cols.first()?;
        if cols.iter().any(|c| c != first) {
            return Some(Err("predicado com mais de uma coluna".into()));
        }
        Some(self.col_of(first))
    }

    /// Valores da coluna que passam num predicado.
    fn predicate(&self, n: &Node, col: (usize, usize), s: &mut Scope) -> Res<Bits> {
        let c = self.m.column(col.0, col.1)?;
        let name = self.m.tables[col.0].columns[col.1].name.clone();
        let cols = Arc::new(vec![TabCol { name, src: Some(col) }]);
        let rows: Arc<Vec<Vec<Val>>> = Arc::new(c.dict.iter().map(|v| vec![v.clone()]).collect());
        let mut b = Bits::new(c.dict.len());
        for i in 0..c.dict.len() {
            s.rows.push(RowCtx::Values(cols.clone(), rows.clone(), i));
            let r = self.scalar(n, s).and_then(|v| truth(&v));
            s.rows.pop();
            if r? {
                b.set(i as u32);
            }
        }
        Ok(b)
    }

    fn with_filters(&self, args: &[Node], s: &mut Scope) -> Res<Ctx> {
        let mut ctx = self.transition(s)?;
        // `ALL` vale antes dos predicados, em qualquer ordem.
        let is_all = |n: &Node| matches!(n, Node::Func(f, _) if f.starts_with("ALL") || f == "REMOVEFILTERS");
        let mut new = Ctx::default();
        for a in args.iter().filter(|a| is_all(a)) {
            self.apply_filter(a, &mut ctx, &mut new, s)?;
        }
        for a in args.iter().filter(|a| !is_all(a)) {
            self.apply_filter(a, &mut ctx, &mut new, s)?;
        }
        for (col, b) in new.f {
            ctx.set(col, (*b).clone());
        }
        Ok(ctx)
    }

    fn func(&self, f: &str, args: &[Node], s: &mut Scope) -> Res<Out> {
        let val = |v: Val| Ok(Out::Scalar(v));
        let arg = |i: usize| args.get(i).ok_or_else(|| format!("{f}: faltam argumentos"));
        let agg = match f {
            "SUM" => Some(Agg::Sum),
            "MIN" if args.len() == 1 => Some(Agg::Min),
            "MAX" if args.len() == 1 => Some(Agg::Max),
            "COUNT" => Some(Agg::Count),
            "COUNTA" => Some(Agg::CountA),
            "DISTINCTCOUNT" => Some(Agg::Distinct),
            "AVERAGE" => Some(Agg::Average),
            _ => None,
        };
        if let Some(k) = agg {
            let (t, c) = self.col_of(arg(0)?)?;
            return val(self.aggregate(k, t, c, s.ctx)?);
        }
        match f {
            "COUNTROWS" => {
                if let Node::Name(n) = arg(0)?
                    && !s.vars.iter().any(|v| v.0 == n.to_lowercase())
                    && let Some(t) = self.m.table(n)
                {
                    return val(self.aggregate(Agg::CountRows, t, 0, s.ctx)?);
                }
                let t = self.table(arg(0)?, s)?;
                val(if t.len() == 0 { Val::Blank } else { Val::Num(t.len() as f64) })
            }
            "CALCULATE" | "CALCULATETABLE" => {
                let ctx = self.with_filters(&args[1.min(args.len())..], s)?;
                let mut inner = Scope { ctx: &ctx, rows: Vec::new(), vars: s.vars.clone(), depth: s.depth };
                self.eval(arg(0)?, &mut inner)
            }
            "DIVIDE" => {
                let a = self.scalar(arg(0)?, s)?;
                let b = self.scalar(arg(1)?, s)?;
                let d = num(&b)?;
                if d == 0.0 {
                    return match args.get(2) {
                        Some(x) => val(self.scalar(x, s)?),
                        None => val(Val::Blank),
                    };
                }
                val(if a == Val::Blank { Val::Blank } else { Val::Num(num(&a)? / d) })
            }
            "IF" => {
                if truth(&self.scalar(arg(0)?, s)?)? {
                    self.eval(arg(1)?, s)
                } else if let Some(x) = args.get(2) {
                    self.eval(x, s)
                } else {
                    val(Val::Blank)
                }
            }
            "IFERROR" => match self.eval(arg(0)?, s) {
                Ok(v) => Ok(v),
                Err(_) => self.eval(arg(1)?, s),
            },
            "SWITCH" => {
                let x = self.scalar(arg(0)?, s)?;
                let mut i = 1;
                while i + 1 < args.len() {
                    if equal(&x, &self.scalar(&args[i], s)?)? {
                        return self.eval(&args[i + 1], s);
                    }
                    i += 2;
                }
                if i < args.len() { self.eval(&args[i], s) } else { val(Val::Blank) }
            }
            "BLANK" => val(Val::Blank),
            "TRUE" => val(Val::Bool(true)),
            "FALSE" => val(Val::Bool(false)),
            "ISBLANK" => val(Val::Bool(self.scalar(arg(0)?, s)? == Val::Blank)),
            "ISERROR" => val(Val::Bool(self.eval(arg(0)?, s).is_err())),
            "NOT" => val(Val::Bool(!truth(&self.scalar(arg(0)?, s)?)?)),
            "AND" => {
                let a = truth(&self.scalar(arg(0)?, s)?)?;
                val(Val::Bool(a && truth(&self.scalar(arg(1)?, s)?)?))
            }
            "OR" => {
                let a = truth(&self.scalar(arg(0)?, s)?)?;
                val(Val::Bool(a || truth(&self.scalar(arg(1)?, s)?)?))
            }
            "COALESCE" => {
                for a in args {
                    let v = self.scalar(a, s)?;
                    if v != Val::Blank {
                        return val(v);
                    }
                }
                val(Val::Blank)
            }
            "TODAY" => val(Val::Num(self.m.today)),
            "NOW" => val(Val::Num(self.m.now)),
            "EOMONTH" | "EDATE" => {
                let d = num(&self.scalar(arg(0)?, s)?)?;
                let k = num(&self.scalar(arg(1)?, s)?)?.trunc() as i64;
                let (y, m, dd) = ymd(d)?;
                let m0 = y * 12 + m - 1 + k;
                let (y2, m2) = (m0.div_euclid(12), m0.rem_euclid(12) + 1);
                let last = serial(y2 + m2 / 12, m2 % 12 + 1, 1)? - 1.0;
                let first = serial(y2, m2, 1)?;
                val(Val::Num(if f == "EOMONTH" { last } else { (first + dd as f64 - 1.0).min(last) }))
            }
            "DATE" => {
                let y = num(&self.scalar(arg(0)?, s)?)? as i64;
                let m = num(&self.scalar(arg(1)?, s)?)? as i64;
                let d = num(&self.scalar(arg(2)?, s)?)? as i64;
                val(Val::Num(serial(y, m, d)?))
            }
            "YEAR" | "MONTH" | "DAY" => match self.scalar(arg(0)?, s)? {
                Val::Blank => val(Val::Blank),
                v => {
                    let (y, m, d) = ymd(num(&v)?)?;
                    val(Val::Num(match f {
                        "YEAR" => y,
                        "MONTH" => m,
                        _ => d,
                    } as f64))
                }
            },
            "WEEKDAY" => {
                let d = num(&self.scalar(arg(0)?, s)?)?.floor();
                let kind = match args.get(1) {
                    Some(x) => num(&self.scalar(x, s)?)? as i64,
                    None => 1,
                };
                // Serial 1 = domingo (1/1/1900 no calendário do Excel).
                let sunday = (d - 1.0).rem_euclid(7.0);
                val(Val::Num(match kind {
                    2 => (sunday + 6.0).rem_euclid(7.0) + 1.0,
                    3 => (sunday + 6.0).rem_euclid(7.0),
                    _ => sunday + 1.0,
                }))
            }
            "FORMAT" => {
                let v = self.scalar(arg(0)?, s)?;
                let code = text(&self.scalar(arg(1)?, s)?);
                val(match v {
                    Val::Blank => Val::str(""),
                    Val::Str(_) => v,
                    v => {
                        let mut out = String::new();
                        duck_core::NumFmt::parse(&code).format_number(num(&v)?, false, &mut out);
                        Val::Str(out.into())
                    }
                })
            }
            "VALUE" => match self.scalar(arg(0)?, s)? {
                Val::Str(t) => {
                    let t = t.trim();
                    let t =
                        if t.contains(',') { t.replace('.', "").replace(',', ".") } else { t.to_string() };
                    val(Val::Num(t.parse().map_err(|_| "texto que não é número")?))
                }
                v => val(v),
            },
            "INT" => val(Val::Num(num(&self.scalar(arg(0)?, s)?)?.floor())),
            "ABS" => val(Val::Num(num(&self.scalar(arg(0)?, s)?)?.abs())),
            "ROUND" => {
                let x = num(&self.scalar(arg(0)?, s)?)?;
                let p = 10f64.powi(num(&self.scalar(arg(1)?, s)?)? as i32);
                val(Val::Num((x * p).round() / p))
            }
            "MIN" | "MAX" => {
                let a = num(&self.scalar(arg(0)?, s)?)?;
                let b = num(&self.scalar(arg(1)?, s)?)?;
                val(Val::Num(if f == "MIN" { a.min(b) } else { a.max(b) }))
            }
            "CONCATENATE" => {
                let a = text(&self.scalar(arg(0)?, s)?);
                let b = text(&self.scalar(arg(1)?, s)?);
                val(Val::Str((a + &b).into()))
            }
            "UPPER" => val(Val::Str(text(&self.scalar(arg(0)?, s)?).to_uppercase().into())),
            "LOWER" => val(Val::Str(text(&self.scalar(arg(0)?, s)?).to_lowercase().into())),
            "TRIM" => val(Val::Str(text(&self.scalar(arg(0)?, s)?).trim().into())),
            "LEN" => val(Val::Num(text(&self.scalar(arg(0)?, s)?).chars().count() as f64)),
            "LEFT" | "RIGHT" => {
                let t: Vec<char> = text(&self.scalar(arg(0)?, s)?).chars().collect();
                let k = match args.get(1) {
                    Some(x) => num(&self.scalar(x, s)?)? as usize,
                    None => 1,
                }
                .min(t.len());
                let part = if f == "LEFT" { &t[..k] } else { &t[t.len() - k..] };
                val(Val::Str(part.iter().collect::<String>().into()))
            }
            // --- tabelas ---
            "ALL" => {
                let empty = Ctx::default();
                match arg(0)? {
                    Node::Name(t) => {
                        let t = self.m.table(t).ok_or("tabela não encontrada")?;
                        Ok(Out::Table(self.base_tab(t, self.visible(t, &empty)?)))
                    }
                    n => {
                        let col = self.col_of(n)?;
                        Ok(Out::Table(self.values(col, &empty)?))
                    }
                }
            }
            "VALUES" | "DISTINCT" if matches!(arg(0)?, Node::Col(..)) => {
                let col = self.col_of(arg(0)?)?;
                Ok(Out::Table(self.values(col, s.ctx)?))
            }
            "VALUES" => Ok(Out::Table(self.table(arg(0)?, s)?)),
            "DISTINCT" => {
                let t = self.table(arg(0)?, s)?;
                let mut seen = HashSet::new();
                let mut rows = Vec::new();
                for i in 0..t.len() {
                    let r: Vec<Val> = (0..t.cols.len()).map(|c| self.cell(&t, i, c)).collect::<Res<_>>()?;
                    if seen.insert(r.iter().map(Val::key).collect::<Vec<_>>()) {
                        rows.push(r);
                    }
                }
                Ok(Out::Table(Tab { cols: t.cols, rows: Rows::Values(Arc::new(rows)) }))
            }
            "FILTER" => {
                let t = self.table(arg(0)?, s)?;
                let cols = Arc::new(t.cols.clone());
                let mut keep = Vec::new();
                for i in 0..t.len() {
                    s.rows.push(Self::row_ctx(&t, &cols, i));
                    let r = self.scalar(arg(1)?, s).and_then(|v| truth(&v));
                    s.rows.pop();
                    if r? {
                        keep.push(i);
                    }
                }
                let rows = match &t.rows {
                    Rows::Base(ti, r) => Rows::Base(*ti, Arc::new(keep.iter().map(|&i| r[i]).collect())),
                    Rows::Values(r) => Rows::Values(Arc::new(keep.iter().map(|&i| r[i].clone()).collect())),
                };
                Ok(Out::Table(Tab { cols: t.cols, rows }))
            }
            "SELECTCOLUMNS" | "ADDCOLUMNS" => {
                let t = self.table(arg(0)?, s)?;
                let row_cols = Arc::new(t.cols.clone());
                let mut cols = if f == "ADDCOLUMNS" { t.cols.clone() } else { Vec::new() };
                let mut exprs = Vec::new();
                let mut i = 1;
                while i + 1 < args.len() {
                    let name = text(&self.scalar(&args[i], s)?);
                    cols.push(TabCol { name, src: None });
                    exprs.push(&args[i + 1]);
                    i += 2;
                }
                let mut rows = Vec::with_capacity(t.len());
                for i in 0..t.len() {
                    let mut r = Vec::with_capacity(cols.len());
                    if f == "ADDCOLUMNS" {
                        for c in 0..t.cols.len() {
                            r.push(self.cell(&t, i, c)?);
                        }
                    }
                    s.rows.push(Self::row_ctx(&t, &row_cols, i));
                    for e in &exprs {
                        match self.scalar(e, s) {
                            Ok(v) => r.push(v),
                            Err(e) => {
                                s.rows.pop();
                                return Err(e);
                            }
                        }
                    }
                    s.rows.pop();
                    rows.push(r);
                }
                Ok(Out::Table(Tab { cols, rows: Rows::Values(Arc::new(rows)) }))
            }
            "UNION" => {
                let mut rows = Vec::new();
                let mut cols: Vec<TabCol> = Vec::new();
                for a in args {
                    let t = self.table(a, s)?;
                    for i in 0..t.len() {
                        rows.push((0..t.cols.len()).map(|c| self.cell(&t, i, c)).collect::<Res<Vec<_>>>()?);
                    }
                    if cols.is_empty() {
                        cols = t.cols;
                    }
                }
                Ok(Out::Table(Tab { cols, rows: Rows::Values(Arc::new(rows)) }))
            }
            "SUMX" | "AVERAGEX" | "MINX" | "MAXX" | "COUNTX" | "COUNTAX" => {
                let t = self.table(arg(0)?, s)?;
                let cols = Arc::new(t.cols.clone());
                let mut sum = Sum::default();
                let mut n = 0u64;
                let mut best: Option<Val> = None;
                for i in 0..t.len() {
                    s.rows.push(Self::row_ctx(&t, &cols, i));
                    let v = self.scalar(arg(1)?, s);
                    s.rows.pop();
                    let v = v?;
                    if v == Val::Blank {
                        continue;
                    }
                    n += 1;
                    match f {
                        "SUMX" | "AVERAGEX" => sum.add(num(&v)?),
                        "MINX" | "MAXX" => {
                            let better = best.as_ref().is_none_or(|b| {
                                let o = compare(&v, b);
                                if f == "MINX" { o.is_lt() } else { o.is_gt() }
                            });
                            if better {
                                best = Some(v);
                            }
                        }
                        _ => {}
                    }
                }
                val(match f {
                    _ if n == 0 => Val::Blank,
                    "SUMX" => Val::Num(sum.value()),
                    "AVERAGEX" => Val::Num(sum.value() / n as f64),
                    "MINX" | "MAXX" => best.unwrap_or(Val::Blank),
                    _ => Val::Num(n as f64),
                })
            }
            "CONCATENATEX" => {
                let t = self.table(arg(0)?, s)?;
                let cols = Arc::new(t.cols.clone());
                let sep = match args.get(2) {
                    Some(x) => text(&self.scalar(x, s)?),
                    None => String::new(),
                };
                let mut parts = Vec::with_capacity(t.len());
                for i in 0..t.len() {
                    s.rows.push(Self::row_ctx(&t, &cols, i));
                    let v = self.scalar(arg(1)?, s);
                    s.rows.pop();
                    parts.push(text(&v?));
                }
                val(if parts.is_empty() { Val::Blank } else { Val::Str(parts.join(&sep).into()) })
            }
            "HASONEVALUE" | "SELECTEDVALUE" => {
                let col = self.col_of(arg(0)?)?;
                let c = self.m.column(col.0, col.1)?;
                let codes: Vec<u32> = self.visible_codes(col.0, col.1, s.ctx)?.iter().take(2).collect();
                if f == "HASONEVALUE" {
                    return val(Val::Bool(codes.len() == 1));
                }
                match (codes.len(), args.get(1)) {
                    (1, _) => val(c.dict[codes[0] as usize].clone()),
                    (_, Some(x)) => self.eval(x, s),
                    _ => val(Val::Blank),
                }
            }
            "RELATED" => {
                let col = self.col_of(arg(0)?)?;
                for r in s.rows.iter().rev() {
                    let RowCtx::Base(t, row) = r else { continue };
                    for (ri, rel) in self.m.rels.iter().enumerate() {
                        if rel.from.0 == *t && rel.to.0 == col.0 {
                            let fk = self.m.column(rel.from.0, rel.from.1)?;
                            let to = self.m.join(ri)?.row_of[fk.codes[*row as usize] as usize];
                            let c = self.m.column(col.0, col.1)?;
                            return val(
                                to.map_or(Val::Blank, |r| c.dict[c.codes[r as usize] as usize].clone())
                            );
                        }
                    }
                }
                Err("RELATED sem relação".into())
            }
            _ => Err(format!("função DAX não suportada: {f}")),
        }
    }

    /// Valores distintos visíveis de uma coluna, em ordem.
    fn values(&self, col: (usize, usize), ctx: &Ctx) -> Res<Tab> {
        let c = self.m.column(col.0, col.1)?;
        // Na ordem em que aparecem (a do modelo).
        let codes = self.visible_codes(col.0, col.1, ctx)?;
        let rows = codes.iter().map(|k| vec![c.dict[k as usize].clone()]);
        Ok(Tab {
            cols: vec![TabCol { name: self.m.tables[col.0].columns[col.1].name.clone(), src: Some(col) }],
            rows: Rows::Values(Arc::new(rows.collect())),
        })
    }
}

/// Valores de uma coluna calculada, linha a linha (por código, se só usa uma
/// coluna).
pub fn calc_column(m: &Model, t: usize, n: &Node) -> Res<Vec<Val>> {
    let ev = Eval::new(m);
    let ctx = Ctx::default();
    // Colunas citadas.
    fn cols<'n>(n: &'n Node, out: &mut Vec<&'n str>) {
        match n {
            Node::Col(_, c) | Node::Bracket(c) => out.push(c),
            Node::Neg(a) | Node::Not(a) => cols(a, out),
            Node::Bin(_, a, b) => {
                cols(a, out);
                cols(b, out);
            }
            Node::Func(_, l) | Node::List(l) => l.iter().for_each(|x| cols(x, out)),
            Node::Vars(v, b) => {
                v.iter().for_each(|x| cols(&x.1, out));
                cols(b, out);
            }
            _ => {}
        }
    }
    let mut used = Vec::new();
    cols(n, &mut used);
    used.sort_unstable();
    used.dedup();
    let at = |row: u32| -> Val {
        let mut s = Scope { ctx: &ctx, rows: vec![RowCtx::Base(t, row)], vars: Vec::new(), depth: 0 };
        ev.scalar(n, &mut s).unwrap_or(Val::Blank)
    };
    let rows = m.rows(t);
    if let [one] = used[..]
        && let Some(c) = m.tables[t].columns.iter().position(|c| c.name.to_lowercase() == one.to_lowercase())
    {
        let col = m.column(t, c)?;
        // Uma linha de cada código.
        let ix = col.index();
        let vals: Vec<Val> = (0..col.dict.len())
            .into_par_iter()
            .map(|k| ix.rows.get(ix.start[k] as usize).map_or(Val::Blank, |&r| at(r)))
            .collect();
        return Ok(col.codes.iter().map(|&k| vals[k as usize].clone()).collect());
    }
    for c in &used {
        if let Some(i) = m.tables[t].columns.iter().position(|x| x.name.to_lowercase() == c.to_lowercase()) {
            m.column(t, i)?;
        }
    }
    Ok((0..rows as u32).into_par_iter().map(at).collect())
}

/// Soma compensada (a ordem das linhas não muda o resultado).
#[derive(Clone, Copy, Default)]
struct Sum {
    s: f64,
    c: f64,
}

impl Sum {
    fn add(&mut self, x: f64) {
        let t = self.s + x;
        self.c += if self.s.abs() >= x.abs() { (self.s - t) + x } else { (x - t) + self.s };
        self.s = t;
    }

    fn join(mut self, o: Sum) -> Sum {
        self.add(o.s);
        self.c += o.c;
        self
    }

    fn value(&self) -> f64 {
        self.s + self.c
    }
}

fn hash(v: &[(usize, Arc<Bits>)]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

#[inline]
fn pass(cols: &[(Arc<Column>, Arc<Bits>)], r: u32) -> bool {
    cols.iter().all(|(c, b)| b.get(c.codes[r as usize]))
}

fn ymd(d: f64) -> Res<(i64, i64, i64)> {
    let t = duck_core::numfmt::serial_to_datetime(d.max(0.0), false, 0).ok_or("data inválida")?;
    Ok((t.year as i64, t.month as i64, t.day as i64))
}

fn serial(y: i64, m: i64, d: i64) -> Res<f64> {
    let m0 = y * 12 + m - 1;
    let (y, m) = (m0.div_euclid(12), m0.rem_euclid(12) + 1);
    if !(1900..=9999).contains(&y) {
        return Err("data inválida".into());
    }
    Ok(duck_core::input::serial(y as i32, m as u32, 1) + (d - 1) as f64)
}

fn num(v: &Val) -> Res<f64> {
    match v {
        Val::Blank => Ok(0.0),
        Val::Num(n) => Ok(*n),
        Val::Bool(b) => Ok(*b as u8 as f64),
        Val::Str(s) => s.trim().replace(',', ".").parse().map_err(|_| "texto numa conta".to_string()),
    }
}

fn truth(v: &Val) -> Res<bool> {
    match v {
        Val::Blank => Ok(false),
        Val::Bool(b) => Ok(*b),
        Val::Num(n) => Ok(*n != 0.0),
        Val::Str(_) => Err("texto numa condição".into()),
    }
}

fn text(v: &Val) -> String {
    match v {
        Val::Blank => String::new(),
        Val::Str(s) => s.to_string(),
        Val::Num(n) => num_text(*n),
        Val::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
    }
}

/// `=` do DAX: vazio vale 0, "" e FALSO; textos sem diferenciar maiúsculas.
fn equal(a: &Val, b: &Val) -> Res<bool> {
    Ok(match (a, b) {
        (Val::Blank, Val::Blank) => true,
        (Val::Blank, x) | (x, Val::Blank) => match x {
            Val::Num(n) => *n == 0.0,
            Val::Str(s) => s.is_empty(),
            Val::Bool(b) => !b,
            Val::Blank => true,
        },
        (Val::Str(x), Val::Str(y)) => x.to_lowercase() == y.to_lowercase(),
        (Val::Num(x), Val::Num(y)) => x == y,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        _ => return Err("comparação de tipos diferentes".into()),
    })
}

fn binary(op: Op, a: &Val, b: &Val) -> Res<Val> {
    let blank = *a == Val::Blank && *b == Val::Blank;
    Ok(match op {
        Op::Add if blank => Val::Blank,
        Op::Sub if blank => Val::Blank,
        Op::Add => Val::Num(num(a)? + num(b)?),
        Op::Sub => Val::Num(num(a)? - num(b)?),
        Op::Mul if *a == Val::Blank || *b == Val::Blank => Val::Blank,
        Op::Mul => Val::Num(num(a)? * num(b)?),
        Op::Div => {
            if *a == Val::Blank {
                return Ok(Val::Blank);
            }
            let d = num(b)?;
            if d == 0.0 {
                return Err("divisão por zero".into());
            }
            Val::Num(num(a)? / d)
        }
        Op::Pow => Val::Num(num(a)?.powf(num(b)?)),
        Op::Concat => Val::Str((text(a) + &text(b)).into()),
        Op::Eq => Val::Bool(equal(a, b)?),
        Op::Ne => Val::Bool(!equal(a, b)?),
        Op::StrictEq => Val::Bool(a.key() == b.key()),
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            let o = match (a, b) {
                (Val::Str(x), Val::Str(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
                (Val::Str(x), Val::Blank) => x.to_lowercase().as_str().cmp(""),
                (Val::Blank, Val::Str(y)) => "".cmp(y.to_lowercase().as_str()),
                (Val::Str(_), _) | (_, Val::Str(_)) => return Err("comparação de tipos diferentes".into()),
                (x, y) => num(x)?.total_cmp(&num(y)?),
            };
            Val::Bool(match op {
                Op::Lt => o.is_lt(),
                Op::Le => o.is_le(),
                Op::Gt => o.is_gt(),
                _ => o.is_ge(),
            })
        }
        Op::And | Op::Or | Op::In => return Err("operador".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Source};

    struct Data(Vec<Vec<Vec<Val>>>);

    impl Source for Data {
        fn rows(&self, t: usize) -> usize {
            self.0[t][0].len()
        }
        fn column(&self, t: usize, c: usize) -> Vec<Val> {
            self.0[t].get(c).cloned().unwrap_or_default()
        }
    }

    fn model(measures: &[(&str, &str)]) -> Model {
        let s = |v: &[&str]| v.iter().map(|x| if x.is_empty() { Val::Blank } else { Val::str(x) }).collect();
        let n = |v: &[f64]| v.iter().map(|x| Val::Num(*x)).collect::<Vec<_>>();
        let vendas: Vec<Vec<Val>> = vec![
            s(&["1", "2", "1", "3", "9"]),
            n(&[10.0, 20.0, 30.0, 40.0, 5.0]),
            s(&["a", "B", "b", "", "a"]),
            n(&[46_000.0, 46_010.0, 46_040.0, 46_041.0, 46_000.0]),
        ];
        let lojas: Vec<Vec<Val>> = vec![s(&["1", "2", "3"]), s(&["SP", "RJ", "SP"])];
        let c = |n: &str, k: Kind| (n.to_string(), k, None);
        Model::new(
            vec![
                (
                    "Vendas".into(),
                    vec![
                        c("Loja", Kind::Text),
                        c("Valor", Kind::Number),
                        c("Tipo", Kind::Text),
                        c("Data", Kind::Date),
                        ("Ano".into(), Kind::Text, Some("FORMAT([Data], \"yyyy\")".into())),
                    ],
                ),
                ("Lojas".into(), vec![c("Cod", Kind::Text), c("UF", Kind::Text)]),
            ],
            &[("Vendas".into(), "Loja".into(), "Lojas".into(), "Cod".into())],
            measures.iter().map(|(n, e)| ("Vendas".into(), n.to_string(), e.to_string())).collect(),
            Box::new(Data(vec![vendas, lojas])),
            46_045.5,
        )
    }

    fn get(m: &Model, name: &str, filters: &[(&str, &str, &str)]) -> Val {
        let ev = Eval::new(m);
        let mut ctx = Ctx::default();
        for (t, c, v) in filters {
            let col = m.find(t, c).unwrap();
            let d = m.column(col.0, col.1).unwrap();
            ctx.restrict(col, d.select(|x| x.key() == Val::str(v).key()));
        }
        ev.measure(name, &ctx).unwrap()
    }

    #[test]
    fn aggregates_and_relationships() {
        let m = model(&[
            ("soma", "SUM(Vendas[Valor])"),
            ("linhas", "COUNTROWS(Vendas)"),
            ("tipos", "DISTINCTCOUNT(Vendas[Tipo])"),
            ("media", "AVERAGE(Vendas[Valor])"),
            ("conta", "COUNTA(Vendas[Tipo])"),
            ("maior", "MAX(Vendas[Data])"),
        ]);
        assert_eq!(get(&m, "soma", &[]), Val::Num(105.0));
        // Filtro que chega pela relação: a loja 9 não existe em Lojas.
        assert_eq!(get(&m, "soma", &[("Lojas", "UF", "SP")]), Val::Num(80.0));
        assert_eq!(get(&m, "linhas", &[("Lojas", "UF", "RJ")]), Val::Num(1.0));
        // "B" e "b" são o mesmo valor; o vazio conta.
        assert_eq!(get(&m, "tipos", &[]), Val::Num(3.0));
        assert_eq!(get(&m, "conta", &[]), Val::Num(4.0));
        assert_eq!(get(&m, "media", &[("Vendas", "Tipo", "b")]), Val::Num(25.0));
        assert_eq!(get(&m, "maior", &[]), Val::Num(46_041.0));
        assert_eq!(get(&m, "soma", &[("Vendas", "Tipo", "a"), ("Lojas", "UF", "RJ")]), Val::Blank);
        assert_eq!(get(&m, "linhas", &[("Vendas", "Ano", "2025")]), Val::Num(2.0 + 1.0));
    }

    #[test]
    fn calculate_and_variables() {
        let m = model(&[
            ("sem vazio", "CALCULATE(DISTINCTCOUNT(Vendas[Tipo]), Vendas[Tipo] <> \"\")"),
            ("não vazio", "CALCULATE(COUNTROWS(Vendas), Vendas[Tipo] <> BLANK())"),
            ("tudo", "CALCULATE(SUM(Vendas[Valor]), ALL(Vendas[Tipo]))"),
            ("parte", "DIVIDE(SUM(Vendas[Valor]), [tudo])"),
            (
                "mês",
                "VAR h = TODAY() VAR i = EOMONTH(h, -1) + 1 VAR f = EOMONTH(h, 0) RETURN CALCULATE(SUM(Vendas[Valor]), Vendas[Data] >= i, Vendas[Data] <= f)",
            ),
            ("dif", "[tudo]-[sem]"),
            ("sem", "CALCULATE(SUM(Vendas[Valor]), Vendas[Tipo] = \"zzz\")"),
            ("lista", "CONCATENATEX(VALUES(Vendas[Tipo]), Vendas[Tipo], \", \")"),
            ("x", "SUMX(Vendas, IFERROR(VALUE(Vendas[Loja]), 0))"),
            (
                "união",
                "VAR t = CALCULATETABLE(Vendas, Vendas[Tipo] = \"a\") VAR u = UNION(SELECTCOLUMNS(t, \"v\", Vendas[Loja]), SELECTCOLUMNS(t, \"v\", Vendas[Tipo])) VAR d = FILTER(DISTINCT(u), [v] <> \"-\" && [v] <> BLANK()) RETURN CONCATENATEX(d, [v], \"+\")",
            ),
        ]);
        assert_eq!(get(&m, "sem vazio", &[]), Val::Num(2.0));
        assert_eq!(get(&m, "não vazio", &[]), Val::Num(4.0));
        assert_eq!(get(&m, "parte", &[("Vendas", "Tipo", "a")]), Val::Num(15.0 / 105.0));
        // Hoje 46045 = 23/01/2026: janeiro vai de 46023 a 46053.
        assert_eq!(get(&m, "mês", &[]), Val::Num(70.0));
        assert_eq!(get(&m, "dif", &[]), Val::Num(105.0));
        assert_eq!(get(&m, "sem", &[]), Val::Blank);
        assert_eq!(get(&m, "lista", &[("Vendas", "Tipo", "b")]), Val::str("B"));
        assert_eq!(get(&m, "x", &[]), Val::Num(16.0));
        assert_eq!(get(&m, "união", &[]), Val::str("1+9+a"));
    }

    #[test]
    fn unsupported_is_an_error() {
        let m = model(&[("r", "RANKX(Vendas, 1)"), ("ruim", "SUM(")]);
        let ev = Eval::new(&m);
        assert!(ev.measure("r", &Ctx::default()).unwrap_err().contains("RANKX"));
        assert!(ev.measure("ruim", &Ctx::default()).is_err());
        assert!(ev.measure("não existe", &Ctx::default()).is_err());
    }
}
