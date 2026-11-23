//! Faixa de opções (F3.5): abas (Arquivo, Página Inicial, Inserir, Exibir),
//! grupos com rótulo embaixo e botões em duas fileiras, como nos editores
//! de planilha comuns. Os botões disparam [`Act`]s (ver `actions.rs`).

use duck_core::{HAlign, VAlign};
use duck_text::TextStyle;
use duck_ui::{Align, Painter, Rect, Rgba};

use crate::app::Geom;
use crate::format::{Active, Fmt};
use crate::theme::{Mode, Theme};

/// Altura das abas e do corpo da faixa (pixels lógicos).
pub const TABS_H: f32 = 30.0;
pub const BODY_H: f32 = 78.0;

/// O que um botão, item de menu ou atalho faz.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    Fmt(Fmt),
    Paste,
    Cut,
    Copy,
    Undo,
    Redo,
    Save,
    SaveAs,
    New,
    Open,
    /// Arquivo recente (índice na lista).
    Recent(u8),
    /// Registrar o duck-sheets no "Abrir com" do sistema.
    Register,
    /// Versão, licença e o adaptador gráfico.
    About,
    Quit,
    /// Abre uma lista ou paleta.
    Drop(Drop),
    Shift {
        insert: bool,
        cols: bool,
    },
    Merge(Merge),
    Clear(Clear),
    Find,
    GoTo,
    /// Zoom: mais/menos 10 (0 = 100%).
    Zoom(i8),
    /// Zoom num nível.
    ZoomTo(u16),
    Grid,
    Headers,
    /// Tema claro, escuro ou o do sistema.
    Theme(Mode),
    Freeze(Freeze),
    Hide {
        rows: bool,
        hide: bool,
    },
    ColWidth,
    RowHeight,
    Autofit,
    /// Classificar pela coluna da célula ativa (`true` = Z a A).
    Sort(bool),
    /// Liga/desliga o filtro.
    Filter,
    ClearFilter,
    Reapply,
    /// Regra de validação da seleção; abrir a lista da célula ativa.
    Validation,
    OpenList,
    /// Criar ou editar a nota da célula; excluir; deixar à vista; todas.
    Note,
    DeleteNotes,
    ShowNote,
    ShowNotes,
    /// Próxima nota (`true` = anterior).
    NextNote(bool),
    /// Proteger (`true`) ou desproteger a planilha ou a pasta.
    ProtectSheet(bool),
    ProtectBook(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    File,
    Font,
    Size,
    NumFmt,
    Borders,
    TextColor,
    FillColor,
    Merge,
    Insert,
    Delete,
    Format,
    Clear,
    Freeze,
    Zoom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    Center,
    Cells,
    Across,
    Unmerge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    All,
    Formats,
    Contents,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freeze {
    Here,
    TopRow,
    FirstCol,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Home,
    Insert,
    Data,
    Review,
    View,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Button,
    /// Caixa com o valor atual e seta (largura em pixels lógicos).
    Combo(f32),
    Icon(Icon),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Icon {
    H(HAlign),
    V(VAlign),
}

struct Item {
    act: Act,
    label: &'static str,
    tip: &'static str,
    kind: Kind,
}

const fn b(act: Act, label: &'static str, tip: &'static str) -> Item {
    Item { act, label, tip, kind: Kind::Button }
}

const fn combo(act: Act, w: f32, tip: &'static str) -> Item {
    Item { act, label: "", tip, kind: Kind::Combo(w) }
}

const fn icon(act: Act, i: Icon, tip: &'static str) -> Item {
    Item { act, label: "", tip, kind: Kind::Icon(i) }
}

struct Group {
    title: &'static str,
    rows: [&'static [Item]; 2],
}

const HOME: &[Group] = &[
    Group {
        title: "Área de transferência",
        rows: [
            &[b(Act::Paste, "Colar", "Colar (Ctrl+V)")],
            &[b(Act::Cut, "Recortar", "Recortar (Ctrl+X)"), b(Act::Copy, "Copiar", "Copiar (Ctrl+C)")],
        ],
    },
    Group {
        title: "Fonte",
        rows: [
            &[
                combo(Act::Drop(Drop::Font), 150.0, "Fonte"),
                combo(Act::Drop(Drop::Size), 54.0, "Tamanho da fonte"),
                b(Act::Fmt(Fmt::Grow(1)), "A+", "Aumentar a fonte (Ctrl+Shift+>)"),
                b(Act::Fmt(Fmt::Grow(-1)), "A-", "Diminuir a fonte (Ctrl+Shift+<)"),
            ],
            &[
                b(Act::Fmt(Fmt::Bold), "N", "Negrito (Ctrl+N ou Ctrl+B)"),
                b(Act::Fmt(Fmt::Italic), "I", "Itálico (Ctrl+I)"),
                b(Act::Fmt(Fmt::Underline), "S", "Sublinhado (Ctrl+U)"),
                b(Act::Fmt(Fmt::Strike), "abc", "Tachado (Ctrl+5)"),
                b(Act::Drop(Drop::Borders), "Bordas", "Bordas"),
                b(Act::Drop(Drop::FillColor), "Fundo", "Cor de preenchimento"),
                b(Act::Drop(Drop::TextColor), "A", "Cor do texto"),
            ],
        ],
    },
    Group {
        title: "Alinhamento",
        rows: [
            &[
                icon(Act::Fmt(Fmt::VAlign(VAlign::Top)), Icon::V(VAlign::Top), "Alinhar em cima"),
                icon(Act::Fmt(Fmt::VAlign(VAlign::Center)), Icon::V(VAlign::Center), "Alinhar no meio"),
                icon(Act::Fmt(Fmt::VAlign(VAlign::Bottom)), Icon::V(VAlign::Bottom), "Alinhar embaixo"),
                b(Act::Fmt(Fmt::Wrap), "Quebrar texto", "Quebrar o texto em várias linhas"),
            ],
            &[
                icon(
                    Act::Fmt(Fmt::Align(HAlign::Left)),
                    Icon::H(HAlign::Left),
                    "Alinhar à esquerda (Ctrl+Shift+L)",
                ),
                icon(
                    Act::Fmt(Fmt::Align(HAlign::Center)),
                    Icon::H(HAlign::Center),
                    "Centralizar (Ctrl+Shift+E)",
                ),
                icon(
                    Act::Fmt(Fmt::Align(HAlign::Right)),
                    Icon::H(HAlign::Right),
                    "Alinhar à direita (Ctrl+Shift+R)",
                ),
                b(Act::Fmt(Fmt::Indent(-1)), "Recuo-", "Diminuir o recuo"),
                b(Act::Fmt(Fmt::Indent(1)), "Recuo+", "Aumentar o recuo"),
                b(Act::Drop(Drop::Merge), "Mesclar", "Mesclar células"),
            ],
        ],
    },
    Group {
        title: "Número",
        rows: [
            &[combo(Act::Drop(Drop::NumFmt), 140.0, "Formato de número")],
            &[
                b(Act::Fmt(Fmt::Number(7)), "R$", "Moeda (Ctrl+Shift+4)"),
                b(Act::Fmt(Fmt::Number(10)), "%", "Porcentagem (Ctrl+Shift+5)"),
                b(Act::Fmt(Fmt::Number(4)), "000", "Separador de milhar (Ctrl+Shift+1)"),
                b(Act::Fmt(Fmt::Decimals(1)), "+,0", "Aumentar casas decimais"),
                b(Act::Fmt(Fmt::Decimals(-1)), "-,0", "Diminuir casas decimais"),
            ],
        ],
    },
    Group {
        title: "Células",
        rows: [
            &[
                b(Act::Drop(Drop::Insert), "Inserir", "Inserir linhas ou colunas"),
                b(Act::Drop(Drop::Delete), "Excluir", "Excluir linhas ou colunas"),
            ],
            &[b(Act::Drop(Drop::Format), "Formatar", "Largura, altura, ocultar e reexibir")],
        ],
    },
    Group {
        title: "Edição",
        rows: [
            &[
                b(Act::Drop(Drop::Clear), "Limpar", "Limpar conteúdo ou formatos"),
                b(Act::Find, "Localizar", "Localizar (Ctrl+F)"),
            ],
            &[b(Act::GoTo, "Ir para", "Ir para uma célula (Ctrl+G)")],
        ],
    },
];

const INSERT: &[Group] = &[
    Group {
        title: "Linhas e colunas",
        rows: [
            &[
                b(
                    Act::Shift { insert: true, cols: false },
                    "Linhas acima",
                    "Inserir linhas acima da seleção (Ctrl+Alt+=)",
                ),
                b(
                    Act::Shift { insert: true, cols: true },
                    "Colunas à esquerda",
                    "Inserir colunas à esquerda da seleção",
                ),
            ],
            &[
                b(
                    Act::Shift { insert: false, cols: false },
                    "Excluir linhas",
                    "Excluir as linhas da seleção (Ctrl+Alt+-)",
                ),
                b(
                    Act::Shift { insert: false, cols: true },
                    "Excluir colunas",
                    "Excluir as colunas da seleção",
                ),
            ],
        ],
    },
    Group {
        title: "Mesclagem",
        rows: [
            &[b(Act::Merge(Merge::Center), "Mesclar e centralizar", "Mesclar a seleção e centralizar")],
            &[b(Act::Merge(Merge::Unmerge), "Desfazer mesclagem", "Separar as células mescladas")],
        ],
    },
];

const VIEW: &[Group] = &[
    Group {
        title: "Zoom",
        rows: [
            &[
                b(Act::Zoom(-1), "-", "Diminuir o zoom (Ctrl+-)"),
                b(Act::Zoom(0), "100%", "Zoom em 100% (Ctrl+0)"),
                b(Act::Zoom(1), "+", "Aumentar o zoom (Ctrl++)"),
            ],
            &[],
        ],
    },
    Group {
        title: "Mostrar",
        rows: [
            &[b(Act::Grid, "Linhas de grade", "Mostrar as linhas de grade")],
            &[b(Act::Headers, "Cabeçalhos", "Mostrar os cabeçalhos de linhas e colunas")],
        ],
    },
    Group {
        title: "Janela",
        rows: [
            &[b(Act::Drop(Drop::Freeze), "Congelar painéis", "Manter linhas e colunas visíveis ao rolar")],
            &[],
        ],
    },
    Group {
        title: "Tema",
        rows: [
            &[
                b(Act::Theme(Mode::Light), "Claro", "Tema claro"),
                b(Act::Theme(Mode::Dark), "Escuro", "Tema escuro (as cores do arquivo não mudam)"),
            ],
            &[b(Act::Theme(Mode::System), "Seguir o sistema", "Claro ou escuro, como o sistema")],
        ],
    },
];

/// Botões de acesso rápido, no fim da linha das abas.
const QUICK: &[Item] = &[
    b(Act::Save, "Salvar", "Salvar (Ctrl+S)"),
    b(Act::Undo, "Desfazer", "Desfazer (Ctrl+Z)"),
    b(Act::Redo, "Refazer", "Refazer (Ctrl+Y)"),
];

const TAB_NAMES: [(&str, Option<Tab>); 6] = [
    ("Arquivo", None),
    ("Página Inicial", Some(Tab::Home)),
    ("Inserir", Some(Tab::Insert)),
    ("Dados", Some(Tab::Data)),
    ("Revisão", Some(Tab::Review)),
    ("Exibir", Some(Tab::View)),
];

const DATA: &[Group] = &[
    Group {
        title: "Classificar e filtrar",
        rows: [
            &[
                b(
                    Act::Sort(false),
                    "Classificar A→Z",
                    "Classificar pela coluna da célula ativa, do menor ao maior",
                ),
                b(
                    Act::Sort(true),
                    "Classificar Z→A",
                    "Classificar pela coluna da célula ativa, do maior ao menor",
                ),
            ],
            &[
                b(Act::Filter, "Filtro", "Ligar ou desligar o filtro (setas nos cabeçalhos)"),
                b(Act::ClearFilter, "Limpar", "Tirar os critérios do filtro e mostrar todas as linhas"),
                b(Act::Reapply, "Reaplicar", "Aplicar o filtro de novo depois de editar os dados"),
            ],
        ],
    },
    Group {
        title: "Validação",
        rows: [
            &[b(
                Act::Validation,
                "Validação de dados",
                "Definir o que as células da seleção aceitam (lista, números, datas, fórmula)",
            )],
            &[b(Act::OpenList, "Abrir a lista", "Abrir a lista suspensa da célula ativa (Alt+↓)")],
        ],
    },
];

const REVIEW: &[Group] = &[
    Group {
        title: "Notas",
        rows: [
            &[
                b(Act::Note, "Nova nota", "Criar ou editar a nota da célula ativa (Shift+F2)"),
                b(Act::DeleteNotes, "Excluir", "Excluir as notas da seleção"),
                b(Act::NextNote(true), "Anterior", "Ir para a nota anterior"),
                b(Act::NextNote(false), "Próxima", "Ir para a próxima nota"),
            ],
            &[
                b(
                    Act::ShowNote,
                    "Mostrar ou ocultar",
                    "Deixar a nota da célula sempre à vista, ou escondê-la",
                ),
                b(Act::ShowNotes, "Mostrar todas", "Mostrar todas as notas da aba"),
            ],
        ],
    },
    Group {
        title: "Proteção",
        rows: [
            &[
                b(
                    Act::ProtectSheet(true),
                    "Proteger planilha",
                    "Impedir mudanças nas células bloqueadas desta planilha",
                ),
                b(
                    Act::ProtectSheet(false),
                    "Desproteger planilha",
                    "Tirar a proteção desta planilha (pede a senha, se houver)",
                ),
            ],
            &[
                b(
                    Act::ProtectBook(true),
                    "Proteger pasta",
                    "Impedir criar, excluir, renomear, mover e ocultar abas",
                ),
                b(
                    Act::ProtectBook(false),
                    "Desproteger pasta",
                    "Tirar a proteção da pasta (pede a senha, se houver)",
                ),
            ],
        ],
    },
];

/// Um botão posicionado.
pub struct Laid {
    pub act: Act,
    pub rect: Rect,
    pub tip: &'static str,
    label: &'static str,
    kind: Kind,
}

/// O que há na faixa num ponto.
pub enum Hit {
    /// Uma aba (`None` = Arquivo).
    Tab(Option<Tab>),
    Button(Act, Rect, &'static str),
}

pub struct Layout {
    tabs: Vec<(Rect, Option<Tab>, &'static str)>,
    pub buttons: Vec<Laid>,
    /// Grupos: (área, título).
    groups: Vec<(Rect, &'static str)>,
}

fn groups(tab: Tab) -> &'static [Group] {
    match tab {
        Tab::Home => HOME,
        Tab::Insert => INSERT,
        Tab::Data => DATA,
        Tab::Review => REVIEW,
        Tab::View => VIEW,
    }
}

fn item_w(it: &Item, text: &mut duck_text::TextSystem, style: TextStyle, s: f32) -> f32 {
    match it.kind {
        Kind::Combo(w) => (w * s).round(),
        Kind::Icon(_) => (28.0 * s).round(),
        Kind::Button => {
            let drop = matches!(it.act, Act::Drop(_));
            let w = text.measure_styled(it.label, style) + if drop { 26.0 } else { 14.0 } * s;
            w.max(28.0 * s).round()
        }
    }
}

pub fn layout(g: &Geom, tab: Tab, text: &mut duck_text::TextSystem, ui: TextStyle) -> Layout {
    let s = g.s;
    let r = g.toolbar;
    let tabs_y1 = r.y0 + (TABS_H * s).round();
    // Abas.
    let mut x = r.x0 + (8.0 * s).round();
    let mut tabs = Vec::new();
    for (name, t) in TAB_NAMES {
        let w = (text.measure_styled(name, ui) + 24.0 * s).round();
        tabs.push((Rect::new(x, r.y0, x + w, tabs_y1), t, name));
        x += w;
    }
    let mut buttons = Vec::new();
    // Acesso rápido, à direita das abas.
    let mut qx = r.x1 - (8.0 * s).round();
    for it in QUICK.iter().rev() {
        let w = item_w(it, text, ui, s);
        let y0 = r.y0 + (3.0 * s).round();
        buttons.push(Laid {
            act: it.act,
            rect: Rect::new(qx - w, y0, qx, tabs_y1 - (3.0 * s).round()),
            tip: it.tip,
            label: it.label,
            kind: it.kind,
        });
        qx -= w + (2.0 * s).round();
    }
    // Grupos.
    let row_h = (26.0 * s).round();
    let gap = (2.0 * s).round();
    let top = tabs_y1 + (6.0 * s).round();
    let mut groups_out = Vec::new();
    let mut gx = r.x0 + (8.0 * s).round();
    for grp in groups(tab) {
        let mut wmax = text.measure_styled(grp.title, ui.with_px((11.0 * s).round())) + 8.0 * s;
        for (k, row) in grp.rows.iter().enumerate() {
            let y0 = top + k as f32 * (row_h + (4.0 * s).round());
            let mut bx = gx + (6.0 * s).round();
            for it in row.iter() {
                let w = item_w(it, text, ui, s);
                buttons.push(Laid {
                    act: it.act,
                    rect: Rect::new(bx, y0, bx + w, y0 + row_h),
                    tip: it.tip,
                    label: it.label,
                    kind: it.kind,
                });
                bx += w + gap;
            }
            wmax = wmax.max(bx - gx + (4.0 * s).round());
        }
        let gr = Rect::new(gx, tabs_y1, gx + wmax.round(), r.y1);
        groups_out.push((gr, grp.title));
        gx = gr.x1 + (6.0 * s).round();
    }
    Layout { tabs, buttons, groups: groups_out }
}

impl Layout {
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if let Some(t) = self.tabs.iter().find(|t| t.0.contains(x, y)) {
            return Some(Hit::Tab(t.1));
        }
        self.buttons.iter().find(|b| b.rect.contains(x, y)).map(|b| Hit::Button(b.act, b.rect, b.tip))
    }

    /// Retângulo do botão que abre `d` (para a lista abrir embaixo dele).
    pub fn rect_of(&self, act: Act) -> Option<Rect> {
        self.buttons.iter().find(|b| b.act == act).map(|b| b.rect)
    }

    /// Retângulo da aba Arquivo.
    pub fn file_tab(&self) -> Rect {
        self.tabs[0].0
    }
}

/// Valores mostrados na faixa: célula ativa e a visão.
pub struct State<'a> {
    pub active: Option<&'a Active>,
    pub grid: bool,
    pub headers: bool,
    pub colors: (u32, u32),
    pub num_fmt_name: &'a str,
    pub mode: Mode,
    pub show_notes: bool,
    /// A planilha e a pasta estão protegidas.
    pub protected: (bool, bool),
}

fn rgba(rgb: u32) -> Rgba {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
}

/// Setinha para baixo (três traços cada vez menores).
fn caret(p: &mut Painter, x: f32, cy: f32, s: f32, c: Rgba) {
    for i in 0..3 {
        let w = (6.0 - 2.0 * i as f32) * s;
        let y = (cy - 1.0 * s + i as f32 * s).round();
        p.fill(
            Rect::new((x - w / 2.0).round(), y, (x + w / 2.0).round(), y + s.max(1.0)),
            &Rect::everything(),
            c,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    p: &mut Painter,
    g: &Geom,
    tab: Tab,
    ui: TextStyle,
    st: &State,
    hover: Option<Act>,
    file_open: bool,
    th: &Theme,
) {
    let s = g.s;
    let px = s.round().max(1.0);
    let r = g.toolbar;
    let lay = layout(g, tab, p.text, ui);
    let tabs_y1 = r.y0 + (TABS_H * s).round();
    p.fill(Rect::new(r.x0, r.y0, r.x1, tabs_y1), &r, th.bg);
    p.fill(Rect::new(r.x0, tabs_y1, r.x1, r.y1), &r, th.body);
    p.fill(Rect::new(r.x0, r.y1 - px, r.x1, r.y1), &r, th.line);
    // Abas.
    for (rect, t, name) in &lay.tabs {
        let current = *t == Some(tab);
        let file = t.is_none();
        if file {
            p.fill(rect.inset((3.0 * s).round()), &r, if file_open { th.on } else { th.accent });
            p.label(name, ui, *rect, Align::Center, 0.0, if file_open { th.text } else { th.on_accent });
            continue;
        }
        p.label(name, ui, *rect, Align::Center, 0.0, if current { th.accent } else { th.text });
        if current {
            let w = p.text.measure_styled(name, ui);
            let cx = (rect.x0 + rect.x1) / 2.0;
            p.fill(
                Rect::new((cx - w / 2.0).round(), rect.y1 - 3.0 * px, (cx + w / 2.0).round(), rect.y1),
                &r,
                th.accent,
            );
        }
    }
    // Grupos: separador e título.
    let small = ui.with_px((11.0 * s).round());
    for (gr, title) in &lay.groups {
        p.fill(Rect::new(gr.x1, gr.y0 + 6.0 * s, gr.x1 + px, gr.y1 - 6.0 * s), &r, th.line);
        let tr = Rect::new(gr.x0, gr.y1 - (18.0 * s).round(), gr.x1, gr.y1 - (2.0 * s).round());
        p.label(title, small, tr, Align::Center, 0.0, th.muted);
    }
    // Botões.
    let on = |a: Act| -> bool {
        let Some(f) = st.active else {
            return match a {
                Act::Grid => st.grid,
                Act::Headers => st.headers,
                Act::Theme(m) => m == st.mode,
                Act::ShowNotes => st.show_notes,
                Act::ProtectBook(true) => st.protected.1,
                _ => false,
            };
        };
        match a {
            Act::Fmt(Fmt::Bold) => f.bold,
            Act::Fmt(Fmt::Italic) => f.italic,
            Act::Fmt(Fmt::Underline) => f.underline,
            Act::Fmt(Fmt::Strike) => f.strike,
            Act::Fmt(Fmt::Align(h)) => f.align == h,
            Act::Fmt(Fmt::VAlign(v)) => f.valign == v,
            Act::Fmt(Fmt::Wrap) => f.wrap,
            Act::Grid => st.grid,
            Act::Headers => st.headers,
            Act::Theme(m) => m == st.mode,
            Act::ShowNotes => st.show_notes,
            Act::ProtectSheet(true) => st.protected.0,
            Act::ProtectBook(true) => st.protected.1,
            _ => false,
        }
    };
    for bt in &lay.buttons {
        let rect = bt.rect;
        match bt.kind {
            Kind::Combo(_) => {
                p.fill(rect, &r, th.field);
                p.stroke(rect, &r, px, if hover == Some(bt.act) { th.accent } else { th.line });
                let text = match bt.act {
                    Act::Drop(Drop::Font) => st.active.map_or(String::new(), |a| a.font.clone()),
                    Act::Drop(Drop::Size) => st.active.map_or(String::new(), |a| {
                        let mut t = format!("{}", a.size);
                        if let Some(x) = t.strip_suffix(".0") {
                            t = x.to_string();
                        }
                        t.replace('.', ",")
                    }),
                    _ => st.num_fmt_name.to_string(),
                };
                let tr = Rect::new(rect.x0, rect.y0, rect.x1 - 16.0 * s, rect.y1);
                p.label(&text, ui, tr, Align::Left, 6.0 * s, th.text);
                caret(p, rect.x1 - 9.0 * s, (rect.y0 + rect.y1) / 2.0, s, th.muted);
                continue;
            }
            _ if on(bt.act) => p.fill(rect, &r, th.on),
            _ if hover == Some(bt.act) => p.fill(rect, &r, th.hover),
            _ => {}
        }
        let (cx, cy) = ((rect.x0 + rect.x1) / 2.0, (rect.y0 + rect.y1) / 2.0);
        match bt.kind {
            Kind::Icon(Icon::H(h)) => {
                let w = 14.0 * s;
                for (i, frac) in [1.0f32, 0.6, 1.0, 0.6].iter().enumerate() {
                    let lw = (w * frac).round();
                    let y = (cy - 6.0 * s + i as f32 * 4.0 * s).round();
                    let x0 = match h {
                        HAlign::Left => cx - w / 2.0,
                        HAlign::Right => cx + w / 2.0 - lw,
                        _ => cx - lw / 2.0,
                    }
                    .round();
                    p.fill(Rect::new(x0, y, x0 + lw, y + (2.0 * s).round().max(1.0)), &r, th.text);
                }
            }
            Kind::Icon(Icon::V(v)) => {
                // Uma moldura com a linha de texto em cima, no meio ou embaixo.
                let bx = Rect::new(
                    (cx - 7.0 * s).round(),
                    (cy - 7.0 * s).round(),
                    (cx + 7.0 * s).round(),
                    (cy + 7.0 * s).round(),
                );
                p.stroke(bx, &r, px, th.muted);
                let y = match v {
                    VAlign::Top => bx.y0 + 3.0 * s,
                    VAlign::Bottom => bx.y1 - 5.0 * s,
                    _ => cy - 1.0 * s,
                }
                .round();
                p.fill(Rect::new(bx.x0 + 3.0 * s, y, bx.x1 - 3.0 * s, y + (2.0 * s).round()), &r, th.text);
            }
            _ => {
                let mut stl = ui;
                stl.bold = matches!(bt.act, Act::Fmt(Fmt::Bold));
                stl.italic = matches!(bt.act, Act::Fmt(Fmt::Italic));
                let drop = matches!(bt.act, Act::Drop(_));
                let lr = if drop { Rect::new(rect.x0, rect.y0, rect.x1 - 12.0 * s, rect.y1) } else { rect };
                p.label(bt.label, stl, lr, Align::Center, 0.0, th.text);
                let tw = p.text.measure_styled(bt.label, stl);
                let lcx = (lr.x0 + lr.x1) / 2.0;
                match bt.act {
                    Act::Fmt(Fmt::Underline) => {
                        let y = (cy + 7.0 * s).round();
                        p.fill(
                            Rect::new((lcx - tw / 2.0).round(), y, (lcx + tw / 2.0).round(), y + px),
                            &r,
                            th.text,
                        );
                    }
                    Act::Fmt(Fmt::Strike) => {
                        let y = (cy + 1.0 * s).round();
                        p.fill(
                            Rect::new((lcx - tw / 2.0).round(), y, (lcx + tw / 2.0).round(), y + px),
                            &r,
                            th.text,
                        );
                    }
                    Act::Drop(Drop::TextColor) | Act::Drop(Drop::FillColor) => {
                        let c = if bt.act == Act::Drop(Drop::TextColor) { st.colors.0 } else { st.colors.1 };
                        let y = (rect.y1 - 5.0 * s).round();
                        p.fill(Rect::new(lr.x0 + 5.0 * s, y, lr.x1 - 3.0 * s, y + 3.0 * s), &r, rgba(c));
                    }
                    _ => {}
                }
                if drop {
                    caret(p, rect.x1 - 8.0 * s, cy, s, th.muted);
                }
            }
        }
    }
}
