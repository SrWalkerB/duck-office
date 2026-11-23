//! Correlação e regressão.

use duck_core::CellError;

use super::{matrix, pairs};
use crate::eval::{Eval, Res};
use crate::funcs::{FnDef, n};
use crate::value::V;

const YX: &str = "val_conhecidos_y; val_conhecidos_x";
const TWO: &str = "matriz1; matriz2";

pub const DEFS: &[FnDef] = &[
    n("CORREL", "CORREL", 2, 2, correl, TWO, "O coeficiente de correlação"),
    n("PEARSON", "PEARSON", 2, 2, correl, TWO, "O coeficiente de correlação de Pearson"),
    n("COVAR", "COVAR", 2, 2, covar_p, TWO, "A covariância da população"),
    n("COVARIANCE.P", "COVARIAÇÃO.P", 2, 2, covar_p, TWO, "A covariância da população"),
    n("COVARIANCE.S", "COVARIAÇÃO.S", 2, 2, covar_s, TWO, "A covariância de uma amostra"),
    n("SLOPE", "INCLINAÇÃO", 2, 2, slope, YX, "A inclinação da reta de regressão"),
    n("INTERCEPT", "INTERCEPÇÃO", 2, 2, intercept, YX, "Onde a reta de regressão cruza o eixo y"),
    n("RSQ", "RQUAD", 2, 2, rsq, YX, "O quadrado da correlação de Pearson"),
    n("STEYX", "EPADYX", 2, 2, steyx, YX, "O erro padrão do y previsto"),
    n(
        "FORECAST",
        "PREVISÃO",
        3,
        3,
        forecast,
        "x; val_conhecidos_y; val_conhecidos_x",
        "O y previsto para um x",
    )
    .lift(0b1),
    n(
        "FORECAST.LINEAR",
        "PREVISÃO.LINEAR",
        3,
        3,
        forecast,
        "x; val_conhecidos_y; val_conhecidos_x",
        "O y previsto para um x",
    )
    .lift(0b1),
    n(
        "TREND",
        "TENDÊNCIA",
        1,
        4,
        trend,
        "val_conhecidos_y; [val_conhecidos_x]; [novos_valores_x]; [constante]",
        "Os y de uma tendência linear",
    ),
    n(
        "GROWTH",
        "CRESCIMENTO",
        1,
        4,
        growth,
        "val_conhecidos_y; [val_conhecidos_x]; [novos_valores_x]; [constante]",
        "Os y de uma tendência exponencial",
    ),
    n(
        "LINEST",
        "PROJ.LIN",
        1,
        4,
        linest,
        "val_conhecidos_y; [val_conhecidos_x]; [constante]; [estatística]",
        "Os coeficientes de uma regressão linear",
    ),
    n(
        "LOGEST",
        "PROJ.LOG",
        1,
        4,
        logest,
        "val_conhecidos_y; [val_conhecidos_x]; [constante]; [estatística]",
        "Os coeficientes de uma regressão exponencial",
    ),
];

const DIV0: CellError = CellError::Div0;

/// Somas dos desvios de pares (x, y).
struct Dev {
    n: f64,
    mx: f64,
    my: f64,
    sxx: f64,
    syy: f64,
    sxy: f64,
}

fn dev(p: &[(f64, f64)]) -> Res<Dev> {
    if p.is_empty() {
        return Err(DIV0);
    }
    let n = p.len() as f64;
    let mx = p.iter().map(|q| q.0).sum::<f64>() / n;
    let my = p.iter().map(|q| q.1).sum::<f64>() / n;
    let mut d = Dev { n, mx, my, sxx: 0.0, syy: 0.0, sxy: 0.0 };
    for (x, y) in p {
        d.sxx += (x - mx) * (x - mx);
        d.syy += (y - my) * (y - my);
        d.sxy += (x - mx) * (y - my);
    }
    Ok(d)
}

/// Os pares dos argumentos (y, x) na ordem (x, y).
fn yx(e: &mut Eval, a: &[u32]) -> Res<Dev> {
    let p: Vec<(f64, f64)> = pairs(e, a[0], a[1])?.into_iter().map(|(y, x)| (x, y)).collect();
    dev(&p)
}

fn ratio<'e>(a: f64, b: f64) -> Res<V<'e>> {
    if b == 0.0 { Err(DIV0) } else { Ok(V::num(a / b)) }
}

fn correl<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let d = yx(e, a)?;
    ratio(d.sxy, d.sxx.sqrt() * d.syy.sqrt())
}

fn covar_p<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let d = yx(e, a)?;
    ratio(d.sxy, d.n)
}

fn covar_s<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let d = yx(e, a)?;
    ratio(d.sxy, d.n - 1.0)
}

fn slope<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let d = yx(e, a)?;
    ratio(d.sxy, d.sxx)
}

fn intercept<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let d = yx(e, a)?;
    if d.sxx == 0.0 { Err(DIV0) } else { Ok(V::num(d.my - d.sxy / d.sxx * d.mx)) }
}

fn rsq<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let d = yx(e, a)?;
    ratio(d.sxy * d.sxy, d.sxx * d.syy)
}

fn steyx<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let d = yx(e, a)?;
    if d.n < 3.0 || d.sxx == 0.0 {
        return Err(DIV0);
    }
    Ok(V::num(((d.syy - d.sxy * d.sxy / d.sxx).max(0.0) / (d.n - 2.0)).sqrt()))
}

fn forecast<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let d = yx(e, &a[1..])?;
    if d.sxx == 0.0 { Err(DIV0) } else { Ok(V::num(d.my + d.sxy / d.sxx * (x - d.mx))) }
}

// ---------------------------------------------------------------------------
// Regressão com várias variáveis
// ---------------------------------------------------------------------------

/// Mínimos quadrados de `y` pelas colunas de `x` (e uma constante).
struct Fit {
    /// Um coeficiente por coluna (0 nas que repetem outras).
    coef: Vec<f64>,
    constant: f64,
    /// Erro padrão de cada coeficiente e da constante.
    se: Vec<f64>,
    se_const: f64,
    r2: f64,
    sey: f64,
    f: f64,
    df: f64,
    ssreg: f64,
    ssresid: f64,
}

fn fit(y: &[f64], x: &[Vec<f64>], with_const: bool) -> Res<Fit> {
    let n = y.len();
    let k = x.len();
    let mean = |v: &[f64]| if with_const { v.iter().sum::<f64>() / n as f64 } else { 0.0 };
    let my = mean(y);
    let mx: Vec<f64> = x.iter().map(|c| mean(c)).collect();
    // Colunas centradas e ortogonalizadas (Gram-Schmidt duas vezes); as que
    // são combinação das anteriores ficam de fora, como no Excel.
    let mut q: Vec<Vec<f64>> = Vec::new();
    let mut r: Vec<Vec<f64>> = Vec::new();
    let mut used: Vec<usize> = Vec::new();
    for (j, c) in x.iter().enumerate() {
        let mut v: Vec<f64> = c.iter().map(|a| a - mx[j]).collect();
        let norm0 = v.iter().map(|a| a * a).sum::<f64>().sqrt();
        let mut row = vec![0.0; q.len()];
        for _ in 0..2 {
            for (i, qi) in q.iter().enumerate() {
                let d: f64 = qi.iter().zip(&v).map(|(a, b)| a * b).sum();
                row[i] += d;
                for (a, b) in v.iter_mut().zip(qi) {
                    *a -= d * b;
                }
            }
        }
        let norm = v.iter().map(|a| a * a).sum::<f64>().sqrt();
        if norm <= 1e-10 * norm0 || norm0 == 0.0 {
            continue;
        }
        v.iter_mut().for_each(|a| *a /= norm);
        row.push(norm);
        q.push(v);
        r.push(row);
        used.push(j);
    }
    let m = used.len();
    let yc: Vec<f64> = y.iter().map(|a| a - my).collect();
    let qy: Vec<f64> = q.iter().map(|c| c.iter().zip(&yc).map(|(a, b)| a * b).sum()).collect();
    // R é triangular: r[j][i] = componente da coluna j no vetor i.
    let mut b = vec![0.0; m];
    for j in (0..m).rev() {
        let s: f64 = (j + 1..m).map(|c| r[c][j] * b[c]).sum();
        b[j] = (qy[j] - s) / r[j][j];
    }
    let mut coef = vec![0.0; k];
    for (j, &c) in used.iter().enumerate() {
        coef[c] = b[j];
    }
    let constant = my - coef.iter().zip(&mx).map(|(a, b)| a * b).sum::<f64>();
    let ssreg: f64 = qy.iter().map(|a| a * a).sum();
    let mut ssresid = 0.0;
    for i in 0..n {
        let p: f64 = (0..k).map(|c| coef[c] * (x[c][i] - mx[c])).sum();
        ssresid += (yc[i] - p) * (yc[i] - p);
    }
    let sstot: f64 = yc.iter().map(|a| a * a).sum();
    // Resíduo que é só arredondamento: ajuste perfeito.
    if ssresid < sstot * 1e-28 {
        ssresid = 0.0;
    }
    let df = n as f64 - m as f64 - if with_const { 1.0 } else { 0.0 };
    let s2 = if df > 0.0 { ssresid / df } else { 0.0 };
    // Inversa de R, para os erros padrão.
    let mut inv = vec![vec![0.0; m]; m];
    let mut c = 0;
    while c < m {
        for j in (0..=c).rev() {
            let s: f64 = (j + 1..=c).map(|t| r[t][j] * inv[t][c]).sum();
            inv[j][c] = (if j == c { 1.0 } else { 0.0 } - s) / r[j][j];
        }
        c += 1;
    }
    let mut se = vec![0.0; k];
    for (j, &c) in used.iter().enumerate() {
        se[c] = (s2 * inv[j].iter().map(|a| a * a).sum::<f64>()).sqrt();
    }
    let se_const = if with_const {
        let spread = |c: usize| -> f64 { inv.iter().zip(&used).map(|(row, &u)| mx[u] * row[c]).sum() };
        let t = 1.0 / n as f64 + (0..m).map(|c| spread(c).powi(2)).sum::<f64>();
        (s2 * t).sqrt()
    } else {
        0.0
    };
    Ok(Fit {
        coef,
        constant,
        se,
        se_const,
        r2: if sstot == 0.0 { 1.0 } else { ssreg / (ssreg + ssresid) },
        sey: s2.sqrt(),
        f: if m == 0 { 0.0 } else { ssreg / m as f64 / s2 },
        df,
        ssreg,
        ssresid,
    })
}

/// Todos os itens são números (senão `#VALOR!`).
fn all_nums(v: &[V]) -> Res<Vec<f64>> {
    v.iter()
        .map(|x| match x {
            V::Num(x) => Ok(*x),
            V::Err(e) => Err(*e),
            _ => Err(CellError::Value),
        })
        .collect()
}

struct Data {
    y: Vec<f64>,
    x: Vec<Vec<f64>>,
    /// y numa coluna (as variáveis são colunas).
    vertical: bool,
    /// Uma variável com o mesmo formato de y.
    single: bool,
}

/// y e x conhecidos (x omitido = 1, 2, 3...).
fn known(e: &mut Eval, a: &[u32]) -> Res<Data> {
    let ya = e.arr(a[0])?;
    let y = all_nums(&ya.v)?;
    let vertical = ya.cols == 1;
    let given = a.get(1).copied().filter(|&n| !e.is_missing(n));
    let Some(xn) = given else {
        let x = vec![(1..=y.len()).map(|i| i as f64).collect()];
        return Ok(Data { y, x, vertical: true, single: true });
    };
    let xa = e.arr(xn)?;
    let v = all_nums(&xa.v)?;
    if (xa.rows, xa.cols) == (ya.rows, ya.cols) {
        return Ok(Data { y, x: vec![v], vertical: true, single: true });
    }
    let (rows, cols) = (xa.rows as usize, xa.cols as usize);
    if vertical && rows == y.len() {
        let x = (0..cols).map(|c| (0..rows).map(|r| v[r * cols + c]).collect()).collect();
        return Ok(Data { y, x, vertical: true, single: false });
    }
    if ya.rows == 1 && cols == y.len() {
        let x = (0..rows).map(|r| v[r * cols..(r + 1) * cols].to_vec()).collect();
        return Ok(Data { y, x, vertical: false, single: false });
    }
    Err(CellError::Ref)
}

/// Valores previstos para os novos x.
fn predict<'e>(e: &mut Eval<'e>, a: &[u32], log: bool) -> Res<V<'e>> {
    let mut d = known(e, a)?;
    let with_const = e.opt_bool(a, 3, true)?;
    if log {
        if d.y.iter().any(|y| *y <= 0.0) {
            return Err(CellError::Num);
        }
        d.y.iter_mut().for_each(|y| *y = y.ln());
    }
    let f = fit(&d.y, &d.x, with_const)?;
    let done = |p: f64| V::num(if log { p.exp() } else { p });
    let new = a.get(2).copied().filter(|&n| !e.is_missing(n));
    let (src, vertical) = match new {
        Some(n) => (e.arr(n)?, d.vertical),
        // Sem novos x: os conhecidos, com o formato deles (ou o de y).
        None => {
            let given = a.get(1).copied().filter(|&n| !e.is_missing(n));
            match given {
                Some(n) => (e.arr(n)?, d.vertical),
                None => {
                    let ya = e.arr(a[0])?;
                    let v: Vec<V> =
                        (0..d.y.len()).map(|i| done(f.constant + f.coef[0] * d.x[0][i])).collect();
                    return matrix(ya.rows, ya.cols, v);
                }
            }
        }
    };
    let v = all_nums(&src.v)?;
    let (rows, cols) = (src.rows as usize, src.cols as usize);
    if d.single {
        return matrix(src.rows, src.cols, v.into_iter().map(|x| done(f.constant + f.coef[0] * x)).collect());
    }
    let k = f.coef.len();
    if vertical {
        if cols != k {
            return Err(CellError::Ref);
        }
        let out =
            (0..rows).map(|r| done(f.constant + (0..k).map(|c| f.coef[c] * v[r * cols + c]).sum::<f64>()));
        return matrix(src.rows, 1, out.collect());
    }
    if rows != k {
        return Err(CellError::Ref);
    }
    let out = (0..cols).map(|c| done(f.constant + (0..k).map(|r| f.coef[r] * v[r * cols + c]).sum::<f64>()));
    matrix(1, src.cols, out.collect())
}

fn trend<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    predict(e, a, false)
}

fn growth<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    predict(e, a, true)
}

fn coefficients<'e>(e: &mut Eval<'e>, a: &[u32], log: bool) -> Res<V<'e>> {
    let mut d = known(e, a)?;
    let with_const = e.opt_bool(a, 2, true)?;
    let stats = e.opt_bool(a, 3, false)?;
    if log {
        if d.y.iter().any(|y| *y <= 0.0) {
            return Err(CellError::Num);
        }
        d.y.iter_mut().for_each(|y| *y = y.ln());
    }
    let f = fit(&d.y, &d.x, with_const)?;
    let k = f.coef.len();
    let cols = k + 1;
    let co = |x: f64| V::num(if log { x.exp() } else { x });
    // Os coeficientes vêm da última variável para a primeira, e a constante.
    let mut v: Vec<V> = f.coef.iter().rev().map(|c| co(*c)).collect();
    v.push(if with_const { co(f.constant) } else { V::Num(if log { 1.0 } else { 0.0 }) });
    if !stats {
        return matrix(1, cols as u32, v);
    }
    let na = || V::Err(CellError::NA);
    v.extend(f.se.iter().rev().map(|s| V::num(*s)));
    v.push(if with_const { V::num(f.se_const) } else { na() });
    for (p, q) in [(f.r2, f.sey), (f.f, f.df), (f.ssreg, f.ssresid)] {
        v.push(V::num(p));
        v.push(V::num(q));
        v.extend((2..cols).map(|_| na()));
    }
    matrix(5, cols as u32, v)
}

fn linest<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    coefficients(e, a, false)
}

fn logest<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    coefficients(e, a, true)
}
