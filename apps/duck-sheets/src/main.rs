//! `duck-sheets`: editor de planilhas `.xlsx` / `.xlsm` / `.csv` (F2, F3).
//! Abre também `.xlsb` (salvo como `.xlsx`/`.xlsm`).
//!
//! ```text
//! duck-sheets [ARQUIVO] [--sheet N] [--cell B12] [--zoom 80] [--find TEXTO]
//!             [--bench] [--screenshot SAIDA.png]
//!             [--edit A1=valor ...] [--undo N] [--save-to SAIDA.xlsx]
//!             [--aba inicial|inserir|exibir] [--menu NOME] [--novo]
//!             [--tema claro|escuro|sistema] [--digitar "=so"]
//! ```
//!
//! Sem arquivo, abre uma planilha em branco. Teclas: setas (+Shift seleciona,
//! +Ctrl vai até a borda dos dados), PageUp/PageDown, Ctrl+Home/End,
//! Ctrl+PageUp/PageDown (abas), Ctrl+C (copia a seleção), Ctrl+F (busca),
//! Ctrl+G (ir para), Ctrl+O (abrir), Ctrl+A, Ctrl + roda / Ctrl +/−/0 (zoom).
//!
//! Edição: digitar substitui a célula; F2, duplo clique ou clique na barra
//! de fórmulas editam o conteúdo; Enter/Tab confirmam, Esc cancela; Delete
//! apaga a seleção; Ctrl+Z/Ctrl+Y desfazem e refazem; Ctrl+S salva e
//! Ctrl+Shift+S salva como. Alt+Enter quebra a linha, Ctrl+Enter preenche a
//! seleção, Ctrl+D e Ctrl+R preenchem para baixo e para a direita, Ctrl+H
//! substitui, Ctrl+Alt+V cola especial, Ctrl+1 formata, Alt+= soma, Shift+F11
//! cria uma aba e Ctrl+clique soma áreas à seleção.
//!
//! `--edit`, `--undo` e `--save-to` fazem edições como se digitadas (na aba
//! inicial, depois de carregar tudo) e salvam numa cópia: para testar a
//! edição e o salvamento sem mexer na janela (a lista está no README).

// Sem um terminal atrás da janela no Windows; as opções que imprimem usam o
// de quem chamou (`attach_console`).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod actions;
mod app;
mod axis;
#[cfg(test)]
mod blocked;
mod calc;
mod cells;
mod cficon;
mod chart;
mod complete;
mod control;
mod cube;
mod dialog;
mod draw;
mod edit;
mod fill;
mod filter;
mod fmtcells;
mod format;
mod forms;
mod images;
mod loader;
mod look;
mod macros;
mod menu;
mod notes;
mod objects;
mod outline;
mod paste;
mod pivot;
mod pointing;
mod protect;
mod recovery;
mod replace;
mod ribbon;
mod search;
mod settings;
mod shape;
mod tablestyle;
mod tabs;
#[cfg(test)]
mod testing;
mod theme;
mod typed;
mod update;
mod valid;
mod zoombar;

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
    /// Aba da faixa de opções (`inicial`, `inserir`, `exibir`).
    pub ribbon: Option<String>,
    /// Menu aberto depois de carregar (para screenshot): `arquivo`, `fonte`,
    /// `tamanho`, `numero`, `bordas`, `cor`, `fundo`, `mesclar`, `inserir`,
    /// `excluir`, `formatar`, `limpar`, `congelar`, `contexto`.
    pub menu: Option<String>,
    /// Começa com uma planilha nova em branco mesmo com um arquivo na linha de
    /// comando (`--novo`; sem arquivo já é assim).
    pub new_file: bool,
    /// Tema desta execução (sem gravar a escolha). Sem isto vale a
    /// preferência; em `--screenshot` e `--bench`, o claro.
    pub theme: Option<theme::Mode>,
    /// No tema escuro, células escuras (`--planilha escura`) ou como no
    /// arquivo (`clara`), só nesta execução.
    pub dark_sheet: Option<bool>,
    /// Texto digitado na célula ativa, com teclas e cliques entre chaves (ver
    /// `scripted_typing`).
    pub typing: Option<String>,
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
            "--aba" => o.ribbon = val(),
            "--menu" => o.menu = val(),
            "--novo" => o.new_file = true,
            "--digitar" => o.typing = val(),
            "--tema" => {
                let v = val().unwrap_or_default();
                o.theme = theme::Mode::parse(&v);
                if o.theme.is_none() {
                    eprintln!("--tema: use claro, escuro ou sistema (veio \"{v}\")");
                }
            }
            "--planilha" => {
                let v = val().unwrap_or_default();
                o.dark_sheet = match v.as_str() {
                    "escura" => Some(true),
                    "clara" => Some(false),
                    _ => None,
                };
                if o.dark_sheet.is_none() {
                    eprintln!("--planilha: use clara ou escura (veio \"{v}\")");
                }
            }
            "--atualizar" => std::process::exit(update::command()),
            "--version" | "--versao" => {
                println!("Duck Sheets {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ if !a.starts_with("--") && o.path.is_none() => o.path = Some(PathBuf::from(a)),
            _ => eprintln!("opção desconhecida: {a}"),
        }
        i += 1;
    }
    o
}

/// O ícone da janela, onde ele não vem do pacote.
fn icon() -> Option<duck_platform::Icon> {
    if cfg!(not(target_os = "linux")) {
        return None;
    }
    let bytes = include_bytes!("../assets/icon-64.png");
    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes[..])).read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    rgba.truncate(info.buffer_size());
    (info.color_type == png::ColorType::Rgba).then_some(duck_platform::Icon { rgba, size: info.width })
}

/// O programa quebrou: guarda o que não estava salvo e avisa. O erro já está
/// no registro (ver [`recovery::install_log`]).
fn crashed(mut app: app::SheetsApp) -> ! {
    let kept =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.recovery_emergency())).unwrap_or(false);
    if !app.unattended() {
        let mut text = String::from("O Duck Sheets encontrou um erro e precisa fechar.");
        if kept {
            text.push_str(
                " As alterações não salvas foram guardadas: abra o Duck Sheets de novo para recuperá-las.",
            );
        }
        if let Some(f) = recovery::log_file() {
            text.push_str(&format!("\n\nOs detalhes do erro estão em {}", f.display()));
        }
        duck_platform::message_box("Duck Sheets", &text);
    }
    // Sem destrutores: o estado pode estar pela metade.
    std::process::exit(101);
}

fn main() {
    configure_allocator();
    recovery::install_log();
    let options = std::env::args().skip(1).any(|a| a.starts_with("--"));
    if options || std::env::var_os("DUCK_TRACE").is_some() {
        duck_platform::attach_console();
    }
    let t0 = Instant::now();
    // Leitura e busca usam os núcleos físicos (hyperthreads quase não ajudam
    // no parse e aumentam a memória em trânsito).
    let _ = rayon::ThreadPoolBuilder::new().num_threads(duck_par::physical_cores()).build_global();
    let opts = parse_args();
    let mut app = app::SheetsApp::new(t0, opts);
    let config = duck_platform::WindowConfig {
        title: "Duck Sheets".into(),
        width: 1440.0,
        height: 900.0,
        app_id: "duck-sheets".into(),
        icon: icon(),
    };
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| duck_platform::run(config, &mut app)));
    let Ok(run) = run else { crashed(app) };
    app.cleanup();
    if run.is_ok() && app.error().is_none() {
        app.update_on_exit();
    }
    if let Err(e) = run {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
    if let Some(e) = app.error() {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
}
