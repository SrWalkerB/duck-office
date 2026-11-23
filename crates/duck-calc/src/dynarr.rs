//! Funções de matriz dinâmica (F5): FILTRO, ÚNICO, CLASSIFICAR,
//! CLASSIFICARPOR, SEQUÊNCIA, MATRIZALEATÓRIA, LET e as de recortar e
//! empilhar (TOMAR, DESCARTAR, ESCOLHERCOLS, ESCOLHERLINS, EMPILHARV,
//! EMPILHARH, PARACOL, PARALIN, EXPANDIR, TRANSPOR).
//!
//! Devolvem matrizes (`V::Arr`); numa célula, o resultado se espalha pelas
//! vizinhas (veja `recalc`). Os argumentos são avaliados em modo matricial
//! (operações elemento a elemento), como no Excel.

use std::cmp::Ordering;
use std::rc::Rc;

use duck_core::CellError;

use crate::eval::{Eval, Res};
use crate::value::{Arr, V, compare};

/// Argumento como matriz (em modo matricial).
fn arr_of<'e>(e: &mut Eval<'e>, n: u32) -> Res<Rc<Arr<'e>>> {
    let saved = e.array;
    e.array = true;
    let v = e.eval(n);
    e.array = saved;
    if let V::Err(x) = v {
        return Err(x);
    }
    Ok(e.to_arr(v))
}

fn out<'e>(rows: u32, cols: u32, v: Vec<V<'e>>) -> Res<V<'e>> {
    if rows == 0 || cols == 0 || v.is_empty() {
        return Err(CellError::Calc);
    }
    Ok(V::Arr(Rc::new(Arr { rows, cols, v })))
}

fn cell<'e>(a: &Arr<'e>, r: u32, c: u32) -> V<'e> {
    a.v.get((r * a.cols + c) as usize).cloned().unwrap_or(V::Err(CellError::NA))
}

fn opt_bool(e: &mut Eval, a: &[u32], i: usize) -> Res<bool> {
    match a.get(i) {
        Some(&n) if !e.is_missing(n) => e.boolean(n),
        _ => Ok(false),
    }
}

/// FILTRO(matriz; incluir; [se_vazio]).
pub fn filter<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let inc = arr_of(e, a[1])?;
    let keep = |v: &V| -> Res<bool> {
        match v {
            V::Err(x) => Err(*x),
            V::Num(x) => Ok(*x != 0.0),
            V::Bool(b) => Ok(*b),
            V::Empty => Ok(false),
            _ => Err(CellError::Value),
        }
    };
    let mut v = Vec::new();
    let (rows, cols) = if inc.cols == 1 && inc.rows == arr.rows {
        let mut n = 0;
        for r in 0..arr.rows {
            if keep(&inc.v[r as usize])? {
                n += 1;
                v.extend((0..arr.cols).map(|c| cell(&arr, r, c)));
            }
        }
        (n, arr.cols)
    } else if inc.rows == 1 && inc.cols == arr.cols {
        let cs: Vec<u32> = (0..arr.cols)
            .filter_map(|c| keep(&inc.v[c as usize]).map(|k| k.then_some(c)).transpose())
            .collect::<Res<_>>()?;
        for r in 0..arr.rows {
            v.extend(cs.iter().map(|&c| cell(&arr, r, c)));
        }
        (arr.rows, cs.len() as u32)
    } else {
        return Err(CellError::Value);
    };
    if v.is_empty() {
        return match a.get(2) {
            Some(&n) if !e.is_missing(n) => Ok(e.eval(n)),
            _ => Err(CellError::Calc),
        };
    }
    out(rows, cols, v)
}

/// Chave de comparação (texto sem diferenciar maiúsculas).
#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) enum Key {
    Empty,
    N(u64),
    S(String),
    B(bool),
    E(u8),
}

pub(crate) fn key(v: &V) -> Key {
    match v {
        V::Empty => Key::Empty,
        V::Num(x) => Key::N(if *x == 0.0 { 0 } else { x.to_bits() }),
        V::Str(s) => Key::S(s.to_lowercase()),
        V::Bool(b) => Key::B(*b),
        V::Err(x) => Key::E(*x as u8),
        _ => Key::Empty,
    }
}

/// ÚNICO(matriz; [por_col]; [ocorrer_uma_vez]).
pub fn unique<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let by_col = opt_bool(e, a, 1)?;
    let once = opt_bool(e, a, 2)?;
    let (n, len) = if by_col { (arr.cols, arr.rows) } else { (arr.rows, arr.cols) };
    let line = |i: u32| -> Vec<V<'e>> {
        (0..len).map(|j| if by_col { cell(&arr, j, i) } else { cell(&arr, i, j) }).collect()
    };
    let mut seen: std::collections::HashMap<Vec<Key>, (usize, u32)> = std::collections::HashMap::new();
    let mut order: Vec<Vec<V<'e>>> = Vec::new();
    for i in 0..n {
        let l = line(i);
        let k: Vec<Key> = l.iter().map(key).collect();
        match seen.get_mut(&k) {
            Some(s) => s.1 += 1,
            None => {
                seen.insert(k, (order.len(), 1));
                order.push(l);
            }
        }
    }
    let kept: Vec<Vec<V<'e>>> = if once {
        order
            .into_iter()
            .filter(|l| seen.get(&l.iter().map(key).collect::<Vec<_>>()).is_some_and(|s| s.1 == 1))
            .collect()
    } else {
        order
    };
    let m = kept.len() as u32;
    if m == 0 {
        return Err(CellError::Calc);
    }
    if by_col {
        let mut v = vec![V::Empty; (len * m) as usize];
        for (c, l) in kept.into_iter().enumerate() {
            for (r, x) in l.into_iter().enumerate() {
                v[r * m as usize + c] = x;
            }
        }
        out(len, m, v)
    } else {
        out(m, len, kept.into_iter().flatten().collect())
    }
}

/// Ordem de duas linhas por um valor: vazios por último, como no Excel.
pub(crate) fn sort_cmp(x: &V, y: &V, desc: bool) -> Ordering {
    match (x, y) {
        (V::Empty, V::Empty) => Ordering::Equal,
        (V::Empty, _) => Ordering::Greater,
        (_, V::Empty) => Ordering::Less,
        _ => {
            let o = match (x, y) {
                (V::Err(_), V::Err(_)) => Ordering::Equal,
                (V::Err(_), _) => Ordering::Greater,
                (_, V::Err(_)) => Ordering::Less,
                _ => compare(x, y),
            };
            if desc { o.reverse() } else { o }
        }
    }
}

/// Reordena linhas (ou colunas) de `arr` pela permutação `perm`.
fn permute<'e>(arr: &Arr<'e>, perm: &[u32], by_col: bool) -> Res<V<'e>> {
    let mut v = Vec::with_capacity(arr.v.len());
    if by_col {
        for r in 0..arr.rows {
            v.extend(perm.iter().map(|&c| cell(arr, r, c)));
        }
    } else {
        for &r in perm {
            v.extend((0..arr.cols).map(|c| cell(arr, r, c)));
        }
    }
    out(arr.rows, arr.cols, v)
}

/// CLASSIFICAR(matriz; [índice]; [ordem]; [por_col]).
pub fn sort<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let idx = e.opt_num(a, 1, 1.0)?.trunc() as i64;
    let order = e.opt_num(a, 2, 1.0)?.trunc() as i64;
    let by_col = opt_bool(e, a, 3)?;
    if order != 1 && order != -1 {
        return Err(CellError::Value);
    }
    let (n, len) = if by_col { (arr.cols, arr.rows) } else { (arr.rows, arr.cols) };
    if idx < 1 || idx as u32 > len {
        return Err(CellError::Value);
    }
    let k = idx as u32 - 1;
    let at = |i: u32| if by_col { cell(&arr, k, i) } else { cell(&arr, i, k) };
    let mut perm: Vec<u32> = (0..n).collect();
    perm.sort_by(|&x, &y| sort_cmp(&at(x), &at(y), order == -1));
    permute(&arr, &perm, by_col)
}

/// CLASSIFICARPOR(matriz; por1; [ordem1]; por2; [ordem2]...).
pub fn sortby<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let mut keys: Vec<(Rc<Arr<'e>>, bool)> = Vec::new();
    let mut i = 1;
    while i < a.len() {
        let by = arr_of(e, a[i])?;
        let order = match a.get(i + 1) {
            Some(&n) if !e.is_missing(n) => e.num(n)?.trunc() as i64,
            _ => 1,
        };
        if order != 1 && order != -1 {
            return Err(CellError::Value);
        }
        keys.push((by, order == -1));
        i += 2;
    }
    // Vetores verticais ordenam linhas; horizontais, colunas.
    let by_col = keys.first().is_some_and(|k| k.0.rows == 1 && k.0.cols > 1);
    let n = if by_col { arr.cols } else { arr.rows };
    for (k, _) in &keys {
        let len = if by_col { k.cols } else { k.rows };
        if len != n || (if by_col { k.rows } else { k.cols }) != 1 {
            return Err(CellError::Value);
        }
    }
    let mut perm: Vec<u32> = (0..n).collect();
    perm.sort_by(|&x, &y| {
        for (k, desc) in &keys {
            let o = sort_cmp(&k.v[x as usize], &k.v[y as usize], *desc);
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    });
    permute(&arr, &perm, by_col)
}

/// SEQUÊNCIA(linhas; [colunas]; [início]; [passo]).
pub fn sequence<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let rows = e.num(a[0])?.trunc();
    let cols = e.opt_num(a, 1, 1.0)?.trunc();
    let start = e.opt_num(a, 2, 1.0)?;
    let step = e.opt_num(a, 3, 1.0)?;
    if rows < 1.0 || cols < 1.0 {
        return Err(if rows == 0.0 || cols == 0.0 { CellError::Calc } else { CellError::Value });
    }
    if rows * cols > 1_048_576.0 * 16.0 {
        return Err(CellError::Num);
    }
    let (r, c) = (rows as u32, cols as u32);
    out(r, c, (0..r * c).map(|i| V::Num(start + step * i as f64)).collect())
}

/// MATRIZALEATÓRIA([linhas]; [colunas]; [mín]; [máx]; [inteiro]).
pub fn randarray<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let rows = e.opt_num(a, 0, 1.0)?.trunc();
    let cols = e.opt_num(a, 1, 1.0)?.trunc();
    let lo = e.opt_num(a, 2, 0.0)?;
    let hi = e.opt_num(a, 3, 1.0)?;
    let int = opt_bool(e, a, 4)?;
    if rows < 1.0 || cols < 1.0 || lo > hi || rows * cols > 1_000_000.0 {
        return Err(CellError::Value);
    }
    let (r, c) = (rows as u32, cols as u32);
    let v = (0..r * c)
        .map(|_| {
            let u = crate::funcs::random01();
            V::Num(if int {
                (lo.ceil() + (u * (hi.floor() - lo.ceil() + 1.0)).floor()).min(hi.floor())
            } else {
                lo + u * (hi - lo)
            })
        })
        .collect();
    out(r, c, v)
}

/// LET(nome1; valor1; [nome2; valor2...]; cálculo): os nomes viram
/// variáveis locais (o compilador os troca por posições).
pub fn let_<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let n = a.len();
    if n < 3 || n.is_multiple_of(2) {
        return Err(CellError::Value);
    }
    let mut base = None;
    for k in (1..n - 1).step_by(2) {
        // O lugar da variável é reservado antes: o valor dela pode ter outro
        // LET ou LAMBDA, com variáveis próprias depois.
        let at = e.push_local();
        base.get_or_insert(at);
        let v = e.eval(a[k]);
        e.set_local(at, v);
    }
    let r = e.eval(a[n - 1]);
    e.pop_locals(base.unwrap_or(0));
    Ok(r)
}

/// Índices escolhidos (base 1, negativos contam do fim).
fn pick(n: u32, k: f64) -> Res<u32> {
    let k = k.trunc() as i64;
    let i = if k > 0 {
        k - 1
    } else if k < 0 {
        n as i64 + k
    } else {
        -1
    };
    if i < 0 || i >= n as i64 { Err(CellError::Value) } else { Ok(i as u32) }
}

fn choose<'e>(e: &mut Eval<'e>, a: &[u32], cols: bool) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let mut idx = Vec::new();
    for &n in &a[1..] {
        let sel = arr_of(e, n)?;
        for x in &sel.v {
            let k = e.to_num(x)?;
            idx.push(pick(if cols { arr.cols } else { arr.rows }, k)?);
        }
    }
    let mut v = Vec::new();
    if cols {
        for r in 0..arr.rows {
            v.extend(idx.iter().map(|&c| cell(&arr, r, c)));
        }
        out(arr.rows, idx.len() as u32, v)
    } else {
        for &r in &idx {
            v.extend((0..arr.cols).map(|c| cell(&arr, r, c)));
        }
        out(idx.len() as u32, arr.cols, v)
    }
}

/// ESCOLHERCOLS(matriz; col1; ...).
pub fn choosecols<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    choose(e, a, true)
}

/// ESCOLHERLINS(matriz; lin1; ...).
pub fn chooserows<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    choose(e, a, false)
}

/// Faixa [i0, i1) que TOMAR (`take`) ou DESCARTAR mantém de `n` itens.
fn span(n: u32, k: Option<f64>, take: bool) -> (u32, u32) {
    let Some(k) = k else { return (0, n) };
    let k = k.trunc() as i64;
    let m = k.unsigned_abs().min(n as u64) as u32;
    match (take, k >= 0) {
        (true, true) => (0, m),
        (true, false) => (n - m, n),
        (false, true) => (m, n),
        (false, false) => (0, n - m),
    }
}

fn take_drop<'e>(e: &mut Eval<'e>, a: &[u32], take: bool) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let rows = match a.get(1) {
        Some(&n) if !e.is_missing(n) => Some(e.num(n)?),
        _ => None,
    };
    let cols = match a.get(2) {
        Some(&n) if !e.is_missing(n) => Some(e.num(n)?),
        _ => None,
    };
    if take && (rows == Some(0.0) || cols == Some(0.0)) {
        return Err(CellError::Calc);
    }
    let (r0, r1) = span(arr.rows, rows, take);
    let (c0, c1) = span(arr.cols, cols, take);
    let mut v = Vec::new();
    for r in r0..r1 {
        v.extend((c0..c1).map(|c| cell(&arr, r, c)));
    }
    out(r1.saturating_sub(r0), c1.saturating_sub(c0), v)
}

/// TOMAR(matriz; linhas; [colunas]).
pub fn take<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    take_drop(e, a, true)
}

/// DESCARTAR(matriz; linhas; [colunas]).
pub fn drop<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    take_drop(e, a, false)
}

fn stack<'e>(e: &mut Eval<'e>, a: &[u32], vertical: bool) -> Res<V<'e>> {
    let parts: Vec<Rc<Arr<'e>>> = a.iter().map(|&n| arr_of(e, n)).collect::<Res<_>>()?;
    let na = V::Err(CellError::NA);
    if vertical {
        let cols = parts.iter().map(|p| p.cols).max().unwrap_or(0);
        let rows: u32 = parts.iter().map(|p| p.rows).sum();
        let mut v = Vec::with_capacity((rows * cols) as usize);
        for p in &parts {
            for r in 0..p.rows {
                v.extend((0..cols).map(|c| if c < p.cols { cell(p, r, c) } else { na.clone() }));
            }
        }
        out(rows, cols, v)
    } else {
        let rows = parts.iter().map(|p| p.rows).max().unwrap_or(0);
        let cols: u32 = parts.iter().map(|p| p.cols).sum();
        let mut v = Vec::with_capacity((rows * cols) as usize);
        for r in 0..rows {
            for p in &parts {
                v.extend((0..p.cols).map(|c| if r < p.rows { cell(p, r, c) } else { na.clone() }));
            }
        }
        out(rows, cols, v)
    }
}

/// EMPILHARV(matriz1; ...).
pub fn vstack<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    stack(e, a, true)
}

/// EMPILHARH(matriz1; ...).
pub fn hstack<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    stack(e, a, false)
}

fn flatten<'e>(e: &mut Eval<'e>, a: &[u32], to_col: bool) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let ignore = e.opt_num(a, 1, 0.0)?.trunc() as i64;
    let by_col = opt_bool(e, a, 2)?;
    let mut v = Vec::new();
    let (n, m) = if by_col { (arr.cols, arr.rows) } else { (arr.rows, arr.cols) };
    for i in 0..n {
        for j in 0..m {
            let x = if by_col { cell(&arr, j, i) } else { cell(&arr, i, j) };
            let skip = match &x {
                V::Empty => ignore & 1 != 0,
                V::Err(_) => ignore & 2 != 0,
                _ => false,
            };
            if !skip {
                v.push(x);
            }
        }
    }
    let k = v.len() as u32;
    if to_col { out(k, 1, v) } else { out(1, k, v) }
}

/// PARACOL(matriz; [ignorar]; [por_coluna]).
pub fn tocol<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    flatten(e, a, true)
}

/// PARALIN(matriz; [ignorar]; [por_coluna]).
pub fn torow<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    flatten(e, a, false)
}

/// EXPANDIR(matriz; linhas; [colunas]; [preencher]).
pub fn expand<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let rows = e.opt_num(a, 1, arr.rows as f64)?.trunc();
    let cols = e.opt_num(a, 2, arr.cols as f64)?.trunc();
    if rows < arr.rows as f64 || cols < arr.cols as f64 {
        return Err(CellError::Value);
    }
    let pad = match a.get(3) {
        Some(&n) if !e.is_missing(n) => e.scalar(n),
        _ => V::Err(CellError::NA),
    };
    let (r, c) = (rows as u32, cols as u32);
    let mut v = Vec::with_capacity((r * c) as usize);
    for i in 0..r {
        for j in 0..c {
            v.push(if i < arr.rows && j < arr.cols { cell(&arr, i, j) } else { pad.clone() });
        }
    }
    out(r, c, v)
}

/// TRANSPOR(matriz).
pub fn transpose<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let arr = arr_of(e, a[0])?;
    let mut v = Vec::with_capacity(arr.v.len());
    for c in 0..arr.cols {
        v.extend((0..arr.rows).map(|r| cell(&arr, r, c)));
    }
    out(arr.cols, arr.rows, v)
}
