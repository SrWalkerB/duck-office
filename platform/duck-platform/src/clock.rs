//! Data e hora no fuso do computador.

/// Data e hora locais.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Agora, no fuso do computador.
#[cfg(windows)]
pub fn local_time() -> LocalTime {
    // SYSTEMTIME: ano, mês, dia da semana, dia, hora, minuto, segundo, ms.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(out: *mut [u16; 8]);
    }
    let mut t = [0u16; 8];
    // SAFETY: a função só escreve os oito campos de 16 bits do SYSTEMTIME.
    unsafe { GetLocalTime(&mut t) };
    LocalTime {
        year: t[0] as i32,
        month: t[1] as u32,
        day: t[3] as u32,
        hour: t[4] as u32,
        minute: t[5] as u32,
        second: t[6] as u32,
    }
}

/// Agora, no fuso do computador.
#[cfg(unix)]
pub fn local_time() -> LocalTime {
    // `struct tm` começa com nove inteiros; o resto (fuso) varia por sistema
    // e só precisa de espaço.
    #[repr(C, align(8))]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        rest: [u64; 8],
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let mut t =
        Tm { sec: 0, min: 0, hour: 0, mday: 1, mon: 0, year: 70, wday: 0, yday: 0, isdst: 0, rest: [0; 8] };
    // SAFETY: `time_t` tem 64 bits nos sistemas suportados e `Tm` é maior que o `struct tm` deles.
    unsafe { localtime_r(&secs, &mut t) };
    LocalTime {
        year: t.year + 1900,
        month: t.mon as u32 + 1,
        day: t.mday as u32,
        hour: t.hour as u32,
        minute: t.min as u32,
        second: t.sec as u32,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn looks_like_a_date() {
        let t = super::local_time();
        assert!((2024..2200).contains(&t.year), "{t:?}");
        assert!((1..=12).contains(&t.month) && (1..=31).contains(&t.day));
        assert!(t.hour < 24 && t.minute < 60 && t.second < 61);
    }
}
