//! Financeiras: parcelas, valor do dinheiro no tempo, taxas e depreciação.

use duck_core::CellError;

use super::{ALL, arr_nums, nums};
use crate::eval::{Eval, Res};
use crate::funcs::{FnDef, date_arg, n};
use crate::index::round15;
use crate::value::V;

pub const DEFS: &[FnDef] = &[
    n(
        "PMT",
        "PGTO",
        3,
        5,
        pmt_fn,
        "taxa; nper; vp; [vf]; [tipo]",
        "A parcela de um empréstimo ou investimento",
    )
    .lift(ALL),
    n("PV", "VP", 3, 5, pv_fn, "taxa; nper; pgto; [vf]; [tipo]", "O valor presente de um investimento")
        .lift(ALL),
    n("FV", "VF", 3, 5, fv_fn, "taxa; nper; pgto; [vp]; [tipo]", "O valor futuro de um investimento")
        .lift(ALL),
    n("NPER", "NPER", 3, 5, nper_fn, "taxa; pgto; vp; [vf]; [tipo]", "O número de parcelas").lift(ALL),
    n(
        "RATE",
        "TAXA",
        3,
        6,
        rate_fn,
        "nper; pgto; vp; [vf]; [tipo]; [estimativa]",
        "A taxa de juros por período",
    )
    .lift(ALL),
    n("IPMT", "IPGTO", 4, 6, ipmt_fn, "taxa; período; nper; vp; [vf]; [tipo]", "Os juros de uma parcela")
        .lift(ALL),
    n(
        "PPMT",
        "PPGTO",
        4,
        6,
        ppmt_fn,
        "taxa; período; nper; vp; [vf]; [tipo]",
        "A amortização de uma parcela",
    )
    .lift(ALL),
    n(
        "CUMIPMT",
        "PGTOJURACUM",
        6,
        6,
        cumipmt,
        "taxa; nper; vp; início_período; final_período; tipo",
        "Os juros pagos entre dois períodos",
    )
    .lift(ALL),
    n(
        "CUMPRINC",
        "PGTOCAPACUM",
        6,
        6,
        cumprinc,
        "taxa; nper; vp; início_período; final_período; tipo",
        "A amortização paga entre dois períodos",
    )
    .lift(ALL),
    n(
        "ISPMT",
        "ÉPGTO",
        4,
        4,
        ispmt,
        "taxa; período; nper; vp",
        "Os juros de um período com amortização constante",
    )
    .lift(ALL),
    n(
        "NPV",
        "VPL",
        2,
        255,
        npv_fn,
        "taxa; valor1; [valor2]; …",
        "O valor presente líquido de um fluxo de caixa",
    ),
    n("IRR", "TIR", 1, 2, irr_fn, "valores; [estimativa]", "A taxa interna de retorno"),
    n(
        "MIRR",
        "MTIR",
        3,
        3,
        mirr,
        "valores; taxa_financ; taxa_reinvest",
        "A taxa interna de retorno modificada",
    ),
    n("XNPV", "XVPL", 3, 3, xnpv_fn, "taxa; valores; datas", "O valor presente líquido com datas"),
    n("XIRR", "XTIR", 2, 3, xirr_fn, "valores; datas; [estimativa]", "A taxa interna de retorno com datas"),
    n("FVSCHEDULE", "VFPLANO", 2, 2, fvschedule, "capital; plano", "O valor futuro com taxas variáveis"),
    n("RRI", "TAXAJURO", 3, 3, rri, "nper; vp; vf", "A taxa equivalente de crescimento").lift(ALL),
    n("PDURATION", "DURAÇÃOP", 3, 3, pduration, "taxa; vp; vf", "Quantos períodos até chegar a um valor")
        .lift(ALL),
    n("EFFECT", "EFETIVA", 2, 2, effect, "taxa_nominal; núm_por_ano", "A taxa anual efetiva").lift(ALL),
    n("NOMINAL", "NOMINAL", 2, 2, nominal, "taxa_efetiva; núm_por_ano", "A taxa anual nominal").lift(ALL),
    n("SLN", "DPD", 3, 3, sln, "custo; recuperação; vida_útil", "A depreciação linear de um período")
        .lift(ALL),
    n("SYD", "SDA", 4, 4, syd, "custo; recuperação; vida_útil; per", "A depreciação pela soma dos dígitos")
        .lift(ALL),
    n(
        "DB",
        "BD",
        4,
        5,
        db,
        "custo; recuperação; vida_útil; período; [mês]",
        "A depreciação por saldo decrescente",
    )
    .lift(ALL),
    n(
        "DDB",
        "BDD",
        4,
        5,
        ddb,
        "custo; recuperação; vida_útil; período; [fator]",
        "A depreciação por saldo decrescente duplo",
    )
    .lift(ALL),
    n(
        "VDB",
        "BDV",
        5,
        7,
        vdb,
        "custo; recuperação; vida_útil; início_período; final_período; [fator]; [sem_mudança]",
        "A depreciação entre dois períodos quaisquer",
    )
    .lift(ALL),
    n("DOLLARDE", "MOEDADEC", 2, 2, dollarde, "moeda_fracionária; fração", "Preço em fração para decimal")
        .lift(ALL),
    n("DOLLARFR", "MOEDAFRA", 2, 2, dollarfr, "moeda_decimal; fração", "Preço decimal para fração").lift(ALL),
];

type R = Res<f64>;

fn finite(x: f64) -> R {
    if x.is_finite() { Ok(x) } else { Err(CellError::Num) }
}

// ---------------------------------------------------------------------------
// Parcelas
// ---------------------------------------------------------------------------

fn begin(t: f64) -> f64 {
    if t != 0.0 { 1.0 } else { 0.0 }
}

/// (1 + r)^n - 1 sem perder precisão com taxas pequenas.
fn growth1(r: f64, n: f64) -> f64 {
    if r > -1.0 { (n * r.ln_1p()).exp_m1() } else { (1.0 + r).powf(n) - 1.0 }
}

pub fn pmt(r: f64, n: f64, pv: f64, fv: f64, t: f64) -> R {
    if n == 0.0 {
        return Err(CellError::Num);
    }
    if r == 0.0 {
        return finite(-(pv + fv) / n);
    }
    let g = growth1(r, n);
    finite(-r * (fv + pv * (g + 1.0)) / (g * (1.0 + r * begin(t))))
}

pub fn fv(r: f64, n: f64, pmt: f64, pv: f64, t: f64) -> R {
    if r == 0.0 {
        return finite(-(pv + pmt * n));
    }
    let g = growth1(r, n);
    finite(-(pv * (g + 1.0) + pmt * (1.0 + r * begin(t)) * g / r))
}

pub fn pv(r: f64, n: f64, pmt: f64, fv: f64, t: f64) -> R {
    if r == 0.0 {
        return finite(-(fv + pmt * n));
    }
    if r <= -1.0 {
        return Err(CellError::Num);
    }
    let g = growth1(r, n);
    finite(-(fv + pmt * (1.0 + r * begin(t)) * g / r) / (g + 1.0))
}

fn ipmt(r: f64, per: f64, n: f64, pv0: f64, fv0: f64, t: f64) -> R {
    if per < 1.0 || per >= n + 1.0 {
        return Err(CellError::Num);
    }
    let p = pmt(r, n, pv0, fv0, t)?;
    if t != 0.0 && per == 1.0 {
        return Ok(0.0);
    }
    if t != 0.0 {
        Ok((fv(r, per - 2.0, p, pv0, 1.0)? - p) * r)
    } else {
        Ok(fv(r, per - 1.0, p, pv0, 0.0)? * r)
    }
}

nums!(pmt_fn, [r, n, p, f = 0.0, t = 0.0], pmt(r, n, p, f, t));
nums!(pv_fn, [r, n, p, f = 0.0, t = 0.0], pv(r, n, p, f, t));
nums!(fv_fn, [r, n, p, v = 0.0, t = 0.0], fv(r, n, p, v, t));
nums!(ipmt_fn, [r, per, n, p, f = 0.0, t = 0.0], ipmt(r, per, n, p, f, t));
nums!(ppmt_fn, [r, per, n, p, f = 0.0, t = 0.0], {
    let i = ipmt(r, per, n, p, f, t)?;
    Ok(pmt(r, n, p, f, t)? - i)
});

nums!(nper_fn, [r, p, v, f = 0.0, t = 0.0], {
    if r == 0.0 {
        return if p == 0.0 { Err(CellError::Num) } else { Ok(V::num(-(v + f) / p)) };
    }
    let k = p * (1.0 + r * begin(t));
    let q = (k - f * r) / (v * r + k);
    if q <= 0.0 || r <= -1.0 { Err(CellError::Num) } else { finite(q.ln() / r.ln_1p()) }
});

nums!(rate_fn, [n, p, v, f = 0.0, t = 0.0, guess = 0.1], {
    if n <= 0.0 {
        return Err(CellError::Num);
    }
    // Valor futuro que sobra com a taxa `r`.
    let left = |r: f64| -> f64 {
        if r.abs() < 1e-12 {
            return v + p * n + f;
        }
        let g = growth1(r, n);
        v * (g + 1.0) + p * (1.0 + r * begin(t)) * g / r + f
    };
    root(left, guess, -1.0).ok_or(CellError::Num)
});

/// Raiz de `f` acima de `lo` perto de `guess`: secante a partir da
/// estimativa e, se ela se perde, procura uma troca de sinal e bissecta.
fn root(f: impl Fn(f64) -> f64, guess: f64, lo: f64) -> Option<f64> {
    let ok = |x: f64| x > lo && x.is_finite();
    if !ok(guess) {
        return None;
    }
    let (mut x0, mut y0) = (guess, f(guess));
    if y0 == 0.0 {
        return Some(x0);
    }
    let scale = y0.abs().max(1e-300);
    let mut x1 = guess + (guess.abs() * 1e-4).max(1e-5);
    for _ in 0..60 {
        let y1 = f(x1);
        if !y1.is_finite() {
            break;
        }
        if y1 == 0.0 || y1 == y0 {
            if y1.abs() <= scale * 1e-9 {
                return Some(x1);
            }
            break;
        }
        let x2 = x1 - y1 * (x1 - x0) / (y1 - y0);
        if !ok(x2) {
            break;
        }
        (x0, y0) = (x1, y1);
        x1 = x2;
        if (x1 - x0).abs() <= 1e-15 * x1.abs().max(1e-3) {
            return Some(x1);
        }
        if (x1 - x0).abs() <= 1e-13 * x1.abs().max(1e-2) && f(x1).abs() <= scale * 1e-9 {
            return Some(x1);
        }
    }
    // Troca de sinal, de perto para longe da estimativa.
    let mut pts: Vec<f64> = Vec::new();
    for k in 0..400 {
        let d = 1.02f64.powi(k) * 0.01 - 0.01;
        pts.push(guess + d);
        let left = lo + (guess - lo) * 0.97f64.powi(k);
        pts.push(left);
    }
    pts.extend((1..60).map(|k| guess + 1.5f64.powi(k)));
    pts.retain(|&x| ok(x));
    pts.sort_by(f64::total_cmp);
    pts.dedup();
    let vals: Vec<f64> = pts.iter().map(|&x| f(x)).collect();
    let mut best: Option<(f64, f64)> = None;
    for w in 0..pts.len().saturating_sub(1) {
        if vals[w].is_finite() && vals[w + 1].is_finite() && (vals[w] < 0.0) != (vals[w + 1] < 0.0) {
            let dist = |x: f64| (x - guess).abs();
            if best.is_none_or(|b| dist(pts[w]).min(dist(pts[w + 1])) < dist(b.0).min(dist(b.1))) {
                best = Some((pts[w], pts[w + 1]));
            }
        }
    }
    let (mut a, mut b) = best?;
    let neg = f(a) < 0.0;
    for _ in 0..200 {
        let m = (a + b) / 2.0;
        if m == a || m == b {
            break;
        }
        if (f(m) < 0.0) == neg {
            a = m;
        } else {
            b = m;
        }
    }
    Some((a + b) / 2.0)
}

fn cumulative<'e>(e: &mut Eval<'e>, a: &[u32], interest: bool) -> Res<V<'e>> {
    let r = e.num(a[0])?;
    let n = e.num(a[1])?.trunc();
    let v = e.num(a[2])?;
    let start = e.num(a[3])?.trunc();
    let end = e.num(a[4])?.trunc();
    let t = e.num(a[5])?;
    if r <= 0.0
        || n <= 0.0
        || v <= 0.0
        || start < 1.0
        || end < start
        || end > n
        || !(t == 0.0 || t == 1.0)
        || end - start > 1e6
    {
        return Err(CellError::Num);
    }
    let p = pmt(r, n, v, 0.0, t)?;
    let mut total = 0.0;
    for k in start as i64..=end as i64 {
        let i = ipmt(r, k as f64, n, v, 0.0, t)?;
        total += if interest { i } else { p - i };
    }
    Ok(V::num(total))
}

fn cumipmt<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    cumulative(e, a, true)
}

fn cumprinc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    cumulative(e, a, false)
}

nums!(ispmt, [r, per, n, v], if n == 0.0 { Err(CellError::Div0) } else { Ok(v * r * (per / n - 1.0)) });

// ---------------------------------------------------------------------------
// Fluxos de caixa
// ---------------------------------------------------------------------------

fn npv(r: f64, v: &[f64]) -> f64 {
    let q = 1.0 + r;
    v.iter().enumerate().map(|(k, x)| x / q.powi(k as i32 + 1)).sum()
}

fn npv_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let r = e.num(a[0])?;
    if r == -1.0 {
        return Err(CellError::Div0);
    }
    let mut v = Vec::new();
    for &n in &a[1..] {
        v.extend(arr_nums(e, n)?);
    }
    if v.is_empty() {
        return Err(CellError::Value);
    }
    Ok(V::num(npv(r, &v)))
}

/// Um valor positivo e um negativo: sem isso não há taxa.
fn both_signs(v: &[f64]) -> bool {
    v.iter().any(|x| *x > 0.0) && v.iter().any(|x| *x < 0.0)
}

fn irr_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = arr_nums(e, a[0])?;
    let guess = e.opt_num(a, 1, 0.1)?;
    if !both_signs(&v) || guess <= -1.0 {
        return Err(CellError::Num);
    }
    // Pelo desconto: v0 + v1·d + v2·d²... (d = 1/(1+r)) não estoura.
    let f = |r: f64| {
        let d = 1.0 / (1.0 + r);
        v.iter().rev().fold(0.0, |acc, x| acc * d + x)
    };
    root(f, guess, -1.0).map(V::num).ok_or(CellError::Num)
}

fn mirr<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = arr_nums(e, a[0])?;
    let fr = e.num(a[1])?;
    let rr = e.num(a[2])?;
    if !both_signs(&v) || fr == -1.0 || rr == -1.0 {
        return Err(CellError::Div0);
    }
    let n = v.len() as f64;
    let pos: Vec<f64> = v.iter().map(|x| x.max(0.0)).collect();
    let neg: Vec<f64> = v.iter().map(|x| x.min(0.0)).collect();
    let q = -npv(rr, &pos) * (1.0 + rr).powf(n) / (npv(fr, &neg) * (1.0 + fr));
    if q < 0.0 {
        return Err(CellError::Num);
    }
    Ok(V::num(q.powf(1.0 / (n - 1.0)) - 1.0))
}

/// Valores e datas (em dias a partir da primeira).
fn dated<'e>(e: &mut Eval<'e>, vals: u32, dates: u32) -> Res<Vec<(f64, f64)>> {
    let v = e.arr(vals)?;
    let d = e.arr(dates)?;
    if v.v.len() != d.v.len() {
        return Err(CellError::Num);
    }
    let mut out = Vec::with_capacity(v.v.len());
    for (x, t) in v.v.iter().zip(&d.v) {
        let x = e.to_num(x)?;
        let t = e.to_num(t)?.floor();
        if t < 0.0 {
            return Err(CellError::Num);
        }
        out.push((x, t));
    }
    let t0 = out.first().ok_or(CellError::NA)?.1;
    for p in &mut out {
        p.1 -= t0;
        if p.1 < 0.0 {
            return Err(CellError::Num);
        }
    }
    Ok(out)
}

fn xnpv(r: f64, v: &[(f64, f64)]) -> f64 {
    let l = r.ln_1p();
    v.iter().map(|(x, t)| x * (-l * t / 365.0).exp()).sum()
}

fn xnpv_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let r = e.num(a[0])?;
    let v = dated(e, a[1], a[2])?;
    if r <= -1.0 {
        return Err(CellError::Num);
    }
    Ok(V::num(xnpv(r, &v)))
}

fn xirr_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = dated(e, a[0], a[1])?;
    let guess = e.opt_num(a, 2, 0.1)?;
    let only: Vec<f64> = v.iter().map(|p| p.0).collect();
    if !both_signs(&only) || guess <= -1.0 {
        return Err(CellError::Num);
    }
    root(|r| xnpv(r, &v), guess, -1.0).map(V::num).ok_or(CellError::Num)
}

fn fvschedule<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut p = e.num(a[0])?;
    for x in &e.arr(a[1])?.v {
        p *= 1.0 + e.to_num(x)?;
    }
    Ok(V::num(p))
}

nums!(rri, [n, p, f], {
    if n <= 0.0 || p <= 0.0 || f < 0.0 { Err(CellError::Num) } else { Ok((f / p).powf(1.0 / n) - 1.0) }
});

nums!(pduration, [r, p, f], {
    if r <= 0.0 || p <= 0.0 || f <= 0.0 { Err(CellError::Num) } else { Ok((f / p).ln() / r.ln_1p()) }
});

nums!(effect, [r, n], {
    let n = n.trunc();
    if r <= 0.0 || n < 1.0 { Err(CellError::Num) } else { Ok(growth1(r / n, n)) }
});

nums!(nominal, [r, n], {
    let n = n.trunc();
    if r <= 0.0 || n < 1.0 { Err(CellError::Num) } else { Ok(n * (r.ln_1p() / n).exp_m1()) }
});

// ---------------------------------------------------------------------------
// Depreciação
// ---------------------------------------------------------------------------

nums!(sln, [c, s, l], if l == 0.0 { Err(CellError::Div0) } else { Ok((c - s) / l) });

nums!(syd, [c, s, l, per], {
    if s < 0.0 || l <= 0.0 || per <= 0.0 || per > l {
        Err(CellError::Num)
    } else {
        Ok((c - s) * (l - per + 1.0) * 2.0 / (l * (l + 1.0)))
    }
});

nums!(db, [c, s, l, per, month = 12.0], {
    let month = month.trunc();
    let per = per.trunc();
    let last = if month < 12.0 { l + 1.0 } else { l };
    if c < 0.0 || s < 0.0 || l <= 0.0 || per < 1.0 || !(1.0..=12.0).contains(&month) || per > last {
        return Err(CellError::Num);
    }
    if c == 0.0 {
        return Ok(V::Num(0.0));
    }
    let r = crate::funcs::round_digits(1.0 - (s / c).powf(1.0 / l), 3, 0);
    let first = c * r * month / 12.0;
    if per == 1.0 {
        return Ok(V::num(first));
    }
    let mut total = first;
    let mut dep = first;
    let full = per.min(l.trunc()) as i64;
    for _ in 2..=full {
        dep = (c - total) * r;
        total += dep;
    }
    if per > l.trunc() {
        dep = (c - total) * r * (12.0 - month) / 12.0;
    }
    Ok(dep)
});

/// Depreciação por saldo decrescente de um período.
fn declining(c: f64, s: f64, l: f64, per: f64, factor: f64) -> f64 {
    let mut r = factor / l;
    let old = if r >= 1.0 {
        r = 1.0;
        if per == 1.0 { c } else { 0.0 }
    } else {
        c * (1.0 - r).powf(per - 1.0)
    };
    let new = c * (1.0 - r).powf(per);
    (if new < s { old - s } else { old - new }).max(0.0)
}

nums!(ddb, [c, s, l, per, factor = 2.0], {
    if c < 0.0 || s < 0.0 || l <= 0.0 || per <= 0.0 || per > l || factor <= 0.0 {
        Err(CellError::Num)
    } else {
        Ok(declining(c, s, l, per, factor))
    }
});

/// Depreciação até `period`, passando para a linear quando ela é maior.
fn with_switch(c: f64, s: f64, l: f64, left: f64, period: f64, factor: f64) -> f64 {
    let end = period.ceil();
    let mut rest = c - s;
    let mut total = 0.0;
    let mut linear: Option<f64> = None;
    for k in 1..=end as i64 {
        let mut term = match linear {
            Some(x) => x,
            None => {
                let d = declining(c, s, l, k as f64, factor);
                let x = rest / (left - (k - 1) as f64);
                if x > d {
                    linear = Some(x);
                    x
                } else {
                    rest -= d;
                    d
                }
            }
        };
        if k == end as i64 {
            term *= period + 1.0 - end;
        }
        total += term;
    }
    total
}

fn vdb<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let c = e.num(a[0])?;
    let s = e.num(a[1])?;
    let l = e.num(a[2])?;
    let start = e.num(a[3])?;
    let end = e.num(a[4])?;
    let factor = e.opt_num(a, 5, 2.0)?;
    let no_switch = e.opt_bool(a, 6, false)?;
    if c < 0.0 || s < 0.0 || l <= 0.0 || start < 0.0 || end < start || end > l || factor <= 0.0 || end > 1e6 {
        return Err(CellError::Num);
    }
    let (i0, i1) = (start.floor(), end.ceil());
    if no_switch {
        let mut total = 0.0;
        for k in i0 as i64 + 1..=i1 as i64 {
            let mut t = declining(c, s, l, k as f64, factor);
            if k == i0 as i64 + 1 {
                t *= end.min(i0 + 1.0) - start;
            } else if k == i1 as i64 {
                t *= end + 1.0 - i1;
            }
            total += t;
        }
        return Ok(V::num(total));
    }
    let mut part = 0.0;
    if start != i0 {
        let v = c - with_switch(c, s, l, l, i0, factor);
        part += (start - i0) * with_switch(v, s, l, l - i0, 1.0, factor);
    }
    if end != i1 {
        let v = c - with_switch(c, s, l, l, i1 - 1.0, factor);
        part += (i1 - end) * with_switch(v, s, l, l - (i1 - 1.0), 1.0, factor);
    }
    let v = c - with_switch(c, s, l, l, i0, factor);
    Ok(V::num(with_switch(v, s, l, l - i0, i1 - i0, factor) - part))
}

// ---------------------------------------------------------------------------
// Preços em fração
// ---------------------------------------------------------------------------

fn fraction(f: f64) -> Res<(f64, f64)> {
    let f = f.trunc();
    if f < 0.0 {
        return Err(CellError::Num);
    }
    if f == 0.0 {
        return Err(CellError::Div0);
    }
    Ok((f, 10f64.powi(f.log10().ceil() as i32)))
}

nums!(dollarde, [x, f], {
    let (f, p) = fraction(f)?;
    Ok(x.trunc() + round15(x.fract()) * p / f)
});

nums!(dollarfr, [x, f], {
    let (f, p) = fraction(f)?;
    Ok(x.trunc() + round15(x.fract()) * f / p)
});

/// Data de um argumento (dias inteiros).
pub fn day_arg(e: &mut Eval, n: u32) -> R {
    let d = date_arg(e, n)?.floor();
    if d < 0.0 { Err(CellError::Num) } else { Ok(d) }
}
