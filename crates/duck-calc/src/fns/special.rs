//! Funções especiais (gama, beta e normal) que as distribuições usam.

use std::f64::consts::{PI, SQRT_2};

/// ln(√(2π)).
const LN_SQRT_2PI: f64 = 0.918_938_533_204_672_8;

pub fn erf(x: f64) -> f64 {
    libm::erf(x)
}

pub fn erfc(x: f64) -> f64 {
    libm::erfc(x)
}

/// ln Γ(x), x > 0.
pub fn ln_gamma(x: f64) -> f64 {
    libm::lgamma(x)
}

pub fn gamma(x: f64) -> f64 {
    libm::tgamma(x)
}

pub fn norm_pdf(z: f64) -> f64 {
    (-z * z / 2.0).exp() / (2.0 * PI).sqrt()
}

pub fn norm_cdf(z: f64) -> f64 {
    0.5 * erfc(-z / SQRT_2)
}

/// Inversa da normal padrão (0 < p < 1).
pub fn norm_inv(p: f64) -> f64 {
    if p > 0.5 {
        return -norm_inv(1.0 - p);
    }
    // Aproximação racional de Acklam e dois passos de Halley.
    const A: [f64; 6] = [
        -3.969683028665376e1,
        2.209460984245205e2,
        -2.759285104469687e2,
        1.38357751867269e2,
        -3.066479806614716e1,
        2.506628277459239,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e1,
        1.615858368580409e2,
        -1.556989798598866e2,
        6.680131188771972e1,
        -1.328068155288572e1,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-3,
        -3.223964580411365e-1,
        -2.400758277161838,
        -2.549732539343734,
        4.374664141464968,
        2.938163982698783,
    ];
    const D: [f64; 4] = [7.784695709041462e-3, 3.224671290700398e-1, 2.445134137142996, 3.754408661907416];
    let mut x = if p < 0.02425 {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    };
    for _ in 0..2 {
        let u = (norm_cdf(x) - p) * (2.0 * PI).sqrt() * (x * x / 2.0).exp();
        x -= u / (1.0 + x * u / 2.0);
    }
    x
}

/// Erro da fórmula de Stirling de ln n!.
fn stirling_err(n: f64) -> f64 {
    if n <= 15.0 {
        return ln_gamma(n + 1.0) - (n + 0.5) * n.ln() + n - LN_SQRT_2PI;
    }
    let m = n * n;
    (1.0 / 12.0 - (1.0 / 360.0 - (1.0 / 1260.0 - (1.0 / 1680.0 - 1.0 / 1188.0 / m) / m) / m) / m) / n
}

/// Desvio x·ln(x/m) + m - x.
fn deviance(x: f64, m: f64) -> f64 {
    if (x - m).abs() < 0.1 * (x + m) {
        let mut v = (x - m) / (x + m);
        let mut s = (x - m) * v;
        let mut ej = 2.0 * x * v;
        v *= v;
        let mut j = 1.0;
        loop {
            ej *= v;
            let next = s + ej / (2.0 * j + 1.0);
            if next == s || j > 1000.0 {
                return next;
            }
            s = next;
            j += 1.0;
        }
    }
    x * (x / m).ln() + m - x
}

/// m^x·e^-m / Γ(x+1) (método de Loader).
pub fn poisson_pmf(x: f64, m: f64) -> f64 {
    if m == 0.0 {
        return if x == 0.0 { 1.0 } else { 0.0 };
    }
    if x == 0.0 {
        return (-m).exp();
    }
    if x < 0.0 {
        return 0.0;
    }
    (-stirling_err(x) - deviance(x, m)).exp() / (2.0 * PI * x).sqrt()
}

/// Probabilidade binomial com `x` e `n` reais (método de Loader).
pub fn binom_pmf(x: f64, n: f64, p: f64, q: f64) -> f64 {
    if p == 0.0 {
        return if x == 0.0 { 1.0 } else { 0.0 };
    }
    if q == 0.0 {
        return if x == n { 1.0 } else { 0.0 };
    }
    if x < 0.0 || x > n {
        return 0.0;
    }
    if x == 0.0 {
        return (n * if p < 0.1 { (-p).ln_1p() } else { q.ln() }).exp();
    }
    if x == n {
        return (n * p.ln()).exp();
    }
    let lc =
        stirling_err(n) - stirling_err(x) - stirling_err(n - x) - deviance(x, n * p) - deviance(n - x, n * q);
    (lc - 0.5 * ((2.0 * PI).ln() + x.ln() + (-x / n).ln_1p())).exp()
}

/// Gama incompleta regularizada: (P(a, x), Q(a, x)).
pub fn gamma_pq(a: f64, x: f64) -> (f64, f64) {
    if x <= 0.0 {
        return (0.0, 1.0);
    }
    if x.is_infinite() {
        return (1.0, 0.0);
    }
    // x^a·e^-x / Γ(a+1)
    let pre = poisson_pmf(a, x);
    if x < a + 1.0 {
        let (mut sum, mut term, mut k) = (1.0, 1.0, a);
        for _ in 0..100_000 {
            k += 1.0;
            term *= x / k;
            sum += term;
            if term < sum * 1e-17 {
                break;
            }
        }
        let p = (pre * sum).min(1.0);
        (p, 1.0 - p)
    } else {
        // Fração contínua (Lentz).
        const TINY: f64 = 1e-300;
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / TINY;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..100_000 {
            let i = i as f64;
            let an = -i * (i - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < TINY {
                d = TINY;
            }
            c = b + an / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            let del = d * c;
            h *= del;
            if (del - 1.0).abs() < 1e-16 {
                break;
            }
        }
        let q = (pre * a * h).min(1.0);
        (1.0 - q, q)
    }
}

/// x^a·y^b / B(a, b), com y = 1 - x.
pub fn beta_kernel(a: f64, b: f64, x: f64, y: f64) -> f64 {
    if x <= 0.0 || y <= 0.0 {
        return 0.0;
    }
    binom_pmf(a, a + b, x, y) * a * b / (a + b)
}

/// Beta incompleta regularizada (I_x(a, b), 1 - I_x(a, b)), com y = 1 - x.
pub fn beta_inc(a: f64, b: f64, x: f64, y: f64) -> (f64, f64) {
    if x <= 0.0 {
        return (0.0, 1.0);
    }
    if y <= 0.0 {
        return (1.0, 0.0);
    }
    let k = beta_kernel(a, b, x, y);
    if x < (a + 1.0) / (a + b + 2.0) {
        let p = (k * beta_cf(a, b, x) / a).min(1.0);
        (p, 1.0 - p)
    } else {
        let q = (k * beta_cf(b, a, y) / b).min(1.0);
        (1.0 - q, q)
    }
}

fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    let fix = |v: f64| if v.abs() < TINY { TINY } else { v };
    let mut c = 1.0;
    let mut d = 1.0 / fix(1.0 - (a + b) * x / (a + 1.0));
    let mut h = d;
    for m in 1..100_000 {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((a - 1.0 + m2) * (a + m2));
        d = 1.0 / fix(1.0 + aa * d);
        c = fix(1.0 + aa / c);
        h *= d * c;
        let aa = -(a + m) * (a + b + m) * x / ((a + m2) * (a + 1.0 + m2));
        d = 1.0 / fix(1.0 + aa * d);
        c = fix(1.0 + aa / c);
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-16 {
            break;
        }
    }
    h
}

/// Inversa de uma função de distribuição crescente em [lo, hi] (hi pode ser
/// infinito) por bissecção.
pub fn invert(p: f64, lo: f64, hi: f64, cdf: impl Fn(f64) -> f64) -> f64 {
    let (mut a, mut b) = (lo, hi);
    if b.is_infinite() {
        b = if lo > 0.0 { lo * 2.0 } else { 1.0 };
        while cdf(b) < p {
            a = b;
            b *= 2.0;
            if b > 1e300 {
                return b;
            }
        }
    }
    if a.is_infinite() {
        a = -1.0;
        while cdf(a) > p {
            b = a;
            a *= 2.0;
            if a < -1e300 {
                return a;
            }
        }
    }
    for _ in 0..1100 {
        let m = a + (b - a) / 2.0;
        if m <= a || m >= b {
            break;
        }
        if cdf(m) < p {
            a = m;
        } else {
            b = m;
        }
    }
    // Entre os dois últimos, o mais perto.
    if (cdf(a) - p).abs() < (cdf(b) - p).abs() { a } else { b }
}

/// Cauda superior da t de Student (P(T > t)).
pub fn t_upper(t: f64, df: f64) -> f64 {
    let x = df / (df + t * t);
    let y = t * t / (df + t * t);
    let tail = 0.5 * beta_inc(df / 2.0, 0.5, x, y).0;
    if t >= 0.0 { tail } else { 1.0 - tail }
}

pub fn t_pdf(t: f64, df: f64) -> f64 {
    let l = ln_gamma((df + 1.0) / 2.0) - ln_gamma(df / 2.0) - 0.5 * (df * PI).ln();
    (l - (df + 1.0) / 2.0 * (t * t / df).ln_1p()).exp()
}

/// t com cauda superior `p` (0 < p < 1).
pub fn t_upper_inv(p: f64, df: f64) -> f64 {
    if p > 0.5 {
        return -t_upper_inv(1.0 - p, df);
    }
    if p == 0.5 {
        return 0.0;
    }
    invert(-p, 0.0, f64::INFINITY, |t| -t_upper(t, df))
}

/// (P(F <= x), P(F > x)).
pub fn f_pq(x: f64, d1: f64, d2: f64) -> (f64, f64) {
    if x <= 0.0 {
        return (0.0, 1.0);
    }
    let s = d1 * x + d2;
    beta_inc(d1 / 2.0, d2 / 2.0, d1 * x / s, d2 / s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol * b.abs().max(1e-300), "{a} != {b}");
    }

    #[test]
    fn special_functions() {
        near(norm_inv(0.975), 1.959963984540054, 1e-14);
        near(norm_inv(1e-10), -6.361340902404056, 1e-13);
        near(gamma_pq(3.0, 2.0).0, 0.3233235838169366, 1e-14);
        near(gamma_pq(0.5, 9.0).1, 2.209049699858544e-05, 1e-13);
        near(beta_inc(2.0, 3.0, 0.4, 0.6).0, 0.5248, 1e-14);
        near(beta_inc(50.0, 80.0, 0.5, 0.5).1, 0.004003836604045413, 1e-11);
        near(t_upper_inv(0.025, 10.0), 2.228138851986274, 1e-13);
        near(poisson_pmf(10.0, 10.0), 0.125110035721133, 1e-14);
        near(binom_pmf(6.0, 10.0, 0.5, 0.5), 0.205078125, 1e-14);
    }
}
