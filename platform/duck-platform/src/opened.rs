//! Arquivos que o sistema pede para abrir com o programa já rodando ou durante
//! a abertura.
//!
//! No macOS o duplo clique no Finder não passa o arquivo na linha de comando:
//! o sistema chama `application:openFiles:` no delegado do aplicativo. O
//! delegado é do `winit`, que não trata esse pedido; aqui o método é posto na
//! classe dele.

use std::path::PathBuf;
use std::sync::Mutex;

use winit::event_loop::EventLoopProxy;

/// Pedidos à espera, e quem acorda o loop quando chega um.
static QUEUE: Mutex<(Vec<PathBuf>, Option<EventLoopProxy<()>>)> = Mutex::new((Vec::new(), None));

/// Os pedidos que chegaram, na ordem.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut QUEUE.lock().unwrap_or_else(|e| e.into_inner()).0)
}

/// Passa a receber os pedidos do sistema. Depois de criar o loop de eventos e
/// antes de rodá-lo.
pub fn listen(proxy: EventLoopProxy<()>) {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).1 = Some(proxy);
    #[cfg(target_os = "macos")]
    mac::install();
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn arrived(path: PathBuf) {
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    q.0.push(path);
    if let Some(p) = &q.1 {
        let _ = p.send_event(());
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CStr, OsStr, c_char, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    type Id = *mut c_void;
    type Sel = *const c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn object_getClass(obj: Id) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn class_addMethod(class: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> bool;
        fn objc_msgSend();
    }

    /// `[obj sel]` que devolve um objeto.
    ///
    /// # Safety
    /// `obj` responde a `sel`, sem argumentos, com um ponteiro.
    unsafe fn get(obj: Id, sel: &CStr) -> Id {
        // SAFETY: o tipo é o que a mensagem tem (garantido por quem chama).
        unsafe {
            let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
            f(obj, sel_registerName(sel.as_ptr()))
        }
    }

    /// `[obj sel: arg]`.
    ///
    /// # Safety
    /// `obj` responde a `sel`, com um argumento do tamanho de um ponteiro.
    unsafe fn send(obj: Id, sel: &CStr, arg: usize) -> Id {
        // SAFETY: idem.
        unsafe {
            let f: unsafe extern "C" fn(Id, Sel, usize) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(obj, sel_registerName(sel.as_ptr()), arg)
        }
    }

    /// `application:openFiles:` (`NSArray<NSString *>`).
    unsafe extern "C" fn open_files(_this: Id, _sel: Sel, app: Id, files: Id) {
        // SAFETY: o AppKit chama na thread principal, com uma lista de textos;
        // `UTF8String` vale até o fim do evento.
        unsafe {
            let n = get(files, c"count") as usize;
            for i in 0..n {
                let s = get(send(files, c"objectAtIndex:", i), c"UTF8String") as *const c_char;
                if !s.is_null() {
                    let bytes = CStr::from_ptr(s).to_bytes();
                    super::arrived(PathBuf::from(OsStr::from_bytes(bytes)));
                }
            }
            // NSApplicationDelegateReplySuccess.
            send(app, c"replyToOpenOrPrint:", 0);
        }
    }

    pub fn install() {
        // SAFETY: na thread principal, com o `NSApplication` e o delegado já
        // criados pelo `winit`. O AppKit guarda o que o delegado sabe fazer
        // quando ele é definido: por isso é definido de novo.
        unsafe {
            let class = objc_getClass(c"NSApplication".as_ptr());
            if class.is_null() {
                return;
            }
            let app = get(class, c"sharedApplication");
            let delegate = get(app, c"delegate");
            if delegate.is_null() {
                return;
            }
            let added = class_addMethod(
                object_getClass(delegate),
                sel_registerName(c"application:openFiles:".as_ptr()),
                open_files as *const c_void,
                c"v@:@@".as_ptr(),
            );
            if added {
                send(app, c"setDelegate:", 0);
                send(app, c"setDelegate:", delegate as usize);
            }
        }
    }
}
