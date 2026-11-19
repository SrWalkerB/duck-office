//! `duck-par`: infraestrutura de paralelismo da suíte (ADR-012).
//!
//! - [`pool`]: pool de threads com tamanho escolhido (benchmarks de 1..N threads).
//! - [`InFlight`]: limita quantos pedaços estão em memória ao mesmo tempo,
//!   **ajudando a executar trabalho** enquanto espera, em vez de bloquear
//!   (bloquear uma thread do rayon esperando outra pode travar com 1 thread).
//! - [`cut_after_last`]: acha onde cortar um buffer de texto num ponto seguro
//!   (ex.: depois do último `</row>`), com busca SIMD (`memchr`).

use std::sync::atomic::{AtomicUsize, Ordering};

pub use rayon;

/// Threads lógicas disponíveis na máquina.
pub fn available_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// Cria um pool com `threads` threads (0 = todas as disponíveis).
pub fn pool(threads: usize) -> rayon::ThreadPool {
    let n = if threads == 0 { available_threads() } else { threads };
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .thread_name(|i| format!("duck-par-{i}"))
        .build()
        .expect("criar pool de threads")
}

/// Contador de trabalhos em andamento com limite.
pub struct InFlight {
    count: AtomicUsize,
    max: usize,
}

impl InFlight {
    pub fn new(max: usize) -> Self {
        Self { count: AtomicUsize::new(0), max: max.max(1) }
    }

    /// Reserva uma vaga. Enquanto o limite estiver cheio, executa trabalho
    /// pendente do pool (se estiver dentro de um) ou cede a CPU.
    pub fn acquire(&self) {
        loop {
            let cur = self.count.load(Ordering::Acquire);
            if cur < self.max
                && self.count.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Relaxed).is_ok()
            {
                return;
            }
            match rayon::yield_now() {
                Some(rayon::Yield::Executed) => {}
                _ => std::thread::yield_now(),
            }
        }
    }

    pub fn release(&self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn current(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

/// Posição logo depois da última ocorrência de `needle` em `buf`.
pub fn cut_after_last(buf: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::rfind(buf, needle).map(|i| i + needle.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_points() {
        let b = b"<row>a</row><row>b</row><row>c";
        assert_eq!(cut_after_last(b, b"</row>"), Some(24));
        assert_eq!(cut_after_last(b"nada", b"</row>"), None);
    }

    #[test]
    fn in_flight_with_one_thread_does_not_deadlock() {
        // Com 1 thread, quem espera precisa executar o trabalho pendente.
        let p = pool(1);
        let limit = InFlight::new(2);
        let done = AtomicUsize::new(0);
        p.install(|| {
            rayon::scope(|s| {
                for _ in 0..50 {
                    limit.acquire();
                    s.spawn(|_| {
                        done.fetch_add(1, Ordering::Relaxed);
                        limit.release();
                    });
                }
            });
        });
        assert_eq!(done.load(Ordering::Relaxed), 50);
        assert_eq!(limit.current(), 0);
    }
}
