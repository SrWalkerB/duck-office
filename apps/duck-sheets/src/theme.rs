//! Tema do app: todas as cores da interface num lugar só (claro e escuro), e
//! como as cores do arquivo aparecem no escuro.
//!
//! No tema escuro a planilha também escurece, só na tela: o modelo, o que se
//! copia e o arquivo salvo continuam com as cores originais. As regras:
//!
//! - **Fundos** ([`Theme::fill`]): os claros viram escuros do mesmo matiz
//!   (branco = fundo da planilha); os escuros e os vivos ficam como estão.
//! - **Textos** ([`Theme::ink`]): ficam como estão se contrastam com o fundo
//!   mostrado; senão mudam de claridade, com o mesmo matiz, até dar para ler.
//!   Texto da cor do próprio fundo (escondido de propósito) continua escondido.
//! - **Linhas** ([`Theme::line_on`]): as discretas continuam discretas.
//! - **Objetos** ([`Theme::adapt_object`]): as mesmas regras sobre o que
//!   gráficos, formas e segmentações desenham; imagens não mudam.
//!
//! No tema claro tudo isso devolve a cor como veio.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use duck_gfx::{Instance, KIND_DASH, KIND_GLYPH_ROT, KIND_IMAGE, KIND_LINE, KIND_SOLID, KIND_TRI};
use duck_platform::ColorScheme;
use duck_ui::{InputColors, Rgba, TabColors, rgb};

// ---------------------------------------------------------------------------
// Paletas
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Theme {
    pub dark: bool,

    // Faixa de opções, menus, caixas de diálogo, barra de status, zoom.
    /// Abas da faixa e barra de status.
    pub bg: Rgba,
    /// Corpo da faixa e barra de fórmulas.
    pub body: Rgba,
    /// Menus e caixas de diálogo.
    pub menu: Rgba,
    /// Campos, caixas com seta, quadradinhos de marcar.
    pub field: Rgba,
    pub line: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub hover: Rgba,
    /// Botão ligado, item escolhido.
    pub on: Rgba,
    pub accent: Rgba,
    /// Texto sobre `accent`.
    pub on_accent: Rgba,
    /// Sombra dos menus e o véu atrás das caixas de diálogo.
    pub shadow: Rgba,
    pub veil: Rgba,
    pub input: InputColors,
    pub tabs: TabColors,

    // Grade.
    /// Fundo da planilha (célula sem preenchimento).
    pub paper: Rgba,
    pub grid: Rgba,
    pub header_bg: Rgba,
    pub header_text: Rgba,
    pub header_line: Rgba,
    pub header_sel: Rgba,
    /// Cabeçalho de uma linha ou coluna inteira selecionada.
    pub header_full: Rgba,
    pub header_full_text: Rgba,
    pub frozen_line: Rgba,
    /// Contorno da seleção e da célula em edição.
    pub selection: Rgba,
    pub sel_fill: Rgba,
    pub hit_fill: Rgba,
    /// Referências da fórmula em edição (texto e contorno).
    pub refs: [Rgba; 7],
    pub track: Rgba,
    pub thumb: Rgba,
    pub thumb_active: Rgba,
    // Agrupamentos.
    pub outline_bg: Rgba,
    pub outline_box: Rgba,
    pub outline_line: Rgba,
    pub outline_text: Rgba,

    // Notas, conversas e mensagens de entrada.
    /// Marca no canto da célula: nota e conversa.
    pub note: Rgba,
    pub thread: Rgba,
    pub note_bg: Rgba,
    pub note_line: Rgba,
    pub note_text: Rgba,
    pub note_muted: Rgba,
    /// Texto de erro numa caixa de diálogo.
    pub danger: Rgba,
}

pub static LIGHT: Theme = Theme {
    dark: false,
    bg: rgb(0xF8F9FA),
    body: rgb(0xFFFFFF),
    menu: rgb(0xFFFFFF),
    field: rgb(0xFFFFFF),
    line: rgb(0xDADCE0),
    text: rgb(0x202124),
    muted: rgb(0x5F6368),
    hover: rgb(0xE8EAED),
    on: rgb(0xD3E3FD),
    accent: rgb(0x0B57D0),
    on_accent: rgb(0xFFFFFF),
    shadow: [0, 0, 0, 30],
    veil: [0, 0, 0, 40],
    input: InputColors {
        background: rgb(0xFFFFFF),
        border: rgb(0xC4C7C5),
        focus: rgb(0x1A73E8),
        text: rgb(0x202124),
        placeholder: rgb(0x80868B),
        selection: rgb(0xC2DBFF),
    },
    tabs: TabColors {
        background: rgb(0xF8F9FA),
        tab: rgb(0xF8F9FA),
        active: rgb(0xFFFFFF),
        text: rgb(0x5F6368),
        active_text: rgb(0x188038),
        line: rgb(0xDADCE0),
        accent: rgb(0x188038),
    },
    paper: rgb(0xFFFFFF),
    grid: rgb(0xE1E1E1),
    header_bg: rgb(0xF8F9FA),
    header_text: rgb(0x5F6368),
    header_line: rgb(0xD5D7DA),
    header_sel: rgb(0xD3E3FD),
    header_full: rgb(0x0B57D0),
    header_full_text: rgb(0xFFFFFF),
    frozen_line: rgb(0xBDC1C6),
    selection: rgb(0x0B57D0),
    sel_fill: [14, 101, 235, 26],
    hit_fill: [255, 213, 0, 110],
    refs: [
        rgb(0x1A56DB),
        rgb(0xC5221F),
        rgb(0x7B1FA2),
        rgb(0x137333),
        rgb(0xA85500),
        rgb(0x00696B),
        rgb(0xB3126B),
    ],
    track: rgb(0xF1F3F4),
    thumb: rgb(0xBDC1C6),
    thumb_active: rgb(0x80868B),
    outline_bg: rgb(0xF3F4F6),
    outline_box: rgb(0xFFFFFF),
    outline_line: rgb(0x7F7F7F),
    outline_text: rgb(0x404040),
    note: rgb(0xD93025),
    thread: rgb(0x8430CE),
    note_bg: rgb(0xFFFBE0),
    note_line: rgb(0xB8AF7A),
    note_text: rgb(0x202124),
    note_muted: rgb(0x6B6546),
    danger: rgb(0xC5221F),
};

pub static DARK: Theme = Theme {
    dark: true,
    bg: rgb(0x1B1C1F),
    body: rgb(0x26282C),
    menu: rgb(0x2B2D31),
    field: rgb(0x1E1F22),
    line: rgb(0x43464C),
    text: rgb(0xE8EAED),
    muted: rgb(0xA7ADB5),
    hover: rgb(0x3A3D42),
    on: rgb(0x2F4A74),
    accent: rgb(0x8AB4F8),
    on_accent: rgb(0x10233F),
    shadow: [0, 0, 0, 110],
    veil: [0, 0, 0, 120],
    input: InputColors {
        background: rgb(0x1E1F22),
        border: rgb(0x5A5E65),
        focus: rgb(0x8AB4F8),
        text: rgb(0xE8EAED),
        placeholder: rgb(0x9AA0A6),
        selection: rgb(0x2F4F80),
    },
    tabs: TabColors {
        background: rgb(0x1B1C1F),
        tab: rgb(0x1B1C1F),
        active: rgb(0x2B2D31),
        text: rgb(0xA7ADB5),
        active_text: rgb(0x81C995),
        line: rgb(0x43464C),
        accent: rgb(0x81C995),
    },
    paper: rgb(0x1E1F22),
    grid: rgb(0x34363B),
    header_bg: rgb(0x26282C),
    header_text: rgb(0xA7ADB5),
    header_line: rgb(0x3C3F45),
    header_sel: rgb(0x2F4A74),
    header_full: rgb(0x8AB4F8),
    header_full_text: rgb(0x10233F),
    frozen_line: rgb(0x6A6F77),
    selection: rgb(0x8AB4F8),
    sel_fill: [138, 180, 248, 44],
    hit_fill: [255, 200, 0, 90],
    refs: [
        rgb(0x8AB4F8),
        rgb(0xF28B82),
        rgb(0xC58AF9),
        rgb(0x81C995),
        rgb(0xFBBC5A),
        rgb(0x78D9EC),
        rgb(0xFF8BCB),
    ],
    track: rgb(0x1B1C1F),
    thumb: rgb(0x53575E),
    thumb_active: rgb(0x8A9099),
    outline_bg: rgb(0x212327),
    outline_box: rgb(0x2B2D31),
    outline_line: rgb(0x8A9099),
    outline_text: rgb(0xC9CDD2),
    note: rgb(0xFF6B5E),
    thread: rgb(0xC58AF9),
    note_bg: rgb(0x3A3726),
    note_line: rgb(0x7C7452),
    note_text: rgb(0xF1EEDD),
    note_muted: rgb(0xBDB694),
    danger: rgb(0xF28B82),
};

// ---------------------------------------------------------------------------
// Escolha do usuário
// ---------------------------------------------------------------------------

/// O que o usuário escolheu na aba Exibir.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
    #[default]
    System,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Light => "claro",
            Mode::Dark => "escuro",
            Mode::System => "sistema",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Mode::Light, Mode::Dark, Mode::System].into_iter().find(|m| m.name() == s.trim().to_lowercase())
    }

    /// O tema desta escolha com o sistema em `system`.
    pub fn theme(self, system: ColorScheme) -> &'static Theme {
        match (self, system) {
            (Mode::Dark, _) | (Mode::System, ColorScheme::Dark) => &DARK,
            _ => &LIGHT,
        }
    }

    /// Cores da moldura da janela (`None` = as do sistema).
    pub fn frame(self) -> Option<ColorScheme> {
        match self {
            Mode::Light => Some(ColorScheme::Light),
            Mode::Dark => Some(ColorScheme::Dark),
            Mode::System => None,
        }
    }
}

const KEY: &str = "tema";

/// Preferências do usuário (ao lado da lista de recentes), uma por linha:
/// `chave=valor`.
pub fn pref_file() -> Option<std::path::PathBuf> {
    duck_platform::config_dir("duck-office").map(|d| d.join("preferencias.txt"))
}

/// O tema gravado no arquivo de preferências (padrão: o do sistema).
pub fn load(file: &Path) -> Mode {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == KEY)
        .and_then(|(_, v)| Mode::parse(v))
        .unwrap_or_default()
}

/// Grava o tema; as outras preferências do arquivo ficam.
pub fn save(file: &Path, mode: Mode) -> std::io::Result<()> {
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let mut text: String = old
        .lines()
        .filter(|l| l.split_once('=').is_none_or(|(k, _)| k.trim() != KEY) && !l.trim().is_empty())
        .map(|l| format!("{l}\n"))
        .collect();
    text.push_str(&format!("{KEY}={}\n", mode.name()));
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, text)
}

// ---------------------------------------------------------------------------
// Cores do arquivo no escuro
// ---------------------------------------------------------------------------

/// Cor em OKLab: claridade (0 a 1) e os dois eixos de cor.
#[derive(Clone, Copy, Debug)]
struct Lab {
    l: f32,
    a: f32,
    b: f32,
}

fn to_linear(c: u8) -> f32 {
    let x = c as f32 / 255.0;
    if x <= 0.040_45 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(x: f32) -> f32 {
    if x <= 0.003_130_8 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

impl Lab {
    fn of(c: Rgba) -> Self {
        let [r, g, b] = [c[0], c[1], c[2]].map(to_linear);
        let l = (0.412_221_47 * r + 0.536_332_5 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b).cbrt();
        Lab {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    fn linear(self) -> [f32; 3] {
        let l = (self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b).powi(3);
        let m = (self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b).powi(3);
        let s = (self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b).powi(3);
        [
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            -0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        ]
    }

    fn chroma(self) -> f32 {
        self.a.hypot(self.b)
    }

    /// A mesma cor com outra claridade (menos cor, se não couber em sRGB).
    fn with_l(self, l: f32, alpha: u8) -> Rgba {
        let l = l.clamp(0.0, 1.0);
        let at = |k: f32| Lab { l, a: self.a * k, b: self.b * k };
        let fits = |c: Lab| c.linear().iter().all(|x| (-0.001..=1.001).contains(x));
        let mut k = 1.0;
        if !fits(at(1.0)) {
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..10 {
                let mid = (lo + hi) / 2.0;
                if fits(at(mid)) { lo = mid } else { hi = mid }
            }
            k = lo;
        }
        let [r, g, b] =
            at(k).linear().map(|x| (from_linear(x.clamp(0.0, 1.0)) * 255.0).round().clamp(0.0, 255.0) as u8);
        [r, g, b, alpha]
    }
}

/// Contraste entre duas cores (WCAG: 1 a 21).
pub fn contrast(a: Rgba, b: Rgba) -> f32 {
    let y = |c: Rgba| 0.2126 * to_linear(c[0]) + 0.7152 * to_linear(c[1]) + 0.0722 * to_linear(c[2]);
    let (x, y) = (y(a) + 0.05, y(b) + 0.05);
    x.max(y) / x.min(y)
}

/// Contraste mínimo de um texto e de uma linha que aparecia bem no claro.
pub const MIN_TEXT: f32 = 4.5;
const MIN_LINE: f32 = 3.0;
/// E de uma barra, fatia ou marcador de gráfico.
const MIN_MARK: f32 = 2.2;
/// Área (pixels) de uma parte pequena de um objeto.
const SMALL: f32 = 400.0;
/// Claridade acima da qual um fundo escurece.
const MID: f32 = 0.60;

fn fill_dark(c: Rgba, paper: Rgba) -> Rgba {
    let lab = Lab::of(c);
    if lab.l <= MID {
        return c;
    }
    let p = Lab::of(paper);
    let t = (lab.l - MID) / (1.0 - MID);
    // Fundos vivos (amarelo, laranja) ficam: o texto é que se ajusta.
    let vivid = ((lab.chroma() - 0.10) / 0.08).clamp(0.0, 1.0);
    let mapped = MID + (p.l - MID) * t;
    let l = mapped + (lab.l - mapped) * vivid;
    // Perto do branco, o matiz do próprio fundo da planilha.
    let near = ((t - 0.75) * 4.0).clamp(0.0, 1.0) * (1.0 - (lab.chroma() / 0.03).clamp(0.0, 1.0));
    let hue = Lab { l, a: lab.a + (p.a - lab.a) * near, b: lab.b + (p.b - lab.b) * near };
    hue.with_l(l, c[3])
}

/// `c` com a claridade mudada até o contraste com `bg` chegar a `min`.
fn readable(c: Rgba, bg: Rgba, min: f32) -> Rgba {
    if contrast(c, bg) >= min {
        return c;
    }
    let lab = Lab::of(c);
    let up = contrast(rgb(0xFFFFFF), bg) >= contrast(rgb(0x000000), bg);
    // O oposto da claridade original: o preto vira quase branco, e um
    // cinza continua mais apagado que ele.
    let mut l = if up { 0.93 - 0.40 * lab.l } else { 0.45 * (1.0 - lab.l) };
    for _ in 0..40 {
        let x = lab.with_l(l, c[3]);
        if contrast(x, bg) >= min || l <= 0.0 || l >= 1.0 {
            return x;
        }
        l += if up { 0.02 } else { -0.02 };
    }
    lab.with_l(l, c[3])
}

thread_local! {
    /// Contas já feitas: (operação, cor, fundo original) → cor na tela.
    static MEMO: RefCell<HashMap<(u8, Rgba, Rgba), Rgba>> = RefCell::new(HashMap::new());
}

fn memo(op: u8, c: Rgba, bg: Rgba, f: impl FnOnce() -> Rgba) -> Rgba {
    MEMO.with(|m| {
        if let Some(x) = m.borrow().get(&(op, c, bg)) {
            return *x;
        }
        let x = f();
        let mut m = m.borrow_mut();
        // Escalas de cor podem ter muitos tons: recomeça se crescer demais.
        if m.len() > 50_000 {
            m.clear();
        }
        m.insert((op, c, bg), x);
        x
    })
}

const WHITE: Rgba = [255, 255, 255, 255];

impl Theme {
    /// Como aparece um fundo do arquivo.
    pub fn fill(&self, c: Rgba) -> Rgba {
        if !self.dark {
            return c;
        }
        memo(0, c, WHITE, || fill_dark(c, self.paper))
    }

    /// Como aparece um texto do arquivo sobre o fundo `bg` (a cor original
    /// dele; `None` = célula sem preenchimento).
    pub fn ink(&self, c: Rgba, bg: Option<Rgba>) -> Rgba {
        if !self.dark {
            return c;
        }
        let orig = bg.unwrap_or(WHITE);
        memo(1, c, orig, || {
            let shown = self.fill(orig);
            // Escondido de propósito (texto branco no branco).
            if contrast(c, orig) < 1.08 { shown } else { readable(c, shown, MIN_TEXT) }
        })
    }

    /// Como aparece uma linha do arquivo (borda, eixo) sobre o fundo `bg`.
    pub fn line_on(&self, c: Rgba, bg: Option<Rgba>) -> Rgba {
        if !self.dark {
            return c;
        }
        let orig = bg.unwrap_or(WHITE);
        memo(2, c, orig, || {
            let shown = self.fill(orig);
            let x = self.fill(c);
            // Aparece tanto quanto aparecia (até o contraste de `MIN_LINE`).
            let was = contrast(c, orig).min(MIN_LINE);
            if contrast(x, shown) >= if was < 2.0 { was * 0.8 } else { was } {
                return x;
            }
            // Uma linha preta vira um cinza claro, não branca.
            let lab = Lab::of(c);
            if Lab::of(shown).l < MID {
                lab.with_l(0.78 - 0.3 * lab.l, c[3])
            } else {
                readable(c, shown, 3.0)
            }
        })
    }

    /// Parte de um desenho (barra, fatia, marcador) sobre um fundo original
    /// `bg`, que aparece como `shown`: clareia se sumiria nele.
    fn mark_on(&self, c: Rgba, bg: Rgba, shown: Rgba) -> Rgba {
        let x = self.fill(c);
        let need = contrast(c, bg).min(MIN_MARK);
        if contrast(x, shown) >= need {
            return x;
        }
        let lab = Lab::of(x);
        (1..50)
            .map(|k| lab.with_l(lab.l + k as f32 * 0.02, x[3]))
            .find(|y| contrast(*y, shown) >= need)
            .unwrap_or(x)
    }

    /// Adapta o que um objeto (gráfico, forma, segmentação) desenhou em
    /// `out`. O fundo de um texto é o último retângulo ou triângulo desenhado
    /// embaixo dele; o das linhas e das outras partes, o primeiro (o fundo do
    /// objeto: uma linha de gráfico não muda de cor ao cruzar uma barra).
    /// `behind` guarda os fundos dos objetos anteriores (um texto sobre outra
    /// forma).
    pub fn adapt_object(&self, out: &mut [Instance], behind: &mut Vec<Surface>) {
        if !self.dark {
            return;
        }
        let mut own: Vec<Surface> = Vec::new();
        let base = (WHITE, self.paper);
        // Linhas da mesma cor continuam da mesma cor no objeto inteiro (um
        // contorno passa por dentro e por fora da forma).
        let mut lines: Vec<(Rgba, Rgba)> = Vec::new();
        for i in out.iter_mut() {
            let [x, y, w, h] = i.rect;
            let kind = i.kind & 0xFF;
            let solid = kind == KIND_SOLID && w.min(h) > 3.0;
            let (p, tri) = match kind {
                KIND_TRI => ([[x, y], [w, h], [i.uv[0], i.uv[1]]], true),
                _ => ([[x, y], [x + w, y + h], [0.0; 2]], false),
            };
            let at = match kind {
                KIND_TRI => ((x + w + i.uv[0]) / 3.0, (y + h + i.uv[1]) / 3.0),
                KIND_LINE => ((x + w) / 2.0, (y + h) / 2.0),
                KIND_GLYPH_ROT => (x, y),
                _ => (x + w / 2.0, y + h / 2.0),
            };
            match kind {
                KIND_IMAGE => own.push(Surface { p, tri, color: None }),
                _ if solid || kind == KIND_TRI => {
                    let orig = i.color;
                    let under = own.iter().find(|s| s.contains(at.0, at.1)).and_then(|s| s.color);
                    i.color = match under {
                        Some((bg, shown)) => self.mark_on(orig, bg, shown),
                        // Ponta de seta, marcador solto.
                        None if Surface { p, tri, color: None }.area() < SMALL => {
                            self.mark_on(orig, WHITE, self.paper)
                        }
                        None => self.fill(orig),
                    };
                    if orig[3] >= 128 {
                        own.push(Surface { p, tri, color: Some((orig, i.color)) });
                    }
                }
                KIND_SOLID | KIND_DASH | KIND_LINE => {
                    let c = i.color;
                    if let Some(l) = lines.iter().find(|l| l.0 == c) {
                        i.color = l.1;
                        continue;
                    }
                    let on = own.iter().find(|s| s.contains(at.0, at.1));
                    let on = on.or_else(|| behind.iter().rev().find(|s| s.contains(at.0, at.1)));
                    // Sobre uma imagem (a moldura dela), como sobre o papel.
                    let (bg, _) = on.and_then(|s| s.color).unwrap_or(base);
                    i.color = self.line_on(c, Some(bg));
                    lines.push((c, i.color));
                }
                _ => {
                    let c = i.color;
                    let mut on = own.iter().rev().chain(behind.iter().rev());
                    i.color = match on.find(|s| s.contains(at.0, at.1)).map(|s| s.color) {
                        Some(Some((bg, shown))) if contrast(c, bg) >= 1.08 => {
                            memo(3, c, shown, || readable(c, shown, MIN_TEXT))
                        }
                        // Escondido de propósito.
                        Some(Some((_, shown))) => shown,
                        // Sobre uma imagem: fica.
                        Some(None) => c,
                        // Sem fundo: está sobre as células. Um texto claro já
                        // estava sobre algo escuro.
                        None => memo(3, c, self.paper, || readable(c, self.paper, MIN_TEXT)),
                    };
                }
            }
        }
        // Só os fundos grandes valem para os objetos de cima.
        let area = own.iter().map(Surface::area).fold(0.0, f32::max);
        behind.extend(own.into_iter().filter(|s| s.area() >= area * 0.5));
    }
}

/// Um fundo desenhado por um objeto: a cor original e a que aparece (`None` =
/// imagem).
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    p: [[f32; 2]; 3],
    tri: bool,
    color: Option<(Rgba, Rgba)>,
}

impl Surface {
    fn area(&self) -> f32 {
        let [a, b, c] = self.p;
        if self.tri {
            ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
        } else {
            (b[0] - a[0]) * (b[1] - a[1])
        }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        let [a, b, c] = self.p;
        if !self.tri {
            return x >= a[0] && x < b[0] && y >= a[1] && y < b[1];
        }
        let side = |p: [f32; 2], q: [f32; 2]| (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0]);
        let (d, e, f) = (side(a, b), side(b, c), side(c, a));
        (d >= 0.0 && e >= 0.0 && f >= 0.0) || (d <= 0.0 && e <= 0.0 && f <= 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_is_readable_in_both() {
        for th in [&LIGHT, &DARK] {
            let pairs = [
                ("texto/faixa", th.text, th.body),
                ("texto/abas", th.text, th.bg),
                ("texto/menu", th.text, th.menu),
                ("texto/hover", th.text, th.hover),
                ("texto/ligado", th.text, th.on),
                ("apagado/faixa", th.muted, th.body),
                ("apagado/status", th.muted, th.bg),
                ("apagado/menu", th.muted, th.menu),
                ("destaque/faixa", th.accent, th.body),
                ("destaque/menu", th.accent, th.menu),
                ("sobre destaque", th.on_accent, th.accent),
                ("cabeçalho", th.header_text, th.header_bg),
                ("cabeçalho cheio", th.header_full_text, th.header_full),
                ("campo", th.input.text, th.input.background),
                ("campo selecionado", th.input.text, th.input.selection),
                ("aba", th.tabs.text, th.tabs.tab),
                ("aba ativa", th.tabs.active_text, th.tabs.active),
                ("agrupamento", th.outline_text, th.outline_box),
            ];
            for (what, a, b) in pairs {
                let c = contrast(a, b);
                assert!(c >= 4.5, "{what} (escuro: {}): contraste {c:.2}", th.dark);
            }
            // As referências coloridas, na célula e na barra de fórmulas.
            for (k, c) in th.refs.iter().enumerate() {
                assert!(contrast(*c, th.input.background) >= 4.5, "referência {k}");
                assert!(contrast(*c, th.body) >= 4.5, "referência {k}");
                assert!(th.refs[..k].iter().all(|o| o != c));
            }
        }
    }

    #[test]
    fn light_theme_changes_nothing() {
        for c in [rgb(0xFFFFFF), rgb(0), rgb(0xC6EFCE), [1, 2, 3, 40]] {
            assert_eq!(LIGHT.fill(c), c);
            assert_eq!(LIGHT.ink(c, None), c);
            assert_eq!(LIGHT.line_on(c, Some(c)), c);
        }
    }

    #[test]
    fn fills_in_dark() {
        // Branco = fundo da planilha; os escuros e os vivos ficam.
        assert_eq!(DARK.fill(rgb(0xFFFFFF)), DARK.paper);
        assert_eq!(DARK.fill(rgb(0x1F4E79)), rgb(0x1F4E79));
        assert_eq!(DARK.fill(rgb(0xFFFF00)), rgb(0xFFFF00));
        // Pastéis escurecem e guardam o matiz.
        for (c, strongest) in [(0xC6EFCE, 1), (0xFFC7CE, 0), (0xDDEBF7, 2)] {
            let x = DARK.fill(rgb(c));
            assert!(Lab::of(x).l < 0.5, "{c:06X} → {x:?}");
            assert_eq!(x.iter().take(3).max(), Some(&x[strongest]), "{c:06X} → {x:?}");
        }
        // A transparência fica.
        assert_eq!(DARK.fill([255, 255, 255, 100])[3], 100);
    }

    #[test]
    fn text_is_always_readable() {
        let fills = [
            0xFFFFFF, 0x000000, 0xC6EFCE, 0xFFEB9C, 0xFFC7CE, 0xFFFF00, 0x4472C4, 0x1F4E79, 0x70AD47,
            0xED7D31, 0xFFC000, 0xD9D9D9, 0x808080, 0xF2F2F2, 0xFF0000, 0x00B0F0,
        ];
        for bg in fills {
            for ink in [0x000000, 0xFFFFFF, 0x006100, 0x9C0006, 0x0000FF, 0xFF0000, 0x808080, 0xBFBFBF] {
                let (bg, ink) = (rgb(bg), rgb(ink));
                let (x, shown) = (DARK.ink(ink, Some(bg)), DARK.fill(bg));
                if contrast(ink, bg) < 1.08 {
                    assert_eq!(x, shown, "texto escondido continua escondido");
                } else {
                    let c = contrast(x, shown);
                    assert!(c >= MIN_TEXT - 0.05, "{ink:?} em {bg:?}: {x:?} em {shown:?} = {c:.2}");
                }
            }
        }
        // Preto automático numa célula vazia: quase branco. Vermelho continua vermelho.
        assert!(Lab::of(DARK.ink(rgb(0), None)).l > 0.9);
        let red = DARK.ink(rgb(0xFF0000), None);
        assert!(red[0] > red[1] && red[0] > red[2]);
    }

    #[test]
    fn lines_in_dark() {
        // Uma borda preta aparece; um cinza discreto continua discreto.
        let black = DARK.line_on(rgb(0), None);
        assert!(contrast(black, DARK.paper) > 4.0);
        let soft = contrast(DARK.line_on(rgb(0xD9D9D9), None), DARK.paper);
        assert!((1.1..2.5).contains(&soft), "{soft}");
        assert!(contrast(DARK.line_on(rgb(0xC00000), None), DARK.paper) > 3.0);
        // Branca sobre branco (não aparecia) continua sem aparecer.
        assert_eq!(DARK.line_on(rgb(0xFFFFFF), None), DARK.paper);
    }

    #[test]
    fn objects_in_dark() {
        let glyph = |x: f32, c: Rgba| Instance::glyph([x, 10.0, 6.0, 8.0], [0.0; 4], c);
        let mut out = vec![
            Instance::solid(0.0, 0.0, 100.0, 50.0, rgb(0xFFFFFF)),
            glyph(10.0, rgb(0x595959)),
            Instance::solid(50.0, 0.0, 50.0, 50.0, rgb(0x1F4E79)),
            glyph(60.0, rgb(0xFFFFFF)),
            Instance::solid(0.0, 25.0, 50.0, 1.0, rgb(0)),
            Instance::image([0.0, 0.0, 10.0, 10.0], [0.0; 4], 255),
        ];
        let before = out.clone();
        let mut behind = Vec::new();
        DARK.adapt_object(&mut out, &mut behind);
        assert_eq!(out[0].color, DARK.paper);
        assert!(contrast(out[1].color, DARK.paper) >= 4.4);
        // Um bloco escuro clareia só até se destacar do fundo.
        assert!((MIN_MARK..2.6).contains(&contrast(out[2].color, DARK.paper)));
        assert_eq!(out[3].color, rgb(0xFFFFFF));
        assert!(contrast(out[4].color, DARK.paper) > 3.0);
        assert_eq!(out[5].color, before[5].color);
        // Uma linha de gráfico tem a mesma cor dentro e fora de uma barra.
        let line = |x: f32| Instance::line([x, 30.0], [x + 4.0, 34.0], 1.0, rgb(0x1F3864));
        let mut chart = vec![
            Instance::solid(0.0, 0.0, 200.0, 100.0, rgb(0xFFFFFF)),
            Instance::solid(50.0, 0.0, 20.0, 100.0, rgb(0xED7D31)),
            line(10.0),
            line(58.0),
            // Barra azul-marinho: clareia para não sumir no fundo.
            Instance::solid(100.0, 0.0, 20.0, 100.0, rgb(0x1F3864)),
        ];
        DARK.adapt_object(&mut chart, &mut Vec::new());
        assert_eq!(chart[2].color, chart[3].color);
        assert!(contrast(chart[4].color, DARK.paper) >= MIN_MARK);
        // Caixa de texto sem fundo, em cima do objeto anterior.
        let mut text = vec![glyph(70.0, rgb(0xFFFFFF)), glyph(400.0, rgb(0))];
        DARK.adapt_object(&mut text, &mut behind);
        assert_eq!(text[0].color, rgb(0xFFFFFF));
        assert!(contrast(text[1].color, DARK.paper) >= 4.4);
        // No claro, nada muda.
        let mut same = before.clone();
        LIGHT.adapt_object(&mut same, &mut Vec::new());
        assert!(same.iter().zip(&before).all(|(a, b)| a.color == b.color));
    }

    #[test]
    fn choice_and_system() {
        use ColorScheme::{Dark, Light};
        assert!(Mode::Dark.theme(Light).dark && !Mode::Light.theme(Dark).dark);
        assert!(Mode::System.theme(Dark).dark && !Mode::System.theme(Light).dark);
        for m in [Mode::Light, Mode::Dark, Mode::System] {
            assert_eq!(Mode::parse(m.name()), Some(m));
        }
        assert_eq!(Mode::parse(" Escuro "), Some(Mode::Dark));
        assert_eq!(Mode::parse("azul"), None);
        assert_eq!(Mode::System.frame(), None);
    }

    #[test]
    fn remembered_between_runs() {
        let dir = std::env::temp_dir().join(format!("duck-tema-{}", std::process::id()));
        let file = dir.join("preferencias.txt");
        // Sem arquivo: o do sistema.
        assert_eq!(load(&file), Mode::System);
        save(&file, Mode::Dark).unwrap();
        assert_eq!(load(&file), Mode::Dark);
        // Outras preferências ficam, e valor estranho vira o padrão.
        std::fs::write(&file, "outra=1\ntema=azul\n").unwrap();
        assert_eq!(load(&file), Mode::System);
        save(&file, Mode::Light).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "outra=1\ntema=claro\n");
        assert_eq!(load(&file), Mode::Light);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
