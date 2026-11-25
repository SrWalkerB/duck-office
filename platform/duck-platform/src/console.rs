//! Saída no terminal de um programa com janela.

/// No Windows, um programa com janela não tem terminal: o que ele imprime some.
/// Liga a saída ao terminal de quem o chamou, se houver (`--version`,
/// `--bench`...). Saídas já redirecionadas (para arquivo ou outro programa)
/// ficam como estão. Nos outros sistemas não faz nada.
pub fn attach_console() {
    #[cfg(windows)]
    imp::attach();
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    type Handle = *mut c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> Handle;
        fn SetStdHandle(which: u32, h: Handle) -> i32;
        fn GetFileType(h: Handle) -> u32;
        fn AttachConsole(pid: u32) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            create: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
    }

    const PARENT: u32 = u32::MAX;
    const STDOUT: u32 = -11i32 as u32;
    const STDERR: u32 = -12i32 as u32;
    const READ_WRITE: u32 = 0xC000_0000;
    const SHARE_READ_WRITE: u32 = 3;
    const OPEN_EXISTING: u32 = 3;

    fn valid(h: Handle) -> bool {
        // SAFETY: a função aceita qualquer valor (0 = desconhecido).
        !h.is_null() && h as isize != -1 && unsafe { GetFileType(h) } != 0
    }

    pub fn attach() {
        // SAFETY: chamadas simples ao sistema, antes de qualquer saída; o nome
        // passado termina em zero.
        unsafe {
            let before = [STDOUT, STDERR].map(|w| (w, GetStdHandle(w)));
            if before.iter().all(|(_, h)| valid(*h)) || AttachConsole(PARENT) == 0 {
                return;
            }
            let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
            for (which, h) in before {
                let h = if valid(h) {
                    h
                } else {
                    CreateFileW(
                        name.as_ptr(),
                        READ_WRITE,
                        SHARE_READ_WRITE,
                        std::ptr::null(),
                        OPEN_EXISTING,
                        0,
                        std::ptr::null_mut(),
                    )
                };
                if valid(h) {
                    SetStdHandle(which, h);
                }
            }
        }
    }
}
