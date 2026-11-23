//! Matrizes: multiplicação, inversa, determinante; somas de pares.

use duck_core::CellError;

use super::{matrix, pairs};
use crate::eval::{Eval, Res};
use crate::funcs::{Acc, FnDef, n};
use crate::value::V;

pub const DEFS: &[FnDef] = &[
    n("MMULT", "MATRIZ.MULT", 2, 2, mmult, "matriz1; matriz2", "O produto de duas matrizes"),
    n("MINVERSE", "MATRIZ.INVERSO", 1, 1, minverse, "matriz", "A matriz inversa"),
    n("MDETERM", "MATRIZ.DETERM", 1, 1, mdeterm, "matriz", "O determinante"),
    n("MUNIT", "MUNIT", 1, 1, munit, "dimensão", "A matriz identidade"),
    n("SUMX2MY2", "SOMAX2DY2", 2, 2, sumx2my2, "matriz_x; matriz_y", "A soma das diferenças dos quadrados"),
    n("SUMX2PY2", "SOMAX2SY2", 2, 2, sumx2py2, "matriz_x; matriz_y", "A soma das somas dos quadrados"),
    n("SUMXMY2", "SOMAXMY2", 2, 2, sumxmy2, "matriz_x; matriz_y", "A soma dos quadrados das diferenças"),
];

/// Maior lado aceito na inversa e no determinante.
const MAX_SIDE: usize = 1000;

struct Mat {
    rows: usize,
    cols: usize,
    v: Vec<f64>,
}

fn mat(e: &mut Eval, n: u32) -> Res<Mat> {
    let a = e.arr(n)?;
    let v =
        a.v.iter()
            .map(|x| match x {
                V::Num(x) => Ok(*x),
                V::Err(e) => Err(*e),
                _ => Err(CellError::Value),
            })
            .collect::<Res<Vec<f64>>>()?;
    Ok(Mat { rows: a.rows as usize, cols: a.cols as usize, v })
}

fn square(e: &mut Eval, n: u32) -> Res<Mat> {
    let m = mat(e, n)?;
    if m.rows != m.cols || m.rows > MAX_SIDE { Err(CellError::Value) } else { Ok(m) }
}

fn mmult<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = mat(e, a[0])?;
    let y = mat(e, a[1])?;
    if x.cols != y.rows {
        return Err(CellError::Value);
    }
    let mut v = Vec::with_capacity(x.rows * y.cols);
    for r in 0..x.rows {
        for c in 0..y.cols {
            let mut acc = Acc::default();
            for k in 0..x.cols {
                acc.add(x.v[r * x.cols + k] * y.v[k * y.cols + c]);
            }
            v.push(V::num(acc.get()));
        }
    }
    matrix(x.rows as u32, y.cols as u32, v)
}

/// Eliminação com pivô parcial em `m` (e as mesmas contas em `right`).
/// Devolve o determinante.
fn eliminate(m: &mut Mat, mut right: Option<&mut Mat>) -> f64 {
    let n = m.rows;
    let mut det = 1.0;
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| m.v[i * n + c].abs().total_cmp(&m.v[j * n + c].abs())).unwrap_or(c);
        let pivot = m.v[p * n + c];
        // Pivô que é só resto de arredondamento: matriz singular.
        let scale = (0..n).map(|k| m.v[p * n + k].abs()).fold(0.0, f64::max);
        if pivot == 0.0 || pivot.abs() < scale * 1e-16 {
            return 0.0;
        }
        if p != c {
            det = -det;
            for k in 0..n {
                m.v.swap(p * n + k, c * n + k);
            }
            if let Some(r) = right.as_deref_mut() {
                for k in 0..n {
                    r.v.swap(p * n + k, c * n + k);
                }
            }
        }
        det *= pivot;
        for i in 0..n {
            // Sem inversa basta zerar abaixo da diagonal.
            if i == c || (right.is_none() && i < c) {
                continue;
            }
            let f = m.v[i * n + c] / pivot;
            if f == 0.0 {
                continue;
            }
            for k in c..n {
                m.v[i * n + k] -= f * m.v[c * n + k];
            }
            if let Some(r) = right.as_deref_mut() {
                for k in 0..n {
                    r.v[i * n + k] -= f * r.v[c * n + k];
                }
            }
        }
        if let Some(r) = right.as_deref_mut() {
            for k in 0..n {
                r.v[c * n + k] /= pivot;
            }
            for k in c..n {
                m.v[c * n + k] /= pivot;
            }
        }
    }
    det
}

fn minverse<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut m = square(e, a[0])?;
    let n = m.rows;
    let mut inv = Mat { rows: n, cols: n, v: vec![0.0; n * n] };
    for i in 0..n {
        inv.v[i * n + i] = 1.0;
    }
    if eliminate(&mut m, Some(&mut inv)) == 0.0 {
        return Err(CellError::Num);
    }
    matrix(n as u32, n as u32, inv.v.into_iter().map(V::num).collect())
}

fn mdeterm<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut m = square(e, a[0])?;
    Ok(V::num(eliminate(&mut m, None)))
}

fn munit<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let n = e.num(a[0])?.trunc();
    if !(1.0..=MAX_SIDE as f64).contains(&n) {
        return Err(CellError::Value);
    }
    let n = n as u32;
    matrix(n, n, (0..n * n).map(|i| V::Num(if i / n == i % n { 1.0 } else { 0.0 })).collect())
}

fn pair_sum<'e>(e: &mut Eval<'e>, a: &[u32], f: impl Fn(f64, f64) -> f64) -> Res<V<'e>> {
    let p = pairs(e, a[0], a[1])?;
    if p.is_empty() {
        return Err(CellError::Div0);
    }
    let mut acc = Acc::default();
    p.iter().for_each(|&(x, y)| acc.add(f(x, y)));
    Ok(V::num(acc.get()))
}

fn sumx2my2<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    pair_sum(e, a, |x, y| x * x - y * y)
}

fn sumx2py2<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    pair_sum(e, a, |x, y| x * x + y * y)
}

fn sumxmy2<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    pair_sum(e, a, |x, y| (x - y) * (x - y))
}
