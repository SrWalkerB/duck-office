//! O que o usuário digita numa célula, interpretado como o Excel em
//! português (F3).
//!
//! - `=…` é fórmula; `'` no começo força texto.
//! - Números com vírgula decimal e ponto de milhar (`1.234,56`, `-3,5`),
//!   percentuais (`10%`), moeda (`R$ 1.234,56`), científico (`1,5E+3`).
//! - Datas `22/11/2026`, `22/11/26`, `22/11`; horas `14:30`, `14:30:15`; os dois juntos.
//! - `VERDADEIRO`/`FALSO` (e `TRUE`/`FALSE`), erros (`#N/D`, `#DIV/0!`…).
//!
//! Junto com o valor vem o formato de número que o Excel aplicaria numa
//! célula sem formato (id interno do `styles.xml`).

use crate::{CellError, CellValue};

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// Valor e o formato sugerido (`None` = deixa como está).
    Value(CellValue, Option<u16>),
    /// Fórmula, sem o `=`.
    Formula(String),
}

/// Formatos internos usados.
pub mod fmt {
    pub const THOUSANDS: u16 = 3;
    pub const THOUSANDS_DEC: u16 = 4;
    pub const CURRENCY: u16 = 7;
    pub const PERCENT: u16 = 9;
    pub const PERCENT_DEC: u16 = 10;
    pub const SCIENTIFIC: u16 = 11;
    pub const DATE: u16 = 14;
    pub const TIME: u16 = 20;
    pub const TIME_SEC: u16 = 21;
    pub const DATE_TIME: u16 = 22;
}

pub fn parse_input(s: &str, date1904: bool, today: (i32, u32, u32)) -> Input {
    if let Some(rest) = s.strip_prefix('\'') {
        return Input::Value(CellValue::Text(rest.to_string()), None);
    }
    if let Some(f) = s.strip_prefix('=')
        && !f.trim().is_empty()
    {
        return Input::Formula(f.trim().to_string());
    }
    let t = s.trim();
    if t.is_empty() {
        return Input::Value(CellValue::Empty, None);
    }
    let v = |x: f64, f: Option<u16>| Input::Value(CellValue::Number(x), f);
    match t.to_uppercase().as_str() {
        "VERDADEIRO" | "TRUE" => return Input::Value(CellValue::Bool(true), None),
        "FALSO" | "FALSE" => return Input::Value(CellValue::Bool(false), None),
        _ => {}
    }
    if t.starts_with('#') {
        let e = match t.to_uppercase().as_str() {
            "#N/D" | "#N/A" => Some(CellError::NA),
            "#DIV/0!" => Some(CellError::Div0),
            "#VALOR!" | "#VALUE!" => Some(CellError::Value),
            "#REF!" => Some(CellError::Ref),
            "#NOME?" | "#NAME?" => Some(CellError::Name),
            "#NÚM!" | "#NUM!" => Some(CellError::Num),
            "#NULO!" | "#NULL!" => Some(CellError::Null),
            _ => None,
        };
        if let Some(e) = e {
            return Input::Value(CellValue::Error(e), None);
        }
    }
    if let Some(p) = t.strip_suffix('%')
        && let Some((x, n)) = number(p.trim())
    {
        return v(x / 100.0, Some(if n.decimals { fmt::PERCENT_DEC } else { fmt::PERCENT }));
    }
    if let Some((neg, rest)) = currency(t)
        && let Some((x, _)) = number(rest.trim())
    {
        return v(if neg { -x } else { x }, Some(fmt::CURRENCY));
    }
    if let Some((x, n)) = number(t) {
        let f = if n.exponent {
            Some(fmt::SCIENTIFIC)
        } else if n.thousands {
            Some(if n.decimals { fmt::THOUSANDS_DEC } else { fmt::THOUSANDS })
        } else {
            None
        };
        return v(x, f);
    }
    if let Some((x, f)) = date_time(t, today) {
        let x = if date1904 && f != fmt::TIME && f != fmt::TIME_SEC { x - 1462.0 } else { x };
        return v(x, Some(f));
    }
    Input::Value(CellValue::Text(s.to_string()), None)
}

#[derive(Default)]
struct Num {
    decimals: bool,
    thousands: bool,
    exponent: bool,
}

/// `R$ 10`, `-R$ 10`, `R$ -10` → (negativo, resto).
fn currency(t: &str) -> Option<(bool, &str)> {
    if let Some(r) = t.strip_prefix("R$") {
        return Some(match r.trim_start().strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, r),
        });
    }
    t.strip_prefix('-')?.trim_start().strip_prefix("R$").map(|r| (true, r))
}

/// Número em pt-BR: vírgula decimal, ponto de milhar em grupos de 3, expoente.
fn number(t: &str) -> Option<(f64, Num)> {
    let mut n = Num::default();
    let (sign, body) = match t.as_bytes().first()? {
        b'-' => (-1.0, &t[1..]),
        b'+' => (1.0, &t[1..]),
        _ => (1.0, t),
    };
    let (mantissa, exp) = match body.find(['e', 'E']) {
        Some(k) => (&body[..k], Some(&body[k + 1..])),
        None => (body, None),
    };
    let (int, frac) = match mantissa.split_once(',') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    if int.is_empty() && frac.is_none_or(str::is_empty) {
        return None;
    }
    let mut digits = String::with_capacity(t.len());
    if int.contains('.') {
        let groups: Vec<&str> = int.split('.').collect();
        if groups[0].is_empty() || groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3) {
            return None;
        }
        n.thousands = true;
    }
    for c in int.chars().filter(|&c| c != '.') {
        if !c.is_ascii_digit() {
            return None;
        }
        digits.push(c);
    }
    if let Some(f) = frac {
        if !f.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        n.decimals = !f.is_empty();
        digits.push('.');
        digits.push_str(f);
    }
    if let Some(e) = exp {
        let e = e.strip_prefix('+').unwrap_or(e);
        let body = e.strip_prefix('-').unwrap_or(e);
        if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        n.exponent = true;
        digits.push('e');
        digits.push_str(e);
    }
    let x: f64 = digits.parse().ok()?;
    x.is_finite().then_some((sign * x, n))
}

/// Data e/ou hora → (número de série, formato).
fn date_time(t: &str, today: (i32, u32, u32)) -> Option<(f64, u16)> {
    let (date, time) = match t.split_once(' ') {
        Some((d, h)) => (Some(d.trim()), Some(h.trim())),
        None if t.contains(':') => (None, Some(t)),
        None => (Some(t), None),
    };
    let day = match date {
        Some(d) => Some(parse_date(d, today)?),
        None => None,
    };
    let (frac, secs) = match time {
        Some(h) => {
            let (f, s) = parse_time(h)?;
            (Some(f), s)
        }
        None => (None, false),
    };
    Some(match (day, frac) {
        (Some(d), Some(f)) => (d + f, fmt::DATE_TIME),
        (Some(d), None) => (d, fmt::DATE),
        (None, Some(f)) => (f, if secs { fmt::TIME_SEC } else { fmt::TIME }),
        (None, None) => return None,
    })
}

fn parse_date(d: &str, today: (i32, u32, u32)) -> Option<f64> {
    let sep = if d.contains('/') { '/' } else { '-' };
    let parts: Vec<&str> = d.split(sep).collect();
    let num = |s: &str| {
        (!s.is_empty() && s.len() <= 4 && s.chars().all(|c| c.is_ascii_digit()))
            .then(|| s.parse::<i64>().ok())
            .flatten()
    };
    let (day, month, year) = match parts.as_slice() {
        [dd, mm] => (num(dd)?, num(mm)?, today.0 as i64),
        [dd, mm, yy] => {
            let y = num(yy)?;
            let y = match yy.len() {
                1 | 2 => {
                    if y < 30 {
                        2000 + y
                    } else {
                        1900 + y
                    }
                }
                4 => y,
                _ => return None,
            };
            (num(dd)?, num(mm)?, y)
        }
        _ => return None,
    };
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month as u32) as i64
        || !(1900..=9999).contains(&year)
    {
        return None;
    }
    Some(serial(year as i32, month as u32, day as u32))
}

fn parse_time(h: &str) -> Option<(f64, bool)> {
    let parts: Vec<&str> = h.split(':').collect();
    let num = |s: &str| {
        (!s.is_empty() && s.len() <= 2 && s.chars().all(|c| c.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    let (hh, mm, ss) = match parts.as_slice() {
        [a, b] => (num(a)?, num(b)?, 0),
        [a, b, c] => (num(a)?, num(b)?, num(c)?),
        _ => return None,
    };
    if hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    Some(((hh * 3600 + mm * 60 + ss) as f64 / 86400.0, parts.len() == 3))
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    }
}

/// Data de hoje (UTC): ano, mês, dia.
pub fn today() -> (i32, u32, u32) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let z = (secs / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// Número de série do Excel (sistema 1900) de uma data.
pub fn serial(y: i32, m: u32, d: u32) -> f64 {
    // Dias desde 1970-01-01 (algoritmo de Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    // 1899-12-30 = dia 0 (já conta o 29/02/1900 que o Excel inventou).
    let s = days + 25569;
    // Antes de 01/03/1900 o Excel está um dia atrás.
    (if s < 61 { s - 1 } else { s }) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: (i32, u32, u32) = (2026, 11, 22);

    fn val(s: &str) -> Input {
        parse_input(s, false, TODAY)
    }

    fn num(x: f64, f: Option<u16>) -> Input {
        Input::Value(CellValue::Number(x), f)
    }

    #[test]
    fn numbers_and_formats() {
        assert_eq!(val("42"), num(42.0, None));
        assert_eq!(val("-3,5"), num(-3.5, None));
        assert_eq!(val("1.234,56"), num(1234.56, Some(fmt::THOUSANDS_DEC)));
        assert_eq!(val("1.234"), num(1234.0, Some(fmt::THOUSANDS)));
        assert_eq!(val("10%"), num(0.1, Some(fmt::PERCENT)));
        assert_eq!(val("12,5%"), num(0.125, Some(fmt::PERCENT_DEC)));
        assert_eq!(val("R$ 1.234,50"), num(1234.5, Some(fmt::CURRENCY)));
        assert_eq!(val("-R$ 10"), num(-10.0, Some(fmt::CURRENCY)));
        assert_eq!(val("1,5E+3"), num(1500.0, Some(fmt::SCIENTIFIC)));
        assert_eq!(val("12.34"), Input::Value(CellValue::Text("12.34".into()), None));
        assert_eq!(val("abc"), Input::Value(CellValue::Text("abc".into()), None));
        assert_eq!(val("'123"), Input::Value(CellValue::Text("123".into()), None));
    }

    #[test]
    fn dates_times_and_others() {
        assert_eq!(val("22/11/2026"), num(46348.0, Some(fmt::DATE)));
        assert_eq!(val("22/11/26"), num(46348.0, Some(fmt::DATE)));
        assert_eq!(val("22/11"), num(46348.0, Some(fmt::DATE)));
        assert_eq!(val("01/01/1900"), num(1.0, Some(fmt::DATE)));
        assert_eq!(val("01/03/1900"), num(61.0, Some(fmt::DATE)));
        assert_eq!(val("31/02/2026"), Input::Value(CellValue::Text("31/02/2026".into()), None));
        assert_eq!(val("14:30"), num(14.5 / 24.0, Some(fmt::TIME)));
        assert_eq!(val("22/11/2026 12:00"), num(46348.5, Some(fmt::DATE_TIME)));
        assert_eq!(parse_input("22/11/2026", true, TODAY), num(46348.0 - 1462.0, Some(fmt::DATE)));
        assert_eq!(val("verdadeiro"), Input::Value(CellValue::Bool(true), None));
        assert_eq!(val("#N/D"), Input::Value(CellValue::Error(CellError::NA), None));
        assert_eq!(val("=SUM(A1:A3)"), Input::Formula("SUM(A1:A3)".into()));
        assert_eq!(val(""), Input::Value(CellValue::Empty, None));
    }
}
