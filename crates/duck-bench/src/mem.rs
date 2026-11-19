//! Memória do processo (atual e pico), em bytes.

#[derive(Clone, Copy, Debug, Default)]
pub struct Mem {
    pub current: u64,
    pub peak: u64,
}

#[cfg(windows)]
pub fn process() -> Mem {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: chamada simples da API do Windows com um struct zerado do tamanho declarado.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            return Mem { current: c.WorkingSetSize as u64, peak: c.PeakWorkingSetSize as u64 };
        }
    }
    Mem::default()
}

#[cfg(target_os = "linux")]
pub fn process() -> Mem {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| {
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    Mem { current: kb("VmRSS:"), peak: kb("VmHWM:") }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn process() -> Mem {
    Mem::default()
}

pub fn mb(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}
