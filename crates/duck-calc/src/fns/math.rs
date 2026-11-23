//! Matemática e trigonometria.

use std::f64::consts::PI;

use duck_core::CellError;

use super::{ALL, num1, nums};
use crate::eval::{Eval, Res};
use crate::funcs::{Acc, FnDef, n, nums as each_num};
use crate::index::round15;
use crate::value::V;

const NUMS: &str = "núm1; [núm2]; …";

pub const DEFS: &[FnDef] = &[
    n("SIN", "SEN", 1, 1, sin, "núm", "O seno").lift(ALL),
    n("COS", "COS", 1, 1, cos, "núm", "O cosseno").lift(ALL),
    n("TAN", "TAN", 1, 1, tan, "núm", "A tangente").lift(ALL),
    n("COT", "COT", 1, 1, cot, "núm", "A cotangente").lift(ALL),
    n("SEC", "SEC", 1, 1, sec, "núm", "A secante").lift(ALL),
    n("CSC", "COSEC", 1, 1, csc, "núm", "A cossecante").lift(ALL),
    n("ASIN", "ASEN", 1, 1, asin, "núm", "O arco seno").lift(ALL),
    n("ACOS", "ACOS", 1, 1, acos, "núm", "O arco cosseno").lift(ALL),
    n("ATAN", "ATAN", 1, 1, atan, "núm", "O arco tangente").lift(ALL),
    n("ACOT", "ACOT", 1, 1, acot, "núm", "O arco cotangente").lift(ALL),
    n("ATAN2", "ATAN2", 2, 2, atan2, "núm_x; núm_y", "O ângulo de um ponto").lift(ALL),
    n("SINH", "SENH", 1, 1, sinh, "núm", "O seno hiperbólico").lift(ALL),
    n("COSH", "COSH", 1, 1, cosh, "núm", "O cosseno hiperbólico").lift(ALL),
    n("TANH", "TANH", 1, 1, tanh, "núm", "A tangente hiperbólica").lift(ALL),
    n("COTH", "COTH", 1, 1, coth, "núm", "A cotangente hiperbólica").lift(ALL),
    n("SECH", "SECH", 1, 1, sech, "núm", "A secante hiperbólica").lift(ALL),
    n("CSCH", "COSECH", 1, 1, csch, "núm", "A cossecante hiperbólica").lift(ALL),
    n("ASINH", "ASENH", 1, 1, asinh, "núm", "O seno hiperbólico inverso").lift(ALL),
    n("ACOSH", "ACOSH", 1, 1, acosh, "núm", "O cosseno hiperbólico inverso").lift(ALL),
    n("ATANH", "ATANH", 1, 1, atanh, "núm", "A tangente hiperbólica inversa").lift(ALL),
    n("ACOTH", "ACOTH", 1, 1, acoth, "núm", "A cotangente hiperbólica inversa").lift(ALL),
    n("RADIANS", "RADIANOS", 1, 1, radians, "ângulo", "Graus em radianos").lift(ALL),
    n("DEGREES", "GRAUS", 1, 1, degrees, "ângulo", "Radianos em graus").lift(ALL),
    n("COMBIN", "COMBIN", 2, 2, combin, "núm; núm_escolhido", "O número de combinações").lift(ALL),
    n("COMBINA", "COMBINA", 2, 2, combina, "núm; núm_escolhido", "O número de combinações com repetição")
        .lift(ALL),
    n("PERMUT", "PERMUT", 2, 2, permut, "núm; núm_escolhido", "O número de permutações").lift(ALL),
    n("PERMUTATIONA", "PERMUTAS", 2, 2, permutationa, "núm; núm_escolhido", "As permutações com repetição")
        .lift(ALL),
    n("FACTDOUBLE", "FATDUPLO", 1, 1, factdouble, "núm", "O fatorial duplo").lift(ALL),
    n(
        "MULTINOMIAL",
        "MULTINOMIAL",
        1,
        255,
        multinomial,
        NUMS,
        "O fatorial da soma pelo produto dos fatoriais",
    ),
    n("GCD", "MDC", 1, 255, gcd, NUMS, "O máximo divisor comum"),
    n("LCM", "MMC", 1, 255, lcm, NUMS, "O mínimo múltiplo comum"),
    n("SUMSQ", "SOMAQUAD", 1, 255, sumsq, NUMS, "A soma dos quadrados"),
    n("SQRTPI", "RAIZPI", 1, 1, sqrtpi, "núm", "A raiz quadrada do número vezes pi").lift(ALL),
    n(
        "SERIESSUM",
        "SOMASEQÜÊNCIA",
        4,
        4,
        seriessum,
        "x; n; m; coeficientes",
        "A soma de uma série de potências",
    ),
    n("ROMAN", "ROMANO", 1, 2, roman, "núm; [forma]", "O número em algarismos romanos").lift(ALL),
    n("ARABIC", "ARÁBICO", 1, 1, arabic, "texto", "O número de um texto em algarismos romanos").lift(ALL),
    n("BASE", "BASE", 2, 3, base, "núm; radical; [mín_comp]", "O número em outra base").lift(ALL),
    n("DECIMAL", "DECIMAL", 2, 2, decimal, "texto; radical", "O número decimal de um texto em outra base")
        .lift(ALL),
    n(
        "CEILING.PRECISE",
        "TETO.PRECISO",
        1,
        2,
        ceiling_precise,
        "núm; [significância]",
        "Arredonda para cima",
    )
    .lift(ALL),
    n("ISO.CEILING", "ISO.TETO", 1, 2, ceiling_precise, "núm; [significância]", "Arredonda para cima")
        .lift(ALL),
    n(
        "FLOOR.PRECISE",
        "ARREDMULTB.PRECISO",
        1,
        2,
        floor_precise,
        "núm; [significância]",
        "Arredonda para baixo",
    )
    .lift(ALL),
];

const NUM: CellError = CellError::Num;
const DIV0: CellError = CellError::Div0;

/// Seno e cosseno de números grandes demais não têm precisão: `#NÚM!`.
fn angle(x: f64) -> Res<f64> {
    if x.abs() < 134_217_728.0 { Ok(x) } else { Err(NUM) }
}

/// 1/x, ou `#DIV/0!`.
fn inv(x: f64) -> Res<f64> {
    if x == 0.0 { Err(DIV0) } else { Ok(1.0 / x) }
}

num1!(sin, x, Ok(angle(x)?.sin()));
num1!(cos, x, Ok(angle(x)?.cos()));
num1!(tan, x, Ok(angle(x)?.tan()));
num1!(cot, x, inv(angle(x)?.tan()));
num1!(sec, x, inv(angle(x)?.cos()));
num1!(csc, x, inv(angle(x)?.sin()));
num1!(asin, x, if x.abs() > 1.0 { Err(NUM) } else { Ok(x.asin()) });
num1!(acos, x, if x.abs() > 1.0 { Err(NUM) } else { Ok(x.acos()) });
num1!(atan, x, Ok(x.atan()));
num1!(acot, x, Ok(PI / 2.0 - x.atan()));
nums!(atan2, [x, y], if x == 0.0 && y == 0.0 { Err(DIV0) } else { Ok(y.atan2(x)) });
num1!(sinh, x, Ok(x.sinh()));
num1!(cosh, x, Ok(x.cosh()));
num1!(tanh, x, Ok(x.tanh()));
num1!(coth, x, inv(x.tanh()));
num1!(sech, x, inv(angle(x)?.cosh()));
num1!(csch, x, inv(angle(x)?.sinh()));
num1!(asinh, x, Ok(x.asinh()));
num1!(acosh, x, if x < 1.0 { Err(NUM) } else { Ok(x.acosh()) });
num1!(atanh, x, if x.abs() >= 1.0 { Err(NUM) } else { Ok(x.atanh()) });
num1!(acoth, x, if x.abs() <= 1.0 { Err(NUM) } else { Ok((1.0 / x).atanh()) });
num1!(radians, x, Ok(x * (PI / 180.0)));
num1!(degrees, x, Ok(x / (PI / 180.0)));
num1!(sqrtpi, x, if x < 0.0 { Err(NUM) } else { Ok((x * PI).sqrt()) });

fn choose(n: f64, k: f64) -> f64 {
    let k = k.min(n - k);
    let mut r = 1.0f64;
    let mut i = 1.0;
    while i <= k {
        // Exato enquanto cabe em inteiro.
        r = r * (n - k + i) / i;
        if r < 9e15 {
            r = r.round();
        }
        if r.is_infinite() {
            break;
        }
        i += 1.0;
    }
    r
}

nums!(combin, [x, k], {
    let (x, k) = (x.trunc(), k.trunc());
    if x < 0.0 || k < 0.0 || x < k { Err(NUM) } else { Ok(choose(x, k)) }
});
nums!(combina, [x, k], {
    let (x, k) = (x.trunc(), k.trunc());
    if x < 0.0 || k < 0.0 || (x == 0.0 && k > 0.0) {
        return Err(NUM);
    }
    if k == 0.0 { Ok(1.0) } else { Ok(choose(x + k - 1.0, k)) }
});
nums!(permut, [x, k], {
    let (x, k) = (x.trunc(), k.trunc());
    if x <= 0.0 || k < 0.0 || x < k {
        return Err(NUM);
    }
    let mut r = 1.0f64;
    let mut i = x - k + 1.0;
    while i <= x && r.is_finite() {
        r *= i;
        i += 1.0;
    }
    Ok(r)
});
nums!(permutationa, [x, k], {
    let (x, k) = (x.trunc(), k.trunc());
    if x < 0.0 || k < 0.0 || (x == 0.0 && k > 0.0) { Err(NUM) } else { Ok(x.powf(k)) }
});
num1!(factdouble, x, {
    let x = x.trunc();
    if x < -1.0 {
        return Err(NUM);
    }
    let mut r = 1.0f64;
    let mut i = x;
    while i > 1.0 && r.is_finite() {
        r *= i;
        i -= 2.0;
    }
    Ok(r)
});

fn multinomial<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (mut sum, mut r, mut bad) = (0.0f64, 1.0f64, false);
    each_num(e, a, &mut |x| {
        let x = x.trunc();
        bad |= x < 0.0;
        if !bad {
            sum += x;
            r *= choose(sum, x);
        }
    })?;
    if bad { Err(NUM) } else { Ok(V::num(r)) }
}

/// Inteiros para MDC e MMC.
fn naturals<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<Vec<u64>> {
    let mut out = Vec::new();
    let mut bad = false;
    each_num(e, a, &mut |x| {
        let x = x.trunc();
        if !(0.0..9_007_199_254_740_992.0).contains(&x) {
            bad = true;
        } else {
            out.push(x as u64);
        }
    })?;
    if bad { Err(NUM) } else { Ok(out) }
}

fn gcd2(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn gcd<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(naturals(e, a)?.into_iter().fold(0, gcd2) as f64))
}

fn lcm<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = naturals(e, a)?;
    if v.contains(&0) || v.is_empty() {
        return Ok(V::Num(0.0));
    }
    let mut r: u64 = 1;
    for x in v {
        r = (r / gcd2(r, x)).checked_mul(x).filter(|r| *r < 1 << 53).ok_or(NUM)?;
    }
    Ok(V::Num(r as f64))
}

fn sumsq<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut acc = Acc::default();
    each_num(e, a, &mut |x| acc.add(x * x))?;
    Ok(V::num(acc.get()))
}

fn seriessum<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let p = e.num(a[1])?;
    let step = e.num(a[2])?;
    let mut acc = Acc::default();
    for (i, c) in e.arr(a[3])?.v.iter().enumerate() {
        let c = e.to_num(c)?;
        acc.add(c * x.powf(p + i as f64 * step));
    }
    Ok(V::num(acc.get()))
}

fn roman<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut x = e.num(a[0])?.trunc() as i64;
    let form = match a.get(1) {
        Some(&n) if !e.is_missing(n) => match e.scalar(n) {
            V::Bool(true) => 0,
            V::Bool(false) => 4,
            v => e.to_num(&v)?.trunc() as i64,
        },
        _ => 0,
    };
    if !(0..=3999).contains(&x) || !(0..=4).contains(&form) {
        return Err(CellError::Value);
    }
    const LET: [char; 7] = ['M', 'D', 'C', 'L', 'X', 'V', 'I'];
    const VAL: [i64; 7] = [1000, 500, 100, 50, 10, 5, 1];
    let mut out = String::new();
    for i in 0..4 {
        let mut k = 2 * i;
        let digit = x / VAL[k];
        if digit % 5 == 4 {
            let high = if digit == 4 { k - 1 } else { k - 2 };
            let mut steps = 0;
            // Formas mais curtas: o número que se subtrai pode ser menor.
            while steps < form && k < 6 {
                steps += 1;
                if VAL[high] - VAL[k + 1] <= x {
                    k += 1;
                } else {
                    steps = form;
                }
            }
            out.push(LET[k]);
            out.push(LET[high]);
            x = x + VAL[k] - VAL[high];
        } else {
            if digit > 4 {
                out.push(LET[k - 1]);
            }
            for _ in 0..digit % 5 {
                out.push(LET[k]);
            }
            x %= VAL[k];
        }
    }
    Ok(V::text(out))
}

fn arabic<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let t = s.trim();
    if t.len() > 255 {
        return Err(CellError::Value);
    }
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t),
    };
    let mut vals = Vec::new();
    for c in t.chars() {
        vals.push(match c.to_ascii_uppercase() {
            'M' => 1000i64,
            'D' => 500,
            'C' => 100,
            'L' => 50,
            'X' => 10,
            'V' => 5,
            'I' => 1,
            _ => return Err(CellError::Value),
        });
    }
    let mut r = 0;
    for (i, &x) in vals.iter().enumerate() {
        r += if vals.get(i + 1).is_some_and(|&y| y > x) { -x } else { x };
    }
    Ok(V::Num(if neg { -r } else { r } as f64))
}

fn base<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?.trunc();
    let radix = e.num(a[1])?.trunc();
    let min = e.opt_num(a, 2, 0.0)?.trunc();
    if !(0.0..9_007_199_254_740_992.0).contains(&x)
        || !(2.0..=36.0).contains(&radix)
        || !(0.0..=255.0).contains(&min)
    {
        return Err(NUM);
    }
    let (mut x, radix) = (x as u64, radix as u64);
    let mut d = Vec::new();
    while x > 0 || d.is_empty() {
        d.push(char::from_digit((x % radix) as u32, 36).unwrap_or('0').to_ascii_uppercase());
        x /= radix;
    }
    while d.len() < min as usize {
        d.push('0');
    }
    Ok(V::text(d.into_iter().rev().collect::<String>()))
}

fn decimal<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let radix = e.num(a[1])?.trunc();
    if !(2.0..=36.0).contains(&radix) {
        return Err(NUM);
    }
    let radix = radix as u32;
    let mut t = s.trim();
    if radix == 16 {
        t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    }
    if t.is_empty() || t.len() > 255 {
        return Err(CellError::Value);
    }
    let mut r = 0.0f64;
    for c in t.chars() {
        let d = c.to_digit(radix).ok_or(NUM)?;
        r = r * radix as f64 + d as f64;
    }
    Ok(V::num(r))
}

nums!(ceiling_precise, [x, s = 1.0], Ok(if s == 0.0 { 0.0 } else { round15(x / s.abs()).ceil() * s.abs() }));
nums!(floor_precise, [x, s = 1.0], Ok(if s == 0.0 { 0.0 } else { round15(x / s.abs()).floor() * s.abs() }));
