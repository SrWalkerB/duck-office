//! Distribuições de probabilidade e testes de hipótese.

use duck_core::CellError;

use super::special::*;
use super::{ALL, arr_nums, num1, nums, pairs};
use crate::eval::{Eval, Res};
use crate::funcs::{FnDef, n, variance};
use crate::value::V;

pub const DEFS: &[FnDef] = &[
    n(
        "NORM.DIST",
        "DIST.NORM.N",
        4,
        4,
        norm_dist,
        "x; média; desv_padrão; cumulativo",
        "A distribuição normal",
    )
    .lift(ALL),
    n("NORMDIST", "DIST.NORM", 4, 4, norm_dist, "x; média; desv_padrão; cumulativo", "A distribuição normal")
        .lift(ALL),
    n(
        "NORM.INV",
        "INV.NORM.N",
        3,
        3,
        norm_inv_fn,
        "probabilidade; média; desv_padrão",
        "O inverso da normal",
    )
    .lift(ALL),
    n("NORMINV", "INV.NORM", 3, 3, norm_inv_fn, "probabilidade; média; desv_padrão", "O inverso da normal")
        .lift(ALL),
    n("NORM.S.DIST", "DIST.NORMP.N", 2, 2, norm_s_dist, "z; cumulativo", "A distribuição normal padrão")
        .lift(ALL),
    n("NORMSDIST", "DIST.NORMP", 1, 1, normsdist, "z", "A distribuição normal padrão acumulada").lift(ALL),
    n("NORM.S.INV", "INV.NORMP.N", 1, 1, norm_s_inv, "probabilidade", "O inverso da normal padrão").lift(ALL),
    n("NORMSINV", "INV.NORMP", 1, 1, norm_s_inv, "probabilidade", "O inverso da normal padrão").lift(ALL),
    n("PHI", "PHI", 1, 1, phi, "x", "A densidade da normal padrão").lift(ALL),
    n("GAUSS", "GAUSS", 1, 1, gauss, "z", "A probabilidade entre a média e z desvios").lift(ALL),
    n(
        "LOGNORM.DIST",
        "DIST.LOGNORMAL.N",
        4,
        4,
        lognorm_dist,
        "x; média; desv_padrão; cumulativo",
        "A distribuição log-normal",
    )
    .lift(ALL),
    n("LOGNORMDIST", "DIST.LOGNORMAL", 3, 3, lognormdist, "x; média; desv_padrão", "A log-normal acumulada")
        .lift(ALL),
    n(
        "LOGNORM.INV",
        "INV.LOGNORMAL",
        3,
        3,
        lognorm_inv,
        "probabilidade; média; desv_padrão",
        "O inverso da log-normal",
    )
    .lift(ALL),
    n("LOGINV", "INVLOG", 3, 3, lognorm_inv, "probabilidade; média; desv_padrão", "O inverso da log-normal")
        .lift(ALL),
    n("EXPON.DIST", "DISTR.EXPON", 3, 3, expon_dist, "x; lambda; cumulativo", "A distribuição exponencial")
        .lift(ALL),
    n("EXPONDIST", "DISTEXPON", 3, 3, expon_dist, "x; lambda; cumulativo", "A distribuição exponencial")
        .lift(ALL),
    n(
        "WEIBULL.DIST",
        "DIST.WEIBULL",
        4,
        4,
        weibull,
        "x; alfa; beta; cumulativo",
        "A distribuição de Weibull",
    )
    .lift(ALL),
    n("WEIBULL", "WEIBULL", 4, 4, weibull, "x; alfa; beta; cumulativo", "A distribuição de Weibull")
        .lift(ALL),
    n("GAMMA.DIST", "DIST.GAMA", 4, 4, gamma_dist, "x; alfa; beta; cumulativo", "A distribuição gama")
        .lift(ALL),
    n("GAMMADIST", "DISTGAMA", 4, 4, gamma_dist, "x; alfa; beta; cumulativo", "A distribuição gama")
        .lift(ALL),
    n("GAMMA.INV", "INV.GAMA", 3, 3, gamma_inv, "probabilidade; alfa; beta", "O inverso da gama").lift(ALL),
    n("GAMMAINV", "INVGAMA", 3, 3, gamma_inv, "probabilidade; alfa; beta", "O inverso da gama").lift(ALL),
    n("GAMMA", "GAMA", 1, 1, gamma_fn, "núm", "A função gama").lift(ALL),
    n("GAMMALN", "LNGAMA", 1, 1, gammaln, "x", "O logaritmo natural da função gama").lift(ALL),
    n("GAMMALN.PRECISE", "LNGAMA.PRECISO", 1, 1, gammaln, "x", "O logaritmo natural da função gama")
        .lift(ALL),
    n(
        "BETA.DIST",
        "DIST.BETA",
        4,
        6,
        beta_dist,
        "x; alfa; beta; cumulativo; [A]; [B]",
        "A distribuição beta",
    )
    .lift(ALL),
    n("BETADIST", "DISTBETA", 3, 5, betadist, "x; alfa; beta; [A]; [B]", "A distribuição beta acumulada")
        .lift(ALL),
    n("BETA.INV", "INV.BETA", 3, 5, beta_inv, "probabilidade; alfa; beta; [A]; [B]", "O inverso da beta")
        .lift(ALL),
    n("BETAINV", "BETA.ACUM.INV", 3, 5, beta_inv, "probabilidade; alfa; beta; [A]; [B]", "O inverso da beta")
        .lift(ALL),
    n(
        "CHISQ.DIST",
        "DIST.QUIQUA",
        3,
        3,
        chisq_dist,
        "x; graus_liberdade; cumulativo",
        "A distribuição qui-quadrado",
    )
    .lift(ALL),
    n(
        "CHISQ.DIST.RT",
        "DIST.QUIQUA.CD",
        2,
        2,
        chisq_rt,
        "x; graus_liberdade",
        "A cauda direita da qui-quadrado",
    )
    .lift(ALL),
    n("CHIDIST", "DIST.QUI", 2, 2, chisq_rt, "x; graus_liberdade", "A cauda direita da qui-quadrado")
        .lift(ALL),
    n(
        "CHISQ.INV",
        "INV.QUIQUA",
        2,
        2,
        chisq_inv,
        "probabilidade; graus_liberdade",
        "O inverso da qui-quadrado",
    )
    .lift(ALL),
    n(
        "CHISQ.INV.RT",
        "INV.QUIQUA.CD",
        2,
        2,
        chisq_inv_rt,
        "probabilidade; graus_liberdade",
        "O inverso da cauda direita da qui-quadrado",
    )
    .lift(ALL),
    n(
        "CHIINV",
        "INV.QUI",
        2,
        2,
        chisq_inv_rt,
        "probabilidade; graus_liberdade",
        "O inverso da cauda direita da qui-quadrado",
    )
    .lift(ALL),
    n("T.DIST", "DIST.T", 3, 3, t_dist, "x; graus_liberdade; cumulativo", "A distribuição t de Student")
        .lift(ALL),
    n("T.DIST.RT", "DIST.T.CD", 2, 2, t_dist_rt, "x; graus_liberdade", "A cauda direita da t").lift(ALL),
    n("T.DIST.2T", "DIST.T.BC", 2, 2, t_dist_2t, "x; graus_liberdade", "As duas caudas da t").lift(ALL),
    n("TDIST", "DISTT", 3, 3, tdist, "x; graus_liberdade; caudas", "A distribuição t de Student").lift(ALL),
    n("T.INV", "INV.T", 2, 2, t_inv, "probabilidade; graus_liberdade", "O inverso da t").lift(ALL),
    n(
        "T.INV.2T",
        "INV.T.BC",
        2,
        2,
        t_inv_2t,
        "probabilidade; graus_liberdade",
        "O inverso da t com duas caudas",
    )
    .lift(ALL),
    n("TINV", "INVT", 2, 2, t_inv_2t, "probabilidade; graus_liberdade", "O inverso da t com duas caudas")
        .lift(ALL),
    n(
        "F.DIST",
        "DIST.F",
        4,
        4,
        f_dist,
        "x; graus_liberdade1; graus_liberdade2; cumulativo",
        "A distribuição F",
    )
    .lift(ALL),
    n("F.DIST.RT", "DIST.F.CD", 3, 3, f_rt, "x; graus_liberdade1; graus_liberdade2", "A cauda direita da F")
        .lift(ALL),
    n("FDIST", "DISTF", 3, 3, f_rt, "x; graus_liberdade1; graus_liberdade2", "A cauda direita da F")
        .lift(ALL),
    n("F.INV", "INV.F", 3, 3, f_inv, "probabilidade; graus_liberdade1; graus_liberdade2", "O inverso da F")
        .lift(ALL),
    n(
        "F.INV.RT",
        "INV.F.CD",
        3,
        3,
        f_inv_rt,
        "probabilidade; graus_liberdade1; graus_liberdade2",
        "O inverso da cauda direita da F",
    )
    .lift(ALL),
    n(
        "FINV",
        "INVF",
        3,
        3,
        f_inv_rt,
        "probabilidade; graus_liberdade1; graus_liberdade2",
        "O inverso da cauda direita da F",
    )
    .lift(ALL),
    n(
        "BINOM.DIST",
        "DISTR.BINOM",
        4,
        4,
        binom_dist,
        "núm_s; tentativas; probabilidade_s; cumulativo",
        "A distribuição binomial",
    )
    .lift(ALL),
    n(
        "BINOMDIST",
        "DISTRBINOM",
        4,
        4,
        binom_dist,
        "núm_s; tentativas; probabilidade_s; cumulativo",
        "A distribuição binomial",
    )
    .lift(ALL),
    n(
        "BINOM.INV",
        "INV.BINOM",
        3,
        3,
        binom_inv,
        "tentativas; probabilidade_s; alfa",
        "O inverso da binomial",
    )
    .lift(ALL),
    n(
        "CRITBINOM",
        "CRIT.BINOM",
        3,
        3,
        binom_inv,
        "tentativas; probabilidade_s; alfa",
        "O inverso da binomial",
    )
    .lift(ALL),
    n(
        "BINOM.DIST.RANGE",
        "INTERV.DISTR.BINOM",
        3,
        4,
        binom_range,
        "tentativas; probabilidade_s; núm_s; [núm_s2]",
        "A probabilidade binomial de um intervalo",
    )
    .lift(ALL),
    n(
        "NEGBINOM.DIST",
        "DIST.BIN.NEG.N",
        4,
        4,
        negbinom_dist,
        "núm_f; núm_s; probabilidade_s; cumulativo",
        "A distribuição binomial negativa",
    )
    .lift(ALL),
    n(
        "NEGBINOMDIST",
        "DIST.BIN.NEG",
        3,
        3,
        negbinomdist,
        "núm_f; núm_s; probabilidade_s",
        "A distribuição binomial negativa",
    )
    .lift(ALL),
    n("POISSON.DIST", "DIST.POISSON", 3, 3, poisson, "x; média; cumulativo", "A distribuição de Poisson")
        .lift(ALL),
    n("POISSON", "POISSON", 3, 3, poisson, "x; média; cumulativo", "A distribuição de Poisson").lift(ALL),
    n(
        "HYPGEOM.DIST",
        "DIST.HIPERGEOM.N",
        5,
        5,
        hypgeom_dist,
        "exemplo_s; exemplo_núm; população_s; núm_população; cumulativo",
        "A distribuição hipergeométrica",
    )
    .lift(ALL),
    n(
        "HYPGEOMDIST",
        "DIST.HIPERGEOM",
        4,
        4,
        hypgeomdist,
        "exemplo_s; exemplo_núm; população_s; núm_população",
        "A distribuição hipergeométrica",
    )
    .lift(ALL),
    n(
        "CONFIDENCE",
        "INT.CONFIANÇA",
        3,
        3,
        confidence,
        "alfa; desv_padrão; tamanho",
        "O intervalo de confiança da média (normal)",
    )
    .lift(ALL),
    n(
        "CONFIDENCE.NORM",
        "INT.CONFIANÇA.NORM",
        3,
        3,
        confidence,
        "alfa; desv_padrão; tamanho",
        "O intervalo de confiança da média (normal)",
    )
    .lift(ALL),
    n(
        "CONFIDENCE.T",
        "INT.CONFIANÇA.T",
        3,
        3,
        confidence_t,
        "alfa; desv_padrão; tamanho",
        "O intervalo de confiança da média (t de Student)",
    )
    .lift(ALL),
    n("FISHER", "FISHER", 1, 1, fisher, "x", "A transformação de Fisher").lift(ALL),
    n("FISHERINV", "FISHERINV", 1, 1, fisherinv, "y", "O inverso da transformação de Fisher").lift(ALL),
    n("T.TEST", "TESTE.T", 4, 4, t_test, "matriz1; matriz2; caudas; tipo", "A probabilidade do teste t"),
    n("TTEST", "TESTET", 4, 4, t_test, "matriz1; matriz2; caudas; tipo", "A probabilidade do teste t"),
    n("F.TEST", "TESTE.F", 2, 2, f_test, "matriz1; matriz2", "A probabilidade do teste F"),
    n("FTEST", "TESTEF", 2, 2, f_test, "matriz1; matriz2", "A probabilidade do teste F"),
    n("Z.TEST", "TESTE.Z", 2, 3, z_test, "matriz; x; [sigma]", "A probabilidade do teste z"),
    n("ZTEST", "TESTEZ", 2, 3, z_test, "matriz; x; [sigma]", "A probabilidade do teste z"),
    n(
        "CHISQ.TEST",
        "TESTE.QUIQUA",
        2,
        2,
        chisq_test,
        "intervalo_real; intervalo_esperado",
        "A probabilidade do teste qui-quadrado",
    ),
    n(
        "CHITEST",
        "TESTE.QUI",
        2,
        2,
        chisq_test,
        "intervalo_real; intervalo_esperado",
        "A probabilidade do teste qui-quadrado",
    ),
    n(
        "PROB",
        "PROB",
        3,
        4,
        prob,
        "intervalo_x; intervalo_prob; limite_inferior; [limite_superior]",
        "A probabilidade de valores entre dois limites",
    ),
];

const NUM: CellError = CellError::Num;

/// Probabilidade válida para as inversas: 0 < p < 1.
fn open01(p: f64) -> Res<f64> {
    if p > 0.0 && p < 1.0 { Ok(p) } else { Err(NUM) }
}

/// Graus de liberdade: inteiro de 1 a 10^10.
fn degrees(x: f64) -> Res<f64> {
    let d = x.trunc();
    if (1.0..=1e10).contains(&d) { Ok(d) } else { Err(NUM) }
}

fn positive(x: f64) -> Res<f64> {
    if x > 0.0 { Ok(x) } else { Err(NUM) }
}

// Normal

nums!(norm_dist, [x, m, s, cum], {
    let z = (x - m) / positive(s)?;
    Ok(if cum != 0.0 { norm_cdf(z) } else { norm_pdf(z) / s })
});
nums!(norm_inv_fn, [p, m, s], Ok(m + norm_inv(open01(p)?) * positive(s)?));
nums!(norm_s_dist, [z, cum], Ok(if cum != 0.0 { norm_cdf(z) } else { norm_pdf(z) }));
num1!(normsdist, z, Ok(norm_cdf(z)));
num1!(norm_s_inv, p, Ok(norm_inv(open01(p)?)));
num1!(phi, x, Ok(norm_pdf(x)));
num1!(gauss, z, Ok(0.5 * erf(z / std::f64::consts::SQRT_2)));

nums!(lognorm_dist, [x, m, s, cum], {
    let z = (positive(x)?.ln() - m) / positive(s)?;
    Ok(if cum != 0.0 { norm_cdf(z) } else { norm_pdf(z) / (s * x) })
});
nums!(lognormdist, [x, m, s], Ok(norm_cdf((positive(x)?.ln() - m) / positive(s)?)));
nums!(lognorm_inv, [p, m, s], Ok((m + norm_inv(open01(p)?) * positive(s)?).exp()));

// Contínuas

nums!(expon_dist, [x, l, cum], {
    if x < 0.0 || l <= 0.0 {
        return Err(NUM);
    }
    Ok(if cum != 0.0 { -(-l * x).exp_m1() } else { l * (-l * x).exp() })
});

nums!(weibull, [x, a, b, cum], {
    if x < 0.0 || a <= 0.0 || b <= 0.0 {
        return Err(NUM);
    }
    let t = (x / b).powf(a);
    Ok(if cum != 0.0 { -(-t).exp_m1() } else { a / b * (x / b).powf(a - 1.0) * (-t).exp() })
});

nums!(gamma_dist, [x, a, b, cum], {
    if x < 0.0 || a <= 0.0 || b <= 0.0 {
        return Err(NUM);
    }
    if cum != 0.0 {
        return Ok(V::num(gamma_pq(a, x / b).0));
    }
    if x == 0.0 {
        return if a < 1.0 { Err(NUM) } else { Ok(V::num(if a == 1.0 { 1.0 / b } else { 0.0 })) };
    }
    Ok(poisson_pmf(a, x / b) * a / x)
});

fn gamma_quantile(p: f64, a: f64, b: f64) -> f64 {
    if p == 0.0 { 0.0 } else { invert(p, 0.0, f64::INFINITY, |x| gamma_pq(a, x).0) * b }
}

nums!(gamma_inv, [p, a, b], {
    if !(0.0..1.0).contains(&p) || a <= 0.0 || b <= 0.0 { Err(NUM) } else { Ok(gamma_quantile(p, a, b)) }
});

num1!(gamma_fn, x, if x == 0.0 || (x < 0.0 && x == x.trunc()) { Err(NUM) } else { Ok(gamma(x)) });
num1!(gammaln, x, Ok(ln_gamma(positive(x)?)));

/// x na escala [0, 1] da beta entre `lo` e `hi`.
fn unit(x: f64, a: f64, b: f64, lo: f64, hi: f64) -> Res<f64> {
    if a <= 0.0 || b <= 0.0 || x < lo || x > hi || lo == hi { Err(NUM) } else { Ok((x - lo) / (hi - lo)) }
}

nums!(beta_dist, [x, a, b, cum, lo = 0.0, hi = 1.0], {
    let t = unit(x, a, b, lo, hi)?;
    if cum != 0.0 {
        return Ok(V::num(beta_inc(a, b, t, 1.0 - t).0));
    }
    if (t == 0.0 && a < 1.0) || (t == 1.0 && b < 1.0) {
        return Err(NUM);
    }
    let d = if t == 0.0 {
        if a == 1.0 { b } else { 0.0 }
    } else if t == 1.0 {
        if b == 1.0 { a } else { 0.0 }
    } else {
        beta_kernel(a, b, t, 1.0 - t) / (t * (1.0 - t))
    };
    Ok(d / (hi - lo))
});
nums!(betadist, [x, a, b, lo = 0.0, hi = 1.0], {
    let t = unit(x, a, b, lo, hi)?;
    Ok(beta_inc(a, b, t, 1.0 - t).0)
});
nums!(beta_inv, [p, a, b, lo = 0.0, hi = 1.0], {
    if a <= 0.0 || b <= 0.0 || lo == hi {
        return Err(NUM);
    }
    let p = open01(p)?;
    Ok(lo + (hi - lo) * invert(p, 0.0, 1.0, |t| beta_inc(a, b, t, 1.0 - t).0))
});

// Qui-quadrado

nums!(chisq_dist, [x, df, cum], {
    let k = degrees(df)? / 2.0;
    if x < 0.0 {
        return Err(NUM);
    }
    if cum != 0.0 {
        return Ok(V::num(gamma_pq(k, x / 2.0).0));
    }
    if x == 0.0 {
        return match k {
            k if k < 1.0 => Err(NUM),
            1.0 => Ok(V::Num(0.5)),
            _ => Ok(V::Num(0.0)),
        };
    }
    Ok(poisson_pmf(k, x / 2.0) * k / x)
});
nums!(chisq_rt, [x, df], {
    let k = degrees(df)? / 2.0;
    if x < 0.0 { Err(NUM) } else { Ok(gamma_pq(k, x / 2.0).1) }
});
nums!(chisq_inv, [p, df], {
    let k = degrees(df)? / 2.0;
    if !(0.0..1.0).contains(&p) { Err(NUM) } else { Ok(gamma_quantile(p, k, 2.0)) }
});
nums!(chisq_inv_rt, [p, df], {
    let k = degrees(df)? / 2.0;
    if !(p > 0.0 && p <= 1.0) {
        return Err(NUM);
    }
    if p == 1.0 { Ok(0.0) } else { Ok(invert(-p, 0.0, f64::INFINITY, |x| -gamma_pq(k, x).1) * 2.0) }
});

// t de Student

nums!(t_dist, [x, df, cum], {
    let d = degrees(df)?;
    Ok(if cum != 0.0 { t_upper(-x, d) } else { t_pdf(x, d) })
});
nums!(t_dist_rt, [x, df], Ok(t_upper(x, degrees(df)?)));
nums!(t_dist_2t, [x, df], {
    let d = degrees(df)?;
    if x < 0.0 { Err(NUM) } else { Ok(2.0 * t_upper(x, d)) }
});
nums!(tdist, [x, df, tails], {
    let d = degrees(df)?;
    let tails = tails.trunc();
    if x < 0.0 || !(tails == 1.0 || tails == 2.0) { Err(NUM) } else { Ok(tails * t_upper(x, d)) }
});
nums!(t_inv, [p, df], {
    let d = degrees(df)?;
    Ok(-t_upper_inv(open01(p)?, d))
});
nums!(t_inv_2t, [p, df], {
    let d = degrees(df)?;
    if !(p > 0.0 && p <= 1.0) { Err(NUM) } else { Ok(t_upper_inv(p / 2.0, d)) }
});

// F

nums!(f_dist, [x, d1, d2, cum], {
    let (a, b) = (degrees(d1)?, degrees(d2)?);
    if x < 0.0 {
        return Err(NUM);
    }
    if cum != 0.0 {
        return Ok(V::num(f_pq(x, a, b).0));
    }
    if x == 0.0 {
        return match a {
            1.0 => Err(NUM),
            2.0 => Ok(V::Num(1.0)),
            _ => Ok(V::Num(0.0)),
        };
    }
    let s = a * x + b;
    Ok(beta_kernel(a / 2.0, b / 2.0, a * x / s, b / s) / x)
});
nums!(f_rt, [x, d1, d2], {
    let (a, b) = (degrees(d1)?, degrees(d2)?);
    if x < 0.0 { Err(NUM) } else { Ok(f_pq(x, a, b).1) }
});
nums!(f_inv, [p, d1, d2], {
    let (a, b) = (degrees(d1)?, degrees(d2)?);
    if !(0.0..1.0).contains(&p) {
        return Err(NUM);
    }
    if p == 0.0 { Ok(0.0) } else { Ok(invert(p, 0.0, f64::INFINITY, |x| f_pq(x, a, b).0)) }
});
nums!(f_inv_rt, [p, d1, d2], {
    let (a, b) = (degrees(d1)?, degrees(d2)?);
    if !(p > 0.0 && p <= 1.0) {
        return Err(NUM);
    }
    if p == 1.0 { Ok(0.0) } else { Ok(invert(-p, 0.0, f64::INFINITY, |x| -f_pq(x, a, b).1)) }
});

// Discretas

/// P(X <= k) da binomial.
fn binom_cdf(k: f64, n: f64, p: f64) -> f64 {
    if k >= n { 1.0 } else { beta_inc(n - k, k + 1.0, 1.0 - p, p).0 }
}

nums!(binom_dist, [k, n, p, cum], {
    let (k, n) = (k.trunc(), n.trunc());
    if k < 0.0 || k > n || !(0.0..=1.0).contains(&p) {
        return Err(NUM);
    }
    Ok(if cum != 0.0 { binom_cdf(k, n, p) } else { binom_pmf(k, n, p, 1.0 - p) })
});

nums!(binom_inv, [n, p, alpha], {
    let n = n.trunc();
    if n < 0.0 || !(0.0..=1.0).contains(&p) || !(0.0..=1.0).contains(&alpha) {
        return Err(NUM);
    }
    let (mut lo, mut hi) = (0.0f64, n);
    while lo < hi {
        let m = ((lo + hi) / 2.0).floor();
        if binom_cdf(m, n, p) >= alpha {
            hi = m;
        } else {
            lo = m + 1.0;
        }
    }
    Ok(lo)
});

fn binom_range<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let n = e.num(a[0])?.trunc();
    let p = e.num(a[1])?;
    let s = e.num(a[2])?.trunc();
    let t = e.opt_num(a, 3, s)?.trunc();
    if n < 0.0 || !(0.0..=1.0).contains(&p) || s < 0.0 || s > n || t < s || t > n {
        return Err(NUM);
    }
    if t == s {
        return Ok(V::num(binom_pmf(s, n, p, 1.0 - p)));
    }
    let below = if s == 0.0 { 0.0 } else { binom_cdf(s - 1.0, n, p) };
    Ok(V::num(binom_cdf(t, n, p) - below))
}

fn negbinom(f: f64, s: f64, p: f64, cum: bool) -> Res<f64> {
    let (f, s) = (f.trunc(), s.trunc());
    if f < 0.0 || s < 1.0 || !(p > 0.0 && p < 1.0) {
        return Err(NUM);
    }
    Ok(if cum { beta_inc(s, f + 1.0, p, 1.0 - p).0 } else { binom_pmf(s, f + s, p, 1.0 - p) * s / (f + s) })
}

nums!(negbinom_dist, [f, s, p, cum], negbinom(f, s, p, cum != 0.0));
nums!(negbinomdist, [f, s, p], negbinom(f, s, p, false));

nums!(poisson, [x, m, cum], {
    let x = x.trunc();
    if x < 0.0 || m < 0.0 {
        return Err(NUM);
    }
    Ok(if cum == 0.0 {
        poisson_pmf(x, m)
    } else if m == 0.0 {
        1.0
    } else {
        gamma_pq(x + 1.0, m).1
    })
});

fn hypgeom(x: f64, n: f64, big_m: f64, big_n: f64, cum: bool) -> Res<f64> {
    let (x, n, m, t) = (x.trunc(), n.trunc(), big_m.trunc(), big_n.trunc());
    let lo = (n + m - t).max(0.0);
    if x < lo || x > n.min(m) || n <= 0.0 || n > t || m <= 0.0 || m > t || t <= 0.0 {
        return Err(NUM);
    }
    let p = n / t;
    let pmf = |k: f64| {
        binom_pmf(k, m, p, 1.0 - p) * binom_pmf(n - k, t - m, p, 1.0 - p) / binom_pmf(n, t, p, 1.0 - p)
    };
    if !cum {
        return Ok(pmf(x));
    }
    let mut s = 0.0;
    let mut k = lo;
    while k <= x {
        s += pmf(k);
        k += 1.0;
    }
    Ok(s.min(1.0))
}

nums!(hypgeom_dist, [x, n, m, t, cum], hypgeom(x, n, m, t, cum != 0.0));
nums!(hypgeomdist, [x, n, m, t], hypgeom(x, n, m, t, false));

// Confiança e testes

nums!(confidence, [alpha, s, n], {
    let n = n.trunc();
    if n < 1.0 { Err(NUM) } else { Ok(-norm_inv(open01(alpha)? / 2.0) * positive(s)? / n.sqrt()) }
});
nums!(confidence_t, [alpha, s, n], {
    let n = n.trunc();
    if n < 1.0 {
        return Err(NUM);
    }
    let (a, s) = (open01(alpha)?, positive(s)?);
    if n == 1.0 { Err(CellError::Div0) } else { Ok(t_upper_inv(a / 2.0, n - 1.0) * s / n.sqrt()) }
});

num1!(fisher, x, if x <= -1.0 || x >= 1.0 { Err(NUM) } else { Ok(x.atanh()) });
num1!(fisherinv, y, Ok(y.tanh()));

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn t_test<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let tails = e.num(a[2])?.trunc();
    let kind = e.num(a[3])?.trunc() as i64;
    if !(tails == 1.0 || tails == 2.0) || !(1..=3).contains(&kind) {
        return Err(NUM);
    }
    let (t, df) = if kind == 1 {
        let p = pairs(e, a[0], a[1])?;
        let d: Vec<f64> = p.iter().map(|(x, y)| x - y).collect();
        let n = d.len() as f64;
        (mean(&d) / (variance(&d, true)? / n).sqrt(), n - 1.0)
    } else {
        let x = arr_nums(e, a[0])?;
        let y = arr_nums(e, a[1])?;
        let (n, m) = (x.len() as f64, y.len() as f64);
        let (v, w) = (variance(&x, true)?, variance(&y, true)?);
        let d = mean(&x) - mean(&y);
        if kind == 2 {
            let pooled = ((n - 1.0) * v + (m - 1.0) * w) / (n + m - 2.0);
            (d / (pooled * (1.0 / n + 1.0 / m)).sqrt(), n + m - 2.0)
        } else {
            let (p, q) = (v / n, w / m);
            (d / (p + q).sqrt(), (p + q) * (p + q) / (p * p / (n - 1.0) + q * q / (m - 1.0)))
        }
    };
    if !t.is_finite() {
        return Err(CellError::Div0);
    }
    Ok(V::num(tails * t_upper(t.abs(), df)))
}

fn f_test<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = arr_nums(e, a[0])?;
    let y = arr_nums(e, a[1])?;
    let (v, w) = (variance(&x, true)?, variance(&y, true)?);
    if v == 0.0 || w == 0.0 {
        return Err(CellError::Div0);
    }
    let (p, q) = f_pq(v / w, x.len() as f64 - 1.0, y.len() as f64 - 1.0);
    Ok(V::num(2.0 * p.min(q)))
}

fn z_test<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = arr_nums(e, a[0])?;
    let x = e.num(a[1])?;
    if v.is_empty() {
        return Err(CellError::NA);
    }
    let s = match a.get(2) {
        Some(&n) if !e.is_missing(n) => positive(e.num(n)?)?,
        _ => variance(&v, true)?.sqrt(),
    };
    if s == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(V::num(norm_cdf(-(mean(&v) - x) / (s / (v.len() as f64).sqrt()))))
}

fn chisq_test<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.arr(a[0])?;
    let y = e.arr(a[1])?;
    if (x.rows, x.cols) != (y.rows, y.cols) || x.v.len() < 2 {
        return Err(CellError::NA);
    }
    let mut s = 0.0;
    for (p, q) in x.v.iter().zip(&y.v) {
        match (p, q) {
            (V::Err(x), _) | (_, V::Err(x)) => return Err(*x),
            (V::Num(o), V::Num(w)) => {
                if *w == 0.0 {
                    return Err(CellError::Div0);
                }
                if *w < 0.0 {
                    return Err(NUM);
                }
                s += (o - w) * (o - w) / w;
            }
            _ => {}
        }
    }
    let df = if x.rows == 1 || x.cols == 1 { x.v.len() as u32 - 1 } else { (x.rows - 1) * (x.cols - 1) };
    Ok(V::num(gamma_pq(df as f64 / 2.0, s / 2.0).1))
}

fn prob<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = pairs(e, a[0], a[1])?;
    let lo = e.num(a[2])?;
    let hi = e.opt_num(a, 3, lo)?;
    let total: f64 = v.iter().map(|p| p.1).sum();
    if v.iter().any(|p| p.1 <= 0.0 || p.1 > 1.0) || (total - 1.0).abs() > 1e-9 {
        return Err(NUM);
    }
    Ok(V::num(v.iter().filter(|p| p.0 >= lo && p.0 <= hi).map(|p| p.1).sum()))
}
