//! Estatísticas descritivas e AGREGAR.

use std::collections::HashMap;

use duck_core::CellError;

use super::{ALL, arr_nums, column, matrix, nums};
use crate::eval::{Eval, Res};
use crate::funcs::{self, Acc, FnDef, each_value, n, num_list, variance};
use crate::index::round15;
use crate::parse::flag;
use crate::value::{Sid, V};

const NUMS: &str = "núm1; [núm2]; …";
const VALS: &str = "valor1; [valor2]; …";

pub const DEFS: &[FnDef] = &[
    n("MODE", "MODO", 1, 255, mode, NUMS, "O número que mais se repete"),
    n("MODE.SNGL", "MODO.ÚNICO", 1, 255, mode, NUMS, "O número que mais se repete"),
    n("MODE.MULT", "MODO.MULT", 1, 255, mode_mult, NUMS, "Os números que mais se repetem"),
    n("PERCENTILE", "PERCENTIL", 2, 2, percentile_inc, "matriz; k", "O k-ésimo percentil").lift(0b10),
    n("PERCENTILE.INC", "PERCENTIL.INC", 2, 2, percentile_inc, "matriz; k", "O k-ésimo percentil").lift(0b10),
    n("PERCENTILE.EXC", "PERCENTIL.EXC", 2, 2, percentile_exc, "matriz; k", "O k-ésimo percentil, sem 0 e 1")
        .lift(0b10),
    n("QUARTILE", "QUARTIL", 2, 2, quartile_inc, "matriz; quarto", "O quartil").lift(0b10),
    n("QUARTILE.INC", "QUARTIL.INC", 2, 2, quartile_inc, "matriz; quarto", "O quartil").lift(0b10),
    n("QUARTILE.EXC", "QUARTIL.EXC", 2, 2, quartile_exc, "matriz; quarto", "O quartil, sem 0 e 1").lift(0b10),
    n(
        "PERCENTRANK",
        "ORDEM.PORCENTUAL",
        2,
        3,
        percentrank_inc,
        "matriz; x; [significância]",
        "A posição percentual",
    )
    .lift(0b10),
    n(
        "PERCENTRANK.INC",
        "ORDEM.PORCENTUAL.INC",
        2,
        3,
        percentrank_inc,
        "matriz; x; [significância]",
        "A posição percentual",
    )
    .lift(0b10),
    n(
        "PERCENTRANK.EXC",
        "ORDEM.PORCENTUAL.EXC",
        2,
        3,
        percentrank_exc,
        "matriz; x; [significância]",
        "A posição percentual, sem 0 e 1",
    )
    .lift(0b10),
    n(
        "RANK.AVG",
        "ORDEM.MÉD",
        2,
        3,
        rank_avg,
        "núm; ref; [ordem]",
        "A posição de um número, com média nos empates",
    )
    .lift(0b1),
    n(
        "FREQUENCY",
        "FREQUÊNCIA",
        2,
        2,
        frequency,
        "matriz_dados; matriz_bin",
        "Quantos números em cada faixa",
    ),
    n("GEOMEAN", "MÉDIA.GEOMÉTRICA", 1, 255, geomean, NUMS, "A média geométrica"),
    n("HARMEAN", "MÉDIA.HARMÔNICA", 1, 255, harmean, NUMS, "A média harmônica"),
    n("TRIMMEAN", "MÉDIA.INTERNA", 2, 2, trimmean, "matriz; porcentagem", "A média sem os extremos"),
    n("AVEDEV", "DESV.MÉDIO", 1, 255, avedev, NUMS, "A média dos desvios absolutos"),
    n("DEVSQ", "DESVQ", 1, 255, devsq, NUMS, "A soma dos quadrados dos desvios"),
    n("SKEW", "DISTORÇÃO", 1, 255, skew, NUMS, "A assimetria de uma amostra"),
    n("SKEW.P", "DISTORÇÃO.P", 1, 255, skew_p, NUMS, "A assimetria da população"),
    n("KURT", "CURT", 1, 255, kurt, NUMS, "A curtose"),
    n("STANDARDIZE", "PADRONIZAR", 3, 3, standardize, "x; média; desv_padrão", "O valor normalizado")
        .lift(ALL),
    n("MAXA", "MÁXIMOA", 1, 255, maxa, VALS, "O maior valor, contando textos e lógicos"),
    n("MINA", "MÍNIMOA", 1, 255, mina, VALS, "O menor valor, contando textos e lógicos"),
    n(
        "STDEVA",
        "DESVPADA",
        1,
        255,
        stdeva,
        VALS,
        "O desvio padrão de uma amostra, contando textos e lógicos",
    ),
    n(
        "STDEVPA",
        "DESVPADPA",
        1,
        255,
        stdevpa,
        VALS,
        "O desvio padrão da população, contando textos e lógicos",
    ),
    n("VARA", "VARA", 1, 255, vara, VALS, "A variância de uma amostra, contando textos e lógicos"),
    n("VARPA", "VARPA", 1, 255, varpa, VALS, "A variância da população, contando textos e lógicos"),
    n(
        "AGGREGATE",
        "AGREGAR",
        3,
        255,
        aggregate,
        "núm_função; opções; ref1; [ref2]; …",
        "Um total que pode ignorar erros, linhas ocultas e outros totais",
    ),
];

const NUM: CellError = CellError::Num;

fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v
}

/// Quantas vezes cada número aparece, na ordem em que aparecem.
fn repeats(v: &[f64]) -> Vec<(f64, u32)> {
    let mut at: HashMap<u64, usize> = HashMap::new();
    let mut out: Vec<(f64, u32)> = Vec::new();
    for &x in v {
        let k = if x == 0.0 { 0 } else { round15(x).to_bits() };
        match at.get(&k) {
            Some(&i) => out[i].1 += 1,
            None => {
                at.insert(k, out.len());
                out.push((x, 1));
            }
        }
    }
    out
}

fn mode<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = num_list(e, a)?;
    let r = repeats(&v);
    let top = r.iter().map(|x| x.1).max().unwrap_or(0);
    if top < 2 {
        return Err(CellError::NA);
    }
    r.iter().find(|x| x.1 == top).map(|x| V::Num(x.0)).ok_or(CellError::NA)
}

fn mode_mult<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = num_list(e, a)?;
    let r = repeats(&v);
    let top = r.iter().map(|x| x.1).max().unwrap_or(0);
    if top < 2 {
        return Err(CellError::NA);
    }
    column(r.into_iter().filter(|x| x.1 == top).map(|x| x.0))
}

/// Percentil com interpolação (0 e 1 são os extremos).
pub fn pct_inc(v: &[f64], k: f64) -> Res<f64> {
    if v.is_empty() || !(0.0..=1.0).contains(&k) {
        return Err(NUM);
    }
    Ok(at_rank(v, k * (v.len() - 1) as f64))
}

/// Percentil exclusivo.
pub fn pct_exc(v: &[f64], k: f64) -> Res<f64> {
    let n = v.len() as f64;
    let p = round15(k * (n + 1.0));
    if v.is_empty() || k <= 0.0 || k >= 1.0 || p < 1.0 || p > n {
        return Err(NUM);
    }
    Ok(at_rank(v, p - 1.0))
}

/// Valor na posição `p` (base 0, fracionária) de uma lista em ordem.
fn at_rank(v: &[f64], p: f64) -> f64 {
    let p = round15(p);
    let i = (p.floor() as usize).min(v.len() - 1);
    let f = p - i as f64;
    if f <= 0.0 || i + 1 >= v.len() { v[i] } else { v[i] + f * (v[i + 1] - v[i]) }
}

fn percentile_inc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = sorted(arr_nums(e, a[0])?);
    let k = e.num(a[1])?;
    Ok(V::num(pct_inc(&v, k)?))
}

fn percentile_exc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = sorted(arr_nums(e, a[0])?);
    let k = e.num(a[1])?;
    Ok(V::num(pct_exc(&v, k)?))
}

fn quartile_inc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = sorted(arr_nums(e, a[0])?);
    let q = e.num(a[1])?.trunc();
    if !(0.0..=4.0).contains(&q) {
        return Err(NUM);
    }
    Ok(V::num(pct_inc(&v, q / 4.0)?))
}

fn quartile_exc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = sorted(arr_nums(e, a[0])?);
    let q = e.num(a[1])?.trunc();
    if !(1.0..=3.0).contains(&q) {
        return Err(NUM);
    }
    Ok(V::num(pct_exc(&v, q / 4.0)?))
}

fn percentrank<'e>(e: &mut Eval<'e>, a: &[u32], exc: bool) -> Res<V<'e>> {
    let v = sorted(arr_nums(e, a[0])?);
    let x = e.num(a[1])?;
    let sig = e.opt_num(a, 2, 3.0)?.trunc();
    if v.is_empty() || sig < 1.0 {
        return Err(NUM);
    }
    let n = v.len();
    if x < v[0] || x > v[n - 1] {
        return Err(CellError::NA);
    }
    let rank = |less: f64| if exc { (less + 1.0) / (n as f64 + 1.0) } else { less / (n as f64 - 1.0) };
    let below = v.partition_point(|y| *y < x);
    let r = if v[below] == x {
        if n == 1 && !exc { 1.0 } else { rank(below as f64) }
    } else {
        // Entre dois valores: interpola as posições.
        let (lo, hi) = (v[below - 1], v[below]);
        let first = v.partition_point(|y| *y < lo);
        let (p, q) = (rank(first as f64), rank(below as f64));
        p + (x - lo) / (hi - lo) * (q - p)
    };
    Ok(V::num(funcs::round_digits(r, sig as i64, 2)))
}

fn percentrank_inc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    percentrank(e, a, false)
}

fn percentrank_exc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    percentrank(e, a, true)
}

fn rank_avg<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let v = num_list(e, &a[1..2])?;
    let asc = e.opt_num(a, 2, 0.0)? != 0.0;
    let same = v.iter().filter(|&&y| y == x).count();
    if same == 0 {
        return Err(CellError::NA);
    }
    let before = v.iter().filter(|&&y| if asc { y < x } else { y > x }).count();
    Ok(V::Num(before as f64 + 1.0 + (same as f64 - 1.0) / 2.0))
}

fn frequency<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let data = sorted(arr_nums(e, a[0])?);
    let bins = arr_nums(e, a[1])?;
    if bins.is_empty() {
        return column([0.0, data.len() as f64]);
    }
    let mut order: Vec<usize> = (0..bins.len()).collect();
    order.sort_by(|&i, &j| bins[i].total_cmp(&bins[j]));
    let mut out = vec![0.0; bins.len() + 1];
    let mut done = 0;
    for &i in &order {
        let end = data.partition_point(|x| *x <= bins[i]);
        out[i] = (end - done) as f64;
        done = end;
    }
    out[bins.len()] = (data.len() - done) as f64;
    column(out)
}

fn positives<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<Vec<f64>> {
    let v = num_list(e, a)?;
    if v.is_empty() || v.iter().any(|x| *x <= 0.0) { Err(NUM) } else { Ok(v) }
}

fn geomean<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = positives(e, a)?;
    let p: f64 = v.iter().product();
    let n = v.len() as f64;
    // Pelo produto quando ele cabe (exato em casos simples); senão pelos logaritmos.
    Ok(V::num(if p.is_normal() {
        p.powf(1.0 / n)
    } else {
        (v.iter().map(|x| x.ln()).sum::<f64>() / n).exp()
    }))
}

fn harmean<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = positives(e, a)?;
    Ok(V::num(v.len() as f64 / v.iter().map(|x| 1.0 / x).sum::<f64>()))
}

fn trimmean<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = sorted(arr_nums(e, a[0])?);
    let p = e.num(a[1])?;
    if v.is_empty() || !(0.0..1.0).contains(&p) {
        return Err(NUM);
    }
    let cut = (round15(v.len() as f64 * p) / 2.0).floor() as usize;
    let mid = &v[cut..v.len() - cut];
    Ok(V::num(mean(mid)))
}

pub fn mean(v: &[f64]) -> f64 {
    let mut acc = Acc::default();
    v.iter().for_each(|&x| acc.add(x));
    acc.get() / v.len() as f64
}

/// Soma das potências `k` dos desvios da média.
fn moment(v: &[f64], k: i32) -> f64 {
    let m = mean(v);
    v.iter().map(|x| (x - m).powi(k)).sum()
}

fn avedev<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = num_list(e, a)?;
    if v.is_empty() {
        return Err(NUM);
    }
    let m = mean(&v);
    Ok(V::num(v.iter().map(|x| (x - m).abs()).sum::<f64>() / v.len() as f64))
}

fn devsq<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = num_list(e, a)?;
    if v.is_empty() { Err(NUM) } else { Ok(V::num(moment(&v, 2))) }
}

fn skew<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = num_list(e, a)?;
    let n = v.len() as f64;
    if n < 3.0 {
        return Err(CellError::Div0);
    }
    let s = variance(&v, true)?.sqrt();
    if s == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(V::num(moment(&v, 3) / s.powi(3) * n / ((n - 1.0) * (n - 2.0))))
}

fn skew_p<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = num_list(e, a)?;
    let n = v.len() as f64;
    if n < 3.0 {
        return Err(CellError::Div0);
    }
    let s = variance(&v, false)?.sqrt();
    if s == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(V::num(moment(&v, 3) / s.powi(3) / n))
}

fn kurt<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = num_list(e, a)?;
    let n = v.len() as f64;
    if n < 4.0 {
        return Err(CellError::Div0);
    }
    let s2 = variance(&v, true)?;
    if s2 == 0.0 {
        return Err(CellError::Div0);
    }
    let k = moment(&v, 4) / (s2 * s2);
    Ok(V::num(
        k * n * (n + 1.0) / ((n - 1.0) * (n - 2.0) * (n - 3.0))
            - 3.0 * (n - 1.0) * (n - 1.0) / ((n - 2.0) * (n - 3.0)),
    ))
}

nums!(standardize, [x, m, s], if s <= 0.0 { Err(NUM) } else { Ok((x - m) / s) });

/// Números como nas funções terminadas em A: texto vale 0 e lógicos 0 ou 1.
fn values_a<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<Vec<f64>> {
    let mut out = Vec::new();
    let mut err = None;
    each_value(e, a, &mut |v| match v {
        V::Num(x) => out.push(*x),
        V::Bool(b) => out.push(*b as u8 as f64),
        V::Str(_) => out.push(0.0),
        V::Err(x) => err = err.or(Some(*x)),
        _ => {}
    });
    match err {
        Some(x) => Err(x),
        None => Ok(out),
    }
}

fn maxa<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(values_a(e, a)?.into_iter().reduce(f64::max).unwrap_or(0.0)))
}

fn mina<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(values_a(e, a)?.into_iter().reduce(f64::min).unwrap_or(0.0)))
}

fn stdeva<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&values_a(e, a)?, true)?.sqrt()))
}

fn stdevpa<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&values_a(e, a)?, false)?.sqrt()))
}

fn vara<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&values_a(e, a)?, true)?))
}

fn varpa<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&values_a(e, a)?, false)?))
}

/// AGREGAR(função; opções; ref1...) e AGREGAR(função; opções; matriz; k).
fn aggregate<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let func = e.num(a[0])?.trunc() as i64;
    let opt = e.opt_num(a, 1, 0.0)?.trunc() as i64;
    if !(1..=19).contains(&func) || !(0..=7).contains(&opt) {
        return Err(CellError::Value);
    }
    let skip_nested = opt <= 3;
    let skip_hidden = matches!(opt, 1 | 3 | 5 | 7);
    let skip_errors = matches!(opt, 2 | 3 | 6 | 7);
    // De 14 a 19 o último argumento é o k e a matriz pode ser uma conta.
    let refs = if func >= 14 { &a[2..3] } else { &a[2..] };
    if func >= 14 && a.len() != 4 {
        return Err(CellError::Value);
    }
    let mut vals = Vec::new();
    let mut filled = 0u64;
    let mut err = None;
    let mut take = |v: &V| {
        match v {
            V::Empty => return,
            V::Num(x) => vals.push(*x),
            V::Err(_) if skip_errors => return,
            // CONT.NÚM e CONT.VALORES não param em erros.
            V::Err(x) if func != 2 && func != 3 => err = err.or(Some(*x)),
            _ => {}
        }
        filled += 1;
    };
    for &n in refs {
        let saved = std::mem::replace(&mut e.array, true);
        let v = e.eval(n);
        e.array = saved;
        match v {
            V::Ref(r) => {
                let sheet = e.book.sheet(r.sheet);
                let (book, eng) = (e.book, e.eng);
                book.each(&r, |row, col, v| {
                    if skip_hidden && sheet.is_some_and(|s| s.row_hidden(row)) {
                        return true;
                    }
                    if skip_nested
                        && let Sid::Local(s) = r.sheet
                        && sheet.is_some_and(|s| s.formula_id(row, col) != 0)
                        && eng.formula(book, s, row, col).is_some_and(|f| f.flags & flag::SUBTOTAL != 0)
                    {
                        return true;
                    }
                    take(&v);
                    true
                });
            }
            V::Err(x) if func < 14 => return Err(x),
            V::Arr(x) if func >= 14 => x.v.iter().for_each(&mut take),
            v if func >= 14 => take(&v),
            _ => return Err(CellError::Value),
        }
    }
    if let Some(x) = err {
        return Err(x);
    }
    let sum = |v: &[f64]| {
        let mut acc = Acc::default();
        v.iter().for_each(|&x| acc.add(x));
        acc.get()
    };
    let n = vals.len();
    let r = match func {
        1 if n == 0 => return Err(CellError::Div0),
        1 => sum(&vals) / n as f64,
        2 => n as f64,
        3 => filled as f64,
        4 => vals.into_iter().reduce(f64::max).unwrap_or(0.0),
        5 => vals.into_iter().reduce(f64::min).unwrap_or(0.0),
        6 => vals.iter().product::<f64>() * if n == 0 { 0.0 } else { 1.0 },
        7 => variance(&vals, true)?.sqrt(),
        8 => variance(&vals, false)?.sqrt(),
        9 => sum(&vals),
        10 => variance(&vals, true)?,
        11 => variance(&vals, false)?,
        12 if n == 0 => return Err(NUM),
        12 => at_rank(&sorted(vals), (n - 1) as f64 / 2.0),
        13 => {
            let r = repeats(&vals);
            let top = r.iter().map(|x| x.1).max().unwrap_or(0);
            if top < 2 {
                return Err(CellError::NA);
            }
            r.iter().find(|x| x.1 == top).map_or(0.0, |x| x.0)
        }
        _ => {
            let v = sorted(vals);
            let ks = e.eval(a[3]);
            if e.is_arrayish(&ks) {
                // Um resultado por k (`AGREGAR(14;6;A:A;{1;2;3})`).
                let ks = e.to_arr(ks);
                let one = |k: &V| e.to_num(k).and_then(|k| kth(func, &v, k)).map_or_else(V::Err, V::num);
                return matrix(ks.rows, ks.cols, ks.v.iter().map(one).collect());
            }
            let k = e.to_num(&ks)?;
            kth(func, &v, k)?
        }
    };
    Ok(V::num(r))
}

/// Funções 14 a 19 de AGREGAR sobre uma lista em ordem.
fn kth(func: i64, v: &[f64], k: f64) -> Res<f64> {
    match func {
        14 | 15 => {
            let (k, n) = (k.ceil(), v.len());
            if k < 1.0 || k > n as f64 {
                return Err(NUM);
            }
            Ok(if func == 14 { v[n - k as usize] } else { v[k as usize - 1] })
        }
        16 => pct_inc(v, k),
        17 if (0.0..5.0).contains(&k) => pct_inc(v, k.trunc() / 4.0),
        18 => pct_exc(v, k),
        19 if (1.0..4.0).contains(&k) => pct_exc(v, k.trunc() / 4.0),
        _ => Err(NUM),
    }
}
