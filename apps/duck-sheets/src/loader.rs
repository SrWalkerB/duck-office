//! Leitura do arquivo em segundo plano, em etapas (carga progressiva):
//!
//! 1. pacote + workbook, estilos + tema, strings e a **prévia da aba ativa**
//!    (os primeiros MB de XML) — o que a primeira tela precisa;
//! 2. a aba ativa inteira;
//! 3. as outras abas, em paralelo, cada uma entregue assim que fica pronta.
//!
//! Cada etapa vira uma [`Msg`] e acorda a janela ([`Waker`]): a UI nunca espera.
//!
//! A etapa 2 só começa depois que a janela mostrou a primeira tela com dados
//! ([`Loader::proceed`], ou no máximo [`GATE_TIMEOUT`]): a leitura completa usa
//! todos os núcleos e, se começasse junto, atrasaria a própria janela (medido
//! no WSLg: primeiro frame de 160 para 550 ms).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use duck_core::{Sheet, StringTable, Styles};
use duck_platform::Waker;
use duck_xlsx::{Book, Options, SheetInfo};

/// XML lido para a prévia da aba ativa (algumas centenas a milhares de linhas).
const PREVIEW_BYTES: usize = 4 << 20;

/// Espera máxima pela primeira tela antes da leitura completa.
pub const GATE_TIMEOUT: Duration = Duration::from_millis(400);

/// Sinal "a primeira tela já apareceu".
type Gate = Arc<(Mutex<bool>, Condvar)>;

pub enum Msg {
    Opened {
        sheets: Vec<SheetInfo>,
        active: usize,
        styles: Arc<Styles>,
        strings: Arc<StringTable>,
        date1904: bool,
        at: Duration,
    },
    Preview {
        index: usize,
        sheet: Sheet,
        at: Duration,
    },
    Sheet {
        index: usize,
        sheet: Sheet,
        at: Duration,
    },
    Finished {
        at: Duration,
    },
    Failed(String),
}

pub struct Loader {
    pub rx: Receiver<Msg>,
    pub path: PathBuf,
    cancel: Arc<AtomicBool>,
    gate: Gate,
}

impl Loader {
    /// Começa a ler `path` numa thread. `t0` = início da contagem (os tempos
    /// das mensagens são relativos a ele). `want` = aba a mostrar primeiro
    /// (índice no workbook); `None` = a que estava ativa ao salvar.
    pub fn start(path: PathBuf, waker: Waker, t0: Instant, want: Option<usize>) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
        let c = cancel.clone();
        let g = gate.clone();
        let p = path.clone();
        std::thread::Builder::new()
            .name("duck-loader".into())
            .spawn(move || {
                let send = |m: Msg| {
                    let _ = tx.send(m);
                    waker.wake();
                };
                if let Err(e) = load(&p, &send, &c, &g, t0, want) {
                    send(Msg::Failed(e));
                }
                release_memory();
            })
            .expect("thread de leitura");
        Self { rx, path, cancel, gate }
    }

    /// A primeira tela já está na tela: pode começar a leitura completa.
    pub fn proceed(&self) {
        let (m, cv) = &*self.gate;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }

    /// Desiste da leitura (outro arquivo foi aberto). As abas em andamento
    /// terminam, mas nenhuma nova começa.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.proceed();
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn load(
    path: &PathBuf,
    send: &(dyn Fn(Msg) + Sync),
    cancel: &AtomicBool,
    gate: &Gate,
    t0: Instant,
    want: Option<usize>,
) -> Result<(), String> {
    let book = Book::open(path).map_err(|e| e.to_string())?;
    let sheets = book.sheets();
    let active = want.filter(|&i| i < sheets.len()).unwrap_or(book.active_sheet());

    // Etapa 1: tudo o que a primeira tela precisa, em paralelo.
    let (styles, (strings, preview)) = rayon::join(
        || book.read_styles(),
        || rayon::join(|| book.read_shared_strings(), || book.preview_sheet(active, PREVIEW_BYTES)),
    );
    let styles = styles.map_err(|e| e.to_string())?;
    let strings = strings.map_err(|e| e.to_string())?;
    send(Msg::Opened {
        sheets,
        active,
        styles: Arc::new(styles),
        strings: Arc::new(strings),
        date1904: book.date1904(),
        at: t0.elapsed(),
    });
    if let Ok(sheet) = preview {
        send(Msg::Preview { index: active, sheet, at: t0.elapsed() });
    }
    {
        let (m, cv) = &**gate;
        let go = m.lock().unwrap();
        let _ = cv.wait_timeout_while(go, GATE_TIMEOUT, |go| !*go).unwrap();
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    // Etapa 2 e 3: a aba ativa inteira, depois as outras.
    let opts = Options::default();
    let deliver = |index: usize, r: duck_xlsx::SheetResult| match r {
        Ok((sheet, _)) => send(Msg::Sheet { index, sheet, at: t0.elapsed() }),
        Err(e) => send(Msg::Failed(format!("aba {}: {e}", index + 1))),
    };
    book.load_sheets(&[active], &opts, &deliver);
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let others: Vec<usize> = (0..book.sheets().len()).filter(|&i| i != active).collect();
    book.load_sheets(&others, &opts, &deliver);
    send(Msg::Finished { at: t0.elapsed() });
    Ok(())
}

/// Devolve ao sistema a memória que a leitura usou e liberou (pedaços de XML).
fn release_memory() {
    // SAFETY: chamada simples do mimalloc, segura de qualquer thread.
    unsafe { libmimalloc_sys::mi_collect(true) };
}

/// Mensagens que chegaram, sem esperar.
pub fn drain(loader: &Loader) -> Vec<Msg> {
    loader.rx.try_iter().collect()
}
