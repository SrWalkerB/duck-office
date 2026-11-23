//! `duck-platform`: a camada que conversa com o sistema operacional.
//!
//! Janelas, eventos de entrada, DPI, clipboard e diálogos de arquivo, atrás de
//! uma API própria e estável. Hoje o backend é `winit` + `arboard` + `rfd`;
//! qualquer um deles pode ser trocado por chamadas diretas ao Win32 / X11 /
//! Wayland sem mudar os apps.
//!
//! Este crate não sabe nada de renderização: [`Window`] expõe os handles
//! nativos (`raw-window-handle`) para quem desenha (ex.: `duck-gfx`).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::window::{CursorIcon, WindowId};

pub use raw_window_handle;

// ---------------------------------------------------------------------------
// Configuração e erros
// ---------------------------------------------------------------------------

/// Parâmetros da janela principal.
#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub title: String,
    /// Tamanho inicial em pixels lógicos (antes do fator de escala do SO).
    pub width: f64,
    pub height: f64,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { title: "duck".into(), width: 1280.0, height: 800.0 }
    }
}

#[derive(Debug)]
pub enum Error {
    EventLoop(String),
    Window(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::EventLoop(e) => write!(f, "falha no loop de eventos: {e}"),
            Error::Window(e) => write!(f, "falha ao criar a janela: {e}"),
        }
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------
// Janela
// ---------------------------------------------------------------------------

/// Handle barato (clonável) para a janela nativa.
#[derive(Clone)]
pub struct Window {
    inner: Arc<winit::window::Window>,
    pending: Arc<Pending>,
}

/// Espera máxima de um pedido de frame enquanto chegam outros eventos.
const STARVED: Duration = Duration::from_millis(4);

/// Pedido de frame ainda não atendido: quando foi feito (µs desde `epoch`,
/// mais 1; 0 = nenhum).
struct Pending {
    epoch: Instant,
    at: AtomicU64,
}

impl Default for Pending {
    fn default() -> Self {
        Self { epoch: Instant::now(), at: AtomicU64::new(0) }
    }
}

impl Pending {
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64 + 1
    }

    /// `true` = peça ao sistema. Um pedido já feito não se repete (cada um é
    /// uma chamada ao sistema, e o mouse faz milhares por segundo), a não ser
    /// que o sistema o tenha deixado para trás.
    fn request(&self) -> bool {
        const LOST: u64 = 50_000;
        let now = self.now();
        match self.at.load(Ordering::Relaxed) {
            0 => {
                self.at.store(now, Ordering::Relaxed);
                true
            }
            at if now - at.min(now) > LOST => true,
            _ => false,
        }
    }

    fn clear(&self) {
        self.at.store(0, Ordering::Relaxed);
    }

    fn age(&self) -> Option<Duration> {
        match self.at.load(Ordering::Relaxed) {
            0 => None,
            at => Some(Duration::from_micros(self.now().saturating_sub(at))),
        }
    }
}

impl fmt::Debug for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Window").field("id", &self.inner.id()).finish()
    }
}

/// Cursores do mouse suportados.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Default,
    Pointer,
    Text,
    ResizeColumn,
    ResizeRow,
    /// Cruz (alça de preenchimento).
    Cross,
    /// Mover.
    Move,
    /// Redimensionar pelos cantos (↖↘ e ↗↙).
    ResizeNwse,
    ResizeNesw,
}

mod clock;

pub use clock::{LocalTime, local_time};

/// Esquema de cores do sistema (e da moldura da janela).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    fn from_winit(t: winit::window::Theme) -> Self {
        match t {
            winit::window::Theme::Light => Self::Light,
            winit::window::Theme::Dark => Self::Dark,
        }
    }

    fn to_winit(self) -> winit::window::Theme {
        match self {
            Self::Light => winit::window::Theme::Light,
            Self::Dark => winit::window::Theme::Dark,
        }
    }
}

impl Window {
    /// Cores da moldura da janela (barra de título). `None` = as do sistema:
    /// só assim chegam eventos [`Event::SchemeChanged`].
    pub fn set_color_scheme(&self, scheme: Option<ColorScheme>) {
        self.inner.set_theme(scheme.map(ColorScheme::to_winit));
    }

    /// Esquema em uso pela moldura: o do sistema, se a janela o segue.
    /// `None` onde o sistema não informa (X11, alguns Wayland).
    pub fn color_scheme(&self) -> Option<ColorScheme> {
        self.inner.theme().map(ColorScheme::from_winit)
    }

    /// Pede um novo frame. Várias chamadas antes do frame viram uma só.
    pub fn request_redraw(&self) {
        if self.pending.request() {
            self.inner.request_redraw();
        }
    }

    pub fn set_title(&self, title: &str) {
        self.inner.set_title(title);
    }

    /// Tamanho da área cliente em pixels físicos.
    pub fn inner_size(&self) -> (u32, u32) {
        let s = self.inner.inner_size();
        (s.width, s.height)
    }

    /// Fator de escala do monitor (1.0 = 96 DPI).
    pub fn scale_factor(&self) -> f64 {
        self.inner.scale_factor()
    }

    pub fn set_cursor(&self, cursor: Cursor) {
        let icon = match cursor {
            Cursor::Default => CursorIcon::Default,
            Cursor::Pointer => CursorIcon::Pointer,
            Cursor::Text => CursorIcon::Text,
            Cursor::ResizeColumn => CursorIcon::ColResize,
            Cursor::ResizeRow => CursorIcon::RowResize,
            Cursor::Cross => CursorIcon::Crosshair,
            Cursor::Move => CursorIcon::Move,
            Cursor::ResizeNwse => CursorIcon::NwseResize,
            Cursor::ResizeNesw => CursorIcon::NeswResize,
        };
        self.inner.set_cursor(icon);
    }

    /// Diálogo nativo "Abrir arquivo" (modal, bloqueia até o usuário escolher).
    /// `filters`: (nome, extensões sem ponto).
    pub fn pick_file(&self, title: &str, filters: &[(&str, &[&str])]) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new().set_title(title).set_parent(&*self.inner);
        for (name, exts) in filters {
            d = d.add_filter(*name, exts);
        }
        d.pick_file()
    }

    /// Diálogo nativo "Salvar como" (modal). `name` = nome sugerido.
    pub fn save_file(
        &self,
        title: &str,
        dir: Option<&Path>,
        name: &str,
        filters: &[(&str, &[&str])],
    ) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new().set_title(title).set_parent(&*self.inner).set_file_name(name);
        if let Some(dir) = dir {
            d = d.set_directory(dir);
        }
        for (n, exts) in filters {
            d = d.add_filter(*n, exts);
        }
        d.save_file()
    }

    /// Pergunta sim/não (modal). `true` = sim.
    pub fn confirm(&self, title: &str, text: &str) -> bool {
        rfd::MessageDialog::new()
            .set_title(title)
            .set_description(text)
            .set_level(rfd::MessageLevel::Warning)
            .set_parent(&*self.inner)
            .set_buttons(rfd::MessageButtons::OkCancelCustom("Continuar".into(), "Cancelar".into()))
            .show()
            == rfd::MessageDialogResult::Custom("Continuar".into())
    }

    /// Pergunta "salvar as alterações?" com três respostas (modal).
    pub fn ask_save(&self, title: &str, text: &str) -> SaveChoice {
        let r = rfd::MessageDialog::new()
            .set_title(title)
            .set_description(text)
            .set_level(rfd::MessageLevel::Warning)
            .set_parent(&*self.inner)
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                "Salvar".into(),
                "Não salvar".into(),
                "Cancelar".into(),
            ))
            .show();
        match r {
            rfd::MessageDialogResult::Yes => SaveChoice::Save,
            rfd::MessageDialogResult::No => SaveChoice::Discard,
            rfd::MessageDialogResult::Custom(s) if s == "Salvar" => SaveChoice::Save,
            rfd::MessageDialogResult::Custom(s) if s == "Não salvar" => SaveChoice::Discard,
            _ => SaveChoice::Cancel,
        }
    }
}

/// Resposta de [`Window::ask_save`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    Discard,
    Cancel,
}

/// Abre um endereço (`http`, `https`, `mailto`) no programa padrão do sistema.
/// Outros esquemas são recusados: o endereço vem de arquivos de terceiros.
pub fn open_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")) {
        return false;
    }
    let url = url.trim();
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.is_ok()
}

/// Pasta de configuração do usuário para o app `app` (não é criada aqui):
/// `%APPDATA%\app` no Windows, `$XDG_CONFIG_HOME/app` (ou `~/.config/app`)
/// no Linux, `~/Library/Application Support/app` no macOS.
pub fn config_dir(app: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(|b| b.join(app))
}

/// Registra o programa `exe` no "Abrir com" do sistema para as extensões
/// `exts`, só para o usuário atual (o programa padrão não muda).
///
/// - Windows: `HKCU\Software\Classes` (ProgId `name.Arquivo`, `OpenWithProgids`
///   de cada extensão e `Applications\<exe>`).
/// - Linux: um `.desktop` em `~/.local/share/applications` com os tipos MIME.
pub fn register_file_types(exe: &Path, name: &str, exts: &[&str]) -> Result<(), String> {
    let exe = exe.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        let prog = format!("{name}.Arquivo");
        let file = Path::new(&exe).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let cmd = format!("\"{exe}\" \"%1\"");
        let classes = r"HKCU\Software\Classes";
        let mut calls: Vec<Vec<String>> = vec![
            vec![format!(r"{classes}\{prog}"), "/ve".into(), "/d".into(), format!("Planilha ({name})")],
            vec![format!(r"{classes}\{prog}\shell\open\command"), "/ve".into(), "/d".into(), cmd.clone()],
            vec![
                format!(r"{classes}\Applications\{file}\shell\open\command"),
                "/ve".into(),
                "/d".into(),
                cmd,
            ],
        ];
        for e in exts {
            calls.push(vec![
                format!(r"{classes}\.{e}\OpenWithProgids"),
                "/v".into(),
                prog.clone(),
                "/t".into(),
                "REG_NONE".into(),
            ]);
            calls.push(vec![
                format!(r"{classes}\Applications\{file}\SupportedTypes"),
                "/v".into(),
                format!(".{e}"),
                "/t".into(),
                "REG_SZ".into(),
                "/d".into(),
                String::new(),
            ]);
        }
        for args in calls {
            let out = std::process::Command::new("reg")
                .arg("add")
                .args(&args)
                .arg("/f")
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).into_owned());
            }
        }
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mime: Vec<&str> = exts
            .iter()
            .map(|e| match *e {
                "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "xlsm" => "application/vnd.ms-excel.sheet.macroEnabled.12",
                "csv" => "text/csv",
                _ => "application/octet-stream",
            })
            .collect();
        let dir = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or("pasta do usuário não encontrada")?
            .join("applications");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec=\"{exe}\" %f\nMimeType={};\nTerminal=false\nCategories=Office;Spreadsheet;\n",
            mime.join(";")
        );
        std::fs::write(dir.join(format!("{name}.desktop")), desktop).map_err(|e| e.to_string())?;
        let _ = std::process::Command::new("update-desktop-database").arg(&dir).output();
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (exe, name, exts);
        Err("no macOS o duck-sheets.app do pacote já se registra para .xlsx, .xlsm e .csv".into())
    }
}

/// Mensagem simples numa caixa de diálogo nativa (erro ao abrir, por exemplo).
pub fn message_box(title: &str, text: &str) {
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(text)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

/// Mensagem informativa numa caixa de diálogo nativa ("Sobre", avisos).
pub fn info_box(title: &str, text: &str) {
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(text)
        .set_level(rfd::MessageLevel::Info)
        .show();
}

/// Acorda o loop de eventos de qualquer thread: o app recebe [`Event::Wake`].
/// Usado por quem trabalha em segundo plano (ex.: leitura do arquivo).
#[derive(Clone)]
pub struct Waker {
    proxy: EventLoopProxy<()>,
}

impl Waker {
    pub fn wake(&self) {
        let _ = self.proxy.send_event(());
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.inner.display_handle()
    }
}

// ---------------------------------------------------------------------------
// Eventos
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Tecla Windows / Super / Command.
    pub logo: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Space,
    /// F1..F24
    F(u8),
    /// Tecla que produz um caractere (já considerando o layout do teclado).
    Char(String),
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WheelDelta {
    /// Mouse de roda: unidades em "linhas".
    Lines { x: f32, y: f32 },
    /// Touchpad / roda de alta precisão: pixels físicos.
    Pixels { x: f64, y: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Nova área cliente, em pixels físicos.
    Resized {
        width: u32,
        height: u32,
    },
    ScaleFactorChanged(f64),
    /// Posição em pixels físicos, relativa ao canto superior esquerdo.
    MouseMove {
        x: f64,
        y: f64,
    },
    MouseDown {
        button: MouseButton,
        x: f64,
        y: f64,
        modifiers: Modifiers,
        /// 1 = clique, 2 = duplo clique, 3 = triplo...
        clicks: u8,
    },
    MouseUp {
        button: MouseButton,
        x: f64,
        y: f64,
        modifiers: Modifiers,
    },
    MouseLeft,
    Wheel {
        delta: WheelDelta,
        modifiers: Modifiers,
    },
    /// Pinça no touchpad (macOS): variação da escala (0,1 = +10%).
    Pinch(f64),
    Key {
        key: Key,
        pressed: bool,
        repeat: bool,
        modifiers: Modifiers,
        /// Texto produzido pela tecla, se houver.
        text: Option<String>,
    },
    Focused(bool),
    /// O sistema trocou entre claro e escuro.
    SchemeChanged(ColorScheme),
    /// Arquivo arrastado e solto na janela.
    FileDropped(PathBuf),
    /// Pedido de [`Waker::wake`].
    Wake,
    CloseRequested,
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// Clipboard do sistema. Criado sob demanda e mantido vivo (no X11 o conteúdo
/// copiado só existe enquanto o dono do clipboard existir).
#[derive(Default)]
pub struct Clipboard {
    inner: Option<arboard::Clipboard>,
}

impl Clipboard {
    fn get(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.inner = Some(c),
                Err(e) => log::warn!("clipboard indisponível: {e}"),
            }
        }
        self.inner.as_mut()
    }

    pub fn get_text(&mut self) -> Option<String> {
        self.get()?.get_text().ok()
    }

    pub fn set_text(&mut self, text: &str) -> bool {
        match self.get() {
            Some(c) => c.set_text(text.to_owned()).is_ok(),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// App e loop
// ---------------------------------------------------------------------------

/// Acesso a serviços da plataforma durante um callback do [`App`].
pub struct Context<'a> {
    event_loop: &'a ActiveEventLoop,
    clipboard: &'a mut Clipboard,
    proxy: &'a EventLoopProxy<()>,
}

impl Context<'_> {
    /// Encerra o loop de eventos depois do callback atual.
    pub fn exit(&self) {
        self.event_loop.exit();
    }

    pub fn clipboard(&mut self) -> &mut Clipboard {
        self.clipboard
    }

    /// Um [`Waker`] para acordar o loop de outra thread.
    pub fn waker(&self) -> Waker {
        Waker { proxy: self.proxy.clone() }
    }
}

/// Um aplicativo que roda sobre a plataforma.
pub trait App {
    /// Chamado uma vez, quando a janela nativa existe.
    fn init(&mut self, cx: &mut Context, window: Window);
    /// Entrada do usuário e mudanças da janela.
    fn event(&mut self, cx: &mut Context, event: Event);
    /// Hora de desenhar um frame (após [`Window::request_redraw`]).
    fn redraw(&mut self, cx: &mut Context);
    /// O usuário pediu para fechar a janela. `false` cancela (ex.: há
    /// alterações não salvas e ele escolheu "Cancelar").
    fn close_requested(&mut self, _cx: &mut Context) -> bool {
        true
    }
}

/// Cria a janela e roda o loop de eventos até o app encerrar.
/// O app continua com quem chamou (para ler estado final, erros etc.).
pub fn run<A: App>(config: WindowConfig, app: &mut A) -> Result<(), Error> {
    let event_loop =
        EventLoop::<()>::with_user_event().build().map_err(|e| Error::EventLoop(e.to_string()))?;
    // Ocioso = 0% de CPU: só acorda com eventos ou pedidos de redraw.
    event_loop.set_control_flow(ControlFlow::Wait);
    // Os movimentos crus do mouse (até 8 mil por segundo) não são usados e
    // encheriam a fila de mensagens.
    event_loop.listen_device_events(DeviceEvents::Never);

    let mut runner = Runner {
        app,
        config,
        window: None,
        clipboard: Clipboard::default(),
        modifiers: Modifiers::default(),
        cursor: (0.0, 0.0),
        last_click: None,
        proxy: event_loop.create_proxy(),
        error: None,
    };
    event_loop.run_app(&mut runner).map_err(|e| Error::EventLoop(e.to_string()))?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct Runner<'a, A> {
    app: &'a mut A,
    config: WindowConfig,
    window: Option<Window>,
    clipboard: Clipboard,
    modifiers: Modifiers,
    cursor: (f64, f64),
    /// Último clique (quando, onde, botão, contagem), para detectar duplo clique.
    last_click: Option<(Instant, (f64, f64), MouseButton, u8)>,
    proxy: EventLoopProxy<()>,
    error: Option<Error>,
}

impl<A: App> Runner<'_, A> {
    fn dispatch(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let mut cx = Context { event_loop, clipboard: &mut self.clipboard, proxy: &self.proxy };
        self.app.event(&mut cx, event);
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.pending.clear();
        }
        let mut cx = Context { event_loop, clipboard: &mut self.clipboard, proxy: &self.proxy };
        self.app.redraw(&mut cx);
    }

    /// O Windows só manda pintar com a fila de mensagens vazia: um mouse
    /// rápido segura o frame por centenas de milissegundos. Se o pedido já
    /// espera há tempo demais, desenha aqui mesmo.
    fn redraw_if_starved(&mut self, event_loop: &ActiveEventLoop) {
        let late = |w: &Window| w.pending.age().is_some_and(|a| a >= STARVED);
        if cfg!(windows) && self.window.as_ref().is_some_and(late) {
            self.redraw(event_loop);
        }
    }

    /// Conta cliques seguidos no mesmo lugar (intervalo do duplo clique).
    fn click_count(&mut self, button: MouseButton) -> u8 {
        const INTERVAL: Duration = Duration::from_millis(500);
        const SLOP: f64 = 4.0;
        let now = Instant::now();
        let (x, y) = self.cursor;
        let n = match self.last_click {
            Some((t, (px, py), b, n))
                if b == button && now - t < INTERVAL && (px - x).abs() <= SLOP && (py - y).abs() <= SLOP =>
            {
                n.saturating_add(1)
            }
            _ => 1,
        };
        self.last_click = Some((now, (x, y), button, n));
        n
    }
}

impl<A: App> ApplicationHandler<()> for Runner<'_, A> {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        self.dispatch(event_loop, Event::Wake);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = winit::window::Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(self.config.width, self.config.height))
            .with_min_inner_size(PhysicalSize::new(200, 150));
        match event_loop.create_window(attrs) {
            Ok(w) => {
                let window = Window { inner: Arc::new(w), pending: Arc::default() };
                self.window = Some(window.clone());
                let mut cx = Context { event_loop, clipboard: &mut self.clipboard, proxy: &self.proxy };
                self.app.init(&mut cx, window.clone());
                window.request_redraw();
            }
            Err(e) => {
                self.error = Some(Error::Window(e.to_string()));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                let mut cx = Context { event_loop, clipboard: &mut self.clipboard, proxy: &self.proxy };
                if self.app.close_requested(&mut cx) {
                    self.dispatch(event_loop, Event::CloseRequested);
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::Resized(size) => {
                self.dispatch(event_loop, Event::Resized { width: size.width, height: size.height })
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.dispatch(event_loop, Event::ScaleFactorChanged(scale_factor))
            }
            WindowEvent::ThemeChanged(t) => {
                self.dispatch(event_loop, Event::SchemeChanged(ColorScheme::from_winit(t)))
            }
            WindowEvent::Focused(f) => self.dispatch(event_loop, Event::Focused(f)),
            WindowEvent::DroppedFile(path) => self.dispatch(event_loop, Event::FileDropped(path)),
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.modifiers = Modifiers {
                    ctrl: s.control_key(),
                    shift: s.shift_key(),
                    alt: s.alt_key(),
                    logo: s.super_key(),
                };
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                self.dispatch(event_loop, Event::MouseMove { x: position.x, y: position.y });
                self.redraw_if_starved(event_loop);
            }
            WindowEvent::CursorLeft { .. } => self.dispatch(event_loop, Event::MouseLeft),
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    _ => MouseButton::Other,
                };
                let (x, y) = self.cursor;
                let ev = match state {
                    ElementState::Pressed => {
                        let clicks = self.click_count(button);
                        Event::MouseDown { button, x, y, modifiers: self.modifiers, clicks }
                    }
                    ElementState::Released => Event::MouseUp { button, x, y, modifiers: self.modifiers },
                };
                self.dispatch(event_loop, ev);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match delta {
                    MouseScrollDelta::LineDelta(x, y) => WheelDelta::Lines { x, y },
                    MouseScrollDelta::PixelDelta(p) => WheelDelta::Pixels { x: p.x, y: p.y },
                };
                let modifiers = self.modifiers;
                self.dispatch(event_loop, Event::Wheel { delta, modifiers });
                self.redraw_if_starved(event_loop);
            }
            WindowEvent::PinchGesture { delta, .. } if delta.is_finite() => {
                self.dispatch(event_loop, Event::Pinch(delta))
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let key = map_key(&event.logical_key);
                let ev = Event::Key {
                    key,
                    pressed: event.state == ElementState::Pressed,
                    repeat: event.repeat,
                    modifiers: self.modifiers,
                    text: event.text.map(|t| t.to_string()),
                };
                self.dispatch(event_loop, ev);
            }
            _ => {}
        }
    }
}

fn map_key(key: &WKey) -> Key {
    match key {
        WKey::Named(named) => match named {
            NamedKey::ArrowUp => Key::ArrowUp,
            NamedKey::ArrowDown => Key::ArrowDown,
            NamedKey::ArrowLeft => Key::ArrowLeft,
            NamedKey::ArrowRight => Key::ArrowRight,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Tab => Key::Tab,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Space => Key::Space,
            NamedKey::F1 => Key::F(1),
            NamedKey::F2 => Key::F(2),
            NamedKey::F3 => Key::F(3),
            NamedKey::F4 => Key::F(4),
            NamedKey::F5 => Key::F(5),
            NamedKey::F6 => Key::F(6),
            NamedKey::F7 => Key::F(7),
            NamedKey::F8 => Key::F(8),
            NamedKey::F9 => Key::F(9),
            NamedKey::F10 => Key::F(10),
            NamedKey::F11 => Key::F(11),
            NamedKey::F12 => Key::F(12),
            _ => Key::Other,
        },
        WKey::Character(c) => Key::Char(c.to_string()),
        _ => Key::Other,
    }
}
