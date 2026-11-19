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

/// Tempo de CPU do processo (usuário + sistema), em segundos.
#[cfg(windows)]
pub fn cpu_seconds() -> f64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    // SAFETY: chamada simples da API do Windows com structs zerados.
    unsafe {
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) == 0 {
            return 0.0;
        }
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
        t(k) + t(u)
    }
}

#[cfg(target_os = "linux")]
pub fn cpu_seconds() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let f: Vec<&str> = s.rsplit(')').next().unwrap_or("").split_whitespace().collect();
    // utime e stime são os campos 14 e 15 (base 1); depois do ')' começam no 3.
    let tick = 100.0;
    let get = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (get(11) + get(12)) / tick
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn cpu_seconds() -> f64 {
    0.0
}
