//! `duck-platform`: a camada que conversa com o sistema operacional.
//!
//! Janelas, eventos de entrada, DPI e clipboard, atrás de uma API própria e
//! estável. Hoje o backend é `winit` + `arboard`; qualquer um deles pode ser
//! trocado por chamadas diretas ao Win32 / X11 / Wayland sem mudar os apps.
//!
//! Este crate não sabe nada de renderização: [`Window`] expõe os handles
//! nativos (`raw-window-handle`) para quem desenha (ex.: `duck-gfx`).

use std::fmt;
use std::sync::Arc;

use raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
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
}

impl Window {
    /// Pede um novo frame. Várias chamadas antes do frame viram uma só.
    pub fn request_redraw(&self) {
        self.inner.request_redraw();
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
        };
        self.inner.set_cursor(icon);
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
    },
    MouseUp {
        button: MouseButton,
        x: f64,
        y: f64,
    },
    MouseLeft,
    Wheel {
        delta: WheelDelta,
        modifiers: Modifiers,
    },
    Key {
        key: Key,
        pressed: bool,
        repeat: bool,
        modifiers: Modifiers,
        /// Texto produzido pela tecla, se houver.
        text: Option<String>,
    },
    Focused(bool),
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
}

impl Context<'_> {
    /// Encerra o loop de eventos depois do callback atual.
    pub fn exit(&self) {
        self.event_loop.exit();
    }

    pub fn clipboard(&mut self) -> &mut Clipboard {
        self.clipboard
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
}

/// Cria a janela e roda o loop de eventos até o app encerrar.
/// O app continua com quem chamou (para ler estado final, erros etc.).
pub fn run<A: App>(config: WindowConfig, app: &mut A) -> Result<(), Error> {
    let event_loop = EventLoop::new().map_err(|e| Error::EventLoop(e.to_string()))?;
    // Ocioso = 0% de CPU: só acorda com eventos ou pedidos de redraw.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut runner = Runner {
        app,
        config,
        window: None,
        clipboard: Clipboard::default(),
        modifiers: Modifiers::default(),
        cursor: (0.0, 0.0),
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
    error: Option<Error>,
}

impl<A: App> Runner<'_, A> {
    fn dispatch(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let mut cx = Context { event_loop, clipboard: &mut self.clipboard };
        self.app.event(&mut cx, event);
    }
}

impl<A: App> ApplicationHandler for Runner<'_, A> {
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
                let window = Window { inner: Arc::new(w) };
                self.window = Some(window.clone());
                let mut cx = Context { event_loop, clipboard: &mut self.clipboard };
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
                self.dispatch(event_loop, Event::CloseRequested);
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                let mut cx = Context { event_loop, clipboard: &mut self.clipboard };
                self.app.redraw(&mut cx);
            }
            WindowEvent::Resized(size) => {
                self.dispatch(event_loop, Event::Resized { width: size.width, height: size.height })
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.dispatch(event_loop, Event::ScaleFactorChanged(scale_factor))
            }
            WindowEvent::Focused(f) => self.dispatch(event_loop, Event::Focused(f)),
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
                    ElementState::Pressed => Event::MouseDown { button, x, y },
                    ElementState::Released => Event::MouseUp { button, x, y },
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
