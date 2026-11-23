//! Datas: frações de ano, 30/360 e dias úteis com fim de semana próprio.

use duck_core::CellError;

use super::ALL;
use crate::eval::{Eval, Res};
use crate::funcs::{FnDef, date_arg, date_serial, days_in_month, holidays, n, ymd};
use crate::value::V;

pub const DEFS: &[FnDef] = &[
    n(
        "YEARFRAC",
        "FRAÇÃOANO",
        2,
        3,
        yearfrac_fn,
        "data_inicial; data_final; [base]",
        "A fração de ano entre duas datas",
    )
    .lift(ALL),
    n(
        "DAYS360",
        "DIAS360",
        2,
        3,
        days360_fn,
        "data_inicial; data_final; [método]",
        "Os dias entre datas num ano de 360 dias",
    )
    .lift(ALL),
    n(
        "NETWORKDAYS.INTL",
        "DIATRABALHOTOTAL.INTL",
        2,
        4,
        networkdays_intl,
        "data_inicial; data_final; [fim_de_semana]; [feriados]",
        "Os dias úteis entre datas, com fim de semana próprio",
    )
    .lift(0b11),
    n(
        "WORKDAY.INTL",
        "DIATRABALHO.INTL",
        2,
        4,
        workday_intl,
        "data_inicial; dias; [fim_de_semana]; [feriados]",
        "A data depois de tantos dias úteis, com fim de semana próprio",
    )
    .lift(0b11),
];

/// Contagem de dias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// 30/360 americano.
    Us30,
    /// Real/real.
    Actual,
    /// Real/360.
    A360,
    /// Real/365.
    A365,
    /// 30/360 europeu.
    Eu30,
}

pub fn basis_arg(e: &mut Eval, a: &[u32], i: usize) -> Res<Basis> {
    Ok(match e.opt_num(a, i, 0.0)?.trunc() as i64 {
        0 => Basis::Us30,
        1 => Basis::Actual,
        2 => Basis::A360,
        3 => Basis::A365,
        4 => Basis::Eu30,
        _ => return Err(CellError::Num),
    })
}

fn last_day(y: i64, m: i64, d: i64) -> bool {
    d == days_in_month(y, m)
}

/// `k` meses depois. `eom`: se `x` é o último dia do mês, o resultado também é.
pub fn add_months(e: &Eval, x: f64, k: i64, eom: bool) -> Res<f64> {
    let (y, m, d, _) = ymd(e, x)?;
    let t = m - 1 + k;
    let (ny, nm) = (y + t.div_euclid(12), t.rem_euclid(12) + 1);
    let len = days_in_month(ny, nm);
    let nd = if eom && last_day(y, m, d) { len } else { d.min(len) };
    date_serial(ny, nm, nd, e.book.date1904)
}

type Ymd = (i64, i64, i64);

fn thirty((y1, m1, d1): Ymd, (y2, m2, d2): Ymd) -> f64 {
    ((y2 - y1) * 360 + (m2 - m1) * 30 + (d2 - d1)) as f64
}

fn eu30(a: Ymd, b: Ymd) -> f64 {
    thirty((a.0, a.1, a.2.min(30)), (b.0, b.1, b.2.min(30)))
}

/// 30/360 americano das funções de títulos e de FRAÇÃOANO.
fn us30(a: Ymd, b: Ymd) -> f64 {
    let feb = |x: Ymd| x.1 == 2 && last_day(x.0, x.1, x.2);
    let (mut d1, mut d2) = (a.2, b.2);
    if feb(a) {
        if feb(b) {
            d2 = 30;
        }
        d1 = 30;
    }
    if d2 == 31 && d1 >= 30 {
        d2 = 30;
    }
    thirty((a.0, a.1, d1.min(30)), (b.0, b.1, d2))
}

/// Dias entre duas datas (`a` <= `b`).
pub fn day_count(e: &Eval, a: f64, b: f64, basis: Basis) -> Res<f64> {
    let p = |x: f64| ymd(e, x).map(|(y, m, d, _)| (y, m, d));
    Ok(match basis {
        Basis::Us30 => us30(p(a)?, p(b)?),
        Basis::Eu30 => eu30(p(a)?, p(b)?),
        _ => b - a,
    })
}

fn leap(y: i64) -> bool {
    days_in_month(y, 2) == 29
}

/// Fração de ano entre duas datas, em qualquer ordem.
pub fn yearfrac(e: &Eval, a: f64, b: f64, basis: Basis) -> Res<f64> {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let (y1, m1, d1, _) = ymd(e, a)?;
    let (y2, m2, d2, _) = ymd(e, b)?;
    if a == b {
        return Ok(0.0);
    }
    Ok(match basis {
        Basis::Us30 | Basis::Eu30 => day_count(e, a, b, basis)? / 360.0,
        Basis::A360 => (b - a) / 360.0,
        Basis::A365 => (b - a) / 365.0,
        Basis::Actual => {
            let within = y1 == y2 || (y1 + 1 == y2 && (m1 > m2 || (m1 == m2 && d1 >= d2)));
            let year = if within {
                // Passa por um 29 de fevereiro, ou está todo num ano bissexto.
                let feb29 = |y: i64| -> Res<bool> {
                    if !leap(y) {
                        return Ok(false);
                    }
                    let f = date_serial(y, 2, 29, e.book.date1904)?;
                    Ok(a <= f && f <= b)
                };
                if (y1 == y2 && leap(y1)) || feb29(y1)? || feb29(y2)? { 366.0 } else { 365.0 }
            } else {
                let years = (y2 - y1 + 1) as f64;
                let jan1 = |y: i64| date_serial(y, 1, 1, e.book.date1904);
                (jan1(y2 + 1)? - jan1(y1)?) / years
            };
            (b - a) / year
        }
    })
}

fn yearfrac_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = date_arg(e, a[0])?.floor();
    let t = date_arg(e, a[1])?.floor();
    let basis = basis_arg(e, a, 2)?;
    Ok(V::num(yearfrac(e, s, t, basis)?))
}

fn days360_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = date_arg(e, a[0])?.floor();
    let t = date_arg(e, a[1])?.floor();
    let eu = e.opt_bool(a, 2, false)?;
    let (y1, m1, d1, _) = ymd(e, s)?;
    let (y2, m2, d2, _) = ymd(e, t)?;
    if eu {
        return Ok(V::Num(eu30((y1, m1, d1), (y2, m2, d2))));
    }
    let d1 = if last_day(y1, m1, d1) { 30 } else { d1 };
    let (mut d2, mut m2) = (d2, m2);
    if d2 == 31 {
        if d1 < 30 {
            // Vira o dia 1º do mês seguinte.
            d2 = 1;
            m2 += 1;
        } else {
            d2 = 30;
        }
    }
    Ok(V::Num(thirty((y1, m1, d1), (y2, m2, d2))))
}

/// Dias da semana de folga (0 = domingo).
fn weekend(e: &mut Eval, a: &[u32], i: usize) -> Res<[bool; 7]> {
    let mut off = [false; 7];
    let v = match a.get(i) {
        Some(&n) if !e.is_missing(n) => e.scalar(n),
        _ => V::Num(1.0),
    };
    match v {
        V::Str(s) if s.len() == 7 && s.bytes().all(|b| b == b'0' || b == b'1') => {
            // Começa na segunda-feira.
            for (k, b) in s.bytes().enumerate() {
                off[(k + 1) % 7] = b == b'1';
            }
        }
        V::Str(_) => return Err(CellError::Value),
        v => match e.to_num(&v)?.trunc() as i64 {
            k @ 1..=7 => {
                // 1 = sábado e domingo, 2 = domingo e segunda...
                off[((k + 5) % 7) as usize] = true;
                off[((k + 6) % 7) as usize] = true;
            }
            k @ 11..=17 => off[(k - 11) as usize] = true,
            _ => return Err(CellError::Num),
        },
    }
    Ok(off)
}

fn networkdays_intl<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = date_arg(e, a[0])?.floor();
    let t = date_arg(e, a[1])?.floor();
    let off = weekend(e, a, 2)?;
    let mut hol = holidays(e, a, 3)?;
    let (lo, hi, sign) = if s <= t { (s, t, 1.0) } else { (t, s, -1.0) };
    let w0 = ymd(e, lo)?.3 as i64;
    ymd(e, hi)?;
    let off_on = |d: f64| off[((w0 + (d - lo) as i64) % 7) as usize];
    let total = (hi - lo) as i64 + 1;
    let work_week = off.iter().filter(|o| !**o).count() as i64;
    let mut count = total / 7 * work_week;
    count += (0..total % 7).filter(|k| !off[((w0 + k) % 7) as usize]).count() as i64;
    hol.sort_by(f64::total_cmp);
    hol.dedup();
    count -= hol.iter().filter(|&&d| (lo..=hi).contains(&d) && !off_on(d)).count() as i64;
    Ok(V::Num(count as f64 * sign))
}

fn workday_intl<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut d = date_arg(e, a[0])?.floor();
    let mut k = e.num(a[1])?.trunc();
    let off = weekend(e, a, 2)?;
    if off.iter().all(|o| *o) {
        return Err(CellError::Value);
    }
    let mut hol = holidays(e, a, 3)?;
    hol.sort_by(f64::total_cmp);
    if k.abs() > 3_000_000.0 {
        return Err(CellError::Num);
    }
    let step = if k < 0.0 { -1.0 } else { 1.0 };
    let mut w = ymd(e, d)?.3 as i64;
    while k != 0.0 {
        d += step;
        w = (w + step as i64).rem_euclid(7);
        if !(0.0..=2_958_465.0).contains(&d) {
            return Err(CellError::Num);
        }
        if !off[w as usize] && hol.binary_search_by(|x| x.total_cmp(&d)).is_err() {
            k -= step;
        }
    }
    Ok(V::Num(d))
}
