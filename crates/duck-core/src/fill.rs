//! Alça de preenchimento: a continuação de uma lista de células, como no
//! Excel em português.
//!
//! - Dois ou mais números seguem a reta (mínimos quadrados). Um número
//!   sozinho soma 1 (13, 14, 15; no Excel, só com Ctrl); uma porcentagem
//!   sozinha se repete (10% não vira 110%).
//! - Com [`Mode::Copy`] (Ctrl ao arrastar uma série), tudo se repete; com
//!   [`Mode::Series`], a porcentagem sozinha também soma 1.
//! - Datas andam em dias, ou em meses e anos quando o passo entre elas é esse;
//!   horas sozinhas andam de hora em hora.
//! - Meses e dias da semana (por extenso e abreviados) dão a volta; texto com
//!   número no fim ou no começo (`Item 1`, `1º lugar`) conta; trimestres
//!   (`T1`, `Trim 1`, `1º trimestre`) vão até 4.
//! - Uma mistura repete o desenho: cada trecho do mesmo tipo tem a sua série.

use crate::CellValue;
use crate::numfmt::serial_to_datetime;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// O que arrastar a alça faz.
    #[default]
    Auto,
    /// Repete as células.
    Copy,
    /// Continua até o que seria repetido (porcentagens sozinhas somam 1).
    Series,
}

/// Como o número da célula aparece.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NumKind {
    #[default]
    Number,
    /// Data (com ou sem hora).
    Date,
    /// Só hora.
    Time,
    /// Porcentagem.
    Percent,
}

/// Uma célula de origem.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub value: CellValue,
    pub num: NumKind,
    /// Tem fórmula: é copiada (as referências andam).
    pub formula: bool,
}

/// O conteúdo da célula `i` da lista continuada.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Igual à célula de origem (valor ou fórmula).
    Copy,
    Value(CellValue),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum List {
    Months,
    MonthsShort,
    Days,
    DaysShort,
}

const MONTHS: [&str; 12] = [
    "janeiro",
    "fevereiro",
    "março",
    "abril",
    "maio",
    "junho",
    "julho",
    "agosto",
    "setembro",
    "outubro",
    "novembro",
    "dezembro",
];
const MONTHS_SHORT: [&str; 12] =
    ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"];
const DAYS: [&str; 7] =
    ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"];
const DAYS_SHORT: [&str; 7] = ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"];

impl List {
    fn items(self) -> &'static [&'static str] {
        match self {
            List::Months => &MONTHS,
            List::MonthsShort => &MONTHS_SHORT,
            List::Days => &DAYS,
            List::DaysShort => &DAYS_SHORT,
        }
    }

    fn find(s: &str) -> Vec<(List, usize)> {
        let l = s.trim().to_lowercase();
        [List::Months, List::MonthsShort, List::Days, List::DaysShort]
            .into_iter()
            .filter_map(|k| Some((k, k.items().iter().position(|m| *m == l)?)))
            .collect()
    }
}

/// Maiúsculas como no texto de exemplo.
fn like_case(s: &str, sample: &str) -> String {
    let sample = sample.trim();
    let letters = || sample.chars().filter(|c| c.is_alphabetic());
    if letters().count() > 1 && letters().all(char::is_uppercase) {
        return s.to_uppercase();
    }
    if letters().next().is_some_and(char::is_uppercase) {
        let mut c = s.chars();
        return c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
    }
    s.to_string()
}

/// Texto com um número: (antes, número, casas com zeros à esquerda, depois).
fn numbered(s: &str) -> Option<(String, i64, usize, String)> {
    let end_digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if end_digits > 0 && end_digits <= 15 && end_digits < s.len() {
        let (a, d) = s.split_at(s.len() - end_digits);
        // `R$ 1.234`, `A-1,5`: o número não é só o fim.
        if a.ends_with([',', '.']) && a[..a.len() - 1].ends_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let w = if d.starts_with('0') { d.len() } else { 0 };
        return Some((a.to_string(), d.parse().ok()?, w, String::new()));
    }
    let start_digits = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if start_digits > 0 && start_digits <= 15 && start_digits < s.len() {
        let (d, b) = s.split_at(start_digits);
        // `1º`, `2ª`, `1o`: só números ordinais (`3 maçãs` também no Excel).
        if b.starts_with([',', '.', '/', ':']) {
            return None;
        }
        let w = if d.starts_with('0') { d.len() } else { 0 };
        return Some((String::new(), d.parse().ok()?, w, b.to_string()));
    }
    None
}

/// O texto em volta do número é de trimestre.
fn is_quarter(before: &str, after: &str) -> bool {
    let b = before.trim().to_lowercase();
    let a = after.trim().trim_start_matches(['º', '°', 'o']).trim().to_lowercase();
    let word = |s: &str| matches!(s, "t" | "tri" | "trim" | "trim." | "trimestre");
    (word(&b) && after.trim().is_empty()) || (b.is_empty() && word(&a))
}

/// Série de um trecho da origem (células seguidas do mesmo tipo).
#[derive(Clone, Debug)]
enum Series {
    Copy,
    /// Reta: valor em 0 e passo.
    Linear(f64, f64),
    /// Data em meses: ano × 12 + mês do primeiro, dia, passo em meses, hora.
    Months {
        first: i64,
        day: u32,
        step: i64,
        time: f64,
    },
    List {
        list: List,
        first: i64,
        step: i64,
    },
    Numbered {
        first: f64,
        step: f64,
        cycle: bool,
    },
}

/// Reta por mínimos quadrados (exata para dois pontos).
fn line(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    if v.len() < 2 {
        return (v.first().copied().unwrap_or(0.0), 0.0);
    }
    let step = v[1] - v[0];
    // Passo constante: sem erro de arredondamento.
    if v.windows(2).all(|w| (w[1] - w[0] - step).abs() <= 1e-12 * step.abs().max(1.0)) {
        return (v[0], step);
    }
    let mx = (n - 1.0) / 2.0;
    let my = v.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, y) in v.iter().enumerate() {
        let dx = i as f64 - mx;
        sxy += dx * (y - my);
        sxx += dx * dx;
    }
    let b = sxy / sxx;
    (my - b * mx, b)
}

/// Tira o ruído do fim (0,1 + 0,2 = 0,30000000000000004 vira 0,3).
fn tidy(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    format!("{x:.14e}").parse().unwrap_or(x)
}

fn add_months(first: i64, day: u32, k: i64, time: f64, date1904: bool) -> f64 {
    let m = first + k;
    let (y, mo) = (m.div_euclid(12), m.rem_euclid(12) + 1);
    let last = match mo {
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    };
    let s = crate::input::serial(y as i32, mo as u32, day.min(last));
    s + time - if date1904 { 1462.0 } else { 0.0 }
}

#[derive(Clone, Debug)]
pub struct Filler {
    items: Vec<Item>,
    /// Trecho (início, tamanho, série) de cada célula.
    runs: Vec<(usize, usize, Series)>,
    run_of: Vec<usize>,
    date1904: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Number,
    Date,
    Time,
    List(List),
    Numbered,
    Other,
}

impl Filler {
    pub fn new(items: Vec<Item>, mode: Mode, date1904: bool) -> Self {
        let mut f = Self { run_of: vec![0; items.len()], items, runs: Vec::new(), date1904 };
        let classes: Vec<Class> = (0..f.items.len()).map(|i| f.class(i)).collect();
        let mut i = 0;
        while i < f.items.len() {
            let mut j = i + 1;
            while j < f.items.len() && classes[j] == classes[i] && f.joins(i, j, classes[i]) {
                j += 1;
            }
            let s = if mode == Mode::Copy { Series::Copy } else { f.series(i, j, classes[i], mode) };
            for k in i..j {
                f.run_of[k] = f.runs.len();
            }
            f.runs.push((i, j - i, s));
            i = j;
        }
        f
    }

    fn class(&self, i: usize) -> Class {
        let it = &self.items[i];
        if it.formula {
            return Class::Other;
        }
        match &it.value {
            CellValue::Number(_) => match it.num {
                NumKind::Number | NumKind::Percent => Class::Number,
                NumKind::Date => Class::Date,
                NumKind::Time => Class::Time,
            },
            CellValue::Text(s) => {
                let l = List::find(s);
                if !l.is_empty() {
                    // `mai` e `maio`: a lista dos vizinhos decide.
                    let pick = l.iter().find(|(k, _)| {
                        self.items.iter().all(|o| match &o.value {
                            CellValue::Text(t) => List::find(t).iter().any(|x| x.0 == *k),
                            _ => true,
                        })
                    });
                    return Class::List(pick.unwrap_or(&l[0]).0);
                }
                if numbered(s).is_some() { Class::Numbered } else { Class::Other }
            }
            _ => Class::Other,
        }
    }

    /// As células `i` e `j` são do mesmo trecho?
    fn joins(&self, i: usize, j: usize, c: Class) -> bool {
        if c != Class::Numbered {
            return true;
        }
        let part = |k: usize| match &self.items[k].value {
            CellValue::Text(s) => numbered(s).map(|(a, _, _, b)| (a.to_lowercase(), b.to_lowercase())),
            _ => None,
        };
        part(i) == part(j)
    }

    fn num(&self, i: usize) -> f64 {
        match self.items[i].value {
            CellValue::Number(x) => x,
            _ => 0.0,
        }
    }

    fn series(&self, a: usize, b: usize, c: Class, mode: Mode) -> Series {
        let n = b - a;
        let vals: Vec<f64> = (a..b).map(|i| self.num(i)).collect();
        match c {
            Class::Number if n == 1 && mode == Mode::Auto && self.items[a].num == NumKind::Percent => {
                Series::Copy
            }
            Class::Number if n == 1 => Series::Linear(vals[0], 1.0),
            Class::Number => {
                let (y, s) = line(&vals);
                Series::Linear(y, s)
            }
            Class::Time if n == 1 => Series::Linear(vals[0], 1.0 / 24.0),
            Class::Date if n == 1 => Series::Linear(vals[0], 1.0),
            Class::Date => self.months(&vals).unwrap_or_else(|| {
                let (y, s) = line(&vals);
                Series::Linear(y, s)
            }),
            Class::Time => {
                let (y, s) = line(&vals);
                Series::Linear(y, s)
            }
            Class::List(list) => {
                let pos: Vec<i64> = (a..b)
                    .filter_map(|i| match &self.items[i].value {
                        CellValue::Text(s) => {
                            List::find(s).into_iter().find(|x| x.0 == list).map(|x| x.1 as i64)
                        }
                        _ => None,
                    })
                    .collect();
                let len = list.items().len() as i64;
                if pos.len() < 2 {
                    return Series::List { list, first: pos.first().copied().unwrap_or(0), step: 1 };
                }
                let step = (pos[1] - pos[0]).rem_euclid(len);
                if pos.windows(2).all(|w| (w[1] - w[0]).rem_euclid(len) == step) {
                    Series::List { list, first: pos[0], step }
                } else {
                    Series::Copy
                }
            }
            Class::Numbered => {
                let parts: Vec<(String, i64, usize, String)> = (a..b)
                    .filter_map(|i| match &self.items[i].value {
                        CellValue::Text(s) => numbered(s),
                        _ => None,
                    })
                    .collect();
                let nums: Vec<f64> = parts.iter().map(|p| p.1 as f64).collect();
                let (first, step) = if n == 1 { (nums[0], 1.0) } else { line(&nums) };
                let cycle =
                    is_quarter(&parts[0].0, &parts[0].3) && nums.iter().all(|x| (1.0..=4.0).contains(x));
                Series::Numbered { first, step, cycle }
            }
            Class::Other => Series::Copy,
        }
    }

    /// Datas com passo de meses inteiros (mesmo dia, ou o último do mês).
    fn months(&self, v: &[f64]) -> Option<Series> {
        let dt: Vec<_> = v.iter().map(|x| serial_to_datetime(*x, self.date1904, 0)).collect::<Option<_>>()?;
        let idx = |i: usize| dt[i].year as i64 * 12 + dt[i].month as i64 - 1;
        let step = idx(1) - idx(0);
        if step == 0 || (2..v.len()).any(|i| idx(i) - idx(i - 1) != step) {
            return None;
        }
        let time = v[0].fract();
        let day = dt.iter().map(|d| d.day).max()?;
        let same = (0..v.len())
            .all(|i| (add_months(idx(0), day, step * i as i64, time, self.date1904) - v[i]).abs() < 1e-6);
        same.then_some(Series::Months { first: idx(0), day, step, time })
    }

    /// O que vai na posição `i` (0 = primeira célula da origem; negativo para
    /// preencher para cima ou para a esquerda) e a célula de origem de que
    /// vêm o formato e a fórmula.
    pub fn at(&self, i: i64) -> (usize, Out) {
        let n = self.items.len() as i64;
        if n == 0 {
            return (0, Out::Copy);
        }
        let src = i.rem_euclid(n) as usize;
        let cycle = i.div_euclid(n);
        let (start, len, ref s) = self.runs[self.run_of[src]];
        // Posição na série do trecho.
        let k = cycle * len as i64 + (src - start) as i64;
        let out = match *s {
            Series::Copy => Out::Copy,
            Series::Linear(y, step) => Out::Value(CellValue::Number(tidy(y + step * k as f64))),
            Series::Months { first, day, step, time } => {
                Out::Value(CellValue::Number(add_months(first, day, step * k, time, self.date1904)))
            }
            Series::List { list, first, step } => {
                let items = list.items();
                let p = (first + step * k).rem_euclid(items.len() as i64) as usize;
                let sample = match &self.items[src].value {
                    CellValue::Text(t) => t.as_str(),
                    _ => "",
                };
                Out::Value(CellValue::Text(like_case(items[p], sample)))
            }
            Series::Numbered { first, step, cycle } => {
                let CellValue::Text(t) = &self.items[src].value else { return (src, Out::Copy) };
                let Some((a, _, w, b)) = numbered(t) else { return (src, Out::Copy) };
                let mut x = (first + step * k as f64).round() as i64;
                if cycle {
                    x = (x - 1).rem_euclid(4) + 1;
                }
                // O Excel não desce abaixo de zero: volta a subir.
                let x = x.unsigned_abs();
                Out::Value(CellValue::Text(format!("{a}{x:0w$}{b}")))
            }
        };
        (src, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(v: CellValue) -> Item {
        Item { value: v, ..Default::default() }
    }

    fn texts(v: &[&str]) -> Vec<Item> {
        v.iter().map(|s| item(CellValue::Text(s.to_string()))).collect()
    }

    fn nums(v: &[f64]) -> Vec<Item> {
        v.iter().map(|x| item(CellValue::Number(*x))).collect()
    }

    fn run(items: Vec<Item>, mode: Mode, range: std::ops::Range<i64>) -> Vec<String> {
        let f = Filler::new(items, mode, false);
        range
            .map(|i| match f.at(i) {
                (_, Out::Value(CellValue::Text(s))) => s,
                (_, Out::Value(CellValue::Number(x))) => format!("{x}"),
                (k, Out::Copy) => format!("={k}"),
                o => format!("{o:?}"),
            })
            .collect()
    }

    #[test]
    fn numbers() {
        assert_eq!(run(nums(&[13.0]), Mode::Auto, 1..3), ["14", "15"]);
        assert_eq!(run(nums(&[13.0]), Mode::Auto, -2..0), ["11", "12"]);
        assert_eq!(run(nums(&[13.0]), Mode::Copy, 1..3), ["=0", "=0"]);
        let percent = vec![Item { num: NumKind::Percent, ..item(CellValue::Number(0.1)) }];
        assert_eq!(run(percent.clone(), Mode::Auto, 1..3), ["=0", "=0"]);
        assert_eq!(run(percent, Mode::Series, 1..2), ["1.1"]);
        assert_eq!(run(nums(&[1.0, 3.0]), Mode::Auto, 2..5), ["5", "7", "9"]);
        assert_eq!(run(nums(&[1.0, 3.0]), Mode::Auto, -2..0), ["-3", "-1"]);
        assert_eq!(run(nums(&[1.0, 3.0]), Mode::Copy, 2..5), ["=0", "=1", "=0"]);
        assert_eq!(run(nums(&[0.1, 0.2]), Mode::Auto, 2..4), ["0.3", "0.4"]);
        // Reta de tendência com três valores.
        assert_eq!(run(nums(&[1.0, 2.0, 4.0]), Mode::Auto, 3..4), ["5.33333333333333"]);
    }

    #[test]
    fn lists() {
        assert_eq!(run(texts(&["jan"]), Mode::Auto, 1..4), ["fev", "mar", "abr"]);
        assert_eq!(run(texts(&["Novembro"]), Mode::Auto, 1..4), ["Dezembro", "Janeiro", "Fevereiro"]);
        assert_eq!(run(texts(&["SEG"]), Mode::Auto, 1..3), ["TER", "QUA"]);
        assert_eq!(
            run(texts(&["segunda-feira", "quarta-feira"]), Mode::Auto, 2..4),
            ["sexta-feira", "domingo"]
        );
        assert_eq!(run(texts(&["sáb"]), Mode::Auto, -2..0), ["qui", "sex"]);
        assert_eq!(run(texts(&["jan", "abr"]), Mode::Auto, 2..4), ["jul", "out"]);
        assert_eq!(run(texts(&["abc"]), Mode::Auto, 1..2), ["=0"]);
    }

    #[test]
    fn numbered_text() {
        assert_eq!(run(texts(&["Item 1"]), Mode::Auto, 1..3), ["Item 2", "Item 3"]);
        assert_eq!(run(texts(&["Item 1", "Item 3"]), Mode::Auto, 2..4), ["Item 5", "Item 7"]);
        assert_eq!(run(texts(&["X007"]), Mode::Auto, 1..3), ["X008", "X009"]);
        assert_eq!(run(texts(&["1º lugar"]), Mode::Auto, 1..3), ["2º lugar", "3º lugar"]);
        assert_eq!(run(texts(&["T3"]), Mode::Auto, 1..4), ["T4", "T1", "T2"]);
        assert_eq!(run(texts(&["4º trimestre"]), Mode::Auto, 1..3), ["1º trimestre", "2º trimestre"]);
        assert_eq!(run(texts(&["Item 1"]), Mode::Copy, 1..3), ["=0", "=0"]);
        // Dois textos diferentes: cada um com a sua série.
        assert_eq!(run(texts(&["A1", "B1"]), Mode::Auto, 2..6), ["A2", "B2", "A3", "B3"]);
    }

    #[test]
    fn dates() {
        let d = |y, m, d| crate::input::serial(y, m, d);
        let date = |v: &[f64]| -> Vec<Item> {
            v.iter()
                .map(|x| Item { value: CellValue::Number(*x), num: NumKind::Date, formula: false })
                .collect()
        };
        let go = |v: &[f64], r: std::ops::Range<i64>| run(date(v), Mode::Auto, r);
        let s = |x: f64| format!("{x}");
        assert_eq!(go(&[d(2026, 12, 31)], 1..3), [s(d(2027, 1, 1)), s(d(2027, 1, 2))]);
        assert_eq!(go(&[d(2026, 1, 1), d(2026, 1, 8)], 2..3), [s(d(2026, 1, 15))]);
        assert_eq!(go(&[d(2026, 1, 15), d(2026, 2, 15)], 2..4), [s(d(2026, 3, 15)), s(d(2026, 4, 15))]);
        // Fim de mês continua no fim do mês.
        assert_eq!(go(&[d(2026, 1, 31), d(2026, 2, 28)], 2..4), [s(d(2026, 3, 31)), s(d(2026, 4, 30))]);
        assert_eq!(go(&[d(2024, 2, 29), d(2025, 2, 28)], 2..3), [s(d(2026, 2, 28))]);
        assert_eq!(go(&[d(2026, 3, 15), d(2026, 4, 15)], -1..0), [s(d(2026, 2, 15))]);
        let time = vec![Item { value: CellValue::Number(0.5), num: NumKind::Time, formula: false }];
        assert_eq!(run(time, Mode::Auto, 1..2), [s(tidy(0.5 + 1.0 / 24.0))]);
    }

    #[test]
    fn mixed_and_formulas() {
        let mut v = texts(&["Loja"]);
        v.extend(nums(&[1.0]));
        assert_eq!(run(v, Mode::Auto, 2..6), ["=0", "2", "=0", "3"]);
        let mut v = nums(&[1.0, 2.0]);
        v.push(Item { value: CellValue::Number(3.0), num: NumKind::Number, formula: true });
        // A fórmula é copiada; os números seguem a reta deles.
        assert_eq!(run(v, Mode::Auto, 3..6), ["3", "4", "=2"]);
    }
}
