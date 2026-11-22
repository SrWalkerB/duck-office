//! `duck-sheets`: editor de planilhas `.xlsx` / `.xlsm` / `.csv` (F2, F3).
//!
//! ```text
//! duck-sheets [ARQUIVO] [--sheet N] [--cell B12] [--zoom 80] [--find TEXTO]
//!             [--bench] [--screenshot SAIDA.png]
//!             [--edit A1=valor ...] [--undo N] [--save-to SAIDA.xlsx]
//! ```
//!
//! Sem arquivo, abre o diálogo nativo. Teclas: setas (+Shift seleciona,
//! +Ctrl vai até a borda dos dados), PageUp/PageDown, Ctrl+Home/End,
//! Ctrl+PageUp/PageDown (abas), Ctrl+C (copia a seleção), Ctrl+F (busca),
//! Ctrl+G (ir para), Ctrl+O (abrir), Ctrl+A, Ctrl + roda / Ctrl +/−/0 (zoom).
//!
//! Edição: digitar substitui a célula; F2, duplo clique ou clique na barra
//! de fórmulas editam o conteúdo; Enter/Tab confirmam, Esc cancela; Delete
//! apaga a seleção; Ctrl+Z/Ctrl+Y desfazem e refazem; Ctrl+S salva e
//! Ctrl+Shift+S salva como.
//!
//! `--edit`, `--undo` e `--save-to` fazem edições como se digitadas (na aba
//! inicial, depois de carregar tudo) e salvam numa cópia: para testar a
//! edição e o salvamento sem mexer na janela.

mod app;
mod axis;
mod draw;
mod edit;
mod format;
mod loader;
mod look;
mod search;

use std::path::PathBuf;
use std::time::Instant;

// Mesmo alocador e opções do duck-bench (ADR-012, medidos no arquivo de
// referência): arenas por thread, memória liberada devolvida logo ao SO.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const MI_OPTION_ARENA_EAGER_COMMIT: libmimalloc_sys::mi_option_t = 4;
const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;

extern "C" fn configure_allocator() {
    // SAFETY: só ajusta opções do mimalloc; roda antes de qualquer outra thread.
    unsafe {
        libmimalloc_sys::mi_option_set(MI_OPTION_ARENA_EAGER_COMMIT, 0);
        libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, 1);
    }
}

// No Linux a opção de arena precisa valer antes do runtime do Rust alocar.
#[cfg(target_os = "linux")]
#[used]
#[unsafe(link_section = ".init_array")]
static CONFIGURE_ALLOCATOR: extern "C" fn() = configure_allocator;

/// Opções da linha de comando.
#[derive(Clone, Debug, Default)]
pub struct Opts {
    pub path: Option<PathBuf>,
    /// Aba inicial (1 = primeira do workbook).
    pub sheet: Option<usize>,
    pub cell: Option<String>,
    pub zoom: Option<u16>,
    pub find: Option<String>,
    pub bench: bool,
    pub screenshot: Option<PathBuf>,
    /// `REF=texto`, aplicados em ordem como se digitados.
    pub edits: Vec<String>,
    /// Desfazer N vezes depois das edições.
    pub undo: usize,
    /// Salvar numa cópia depois das edições e sair.
    pub save_to: Option<PathBuf>,
}

fn parse_args() -> Opts {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || {
            i += 1;
            args.get(i).cloned()
        };
        match a {
            "--sheet" => o.sheet = val().and_then(|v| v.parse().ok()),
            "--cell" => o.cell = val(),
            "--zoom" => o.zoom = val().and_then(|v| v.parse().ok()),
            "--find" => o.find = val(),
            "--bench" => o.bench = true,
            "--screenshot" => o.screenshot = val().map(PathBuf::from),
            "--edit" => o.edits.extend(val()),
            "--undo" => o.undo = val().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--save-to" => o.save_to = val().map(PathBuf::from),
            _ if !a.starts_with("--") && o.path.is_none() => o.path = Some(PathBuf::from(a)),
            _ => eprintln!("opção desconhecida: {a}"),
        }
        i += 1;
    }
    o
}

fn main() {
    configure_allocator();
    let t0 = Instant::now();
    // Leitura e busca usam os núcleos físicos (hyperthreads quase não ajudam
    // no parse e aumentam a memória em trânsito).
    let _ = rayon::ThreadPoolBuilder::new().num_threads(duck_par::physical_cores()).build_global();
    let opts = parse_args();
    let mut app = app::SheetsApp::new(t0, opts);
    let config = duck_platform::WindowConfig { title: "duck-sheets".into(), width: 1440.0, height: 900.0 };
    if let Err(e) = duck_platform::run(config, &mut app) {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
    if let Some(e) = app.error() {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
}
