//! Valores durante o cálculo e as conversões do Excel (em português: vírgula
//! decimal, `VERDADEIRO`/`FALSO`).

use std::borrow::Cow;
use std::cmp::Ordering;
use std::rc::Rc;

use duck_core::{CellError, CellValue};

/// De que aba é um intervalo já resolvido.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sid {
    Local(u32),
    Ext(u16, u16),
}

/// Intervalo resolvido (coordenadas absolutas, base 0, inclusivas).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rng {
    pub sheet: Sid,
    pub r0: u32,
    pub c0: u32,
    pub r1: u32,
    pub c1: u32,
}

impl Rng {
    pub fn rows(&self) -> u32 {
        self.r1 - self.r0 + 1
    }

    pub fn cols(&self) -> u32 {
        self.c1 - self.c0 + 1
    }

    pub fn is_cell(&self) -> bool {
        self.r0 == self.r1 && self.c0 == self.c1
    }
}

/// Matriz de valores (sem referências dentro).
#[derive(Clone, Debug, PartialEq)]
pub struct Arr<'e> {
    pub rows: u32,
    pub cols: u32,
    pub v: Vec<V<'e>>,
}

impl<'e> Arr<'e> {
    pub fn get(&self, r: u32, c: u32) -> &V<'e> {
        // Matriz 1×n ou n×1 se estende como no Excel.
        let r = if self.rows == 1 { 0 } else { r };
        let c = if self.cols == 1 { 0 } else { c };
        self.v.get((r * self.cols + c) as usize).unwrap_or(&V::Err(CellError::NA))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum V<'e> {
    Empty,
    Num(f64),
    Bool(bool),
    Err(CellError),
    Str(Cow<'e, str>),
    Ref(Rng),
    Arr(Rc<Arr<'e>>),
    /// Função como valor (LAMBDA, ou o nome de uma função: `BYROW(A1:B3;SOMA)`).
    Fn(Rc<Fun<'e>>),
}

/// Valor de função.
#[derive(Clone, Debug)]
pub enum Fun<'e> {
    /// LAMBDA com as variáveis visíveis onde ela foi escrita.
    Lambda {
        f: &'e crate::parse::Formula,
        params: u32,
        /// Bit `k` = o parâmetro `k` é opcional.
        optional: u64,
        body: u32,
        captured: Vec<V<'e>>,
    },
    /// Função da planilha pelo nome.
    Native(crate::funcs::Func),
    /// Parâmetro opcional que não foi passado.
    Omitted,
}

impl PartialEq for Fun<'_> {
    fn eq(&self, o: &Self) -> bool {
        match (self, o) {
            (Fun::Lambda { f, body, .. }, Fun::Lambda { f: g, body: b, .. }) => {
                std::ptr::eq(*f, *g) && body == b
            }
            (Fun::Native(a), Fun::Native(b)) => a == b,
            (Fun::Omitted, Fun::Omitted) => true,
            _ => false,
        }
    }
}

impl<'e> V<'e> {
    pub fn text(s: impl Into<Cow<'e, str>>) -> Self {
        V::Str(s.into())
    }

    /// Número finito ou `#NÚM!`.
    pub fn num(x: f64) -> Self {
        if x.is_finite() { V::Num(x) } else { V::Err(CellError::Num) }
    }

    pub fn is_err(&self) -> bool {
        matches!(self, V::Err(_))
    }

    pub fn into_owned(self) -> V<'static> {
        match self {
            V::Empty => V::Empty,
            V::Num(x) => V::Num(x),
            V::Bool(b) => V::Bool(b),
            V::Err(e) => V::Err(e),
            V::Str(s) => V::Str(Cow::Owned(s.into_owned())),
            V::Ref(r) => V::Ref(r),
            V::Arr(a) => V::Arr(Rc::new(Arr {
                rows: a.rows,
                cols: a.cols,
                v: a.v.iter().cloned().map(V::into_owned).collect(),
            })),
            V::Fn(_) => V::Err(CellError::Calc),
        }
    }

    /// Valor de célula (referências e matrizes já resolvidas por quem chama).
    pub fn to_cell(&self) -> CellValue {
        match self {
            V::Empty => CellValue::Number(0.0),
            V::Num(x) => CellValue::Number(if *x == 0.0 { 0.0 } else { *x }),
            V::Bool(b) => CellValue::Bool(*b),
            V::Err(e) => CellValue::Error(*e),
            V::Str(s) => CellValue::Text(s.to_string()),
            V::Ref(_) | V::Arr(_) => CellValue::Error(CellError::Value),
            V::Fn(_) => CellValue::Error(CellError::Calc),
        }
    }
}

// ---------------------------------------------------------------------------
// Conversões
// ---------------------------------------------------------------------------

/// Texto → número como o Excel em português faz em contas (`"1,5"*2`):
/// números com vírgula decimal, percentuais, moeda, datas e horas.
pub fn parse_number(s: &str, date1904: bool) -> Option<f64> {
    parse_number_at(s, date1904, today())
}

/// "Hoje" para datas digitadas sem o ano ("26-jan"): o dia do cálculo.
static TODAY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(20000101);

pub fn set_today(y: i32, m: u32, d: u32) {
    TODAY.store(y as u32 * 10000 + m * 100 + d, std::sync::atomic::Ordering::Relaxed);
}

pub fn today() -> (i32, u32, u32) {
    let t = TODAY.load(std::sync::atomic::Ordering::Relaxed);
    ((t / 10000) as i32, t / 100 % 100, t % 100)
}

pub fn parse_number_at(s: &str, date1904: bool, today: (i32, u32, u32)) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // Rápido: só dígitos.
    if t.bytes().all(|b| b.is_ascii_digit()) && t.len() < 16 {
        return t.parse().ok();
    }
    match duck_core::input::parse_input(t, date1904, today) {
        duck_core::input::Input::Value(CellValue::Number(x), _) => Some(x),
        _ => None,
    }
}

/// Número → texto como no Excel em português (até 15 algarismos
/// significativos, vírgula decimal, científico fora da faixa).
pub fn num_to_text(x: f64, out: &mut String) {
    if x == 0.0 {
        out.push('0');
        return;
    }
    // Arredonda para 15 algarismos significativos.
    let r: f64 = format!("{x:.14e}").parse().unwrap_or(x);
    let a = r.abs();
    if !(1e-9..1e15).contains(&a) {
        // 1,23E+15
        let s = format!("{r:.14e}");
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let mant = if mant.contains('.') { mant.trim_end_matches('0').trim_end_matches('.') } else { mant };
        let e: i32 = exp.parse().unwrap_or(0);
        out.push_str(&mant.replace('.', ","));
        out.push('E');
        out.push(if e < 0 { '-' } else { '+' });
        let _ = std::fmt::Write::write_fmt(out, format_args!("{:02}", e.abs()));
        return;
    }
    let s = format!("{r}");
    out.push_str(&s.replace('.', ","));
}

pub fn bool_text(b: bool) -> &'static str {
    if b { "VERDADEIRO" } else { "FALSO" }
}

/// Minúsculas para comparar sem diferenciar maiúsculas (sem alocar se já está).
pub fn fold(s: &str) -> Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase() || b >= 0x80) {
        Cow::Owned(s.to_lowercase())
    } else {
        Cow::Borrowed(s)
    }
}

/// Compara textos sem diferenciar maiúsculas.
pub fn text_cmp(a: &str, b: &str) -> Ordering {
    if a.is_ascii() && b.is_ascii() {
        let (x, y) = (a.as_bytes(), b.as_bytes());
        for (p, q) in x.iter().zip(y) {
            match p.to_ascii_lowercase().cmp(&q.to_ascii_lowercase()) {
                Ordering::Equal => {}
                o => return o,
            }
        }
        return x.len().cmp(&y.len());
    }
    let mut i = a.chars().flat_map(char::to_lowercase);
    let mut j = b.chars().flat_map(char::to_lowercase);
    loop {
        match (i.next(), j.next()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(p), Some(q)) => match p.cmp(&q) {
                Ordering::Equal => {}
                o => return o,
            },
        }
    }
}

pub fn text_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b)
        || (!a.is_ascii() || !b.is_ascii()) && text_cmp(a, b).is_eq()
}

/// Ordem do Excel entre valores escalares: números < textos < lógicos;
/// vazio vale 0, "" ou FALSO conforme o outro lado.
pub fn compare(a: &V, b: &V) -> Ordering {
    fn rank(v: &V) -> u8 {
        match v {
            V::Num(_) => 0,
            V::Str(_) => 1,
            V::Bool(_) => 2,
            _ => 3,
        }
    }
    match (a, b) {
        (V::Empty, V::Empty) => Ordering::Equal,
        (V::Empty, V::Num(y)) => 0f64.partial_cmp(y).unwrap_or(Ordering::Equal),
        (V::Num(x), V::Empty) => x.partial_cmp(&0.0).unwrap_or(Ordering::Equal),
        (V::Empty, V::Str(s)) => "".cmp(s.as_ref()),
        (V::Str(s), V::Empty) => s.as_ref().cmp(""),
        (V::Empty, V::Bool(y)) => false.cmp(y),
        (V::Bool(x), V::Empty) => x.cmp(&false),
        (V::Num(x), V::Num(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (V::Str(x), V::Str(y)) => text_cmp(x, y),
        (V::Bool(x), V::Bool(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: f64) -> String {
        let mut s = String::new();
        num_to_text(x, &mut s);
        s
    }

    #[test]
    fn number_text() {
        assert_eq!(t(1.5), "1,5");
        assert_eq!(t(0.1 + 0.2), "0,3");
        assert_eq!(t(-12.0), "-12");
        assert_eq!(t(1e15), "1E+15");
        assert_eq!(t(123456789012.0), "123456789012");
        assert_eq!(parse_number("1,5", false), Some(1.5));
        assert_eq!(parse_number("12", false), Some(12.0));
        assert_eq!(parse_number("abc", false), None);
    }

    #[test]
    fn ordering() {
        assert_eq!(compare(&V::Num(5.0), &V::text("a")), Ordering::Less);
        assert_eq!(compare(&V::text("ABC"), &V::text("abc")), Ordering::Equal);
        assert_eq!(compare(&V::Empty, &V::Num(0.0)), Ordering::Equal);
        assert!(text_eq("Ação", "AÇÃO"));
    }
}
