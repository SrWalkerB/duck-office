//! Índices de busca e critérios.
//!
//! PROCV, CORRESP, PROCX, CONT.SE e SOMASES com igualdade procuram num
//! índice por valor (hash → posições, em ordem) montado uma vez por vetor
//! (coluna ou linha) e compartilhado entre as threads: um PROCV numa coluna
//! de 100 mil linhas repetido em 100 mil células vira 100 mil consultas de
//! tempo constante, não 10 bilhões de comparações.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use crate::eval::Book;
use crate::value::{Rng, V, compare, fold, parse_number, text_eq};

/// Hasher simples e rápido (estilo FxHash): as chaves já vêm misturadas.
#[derive(Default, Clone, Copy)]
pub struct Fx(u64);

impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(*b as u64);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

/// Número com 15 algarismos significativos (o que o Excel compara).
pub fn round15(x: f64) -> f64 {
    if x == x.trunc() || !x.is_finite() {
        return x;
    }
    format!("{x:.14e}").parse().unwrap_or(x)
}

fn mix(h: u64) -> u64 {
    let mut x = h ^ (h >> 33);
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

/// Chave de busca de um valor (sem diferenciar maiúsculas). `None` = não
/// entra no índice (vazio, erro).
pub fn key_hash(v: &V) -> Option<u64> {
    match v {
        V::Num(x) => {
            let x = round15(*x);
            let bits = if x == 0.0 { 0 } else { x.to_bits() };
            Some(mix(bits ^ 0x1111))
        }
        V::Str(s) => {
            let mut h = Fx(0x2222);
            if s.is_ascii() {
                for b in s.bytes() {
                    h.write_u64(b.to_ascii_lowercase() as u64);
                }
            } else {
                for c in s.chars().flat_map(char::to_lowercase) {
                    h.write_u64(c as u64);
                }
            }
            Some(mix(h.finish()))
        }
        V::Bool(b) => Some(mix(0x3333 + *b as u64)),
        _ => None,
    }
}

/// Igualdade de busca exata (PROCV com FALSO): mesmo tipo, texto sem
/// diferenciar maiúsculas, números com 15 algarismos.
pub fn lookup_eq(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Num(x), V::Num(y)) => x == y || round15(*x) == round15(*y),
        (V::Str(x), V::Str(y)) => text_eq(x, y),
        (V::Bool(x), V::Bool(y)) => x == y,
        _ => false,
    }
}

/// Índice de um vetor: posições (deslocamento no vetor) agrupadas por chave.
pub struct ExactIndex {
    map: FxMap<u64, (u32, u32)>,
    pos: Vec<u32>,
}

impl ExactIndex {
    /// `numeric_text`: textos que são números também entram com a chave do
    /// número (critérios de CONT.SE/SOMASES casam `5` com `"5"`; PROCV não).
    fn build(book: &Book, vec: &Rng, numeric_text: bool) -> Self {
        let horizontal = vec.r0 == vec.r1 && vec.c0 != vec.c1;
        let mut keys: Vec<(u64, u32)> = Vec::new();
        book.each(vec, |row, col, v| {
            if let Some(h) = key_hash(&v) {
                let off = if horizontal { col - vec.c0 } else { row - vec.r0 };
                keys.push((h, off));
                if numeric_text
                    && let V::Str(s) = &v
                    && s.bytes().any(|b| b.is_ascii_digit())
                    && let Some(x) = parse_number(s, book.date1904)
                    && let Some(h2) = key_hash(&V::Num(x))
                {
                    keys.push((h2, off));
                }
            }
            true
        });
        keys.sort_unstable();
        let mut map = FxMap::default();
        map.reserve(keys.len() / 2);
        let mut pos = Vec::with_capacity(keys.len());
        let mut i = 0;
        while i < keys.len() {
            let h = keys[i].0;
            let start = pos.len() as u32;
            while i < keys.len() && keys[i].0 == h {
                pos.push(keys[i].1);
                i += 1;
            }
            map.insert(h, (start, pos.len() as u32 - start));
        }
        Self { map, pos }
    }

    /// Posições candidatas (mesmo hash), em ordem crescente.
    pub fn candidates(&self, key: &V) -> &[u32] {
        match key_hash(key).and_then(|h| self.map.get(&h)) {
            Some(&(s, n)) => &self.pos[s as usize..(s + n) as usize],
            None => &[],
        }
    }
}

/// Vetores pequenos são varridos direto (montar o índice não compensa).
pub const MIN_INDEXED: u32 = 48;

type Slots = Mutex<HashMap<Rng, Arc<OnceLock<ExactIndex>>>>;

#[derive(Default)]
pub struct Cache {
    exact: Slots,
    criteria: Slots,
}

impl Cache {
    /// Índice do vetor `vec` (uma coluna ou linha) para buscas exatas.
    /// Montado uma vez; threads que pedem o mesmo índice esperam quem está
    /// montando.
    pub fn exact(&self, book: &Book, vec: &Rng) -> Arc<OnceLock<ExactIndex>> {
        let cell = self.exact.lock().unwrap().entry(*vec).or_default().clone();
        cell.get_or_init(|| ExactIndex::build(book, vec, false));
        cell
    }

    /// Índice para critérios (textos numéricos também pela chave do número).
    pub fn criteria(&self, book: &Book, vec: &Rng) -> Arc<OnceLock<ExactIndex>> {
        let cell = self.criteria.lock().unwrap().entry(*vec).or_default().clone();
        cell.get_or_init(|| ExactIndex::build(book, vec, true));
        cell
    }

    pub fn clear(&self) {
        self.exact.lock().unwrap().clear();
        self.criteria.lock().unwrap().clear();
    }

    /// Descarta os índices que cobrem células da aba `sheet` nas colunas
    /// `c0..=c1` (os valores delas mudaram).
    pub fn invalidate(&self, sheet: crate::value::Sid, c0: u32, c1: u32) {
        for m in [&self.exact, &self.criteria] {
            m.lock().unwrap().retain(|r, _| r.sheet != sheet || r.c1 < c0 || r.c0 > c1);
        }
    }

    pub fn len(&self) -> usize {
        self.exact.lock().unwrap().len() + self.criteria.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Vetor efetivo de um intervalo de busca: colunas inteiras param na última
/// linha usada da aba.
pub fn clamp(book: &Book, r: &Rng) -> Rng {
    let rows = book.used_rows(r.sheet).max(1);
    let cols = book.used_cols(r.sheet).max(1);
    Rng { r1: r.r1.min(r.r0.max(rows - 1)), c1: r.c1.min(r.c0.max(cols - 1)), ..*r }
}

/// Primeira posição de `key` no vetor (busca exata, sem curingas).
pub fn find_first(book: &Book, cache: &Cache, vec: &Rng, key: &V) -> Option<u32> {
    let vec = clamp(book, vec);
    let horizontal = vec.r0 == vec.r1 && vec.c0 != vec.c1;
    let len = if horizontal { vec.cols() } else { vec.rows() };
    let at = |o: u32| {
        if horizontal {
            book.get(vec.sheet, vec.r0, vec.c0 + o)
        } else {
            book.get(vec.sheet, vec.r0 + o, vec.c0)
        }
    };
    if len < MIN_INDEXED {
        return (0..len).find(|&o| lookup_eq(&at(o), key));
    }
    let idx = cache.exact(book, &vec);
    let idx = idx.get().unwrap();
    idx.candidates(key).iter().copied().find(|&o| lookup_eq(&at(o), key))
}

/// Última posição de `key` (PROCX de trás para a frente).
pub fn find_last(book: &Book, cache: &Cache, vec: &Rng, key: &V) -> Option<u32> {
    let vec = clamp(book, vec);
    let horizontal = vec.r0 == vec.r1 && vec.c0 != vec.c1;
    let len = if horizontal { vec.cols() } else { vec.rows() };
    let at = |o: u32| {
        if horizontal {
            book.get(vec.sheet, vec.r0, vec.c0 + o)
        } else {
            book.get(vec.sheet, vec.r0 + o, vec.c0)
        }
    };
    if len < MIN_INDEXED {
        return (0..len).rev().find(|&o| lookup_eq(&at(o), key));
    }
    let idx = cache.exact(book, &vec);
    let idx = idx.get().unwrap();
    idx.candidates(key).iter().rev().copied().find(|&o| lookup_eq(&at(o), key))
}

// ---------------------------------------------------------------------------
// Curingas
// ---------------------------------------------------------------------------

/// O texto tem `*` ou `?` não escapados (`~`)?
pub fn has_wildcards(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'~' => i += 2,
            b'*' | b'?' => return true,
            _ => i += 1,
        }
    }
    false
}

/// Padrão com `*`, `?` e `~` (escape), sem diferenciar maiúsculas.
pub struct Wild {
    pat: Vec<WTok>,
}

#[derive(Clone, Copy, PartialEq)]
enum WTok {
    Ch(char),
    Any,
    Star,
}

impl Wild {
    pub fn new(p: &str) -> Self {
        let mut pat = Vec::new();
        let mut it = p.chars().flat_map(char::to_lowercase).peekable();
        while let Some(c) = it.next() {
            match c {
                '~' => {
                    if let Some(n) = it.next() {
                        pat.push(WTok::Ch(n));
                    }
                }
                '*' => {
                    if pat.last() != Some(&WTok::Star) {
                        pat.push(WTok::Star)
                    }
                }
                '?' => pat.push(WTok::Any),
                c => pat.push(WTok::Ch(c)),
            }
        }
        Self { pat }
    }

    pub fn matches(&self, s: &str) -> bool {
        let t: Vec<char> = s.chars().flat_map(char::to_lowercase).collect();
        let p = &self.pat;
        let (mut i, mut j) = (0usize, 0usize);
        let (mut star, mut mark) = (usize::MAX, 0usize);
        while i < t.len() {
            if j < p.len() && (p[j] == WTok::Any || p[j] == WTok::Ch(t[i])) {
                i += 1;
                j += 1;
            } else if j < p.len() && p[j] == WTok::Star {
                star = j;
                mark = i;
                j += 1;
            } else if star != usize::MAX {
                j = star + 1;
                mark += 1;
                i = mark;
            } else {
                return false;
            }
        }
        while j < p.len() && p[j] == WTok::Star {
            j += 1;
        }
        j == p.len()
    }

    /// Posição (em caracteres) do primeiro trecho de `s` que casa, a partir
    /// de `from` (para LOCALIZAR com curingas).
    pub fn find_in(&self, s: &str, from: usize) -> Option<usize> {
        let chars: Vec<char> = s.chars().collect();
        for st in from..=chars.len() {
            for end in st..=chars.len() {
                let sub: String = chars[st..end].iter().collect();
                if self.matches(&sub) {
                    return Some(st);
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Critérios (CONT.SE, SOMASES...)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

pub enum Crit {
    /// Igual a um número (ou texto que vira o mesmo número).
    Num(Op, f64),
    /// Comparação de texto (sem diferenciar maiúsculas).
    Text(Op, String),
    Wild(bool, Wild),
    Bool(Op, bool),
    /// `""` ou `"="`: vazias.
    Blank,
    /// `"<>"`: não vazias.
    NonBlank,
    Err(Op, duck_core::CellError),
}

impl Crit {
    pub fn parse(v: &V, date1904: bool) -> Self {
        match v {
            V::Num(x) => Crit::Num(Op::Eq, *x),
            V::Bool(b) => Crit::Bool(Op::Eq, *b),
            V::Err(e) => Crit::Err(Op::Eq, *e),
            V::Empty => Crit::Num(Op::Eq, 0.0),
            V::Str(s) => {
                let s: &str = s;
                let (op, rest) = if let Some(r) = s.strip_prefix("<=") {
                    (Op::Le, r)
                } else if let Some(r) = s.strip_prefix(">=") {
                    (Op::Ge, r)
                } else if let Some(r) = s.strip_prefix("<>") {
                    (Op::Ne, r)
                } else if let Some(r) = s.strip_prefix('<') {
                    (Op::Lt, r)
                } else if let Some(r) = s.strip_prefix('>') {
                    (Op::Gt, r)
                } else if let Some(r) = s.strip_prefix('=') {
                    (Op::Eq, r)
                } else {
                    (Op::Eq, s)
                };
                if rest.is_empty() {
                    return match op {
                        Op::Eq => Crit::Blank,
                        Op::Ne => Crit::NonBlank,
                        _ => Crit::Text(op, String::new()),
                    };
                }
                if let Some(x) = parse_number(rest, date1904) {
                    return Crit::Num(op, x);
                }
                if rest.eq_ignore_ascii_case("TRUE") || rest.eq_ignore_ascii_case("VERDADEIRO") {
                    return Crit::Bool(op, true);
                }
                if rest.eq_ignore_ascii_case("FALSE") || rest.eq_ignore_ascii_case("FALSO") {
                    return Crit::Bool(op, false);
                }
                if rest.starts_with('#') {
                    let e = duck_core::CellError::parse(rest.to_ascii_uppercase().as_bytes());
                    if e != duck_core::CellError::Other {
                        return Crit::Err(op, e);
                    }
                }
                if matches!(op, Op::Eq | Op::Ne) && has_wildcards(rest) {
                    return Crit::Wild(op == Op::Ne, Wild::new(rest));
                }
                Crit::Text(op, fold(rest).into_owned())
            }
            _ => Crit::Num(Op::Eq, f64::NAN),
        }
    }

    /// Valor para procurar no índice (critério de igualdade simples).
    pub fn eq_key(&self) -> Option<V<'static>> {
        match self {
            Crit::Num(Op::Eq, x) => Some(V::Num(*x)),
            Crit::Text(Op::Eq, s) => Some(V::Str(s.clone().into())),
            Crit::Bool(Op::Eq, b) => Some(V::Bool(*b)),
            _ => None,
        }
    }

    /// Critério pode casar com célula vazia (precisa varrer todas as posições).
    pub fn matches_blank(&self) -> bool {
        self.matches(&V::Empty)
    }

    pub fn matches(&self, v: &V) -> bool {
        fn cmp_ok(op: Op, o: std::cmp::Ordering) -> bool {
            match op {
                Op::Eq => o.is_eq(),
                Op::Ne => o.is_ne(),
                Op::Lt => o.is_lt(),
                Op::Le => o.is_le(),
                Op::Gt => o.is_gt(),
                Op::Ge => o.is_ge(),
            }
        }
        match self {
            Crit::Blank => match v {
                V::Empty => true,
                V::Str(s) => s.is_empty(),
                _ => false,
            },
            Crit::NonBlank => !matches!(v, V::Empty),
            Crit::Num(op, x) => match v {
                V::Num(y) => {
                    cmp_ok(*op, round15(*y).partial_cmp(&round15(*x)).unwrap_or(std::cmp::Ordering::Less))
                }
                // Texto que é o mesmo número só casa na igualdade.
                V::Str(s) if *op == Op::Eq => parse_number(s, false) == Some(*x),
                _ => *op == Op::Ne,
            },
            Crit::Text(op, t) => match v {
                V::Str(s) => cmp_ok(*op, compare(&V::Str(fold(s)), &V::Str(t.as_str().into()))),
                V::Empty => match op {
                    Op::Ne => true,
                    _ => t.is_empty() && *op == Op::Eq,
                },
                _ => *op == Op::Ne,
            },
            Crit::Wild(neg, w) => {
                let m = match v {
                    V::Str(s) => w.matches(s),
                    _ => false,
                };
                m != *neg
            }
            Crit::Bool(op, b) => match v {
                V::Bool(x) => cmp_ok(*op, x.cmp(b)),
                _ => *op == Op::Ne,
            },
            Crit::Err(op, e) => match v {
                V::Err(x) => (x == e) == (*op == Op::Eq),
                _ => *op == Op::Ne,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        let w = Wild::new("ab*c?");
        assert!(w.matches("ABxxcz"));
        assert!(!w.matches("abc"));
        assert!(Wild::new("~*x").matches("*X"));
        assert!(has_wildcards("a*") && !has_wildcards("a~*"));
    }

    #[test]
    fn criteria() {
        let c = Crit::parse(&V::text(">=10"), false);
        assert!(c.matches(&V::Num(10.0)) && !c.matches(&V::Num(9.0)) && !c.matches(&V::text("x")));
        let c = Crit::parse(&V::text("<>"), false);
        assert!(c.matches(&V::Num(0.0)) && !c.matches(&V::Empty));
        let c = Crit::parse(&V::text("abc"), false);
        assert!(c.matches(&V::text("ABC")) && c.eq_key().is_some());
        let c = Crit::parse(&V::text("<>x"), false);
        assert!(c.matches(&V::Empty) && !c.matches(&V::text("X")));
        assert_eq!(key_hash(&V::text("Ação")), key_hash(&V::text("AÇÃO")));
        assert_eq!(key_hash(&V::Num(0.1 + 0.2)), key_hash(&V::Num(0.3)));
    }
}
