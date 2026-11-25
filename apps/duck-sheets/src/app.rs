//! Estado do app, geometria da janela, carga progressiva e entrada do usuário.
//! O desenho do frame fica em `draw.rs`.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use duck_core::{CellRange, Sheet, StringTable, Styles};
use duck_gfx::{FrameStatus, Gfx, Instance};
use duck_platform::{
    App, ColorScheme, Context, Cursor, Event, Key, Modifiers, MouseButton, WheelDelta, Window,
};
use duck_text::{FontSet, TextStyle, TextSystem};
use duck_ui::{Rect, TabBar, TextInput};
use duck_xlsx::SheetInfo;

use crate::Opts;
use crate::axis::Axis;
use crate::loader::{Loader, Msg, drain};
use crate::look::{Looks, edit_text, show};
use crate::search::{self, SearchJob, SearchResult};
use crate::settings::Prefs;
use crate::theme::{LIGHT, Mode, Theme};

// ---------------------------------------------------------------------------
// Parâmetros (pixels lógicos)
// ---------------------------------------------------------------------------

pub const UI_FONT_PX: f32 = 13.0;
pub const TOOLBAR_H: f32 = crate::ribbon::TABS_H + crate::ribbon::BODY_H;
pub const FBAR_H: f32 = 32.0;
pub const NAMEBOX_W: f32 = 110.0;
pub const HEADER_H: f32 = 20.0;
pub const TABS_H: f32 = 30.0;
pub const STATUS_H: f32 = 24.0;
pub const SCROLLBAR: f32 = 14.0;
pub const MIN_THUMB: f32 = 28.0;
pub const ATLAS: u32 = 2048;
/// Linhas e colunas máximas do Excel.
pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u32 = 16_384;

// ---------------------------------------------------------------------------
// Documento
// ---------------------------------------------------------------------------

/// Mesclagens de uma aba, ordenadas pela primeira linha.
#[derive(Default)]
pub struct Merges {
    list: Vec<CellRange>,
    /// Maior número de linhas de uma mesclagem (limita a busca para trás).
    max_span: u32,
}

impl Merges {
    pub fn new(mut list: Vec<CellRange>) -> Self {
        list.sort_by_key(|m| (m.first_row, m.first_col));
        let max_span = list.iter().map(|m| m.last_row - m.first_row).max().unwrap_or(0);
        Self { list, max_span }
    }

    /// Mesclagens que tocam as linhas `[r0, r1]`.
    pub fn in_rows(&self, r0: u32, r1: u32) -> impl Iterator<Item = &CellRange> {
        let from = r0.saturating_sub(self.max_span);
        let start = self.list.partition_point(|m| m.first_row < from);
        self.list[start..].iter().take_while(move |m| m.first_row <= r1).filter(move |m| m.last_row >= r0)
    }

    /// A mesclagem que contém a célula, se houver.
    pub fn at(&self, r: u32, c: u32) -> Option<CellRange> {
        self.in_rows(r, r).find(|m| m.contains(r, c)).copied()
    }
}

/// Estado de exibição de uma aba (rolagem, seleção, zoom, geometria).
pub struct View {
    pub rows: Axis,
    pub cols: Axis,
    pub frozen_rows: u32,
    pub frozen_cols: u32,
    /// Rolagem do painel principal, em pixels lógicos a 100%.
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub cur: (u32, u32),
    pub anchor: (u32, u32),
    /// Outras áreas da seleção múltipla e a âncora com que elas valem (uma
    /// seleção nova muda a âncora e as deixa para trás).
    pub extra: Vec<(u32, u32, u32, u32)>,
    pub extra_anchor: (u32, u32),
    pub zoom: u16,
    /// Agrupamento de linhas e colunas.
    pub row_groups: Vec<duck_core::outline::Group>,
    pub col_groups: Vec<duck_core::outline::Group>,
    pub outline: duck_core::outline::Outline,
    pub merges: Merges,
    pub grid: bool,
    pub headers: bool,
    /// Estilo de cada coluna (`<col style>`), para células sem `<c>`.
    pub col_styles: Vec<(u32, u32, u16)>,
    /// Estilo de linha inteira (`<row s customFormat>`), em ordem de linha.
    /// Vale antes do da coluna, como no Excel.
    pub row_styles: Vec<(u32, u16)>,
    /// Dados calculados de cada regra de formatação condicional (mesma ordem
    /// de `sheet.layout.cond`).
    pub cf: Vec<duck_core::CfState>,
    /// Larguras e alturas mudadas pelo usuário (pixels a 100%), mantidas
    /// quando a aba é remontada (prévia → aba inteira, fontes novas).
    pub resized_cols: Vec<(u32, u32)>,
    pub resized_rows: Vec<(u32, u32)>,
    /// Linhas e colunas que a grade mostra no mínimo (cresce quando o cursor
    /// passa do fim).
    pub min: (u32, u32),
}

impl View {
    /// Muda a largura de uma coluna (pixels lógicos a 100%).
    pub fn resize_col(&mut self, c: u32, size: u32) {
        self.cols.set_size(c, size);
        self.resized_cols.retain(|x| x.0 != c);
        self.resized_cols.push((c, size));
    }

    /// Muda a altura de uma linha (pixels lógicos a 100%).
    pub fn resize_row(&mut self, r: u32, size: u32) {
        self.rows.set_size(r, size);
        self.resized_rows.retain(|x| x.0 != r);
        self.resized_rows.push((r, size));
    }
}

impl View {
    /// Seleção normalizada (r0, c0, r1, c1).
    pub fn sel(&self) -> (u32, u32, u32, u32) {
        let (a, b) = (self.anchor, self.cur);
        (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
    }

    /// Há mais de uma área selecionada?
    pub fn multi(&self) -> bool {
        !self.extra.is_empty() && self.extra_anchor == self.anchor
    }

    /// Todas as áreas selecionadas; a última é a da célula ativa.
    pub fn areas(&self) -> Vec<(u32, u32, u32, u32)> {
        let mut v = if self.multi() { self.extra.clone() } else { Vec::new() };
        v.push(self.sel());
        v
    }

    /// Começa uma área: com `add`, a seleção de agora vai para as outras;
    /// com `extend`, nada muda; senão, as outras saem.
    pub fn begin_area(&mut self, add: bool, extend: bool) {
        if add {
            if !self.multi() {
                self.extra.clear();
            }
            let s = self.sel();
            self.extra.push(s);
        } else if !extend {
            self.extra.clear();
        }
    }

    /// Seleção nova: as outras áreas saem.
    pub fn single(&mut self) {
        self.extra.clear();
    }

    pub fn col_style(&self, c: u32) -> u16 {
        let i = self.col_styles.partition_point(|s| s.1 < c);
        match self.col_styles.get(i) {
            Some(&(a, _, st)) if a <= c => st,
            _ => 0,
        }
    }

    /// Estilo de uma célula sem `<c>` (ou com `s="0"`): o da linha, se houver,
    /// senão o da coluna.
    pub fn blank_style(&self, r: u32, c: u32) -> u16 {
        if !self.row_styles.is_empty()
            && let Ok(i) = self.row_styles.binary_search_by_key(&r, |s| s.0)
        {
            return self.row_styles[i].1;
        }
        self.col_style(c)
    }
}

pub struct Slot {
    pub info: SheetInfo,
    pub data: Option<Arc<Sheet>>,
    /// `data` é só a prévia (primeiras linhas).
    pub preview: bool,
    pub view: Option<View>,
    /// De onde vem o XML da aba ao salvar.
    pub origin: duck_xlsx::TabOrigin,
}

pub struct Doc {
    pub path: PathBuf,
    /// O arquivo como estava ao abrir (ou ao salvar): de onde vem o que não
    /// mudou.
    pub source: Option<Arc<duck_xlsx::Source>>,
    pub sheets: Vec<Slot>,
    pub active: usize,
    pub styles: Arc<Styles>,
    pub looks: Arc<Looks>,
    pub strings: Arc<StringTable>,
    pub date1904: bool,
    /// Largura máxima de dígito da fonte padrão a 100% (pixels): unidade das larguras de coluna.
    pub mdw: f32,
    pub loaded: usize,
    pub finished: Option<Duration>,
    /// Desfazer/refazer (F3).
    pub history: crate::edit::History,
    /// Há alterações não salvas.
    pub dirty: bool,
    /// Quantas alterações já houve (a cópia de recuperação sabe até onde foi).
    pub changes: u64,
    /// Aberto de um `.xlsb`: o que o arquivo tem e não é lido (salvar gera um
    /// `.xlsx`/`.xlsm` sem isso).
    pub binary: Option<String>,
    /// Recálculo em segundo plano (F4).
    pub calc: crate::calc::Calc,
    /// Tabelas dinâmicas (registros lidos sob demanda).
    pub pivots: crate::pivot::PivotState,
    /// Módulos VBA (lidos na primeira macro) e se o usuário deixou executar.
    pub macros: Option<Vec<duck_vba::Module>>,
    pub macros_ok: bool,
    /// Proteção da pasta, e se mudou desde o último salvamento.
    pub protection: Option<duck_core::BookProtection>,
    pub protection_changed: bool,
    /// Abas que as macros podem mudar mesmo protegidas (`UserInterfaceOnly`).
    pub macro_free: std::collections::HashSet<u32>,
}

/// Aviso de um `.xlsb` aberto, para a barra de status.
fn binary_note(left_out: &str) -> String {
    let mut s = String::from(" · .xlsb: salvar gera .xlsx ou .xlsm");
    if !left_out.is_empty() {
        s.push_str(&format!(" · não lidos: {left_out}"));
    }
    s
}

impl Doc {
    pub fn slot(&self) -> &Slot {
        &self.sheets[self.active]
    }

    /// O arquivo de origem, para ler mais partes dele.
    pub fn book(&self) -> Result<duck_xlsx::Book, duck_xlsx::Error> {
        match &self.source {
            Some(s) => s.book(),
            None => duck_xlsx::Book::open(&self.path),
        }
    }

    /// Aba ativa e a exibição dela, se já tiver dados.
    pub fn current(&self) -> Option<(&Arc<Sheet>, &View)> {
        let s = &self.sheets[self.active];
        Some((s.data.as_ref()?, s.view.as_ref()?))
    }

    pub fn view_mut(&mut self) -> Option<&mut View> {
        self.sheets[self.active].view.as_mut()
    }
}

/// Largura de coluna em caracteres (com a margem) → pixels, como o Excel.
pub fn col_px(width: f32, mdw: f32) -> u32 {
    if width <= 0.0 {
        return 0;
    }
    (((256.0 * width + (128.0 / mdw).trunc()) / 256.0) * mdw).trunc().max(0.0) as u32
}

/// Altura em pontos → pixels a 96 DPI.
pub fn row_px(pt: f32) -> u32 {
    (pt * 96.0 / 72.0).round().max(0.0) as u32
}

pub(crate) fn build_view(sheet: &Sheet, mdw: f32, prev: Option<View>, opts_zoom: Option<u16>) -> View {
    let l = &sheet.layout;
    let sv = l.view.unwrap_or_default();
    let dim = l.dimension;
    let used_rows = sheet.rows().max(dim.map_or(0, |d| d.last_row + 1));
    let used_cols = sheet.cols().max(dim.map_or(0, |d| d.last_col + 1));
    let min = prev.as_ref().map_or((1000, 100), |p| p.min);
    let nrows = (used_rows + 200).clamp(min.0.min(MAX_ROWS), MAX_ROWS);
    // Colunas bastantes para encher a janela também com o zoom reduzido.
    let ncols = (used_cols + 12).clamp(min.1.min(MAX_COLS), MAX_COLS);

    let default_row = row_px(l.default_row_height.unwrap_or(15.0)).max(1);
    let rows = Axis::new(
        nrows,
        default_row,
        l.rows.iter().map(|r| (r.row, if r.hidden { 0 } else { r.height.map_or(default_row, row_px) })),
    );
    let default_col = match l.default_col_width {
        Some(w) => col_px(w, mdw),
        None => ((l.base_col_width.unwrap_or(8.0) + 0.43) * mdw).round() as u32 + 5,
    }
    .max(1);
    let mut col_ov = Vec::new();
    let mut col_styles = Vec::new();
    for c in &l.cols {
        let size = if c.hidden { 0 } else { c.width.map_or(default_col, |w| col_px(w, mdw)) };
        let last = c.last.min(ncols.saturating_sub(1));
        for i in c.first..=last {
            col_ov.push((i, size));
        }
        if c.style != 0 {
            col_styles.push((c.first, c.last, c.style));
        }
    }
    let cols = Axis::new(ncols, default_col, col_ov);
    let pane = l.pane.unwrap_or_default();
    let frozen_rows = pane.frozen_rows.min(nrows - 1);
    let frozen_cols = pane.frozen_cols.min(ncols - 1);
    let merges = Merges::new(l.merges.clone());
    let row_styles = l.rows.iter().filter_map(|r| Some((r.row, r.style?))).filter(|s| s.1 != 0).collect();

    let mut v = View {
        frozen_rows,
        frozen_cols,
        scroll_x: cols.offset(frozen_cols) as f64,
        scroll_y: rows.offset(frozen_rows) as f64,
        cur: (0, 0),
        anchor: (0, 0),
        extra: Vec::new(),
        extra_anchor: (0, 0),
        zoom: opts_zoom.unwrap_or(sv.zoom).clamp(10, 400),
        merges,
        row_groups: l.outline.rows.groups(),
        col_groups: l.outline.cols.groups(),
        outline: l.outline.clone(),
        grid: sv.show_grid_lines,
        headers: sv.show_headers,
        col_styles,
        row_styles,
        cf: l.cond.iter().map(|_| duck_core::CfState::default()).collect(),
        resized_cols: Vec::new(),
        resized_rows: Vec::new(),
        min,
        rows,
        cols,
    };
    if let Some((r, c)) = sv.top_left {
        v.scroll_y = v.scroll_y.max(v.rows.offset(r.min(nrows - 1)) as f64);
        v.scroll_x = v.scroll_x.max(v.cols.offset(c.min(ncols - 1)) as f64);
    }
    if let Some(cell) = sv.active_cell {
        v.cur = (cell.0.min(nrows - 1), cell.1.min(ncols - 1));
        v.anchor = v.cur;
    }
    if let Some(p) = prev {
        // A aba inteira chegou depois da prévia: mantém o que o usuário fez.
        v.scroll_x = p.scroll_x;
        v.scroll_y = p.scroll_y;
        v.cur = p.cur;
        v.anchor = p.anchor;
        v.extra = p.extra;
        v.extra_anchor = p.extra_anchor;
        v.zoom = p.zoom;
        for (c, s) in p.resized_cols {
            v.resize_col(c, s);
        }
        for (r, s) in p.resized_rows {
            v.resize_row(r, s);
        }
    }
    v
}

// ---------------------------------------------------------------------------
// Geometria do frame (pixels físicos)
// ---------------------------------------------------------------------------

/// Um dos quatro painéis da grade (painéis congelados).
#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub rect: Rect,
    /// Posição lógica (100%) do canto superior esquerdo.
    pub ox: f64,
    pub oy: f64,
    pub row_start: u32,
    pub row_end: u32,
    pub col_start: u32,
    pub col_end: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Geom {
    /// Escala da interface (DPI) e da grade (DPI × zoom).
    pub s: f32,
    pub g: f32,
    pub w: f32,
    /// Barra de ferramentas (formatação).
    pub toolbar: Rect,
    pub fbar: Rect,
    pub namebox: Rect,
    pub formula: Rect,
    pub colhdr: Rect,
    pub rowhdr: Rect,
    pub corner: Rect,
    pub grid: Rect,
    pub vscroll: Rect,
    pub hscroll: Rect,
    pub tabs: Rect,
    pub status: Rect,
    /// Tamanho dos painéis congelados na tela.
    pub fw: f32,
    pub fh: f32,
    /// Painéis: congelado, topo, esquerda, principal.
    pub quads: [Quad; 4],
}

impl Geom {
    /// Painel e célula sob o ponto (x, y), se estiver na grade.
    pub fn hit(&self, v: &View, x: f32, y: f32) -> Option<(u32, u32)> {
        let q = self.quads.iter().find(|q| q.rect.contains(x, y) && !q.rect.is_empty())?;
        Some(self.cell_in(v, q, x, y))
    }

    pub fn cell_in(&self, v: &View, q: &Quad, x: f32, y: f32) -> (u32, u32) {
        let r = v
            .rows
            .index_at((q.oy + ((y - q.rect.y0) / self.g) as f64) as i64)
            .clamp(q.row_start, q.row_end - 1);
        let c = v
            .cols
            .index_at((q.ox + ((x - q.rect.x0) / self.g) as f64) as i64)
            .clamp(q.col_start, q.col_end - 1);
        (r, c)
    }

    /// Largura e altura lógicas (100%) visíveis no painel principal.
    pub fn main_view(&self) -> (f64, f64) {
        let q = &self.quads[3].rect;
        ((q.w() / self.g).max(1.0) as f64, (q.h() / self.g).max(1.0) as f64)
    }
}

// ---------------------------------------------------------------------------
// O app
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum Drag {
    Select,
    Cols,
    Rows,
    VThumb {
        grab: f32,
    },
    HThumb {
        grab: f32,
    },
    /// Apontando um intervalo para a fórmula em edição.
    Point,
    /// Arrastando a alça de preenchimento a partir de `src`.
    Fill {
        src: CellRange,
    },
    /// Arrastando a borda da seleção (`grab`: célula pega, dentro do bloco).
    MoveCells {
        src: CellRange,
        grab: (u32, u32),
    },
    /// Arrastando a guia da aba `sheet` (x do clique).
    Tab {
        sheet: usize,
        x: f32,
    },
    /// Barra deslizante do zoom.
    Zoom,
    /// Arrastando (ou redimensionando por `grip`) o objeto da âncora `slot`,
    /// que estava em `from`. `keep`: um eixo só, ou a mesma proporção (Shift).
    Object {
        slot: u32,
        grip: crate::objects::Grip,
        from: [f64; 4],
        x: f32,
        y: f32,
        keep: bool,
    },
    /// Arrastando a borda direita da coluna `col` (tamanho inicial e x do mouse).
    ResizeCol {
        col: u32,
        size: u32,
        x: f32,
    },
    ResizeRow {
        row: u32,
        size: u32,
        y: f32,
    },
}

/// Distância (pixels de interface) da borda do cabeçalho que pega o redimensionamento.
const RESIZE_GRAB: f32 = 6.0;

pub struct Find {
    pub input: TextInput,
    pub job: Option<SearchJob>,
    pub result: Option<SearchResult>,
    /// Consulta do resultado atual.
    pub done_query: String,
    pub current: usize,
    pub focused: bool,
}

pub struct Bench {
    pub phase: usize,
    pub frame: usize,
    pub warmup: usize,
    pub rng: u64,
    pub cpu_ms: Vec<f64>,
    /// Só a montagem do frame (sem o envio para a GPU).
    pub build_ms: Vec<f64>,
    pub instances: usize,
    pub phase_start: Instant,
    pub report: String,
}

pub const BENCH_PHASES: [(&str, usize); 3] = [
    ("scroll vertical (40 px/frame)", 1500),
    ("saltos aleatórios", 300),
    ("scroll horizontal (30 px/frame)", 400),
];

pub struct SheetsApp {
    pub t0: Instant,
    pub opts: Opts,
    pub window: Option<Window>,
    pub gfx: Option<Gfx>,
    /// GPU sendo aberta desde o início do processo (ver `Gfx::start_early`).
    gpu_early: Option<duck_gfx::EarlyGpu>,
    pub text: Option<TextSystem>,
    fonts_loading: Option<std::thread::JoinHandle<TextSystem>>,
    /// Resto das fontes do sistema, lido depois da primeira tela.
    font_scan: Option<std::sync::mpsc::Receiver<FontSet>>,
    pub(crate) waker: Option<duck_platform::Waker>,
    pub scale: f32,
    pub doc: Option<Doc>,
    pub loader: Option<Loader>,
    pub instances: Vec<Instance>,
    pub namebox: TextInput,
    pub namebox_focus: bool,
    pub find: Option<Find>,
    pub tabbar: TabBar,
    /// Aba que a barra já trouxe para a vista.
    pub tab_revealed: Option<usize>,
    /// Guia sendo renomeada, a do menu aberto e o maior `sheetId` já usado.
    pub tab_edit: Option<crate::tabs::TabEdit>,
    pub tab_menu: Option<usize>,
    pub last_tab_id: u32,
    pub drag: Option<Drag>,
    /// Onde o arraste da alça ou da borda vai parar.
    pub drag_area: Option<CellRange>,
    /// Objeto selecionado (índice em `layout.objects` da aba ativa) e, no
    /// arraste, onde ele está.
    pub object: Option<usize>,
    pub moving: Option<crate::objects::Moving>,
    pub mouse: (f32, f32),
    pub message: String,
    /// Estilo de texto de cada fonte do arquivo no zoom atual (chave: tamanho base).
    pub font_styles: Vec<TextStyle>,
    pub font_key: (u32, usize),
    pub stats: Option<(crate::cells::StatsKey, crate::cells::SelStats)>,
    /// Pool próprio para trabalho pedido pela janela (estatísticas da
    /// seleção): o pool global pode estar ocupado com a leitura, e a janela
    /// não pode esperar na fila dele.
    pub(crate) ui_pool: Option<rayon::ThreadPool>,
    pub timeline: Vec<(&'static str, Duration)>,
    pub first_content: Option<Duration>,
    pub recent_cpu: Vec<f64>,
    /// Restos da roda e da pinça que ainda não viraram um passo de zoom.
    wheel_zoom: f64,
    pinch: f64,
    pub bench: Option<Bench>,
    pub screenshot_pending: bool,
    pub(crate) error: Option<String>,
    pub buf: String,
    pub buf2: String,
    /// `DUCK_TRACE=1`: imprime eventos e frames dos primeiros segundos.
    trace: bool,
    /// Data de hoje (série do Excel), para a formatação condicional por data.
    pub today: f64,
    /// Destino do hiperlink sob o mouse (barra de status).
    pub hover_link: Option<String>,
    /// Borda de cabeçalho sob o mouse (dá para arrastar).
    pub hover_edge: Option<Edge>,
    /// Célula sendo editada (F3).
    pub editor: Option<crate::edit::Editor>,
    /// Caixa de diálogo aberta.
    pub form: Option<crate::forms::Form>,
    /// Texto do clipboard quando "Colar especial" abriu.
    pub paste_source: String,
    /// Última busca de "Substituir" e as áreas escolhidas ao abrir.
    pub replace_last: crate::replace::Query,
    pub replace_areas: Vec<CellRange>,
    /// "Formatar células": o que foi mexido e as listas de exemplos.
    pub format_state: crate::fmtcells::State,
    pub format_lists: [Vec<String>; 3],
    /// Pincel de formatação ligado.
    pub painter: Option<crate::paste::Painter>,
    /// Salvamento em andamento.
    pub saving: Option<crate::edit::SaveJob>,
    /// Última cópia feita no app (fórmulas e estilos para colar).
    pub clip: Option<crate::paste::Clip>,
    /// O que está copiado ou recortado (borda tracejada).
    pub marked: Option<crate::paste::Marked>,
    /// Menu aberto (lista, paleta ou clique direito).
    pub popup: Option<crate::menu::Popup>,
    /// Caixa de diálogo aberta (largura, altura).
    pub prompt: Option<crate::menu::Prompt>,
    /// Últimas cores de texto e de fundo escolhidas (RGB).
    pub colors: (u32, u32),
    /// Botão da faixa sob o mouse e a dica dele.
    pub hover_act: Option<(crate::ribbon::Act, &'static str)>,
    /// Aba da faixa de opções.
    pub ribbon_tab: crate::ribbon::Tab,
    /// Arquivos abertos recentemente (menu Arquivo).
    pub recent: Vec<PathBuf>,
    /// Painel do filtro aberto.
    pub filter_panel: Option<crate::filter::Panel>,
    /// Caixa de diálogo com campos (validação, nota, proteção, avisos).
    pub dialog: Option<crate::dialog::Dialog>,
    /// Lista suspensa da validação aberta.
    pub dv_list: Option<crate::valid::ListBox>,
    /// Célula com nota sob o mouse, e se todas as notas estão à vista.
    pub hover_note: Option<(u32, u32)>,
    pub show_notes: bool,
    /// Comandos que são consequência de outro, já permitido: a proteção não
    /// confere de novo.
    pub unchecked: bool,
    /// Arquivo temporário da planilha nova (salvar pede o lugar).
    pub untitled_path: Option<PathBuf>,
    pub new_count: u32,
    /// Imagens das abas no atlas da GPU.
    pub images: crate::images::Images,
    /// Configurações e o esquema de cores do sistema.
    pub prefs: Prefs,
    pub system: ColorScheme,
    /// Cópias de recuperação.
    pub recovery: crate::recovery::Recovery,
    /// Atualização automática.
    pub updater: crate::update::Updater,
    /// O mouse estava sobre a lista de sugestões.
    complete_hover: bool,
}

impl SheetsApp {
    pub fn new(t0: Instant, opts: Opts) -> Self {
        let gpu_early = Gfx::start_early();
        Self::build(t0, opts, gpu_early)
    }

    /// Sem abrir a GPU (`None`): também serve para os testes.
    pub(crate) fn build(t0: Instant, opts: Opts, gpu_early: Option<duck_gfx::EarlyGpu>) -> Self {
        let fonts_loading = std::thread::spawn(|| TextSystem::new_fast(UI_FONT_PX, ATLAS));
        let bench = opts.bench.then(|| Bench {
            phase: 0,
            frame: 0,
            warmup: 30,
            rng: 42,
            cpu_ms: Vec::new(),
            build_ms: Vec::new(),
            instances: 0,
            phase_start: Instant::now(),
            report: String::new(),
        });
        let screenshot_pending = opts.screenshot.is_some();
        // Medições e screenshots não dependem da preferência de quem roda.
        let mut prefs = if opts.bench || screenshot_pending || opts.save_to.is_some() {
            Prefs { mode: Mode::Light, ..Default::default() }
        } else {
            crate::settings::file().map(|f| Prefs::load(&f)).unwrap_or_default()
        };
        // Sem ninguém olhando não há cópias de recuperação (só num teste delas).
        let alone = opts.bench || screenshot_pending || opts.save_to.is_some();
        let asked = std::env::var_os("DUCK_RECUPERACAO").is_some();
        let copies = if alone && !asked { None } else { crate::recovery::dir() };
        prefs.mode = opts.theme.unwrap_or(prefs.mode);
        prefs.dark_sheet = opts.dark_sheet.unwrap_or(prefs.dark_sheet);
        Self {
            t0,
            opts,
            window: None,
            gfx: None,
            gpu_early,
            text: None,
            fonts_loading: Some(fonts_loading),
            font_scan: None,
            waker: None,
            scale: 1.0,
            doc: None,
            loader: None,
            instances: Vec::with_capacity(64 * 1024),
            namebox: TextInput::new(),
            namebox_focus: false,
            find: None,
            tabbar: TabBar::default(),
            tab_revealed: None,
            tab_edit: None,
            tab_menu: None,
            last_tab_id: 0,
            drag: None,
            drag_area: None,
            object: None,
            moving: None,
            mouse: (0.0, 0.0),
            message: String::new(),
            font_styles: Vec::new(),
            font_key: (0, 0),
            stats: None,
            ui_pool: None,
            timeline: Vec::new(),
            first_content: None,
            recent_cpu: Vec::with_capacity(120),
            wheel_zoom: 0.0,
            pinch: 0.0,
            bench,
            screenshot_pending,
            error: None,
            buf: String::new(),
            buf2: String::new(),
            trace: std::env::var_os("DUCK_TRACE").is_some(),
            today: duck_core::cond::today_serial(),
            hover_link: None,
            hover_edge: None,
            editor: None,
            form: None,
            paste_source: String::new(),
            replace_last: Default::default(),
            replace_areas: Vec::new(),
            format_state: Default::default(),
            format_lists: Default::default(),
            painter: None,
            saving: None,
            clip: None,
            marked: None,
            popup: None,
            prompt: None,
            colors: (0xFF0000, 0xFFFF00),
            hover_act: None,
            ribbon_tab: Default::default(),
            recent: crate::actions::load_recent(),
            untitled_path: None,
            new_count: 0,
            filter_panel: None,
            dialog: None,
            dv_list: None,
            hover_note: None,
            show_notes: false,
            unchecked: false,
            images: Default::default(),
            prefs,
            system: ColorScheme::Light,
            recovery: crate::recovery::Recovery::new(copies),
            updater: Default::default(),
            complete_hover: false,
        }
    }

    /// Cores em uso.
    pub fn theme(&self) -> &'static Theme {
        self.prefs.mode.theme(self.system)
    }

    /// As da planilha (células e o que fica por cima delas): as mesmas, ou as
    /// claras com a interface escura.
    pub fn sheet_theme(&self) -> &'static Theme {
        if self.prefs.dark_sheet { self.theme() } else { &LIGHT }
    }

    /// Medição ou teste pela linha de comando: ninguém para responder.
    pub(crate) fn unattended(&self) -> bool {
        self.opts.bench || self.opts.screenshot.is_some() || self.opts.save_to.is_some()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn mark(&mut self, what: &'static str) {
        self.note(what, self.t0.elapsed());
    }

    /// Registra um evento da linha do tempo (com `DUCK_TRACE`, imprime também
    /// a memória do processo nesse momento).
    fn note(&mut self, what: &'static str, at: Duration) {
        self.timeline.push((what, at));
        if self.trace {
            let (now, peak) = crate::draw::process_memory();
            eprintln!(
                "{:>8.1} ms  {what}: memória {:.0} MB (privada {:.0} MB, pico {:.0} MB)",
                at.as_secs_f64() * 1000.0,
                now as f64 / 1048576.0,
                crate::draw::private_memory() as f64 / 1048576.0,
                peak as f64 / 1048576.0
            );
        }
    }

    fn redraw_later(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    // --- abrir e carregar ---------------------------------------------------

    pub fn open(&mut self, path: PathBuf, waker: duck_platform::Waker) {
        if !self.confirm_discard() {
            return;
        }
        // A de antes foi salva ou descartada.
        if let Some(d) = &mut self.doc {
            d.calc.stop_snapshot();
        }
        self.recovery.forget();
        self.editor = None;
        self.form = None;
        self.painter = None;
        self.tab_edit = None;
        self.clip = None;
        self.marked = None;
        self.drag = None;
        self.drag_area = None;
        self.object = None;
        self.moving = None;
        if let Some(l) = self.loader.take() {
            l.cancel();
        }
        let name = self.file_label(&path);
        if let Some(w) = &self.window {
            w.set_title(&format!("{name} — Duck Sheets"));
        }
        self.doc = None;
        self.find = None;
        self.stats = None;
        self.message = format!("Abrindo {name}…");
        // Para a primeira abertura, o relógio começa no início do processo;
        // depois, quando o usuário pede outro arquivo.
        let t0 = if self.timeline.iter().any(|t| t.0 == "arquivo aberto") { Instant::now() } else { self.t0 };
        // `--sheet N` vale só para o primeiro arquivo.
        let want = self.opts.sheet.take().map(|n| n.max(1) - 1);
        self.loader = Some(Loader::start(path, waker, t0, want));
    }

    /// O sistema pediu para abrir `path` com o app já rodando: aqui, se só há
    /// uma planilha nova intocada; senão, noutra janela.
    fn open_elsewhere(&mut self, path: PathBuf, waker: duck_platform::Waker) {
        // O macOS também repete o que veio na linha de comando.
        let now = self.doc.as_ref().map(|d| &d.path).or(self.loader.as_ref().map(|l| &l.path));
        let full = |p: &PathBuf| p.canonicalize().ok();
        let same = now.is_some_and(|p| *p == path || (full(p).is_some() && full(p) == full(&path)));
        if same || self.unattended() {
            return;
        }
        let blank = self.untitled_path.as_ref() == now && self.doc.as_ref().is_none_or(|d| !d.dirty);
        if blank {
            return self.open(path, waker);
        }
        let exe = std::env::current_exe();
        if let Err(e) = exe.and_then(|x| std::process::Command::new(x).arg(&path).spawn()) {
            duck_platform::message_box("Não foi possível abrir outra janela", &e.to_string());
        }
    }

    /// Mensagens da leitura em segundo plano.
    fn poll_loader(&mut self) -> bool {
        let Some(loader) = &self.loader else { return false };
        let msgs = drain(loader);
        if msgs.is_empty() {
            return false;
        }
        let path = loader.path.clone();
        for m in msgs {
            match m {
                Msg::Opened { source, sheets, active, styles, strings, date1904, extras, binary, at } => {
                    self.images = Default::default();
                    // O que estava aberto era do outro arquivo.
                    self.dialog.take_if(|d| !d.stays());
                    self.dv_list = None;
                    self.hover_note = None;
                    self.note("arquivo aberto", at);
                    self.remember_recent(&path);
                    let looks = Arc::new(Looks::new(&styles));
                    let mdw = self.measure_mdw(&looks);
                    self.doc = Some(Doc {
                        path: path.clone(),
                        source: Some(source),
                        sheets: sheets
                            .into_iter()
                            .map(|info| Slot {
                                origin: duck_xlsx::TabOrigin::File(info.part.clone()),
                                info,
                                data: None,
                                preview: false,
                                view: None,
                            })
                            .collect(),
                        active,
                        styles,
                        looks,
                        strings,
                        date1904,
                        mdw,
                        loaded: 0,
                        finished: None,
                        history: Default::default(),
                        dirty: false,
                        changes: 0,
                        binary,
                        protection: extras.protection.clone(),
                        protection_changed: false,
                        macro_free: Default::default(),
                        calc: crate::calc::Calc::new(extras),
                        pivots: Default::default(),
                        macros: None,
                        macros_ok: false,
                    });
                    self.font_key = (0, 0);
                }
                Msg::Preview { index, sheet, at } => {
                    self.note("prévia da aba ativa", at);
                    self.install_sheet(index, sheet, true);
                }
                Msg::Sheet { index, sheet, at } => {
                    if self.doc.as_ref().is_some_and(|d| d.active == index) {
                        self.note("aba ativa completa", at);
                    }
                    self.install_sheet(index, sheet, false);
                }
                Msg::Finished { at, unread } => {
                    self.note("todas as abas", at);
                    if let Some(d) = &mut self.doc {
                        d.finished = Some(at);
                    }
                    self.message = format!("Pronto em {} ms", at.as_millis());
                    if let Some(out) = self.doc.as_mut().and_then(|d| d.binary.as_mut()) {
                        if unread > 0 {
                            let sep = if out.is_empty() { "" } else { ", " };
                            out.push_str(&format!("{sep}fórmulas (ficam os valores): {unread}"));
                        }
                        self.message.push_str(&binary_note(out));
                    }
                    self.refresh_table_slicers();
                    self.refresh_charts();
                    self.load_images();
                    self.init_spills();
                    self.recovery_loaded();
                    self.scripted_edits();
                    self.calc_kick();
                }
                Msg::Failed(e) => {
                    self.start_font_scan();
                    self.message = format!("Erro: {e}");
                    if self.doc.is_none() {
                        if self.unattended() {
                            self.error = Some(e);
                        } else {
                            duck_platform::message_box("Não foi possível abrir o arquivo", &e);
                            // Uma cópia de recuperação que não abre fica na pasta.
                            if self.recovery.is_adopted(&path) {
                                self.recovery.adopted = None;
                            }
                            // Nada aberto: volta para uma planilha em branco.
                            if self.untitled_path.as_deref() != Some(path.as_path()) {
                                self.new_file();
                                return true;
                            }
                        }
                    }
                }
            }
        }
        true
    }

    fn install_sheet(&mut self, index: usize, sheet: Sheet, preview: bool) {
        let opts_cell = self.opts.cell.clone();
        let zoom = self.opts.zoom;
        let Some(doc) = &mut self.doc else { return };
        let mdw = doc.mdw;
        let Some(slot) = doc.sheets.get_mut(index) else { return };
        if preview && slot.data.is_some() {
            return;
        }
        let prev = slot.view.take();
        let fresh = prev.is_none();
        let mut view = build_view(&sheet, mdw, prev, zoom);
        if fresh
            && index == doc.active
            && let Some((r, c)) = opts_cell.as_deref().and_then(parse_cell)
        {
            view.cur = (r.min(view.rows.len() - 1), c.min(view.cols.len() - 1));
            view.anchor = view.cur;
            view.scroll_y = view.rows.offset(view.cur.0) as f64 - 100.0;
            view.scroll_x = view.cols.offset(view.cur.1) as f64 - 100.0;
        }
        slot.view = Some(view);
        slot.data = Some(Arc::new(sheet));
        slot.preview = preview;
        if !preview {
            doc.loaded += 1;
        }
        if index == doc.active {
            self.stats = None;
            if let Some(f) = &mut self.find {
                // A busca era na prévia: refaz na aba inteira.
                f.done_query.clear();
            }
        }
        let n = doc.sheets.len();
        if doc.finished.is_none() {
            self.message = format!("Carregando: {} de {} abas", doc.loaded, n);
        }
    }

    /// Lê o resto das fontes do sistema numa thread. Só depois da primeira
    /// tela: com o cache de disco frio, leva até 0,6 s e competiria com ela.
    fn start_font_scan(&mut self) {
        let complete = self.text.as_ref().is_none_or(|t| t.fonts_complete());
        if complete || self.font_scan.is_some() {
            return;
        }
        let Some(waker) = self.waker.clone() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("duck-fonts".into())
            .spawn(move || {
                let _ = tx.send(duck_text::scan_system_fonts());
                waker.wake();
            })
            .expect("thread de fontes");
        self.font_scan = Some(rx);
    }

    /// Junta o resto das fontes, se já chegou. `true` = redesenhar.
    fn poll_fonts(&mut self) -> bool {
        let Some(rx) = &self.font_scan else { return false };
        let Ok(set) = rx.try_recv() else { return false };
        self.font_scan = None;
        self.mark("todas as fontes");
        let Some(text) = &mut self.text else { return false };
        if text.add_fonts(set) {
            // Uma família do arquivo passou a existir: a largura do dígito
            // (unidade das colunas) e os estilos podem mudar.
            self.font_key = (0, 0);
            let looks = self.doc.as_ref().map(|d| d.looks.clone());
            if let Some(looks) = looks {
                let mdw = self.measure_mdw(&looks);
                if let Some(doc) = &mut self.doc
                    && mdw != doc.mdw
                {
                    doc.mdw = mdw;
                    for slot in &mut doc.sheets {
                        if let Some(data) = &slot.data {
                            slot.view = Some(build_view(data, mdw, slot.view.take(), None));
                        }
                    }
                }
            }
        }
        true
    }

    /// Largura do dígito na fonte padrão do arquivo a 100% (96 DPI).
    fn measure_mdw(&mut self, looks: &Looks) -> f32 {
        let Some(text) = &mut self.text else { return 7.0 };
        let f = &looks.fonts[0];
        let st = text.style(&f.name, f.pt * 96.0 / 72.0, false, false);
        let w = (0..10).map(|d| text.measure_styled(&d.to_string(), st)).fold(0.0f32, f32::max);
        w.round().max(4.0)
    }

    // --- geometria ----------------------------------------------------------

    pub fn geom(&mut self) -> Geom {
        let (w, h) =
            self.gfx.as_ref().map(|g| g.size()).unwrap_or(if cfg!(test) { (1200, 800) } else { (1, 1) });
        let (w, h) = (w as f32, h as f32);
        let s = self.scale;
        let zoom = self.doc.as_ref().and_then(|d| d.current()).map_or(100, |(_, v)| v.zoom);
        let g = s * zoom as f32 / 100.0;
        let tb_h = (TOOLBAR_H * s).round();
        let fbar_h = (FBAR_H * s).round();
        let status_h = (STATUS_H * s).round();
        let tabs_h = (TABS_H * s).round();
        let sb = (SCROLLBAR * s).round();
        let show_headers = self.doc.as_ref().and_then(|d| d.current()).is_none_or(|(_, v)| v.headers);
        let hh = if show_headers { (HEADER_H * g).round().max((12.0 * s).round()) } else { 0.0 };
        // Largura do cabeçalho de linhas: o maior número de linha da aba.
        // Faixas dos grupos, antes dos cabeçalhos.
        let (gut_x, gut_y) = self
            .doc
            .as_ref()
            .and_then(|d| d.current())
            .filter(|(_, v)| v.headers)
            .map_or((0.0, 0.0), |(_, v)| crate::outline::gutters(&v.outline, s));
        let rw = gut_x
            + if show_headers {
                let nrows = self.doc.as_ref().and_then(|d| d.current()).map_or(1000, |(_, v)| v.rows.len());
                let style = self.header_style(g);
                let text = self.text.as_mut().expect("texto");
                let digits = nrows.to_string().len().max(3);
                let w = text.measure_styled(&"0".repeat(digits), style);
                (w + 12.0 * g).ceil()
            } else {
                0.0
            };
        let toolbar = Rect::new(0.0, 0.0, w, tb_h);
        let fbar = Rect::new(0.0, tb_h, w, tb_h + fbar_h);
        let namebox = Rect::new(0.0, fbar.y0, (NAMEBOX_W * s).round(), fbar.y1);
        let formula = Rect::new(namebox.x1 + (34.0 * s).round(), fbar.y0, w, fbar.y1);
        let gy0 = fbar.y1 + gut_y + hh;
        let status = Rect::new(0.0, h - status_h, w, h);
        let tabs_y0 = status.y0 - tabs_h;
        let grid = Rect::new(rw, gy0, (w - sb).max(rw + 1.0), tabs_y0.max(gy0 + 1.0));
        let colhdr = Rect::new(rw, fbar.y1 + gut_y, grid.x1, gy0);
        let rowhdr = Rect::new(gut_x, gy0, rw, grid.y1);
        let corner = Rect::new(gut_x, fbar.y1 + gut_y, rw, gy0);
        let vscroll = Rect::new(grid.x1, gy0, w, grid.y1);
        let tabs_w = (w * 0.6).round();
        let tabs = Rect::new(0.0, tabs_y0, tabs_w, status.y0);
        let hscroll = Rect::new(
            tabs_w,
            tabs_y0 + ((tabs_h - sb) / 2.0).round(),
            w,
            tabs_y0 + ((tabs_h + sb) / 2.0).round(),
        );

        let (fw, fh, quads) = match self.doc.as_ref().and_then(|d| d.current()) {
            Some((_, v)) => {
                let fw = ((v.cols.offset(v.frozen_cols) as f32) * g).round().min(grid.w() * 0.9);
                let fh = ((v.rows.offset(v.frozen_rows) as f32) * g).round().min(grid.h() * 0.9);
                let (x1, y1) = (grid.x0 + fw, grid.y0 + fh);
                let (nr, nc, fr, fc) = (v.rows.len(), v.cols.len(), v.frozen_rows, v.frozen_cols);
                let q = |rect: Rect, ox: f64, oy: f64, rs: u32, re: u32, cs: u32, ce: u32| Quad {
                    rect,
                    ox,
                    oy,
                    row_start: rs,
                    row_end: re.max(rs + 1),
                    col_start: cs,
                    col_end: ce.max(cs + 1),
                };
                (
                    fw,
                    fh,
                    [
                        q(Rect::new(grid.x0, grid.y0, x1, y1), 0.0, 0.0, 0, fr, 0, fc),
                        q(Rect::new(x1, grid.y0, grid.x1, y1), v.scroll_x, 0.0, 0, fr, fc, nc),
                        q(Rect::new(grid.x0, y1, x1, grid.y1), 0.0, v.scroll_y, fr, nr, 0, fc),
                        q(Rect::new(x1, y1, grid.x1, grid.y1), v.scroll_x, v.scroll_y, fr, nr, fc, nc),
                    ],
                )
            }
            None => {
                let empty = Quad {
                    rect: Rect::default(),
                    ox: 0.0,
                    oy: 0.0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 1,
                };
                (0.0, 0.0, [empty, empty, empty, Quad { rect: grid, ..empty }])
            }
        };
        Geom {
            s,
            g,
            w,
            toolbar,
            fbar,
            namebox,
            formula,
            colhdr,
            rowhdr,
            corner,
            grid,
            vscroll,
            hscroll,
            tabs,
            status,
            fw,
            fh,
            quads,
        }
    }

    /// Estilo dos cabeçalhos de linha/coluna: a fonte padrão do arquivo.
    pub fn header_style(&mut self, g: f32) -> TextStyle {
        let text = self.text.as_mut().expect("texto");
        match &self.doc {
            Some(d) => {
                let f = &d.looks.fonts[0];
                text.style(&f.name, f.pt * 96.0 / 72.0 * g * 0.95, false, false)
            }
            None => text.default_style().with_px(UI_FONT_PX * g),
        }
    }

    /// Recalcula os estilos de texto das fontes do arquivo para a escala `g`.
    pub fn ensure_font_styles(&mut self, g: f32) {
        let Some(doc) = &self.doc else { return };
        let key = ((g * 1000.0) as u32, Arc::as_ptr(&doc.looks) as usize);
        if self.font_key == key {
            return;
        }
        let text = self.text.as_mut().expect("texto");
        if self.font_key.1 == key.1 {
            // Mesmo arquivo, outro zoom: os tamanhos antigos não voltam tão cedo.
            text.clear_text_caches();
        }
        self.font_styles = doc
            .looks
            .fonts
            .iter()
            .map(|f| text.style(&f.name, f.pt * 96.0 / 72.0 * g, f.bold, f.italic))
            .collect();
        self.font_key = key;
    }

    pub fn clamp_scroll(&mut self) {
        let geom = self.geom();
        let (vw, vh) = geom.main_view();
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return };
        let min_x = v.cols.offset(v.frozen_cols) as f64;
        let min_y = v.rows.offset(v.frozen_rows) as f64;
        let max_x = (v.cols.total() as f64 - vw).max(min_x);
        let max_y = (v.rows.total() as f64 - vh).max(min_y);
        v.scroll_x = v.scroll_x.clamp(min_x, max_x);
        v.scroll_y = v.scroll_y.clamp(min_y, max_y);
    }

    /// Rola para a célula ativa ficar visível (no painel principal).
    pub fn ensure_visible(&mut self) {
        if let Some((r, c)) = self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| v.cur) {
            self.reveal(r, c);
        }
    }

    /// Rola para a célula ficar visível (no painel principal).
    pub(crate) fn reveal(&mut self, r: u32, c: u32) {
        let geom = self.geom();
        let (vw, vh) = geom.main_view();
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return };
        let span =
            v.merges.at(r, c).unwrap_or(CellRange { first_row: r, first_col: c, last_row: r, last_col: c });
        if r >= v.frozen_rows {
            let (top, bottom) =
                (v.rows.offset(span.first_row) as f64, v.rows.offset(span.last_row + 1) as f64);
            if top < v.scroll_y {
                v.scroll_y = top;
            } else if bottom > v.scroll_y + vh {
                v.scroll_y = (bottom - vh).min(top);
            }
        }
        if c >= v.frozen_cols {
            let (left, right) =
                (v.cols.offset(span.first_col) as f64, v.cols.offset(span.last_col + 1) as f64);
            if left < v.scroll_x {
                v.scroll_x = left;
            } else if right > v.scroll_x + vw {
                v.scroll_x = (right - vw).min(left);
            }
        }
        self.clamp_scroll();
    }

    // --- seleção e navegação -------------------------------------------------

    pub(crate) fn move_cur(&mut self, dr: i32, dc: i32, extend: bool, jump: bool) {
        // Seleções (de linhas e colunas inteiras, por exemplo) não mudam a grade.
        if !extend
            && !jump
            && (dr > 0 || dc > 0)
            && let Some((r, c)) = self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| v.cur)
        {
            self.grow_grid(r + dr.max(0) as u32, c + dc.max(0) as u32);
        }
        let Some(doc) = &mut self.doc else { return };
        let slot = &mut doc.sheets[doc.active];
        let (Some(sheet), Some(v)) = (slot.data.as_ref(), slot.view.as_mut()) else { return };
        let (mut r, mut c) = step(sheet, v, v.cur, dr, dc, jump);
        if !extend && let Some(m) = v.merges.at(r, c) {
            (r, c) = (m.first_row, m.first_col);
        }
        // Planilha protegida: pula as células que não dá para selecionar.
        if !self.can_select(r, c) {
            match self.next_selectable((r, c), dr, dc) {
                Some(x) => (r, c) = x,
                None => return,
            }
        }
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return };
        v.cur = (r, c);
        if !extend {
            v.anchor = v.cur;
        }
        self.ensure_visible();
    }

    fn set_cur(&mut self, r: u32, c: u32, extend: bool) {
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return };
        let (r, c) = (r.min(v.rows.len() - 1), c.min(v.cols.len() - 1));
        v.cur = (r, c);
        if !extend {
            v.single();
            v.anchor = (r, c);
        }
    }

    /// Aumenta a grade se a célula fica no fim dela ou depois.
    pub(crate) fn grow_grid(&mut self, r: u32, c: u32) {
        let Some(doc) = &mut self.doc else { return };
        let (mdw, i) = (doc.mdw, doc.active);
        let slot = &mut doc.sheets[i];
        let (Some(sheet), Some(v)) = (slot.data.as_ref(), slot.view.as_ref()) else { return };
        let (r, c) = (r.min(MAX_ROWS - 1), c.min(MAX_COLS - 1));
        let (more_rows, more_cols) = (r + 1 >= v.rows.len(), c + 1 >= v.cols.len());
        if (!more_rows || v.rows.len() == MAX_ROWS) && (!more_cols || v.cols.len() == MAX_COLS) {
            return;
        }
        let Some(mut v) = slot.view.take() else { return };
        if more_rows {
            v.min.0 = r + 500;
        }
        if more_cols {
            v.min.1 = c + 50;
        }
        slot.view = Some(build_view(sheet, mdw, Some(v), None));
    }

    pub fn go_to(&mut self, r: u32, c: u32) {
        self.grow_grid(r, c);
        self.set_cur(r, c, false);
        self.ensure_visible();
    }

    fn visible_rows(&mut self) -> i32 {
        let g = self.geom();
        let d = self.doc.as_ref().and_then(|d| d.current()).map_or(20, |(_, v)| v.rows.default_size());
        ((g.main_view().1 / d as f64) as i32 - 1).max(1)
    }

    pub fn switch_sheet(&mut self, index: usize) {
        let Some(doc) = &mut self.doc else { return };
        if index >= doc.sheets.len() || doc.sheets[index].info.hidden || index == doc.active {
            return;
        }
        doc.active = index;
        self.dv_list = None;
        self.hover_note = None;
        if let Some(ed) = &mut self.editor {
            ed.away = index != ed.sheet;
        }
        self.find = self.find.take().map(|mut f| {
            f.result = None;
            f.job = None;
            f.done_query.clear();
            f
        });
        self.stats = None;
        self.namebox_focus = false;
        self.object = None;
        self.moving = None;
        self.clamp_scroll();
    }

    pub(crate) fn cycle_sheet(&mut self, forward: bool) {
        let Some(doc) = &self.doc else { return };
        let n = doc.sheets.len();
        let mut i = doc.active;
        for _ in 0..n {
            i = if forward { (i + 1) % n } else { (i + n - 1) % n };
            if !doc.sheets[i].info.hidden {
                break;
            }
        }
        self.switch_sheet(i);
    }

    pub fn set_zoom(&mut self, zoom: i32) {
        if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
            v.zoom = zoom.clamp(crate::zoombar::MIN, crate::zoombar::MAX) as u16;
        }
        self.clamp_scroll();
    }

    /// Próximo múltiplo de 10 para cima ou para baixo.
    pub(crate) fn zoom_step(&mut self, up: bool) {
        let z = self.zoom();
        self.set_zoom(if up { (z / 10 + 1) * 10 } else { ((z + 9) / 10 - 1) * 10 });
    }

    pub(crate) fn zoom(&self) -> i32 {
        self.doc.as_ref().and_then(|d| d.current()).map_or(100, |(_, v)| v.zoom as i32)
    }

    // --- copiar, estatísticas, barra de fórmulas -------------------------------

    /// Copia a seleção (texto como aparece, separado por tab) e guarda as
    /// células para colar aqui com fórmulas e estilos. `cut` = recortar.
    fn copy(&mut self, cx: &mut Context, cut: bool) {
        if let Some(out) = self.copy_text(cut) {
            cx.clipboard().set_text(&out);
        }
    }

    /// Texto da barra de fórmulas para a célula ativa.
    pub fn formula_text(&self, out: &mut String) {
        out.clear();
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r, c) = v.cur;
        if sheet.formula_hidden(&doc.styles, r, c) {
            return;
        }
        if let Some(f) = duck_calc::cell_formula(sheet, r, c) {
            out.push('=');
            out.push_str(&duck_calc::locale::to_pt(&f));
            return;
        }
        let cell = sheet.cell(r, c, &doc.strings);
        edit_text(&doc.looks, doc.looks.get(cell.style), cell.value, doc.date1904, out);
    }

    // --- busca ----------------------------------------------------------------

    pub(crate) fn open_find(&mut self) {
        let f = self.find.get_or_insert_with(|| Find {
            input: TextInput::new(),
            job: None,
            result: None,
            done_query: String::new(),
            current: 0,
            focused: true,
        });
        f.focused = true;
        f.input.select_all();
        self.namebox_focus = false;
    }

    /// Enter na busca: começa uma busca nova ou vai para o próximo resultado.
    fn find_next(&mut self, waker: duck_platform::Waker, back: bool) {
        let Some(f) = &mut self.find else { return };
        let q = f.input.text.trim().to_string();
        if q.is_empty() {
            return;
        }
        if q != f.done_query || f.result.is_none() {
            if f.job.as_ref().is_some_and(|j| j.query == q) {
                return; // já buscando
            }
            let Some(doc) = &self.doc else { return };
            let Some((sheet, _)) = doc.current() else { return };
            f.result = None;
            f.job = Some(search::start(
                &q,
                sheet.clone(),
                doc.strings.clone(),
                doc.looks.clone(),
                doc.date1904,
                waker,
            ));
            self.message = format!("Buscando “{q}”…");
            return;
        }
        let Some(res) = &f.result else { return };
        if res.hits.is_empty() {
            return;
        }
        let n = res.hits.len();
        f.current = if back { (f.current + n - 1) % n } else { (f.current + 1) % n };
        let (r, c) = res.hits[f.current];
        self.go_to(r, c);
    }

    fn poll_search(&mut self) -> bool {
        let Some(f) = &mut self.find else { return false };
        let Some(job) = &f.job else { return false };
        let Ok(res) = job.rx.try_recv() else { return false };
        f.done_query = job.query.clone();
        f.job = None;
        let cur = self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| v.cur).unwrap_or((0, 0));
        // Primeiro resultado a partir da célula ativa (em ordem de linha).
        f.current = res.hits.partition_point(|&h| h < cur) % res.hits.len().max(1);
        self.message = if res.total == 0 {
            format!("Nada encontrado para “{}” ({:.0} ms)", f.done_query, res.millis)
        } else {
            format!("{} células com “{}” ({:.0} ms)", fmt_int(res.total as u64), f.done_query, res.millis)
        };
        if self.trace {
            eprintln!("busca: {} células em {:.1} ms", res.total, res.millis);
        }
        let target = res.hits.get(f.current).copied();
        f.result = Some(res);
        if let Some((r, c)) = target {
            self.go_to(r, c);
        }
        true
    }

    // --- mouse ---------------------------------------------------------------

    fn mouse_down(&mut self, cx: &mut Context, x: f32, y: f32, mods: Modifiers, clicks: u8) {
        let g = self.geom();
        // Caixa de diálogo aberta (pode ser um aviso por cima de outra caixa):
        // só ela responde.
        if let (Some(d), Some(text)) = (self.dialog.as_mut(), self.text.as_mut()) {
            let l = d.layout(&g, text, text.default_style());
            let o = d.click(&l, x, y);
            self.dialog_event(o);
            return;
        }
        if let Some(mut f) = self.form.take() {
            let o = match self.text.as_mut() {
                Some(text) => {
                    let l = f.layout(&g, text, text.default_style());
                    // Numa caixa leve, o clique fora vai para a planilha.
                    let outside = f.light && !l.bx.contains(x, y);
                    if outside { None } else { Some(f.click(&l, g.s, x, y)) }
                }
                None => None,
            };
            self.form = Some(f);
            if let Some(o) = o {
                self.form_event(o);
                return;
            }
        }
        if self.dv_list.is_some() {
            self.list_click(x, y);
            return;
        }
        if self.prompt.is_some() {
            let (_, input, ok, cancel) = crate::menu::prompt_rects(&g);
            if ok.contains(x, y) {
                self.prompt_ok();
            } else if cancel.contains(x, y) {
                self.prompt = None;
            } else if input.contains(x, y)
                && let Some(p) = &mut self.prompt
            {
                p.input.select_all();
            }
            return;
        }
        // Painel do filtro.
        if let Some(mut panel) = self.filter_panel.take() {
            if !panel.rect.contains(x, y) {
                return;
            }
            let a = panel.areas(g.s);
            if a.ok.contains(x, y) {
                self.filter_panel = Some(panel);
                self.apply_filter_panel();
            } else if a.cancel.contains(x, y) {
            } else if a.sort_asc.contains(x, y) || a.sort_desc.contains(x, y) {
                let desc = a.sort_desc.contains(x, y);
                self.sort_filter(panel.active.clone(), panel.col, desc);
            } else if a.list.contains(x, y) {
                panel.click_list(g.s, y);
                self.filter_panel = Some(panel);
            } else {
                self.filter_panel = Some(panel);
            }
            return;
        }
        // Lista de sugestões, assinatura ou dica da fórmula.
        if self.popup.is_none() && self.complete_click(&g, x, y) {
            return;
        }
        // Menu aberto: clique nele escolhe; fora, só fecha.
        if let Some(pp) = self.popup.take() {
            if let Some(act) = pp.hit(&g, x, y) {
                self.run_act(cx, act);
                return;
            }
            if pp.rect.contains(x, y) {
                self.popup = Some(pp);
            }
            return;
        }
        if g.toolbar.contains(x, y) {
            self.commit_edit();
            if self.dialog.is_some() {
                return;
            }
            let (Some(text), tab) = (self.text.as_mut(), self.ribbon_tab) else { return };
            let ui = text.default_style();
            let lay = crate::ribbon::layout(&g, tab, text, ui);
            match lay.hit(x, y) {
                Some(crate::ribbon::Hit::Tab(None)) => {
                    let r = lay.file_tab();
                    self.open_drop(crate::ribbon::Drop::File, r, &g);
                }
                Some(crate::ribbon::Hit::Tab(Some(t))) => self.set_ribbon_tab(t),
                Some(crate::ribbon::Hit::Button(crate::ribbon::Act::Drop(d), r, _)) => {
                    self.open_drop(d, r, &g)
                }
                // Duplo clique no pincel: continua ligado.
                Some(crate::ribbon::Hit::Button(crate::ribbon::Act::Painter, ..)) if clicks >= 2 => {
                    self.toggle_painter(true);
                }
                Some(crate::ribbon::Hit::Button(act, ..)) => self.run_act(cx, act),
                None => {}
            }
            return;
        }
        // Zoom na barra de status.
        if g.status.contains(x, y) {
            if self.doc.as_ref().and_then(|d| d.current()).is_none() {
                return;
            }
            self.commit_edit();
            if self.dialog.is_some() {
                return;
            }
            let bar = crate::zoombar::layout(&g);
            match bar.hit(x, y) {
                Some((crate::ribbon::Act::Drop(d), _)) => self.open_drop(d, bar.menu_anchor(&g), &g),
                Some((act, _)) => self.run_act(cx, act),
                None if bar.track.contains(x, y) => {
                    self.drag = Some(Drag::Zoom);
                    self.set_zoom(bar.zoom_at(x));
                }
                None => {}
            }
            return;
        }
        if self.tab_edit.is_some() {
            self.tab_edit_commit();
            // O nome foi recusado (a estrutura da pasta está protegida).
            if self.dialog.is_some() {
                return;
            }
        }
        // Barra de fórmulas: edita a célula ativa ali.
        if g.formula.contains(x, y) && self.doc.as_ref().and_then(|d| d.current()).is_some() {
            match &mut self.editor {
                Some(ed) => ed.in_bar = true,
                None => self.start_edit(None, true),
            }
            if let Some(ed) = &mut self.editor {
                ed.enter_mode = false;
            }
            self.editor_place(&g, x, y, mods.shift);
            return;
        }
        // Dentro do editor: põe o cursor.
        if self.editor.is_some() && self.editor_place(&g, x, y, mods.shift) {
            return;
        }
        // Com uma fórmula aberta, o clique aponta (onde cabe uma referência).
        if self.editor_click(&g, x, y, mods) {
            return;
        }
        // Clique fora do editor confirma o que foi digitado (rolar, não, se
        // for para apontar).
        let scrolling = g.vscroll.contains(x, y) || g.hscroll.contains(x, y);
        if !(scrolling && self.can_point()) {
            let sheet = self.doc.as_ref().map(|d| d.active);
            self.commit_edit();
            // Confirmar voltou para a aba da célula: o clique era na outra.
            if sheet != self.doc.as_ref().map(|d| d.active) && !g.tabs.contains(x, y) {
                return;
            }
            // O valor foi recusado pela validação.
            if self.dialog.is_some() {
                return;
            }
        }
        // Barra de fórmulas: caixa de nome.
        if g.namebox.contains(x, y) {
            self.namebox_focus = true;
            self.cell_ref_into_namebox();
            self.namebox.select_all();
            return;
        }
        self.namebox_focus = false;
        if let Some(f) = &mut self.find {
            f.focused = false;
            if crate::draw::find_rect(&g).contains(x, y) {
                f.focused = true;
                return;
            }
        }
        // Abas.
        if g.tabs.contains(x, y) {
            if let Some(sheet) = self.tab_click(&g, x, y, clicks) {
                self.drag = Some(Drag::Tab { sheet, x });
            }
            return;
        }
        if self.doc.as_ref().and_then(|d| d.current()).is_none() {
            if g.grid.contains(x, y) && self.loader.is_none() {
                self.pick_and_open(cx);
            }
            return;
        }
        // Seta da lista suspensa da célula ativa.
        if self.dv_arrow(&g).is_some_and(|r| r.contains(x, y)) {
            self.open_list();
            return;
        }
        // Alças do objeto selecionado.
        if self.press_grip(&g, x, y, mods) {
            return;
        }
        // Botões, links e macros dos objetos (com Ctrl, só seleciona).
        let select = mods.ctrl || mods.logo;
        if !select && g.grid.contains(x, y) && self.object_click(&g, x, y) {
            return;
        }
        // Segmentação de dados.
        if g.grid.contains(x, y)
            && let Some((obj, item)) = self.slicer_hit(&g, x, y)
        {
            self.commit_edit();
            self.slicer_click(obj, item, mods.ctrl || mods.logo || mods.shift);
            return;
        }
        // Gráfico, imagem ou forma: seleciona e arrasta.
        if self.press_object(&g, x, y, mods) {
            return;
        }
        // Botão de filtro num cabeçalho.
        if let Some((b, f, col)) = self.filter_buttons(&g).into_iter().find(|b| b.0.contains(x, y)) {
            self.commit_edit();
            self.open_filter_panel(f, col, b);
            return;
        }
        // Barras de rolagem.
        if g.vscroll.contains(x, y) {
            let (t0, len) = self.vthumb(&g);
            if y >= t0 && y < t0 + len {
                self.drag = Some(Drag::VThumb { grab: y - t0 });
            } else if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                let page = g.main_view().1 * 0.9;
                v.scroll_y += if y < t0 { -page } else { page };
            }
            return;
        }
        if g.hscroll.contains(x, y) {
            let (t0, len) = self.hthumb(&g);
            if x >= t0 && x < t0 + len {
                self.drag = Some(Drag::HThumb { grab: x - t0 });
            } else if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                let page = g.main_view().0 * 0.9;
                v.scroll_x += if x < t0 { -page } else { page };
            }
            return;
        }
        if self.press_headers(&g, x, y, mods, clicks) || self.press_handle(&g, x, y, mods, clicks) {
            return;
        }
        self.press_cell(&g, x, y, mods, clicks);
    }

    /// Botão nos cabeçalhos: seleciona linhas e colunas inteiras, muda o
    /// tamanho pela borda. `true` = era neles.
    pub(crate) fn press_headers(&mut self, g: &Geom, x: f32, y: f32, mods: Modifiers, clicks: u8) -> bool {
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return true };
        let (nr, nc) = (v.rows.len(), v.cols.len());
        // Canto: seleciona tudo.
        if x < g.rowhdr.x0 || (y >= g.fbar.y1 && y < g.colhdr.y0) {
            self.commit_edit();
            self.outline_click(g, x, y);
            return true;
        }
        // Ctrl soma uma área à seleção; um clique comum começa outra.
        let add = (mods.ctrl || mods.logo) && !mods.shift;
        let in_headers = g.corner.contains(x, y) || g.colhdr.contains(x, y) || g.rowhdr.contains(x, y);
        if in_headers && resize_target(g, v, x, y).is_none() {
            v.begin_area(add, mods.shift);
            self.object = None;
        }
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return true };
        if g.corner.contains(x, y) {
            v.anchor = (0, 0);
            v.cur = (nr - 1, nc - 1);
            return true;
        }
        // Borda de coluna ou linha nos cabeçalhos: redimensionar (duplo clique: ajustar).
        if let Some(edge) = resize_target(g, v, x, y) {
            match edge {
                Edge::Col(col) => {
                    if clicks >= 2 {
                        let list = self.cols_with(col);
                        self.autofit_cols(&list);
                    } else {
                        self.drag = Some(Drag::ResizeCol { col, size: v.cols.size(col), x });
                    }
                }
                Edge::Row(row) => {
                    if clicks >= 2 {
                        let d = v.rows.default_size();
                        v.resize_row(row, d);
                        let list = self.rows_with(row);
                        self.commit_rows(&list, None);
                    } else {
                        self.drag = Some(Drag::ResizeRow { row, size: v.rows.size(row), y });
                    }
                }
            }
            return true;
        }
        // Cabeçalho de colunas.
        if g.colhdr.contains(x, y) {
            let q = if x < g.grid.x0 + g.fw { &g.quads[0] } else { &g.quads[1] };
            let (_, c) = g.cell_in(v, q, x, q.rect.y0);
            if mods.shift {
                v.anchor.0 = 0;
                v.cur = (nr - 1, c);
            } else {
                v.anchor = (0, c);
                v.cur = (nr - 1, c);
            }
            v.extra_anchor = v.anchor;
            self.drag = Some(Drag::Cols);
            return true;
        }
        if g.rowhdr.contains(x, y) {
            let q = if y < g.grid.y0 + g.fh { &g.quads[0] } else { &g.quads[2] };
            let (r, _) = g.cell_in(v, q, q.rect.x0, y);
            if mods.shift {
                v.anchor.1 = 0;
                v.cur = (r, nc - 1);
            } else {
                v.anchor = (r, 0);
                v.cur = (r, nc - 1);
            }
            v.extra_anchor = v.anchor;
            self.drag = Some(Drag::Rows);
            return true;
        }
        false
    }

    /// Botão na alça de preenchimento ou na borda da seleção. `true` = era nelas.
    pub(crate) fn press_handle(&mut self, g: &Geom, x: f32, y: f32, mods: Modifiers, clicks: u8) -> bool {
        if !g.grid.contains(x, y) || mods.shift {
            return false;
        }
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return false };
        let sel = crate::fill::range_of(v.sel());
        if crate::fill::on_handle(g, v, x, y) {
            if clicks >= 2 {
                self.fill_double();
            } else {
                self.drag = Some(Drag::Fill { src: sel });
            }
            return true;
        }
        if clicks < 2
            && crate::fill::on_border(g, v, x, y)
            && let Some((r, c)) = g.hit(v, x, y)
        {
            let grab = (
                r.clamp(sel.first_row, sel.last_row) - sel.first_row,
                c.clamp(sel.first_col, sel.last_col) - sel.first_col,
            );
            self.drag = Some(Drag::MoveCells { src: sel, grab });
            return true;
        }
        false
    }

    /// Botão numa célula: seleciona (Ctrl soma uma área, Shift estende), segue
    /// o link ou, no duplo clique, edita.
    pub(crate) fn press_cell(&mut self, g: &Geom, x: f32, y: f32, mods: Modifiers, clicks: u8) {
        self.object = None;
        let add = (mods.ctrl || mods.logo) && !mods.shift;
        let Some(v) = self.doc.as_ref().and_then(|d| d.current()).map(|x| x.1) else { return };
        if let Some((mut r, mut c)) = g.hit(v, x, y) {
            // Ctrl+clique num hiperlink: segue o link.
            if mods.ctrl || mods.logo {
                let link = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.current())
                    .and_then(|(s, _)| s.layout.link_at(r, c).cloned());
                if let Some(link) = link {
                    self.follow_link(&link);
                    return;
                }
            }
            let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return };
            if !mods.shift
                && let Some(m) = v.merges.at(r, c)
            {
                (r, c) = (m.first_row, m.first_col);
            }
            if !self.can_select(r, c) {
                return;
            }
            let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return };
            v.begin_area(add, mods.shift);
            v.cur = (r, c);
            if !mods.shift {
                v.anchor = (r, c);
            }
            v.extra_anchor = v.anchor;
            if clicks >= 2 && !mods.shift && !add {
                self.start_edit(None, false);
                return;
            }
            self.drag = Some(Drag::Select);
        }
    }

    pub(crate) fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.mouse = (x, y);
        let Some(drag) = self.drag else {
            let g = self.geom();
            let modal = self.prompt.is_some() || self.form.is_some() || self.dialog.is_some();
            if modal || self.popup.is_some() || self.dv_list.is_some() || self.filter_panel.is_some() {
                self.hover_note = None;
                return true;
            }
            let note = self.note_under(&g, x, y);
            if note != self.hover_note {
                self.hover_note = note;
                self.redraw_later();
            }
            // Lista de sugestões: o item sob o mouse fica marcado.
            if self.editor.as_ref().is_some_and(|e| e.complete.list.is_some()) {
                let over = self.complete_boxes(&g).is_some_and(|b| b.contains(x, y));
                if over {
                    if let Some(w) = &self.window {
                        w.set_cursor(Cursor::Default);
                    }
                    self.complete_hover = true;
                    return true;
                }
                if std::mem::take(&mut self.complete_hover) {
                    self.redraw_later();
                }
            }
            let tool = if g.toolbar.contains(x, y) {
                let tab = self.ribbon_tab;
                self.text.as_mut().and_then(|text| {
                    let ui = text.default_style();
                    match crate::ribbon::layout(&g, tab, text, ui).hit(x, y) {
                        Some(crate::ribbon::Hit::Button(a, _, tip)) => Some((a, tip)),
                        _ => None,
                    }
                })
            } else if g.status.contains(x, y) && self.doc.as_ref().and_then(|d| d.current()).is_some() {
                crate::zoombar::layout(&g).hit(x, y)
            } else {
                None
            };
            if tool.map(|t| t.0) != self.hover_act.map(|t| t.0) {
                self.hover_act = tool;
                return true;
            }
            let over_input = g.namebox.contains(x, y)
                || self.find.as_ref().is_some_and(|_| crate::draw::find_rect(&g).contains(x, y));
            let current = self.doc.as_ref().and_then(|d| d.current());
            let edge = current.and_then(|(_, v)| resize_target(&g, v, x, y));
            let link = match current {
                Some((s, v)) if !over_input && !s.layout.links.is_empty() => g
                    .hit(v, x, y)
                    .and_then(|(r, c)| s.layout.link_at(r, c))
                    .map(|l| l.target.clone().or_else(|| l.location.clone()).unwrap_or_default()),
                _ => None,
            };
            // Link, macro ou controle de um objeto sob o mouse.
            let action = g.grid.contains(x, y).then(|| self.object_action(&g, x, y)).flatten();
            let link = link.or_else(|| action.map(|a| a.1));
            let (fill, border) = match current {
                Some((_, v)) if self.editor.is_none() && g.grid.contains(x, y) => {
                    (crate::fill::on_handle(&g, v, x, y), crate::fill::on_border(&g, v, x, y))
                }
                _ => (false, false),
            };
            let object = self.object_cursor(&g, x, y).filter(|_| !fill && link.is_none());
            let cursor = match (edge, &link) {
                _ if object.is_some() => object.unwrap_or(Cursor::Default),
                _ if fill || self.painter.is_some() && g.grid.contains(x, y) => Cursor::Cross,
                _ if border => Cursor::Move,
                (Some(Edge::Col(_)), _) => Cursor::ResizeColumn,
                (Some(Edge::Row(_)), _) => Cursor::ResizeRow,
                (None, Some(_)) => Cursor::Pointer,
                _ if over_input => Cursor::Text,
                _ => Cursor::Default,
            };
            if let Some(w) = &self.window {
                w.set_cursor(cursor);
            }
            let changed = link != self.hover_link || edge != self.hover_edge;
            self.hover_edge = edge;
            self.hover_link = link;
            return changed;
        };
        let g = self.geom();
        if matches!(drag, Drag::Zoom) {
            self.set_zoom(crate::zoombar::layout(&g).zoom_at(x));
            return true;
        }
        let (thumb_v, thumb_h) = (self.vthumb(&g), self.hthumb(&g));
        let Some(doc) = self.doc.as_mut() else { return false };
        let mdw = doc.mdw;
        let Some(v) = doc.view_mut() else { return false };
        match drag {
            Drag::ResizeCol { col, size, x: x0 } => {
                let s = (size as f32 + (x - x0) / g.g).round().max(0.0) as u32;
                v.resize_col(col, s);
                self.message = size_text("Largura", crate::edit::px_to_width(s, mdw), s);
            }
            Drag::ResizeRow { row, size, y: y0 } => {
                let s = (size as f32 + (y - y0) / g.g).round().max(0.0) as u32;
                v.resize_row(row, s);
                self.message = size_text("Altura", s as f32 * 0.75, s);
            }
            Drag::VThumb { grab } => {
                let track = g.vscroll.h() - thumb_v.1;
                let min = v.rows.offset(v.frozen_rows) as f64;
                let max = (v.rows.total() as f64 - g.main_view().1).max(min);
                let t = ((y - grab - g.vscroll.y0) / track.max(1.0)).clamp(0.0, 1.0) as f64;
                v.scroll_y = min + t * (max - min);
            }
            Drag::HThumb { grab } => {
                let track = g.hscroll.w() - thumb_h.1;
                let min = v.cols.offset(v.frozen_cols) as f64;
                let max = (v.cols.total() as f64 - g.main_view().0).max(min);
                let t = ((x - grab - g.hscroll.x0) / track.max(1.0)).clamp(0.0, 1.0) as f64;
                v.scroll_x = min + t * (max - min);
            }
            Drag::Zoom | Drag::Tab { .. } => {}
            Drag::Object { .. } => self.drag_object(drag, &g, x, y),
            Drag::Select
            | Drag::Cols
            | Drag::Rows
            | Drag::Point
            | Drag::Fill { .. }
            | Drag::MoveCells { .. } => {
                // Fora da grade: rola na direção do mouse.
                let main = g.quads[3].rect;
                let step = 2.0 * v.rows.default_size() as f64;
                if y > main.y1 {
                    v.scroll_y += step;
                } else if y < main.y0 && v.scroll_y > v.rows.offset(v.frozen_rows) as f64 {
                    v.scroll_y -= step;
                }
                if x > main.x1 {
                    v.scroll_x += step * 2.0;
                } else if x < main.x0 && v.scroll_x > v.cols.offset(v.frozen_cols) as f64 {
                    v.scroll_x -= step * 2.0;
                }
                let cx = x.clamp(g.grid.x0, g.grid.x1 - 1.0);
                let cy = y.clamp(g.grid.y0, g.grid.y1 - 1.0);
                if let Some((r, c)) = g.hit(v, cx, cy) {
                    match drag {
                        Drag::Point => self.point_drag((r, c)),
                        Drag::Fill { src } => self.drag_area = crate::fill::fill_target(&src, r, c),
                        Drag::MoveCells { src, grab } => {
                            let t = crate::fill::move_target(&src, grab, r, c);
                            self.drag_area = (t != src).then_some(t);
                        }
                        Drag::Select => v.cur = (r, c),
                        Drag::Cols => v.cur.1 = c,
                        _ => v.cur.0 = r,
                    }
                }
            }
        }
        true
    }

    /// Botão solto: termina o arraste. `true` = redesenhar.
    pub(crate) fn mouse_up(&mut self, modifiers: Modifiers) -> bool {
        match self.drag.take() {
            Some(Drag::Select | Drag::Cols | Drag::Rows) if self.painter.is_some() => {
                self.apply_painter();
                true
            }
            Some(Drag::Fill { src }) => {
                if let Some(t) = self.drag_area.take() {
                    let toggle = modifiers.ctrl || modifiers.logo;
                    self.fill_to(src, t, duck_core::fill::Mode::Auto, toggle);
                }
                true
            }
            Some(Drag::MoveCells { src, .. }) => {
                if let Some(t) = self.drag_area.take() {
                    self.drop_cells(src, t, modifiers.ctrl || modifiers.logo);
                }
                true
            }
            Some(Drag::Tab { sheet, x }) => {
                let g = self.geom();
                if (self.mouse.0 - x).abs() > 6.0 * g.s {
                    let slot = self.tab_drop_slot(&g, self.mouse.0);
                    self.tab_drop(sheet, slot, modifiers.ctrl || modifiers.logo);
                }
                true
            }
            Some(Drag::Object { .. }) => {
                self.drop_object();
                true
            }
            Some(Drag::ResizeCol { col, size, .. }) => {
                self.message.clear();
                self.commit_col_size(col, size);
                true
            }
            Some(Drag::ResizeRow { row, size, .. }) => {
                self.message.clear();
                self.commit_row_size(row, size);
                true
            }
            d => d.is_some(),
        }
    }

    pub fn vthumb(&self, g: &Geom) -> (f32, f32) {
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else {
            return (g.vscroll.y0, g.vscroll.h());
        };
        let min = v.rows.offset(v.frozen_rows) as f64;
        thumb(
            g.vscroll.y0,
            g.vscroll.y1,
            v.rows.total() as f64 - min,
            g.main_view().1,
            v.scroll_y - min,
            MIN_THUMB * g.s,
        )
    }

    pub fn hthumb(&self, g: &Geom) -> (f32, f32) {
        let Some((_, v)) = self.doc.as_ref().and_then(|d| d.current()) else {
            return (g.hscroll.x0, g.hscroll.w());
        };
        let min = v.cols.offset(v.frozen_cols) as f64;
        thumb(
            g.hscroll.x0,
            g.hscroll.x1,
            v.cols.total() as f64 - min,
            g.main_view().0,
            v.scroll_x - min,
            MIN_THUMB * g.s,
        )
    }

    /// Segue um hiperlink: lugar dentro do arquivo (`Aba!A1`) ou endereço
    /// externo (só `http`, `https` e `mailto`, no programa padrão).
    pub(crate) fn follow_link(&mut self, link: &duck_core::Hyperlink) {
        if let Some(loc) = link.location.as_deref().filter(|l| !l.is_empty()) {
            let loc = loc.trim_start_matches('#');
            let (sheet, cell) = match loc.rfind('!') {
                Some(i) => (Some(&loc[..i]), &loc[i + 1..]),
                None => (None, loc),
            };
            let target = match sheet {
                Some(name) => {
                    let name = name
                        .strip_prefix('\'')
                        .and_then(|n| n.strip_suffix('\''))
                        .map_or(name.to_string(), |n| n.replace("''", "'"));
                    self.doc
                        .as_ref()
                        .and_then(|d| d.sheets.iter().position(|s| s.info.name.eq_ignore_ascii_case(&name)))
                }
                None => self.doc.as_ref().map(|d| d.active),
            };
            match (target, parse_range(&cell.replace('$', ""))) {
                (Some(i), Some((a, b))) => {
                    self.switch_sheet(i);
                    self.go_to(a.0, a.1);
                    if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                        v.anchor = (b.0.min(v.rows.len() - 1), b.1.min(v.cols.len() - 1));
                    }
                }
                _ => self.message = format!("Destino do link não encontrado: {loc}"),
            }
            return;
        }
        match link.target.as_deref() {
            Some(url) if duck_platform::open_url(url) => self.message = format!("Abrindo {url}"),
            Some(url) => self.message = format!("Link não aberto (só http, https e mailto): {url}"),
            None => self.message = "Link sem destino".into(),
        }
    }

    /// Ajusta a largura da coluna ao texto mais largo dela (até 20 mil células).
    pub(crate) fn autofit_cols(&mut self, cols: &[u32]) {
        let list: Vec<(u32, u32)> = cols.iter().filter_map(|&c| Some((c, self.autofit_size(c)?))).collect();
        self.commit_cols(&list);
    }

    /// Largura (pixels a 100%) em que cabe o que a coluna mostra.
    fn autofit_size(&mut self, col: u32) -> Option<u32> {
        let doc = self.doc.as_ref()?;
        let (sheet, view) = doc.current()?;
        let (sheet, looks, strings, date1904, mdw) =
            (sheet.clone(), doc.looks.clone(), doc.strings.clone(), doc.date1904, doc.mdw);
        let text = self.text.as_mut()?;
        let mut widest = 0.0f32;
        let mut buf = String::new();
        let mut n = 0;
        sheet.for_each_value(col, 0..sheet.rows(), &strings, |r, cell| {
            if view.merges.at(r, col).is_some() {
                return true;
            }
            let s = if cell.style == 0 { view.blank_style(r, col) } else { cell.style };
            let look = looks.get(s);
            if show(&looks, look, cell.value, date1904, &mut buf).is_some() {
                let f = &looks.fonts[(look.font as usize).min(looks.fonts.len() - 1)];
                let st = text.style(&f.name, f.pt * 96.0 / 72.0, f.bold, f.italic);
                widest = widest
                    .max(text.measure_styled(&buf, st) + look.indent as f32 * 3.0 * (st.px() * 0.55).round());
            }
            n += 1;
            n < 20_000
        });
        Some(if widest > 0.0 {
            (widest + 2.0 * 3.0 + 2.0).ceil() as u32
        } else {
            (8.43 * mdw).round() as u32 + 5
        })
    }

    /// Índices (no workbook) das abas visíveis, na ordem das guias.
    pub fn visible_sheet_indices(&self) -> Vec<usize> {
        self.doc
            .as_ref()
            .map(|d| (0..d.sheets.len()).filter(|&i| !d.sheets[i].info.hidden).collect())
            .unwrap_or_default()
    }

    pub(crate) fn cell_ref_into_namebox(&mut self) {
        let r = self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| v.cur);
        if let Some((r, c)) = r {
            let mut s = String::new();
            cell_name(r, c, &mut s);
            self.namebox.set_text(&s);
        }
    }

    pub(crate) fn pick_and_open(&mut self, cx: &mut Context) {
        let Some(w) = &self.window else { return };
        let filters: &[(&str, &[&str])] = &[
            ("Planilhas", &["xlsx", "xlsm", "xlsb", "csv", "tsv", "txt"]),
            ("Planilhas do Excel", &["xlsx", "xlsm", "xlsb"]),
            ("Texto separado (CSV)", &["csv", "tsv", "txt"]),
        ];
        if let Some(p) = w.pick_file("Abrir planilha", filters) {
            self.open(p, cx.waker());
        }
    }

    // --- teclado ---------------------------------------------------------------

    fn key(&mut self, cx: &mut Context, key: Key, mods: Modifiers, text: Option<String>) -> bool {
        let ctrl = mods.ctrl || mods.logo;
        // Caixa de diálogo: Enter confirma, Esc fecha, o resto vai para o campo.
        if let Some(d) = &mut self.dialog {
            let o = d.key(&key, mods, text.as_deref(), cx.clipboard());
            self.dialog_event(o);
            return true;
        }
        if let Some(f) = &mut self.form {
            let o = f.key(&key, mods, text.as_deref(), cx.clipboard());
            self.form_event(o);
            return true;
        }
        if self.dv_list.is_some() {
            self.list_key(&key, mods, text.as_deref());
            return true;
        }
        if let Some(p) = &mut self.prompt {
            match key {
                Key::Enter => self.prompt_ok(),
                Key::Escape => self.prompt = None,
                _ => {
                    p.input.key(&key, mods, text.as_deref(), cx.clipboard());
                }
            }
            return true;
        }
        // Painel do filtro: Enter aplica, Esc fecha, o resto é a busca.
        if let Some(panel) = &mut self.filter_panel {
            match key {
                Key::Enter => self.apply_filter_panel(),
                Key::Escape => self.filter_panel = None,
                _ => {
                    let r = panel.search.key(&key, mods, text.as_deref(), cx.clipboard());
                    if r.changed {
                        panel.scroll = 0;
                    }
                }
            }
            return true;
        }
        if let Some(handled) = self.tab_edit_key(cx.clipboard(), &key, mods, text.as_deref()) {
            return handled;
        }
        if let Some(handled) = self.editor_key(cx.clipboard(), &key, mods, text.as_deref()) {
            return handled;
        }
        // Campo de busca com foco.
        if let Some(f) = &mut self.find
            && f.focused
        {
            let r = f.input.key(&key, mods, text.as_deref(), cx.clipboard());
            if r.cancelled {
                self.find = None;
                return true;
            }
            if r.submitted || r.submitted_back {
                self.find_next(cx.waker(), r.submitted_back);
                return true;
            }
            if r.handled {
                return true;
            }
        }
        // Caixa de nome com foco ("ir para").
        if self.namebox_focus {
            let r = self.namebox.key(&key, mods, text.as_deref(), cx.clipboard());
            if r.cancelled {
                self.namebox_focus = false;
                return true;
            }
            if r.submitted {
                self.namebox_focus = false;
                let typed = self.namebox.text.trim().replace(',', ";");
                if typed.contains(';') && self.scripted_select(&typed) {
                    return true;
                }
                match parse_range(self.namebox.text.trim()) {
                    Some((a, b)) => {
                        // Célula ativa no primeiro canto; seleção até o outro.
                        self.grow_grid(b.0, b.1);
                        self.go_to(a.0, a.1);
                        if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                            v.anchor = (b.0.min(v.rows.len() - 1), b.1.min(v.cols.len() - 1));
                        }
                    }
                    None => self.message = format!("Referência inválida: {}", self.namebox.text.trim()),
                }
                return true;
            }
            if r.handled {
                return true;
            }
        }
        if self.popup.is_none() && self.object_key(&key, mods) {
            return true;
        }
        let shift = mods.shift;
        match key {
            // Alt+=: AutoSoma.
            Key::Char(ref k) if mods.alt && !ctrl && k == "=" => self.autosum(),
            // Ctrl+Alt+= / Ctrl+Alt+-: inserir / apagar linhas ou colunas.
            Key::Char(ref k) if ctrl && mods.alt && matches!(k.as_str(), "=" | "+") => {
                self.shift_selection(true, None)
            }
            Key::Char(ref k) if ctrl && mods.alt && k == "-" => self.shift_selection(false, None),
            Key::Char(ref k) if ctrl => match k.to_lowercase().as_str() {
                "d" if !shift => self.fill_copy(true),
                "r" if !shift => self.fill_copy(false),
                "c" => self.copy(cx, false),
                "x" => self.copy(cx, true),
                // Ctrl+Alt+V: colar especial; Ctrl+Shift+V: só valores.
                "v" if mods.alt => {
                    if let Some(t) = cx.clipboard().get_text() {
                        self.ask_paste_special(t);
                    }
                }
                "v" if shift => {
                    let what = crate::paste::What::Values;
                    self.paste(cx, crate::paste::PasteOpts { what, ..Default::default() });
                }
                "v" => self.paste(cx, Default::default()),
                "h" if !shift => self.open_replace(),
                "1" if !shift => self.ask_format(0),
                ";" | ":" => self.insert_now(shift),
                "z" => self.undo(),
                "y" => self.redo(),
                "s" => self.save(shift),
                "f" => self.open_find(),
                "l" if shift => self.toggle_filter(),
                "g" => {
                    self.namebox_focus = true;
                    self.cell_ref_into_namebox();
                    self.namebox.select_all();
                }
                "o" => self.pick_and_open(cx),
                "," if !shift => self.ask_settings(),
                "a" => {
                    if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                        v.anchor = (0, 0);
                        v.cur = (v.rows.len() - 1, v.cols.len() - 1);
                    }
                }
                // O "+" do teclado principal precisa de Shift.
                "+" => self.zoom_step(true),
                "=" if !shift => self.zoom_step(true),
                "-" if !shift => self.zoom_step(false),
                "0" if !shift => self.set_zoom(100),
                other => {
                    let other = other.to_string();
                    if !self.format_shortcut(&other, shift) {
                        return false;
                    }
                }
            },
            Key::F(3) => self.find_next(cx.waker(), shift),
            Key::F(2) if shift => self.ask_note(),
            Key::F(2) => self.start_edit(None, false),
            Key::F(11) if shift => {
                let at = self.doc.as_ref().map(|d| d.active);
                self.tab_new(None, at);
            }
            // F9: recalcula os voláteis; Ctrl+Alt+F9 (ou Shift+F9): tudo.
            Key::F(9) => {
                let all = shift || (mods.ctrl && mods.alt);
                if let Some(doc) = &mut self.doc {
                    doc.calc.cancel();
                    doc.calc.request(if all {
                        crate::calc::Pending::All
                    } else {
                        crate::calc::Pending::Volatile
                    });
                }
                self.message = "Calculando…".into();
                self.calc_kick();
            }
            Key::Delete => self.clear_selection(),
            Key::Backspace => self.start_edit(Some(""), false),
            Key::F(5) => {
                self.namebox_focus = true;
                self.cell_ref_into_namebox();
                self.namebox.select_all();
            }
            Key::Escape if self.popup.is_some() => self.popup = None,
            Key::Escape if self.drag_area.is_some() => {
                self.drag = None;
                self.drag_area = None;
            }
            Key::Escape if self.painter.is_some() => {
                self.painter = None;
                self.message.clear();
            }
            Key::Escape if self.marked.is_some() => {
                self.unmark();
                self.message.clear();
            }
            Key::Escape => {
                if self.find.take().is_none()
                    && let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut())
                {
                    v.single();
                    v.anchor = v.cur;
                }
            }
            Key::ArrowUp => self.move_cur(-1, 0, shift, ctrl),
            Key::ArrowDown if mods.alt && self.open_list() => {}
            Key::ArrowDown => self.move_cur(1, 0, shift, ctrl),
            Key::ArrowLeft => self.move_cur(0, -1, shift, ctrl),
            Key::ArrowRight => self.move_cur(0, 1, shift, ctrl),
            Key::Tab => self.move_cur(0, if shift { -1 } else { 1 }, false, false),
            Key::Enter => {
                let (dr, dc) = self.prefs.enter.step(shift);
                if (dr, dc) != (0, 0) {
                    self.move_cur(dr, dc, false, false);
                }
            }
            Key::PageUp if ctrl => self.cycle_sheet(false),
            Key::PageDown if ctrl => self.cycle_sheet(true),
            Key::PageUp => {
                let n = self.visible_rows();
                self.move_cur(-n, 0, shift, false);
            }
            Key::PageDown => {
                let n = self.visible_rows();
                self.move_cur(n, 0, shift, false);
            }
            Key::Home if ctrl => {
                let fr = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.current())
                    .map_or((0, 0), |(_, v)| (v.frozen_rows, v.frozen_cols));
                self.set_cur(fr.0, fr.1, shift);
                self.ensure_visible();
            }
            Key::End if ctrl => {
                let end = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.current())
                    .map(|(s, _)| (s.rows().max(1) - 1, s.cols().max(1) - 1));
                if let Some((r, c)) = end {
                    self.set_cur(r, c, shift);
                    self.ensure_visible();
                }
            }
            Key::Home => {
                let r = self.doc.as_ref().and_then(|d| d.current()).map_or(0, |(_, v)| v.cur.0);
                self.set_cur(r, 0, shift);
                self.ensure_visible();
            }
            // Digitar por cima da célula ativa.
            _ if !ctrl
                && !mods.alt
                && text.as_deref().is_some_and(|t| !t.is_empty() && !t.chars().any(char::is_control)) =>
            {
                self.start_edit(text.as_deref(), false);
            }
            _ => return false,
        }
        true
    }

    // --- benchmark -------------------------------------------------------------

    fn bench_ready(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.slot().data.is_some() && !d.slot().preview)
    }

    fn bench_before_frame(&mut self) {
        let ready = self.bench_ready();
        let Some(b) = &mut self.bench else { return };
        if !ready || b.warmup > 0 {
            return;
        }
        let phase = b.phase;
        b.rng = mix(b.rng);
        let rng = b.rng;
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return };
        match phase {
            0 => v.scroll_y += 40.0,
            1 => {
                let row = (rng % v.rows.len() as u64) as u32;
                let col = ((rng >> 32) % v.cols.len() as u64) as u32;
                v.scroll_y = v.rows.offset(row) as f64;
                v.scroll_x = v.cols.offset(col) as f64;
            }
            _ => v.scroll_x += 30.0,
        }
    }

    fn bench_after_frame(&mut self, cpu_ms: f64, build_ms: f64) -> bool {
        let n_inst = self.instances.len();
        let ready = self.bench_ready();
        let Some(b) = &mut self.bench else { return false };
        if !ready {
            return false;
        }
        if b.warmup > 0 {
            b.warmup -= 1;
            if b.warmup == 0 {
                b.phase_start = Instant::now();
                // As medições começam do topo da aba (não da posição salva).
                if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                    v.scroll_x = 0.0;
                    v.scroll_y = 0.0;
                }
                self.clamp_scroll();
            }
            return false;
        }
        b.cpu_ms.push(cpu_ms);
        b.build_ms.push(build_ms);
        b.instances = b.instances.max(n_inst);
        b.frame += 1;
        let (name, frames) = BENCH_PHASES[b.phase];
        if b.frame < frames {
            return false;
        }
        let wall = b.phase_start.elapsed().as_secs_f64();
        let mut v = std::mem::take(&mut b.cpu_ms);
        v.sort_by(|a, b| a.total_cmp(b));
        let mut m = std::mem::take(&mut b.build_ms);
        m.sort_by(|a, b| a.total_cmp(b));
        let pct = |v: &[f64], p: f64| v[(((v.len() - 1) as f64) * p).round() as usize];
        let _ = writeln!(
            b.report,
            "  {name:<34} {frames:>5} frames | montagem p50 {:>6.3} ms p99 {:>6.3} | frame p50 {:>6.3} ms p95 {:>6.3} p99 {:>6.3} máx {:>7.3} | {:>5.0} fps",
            pct(&m, 0.5),
            pct(&m, 0.99),
            pct(&v, 0.5),
            pct(&v, 0.95),
            pct(&v, 0.99),
            v.last().copied().unwrap_or(0.0),
            frames as f64 / wall,
        );
        b.phase += 1;
        b.frame = 0;
        b.phase_start = Instant::now();
        let phase = b.phase;
        if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
            v.scroll_x = 0.0;
            v.scroll_y = 0.0;
        }
        self.clamp_scroll();
        phase >= BENCH_PHASES.len()
    }

    fn print_report(&self) {
        println!("\nduck-sheets · F2");
        if let Some(d) = &self.doc {
            let s = d.slot();
            println!(
                "  arquivo: {} · aba {} ({} linhas)",
                d.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                d.active + 1,
                fmt_int(s.data.as_ref().map_or(0, |x| x.rows() as u64)),
            );
        }
        if let Some(g) = &self.gfx {
            let i = g.adapter_info();
            let (w, h) = g.size();
            println!("  GPU: {} ({}) · janela {w}×{h} · escala {:.2}", i.name, i.backend, self.scale);
            let parts: Vec<String> = g
                .init_timings()
                .iter()
                .map(|(n, d)| format!("{n} {:.0} ms", d.as_secs_f64() * 1000.0))
                .collect();
            println!("  abertura da GPU: {}", parts.join(" · "));
        }
        println!("\nLinha do tempo (desde o início do processo):");
        let mut tl = self.timeline.clone();
        if let Some(t) = self.first_content {
            tl.push(("primeira tela com dados", t));
        }
        tl.sort_by_key(|t| t.1);
        for (name, t) in tl {
            println!("  {name:<32} {:>8.1} ms", t.as_secs_f64() * 1000.0);
        }
        let (cur, peak) = crate::draw::process_memory();
        println!(
            "\nMemória do processo: {:.0} MB agora ({:.0} MB privada), {:.0} MB de pico",
            cur as f64 / 1048576.0,
            crate::draw::private_memory() as f64 / 1048576.0,
            peak as f64 / 1048576.0
        );
        if let Some(b) = &self.bench {
            println!("\nFrames (vsync desligado):");
            print!("{}", b.report);
            println!("  máximo de instâncias num frame: {}", b.instances);
        }
        if let Some(t) = &self.text {
            let st = t.stats();
            println!(
                "\nTexto: {} moldagens, {} pelo caminho rápido, {} hits de cache, {} glifos, {} resets de atlas",
                st.shaped_runs, st.fast_path_runs, st.cache_hits, st.glyphs_rasterized, st.atlas_resets
            );
        }
    }
}

impl App for SheetsApp {
    fn init(&mut self, cx: &mut Context, window: Window) {
        self.mark("janela criada");
        self.scale = window.scale_factor() as f32;
        self.waker = Some(cx.waker());
        self.system = window.color_scheme().unwrap_or(ColorScheme::Light);
        window.set_color_scheme(self.prefs.mode.frame());
        self.window = Some(window.clone());
        // A leitura começa já: corre junto com a GPU e as fontes.
        match self.opts.path.clone() {
            Some(p) if !self.opts.new_file => self.open(p, cx.waker()),
            None if self.opts.bench && !self.opts.new_file => {
                self.error = Some("informe o arquivo".into());
                cx.exit();
                return;
            }
            // Sem arquivo: planilha em branco, como no Excel.
            _ => self.new_file(),
        }
        if !self.unattended() {
            self.recovery_offer();
        }
        self.update_start();
        let (w, h) = window.inner_size();
        let vsync = !self.opts.bench;
        match Gfx::new_with(self.gpu_early.take(), window.clone(), w, h, vsync, ATLAS) {
            Ok(g) => self.gfx = Some(g),
            Err(e) => {
                self.error = Some(e.to_string());
                cx.exit();
                return;
            }
        }
        self.mark("GPU pronta");
        let mut text = self.fonts_loading.take().expect("fontes").join().expect("thread de fontes");
        if let Some(g) = &self.gfx {
            text.set_atlas_size(g.atlas_size());
        }
        text.set_font_size(UI_FONT_PX * self.scale);
        self.text = Some(text);
        self.mark("fontes prontas");
        if self.loader.is_none() {
            // Sem arquivo: nada a esperar para ler o resto das fontes.
            self.start_font_scan();
        }
    }

    fn event(&mut self, cx: &mut Context, event: Event) {
        if matches!(event, Event::Key { .. } | Event::MouseDown { .. } | Event::Wheel { .. }) {
            self.recovery.input();
        }
        let redraw = match event {
            Event::Wake => {
                if self.trace {
                    eprintln!("{:>8.1} ms  wake", self.t0.elapsed().as_secs_f64() * 1000.0);
                }
                let fonts = self.poll_fonts();
                let a = self.poll_loader() || fonts;
                // Sem ninguém olhando, o arquivo que não abre encerra.
                if self.error.is_some() && self.doc.is_none() && self.unattended() {
                    cx.exit();
                    return;
                }
                let saved = self.poll_save(false);
                if saved.is_some() && self.opts.save_to.is_some() && self.opts.screenshot.is_none() {
                    if saved == Some(false) {
                        self.error = Some("falha ao salvar".into());
                    }
                    cx.exit();
                    return;
                }
                self.recovery_poll();
                let b = self.update_poll()
                    | self.poll_search()
                    | saved.is_some()
                    | self.poll_calc()
                    | self.poll_images();
                if a && let Some(q) = self.opts.find.take() {
                    // `--find`: busca assim que a aba ativa chegar.
                    if self.doc.as_ref().is_some_and(|d| d.slot().data.is_some() && !d.slot().preview) {
                        self.open_find();
                        if let Some(f) = &mut self.find {
                            f.input.set_text(&q);
                        }
                        self.find_next(cx.waker(), false);
                    } else {
                        self.opts.find = Some(q);
                    }
                }
                a || b
            }
            Event::SchemeChanged(s) => {
                self.system = s;
                self.prefs.mode == Mode::System
            }
            // O Windows repinta a moldura com as cores do sistema quando
            // alguma configuração muda: repõe as do tema escolhido.
            Event::Focused(true) => {
                if let (Some(w), Some(f)) = (&self.window, self.prefs.mode.frame()) {
                    w.set_color_scheme(Some(f));
                }
                false
            }
            Event::Resized { width, height } => {
                if let Some(g) = &mut self.gfx {
                    g.resize(width, height);
                }
                true
            }
            Event::ScaleFactorChanged(f) => {
                self.scale = f as f32;
                if let Some(t) = &mut self.text {
                    t.set_font_size(UI_FONT_PX * self.scale);
                }
                true
            }
            Event::Pinch(d) => {
                // Pinça no touchpad (macOS): zoom contínuo.
                self.pinch += d * self.zoom() as f64;
                let step = self.pinch.trunc();
                self.pinch -= step;
                self.set_zoom(self.zoom() + step as i32);
                true
            }
            Event::FileDropped(p) => {
                self.open(p, cx.waker());
                true
            }
            Event::OpenFile(p) => {
                self.open_elsewhere(p, cx.waker());
                true
            }
            Event::Wheel { delta, modifiers } => {
                let g = self.geom();
                let (mx, my) = self.mouse;
                let (mut dx, mut dy) = match delta {
                    WheelDelta::Lines { x, y } => (x as f64 * 60.0, y as f64 * 60.0),
                    WheelDelta::Pixels { x, y } => (x / g.g as f64, y / g.g as f64),
                };
                if self.dialog.is_some() {
                } else if let Some(l) = self.dv_list.as_mut().filter(|l| l.rect.contains(mx, my)) {
                    l.scroll_by(if dy > 0.0 { -3 } else { 3 }, g.s);
                } else if modifiers.ctrl {
                    // Roda de precisão e touchpad mandam pedaços: um passo por
                    // entalhe (60) acumulado.
                    if self.wheel_zoom * dy < 0.0 {
                        self.wheel_zoom = 0.0;
                    }
                    self.wheel_zoom += dy;
                    while self.wheel_zoom.abs() >= 59.0 {
                        let up = self.wheel_zoom > 0.0;
                        self.wheel_zoom -= if up { 60.0 } else { -60.0 };
                        self.zoom_step(up);
                    }
                } else if let Some(mut f) = self.form.take() {
                    if let Some(text) = self.text.as_mut() {
                        let l = f.layout(&g, text, text.default_style());
                        f.wheel(&l, mx, my, dy > 0.0);
                    }
                    self.form = Some(f);
                } else if let Some(panel) = self.filter_panel.as_mut().filter(|p| p.rect.contains(mx, my)) {
                    panel.scroll_by(if dy > 0.0 { -3 } else { 3 });
                } else if self.complete_wheel(&g, mx, my, dy > 0.0) {
                } else if g.tabs.contains(mx, my) {
                    self.tabbar.scroll_by(-(dy as f32) * g.s, g.tabs.w());
                } else {
                    if modifiers.shift {
                        std::mem::swap(&mut dx, &mut dy);
                    }
                    if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
                        v.scroll_x -= dx;
                        v.scroll_y -= dy;
                    }
                }
                true
            }
            Event::MouseDown { button: MouseButton::Left, x, y, modifiers, clicks } => {
                self.mouse_down(cx, x as f32, y as f32, modifiers, clicks);
                true
            }
            Event::MouseDown { button: MouseButton::Right, x, y, .. } => {
                self.popup = None;
                self.dv_list = None;
                let busy = self.prompt.is_some() || self.form.is_some() || self.dialog.is_some();
                if !busy && self.geom().tabs.contains(x as f32, y as f32) {
                    self.tab_context_menu(x as f32, y as f32);
                } else if !busy {
                    self.context_menu(x as f32, y as f32);
                }
                true
            }
            Event::MouseUp { button: MouseButton::Left, modifiers, .. } => self.mouse_up(modifiers),
            Event::MouseMove { x, y } => self.mouse_move(x as f32, y as f32),
            Event::Key { key, pressed: true, modifiers, text, .. } => {
                if key == Key::F(12) && self.loader.is_none() && self.doc.is_none() {
                    self.pick_and_open(cx);
                    true
                } else {
                    self.key(cx, key, modifiers, text)
                }
            }
            _ => false,
        };
        self.recovery_arm();
        if redraw {
            self.clamp_scroll();
            self.redraw_later();
        }
    }

    fn close_requested(&mut self, _cx: &mut Context) -> bool {
        self.confirm_discard()
    }

    fn redraw(&mut self, cx: &mut Context) {
        let t0 = Instant::now();
        // Num arraste, espera a vez do frame antes de montá-lo e trata primeiro
        // o que chegou nesse tempo: o que se arrasta acompanha o mouse. (Fora
        // dele não: ao redimensionar a janela, o quadro reservado envelhece.)
        if self.drag.is_some()
            && self.gfx.as_mut().is_some_and(|g| g.begin_frame())
            && t0.elapsed() > Duration::from_millis(2)
        {
            self.redraw_later();
            return;
        }
        self.bench_before_frame();
        self.clamp_scroll();
        let tb = Instant::now();
        self.build_frame();
        let build_ms = tb.elapsed().as_secs_f64() * 1000.0;
        let has_content = self.doc.as_ref().is_some_and(|d| d.slot().data.is_some());

        let clear = self.theme().paper;
        let (Some(gfx), Some(text)) = (self.gfx.as_mut(), self.text.as_mut()) else { return };
        for u in text.take_uploads() {
            gfx.upload_atlas(u.x, u.y, u.w, u.h, &u.data);
        }
        let atlas_reset = text.take_atlas_reset();
        // Screenshot: depois de todas as abas, de todas as fontes e de uma
        // busca pedida (`--find`).
        let searching = self.opts.find.is_some() || self.find.as_ref().is_some_and(|f| f.job.is_some());
        let done = self.doc.as_ref().is_some_and(|d| d.finished.is_some())
            && !searching
            && self.saving.is_none()
            && text.fonts_complete();
        if self.screenshot_pending && done && !atlas_reset {
            let (w, h, rgba) = gfx.render_to_rgba(&self.instances, clear);
            let path = self.opts.screenshot.clone().unwrap();
            match crate::draw::save_png(&path, w, h, &rgba) {
                Ok(()) => println!("screenshot salvo em {} ({w}×{h})", path.display()),
                Err(e) => self.error = Some(format!("não foi possível salvar {}: {e}", path.display())),
            }
            self.print_report();
            cx.exit();
            return;
        }
        let status = gfx.render(&self.instances, clear);
        let cpu_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if self.trace && self.t0.elapsed().as_secs_f64() < 5.0 {
            eprintln!(
                "{:>8.1} ms  frame: montagem {build_ms:.2} ms, total {cpu_ms:.2} ms, {status:?}, dados {has_content}",
                self.t0.elapsed().as_secs_f64() * 1000.0
            );
        }
        if status == FrameStatus::Presented && has_content && self.first_content.is_none() {
            self.first_content = Some(self.t0.elapsed());
            // Primeira tela com dados apresentada: a leitura completa pode usar todos os núcleos.
            if let Some(l) = &self.loader {
                l.proceed();
            }
            self.start_font_scan();
        }
        if self.recent_cpu.len() == 120 {
            self.recent_cpu.remove(0);
        }
        self.recent_cpu.push(cpu_ms);

        if self.bench.is_some() {
            if self.bench_after_frame(cpu_ms, build_ms) {
                self.print_report();
                cx.exit();
                return;
            }
            self.redraw_later();
        } else if atlas_reset || status == FrameStatus::Skipped || self.screenshot_pending {
            self.redraw_later();
        }
    }
}

// ---------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------

/// Borda de cabeçalho que pode ser arrastada.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edge {
    Col(u32),
    Row(u32),
}

/// `Largura: 12,29 (91 pixels)`.
fn size_text(what: &str, size: f32, px: u32) -> String {
    format!("{what}: {:.2} ({px} pixels)", size).replace('.', ",")
}

/// Onde a borda aparece: a faixa no cabeçalho e, até o fim da grade, a linha
/// que acompanha o arraste.
pub fn edge_rects(g: &Geom, v: &View, e: Edge, w: f32) -> Option<(Rect, Rect)> {
    let half = (w / 2.0).floor();
    match e {
        Edge::Col(c) => {
            let q = &g.quads[if c < v.frozen_cols { 0 } else { 1 }];
            let x = crate::draw::pos(&v.cols, q.ox, c + 1, q.rect.x0, g.g);
            (x >= q.rect.x0 - 1.0 && x <= q.rect.x1 + 1.0).then(|| {
                let (a, b) = (x - half, x - half + w);
                (Rect::new(a, g.colhdr.y0, b, g.colhdr.y1), Rect::new(a, g.colhdr.y1, a + 1.0, g.grid.y1))
            })
        }
        Edge::Row(r) => {
            let q = &g.quads[if r < v.frozen_rows { 0 } else { 2 }];
            let y = crate::draw::pos(&v.rows, q.oy, r + 1, q.rect.y0, g.g);
            (y >= q.rect.y0 - 1.0 && y <= q.rect.y1 + 1.0).then(|| {
                let (a, b) = (y - half, y - half + w);
                (Rect::new(g.rowhdr.x0, a, g.rowhdr.x1, b), Rect::new(g.rowhdr.x1, a, g.grid.x1, a + 1.0))
            })
        }
    }
}

/// Borda (direita de uma coluna, de baixo de uma linha) perto do ponto, nos cabeçalhos.
/// Colunas e linhas estreitas guardam o meio para o clique que seleciona.
pub fn resize_target(g: &Geom, v: &View, x: f32, y: f32) -> Option<Edge> {
    let grab = |size: u32| (RESIZE_GRAB * g.s).min((size as f32 * g.g / 4.0).max(3.0 * g.s));
    let most = RESIZE_GRAB * g.s;
    if g.colhdr.contains(x, y) {
        for q in [&g.quads[0], &g.quads[1]] {
            // Sem painéis congelados, o de cima tem altura 0 (mas tem a largura).
            if x < q.rect.x0 - most || x > q.rect.x1 + most || q.rect.w() <= 0.0 {
                continue;
            }
            let (_, c) = g.cell_in(v, q, x.clamp(q.rect.x0, q.rect.x1 - 1.0), q.rect.y0);
            // A borda mais próxima: a da coluna sob o mouse ou a da anterior.
            let right = |c: u32| q.rect.x0 + ((v.cols.offset(c + 1) as f64 - q.ox) * g.g as f64) as f32;
            let near = grab(v.cols.size(c));
            if (right(c) - x).abs() <= near {
                return Some(Edge::Col(c));
            }
            if c > 0 {
                let prev = v.cols.prev_visible(c);
                if prev < c && (right(prev) - x).abs() <= near {
                    return Some(Edge::Col(prev));
                }
            }
        }
    }
    if g.rowhdr.contains(x, y) {
        for q in [&g.quads[0], &g.quads[2]] {
            // E o da esquerda, largura 0.
            if y < q.rect.y0 - most || y > q.rect.y1 + most || q.rect.h() <= 0.0 {
                continue;
            }
            let (r, _) = g.cell_in(v, q, q.rect.x0, y.clamp(q.rect.y0, q.rect.y1 - 1.0));
            let bottom = |r: u32| q.rect.y0 + ((v.rows.offset(r + 1) as f64 - q.oy) * g.g as f64) as f32;
            let near = grab(v.rows.size(r));
            if (bottom(r) - y).abs() <= near {
                return Some(Edge::Row(r));
            }
            if r > 0 {
                let prev = v.rows.prev_visible(r);
                if prev < r && (bottom(prev) - y).abs() <= near {
                    return Some(Edge::Row(prev));
                }
            }
        }
    }
    None
}

/// Texto da seleção como o Excel copia: o que as células mostram, colunas
/// separadas por tab e linhas por CRLF; células com tab, quebra ou aspas
/// vão entre aspas.
pub fn selection_tsv(
    sheet: &Sheet,
    strings: &StringTable,
    looks: &Looks,
    date1904: bool,
    (r0, c0, r1, c1): (u32, u32, u32, u32),
) -> String {
    let cells = (r1 - r0 + 1) as usize * (c1 - c0 + 1) as usize;
    let mut out = String::with_capacity(cells * 8);
    let mut buf = String::new();
    for r in r0..=r1 {
        for c in c0..=c1 {
            if c > c0 {
                out.push('\t');
            }
            let cell = sheet.cell(r, c, strings);
            if show(looks, looks.get(cell.style), cell.value, date1904, &mut buf).is_some() {
                if buf.contains(['\t', '\n', '"']) {
                    out.push('"');
                    out.push_str(&buf.replace('"', "\"\""));
                    out.push('"');
                } else {
                    out.push_str(&buf);
                }
            }
        }
        out.push_str("\r\n");
    }
    out
}

/// A célula a um passo de `from` (pulando escondidas e saindo de mesclagens)
/// ou, com `jump`, na borda dos dados.
pub(crate) fn step(sheet: &Sheet, v: &View, from: (u32, u32), dr: i32, dc: i32, jump: bool) -> (u32, u32) {
    let (mut r, mut c) = from;
    if jump {
        if dr != 0 {
            r = data_edge_row(sheet, v, r, c, dr > 0);
        }
        if dc != 0 {
            c = data_edge_col(sheet, v, r, c, dc > 0);
        }
        return (r, c);
    }
    // Sair de uma mesclagem pelo lado dela.
    if let Some(m) = v.merges.at(r, c) {
        if dr > 0 {
            r = m.last_row;
        } else if dr < 0 {
            r = m.first_row;
        }
        if dc > 0 {
            c = m.last_col;
        } else if dc < 0 {
            c = m.first_col;
        }
    }
    for _ in 0..dr.unsigned_abs() {
        r = if dr > 0 { v.rows.next_visible(r) } else { v.rows.prev_visible(r) };
    }
    for _ in 0..dc.unsigned_abs() {
        c = if dc > 0 { v.cols.next_visible(c) } else { v.cols.prev_visible(c) };
    }
    (r, c)
}

/// Ctrl+↑/↓: até a borda do bloco de dados, como no Excel.
fn data_edge_row(sheet: &Sheet, v: &View, r: u32, c: u32, down: bool) -> u32 {
    let last = v.rows.len() - 1;
    if down {
        if r >= last {
            return last;
        }
        if sheet.has_value(r, c) && sheet.has_value(r + 1, c) {
            let mut x = r + 1;
            while x < last && sheet.has_value(x + 1, c) {
                x += 1;
            }
            x
        } else {
            sheet.next_value_row(c, r).unwrap_or(last).min(last)
        }
    } else {
        if r == 0 {
            return 0;
        }
        if sheet.has_value(r, c) && sheet.has_value(r - 1, c) {
            let mut x = r - 1;
            while x > 0 && sheet.has_value(x - 1, c) {
                x -= 1;
            }
            x
        } else {
            sheet.prev_value_row(c, r).unwrap_or(0)
        }
    }
}

/// Ctrl+←/→.
fn data_edge_col(sheet: &Sheet, v: &View, r: u32, c: u32, right: bool) -> u32 {
    let last = v.cols.len() - 1;
    let has = |c: u32| sheet.has_value(r, c);
    if right {
        if c >= last {
            return last;
        }
        if has(c) && has(c + 1) {
            let mut x = c + 1;
            while x < last && has(x + 1) {
                x += 1;
            }
            x
        } else {
            (c + 1..=last).find(|&x| has(x)).unwrap_or(last)
        }
    } else {
        if c == 0 {
            return 0;
        }
        if has(c) && has(c - 1) {
            let mut x = c - 1;
            while x > 0 && has(x - 1) {
                x -= 1;
            }
            x
        } else {
            (0..c).rev().find(|&x| has(x)).unwrap_or(0)
        }
    }
}

/// (início, tamanho) do polegar de uma barra de rolagem.
fn thumb(a: f32, b: f32, total: f64, view: f64, scroll: f64, min: f32) -> (f32, f32) {
    let track = (b - a).max(1.0);
    let len = if total <= view { track } else { (track * (view / total) as f32).max(min).min(track) };
    let max = (total - view).max(0.0);
    let pos = if max > 0.0 { (track - len) * (scroll / max).clamp(0.0, 1.0) as f32 } else { 0.0 };
    ((a + pos).round(), len.round())
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

pub fn col_name(mut c: u32, out: &mut String) {
    let mut buf = [0u8; 8];
    let mut n = 0;
    loop {
        buf[n] = b'A' + (c % 26) as u8;
        n += 1;
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    for i in (0..n).rev() {
        out.push(buf[i] as char);
    }
}

pub fn cell_name(r: u32, c: u32, out: &mut String) {
    col_name(c, out);
    let _ = write!(out, "{}", r + 1);
}

/// "B12" → (11, 1). Aceita `$`, minúsculas e prefixo de aba (`Aba!B12`).
pub fn parse_cell(s: &str) -> Option<(u32, u32)> {
    let s = s.rsplit('!').next()?.trim().replace('$', "");
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    if letters.is_empty() || letters.len() > 3 || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut col = 0u32;
    for b in letters.to_ascii_uppercase().bytes() {
        col = col * 26 + (b - b'A' + 1) as u32;
    }
    let row: u32 = digits.parse().ok()?;
    ((1..=MAX_ROWS).contains(&row) && col <= MAX_COLS).then(|| (row - 1, col - 1))
}

/// "B2:D10" ou "B2" → (canto, outro canto).
pub fn parse_range(s: &str) -> Option<((u32, u32), (u32, u32))> {
    match s.split_once(':') {
        Some((a, b)) => Some((parse_cell(a)?, parse_cell(b)?)),
        None => parse_cell(s).map(|a| (a, a)),
    }
}

/// Número inteiro com ponto de milhar.
pub fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_column_widths() {
        // Calibri 11 (dígito de 7 px): os valores que o Excel grava.
        assert_eq!(col_px(9.140625, 7.0), 64);
        assert_eq!(col_px(13.7109375, 7.0), 96);
        assert_eq!(col_px(0.0, 7.0), 0);
        assert_eq!(row_px(15.0), 20);
        assert_eq!(row_px(12.75), 17);
    }

    #[test]
    fn cell_refs() {
        assert_eq!(parse_cell("A1"), Some((0, 0)));
        assert_eq!(parse_cell("$b$12"), Some((11, 1)));
        assert_eq!(parse_cell("Aba!AA10"), Some((9, 26)));
        assert_eq!(parse_cell("XFD1048576"), Some((1_048_575, 16_383)));
        assert_eq!(parse_cell("1A"), None);
        assert_eq!(parse_cell("A0"), None);
        assert_eq!(parse_range("B2:C3"), Some(((1, 1), (2, 2))));
        let mut s = String::new();
        cell_name(9, 26, &mut s);
        assert_eq!(s, "AA10");
        assert_eq!(fmt_int(1234567), "1.234.567");
        assert_eq!(fmt_int(12), "12");
    }

    fn small_sheet() -> (Sheet, StringTable, Looks) {
        use duck_core::{SheetPart, kind};
        let mut strings = StringTable::new();
        let a = strings.push("Loja \"A\"");
        let b = strings.push("linha 1\nlinha 2");
        let mut p = SheetPart::new();
        // Coluna 0: linhas 0..=4 e 10..=12 com valor; coluna 1 só na linha 0.
        for r in (0..=4).chain(10..=12) {
            p.set(r, 0, kind::NUMBER, 0, (r as f64 * 1.5).to_bits());
        }
        p.set(0, 1, kind::SHARED_STR, 0, a as u64);
        p.set(1, 1, kind::SHARED_STR, 0, b as u64);
        p.set(0, 3, kind::BOOL, 0, 1);
        let mut s = Sheet::new("t");
        s.merge_part(p);
        s.finish();
        let looks = Looks::new(&Styles::default());
        (s, strings, looks)
    }

    #[test]
    fn copy_as_tsv() {
        let (sheet, strings, looks) = small_sheet();
        let tsv = selection_tsv(&sheet, &strings, &looks, false, (0, 0, 1, 3));
        assert_eq!(tsv, "0\t\"Loja \"\"A\"\"\"\t\tVERDADEIRO\r\n1,5\t\"linha 1\nlinha 2\"\t\t\r\n");
    }

    #[test]
    fn ctrl_arrows_follow_data_edges() {
        let (sheet, _, _) = small_sheet();
        let v = build_view(&sheet, 7.0, None, None);
        // Dentro do bloco: vai até a última com valor.
        assert_eq!(data_edge_row(&sheet, &v, 0, 0, true), 4);
        // Na borda: pula o vazio até o próximo bloco.
        assert_eq!(data_edge_row(&sheet, &v, 4, 0, true), 10);
        assert_eq!(data_edge_row(&sheet, &v, 10, 0, true), 12);
        // Depois do último: vai até o fim da grade.
        assert_eq!(data_edge_row(&sheet, &v, 12, 0, true), v.rows.len() - 1);
        assert_eq!(data_edge_row(&sheet, &v, 12, 0, false), 10);
        assert_eq!(data_edge_row(&sheet, &v, 10, 0, false), 4);
        assert_eq!(data_edge_row(&sheet, &v, 4, 0, false), 0);
        // Colunas: 0 e 1 juntas, 2 vazia, 3 com valor.
        assert_eq!(data_edge_col(&sheet, &v, 0, 0, true), 1);
        assert_eq!(data_edge_col(&sheet, &v, 0, 1, true), 3);
        assert_eq!(data_edge_col(&sheet, &v, 0, 3, false), 1);
    }

    #[test]
    fn grid_grows() {
        let (sheet, _, _) = small_sheet();
        let mut v = build_view(&sheet, 7.0, None, None);
        assert_eq!((v.rows.len(), v.cols.len()), (1000, 100));
        v.min = (5000, 400);
        let v = build_view(&sheet, 7.0, Some(v), None);
        assert_eq!((v.rows.len(), v.cols.len()), (5000, 400));
        let mut v = build_view(&sheet, 7.0, Some(v), None);
        v.min = (u32::MAX, u32::MAX);
        let v = build_view(&sheet, 7.0, Some(v), None);
        assert_eq!((v.rows.len(), v.cols.len()), (MAX_ROWS, MAX_COLS));
    }

    #[test]
    fn merges_lookup() {
        let m = Merges::new(vec![
            CellRange { first_row: 10, first_col: 0, last_row: 30, last_col: 2 },
            CellRange { first_row: 2, first_col: 5, last_row: 2, last_col: 8 },
        ]);
        assert_eq!(m.at(20, 1).map(|r| r.first_row), Some(10));
        assert_eq!(m.at(2, 6).map(|r| r.first_col), Some(5));
        assert!(m.at(31, 0).is_none());
        assert_eq!(m.in_rows(25, 26).count(), 1);
        assert_eq!(m.in_rows(0, 100).count(), 2);
    }
}
