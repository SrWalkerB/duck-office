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

/// Núcleos físicos da máquina (sem contar SMT/Hyper-Threading). Cai para as
/// threads lógicas se o sistema não informar.
pub fn physical_cores() -> usize {
    physical_cores_os().filter(|&n| n > 0).unwrap_or_else(available_threads)
}

#[cfg(windows)]
fn physical_cores_os() -> Option<usize> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    let mut len = 0u32;
    // SAFETY: primeira chamada só pergunta o tamanho do buffer; a segunda
    // preenche um buffer desse tamanho, alinhado para a struct.
    unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len);
        if len == 0 {
            return None;
        }
        let words = (len as usize).div_ceil(8);
        let mut buf = vec![0u64; words];
        let ptr = buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
        if GetLogicalProcessorInformationEx(RelationProcessorCore, ptr, &mut len) == 0 {
            return None;
        }
        // Registros de tamanho variável, um por núcleo físico.
        let bytes = buf.as_ptr() as *const u8;
        let (mut off, mut cores) = (0usize, 0usize);
        while off < len as usize {
            let rec = &*(bytes.add(off) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX);
            if rec.Size == 0 {
                break;
            }
            cores += 1;
            off += rec.Size as usize;
        }
        Some(cores)
    }
}

#[cfg(target_os = "linux")]
fn physical_cores_os() -> Option<usize> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let mut cores = std::collections::HashSet::new();
    let (mut phys, mut core) = (None::<String>, None::<String>);
    for line in info.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if let (Some(p), Some(c)) = (phys.take(), core.take()) {
                cores.insert((p, c));
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim() {
                "physical id" => phys = Some(v.trim().to_string()),
                "core id" => core = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    (!cores.is_empty()).then_some(cores.len())
}

#[cfg(not(any(windows, target_os = "linux")))]
fn physical_cores_os() -> Option<usize> {
    None
}

/// Cria um pool com `threads` threads (0 = uma por núcleo físico).
///
/// O padrão é por núcleo físico: na leitura de planilhas, a segunda thread
/// de SMT no mesmo núcleo não ganhou tempo e aumentou o pico de memória
/// (medido: 16 × 32 threads num Ryzen 9 5950X, ver docs/ARCHITECTURE.md).
pub fn pool(threads: usize) -> rayon::ThreadPool {
    let n = if threads == 0 { physical_cores() } else { threads };
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
    /// Thread dormindo em [`InFlight::acquire_without_helping`], acordada por `release`.
    waiter: std::sync::Mutex<Option<std::thread::Thread>>,
}

impl InFlight {
    pub fn new(max: usize) -> Self {
        Self { count: AtomicUsize::new(0), max: max.max(1), waiter: std::sync::Mutex::new(None) }
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

    /// Como [`Self::acquire`], mas sem executar trabalho do pool enquanto
    /// espera. Para a thread que alimenta um pipeline (ex.: quem entrega a
    /// saída do inflate): pegar uma tarefa longa de outro trava a entrega.
    /// Só use com 3+ threads no pool; com menos, ninguém mais liberaria a vaga.
    ///
    /// Dorme (não gira) até `release` liberar uma vaga: girar ocupa um núcleo
    /// sem trabalho útil e atrasa o irmão de SMT.
    pub fn acquire_without_helping(&self) {
        loop {
            let cur = self.count.load(Ordering::Acquire);
            if cur < self.max {
                if self.count.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Relaxed).is_ok()
                {
                    return;
                }
                continue;
            }
            *self.waiter.lock().unwrap() = Some(std::thread::current());
            // Confere de novo depois de se registrar (um `release` pode ter vindo antes).
            if self.count.load(Ordering::Acquire) < self.max {
                continue;
            }
            std::thread::park_timeout(std::time::Duration::from_millis(2));
        }
    }

    pub fn release(&self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
        if let Some(t) = self.waiter.lock().unwrap().take() {
            t.unpark();
        }
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
    fn physical_cores_is_sane() {
        let p = physical_cores();
        assert!(p >= 1 && p <= available_threads(), "{p} núcleos físicos, {} lógicos", available_threads());
    }

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
