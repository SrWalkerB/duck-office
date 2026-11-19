//! Busca (Ctrl+F) em paralelo, fora da thread da janela.
//!
//! Sem diferença de maiúsculas nem de acentos ("acao" acha "Ação"). As
//! strings compartilhadas são testadas uma vez cada (dezenas de milhares) e o
//! resultado vale para os milhões de células que apontam para elas; números
//! são comparados pelo texto formatado, como aparecem na tela.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use duck_core::{BLOCK_ROWS, Sheet, StringTable, Value};
use duck_platform::Waker;
use rayon::prelude::*;

use crate::look::{Looks, show};

/// Máximo de resultados guardados (o resto só é contado).
pub const MAX_RESULTS: usize = 2_000_000;

pub struct SearchJob {
    pub query: String,
    pub rx: Receiver<SearchResult>,
    cancel: Arc<AtomicBool>,
}

pub struct SearchResult {
    /// (linha, coluna), em ordem de linha e depois coluna.
    pub hits: Vec<(u32, u32)>,
    /// Total de células encontradas (pode passar de `hits.len()`).
    pub total: usize,
    pub millis: f64,
}

impl Drop for SearchJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Minúsculas e sem acentos (latim): a forma usada para comparar.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            'ñ' | 'Ñ' => 'n',
            c => c,
        };
        out.extend(c.to_lowercase());
    }
    out
}

/// `fold(hay)` contém `needle` (já dobrado)? Sem alocar no caso ASCII.
fn contains_folded(hay: &str, needle: &str) -> bool {
    if hay.is_ascii() {
        let (h, n) = (hay.as_bytes(), needle.as_bytes());
        if n.len() > h.len() {
            return false;
        }
        return h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    }
    fold(hay).contains(needle)
}

/// Começa uma busca. O resultado chega por `rx` e acorda a janela.
pub fn start(
    query: &str,
    sheet: Arc<Sheet>,
    strings: Arc<StringTable>,
    looks: Arc<Looks>,
    date1904: bool,
    waker: Waker,
) -> SearchJob {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let needle = fold(query);
    std::thread::Builder::new()
        .name("duck-search".into())
        .spawn(move || {
            let t = std::time::Instant::now();
            let (hits, total) = run(&needle, &sheet, &strings, &looks, date1904, &c);
            if !c.load(Ordering::Relaxed) {
                let _ = tx.send(SearchResult { hits, total, millis: t.elapsed().as_secs_f64() * 1000.0 });
                waker.wake();
            }
        })
        .expect("thread de busca");
    SearchJob { query: query.to_string(), rx, cancel }
}

/// A busca em si (pública para testes e benchmark).
pub fn run(
    needle: &str,
    sheet: &Sheet,
    strings: &StringTable,
    looks: &Looks,
    date1904: bool,
    cancel: &AtomicBool,
) -> (Vec<(u32, u32)>, usize) {
    if needle.is_empty() {
        return (Vec::new(), 0);
    }
    // Strings compartilhadas que batem, por endereço (o texto de uma célula é
    // uma fatia da tabela: o início identifica a string).
    let shared_hits: std::collections::HashSet<usize> = (0..strings.len() as u32)
        .into_par_iter()
        .filter_map(|i| {
            let s = strings.get(i)?;
            (!s.is_empty() && contains_folded(s, needle)).then_some(s.as_ptr() as usize)
        })
        .collect();
    let numbers = needle.chars().any(|c| c.is_ascii_digit());
    let rows = sheet.rows();
    // Tarefas: coluna × faixa de 64 blocos (65 mil linhas).
    const SPAN: u32 = 64 * BLOCK_ROWS as u32;
    let tasks: Vec<(u32, u32)> =
        (0..sheet.cols()).flat_map(|c| (0..rows.div_ceil(SPAN)).map(move |k| (c, k * SPAN))).collect();
    let mut found: Vec<Vec<(u32, u32)>> = tasks
        .par_iter()
        .map(|&(col, start)| {
            let mut out = Vec::new();
            if cancel.load(Ordering::Relaxed) {
                return out;
            }
            let mut buf = String::new();
            sheet.for_each_value(col, start..(start + SPAN).min(rows), strings, |row, cell| {
                let hit = match cell.value {
                    Value::Text(s) => {
                        if is_shared(strings, s) {
                            shared_hits.contains(&(s.as_ptr() as usize))
                        } else {
                            contains_folded(s, needle)
                        }
                    }
                    Value::Number(_) | Value::Bool(_) | Value::Error(_) => {
                        if !numbers && matches!(cell.value, Value::Number(_)) {
                            false
                        } else {
                            show(looks, looks.get(cell.style), cell.value, date1904, &mut buf);
                            contains_folded(&buf, needle)
                        }
                    }
                    Value::Empty => false,
                };
                if hit {
                    out.push((row, col));
                }
                true
            });
            out
        })
        .collect();
    let total: usize = found.iter().map(Vec::len).sum();
    let mut hits: Vec<(u32, u32)> = Vec::with_capacity(total.min(MAX_RESULTS));
    for v in found.iter_mut() {
        hits.append(v);
    }
    hits.par_sort_unstable();
    hits.truncate(MAX_RESULTS);
    (hits, total)
}

/// O texto aponta para dentro da tabela compartilhada?
fn is_shared(strings: &StringTable, s: &str) -> bool {
    let (a, b) = strings.byte_range();
    let p = s.as_ptr() as usize;
    p >= a && p < b.max(a + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_shared_local_and_numbers() {
        use duck_core::{SheetPart, Styles, kind};
        let mut strings = StringTable::new();
        let acao = strings.push("Ação de Reposição");
        let outra = strings.push("Outra coisa");
        let mut p = SheetPart::new();
        // A mesma string compartilhada em várias células.
        for r in 0..3000 {
            p.set(r, 0, kind::SHARED_STR, 0, if r % 3 == 0 { acao } else { outra } as u64);
        }
        let local = p.strings.push("reposicao local");
        p.set(5, 2, kind::LOCAL_STR, 0, local as u64);
        p.set(7, 1, kind::NUMBER, 0, 12345.5f64.to_bits());
        let mut sheet = Sheet::new("t");
        sheet.merge_part(p);
        sheet.finish();
        let looks = Looks::new(&Styles::default());
        let stop = AtomicBool::new(false);
        let (hits, total) = run(&fold("REPOSIÇÃO"), &sheet, &strings, &looks, false, &stop);
        assert_eq!(total, 1000 + 1);
        assert_eq!(hits[0], (0, 0));
        assert!(hits.contains(&(5, 2)));
        assert!(hits.windows(2).all(|w| w[0] < w[1]), "em ordem de linha e coluna");
        // Números pelo texto formatado (vírgula decimal).
        let (hits, _) = run(&fold("12345,5"), &sheet, &strings, &looks, false, &stop);
        assert_eq!(hits, vec![(7, 1)]);
        let (hits, _) = run("", &sheet, &strings, &looks, false, &stop);
        assert!(hits.is_empty());
    }

    #[test]
    fn folding() {
        assert_eq!(fold("Ação Ótima"), "acao otima");
        assert!(contains_folded("Relatório de AÇÕES", "acoes"));
        assert!(contains_folded("Loja 12", "loja"));
        assert!(!contains_folded("abc", "abcd"));
    }
}
