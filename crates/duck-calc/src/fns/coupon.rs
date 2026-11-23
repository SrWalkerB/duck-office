//! Títulos: cupons, preço, rendimento e duração.

use duck_core::CellError;

use super::ALL;
use super::dates::{Basis, add_months, basis_arg, day_count, yearfrac};
use super::financial::day_arg;
use crate::eval::{Eval, Res};
use crate::funcs::{FnDef, n};
use crate::value::V;

const CUP: &str = "liquidação; vencimento; frequência; [base]";

pub const DEFS: &[FnDef] = &[
    n("COUPNUM", "CUPNÚM", 3, 4, coupnum, CUP, "Quantos cupons faltam").lift(ALL),
    n("COUPPCD", "CUPDATAANT", 3, 4, couppcd, CUP, "A data do cupom anterior").lift(ALL),
    n("COUPNCD", "CUPDATAPRÓX", 3, 4, coupncd, CUP, "A data do próximo cupom").lift(ALL),
    n("COUPDAYS", "CUPDIAS", 3, 4, coupdays, CUP, "Os dias do período do cupom").lift(ALL),
    n("COUPDAYBS", "CUPDIASINLIQ", 3, 4, coupdaybs, CUP, "Os dias do início do cupom até a liquidação")
        .lift(ALL),
    n("COUPDAYSNC", "CUPDIASPRÓX", 3, 4, coupdaysnc, CUP, "Os dias da liquidação até o próximo cupom")
        .lift(ALL),
    n(
        "PRICE",
        "PREÇO",
        6,
        7,
        price_fn,
        "liquidação; vencimento; taxa; lcr; resgate; frequência; [base]",
        "O preço por 100 de um título com cupons",
    )
    .lift(ALL),
    n(
        "YIELD",
        "LUCRO",
        6,
        7,
        yield_fn,
        "liquidação; vencimento; taxa; pr; resgate; frequência; [base]",
        "O rendimento de um título com cupons",
    )
    .lift(ALL),
    n(
        "DURATION",
        "DURAÇÃO",
        5,
        6,
        duration,
        "liquidação; vencimento; cupom; lcr; frequência; [base]",
        "A duração de Macaulay de um título",
    )
    .lift(ALL),
    n(
        "MDURATION",
        "MDURAÇÃO",
        5,
        6,
        mduration,
        "liquidação; vencimento; cupom; lcr; frequência; [base]",
        "A duração modificada de um título",
    )
    .lift(ALL),
    n(
        "DISC",
        "DESC",
        4,
        5,
        disc,
        "liquidação; vencimento; pr; resgate; [base]",
        "A taxa de desconto de um título",
    )
    .lift(ALL),
    n(
        "INTRATE",
        "TAXAJUROS",
        4,
        5,
        intrate,
        "liquidação; vencimento; investimento; resgate; [base]",
        "A taxa de juros de um título",
    )
    .lift(ALL),
    n(
        "RECEIVED",
        "RECEBER",
        4,
        5,
        received,
        "liquidação; vencimento; investimento; desconto; [base]",
        "O valor recebido no vencimento",
    )
    .lift(ALL),
    n(
        "PRICEDISC",
        "PREÇODESC",
        4,
        5,
        pricedisc,
        "liquidação; vencimento; desconto; resgate; [base]",
        "O preço por 100 de um título com desconto",
    )
    .lift(ALL),
    n(
        "YIELDDISC",
        "LUCRODESC",
        4,
        5,
        yielddisc,
        "liquidação; vencimento; pr; resgate; [base]",
        "O rendimento de um título com desconto",
    )
    .lift(ALL),
    n(
        "PRICEMAT",
        "PREÇOVENC",
        5,
        6,
        pricemat,
        "liquidação; vencimento; emissão; taxa; lcr; [base]",
        "O preço por 100 de um título com juros no vencimento",
    )
    .lift(ALL),
    n(
        "YIELDMAT",
        "LUCROVENC",
        5,
        6,
        yieldmat,
        "liquidação; vencimento; emissão; taxa; pr; [base]",
        "O rendimento de um título com juros no vencimento",
    )
    .lift(ALL),
    n(
        "ACCRINTM",
        "JUROSACUMV",
        4,
        5,
        accrintm,
        "emissão; liquidação; taxa; valor_nominal; [base]",
        "Os juros acumulados até o vencimento",
    )
    .lift(ALL),
    n(
        "TBILLEQ",
        "OTN",
        3,
        3,
        tbilleq,
        "liquidação; vencimento; desconto",
        "O rendimento equivalente de uma letra",
    )
    .lift(ALL),
    n("TBILLPRICE", "OTNVALOR", 3, 3, tbillprice, "liquidação; vencimento; desconto", "O preço de uma letra")
        .lift(ALL),
    n("TBILLYIELD", "OTNLUCRO", 3, 3, tbillyield, "liquidação; vencimento; pr", "O rendimento de uma letra")
        .lift(ALL),
];

/// Cupons de um título na data da liquidação.
struct Coupons {
    settle: f64,
    prev: f64,
    next: f64,
    /// Cupons que faltam.
    count: f64,
    freq: f64,
    basis: Basis,
}

impl Coupons {
    /// Argumentos: liquidação, vencimento e, em `k`, frequência e base.
    fn read(e: &mut Eval, a: &[u32], k: usize) -> Res<Self> {
        let settle = day_arg(e, a[0])?;
        let mat = day_arg(e, a[1])?;
        let freq = e.num(a[k])?.trunc();
        let basis = basis_arg(e, a, k + 1)?;
        if !matches!(freq as i64, 1 | 2 | 4) || settle >= mat {
            return Err(CellError::Num);
        }
        let step = 12 / freq as i64;
        // Do vencimento para trás (o dia do vencimento manda).
        let mut i = ((mat - settle) / 366.0 * freq).floor().max(0.0) as i64;
        let mut next = add_months(e, mat, -i * step, true)?;
        loop {
            let d = add_months(e, mat, -(i + 1) * step, true)?;
            if d <= settle {
                return Ok(Self { settle, prev: d, next, count: i as f64 + 1.0, freq, basis });
            }
            next = d;
            i += 1;
        }
    }

    /// Dias do período do cupom (E).
    fn days(&self) -> f64 {
        match self.basis {
            Basis::Actual => self.next - self.prev,
            Basis::A365 => 365.0 / self.freq,
            _ => 360.0 / self.freq,
        }
    }

    /// Dias do início do período até a liquidação (A).
    fn accrued(&self, e: &Eval) -> Res<f64> {
        day_count(e, self.prev, self.settle, self.basis)
    }

    /// Dias da liquidação até o próximo cupom (DSC).
    fn to_next(&self, e: &Eval) -> Res<f64> {
        Ok(match self.basis {
            Basis::Us30 | Basis::Eu30 => self.days() - self.accrued(e)?,
            _ => self.next - self.settle,
        })
    }
}

fn coupnum<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(Coupons::read(e, a, 2)?.count))
}

fn couppcd<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(Coupons::read(e, a, 2)?.prev))
}

fn coupncd<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(Coupons::read(e, a, 2)?.next))
}

fn coupdays<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(Coupons::read(e, a, 2)?.days()))
}

fn coupdaybs<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let c = Coupons::read(e, a, 2)?;
    Ok(V::Num(c.accrued(e)?))
}

fn coupdaysnc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let c = Coupons::read(e, a, 2)?;
    Ok(V::Num(c.to_next(e)?))
}

/// Preço por 100 com rendimento `y`, dados A, DSC e E.
fn price(c: &Coupons, rate: f64, y: f64, red: f64, (acc, dsc, e): (f64, f64, f64)) -> f64 {
    let cp = 100.0 * rate / c.freq;
    if c.count <= 1.0 {
        return (cp + red) / (1.0 + y / c.freq * dsc / e) - cp * acc / e;
    }
    let q = 1.0 + y / c.freq;
    let w = dsc / e;
    let mut p = red / q.powf(c.count - 1.0 + w);
    for k in 0..c.count as i64 {
        p += cp / q.powf(k as f64 + w);
    }
    p - cp * acc / e
}

fn price_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let c = Coupons::read(e, a, 5)?;
    let rate = e.num(a[2])?;
    let y = e.num(a[3])?;
    let red = e.num(a[4])?;
    if rate < 0.0 || y < 0.0 || red <= 0.0 {
        return Err(CellError::Num);
    }
    let d = (c.accrued(e)?, c.to_next(e)?, c.days());
    Ok(V::num(price(&c, rate, y, red, d)))
}

fn yield_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let c = Coupons::read(e, a, 5)?;
    let rate = e.num(a[2])?;
    let pr = e.num(a[3])?;
    let red = e.num(a[4])?;
    if rate < 0.0 || pr <= 0.0 || red <= 0.0 {
        return Err(CellError::Num);
    }
    let d @ (acc, dsc, days) = (c.accrued(e)?, c.to_next(e)?, c.days());
    if c.count <= 1.0 {
        let paid = pr / 100.0 + acc / days * rate / c.freq;
        return Ok(V::num((red / 100.0 + rate / c.freq - paid) / paid * c.freq * days / dsc));
    }
    // O preço cai quando o rendimento sobe: bissecção.
    let (mut lo, mut hi) = (-0.99 * c.freq, 1.0);
    while price(&c, rate, hi, red, d) > pr {
        hi *= 2.0;
        if hi > 1e6 {
            return Err(CellError::Num);
        }
    }
    for _ in 0..200 {
        let m = (lo + hi) / 2.0;
        if m == lo || m == hi {
            break;
        }
        if price(&c, rate, m, red, d) > pr {
            lo = m;
        } else {
            hi = m;
        }
    }
    Ok(V::num((lo + hi) / 2.0))
}

fn macaulay(e: &mut Eval, a: &[u32]) -> Res<(f64, f64)> {
    let c = Coupons::read(e, a, 4)?;
    let rate = e.num(a[2])?;
    let y = e.num(a[3])?;
    if rate < 0.0 || y < 0.0 {
        return Err(CellError::Num);
    }
    let cp = 100.0 * rate / c.freq;
    let q = 1.0 + y / c.freq;
    let w = c.to_next(e)? / c.days();
    let (mut p, mut t) = (0.0, 0.0);
    for k in 0..c.count as i64 {
        let at = k as f64 + w;
        let v = (cp + if k as f64 == c.count - 1.0 { 100.0 } else { 0.0 }) / q.powf(at);
        p += v;
        t += v * at;
    }
    Ok((t / p / c.freq, q))
}

fn duration<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(macaulay(e, a)?.0))
}

fn mduration<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (d, q) = macaulay(e, a)?;
    Ok(V::num(d / q))
}

/// Liquidação e vencimento em anos, e dois números positivos.
fn simple(e: &mut Eval, a: &[u32]) -> Res<(f64, f64, f64)> {
    let s = day_arg(e, a[0])?;
    let m = day_arg(e, a[1])?;
    let x = e.num(a[2])?;
    let y = e.num(a[3])?;
    let basis = basis_arg(e, a, 4)?;
    if x <= 0.0 || y <= 0.0 || s >= m {
        return Err(CellError::Num);
    }
    Ok((yearfrac(e, s, m, basis)?, x, y))
}

fn disc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (t, pr, red) = simple(e, a)?;
    Ok(V::num((red - pr) / red / t))
}

fn intrate<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (t, inv, red) = simple(e, a)?;
    Ok(V::num((red - inv) / inv / t))
}

fn received<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (t, inv, d) = simple(e, a)?;
    if d * t >= 1.0 { Err(CellError::Num) } else { Ok(V::num(inv / (1.0 - d * t))) }
}

fn pricedisc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (t, d, red) = simple(e, a)?;
    Ok(V::num(red * (1.0 - d * t)))
}

fn yielddisc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (t, pr, red) = simple(e, a)?;
    Ok(V::num((red - pr) / pr / t))
}

/// (emissão→vencimento, liquidação→vencimento, emissão→liquidação) em anos,
/// taxa e o quinto argumento.
fn at_maturity(e: &mut Eval, a: &[u32]) -> Res<(f64, f64, f64, f64, f64)> {
    let s = day_arg(e, a[0])?;
    let m = day_arg(e, a[1])?;
    let i = day_arg(e, a[2])?;
    let rate = e.num(a[3])?;
    let x = e.num(a[4])?;
    let basis = basis_arg(e, a, 5)?;
    if s >= m || i > s || rate < 0.0 || x < 0.0 {
        return Err(CellError::Num);
    }
    Ok((yearfrac(e, i, m, basis)?, yearfrac(e, s, m, basis)?, yearfrac(e, i, s, basis)?, rate, x))
}

fn pricemat<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (dim, dsm, acc, rate, y) = at_maturity(e, a)?;
    Ok(V::num(100.0 * ((1.0 + dim * rate) / (1.0 + dsm * y) - acc * rate)))
}

fn yieldmat<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (dim, dsm, acc, rate, pr) = at_maturity(e, a)?;
    let paid = pr / 100.0 + acc * rate;
    Ok(V::num((1.0 + dim * rate - paid) / paid / dsm))
}

fn accrintm<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (t, rate, par) = simple(e, a)?;
    Ok(V::num(par * rate * t))
}

/// Dias até o vencimento de uma letra (no máximo um ano) e o terceiro argumento.
fn bill(e: &mut Eval, a: &[u32]) -> Res<(f64, f64)> {
    let s = day_arg(e, a[0])?;
    let m = day_arg(e, a[1])?;
    let x = e.num(a[2])?;
    if s >= m || m > add_months(e, s, 12, false)? || x <= 0.0 {
        return Err(CellError::Num);
    }
    Ok((m - s, x))
}

fn tbillprice<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (d, x) = bill(e, a)?;
    let p = 100.0 * (1.0 - x * d / 360.0);
    if p < 0.0 { Err(CellError::Num) } else { Ok(V::num(p)) }
}

fn tbillyield<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (d, pr) = bill(e, a)?;
    Ok(V::num((100.0 - pr) / pr * 360.0 / d))
}

fn tbilleq<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (d, x) = bill(e, a)?;
    if d <= 182.0 {
        return Ok(V::num(365.0 * x / (360.0 - x * d)));
    }
    let p = 1.0 - x * d / 360.0;
    let t = d / if d == 366.0 { 366.0 } else { 365.0 };
    Ok(V::num((-2.0 * t + 2.0 * (t * t - (2.0 * t - 1.0) * (1.0 - 1.0 / p)).sqrt()) / (2.0 * t - 1.0)))
}
