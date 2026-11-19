//! `duck-deflate`: descompactação deflate (RFC 1951) **em paralelo**.
//!
//! Um stream deflate é sequencial por natureza: cada bloco pode copiar bytes
//! dos 32 KB anteriores. Para usar vários núcleos numa parte só de um zip (a
//! aba de 1,6 GB do arquivo de referência), seguimos a técnica do rapidgzip:
//!
//! 1. Os dados comprimidos são divididos em partes (ex.: 1 MB).
//! 2. Para cada parte, uma thread procura o início do próximo bloco dinâmico,
//!    validando o cabeçalho a fundo (códigos de Huffman completos etc.).
//! 3. A parte é decodificada **sem conhecer** os 32 KB anteriores: cópias que
//!    apontam para antes do início viram marcadores de 16 bits. Assim que os
//!    últimos 32 KB da saída não têm mais marcadores, a decodificação passa
//!    para bytes normais.
//! 4. Em ordem, cada parte confere se começa exatamente onde a anterior
//!    terminou e troca os marcadores pelos bytes reais da janela anterior.
//!    Se não encaixar (falso positivo na busca), a parte é decodificada de
//!    novo em sequência, já com a janela conhecida.
//! 5. O CRC32 de toda a saída é conferido com o do zip.
//!
//! O resultado é idêntico ao de um descompactador sequencial; os testes
//! comparam com o `zlib-rs`.

mod bits;
mod block;
mod huffman;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use bits::Bits;
use block::{Corrupt, MARKER, WINDOW, decode_block, read_dynamic_tables, read_header};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Dados deflate inválidos ou truncados.
    Corrupt,
    /// A saída não bate com o CRC32 esperado.
    Crc { expected: u32, got: u32 },
    /// Erro devolvido por quem recebe a saída.
    Sink(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Corrupt => write!(f, "deflate inválido"),
            Error::Crc { expected, got } => {
                write!(f, "CRC32 não bate: esperado {expected:08x}, obtido {got:08x}")
            }
            Error::Sink(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Corrupt> for Error {
    fn from(_: Corrupt) -> Self {
        Error::Corrupt
    }
}

/// Descompacta tudo numa thread (referência e partes pequenas).
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(data.len() * 4);
    let (_, last) = decode_known(data, 0, u64::MAX, &mut out)?;
    if !last {
        return Err(Error::Corrupt);
    }
    Ok(out)
}

/// Decodifica a partir de `start` (bit), com `out` já contendo a janela
/// anterior, até a primeira fronteira de bloco em ou depois de `stop` (ou o
/// último bloco). Retorna (posição final em bits, achou o último bloco).
fn decode_known(data: &[u8], start: u64, stop: u64, out: &mut Vec<u8>) -> Result<(u64, bool), Error> {
    let mut b = Bits::new(data, start);
    loop {
        let pos = b.bit_pos();
        if pos >= stop && pos != start {
            return Ok((pos, false));
        }
        if b.overrun() {
            return Err(Error::Corrupt);
        }
        let h = read_header(&mut b)?;
        decode_block(&mut b, &h, out)?;
        if h.last {
            return Ok((b.bit_pos(), true));
        }
    }
}

// ---------------------------------------------------------------------------
// Buffers reciclados
// ---------------------------------------------------------------------------

/// Vetores grandes reaproveitados entre partes: a memória em trânsito vira um
/// conjunto fixo, sem alocar (e tocar páginas novas) a cada parte.
struct Pool {
    free: Mutex<Vec<Vec<u8>>>,
    keep: usize,
}

impl Pool {
    fn take(&self, min_capacity: usize) -> Vec<u8> {
        let mut v = self.free.lock().unwrap().pop().unwrap_or_default();
        v.clear();
        v.reserve(min_capacity);
        v
    }

    fn give(&self, mut v: Vec<u8>) {
        let mut free = self.free.lock().unwrap();
        if free.len() < self.keep {
            v.clear();
            free.push(v);
        }
    }
}

/// Saída descompactada entregue por posse. Ao ser descartada, a memória volta
/// para o pool do `inflate_parallel` (se ele ainda existir).
pub struct Buffer {
    vec: Vec<u8>,
    start: usize,
    pool: std::sync::Weak<Pool>,
    input_end: usize,
}

impl Buffer {
    /// Os bytes novos deste pedaço.
    pub fn bytes(&self) -> &[u8] {
        &self.vec[self.start..]
    }

    /// Até que byte da entrada comprimida a descompactação já passou quando
    /// este pedaço ficou pronto: antes disso, a entrada não é mais lida.
    pub fn input_consumed(&self) -> usize {
        self.input_end
    }
}

impl std::ops::Deref for Buffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.bytes()
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.upgrade() {
            pool.give(std::mem::take(&mut self.vec));
        }
    }
}

// ---------------------------------------------------------------------------
// Especulação
// ---------------------------------------------------------------------------

/// Resultado da decodificação especulativa de uma parte.
struct Spec {
    /// Bit onde a parte começou (cabeçalho de bloco encontrado).
    start: u64,
    /// Primeira fronteira de bloco em ou depois do fim da parte.
    end: u64,
    last: bool,
    /// Saída de 16 bits até a janela ficar limpa (pode ter marcadores).
    head: Vec<u16>,
    /// Saída em bytes depois disso, precedida dos 32 KB de contexto.
    tail: Vec<u8>,
    switched: bool,
    tail_crc: crc32fast::Hasher,
}

/// Acha o primeiro cabeçalho de bloco dinâmico válido em `[from, to)`.
fn find_block(data: &[u8], from: u64, to: u64) -> Option<u64> {
    let to = to.min(data.len() as u64 * 8);
    let mut bit = from;
    while bit < to {
        let byte = (bit / 8) as usize;
        // Filtro rápido com 17 bits: BFINAL=0, BTYPE=10, HLIT ≤ 29, HDIST ≤ 29.
        if byte + 4 <= data.len() {
            let w = u32::from_le_bytes(data[byte..byte + 4].try_into().unwrap()) >> (bit % 8);
            let ok = w & 0b111 == 0b100 && (w >> 3) & 31 <= 29 && (w >> 8) & 31 <= 29;
            if ok {
                let mut b = Bits::new(data, bit + 3);
                if read_dynamic_tables(&mut b).is_ok() {
                    return Some(bit);
                }
            }
        }
        bit += 1;
    }
    None
}

fn speculate(data: &[u8], from: u64, to: u64, pool: &Pool, expect: usize) -> Option<Spec> {
    let start = find_block(data, from, to)?;
    let mut b = Bits::new(data, start);
    let mut head: Vec<u16> = Vec::with_capacity(4 * WINDOW);
    let mut tail: Option<Vec<u8>> = None;
    let mut switched = false;
    // Proteção contra falso positivo que decodifica lixo sem parar.
    let limit = ((to - from) as usize / 8).max(1 << 20) * 64;
    let (end, last) = loop {
        let pos = b.bit_pos();
        if pos >= to && pos != start {
            break (pos, false);
        }
        let h = read_header(&mut b).ok()?;
        if !switched {
            decode_block(&mut b, &h, &mut head).ok()?;
        } else {
            decode_block(&mut b, &h, tail.as_mut().unwrap()).ok()?;
        }
        if h.last {
            break (b.bit_pos(), true);
        }
        if !switched && head.len() >= WINDOW && head[head.len() - WINDOW..].iter().all(|&v| v < MARKER) {
            let mut t = pool.take(expect + WINDOW);
            t.extend(head[head.len() - WINDOW..].iter().map(|&v| v as u8));
            tail = Some(t);
            switched = true;
        }
        if head.len() + tail.as_ref().map_or(0, Vec::len) > limit {
            if let Some(t) = tail {
                pool.give(t);
            }
            return None;
        }
    };
    let mut tail_crc = crc32fast::Hasher::new();
    if let Some(t) = &tail {
        tail_crc.update(&t[WINDOW..]);
    }
    let tail = tail.unwrap_or_default();
    Some(Spec { start, end, last, head, tail, switched, tail_crc })
}

// ---------------------------------------------------------------------------
// Orquestração
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Tamanho de cada parte, em bytes comprimidos.
    pub part_bytes: usize,
    /// Quantas partes especular à frente (limita a memória).
    pub lookahead: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { part_bytes: 1 << 20, lookahead: 16 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub parts: usize,
    /// Partes aproveitadas da especulação.
    pub speculative: usize,
    /// Partes refeitas em sequência (início ou especulação que não encaixou).
    pub sequential: usize,
    /// Motivos: nenhum bloco achado / começou em outro ponto / marcador sem janela.
    pub no_block: usize,
    pub misaligned: usize,
    pub unresolved: usize,
    /// Partes puladas porque um bloco anterior as atravessou inteiras.
    pub skipped: usize,
    pub output_bytes: u64,
    /// Bytes que saíram da fase de marcadores (resolvidos em série).
    pub head_bytes: u64,
}

/// Descompacta `data` em paralelo, entregando a saída **em ordem** a `sink`,
/// na thread que chamou. Deve rodar dentro de um pool rayon.
///
/// Cada pedaço chega **por posse** ([`Buffer`]), para quem recebe poder
/// repassar sem copiar (a saída de uma aba grande tem gigabytes). Quando o
/// `Buffer` é descartado, a memória volta para ser reaproveitada.
pub fn inflate_parallel(
    data: &[u8],
    expected_crc: Option<u32>,
    opts: Options,
    mut sink: impl FnMut(Buffer) -> Result<(), String> + Send,
) -> Result<Report, Error> {
    let part_bits = (opts.part_bytes.max(64 * 1024) as u64) * 8;
    let total_bits = data.len() as u64 * 8;
    let n = total_bits.div_ceil(part_bits).max(1) as usize;
    let bound = |k: usize| (k as u64 * part_bits).min(total_bits);
    // Uma vaga por parte: o resultado entra quando a especulação termina e sai
    // (liberando a memória) assim que é usado.
    let slots: Vec<(AtomicBool, Mutex<Option<Spec>>)> =
        (0..n).map(|_| (AtomicBool::new(false), Mutex::new(None))).collect();
    // Com poucas threads, quem espera precisa ajudar o pool (senão trava); com
    // várias, ajudar faz a entrega parar enquanto roda uma tarefa longa de outro.
    let help_while_waiting = rayon::current_num_threads() <= 2;
    let mut report = Report { parts: n, ..Default::default() };
    let mut crc = crc32fast::Hasher::new();

    // Mantém vetores suficientes para as partes adiantadas e as que ainda
    // estão sendo lidas por quem recebe a saída.
    let pool = std::sync::Arc::new(Pool { free: Mutex::new(Vec::new()), keep: 4 });
    // XML de planilha comprime ~15:1; folga para não realocar.
    let expect = opts.part_bytes.max(64 * 1024) * 20;
    let result = rayon::scope(|s| -> Result<(), Error> {
        let pool_ref: &Pool = &pool;
        let weak = std::sync::Arc::downgrade(&pool);
        let slots = &slots;
        let mut next_spawn = 1usize;
        let spawn_until = |upto: usize, next_spawn: &mut usize| {
            while *next_spawn < n && *next_spawn <= upto {
                let k = *next_spawn;
                s.spawn(move |_| {
                    let r = speculate(data, bound(k), bound(k + 1), pool_ref, expect);
                    *slots[k].1.lock().unwrap() = r;
                    slots[k].0.store(true, Ordering::Release);
                });
                *next_spawn += 1;
            }
        };
        spawn_until(opts.lookahead.max(1), &mut next_spawn);

        let mut window: Vec<u8> = Vec::with_capacity(2 * WINDOW);
        let mut cur = 0u64;
        let mut k = 0usize;
        loop {
            spawn_until(k + opts.lookahead.max(1), &mut next_spawn);
            let mut done = None;
            if k > 0 {
                // Espera a especulação da parte k, ajudando o pool enquanto isso.
                while !slots[k].0.load(Ordering::Acquire) {
                    if help_while_waiting {
                        match rayon::yield_now() {
                            Some(rayon::Yield::Executed) => {}
                            _ => std::thread::yield_now(),
                        }
                    } else {
                        std::thread::yield_now();
                    }
                }
                let taken = slots[k].1.lock().unwrap().take();
                // A parte anterior pode ter parado num bloco vazio de flush; a
                // especulação começa no bloco de verdade logo depois.
                let real = skip_empty_stored(data, cur);
                match &taken {
                    None => report.no_block += 1,
                    Some(spec) if spec.start != real => report.misaligned += 1,
                    _ => {}
                }
                if let Some(spec) = taken
                    && spec.start == real
                {
                    // Troca os marcadores pelos bytes da janela anterior.
                    let missing = WINDOW - window.len();
                    let mut resolved: Vec<u8> = Vec::with_capacity(spec.head.len());
                    let mut ok = true;
                    for &v in &spec.head {
                        if v < MARKER {
                            resolved.push(v as u8);
                        } else {
                            let w = (v - MARKER) as usize;
                            if w < missing {
                                ok = false;
                                report.unresolved += 1;
                                break;
                            }
                            resolved.push(window[w - missing]);
                        }
                    }
                    if ok {
                        crc.update(&resolved);
                        push_window(&mut window, &resolved);
                        report.output_bytes += resolved.len() as u64;
                        report.head_bytes += resolved.len() as u64;
                        let input_end = (spec.end / 8) as usize;
                        sink(Buffer { vec: resolved, start: 0, pool: weak.clone(), input_end })
                            .map_err(Error::Sink)?;
                        if spec.switched {
                            let tail = spec.tail;
                            crc.combine(&spec.tail_crc);
                            push_window(&mut window, &tail[WINDOW..]);
                            report.output_bytes += (tail.len() - WINDOW) as u64;
                            sink(Buffer { vec: tail, start: WINDOW, pool: weak.clone(), input_end })
                                .map_err(Error::Sink)?;
                        }
                        report.speculative += 1;
                        done = Some((spec.end, spec.last));
                    }
                }
            }
            let (end, last) = match done {
                Some(d) => d,
                None => {
                    // Em sequência, com a janela conhecida.
                    let mut out = pool_ref.take(WINDOW + expect);
                    out.extend_from_slice(&window);
                    let (end, last) = decode_known(data, cur, bound(k + 1), &mut out)?;
                    let start = window.len();
                    crc.update(&out[start..]);
                    report.output_bytes += (out.len() - start) as u64;
                    let tail_start = out.len().saturating_sub(WINDOW);
                    window.clear();
                    window.extend_from_slice(&out[tail_start..]);
                    let input_end = (end / 8) as usize;
                    sink(Buffer { vec: out, start, pool: weak.clone(), input_end }).map_err(Error::Sink)?;
                    report.sequential += 1;
                    (end, last)
                }
            };
            cur = end;
            if last {
                return Ok(());
            }
            if cur >= total_bits {
                return Err(Error::Corrupt);
            }
            let next = (cur / part_bits) as usize;
            report.skipped += next.saturating_sub(k + 1);
            k = next;
            if k >= n {
                return Err(Error::Corrupt);
            }
        }
    });
    result?;
    let got = crc.finalize();
    if let Some(expected) = expected_crc
        && expected != got
    {
        return Err(Error::Crc { expected, got });
    }
    Ok(report)
}

/// Pula blocos sem compressão vazios (`00 00 FF FF`, inseridos por flush do
/// compressor) a partir de `bit`. Retorna a posição do próximo bloco de
/// verdade (ou `bit`, se não houver bloco vazio ali).
fn skip_empty_stored(data: &[u8], mut bit: u64) -> u64 {
    loop {
        let mut b = Bits::new(data, bit);
        if b.read(3) != 0 {
            return bit; // não é bloco sem compressão (ou é o último)
        }
        b.align();
        let len = b.read(16);
        let nlen = b.read(16);
        if len != 0 || nlen != 0xffff || b.overrun() {
            return bit;
        }
        bit = b.bit_pos();
    }
}

/// Mantém só os últimos 32 KB.
fn push_window(window: &mut Vec<u8>, s: &[u8]) {
    if s.len() >= WINDOW {
        window.clear();
        window.extend_from_slice(&s[s.len() - WINDOW..]);
        return;
    }
    window.extend_from_slice(s);
    if window.len() > WINDOW {
        let cut = window.len() - WINDOW;
        window.drain(..cut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn deflate(data: &[u8], level: u32) -> Vec<u8> {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(level));
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    /// XML parecido com o de uma planilha, com números variando.
    fn xml(rows: usize) -> Vec<u8> {
        let mut s = String::from("<sheetData>");
        let mut x: u64 = 7;
        for r in 1..=rows {
            s.push_str(&format!("<row r=\"{r}\">"));
            for c in 0..12 {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let v = x >> 40;
                match c % 4 {
                    0 => s.push_str(&format!("<c r=\"A{r}\" t=\"s\"><v>{}</v></c>", v % 500)),
                    1 => s.push_str(&format!("<c r=\"B{r}\" s=\"3\"><v>{}.{}</v></c>", v % 100000, v % 97)),
                    2 => s.push_str(&format!(
                        "<c r=\"C{r}\"><f>Tabela1[[#This Row],[Valor]]*2</f><v>{v}</v></c>"
                    )),
                    _ => s.push_str("<c r=\"D1\" s=\"1\"/>"),
                }
            }
            s.push_str("</row>");
        }
        s.push_str("</sheetData>");
        s.into_bytes()
    }

    fn random(n: usize) -> Vec<u8> {
        let mut x: u64 = 99;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (x >> 56) as u8
            })
            .collect()
    }

    fn parallel(data: &[u8], crc: u32, part: usize) -> (Vec<u8>, Report) {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        let mut out = Vec::new();
        let rep = pool
            .install(|| {
                inflate_parallel(data, Some(crc), Options { part_bytes: part, lookahead: 6 }, |b| {
                    out.extend_from_slice(&b);
                    Ok(())
                })
            })
            .unwrap();
        (out, rep)
    }

    #[test]
    fn sequential_matches_zlib_all_levels() {
        let src = xml(3000);
        for level in [0, 1, 6, 9] {
            let z = deflate(&src, level);
            assert_eq!(inflate(&z).unwrap(), src, "nível {level}");
        }
        let rnd = random(300_000);
        assert_eq!(inflate(&deflate(&rnd, 6)).unwrap(), rnd);
        assert_eq!(inflate(&deflate(b"", 6)).unwrap(), b"");
        assert_eq!(inflate(&deflate(b"a", 6)).unwrap(), b"a");
    }

    #[test]
    fn parallel_matches_original() {
        let src = xml(60_000);
        let crc = crc32fast::hash(&src);
        for level in [1, 6, 9] {
            let z = deflate(&src, level);
            let (out, rep) = parallel(&z, crc, 64 * 1024);
            assert_eq!(out.len(), src.len(), "nível {level}");
            assert!(out == src, "nível {level}");
            // O nível 1 do zlib-rs usa blocos fixos enormes: sem cabeçalho dinâmico
            // para achar, tudo sai em sequência (correto, só sem ganho).
            if level > 1 {
                assert!(rep.speculative > 0, "nível {level}: nenhuma parte especulada ({rep:?})");
            }
        }
    }

    #[test]
    fn parallel_handles_incompressible_and_tiny() {
        let rnd = random(2_000_000);
        let (out, _) = parallel(&deflate(&rnd, 6), crc32fast::hash(&rnd), 64 * 1024);
        assert!(out == rnd);
        let (out, _) = parallel(&deflate(&rnd, 0), crc32fast::hash(&rnd), 64 * 1024);
        assert!(out == rnd);
        let (out, rep) = parallel(&deflate(b"oi", 6), crc32fast::hash(b"oi"), 64 * 1024);
        assert_eq!(out, b"oi");
        assert_eq!(rep.parts, 1);
    }

    #[test]
    fn parallel_with_sync_flushes() {
        // Compressores como o do Office fazem flush periódico, que insere
        // blocos sem compressão vazios entre os blocos dinâmicos.
        let src = xml(60_000);
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
        for piece in src.chunks(50_000) {
            e.write_all(piece).unwrap();
            e.flush().unwrap();
        }
        let z = e.finish().unwrap();
        let (out, rep) = parallel(&z, crc32fast::hash(&src), 64 * 1024);
        assert!(out == src);
        assert_eq!(rep.misaligned, 0, "{rep:?}");
        assert!(rep.speculative > 0, "{rep:?}");
    }

    #[test]
    fn wrong_crc_is_reported() {
        let src = xml(2000);
        let z = deflate(&src, 6);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let r = pool.install(|| inflate_parallel(&z, Some(0xdead_beef), Options::default(), |_| Ok(())));
        assert!(matches!(r, Err(Error::Crc { .. })));
    }

    #[test]
    fn corrupt_input_is_an_error() {
        let src = xml(2000);
        let mut z = deflate(&src, 6);
        let mid = z.len() / 2;
        z.truncate(mid);
        assert!(inflate(&z).is_err());
    }
}
