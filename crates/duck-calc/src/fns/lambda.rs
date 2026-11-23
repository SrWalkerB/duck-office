//! LAMBDA e as funções que chamam outra função (MAP, REDUCE, SCAN, BYROW,
//! BYCOL, MAKEARRAY), e as de quebrar vetores em linhas ou colunas.
//!
//! `LAMBDA(...)` é compilada direto ([`crate::parse::Node::Lambda`]); aqui
//! fica a entrada da tabela de funções.

use std::rc::Rc;

use duck_core::CellError;

use super::matrix;
use crate::eval::{Eval, Res};
use crate::funcs::{FnDef, n};
use crate::parse::Node;
use crate::value::{Arr, Fun, Rng, V};

pub const DEFS: &[FnDef] = &[
    n("LAMBDA", "LAMBDA", 1, 255, lambda, "parâmetro_ou_cálculo; …", "Cria uma função"),
    n("MAP", "MAP", 2, 255, map, "matriz1; lambda_ou_matriz2; …", "Aplica uma função a cada item"),
    n("REDUCE", "REDUCE", 3, 3, reduce, "valor_inicial; matriz; lambda", "Junta os itens num valor só"),
    n("SCAN", "SCAN", 3, 3, scan, "valor_inicial; matriz; lambda", "Os valores parciais ao juntar os itens"),
    n("BYROW", "BYROW", 2, 2, byrow, "matriz; lambda", "Aplica uma função a cada linha"),
    n("BYCOL", "BYCOL", 2, 2, bycol, "matriz; lambda", "Aplica uma função a cada coluna"),
    n(
        "MAKEARRAY",
        "MAKEARRAY",
        3,
        3,
        makearray,
        "linhas; colunas; lambda",
        "Uma matriz calculada item a item",
    ),
    n("ISOMITTED", "ISOMITTED", 1, 1, isomitted, "argumento", "VERDADEIRO se o parâmetro não foi passado"),
    n(
        "WRAPROWS",
        "QUEBRARLINS",
        2,
        3,
        wraprows,
        "vetor; contagem_de_quebra; [preencher_com]",
        "Um vetor quebrado em linhas",
    ),
    n(
        "WRAPCOLS",
        "QUEBRARCOLS",
        2,
        3,
        wrapcols,
        "vetor; contagem_de_quebra; [preencher_com]",
        "Um vetor quebrado em colunas",
    ),
];

/// Maior quantidade de itens que uma matriz criada pode ter.
const MAX_ITEMS: f64 = 1_048_576.0 * 16.0;

fn lambda<'e>(_: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    Err(CellError::Value)
}

/// Argumento que é uma função.
pub fn fun_arg<'e>(e: &mut Eval<'e>, n: u32) -> Res<Rc<Fun<'e>>> {
    match e.eval(n) {
        V::Fn(f) => Ok(f),
        V::Err(x) => Err(x),
        _ => Err(CellError::Value),
    }
}

/// Matriz ou referência vista por itens. Itens de referências vão como
/// referências (`SUBTOTAL` dentro de `BYROW` vê as células).
enum Grid<'e> {
    Ref(Rng),
    Arr(Rc<Arr<'e>>),
}

impl<'e> Grid<'e> {
    fn read(e: &mut Eval<'e>, n: u32) -> Res<Self> {
        let saved = std::mem::replace(&mut e.array, true);
        let v = e.eval(n);
        e.array = saved;
        match v {
            V::Err(x) => Err(x),
            V::Ref(r) => {
                // Colunas e linhas inteiras param no que o arquivo usa.
                let r1 = r.r1.min(r.r0.max(e.book.max_rows.saturating_sub(1)));
                let c1 = r.c1.min(r.c0.max(e.book.used_cols(r.sheet).saturating_sub(1)));
                let full = r.rows() > 100_000 || r.cols() > 10_000;
                Ok(Grid::Ref(if full { Rng { r1, c1, ..r } } else { r }))
            }
            v => Ok(Grid::Arr(e.to_arr(v))),
        }
    }

    fn dims(&self) -> (u32, u32) {
        match self {
            Grid::Ref(r) => (r.rows(), r.cols()),
            Grid::Arr(a) => (a.rows, a.cols),
        }
    }

    fn item(&self, r: u32, c: u32) -> V<'e> {
        match self {
            Grid::Ref(g) => {
                let (r, c) = (g.r0 + r, g.c0 + c);
                V::Ref(Rng { sheet: g.sheet, r0: r, r1: r, c0: c, c1: c })
            }
            Grid::Arr(a) => a.get(r, c).clone(),
        }
    }

    /// Linha (ou coluna) `i` inteira.
    fn line(&self, i: u32, by_row: bool) -> V<'e> {
        match self {
            Grid::Ref(g) => V::Ref(if by_row {
                Rng { r0: g.r0 + i, r1: g.r0 + i, ..*g }
            } else {
                Rng { c0: g.c0 + i, c1: g.c0 + i, ..*g }
            }),
            Grid::Arr(a) => V::Arr(Rc::new(if by_row {
                Arr { rows: 1, cols: a.cols, v: (0..a.cols).map(|c| a.get(i, c).clone()).collect() }
            } else {
                Arr { rows: a.rows, cols: 1, v: (0..a.rows).map(|r| a.get(r, i).clone()).collect() }
            })),
        }
    }
}

/// Resultado de uma chamada que precisa ser um valor só (`#CALC!` se não for).
pub fn one<'e>(e: &Eval<'e>, v: V<'e>) -> V<'e> {
    match v {
        V::Ref(r) if r.is_cell() => e.book.get(r.sheet, r.r0, r.c0),
        V::Arr(a) if a.v.len() == 1 => a.v[0].clone(),
        V::Ref(_) | V::Arr(_) | V::Fn(_) => V::Err(CellError::Calc),
        v => v,
    }
}

fn map<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let last = a.len() - 1;
    let fun = fun_arg(e, a[last])?;
    let grids: Vec<Grid> = a[..last].iter().map(|&n| Grid::read(e, n)).collect::<Res<_>>()?;
    let rows = grids.iter().map(|g| g.dims().0).max().unwrap_or(0);
    let cols = grids.iter().map(|g| g.dims().1).max().unwrap_or(0);
    let mut v = Vec::with_capacity((rows * cols) as usize);
    for r in 0..rows {
        for c in 0..cols {
            let args = grids
                .iter()
                .map(|g| {
                    let (gr, gc) = g.dims();
                    if r < gr && c < gc { g.item(r, c) } else { V::Err(CellError::NA) }
                })
                .collect();
            let x = e.call(&fun, args);
            v.push(one(e, x));
        }
    }
    matrix(rows, cols, v)
}

fn reduce<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let fun = fun_arg(e, a[2])?;
    let g = Grid::read(e, a[1])?;
    let (rows, cols) = g.dims();
    // Sem valor inicial, começa pelo primeiro item.
    let mut acc = if e.is_missing(a[0]) { None } else { Some(e.eval(a[0])) };
    for r in 0..rows {
        for c in 0..cols {
            let x = g.item(r, c);
            acc = Some(match acc {
                None => x,
                Some(p) => e.call(&fun, vec![p, x]),
            });
        }
    }
    Ok(match acc {
        Some(V::Fn(_)) | None => V::Err(CellError::Calc),
        Some(v) => v,
    })
}

fn scan<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let fun = fun_arg(e, a[2])?;
    let g = Grid::read(e, a[1])?;
    let (rows, cols) = g.dims();
    let mut acc = if e.is_missing(a[0]) { None } else { Some(e.scalar(a[0])) };
    let mut v = Vec::with_capacity((rows * cols) as usize);
    for r in 0..rows {
        for c in 0..cols {
            let x = g.item(r, c);
            let next = match acc {
                None => x,
                Some(p) => e.call(&fun, vec![p, x]),
            };
            let next = one(e, next);
            v.push(next.clone());
            acc = Some(next);
        }
    }
    matrix(rows, cols, v)
}

fn by_line<'e>(e: &mut Eval<'e>, a: &[u32], by_row: bool) -> Res<V<'e>> {
    let fun = fun_arg(e, a[1])?;
    let g = Grid::read(e, a[0])?;
    let (rows, cols) = g.dims();
    let count = if by_row { rows } else { cols };
    let mut v = Vec::with_capacity(count as usize);
    for i in 0..count {
        let x = e.call(&fun, vec![g.line(i, by_row)]);
        v.push(one(e, x));
    }
    if by_row { matrix(rows, 1, v) } else { matrix(1, cols, v) }
}

fn byrow<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    by_line(e, a, true)
}

fn bycol<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    by_line(e, a, false)
}

fn makearray<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let rows = e.num(a[0])?.trunc();
    let cols = e.num(a[1])?.trunc();
    let fun = fun_arg(e, a[2])?;
    if rows < 1.0 || cols < 1.0 || rows * cols > MAX_ITEMS {
        return Err(CellError::Value);
    }
    let (rows, cols) = (rows as u32, cols as u32);
    let mut v = Vec::with_capacity((rows * cols) as usize);
    for r in 0..rows {
        for c in 0..cols {
            let x = e.call(&fun, vec![V::Num(r as f64 + 1.0), V::Num(c as f64 + 1.0)]);
            v.push(one(e, x));
        }
    }
    matrix(rows, cols, v)
}

fn isomitted<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(match e.node(a[0]) {
        Some(Node::Local(k)) => matches!(e.local(*k), V::Fn(f) if matches!(*f, Fun::Omitted)),
        Some(Node::Missing) => true,
        _ => false,
    }))
}

fn wrap<'e>(e: &mut Eval<'e>, a: &[u32], by_row: bool) -> Res<V<'e>> {
    let arr = e.arr(a[0])?;
    let k = e.num(a[1])?.trunc();
    if arr.rows > 1 && arr.cols > 1 {
        return Err(CellError::Value);
    }
    if k < 1.0 {
        return Err(CellError::Num);
    }
    let pad = match a.get(2) {
        Some(&n) if !e.is_missing(n) => e.scalar(n),
        _ => V::Err(CellError::NA),
    };
    let len = arr.v.len();
    let k = (k as usize).min(len);
    let lines = len.div_ceil(k);
    let at = |i: usize| arr.v.get(i).cloned().unwrap_or_else(|| pad.clone());
    if by_row {
        return matrix(lines as u32, k as u32, (0..lines * k).map(at).collect());
    }
    let mut v = Vec::with_capacity(lines * k);
    for r in 0..k {
        v.extend((0..lines).map(|c| at(c * k + r)));
    }
    matrix(k as u32, lines as u32, v)
}

fn wraprows<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    wrap(e, a, true)
}

fn wrapcols<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    wrap(e, a, false)
}
