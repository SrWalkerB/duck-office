//! `duck-gfx`: renderer 2D por instâncias sobre `wgpu`.
//!
//! Tudo que aparece na tela é um retângulo: fundos, bordas, linhas de grade
//! e glifos (retângulos que leem um atlas de alfa). Cada retângulo é uma
//! [`Instance`] de 40 bytes; um frame inteiro sai em **um draw call**.
//!
//! O crate não conhece janelas nem texto: recebe qualquer coisa que exponha
//! handles nativos (`raw-window-handle`) e um atlas que outro crate preenche.

use std::fmt;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

pub use bytemuck;

/// Um retângulo na tela, em pixels físicos.
///
/// - `kind == KIND_SOLID`: preenchido com `color`.
/// - `kind == KIND_GLYPH`: `color` modulado pelo alfa do atlas em `uv`
///   (coordenadas em texels, 1 texel = 1 pixel).
/// - `kind == KIND_DASH`: tracejado ao longo do lado maior; `uv[0]` = período
///   (negativo = vertical), `uv[1]` = parte desenhada, em pixels.
/// - `kind & 0xFF == KIND_GLYPH_ROT`: glifo girado em torno do canto
///   superior esquerdo de `rect`; o ângulo vai nos bits altos de `kind`
///   (ver [`Instance::glyph_rotated`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Instance {
    /// x, y, largura, altura
    pub rect: [f32; 4],
    /// u0, v0, u1, v1 em texels do atlas
    pub uv: [f32; 4],
    /// RGBA 8 bits, sRGB, não pré-multiplicado
    pub color: [u8; 4],
    pub kind: u32,
}

pub const KIND_SOLID: u32 = 0;
pub const KIND_GLYPH: u32 = 1;
pub const KIND_DASH: u32 = 2;
pub const KIND_GLYPH_ROT: u32 = 3;
/// Triângulo: vértices em `rect.xy`, `rect.zw` e `uv.xy` (gráficos).
pub const KIND_TRI: u32 = 4;
/// Segmento de reta suavizado: `rect` = as duas pontas, `uv[0]` = meia
/// espessura.
pub const KIND_LINE: u32 = 6;
/// Imagem: `uv` = região (0..1) do atlas de imagens (RGBA).
pub const KIND_IMAGE: u32 = 5;
/// Lado do atlas de imagens (pixels).
pub const IMAGE_ATLAS: u32 = 2048;

impl Instance {
    #[inline]
    pub fn solid(x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) -> Self {
        Self { rect: [x, y, w, h], uv: [0.0; 4], color, kind: KIND_SOLID }
    }

    #[inline]
    pub fn glyph(rect: [f32; 4], uv: [f32; 4], color: [u8; 4]) -> Self {
        Self { rect, uv, color, kind: KIND_GLYPH }
    }

    /// Glifo girado `degrees` graus no sentido anti-horário (como o texto
    /// girado do Excel), em torno do canto superior esquerdo de `rect`. Não
    /// há recorte: quem chama descarta os glifos que saem da área.
    #[inline]
    pub fn glyph_rotated(rect: [f32; 4], uv: [f32; 4], color: [u8; 4], degrees: f32) -> Self {
        // Centésimos de grau, deslocados para ficarem positivos (24 bits).
        let a = ((degrees.clamp(-360.0, 360.0) + 360.0) * 100.0).round() as u32;
        Self { rect, uv, color, kind: KIND_GLYPH_ROT | (a << 8) }
    }

    /// Triângulo cheio (sem recorte na GPU: quem chama recorta).
    #[inline]
    pub fn triangle(a: [f32; 2], b: [f32; 2], c: [f32; 2], color: [u8; 4]) -> Self {
        Self { rect: [a[0], a[1], b[0], b[1]], uv: [c[0], c[1], c[0], c[1]], color, kind: KIND_TRI }
    }

    /// Segmento de reta suavizado, com pontas redondas (sem recorte na GPU).
    #[inline]
    pub fn line(a: [f32; 2], b: [f32; 2], w: f32, color: [u8; 4]) -> Self {
        Self { rect: [a[0], a[1], b[0], b[1]], uv: [w / 2.0, 0.0, 0.0, 0.0], color, kind: KIND_LINE }
    }

    /// Imagem: `uv` = (u0, v0, u1, v1) no atlas de imagens; `alpha` = opacidade.
    #[inline]
    pub fn image(rect: [f32; 4], uv: [f32; 4], alpha: u8) -> Self {
        Self { rect, uv, color: [255, 255, 255, alpha], kind: KIND_IMAGE }
    }

    /// Linha tracejada (bordas pontilhadas, tracejadas...). O padrão segue a
    /// posição na tela, então linhas vizinhas ficam alinhadas.
    #[inline]
    pub fn dashed(x: f32, y: f32, w: f32, h: f32, color: [u8; 4], period: f32, on: f32) -> Self {
        let p = if w >= h { period } else { -period };
        Self { rect: [x, y, w, h], uv: [p, on, p, on], color, kind: KIND_DASH }
    }
}

#[derive(Debug)]
pub enum GfxError {
    Surface(String),
    NoAdapter(String),
    Device(String),
    UnsupportedSurface,
}

impl fmt::Display for GfxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GfxError::Surface(e) => write!(f, "não foi possível criar a superfície: {e}"),
            GfxError::NoAdapter(e) => write!(f, "nenhuma GPU compatível: {e}"),
            GfxError::Device(e) => write!(f, "falha ao abrir a GPU: {e}"),
            GfxError::UnsupportedSurface => write!(f, "superfície incompatível com a GPU"),
        }
    }
}

impl std::error::Error for GfxError {}

/// Resultado de um frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameStatus {
    /// Frame apresentado.
    Presented,
    /// Frame pulado (janela minimizada, superfície recriada...). Tente de novo.
    Skipped,
}

/// Informação sobre a GPU em uso.
#[derive(Clone, Debug)]
pub struct AdapterInfo {
    pub name: String,
    pub backend: String,
    pub software: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    screen: [f32; 2],
    _pad: [f32; 2],
}

const SHADER: &str = r#"
struct Globals { screen: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var images: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;

struct VIn {
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: u32,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) kind: u32,
    @location(3) @interpolate(flat) bounds: vec4<f32>,
};

fn texel(p: vec2<i32>, b: vec4<f32>) -> f32 {
    if (p.x < i32(b.x) || p.y < i32(b.y) || p.x >= i32(b.z) || p.y >= i32(b.w)) {
        return 0.0;
    }
    return textureLoad(atlas, p, 0).r;
}

@vertex
fn vs(in: VIn) -> VOut {
    // Triangle strip: 0=(0,0) 1=(1,0) 2=(0,1) 3=(1,1)
    let corner = vec2<f32>(f32(in.vi & 1u), f32(in.vi >> 1u));
    var p = in.rect.xy + corner * in.rect.zw;
    if (in.kind == 4u) {
        // Triângulo: o 4º vértice da faixa repete o 3º.
        if (in.vi == 0u) {
            p = in.rect.xy;
        } else if (in.vi == 1u) {
            p = in.rect.zw;
        } else {
            p = in.uv.xy;
        }
    }
    if (in.kind == 6u) {
        // Retângulo em volta do segmento, com folga para a borda suave.
        let d = in.rect.zw - in.rect.xy;
        let len = max(length(d), 0.001);
        let u = d / len;
        let n = vec2<f32>(-u.y, u.x);
        let e = in.uv.x + 1.0;
        let s = corner * 2.0 - vec2<f32>(1.0);
        p = mix(in.rect.xy, in.rect.zw, corner.x) + u * s.x * e + n * s.y * e;
    }
    if ((in.kind & 0xFFu) == 3u) {
        // Anti-horário na tela (y para baixo).
        let a = radians(f32(in.kind >> 8u) / 100.0 - 360.0);
        let d = corner * in.rect.zw;
        p = in.rect.xy + vec2<f32>(d.x * cos(a) + d.y * sin(a), -d.x * sin(a) + d.y * cos(a));
    }
    var out: VOut;
    out.pos = vec4<f32>(p.x / g.screen.x * 2.0 - 1.0, 1.0 - p.y / g.screen.y * 2.0, 0.0, 1.0);
    out.uv = mix(in.uv.xy, in.uv.zw, corner);
    out.color = in.color;
    out.kind = in.kind;
    out.bounds = in.uv;
    if (in.kind == 6u) {
        out.bounds = in.rect;
        out.uv = in.uv.xy;
    }
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    if (in.kind == 5u) {
        // Nível explícito: amostragem fora de fluxo uniforme.
        let c = textureSampleLevel(images, smp, in.uv, 0.0);
        return vec4<f32>(c.rgb, c.a * in.color.a);
    }
    if (in.kind == 6u) {
        let a = in.bounds.xy;
        let d = in.bounds.zw - a;
        let t = clamp(dot(in.pos.xy - a, d) / max(dot(d, d), 0.000001), 0.0, 1.0);
        let dist = length(in.pos.xy - a - d * t);
        return vec4<f32>(in.color.rgb, in.color.a * clamp(in.uv.x + 0.5 - dist, 0.0, 1.0));
    }
    if (in.kind == 1u) {
        let a = textureLoad(atlas, vec2<i32>(floor(in.uv)), 0).r;
        return vec4<f32>(in.color.rgb, in.color.a * a);
    }
    if ((in.kind & 0xFFu) == 3u) {
        // Girado: interpolação bilinear à mão (o atlas não tem sampler), só
        // dentro do glifo para não puxar o vizinho no atlas.
        let t = in.uv - vec2<f32>(0.5);
        let i = vec2<i32>(floor(t));
        let f = t - floor(t);
        let b = in.bounds;
        let a00 = texel(i, b);
        let a10 = texel(i + vec2<i32>(1, 0), b);
        let a01 = texel(i + vec2<i32>(0, 1), b);
        let a11 = texel(i + vec2<i32>(1, 1), b);
        let a = mix(mix(a00, a10, f.x), mix(a01, a11, f.x), f.y);
        return vec4<f32>(in.color.rgb, in.color.a * a);
    }
    if (in.kind == 2u) {
        let along = select(in.pos.y, in.pos.x, in.uv.x > 0.0);
        if ((along % abs(in.uv.x)) >= in.uv.y) {
            discard;
        }
    }
    return in.color;
}
"#;

/// Dono da GPU, da superfície da janela e do pipeline 2D.
pub struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    globals: wgpu::Buffer,
    instances: wgpu::Buffer,
    instance_capacity: usize,
    atlas: wgpu::Texture,
    atlas_size: u32,
    images: wgpu::Texture,
    info: AdapterInfo,
    timings: Vec<(&'static str, Duration)>,
}

/// Backend nativo de cada SO: DX12 no Windows, Vulkan no Linux, Metal no macOS.
/// `WGPU_BACKEND=vulkan|dx12|gl|metal` força outro (para testes).
fn primary_backends() -> wgpu::Backends {
    if cfg!(windows) {
        wgpu::Backends::DX12
    } else if cfg!(target_vendor = "apple") {
        wgpu::Backends::METAL
    } else {
        wgpu::Backends::VULKAN
    }
}

/// Cria instância, superfície e adaptador com os backends dados.
fn open<W>(window: &W, backends: wgpu::Backends) -> Result<(wgpu::Surface<'static>, wgpu::Adapter), GfxError>
where
    W: HasWindowHandle + HasDisplayHandle + fmt::Debug + Clone + Send + Sync + 'static,
{
    let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(window.clone()));
    desc.backends = backends;
    let instance = wgpu::Instance::new(desc.with_env());
    let surface = instance.create_surface(window.clone()).map_err(|e| GfxError::Surface(e.to_string()))?;
    let request = |fallback: bool| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: fallback,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
    };
    // GPU de verdade; se não houver, o adaptador por software do sistema
    // (WARP no Windows, llvmpipe no Linux).
    let adapter =
        request(false).or_else(|_| request(true)).map_err(|e| GfxError::NoAdapter(e.to_string()))?;
    Ok((surface, adapter))
}

fn is_software(adapter: &wgpu::Adapter) -> bool {
    adapter.get_info().device_type == wgpu::DeviceType::Cpu
}

/// Há uma GPU de verdade (não por software) em `backends`? Pergunta sem
/// tocar na janela e sem conectar ao compositor (instância sem display):
/// criar e destruir superfícies na mesma janela, só para sondar, já derrubou
/// o compositor do WSLg (Weston, segfault) com o programa aberto.
fn has_hardware_adapter(backends: wgpu::Backends) -> bool {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    let instance = wgpu::Instance::new(desc);
    pollster::block_on(instance.enumerate_adapters(backends))
        .iter()
        .any(|a| !matches!(a.get_info().device_type, wgpu::DeviceType::Cpu))
}

/// Escolhe superfície e adaptador.
///
/// 1. Backend nativo do SO (rápido de abrir: ~150–250 ms).
/// 2. Se ele falhar, ou só oferecer renderização por software, pergunta aos
///    outros backends se há GPU de verdade, **sem criar superfície na janela**.
///    Ex.: Linux sem driver Vulkan para a placa, mas com OpenGL acelerado.
/// 3. Só se houver, abre a janela nesse backend; senão, fica com o software
///    do backend nativo (a janela só ganhou uma superfície).
///
/// Com `WGPU_BACKEND` definido, respeita a escolha e não procura alternativas.
fn select<W>(window: &W) -> Result<(wgpu::Surface<'static>, wgpu::Adapter), GfxError>
where
    W: HasWindowHandle + HasDisplayHandle + fmt::Debug + Clone + Send + Sync + 'static,
{
    let primary = primary_backends();
    let others = wgpu::Backends::all() - primary;
    let first = open(window, primary);
    let forced = wgpu::Backends::from_env().is_some();
    match &first {
        Ok((_, adapter)) if forced || !is_software(adapter) => return first,
        Ok(_) if !has_hardware_adapter(others) => return first,
        Err(e) => log::warn!("backend principal falhou ({e}); tentando os demais"),
        Ok(_) => {}
    }
    // Há GPU de verdade em outro backend (ou o principal falhou): libera a
    // superfície do principal antes de abrir outra na mesma janela.
    let primary_works = first.is_ok();
    let first_err = first.err();
    let other = open(window, others);
    if let Ok((_, adapter)) = &other
        && (!is_software(adapter) || !primary_works)
    {
        return other;
    }
    let other_err = other.err();
    if primary_works {
        return open(window, primary);
    }
    Err(first_err.or(other_err).unwrap_or(GfxError::UnsupportedSurface))
}

/// GPU aberta antes da janela existir (instância, adaptador e device).
struct Early {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    timings: Vec<(&'static str, Duration)>,
}

/// Abertura da GPU em andamento numa thread (ver [`Gfx::start_early`]).
pub struct EarlyGpu(std::thread::JoinHandle<Option<Early>>);

/// Limites pedidos ao device (os mesmos com ou sem abertura antecipada).
fn limits(max_tex: u32) -> wgpu::Limits {
    wgpu::Limits { max_texture_dimension_2d: max_tex, ..wgpu::Limits::downlevel_webgl2_defaults() }
}

fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), GfxError> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("duck-gfx"),
        required_features: wgpu::Features::empty(),
        required_limits: limits(adapter.limits().max_texture_dimension_2d),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| GfxError::Device(e.to_string()))
}

impl Gfx {
    /// Começa a abrir a GPU numa thread, antes da janela existir: a parte cara
    /// (instância, adaptador, device: 150–250 ms) corre junto com a criação da
    /// janela e a leitura do arquivo. Só no DX12, em que qualquer adaptador
    /// apresenta em qualquer janela; nos outros backends a escolha depende da
    /// superfície (ex.: Vulkan × software no WSLg) e fica para [`Gfx::new`].
    /// Com `WGPU_BACKEND` definido, também fica para depois.
    pub fn start_early() -> Option<EarlyGpu> {
        if primary_backends() != wgpu::Backends::DX12 || wgpu::Backends::from_env().is_some() {
            return None;
        }
        let h = std::thread::Builder::new()
            .name("duck-gpu".into())
            .spawn(|| {
                let mut timings = Vec::new();
                let t = Instant::now();
                let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
                desc.backends = wgpu::Backends::DX12;
                let instance = wgpu::Instance::new(desc);
                let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    ..Default::default()
                }))
                .ok()
                .filter(|a| !is_software(a))?;
                timings.push(("instância + adaptador", t.elapsed()));
                let t = Instant::now();
                let (device, queue) = request_device(&adapter).ok()?;
                timings.push(("device", t.elapsed()));
                Some(Early { instance, adapter, device, queue, timings })
            })
            .ok()?;
        Some(EarlyGpu(h))
    }

    /// Abre a GPU e cria a superfície da janela.
    pub fn new<W>(window: W, width: u32, height: u32, vsync: bool, atlas_size: u32) -> Result<Self, GfxError>
    where
        W: HasWindowHandle + HasDisplayHandle + fmt::Debug + Clone + Send + Sync + 'static,
    {
        Self::new_with(None, window, width, height, vsync, atlas_size)
    }

    /// Como [`Gfx::new`], aproveitando uma abertura antecipada. Se ela tiver
    /// falhado ou o adaptador não servir para a janela, abre do jeito normal.
    pub fn new_with<W>(
        early: Option<EarlyGpu>,
        window: W,
        width: u32,
        height: u32,
        vsync: bool,
        atlas_size: u32,
    ) -> Result<Self, GfxError>
    where
        W: HasWindowHandle + HasDisplayHandle + fmt::Debug + Clone + Send + Sync + 'static,
    {
        let t = Instant::now();
        let early = early.and_then(|e| e.0.join().ok().flatten());
        let wait = t.elapsed();
        if let Some(e) = early {
            let t = Instant::now();
            if let Ok(surface) = e.instance.create_surface(window.clone())
                && e.adapter.is_surface_supported(&surface)
            {
                let mut timings = e.timings;
                timings.push(("espera da thread da GPU", wait));
                timings.push(("superfície criada", t.elapsed()));
                return Self::finish(
                    surface,
                    e.adapter,
                    Some((e.device, e.queue)),
                    width,
                    height,
                    vsync,
                    atlas_size,
                    timings,
                );
            }
            log::warn!("adaptador aberto antes da janela não serve para ela; abrindo de novo");
        }
        let mut timings = Vec::new();
        let t = Instant::now();
        // Inicializar um backend só custa ~250 ms; todos de uma vez, ~900 ms.
        let (surface, adapter) = select(&window)?;
        timings.push(("instância + adaptador", t.elapsed()));
        Self::finish(surface, adapter, None, width, height, vsync, atlas_size, timings)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        surface: wgpu::Surface<'static>,
        adapter: wgpu::Adapter,
        device: Option<(wgpu::Device, wgpu::Queue)>,
        width: u32,
        height: u32,
        vsync: bool,
        atlas_size: u32,
        mut timings: Vec<(&'static str, Duration)>,
    ) -> Result<Self, GfxError> {
        let ainfo = adapter.get_info();
        let info = AdapterInfo {
            name: ainfo.name.clone(),
            backend: format!("{:?}", ainfo.backend),
            software: ainfo.device_type == wgpu::DeviceType::Cpu,
        };

        let max_tex = adapter.limits().max_texture_dimension_2d;
        let atlas_size = atlas_size.min(max_tex);
        let (device, queue) = match device {
            Some(d) => d,
            None => {
                let t = Instant::now();
                let d = request_device(&adapter)?;
                timings.push(("device", t.elapsed()));
                d
            }
        };

        let t = Instant::now();
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or(GfxError::UnsupportedSurface)?;
        // Formato não-sRGB: a mistura de alfa acontece em espaço sRGB, como em
        // navegadores e toolkits de UI (texto com o peso esperado).
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = present_mode(vsync);
        config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &config);
        timings.push(("superfície (swapchain)", t.elapsed()));

        let t = Instant::now();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("duck-gfx shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d { width: atlas_size, height: atlas_size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let images = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("imagens"),
            size: wgpu::Extent3d { width: IMAGE_ATLAS, height: IMAGE_ATLAS, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let images_view = images.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("imagens"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("duck-gfx bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("duck-gfx bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&images_view),
                },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("duck-gfx layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
            0 => Float32x4,
            1 => Float32x4,
            2 => Unorm8x4,
            3 => Uint32,
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("duck-gfx pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &ATTRS,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let instance_capacity = 16 * 1024;
        let instances = create_instance_buffer(&device, instance_capacity);
        timings.push(("shader + pipeline", t.elapsed()));

        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group,
            globals,
            instances,
            instance_capacity,
            atlas,
            atlas_size,
            images,
            info,
            timings,
        })
    }

    pub fn adapter_info(&self) -> &AdapterInfo {
        &self.info
    }

    /// Quanto cada etapa da inicialização levou.
    pub fn init_timings(&self) -> &[(&'static str, Duration)] {
        &self.timings
    }

    /// Lado do atlas quadrado de glifos, em texels.
    pub fn atlas_size(&self) -> u32 {
        self.atlas_size
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if (width, height) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_vsync(&mut self, vsync: bool) {
        let mode = present_mode(vsync);
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Copia uma região de alfa (1 byte por pixel, linhas contíguas) para o atlas.
    pub fn upload_atlas(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        if w == 0 || h == 0 {
            return;
        }
        debug_assert_eq!(data.len(), (w * h) as usize);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Copia pixels RGBA (sRGB, linhas contíguas) para o atlas de imagens.
    pub fn upload_image(&self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        if w == 0 || h == 0 || x + w > IMAGE_ATLAS || y + h > IMAGE_ATLAS {
            return;
        }
        debug_assert_eq!(rgba.len(), (w * h * 4) as usize);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.images,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Desenha as instâncias, na ordem dada, sobre `clear` e apresenta.
    pub fn render(&mut self, instances: &[Instance], clear: [u8; 4]) -> FrameStatus {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(_)
            | wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Lost => {
                // Swapchain não bate mais com a janela: reconfigura e pula o frame.
                self.surface.configure(&self.device, &self.config);
                return FrameStatus::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return FrameStatus::Skipped,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let encoder = self.encode(&view, instances, clear);
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        FrameStatus::Presented
    }

    /// Desenha um frame fora da tela e devolve os pixels em RGBA 8 bits
    /// (para screenshots e testes visuais). Bloqueia até a GPU terminar.
    pub fn render_to_rgba(&mut self, instances: &[Instance], clear: [u8; 4]) -> (u32, u32, Vec<u8>) {
        let (w, h) = (self.config.width, self.config.height);
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.encode(&view, instances, clear);

        let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| {
            if let Err(e) = r {
                log::error!("falha ao ler o frame da GPU: {e}");
            }
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        if let Ok(data) = readback.get_mapped_range(..) {
            for y in 0..h as usize {
                let line = &data[y * row as usize..][..(w * 4) as usize];
                for px in line.as_chunks::<4>().0 {
                    if bgra {
                        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    } else {
                        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                }
            }
        }
        readback.unmap();
        (w, h, out)
    }

    /// Envia instâncias e uniforms e grava o render pass num encoder novo.
    fn encode(
        &mut self,
        view: &wgpu::TextureView,
        instances: &[Instance],
        clear: [u8; 4],
    ) -> wgpu::CommandEncoder {
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instances = create_instance_buffer(&self.device, self.instance_capacity);
        }
        if !instances.is_empty() {
            self.queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
        }
        let globals =
            Globals { screen: [self.config.width as f32, self.config.height as f32], _pad: [0.0; 2] };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let c = |v: u8| v as f64 / 255.0;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: c(clear[0]),
                            g: c(clear[1]),
                            b: c(clear[2]),
                            a: c(clear[3]),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !instances.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..4, 0..instances.len() as u32);
            }
        }
        encoder
    }
}

fn present_mode(vsync: bool) -> wgpu::PresentMode {
    if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: (capacity * std::mem::size_of::<Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
