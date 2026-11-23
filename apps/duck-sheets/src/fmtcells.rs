//! Formatar células (Ctrl+1): número (inclusive códigos personalizados),
//! alinhamento, fonte, borda e preenchimento. Só o que foi mexido na caixa
//! muda nas células; o resto de cada uma fica como estava.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use duck_core::{
    Border, BorderSide, BorderStyle, Color, Command, Fill, HAlign, NumFmt, Styles, VAlign, Value, numfmt_pt,
};

use crate::app::SheetsApp;
use crate::fill::range_of;
use crate::format::{FONTS, SIZES};
use crate::forms::{Control, Field, Form, FormId, SIDES, Side};
use crate::look::Looks;

/// O que a caixa aberta guarda além dos campos.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// Campos mexidos.
    touched: HashSet<&'static str>,
    /// Lados da borda a trocar, na ordem de [`SIDES`].
    sides: [Option<BorderSide>; 6],
    /// Valor da célula ativa (amostra).
    sample: Option<f64>,
    sample_text: String,
    date1904: bool,
}

const CATS: [&str; 11] = [
    "Geral",
    "Número",
    "Moeda",
    "Contábil",
    "Data",
    "Hora",
    "Porcentagem",
    "Fração",
    "Científico",
    "Texto",
    "Personalizado",
];

const DATES: [&str; 11] = [
    "dd/mm/yyyy",
    "d/m/yy",
    "dd/mm/yy",
    "d-mmm",
    "d-mmm-yy",
    "mmm-yy",
    "mmmm-yy",
    "d \"de\" mmmm \"de\" yyyy",
    "dddd, d \"de\" mmmm \"de\" yyyy",
    "yyyy-mm-dd",
    "dd/mm/yyyy hh:mm",
];

const TIMES: [&str; 8] =
    ["h:mm", "h:mm:ss", "h:mm AM/PM", "h:mm:ss AM/PM", "[h]:mm:ss", "mm:ss", "mm:ss.0", "dd/mm/yyyy h:mm"];

const FRACTIONS: [&str; 8] =
    ["# ?/?", "# ??/??", "# ???/???", "# ?/2", "# ?/4", "# ?/8", "# ?/10", "# ??/100"];

const SYMBOLS: [(&str, &str); 4] = [("R$", "\"R$ \""), ("US$", "\"US$ \""), ("€", "\"€ \""), ("Nenhum", "")];

const CUSTOM: [&str; 14] = [
    "General",
    "0",
    "0.00",
    "#,##0",
    "#,##0.00",
    "#,##0;[Red]-#,##0",
    "0%",
    "0.00%",
    "0.00E+00",
    "dd/mm/yyyy",
    "d-mmm-yy",
    "h:mm:ss",
    "[h]:mm:ss",
    "@",
];

const HALIGNS: [(&str, HAlign); 8] = [
    ("Geral", HAlign::General),
    ("Esquerda (recuo)", HAlign::Left),
    ("Centro", HAlign::Center),
    ("Direita (recuo)", HAlign::Right),
    ("Preencher", HAlign::Fill),
    ("Justificado", HAlign::Justify),
    ("Centralizar na seleção", HAlign::CenterContinuous),
    ("Distribuído (recuo)", HAlign::Distributed),
];

const VALIGNS: [(&str, VAlign); 5] = [
    ("Superior", VAlign::Top),
    ("Centro", VAlign::Center),
    ("Inferior", VAlign::Bottom),
    ("Justificado", VAlign::Justify),
    ("Distribuído", VAlign::Distributed),
];

const LINES: [(&str, BorderStyle); 13] = [
    ("Fina", BorderStyle::Thin),
    ("Média", BorderStyle::Medium),
    ("Grossa", BorderStyle::Thick),
    ("Dupla", BorderStyle::Double),
    ("Fio", BorderStyle::Hair),
    ("Pontilhada", BorderStyle::Dotted),
    ("Tracejada", BorderStyle::Dashed),
    ("Tracejada média", BorderStyle::MediumDashed),
    ("Traço e ponto", BorderStyle::DashDot),
    ("Traço e ponto média", BorderStyle::MediumDashDot),
    ("Traço e dois pontos", BorderStyle::DashDotDot),
    ("Traço e dois pontos média", BorderStyle::MediumDashDotDot),
    ("Traço e ponto inclinada", BorderStyle::SlantDashDot),
];

/// Código de um formato da categoria.
fn code_of(cat: usize, decimals: usize, thousands: bool, neg: usize, symbol: usize, kind: usize) -> String {
    let dec = if decimals > 0 { format!(".{}", "0".repeat(decimals)) } else { String::new() };
    let int = if thousands { "#,##0" } else { "0" };
    let negative = |pos: String| match neg {
        1 => format!("{pos};[Red]{pos}"),
        2 => format!("{pos};[Red]\\-{pos}"),
        _ => pos,
    };
    let sym = SYMBOLS[symbol.min(SYMBOLS.len() - 1)].1;
    match cat {
        1 => negative(format!("{int}{dec}")),
        2 => {
            let pos = format!("{sym}#,##0{dec}");
            if neg == 0 { format!("{pos};\\-{pos}") } else { negative(pos) }
        }
        3 => {
            let q = "?".repeat(decimals);
            format!("_-{sym}* #,##0{dec}_-;-{sym}* #,##0{dec}_-;_-{sym}* \"-\"{q}_-;_-@_-")
        }
        4 => DATES[kind.min(DATES.len() - 1)].into(),
        5 => TIMES[kind.min(TIMES.len() - 1)].into(),
        6 => format!("0{dec}%"),
        7 => FRACTIONS[kind.min(FRACTIONS.len() - 1)].into(),
        8 => format!("0{dec}E+00"),
        9 => "@".into(),
        _ => "General".into(),
    }
}

/// Categoria, casas, milhar, negativos, símbolo e tipo de um código.
fn detect(code: &str) -> (usize, usize, bool, usize, usize, usize) {
    if NumFmt::parse(code).is_general() {
        return (0, 2, false, 0, 0, 0);
    }
    for (cat, list) in [(4, &DATES[..]), (5, &TIMES[..]), (7, &FRACTIONS[..])] {
        if let Some(k) = list.iter().position(|c| *c == code) {
            return (cat, 2, false, 0, 0, k);
        }
    }
    for cat in [1, 2, 3, 6, 8, 9] {
        for d in 0..=10 {
            for th in [false, true] {
                for neg in 0..3 {
                    for sy in 0..SYMBOLS.len() {
                        if code_of(cat, d, th, neg, sy, 0) == code {
                            return (cat, d, th, neg, sy, 0);
                        }
                    }
                }
            }
        }
    }
    (10, 2, false, 0, 0, 0)
}

/// O que mudar num estilo.
#[derive(Clone, Debug, Default)]
struct Change {
    num_fmt: Option<u16>,
    h: Option<HAlign>,
    v: Option<VAlign>,
    indent: Option<u8>,
    rotation: Option<u8>,
    wrap: Option<bool>,
    shrink: Option<bool>,
    font: Option<String>,
    size: Option<f32>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<Option<u32>>,
    fill: Option<Option<u32>>,
    sides: [Option<BorderSide>; 6],
}

fn solid(c: Option<u32>) -> Fill {
    match c {
        None => Fill::default(),
        Some(rgb) => {
            Fill { pattern: Some("solid".into()), fg: Color::Rgb(0xFF00_0000 | rgb), bg: Color::Indexed(64) }
        }
    }
}

/// Estilo novo (`edges`: a célula está na borda esquerda, direita, de cima e
/// de baixo da área).
fn restyle(st: &mut Styles, base: u16, ch: &Change, edges: [bool; 4]) -> u16 {
    let mut xf = st.xfs.get(base as usize).copied().unwrap_or_default();
    if let Some(f) = ch.num_fmt {
        xf.num_fmt = f;
    }
    let a = &mut xf.alignment;
    if let Some(h) = ch.h {
        a.horizontal = h;
    }
    if let Some(v) = ch.v {
        a.vertical = v;
    }
    if let Some(i) = ch.indent {
        a.indent = i;
        if i > 0 && !matches!(a.horizontal, HAlign::Left | HAlign::Right | HAlign::Distributed) {
            a.horizontal = HAlign::Left;
        }
    }
    if let Some(r) = ch.rotation {
        a.rotation = r;
    }
    if let Some(w) = ch.wrap {
        a.wrap = w;
    }
    if let Some(s) = ch.shrink {
        a.shrink = s;
    }
    let font = ch.font.is_some()
        || ch.size.is_some()
        || ch.bold.is_some()
        || ch.italic.is_some()
        || ch.underline.is_some()
        || ch.strike.is_some()
        || ch.color.is_some();
    if font {
        let mut f = st.fonts.get(xf.font as usize).cloned().unwrap_or_default();
        if f.size == 0.0 {
            f.size = 11.0;
        }
        if f.name.is_empty() {
            f.name = "Calibri".into();
        }
        if let Some(n) = &ch.font {
            f.name = n.clone();
        }
        f.size = ch.size.unwrap_or(f.size);
        f.bold = ch.bold.unwrap_or(f.bold);
        f.italic = ch.italic.unwrap_or(f.italic);
        f.underline = ch.underline.unwrap_or(f.underline);
        f.strike = ch.strike.unwrap_or(f.strike);
        if let Some(c) = ch.color {
            f.color = c.map_or(Color::Auto, |rgb| Color::Rgb(0xFF00_0000 | rgb));
        }
        xf.font = st.intern_font(f);
    }
    if let Some(c) = ch.fill {
        xf.fill = st.intern_fill(solid(c));
    }
    if ch.sides.iter().any(Option::is_some) {
        let mut b: Border = st.borders.get(xf.border as usize).copied().unwrap_or_default();
        let at = |s: Side| ch.sides[SIDES.iter().position(|x| *x == s).unwrap_or(0)];
        let [l, r, t, bo] = edges;
        if let Some(x) = at(if l { Side::Left } else { Side::InsideV }) {
            b.left = x;
        }
        if let Some(x) = at(if r { Side::Right } else { Side::InsideV }) {
            b.right = x;
        }
        if let Some(x) = at(if t { Side::Top } else { Side::InsideH }) {
            b.top = x;
        }
        if let Some(x) = at(if bo { Side::Bottom } else { Side::InsideH }) {
            b.bottom = x;
        }
        xf.border = st.intern_border(b);
    }
    st.derive_xf(base, |x| *x = xf)
}

impl SheetsApp {
    /// Abre "Formatar células" na página `page`.
    pub fn ask_format(&mut self, page: usize) {
        self.commit_edit();
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        let (r, c) = v.cur;
        let cell = sheet.cell(r, c, &doc.strings);
        let style = if cell.style == 0 { v.blank_style(r, c) } else { cell.style };
        let xf = doc.styles.xfs.get(style as usize).copied().unwrap_or_default();
        let font = doc.styles.fonts.get(xf.font as usize).cloned().unwrap_or_default();
        let border = doc.styles.borders.get(xf.border as usize).copied().unwrap_or_default();
        let fill = doc.styles.fills.get(xf.fill as usize).cloned().unwrap_or_default();
        let rgb = |c: Color| doc.styles.rgb(c).map(|x| x & 0xFF_FFFF);
        let code = doc.styles.num_fmt_code(xf.num_fmt).to_string();
        let (cat, dec, thousands, neg, symbol, kind) = detect(&code);
        let mut state = State { date1904: doc.date1904, ..Default::default() };
        match cell.value {
            Value::Number(x) => state.sample = Some(x),
            Value::Text(t) => state.sample_text = t.chars().take(40).collect(),
            _ => {}
        }
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let today = self.today;
        let shown = |codes: &[&str], x: f64| -> Vec<String> {
            codes
                .iter()
                .map(|c| {
                    let mut s = String::new();
                    NumFmt::parse(c).format_number(x, false, &mut s);
                    s
                })
                .collect()
        };
        let mut codes = names(&CUSTOM);
        codes.extend(doc.styles.num_fmts.values().cloned());
        codes.push(code.clone());
        let mut seen = HashSet::new();
        codes.retain(|c| seen.insert(c.clone()));
        let pt: Vec<String> = codes.iter().map(|c| numfmt_pt::to_pt(c)).collect();
        let code_sel = codes.iter().position(|c| *c == code).unwrap_or(0);

        let mut fonts = names(&FONTS);
        let name = if font.name.is_empty() { "Calibri".to_string() } else { font.name.clone() };
        if !fonts.iter().any(|f| f.eq_ignore_ascii_case(&name)) {
            fonts.insert(0, name.clone());
        }
        let font_sel = fonts.iter().position(|f| f.eq_ignore_ascii_case(&name)).unwrap_or(0);
        let size = if font.size > 0.0 { font.size } else { 11.0 };
        let sizes: Vec<String> = SIZES.iter().map(|s| s.to_string()).collect();
        let size_sel = SIZES.iter().position(|s| (*s as f32 - size).abs() < 0.01).unwrap_or(3);
        let face = usize::from(font.italic) + 2 * usize::from(font.bold);

        let areas = v.areas();
        let multi_rows = areas.iter().any(|a| a.2 > a.0);
        let multi_cols = areas.iter().any(|a| a.3 > a.1);
        let has = |s: BorderSide| Some(s.style != BorderStyle::None);
        let on = [
            has(border.left),
            has(border.right),
            has(border.top),
            has(border.bottom),
            if multi_cols { None } else { Some(false) },
            if multi_rows { None } else { Some(false) },
        ];
        let angle = match xf.alignment.rotation {
            255 => 0,
            r if r > 90 => 90 - r as i32,
            r => r as i32,
        };
        let num = |x: f32| format!("{x}").replace('.', ",");
        let sample = |label: &str, key: &'static str| {
            Field::new(
                key,
                label,
                Control::Sample {
                    text: String::new(),
                    color: None,
                    fill: None,
                    bold: font.bold,
                    italic: font.italic,
                },
            )
        };
        let fields = vec![
            // Número.
            Field::list("cat", "Categoria", names(&CATS), cat, 11).left(),
            sample("Amostra", "sample").right(),
            Field::text("dec", "Casas decimais", &dec.to_string()).right(),
            Field::check("thousands", "Usar separador de milhar (.)", thousands).right(),
            Field::choice("symbol", "Símbolo", &SYMBOLS.map(|s| s.0), symbol).right(),
            Field::list("neg", "Números negativos", Vec::new(), neg, 3).right(),
            Field::list("kind", "Tipo", Vec::new(), kind, 7).right(),
            Field::text("code", "Código", &numfmt_pt::to_pt(&code)).right(),
            Field::list("codes", "", pt, code_sel, 6).right(),
            Field::info("help", "").right(),
            // Alinhamento.
            Field::choice(
                "h",
                "Horizontal",
                &HALIGNS.map(|x| x.0),
                HALIGNS.iter().position(|x| x.1 == xf.alignment.horizontal).unwrap_or(0),
            )
            .page(1)
            .left(),
            Field::choice(
                "v",
                "Vertical",
                &VALIGNS.map(|x| x.0),
                VALIGNS.iter().position(|x| x.1 == xf.alignment.vertical).unwrap_or(2),
            )
            .page(1)
            .left(),
            Field::text("indent", "Recuo", &xf.alignment.indent.to_string()).page(1).right(),
            Field::text("angle", "Orientação (graus, -90 a 90)", &angle.to_string()).page(1).right(),
            Field::check("stacked", "Letras empilhadas na vertical", xf.alignment.rotation == 255).page(1),
            Field::heading("Controle de texto").page(1),
            Field::check("wrap", "Quebrar texto automaticamente", xf.alignment.wrap).page(1),
            Field::check("shrink", "Reduzir para caber", xf.alignment.shrink).page(1),
            Field::check("merge", "Mesclar células", v.merges.at(r, c).is_some()).page(1),
            // Fonte.
            Field::list("font", "Fonte", fonts, font_sel, 7).page(2).left(),
            Field::list(
                "face",
                "Estilo da fonte",
                names(&["Regular", "Itálico", "Negrito", "Negrito e itálico"]),
                face,
                4,
            )
            .page(2)
            .right(),
            Field::text("size", "Tamanho", &num(size)).page(2).right(),
            Field::list("sizes", "", sizes, size_sel, 3).page(2).right(),
            Field::check("underline", "Sublinhado", font.underline).page(2).left(),
            Field::check("strike", "Tachado", font.strike).page(2).left(),
            Field::colors(
                "color",
                "Cor",
                "Automática",
                match font.color {
                    Color::Auto => None,
                    c => rgb(c),
                },
            )
            .page(2)
            .right(),
            sample("Amostra", "fsample").page(2),
            // Borda.
            Field::choice("line", "Estilo da linha", &LINES.map(|l| l.0), 0).page(3).left(),
            Field::choice("preset", "Predefinições", &["—", "Nenhuma", "Contorno", "Interna", "Todas"], 0)
                .page(3)
                .left(),
            Field::colors("bcolor", "Cor da linha", "Automática", None).page(3).right(),
            Field::new("borders", "", Control::Borders { on, multi_rows, multi_cols }).page(3),
            Field::info("", "Clique nos lados do quadro para pôr ou tirar a borda.").page(3),
            // Preenchimento.
            Field::colors(
                "fill",
                "Cor do plano de fundo",
                "Sem cor",
                fill.pattern.as_deref().filter(|p| *p != "none").and_then(|_| rgb(fill.fg)),
            )
            .page(4),
            Field::text("fillhex", "Mais cores (RRGGBB)", "").page(4),
            sample("Amostra", "psample").page(4),
        ];
        let mut f = Form::new(FormId::FormatCells, "Formatar células", fields, &["OK", "Cancelar"]);
        f.pages = vec!["Número", "Alinhamento", "Fonte", "Borda", "Preenchimento"];
        f.page = page.min(4);
        f.width = 560.0;
        f.body = 420.0;
        let dates = shown(&DATES, today + 0.5625);
        let times = shown(&TIMES, today + 0.5625);
        let fractions = shown(&FRACTIONS, 0.375);
        self.format_lists = [dates, times, fractions];
        self.format_state = state;
        self.format_refresh(&mut f);
        f.focus_first();
        self.form = Some(f);
    }

    /// Código (do arquivo) escolhido na página de número.
    fn format_code(f: &Form) -> Result<String, String> {
        let cat = f.sel("cat");
        if cat == 10 {
            return numfmt_pt::from_pt(f.text("code"));
        }
        let dec: usize = match f.text("dec").trim().parse() {
            Ok(d) if d <= 30 => d,
            _ => return Err("As casas decimais vão de 0 a 30".into()),
        };
        Ok(code_of(cat, dec, f.on("thousands"), f.sel("neg"), f.sel("symbol"), f.sel("kind")))
    }

    /// Mostra os campos da categoria e as amostras.
    fn format_refresh(&self, f: &mut Form) {
        let cat = f.sel("cat");
        let decimals = matches!(cat, 1 | 2 | 3 | 6 | 8);
        f.show("dec", decimals);
        f.show("thousands", cat == 1);
        f.show("symbol", matches!(cat, 2 | 3));
        f.show("neg", matches!(cat, 1 | 2));
        f.show("kind", matches!(cat, 4 | 5 | 7));
        f.show("code", cat == 10);
        f.show("codes", cat == 10);
        let help = match cat {
            0 => "Sem formato específico de número.",
            4 => "Datas como no calendário brasileiro.",
            9 => "O conteúdo é tratado como texto, mesmo se parecer um número.",
            10 => {
                "Use # e 0 para dígitos, . e , como no Brasil; dd/mm/aaaa, hh:mm; [Vermelho]; até 4 partes com ;"
            }
            _ => "",
        };
        if let Some(h) = f.field("help") {
            h.label = help.into();
            h.show = !help.is_empty();
        }
        let dec: usize = f.text("dec").trim().parse().unwrap_or(2).min(30);
        let sy = f.sel("symbol");
        let th = f.on("thousands");
        let negs: Vec<String> = (0..3)
            .map(|n| {
                let mut s = String::new();
                NumFmt::parse(&code_of(cat, dec, th, n, sy, 0)).format_number(-1234.1, false, &mut s);
                if n > 0 { format!("{s} (vermelho)") } else { s }
            })
            .collect();
        let lists = [(4, 0), (5, 1), (7, 2)];
        let kinds = lists.iter().find(|l| l.0 == cat).map(|l| self.format_lists[l.1].clone());
        if let Some(Field { control: Control::List { items, .. }, .. }) = f.field("neg") {
            *items = negs;
        }
        if let (Some(k), Some(Field { control: Control::List { items, sel, scroll, .. }, .. })) =
            (kinds, f.field("kind"))
        {
            *sel = (*sel).min(k.len() - 1);
            *scroll = (*scroll).min(*sel);
            *items = k;
        }
        // Amostra do número.
        let st = &self.format_state;
        let (text, red) = match Self::format_code(f) {
            Ok(code) => {
                let fmt = NumFmt::parse(&code);
                let mut s = String::new();
                match st.sample {
                    Some(x) => {
                        let r = fmt.format_number(x, st.date1904, &mut s);
                        (if r.overflow { "########".into() } else { s }, r.color.map(|c| c & 0xFF_FFFF))
                    }
                    None => {
                        if fmt.format_text(&st.sample_text, &mut s).is_none() {
                            s = st.sample_text.clone();
                        }
                        (s, None)
                    }
                }
            }
            Err(e) => (e, Some(0xC5221F)),
        };
        if let Some(Field { control: Control::Sample { text: t, color, .. }, .. }) = f.field("sample") {
            *t = text;
            *color = red;
        }
        let (face, ink, bg) = (f.sel("face"), f.color("color"), f.color("fill"));
        let name = match f.index("font").map(|i| &f.fields[i].control) {
            Some(Control::List { items, sel, .. }) => items.get(*sel).cloned().unwrap_or_default(),
            _ => String::new(),
        };
        let label = format!("{name} {} — AaBbCcÇç 123", f.text("size"));
        for key in ["fsample", "psample"] {
            if let Some(Field { control: Control::Sample { text, color, fill, bold, italic }, .. }) =
                f.field(key)
            {
                *text = label.clone();
                *color = ink;
                *fill = bg;
                *italic = face & 1 == 1;
                *bold = face & 2 == 2;
            }
        }
    }

    /// Um campo da caixa mudou.
    pub fn format_form_changed(&mut self, f: &mut Form, i: usize) {
        let Some(key) = f.fields.get(i).map(|x| x.key) else { return };
        self.format_state.touched.insert(key);
        match key {
            "codes" => {
                if let Control::List { items, sel, .. } = &f.fields[i].control {
                    let t = items.get(*sel).cloned().unwrap_or_default();
                    f.set_text("code", &t);
                }
            }
            "cat" => {
                // Personalizado parte do código de antes.
                if f.sel("cat") == 10
                    && let Ok(c) = Self::format_code(f)
                {
                    f.set_text("code", &numfmt_pt::to_pt(&c));
                }
                if f.sel("cat") == 2 || f.sel("cat") == 3 {
                    f.set_sel("neg", 0);
                }
            }
            "sizes" => {
                if let Control::List { items, sel, .. } = &f.fields[i].control {
                    let t = items.get(*sel).cloned().unwrap_or_default();
                    f.set_text("size", &t);
                    self.format_state.touched.insert("size");
                }
            }
            "preset" => {
                let k = f.sel("preset");
                f.set_sel("preset", 0);
                let side = match k {
                    1 => BorderSide::default(),
                    _ => Self::border_side(f),
                };
                let which: &[Side] = match k {
                    2 => &[Side::Left, Side::Right, Side::Top, Side::Bottom],
                    3 => &[Side::InsideV, Side::InsideH],
                    1 | 4 => &SIDES,
                    _ => &[],
                };
                let list = which.to_vec();
                for s in list {
                    self.set_side(f, s, side);
                }
            }
            "fillhex" => {
                let t = f.text("fillhex").trim().trim_start_matches('#').to_string();
                if let (6, Ok(c)) = (t.len(), u32::from_str_radix(&t, 16))
                    && let Some(Field { control: Control::Colors { sel, .. }, .. }) = f.field("fill")
                {
                    *sel = Some(c);
                    self.format_state.touched.insert("fill");
                }
            }
            _ => {}
        }
        self.format_refresh(f);
    }

    fn border_side(f: &Form) -> BorderSide {
        BorderSide {
            style: LINES[f.sel("line").min(LINES.len() - 1)].1,
            color: f.color("bcolor").map_or(Color::Auto, |c| Color::Rgb(0xFF00_0000 | c)),
        }
    }

    fn set_side(&mut self, f: &mut Form, side: Side, b: BorderSide) {
        let Some(k) = SIDES.iter().position(|s| *s == side) else { return };
        if let Some(Field { control: Control::Borders { on, multi_rows, multi_cols }, .. }) =
            f.field("borders")
        {
            let usable = match side {
                Side::InsideV => *multi_cols,
                Side::InsideH => *multi_rows,
                _ => true,
            };
            if !usable {
                return;
            }
            on[k] = Some(b.style != BorderStyle::None);
            self.format_state.sides[k] = Some(b);
        }
    }

    /// Clique num lado do quadro: põe a linha escolhida, ou tira.
    pub fn format_form_border(&mut self, f: &mut Form, _i: usize, side: Side) {
        let Some(k) = SIDES.iter().position(|s| *s == side) else { return };
        let on = match f.field("borders").map(|x| &x.control) {
            Some(Control::Borders { on, .. }) => on[k] == Some(true),
            _ => return,
        };
        let new = Self::border_side(f);
        // Já tinha: sai, a menos que o estilo escolhido seja outro.
        let same = self.format_state.sides[k].is_none_or(|s| s == new);
        let b = if on && same { BorderSide::default() } else { new };
        self.set_side(f, side, b);
    }

    /// OK: aplica o que foi mexido nas áreas selecionadas.
    pub fn format_form_apply(&mut self, f: &Form) -> Result<(), String> {
        let touched = self.format_state.touched.clone();
        let t = |k: &str| touched.contains(k);
        let mut ch = Change { sides: self.format_state.sides, ..Default::default() };
        let number = ["cat", "dec", "thousands", "neg", "symbol", "kind", "code", "codes"];
        let code = if number.iter().any(|k| t(k)) { Some(Self::format_code(f)?) } else { None };
        if t("h") {
            ch.h = Some(HALIGNS[f.sel("h").min(HALIGNS.len() - 1)].1);
        }
        if t("v") {
            ch.v = Some(VALIGNS[f.sel("v").min(VALIGNS.len() - 1)].1);
        }
        if t("indent") {
            let i: u8 = f.text("indent").trim().parse().map_err(|_| "O recuo vai de 0 a 250")?;
            if i > 250 {
                return Err("O recuo vai de 0 a 250".into());
            }
            ch.indent = Some(i);
        }
        if t("stacked") || t("angle") {
            let a: i32 = f.text("angle").trim().parse().map_err(|_| "A orientação vai de -90 a 90 graus")?;
            if !(-90..=90).contains(&a) {
                return Err("A orientação vai de -90 a 90 graus".into());
            }
            ch.rotation = Some(if f.on("stacked") {
                255
            } else if a < 0 {
                (90 - a) as u8
            } else {
                a as u8
            });
        }
        ch.wrap = t("wrap").then(|| f.on("wrap"));
        ch.shrink = t("shrink").then(|| f.on("shrink"));
        if t("font")
            && let Some(Control::List { items, sel, .. }) = f.index("font").map(|i| &f.fields[i].control)
        {
            ch.font = items.get(*sel).cloned();
        }
        if t("face") {
            ch.italic = Some(f.sel("face") & 1 == 1);
            ch.bold = Some(f.sel("face") & 2 == 2);
        }
        if t("size") {
            let x: f32 =
                f.text("size").trim().replace(',', ".").parse().map_err(|_| "O tamanho vai de 1 a 409")?;
            if !(1.0..=409.0).contains(&x) {
                return Err("O tamanho vai de 1 a 409".into());
            }
            ch.size = Some(x);
        }
        ch.underline = t("underline").then(|| f.on("underline"));
        ch.strike = t("strike").then(|| f.on("strike"));
        ch.color = t("color").then(|| f.color("color"));
        ch.fill = t("fill").then(|| f.color("fill"));
        let merge = t("merge").then(|| f.on("merge"));

        let Some(areas) = self.sel_areas(2_000_000) else { return Err(self.message.clone()) };
        let Some(doc) = self.doc.as_mut() else { return Err(String::new()) };
        let active = doc.active;
        let mut styles = doc.styles.clone();
        let st = Arc::make_mut(&mut styles);
        if let Some(c) = &code {
            ch.num_fmt = Some(st.intern_num_fmt(c));
        }
        let Some((sheet, v)) = doc.current() else { return Err(String::new()) };
        let mut list: Vec<(u32, u32, u16)> = Vec::new();
        let mut cache: HashMap<(u16, u8), u16> = HashMap::new();
        for (r0, c0, r1, c1) in &areas {
            for r in *r0..=*r1 {
                for c in *c0..=*c1 {
                    let s = sheet.raw(r, c).style;
                    let base = if s == 0 { v.blank_style(r, c) } else { s };
                    let edges = [c == *c0, c == *c1, r == *r0, r == *r1];
                    let key = (base, edges.iter().enumerate().fold(0u8, |m, (i, &e)| m | ((e as u8) << i)));
                    let new = *cache.entry(key).or_insert_with(|| restyle(st, base, &ch, edges));
                    if new != s {
                        list.push((r, c, new));
                    }
                }
            }
        }
        let mut cmds = Vec::new();
        if !list.is_empty() {
            cmds.push(Command::SetStyles(list));
        }
        if let Some(on) = merge {
            let (mut add, mut remove) = (Vec::new(), Vec::new());
            for a in &areas {
                let a = range_of(*a);
                remove.extend(
                    v.merges
                        .in_rows(a.first_row, a.last_row)
                        .filter(|m| duck_core::cells::intersect(m, &a).is_some())
                        .copied(),
                );
                if on && crate::fill::cells(&a) > 1 {
                    add.push(a);
                }
            }
            if add != remove {
                cmds.push(Command::Merges { add, remove });
            }
        }
        doc.looks = Arc::new(Looks::new(&styles));
        doc.styles = styles;
        self.font_key = (0, 0);
        if cmds.is_empty() {
            return Ok(());
        }
        if !self.run_step(vec![(active, Command::Group(cmds))]) {
            return Err(self.message.clone());
        }
        self.message = "Formato aplicado".into();
        Ok(())
    }

    /// `--edit formatar-celulas=campo=valor|campo=valor>A1:B2;D1` (os campos
    /// da caixa; ver `ask_format`). Ex.: `codigo=dd/mm/aaaa|fonte=Arial`.
    pub fn scripted_format(&mut self, e: &str) -> bool {
        let Some(arg) = e.strip_prefix("formatar-celulas=") else { return false };
        let Some((list, sel)) = arg.rsplit_once('>') else {
            eprintln!("--edit: use formatar-celulas=campo=valor>A1:B2");
            return true;
        };
        if !self.scripted_select(sel) {
            eprintln!("--edit: intervalo inválido: {sel}");
            return true;
        }
        self.ask_format(0);
        let Some(mut f) = self.form.take() else { return true };
        for part in list.split('|') {
            let (k, v) = part.split_once('=').unwrap_or((part, "1"));
            if !self.scripted_field(&mut f, k, v) {
                eprintln!("--edit formatar-celulas: campo ou valor inválido: {part}");
            }
        }
        let m = match self.format_form_apply(&f) {
            Ok(()) => self.message.clone(),
            Err(e) => e,
        };
        eprintln!("--edit formatar-celulas: {m}");
        true
    }

    /// Mexe num campo da caixa pelo nome em português.
    fn scripted_field(&mut self, f: &mut Form, k: &str, v: &str) -> bool {
        let find = |list: &[&str]| list.iter().position(|x| x.to_lowercase() == v.to_lowercase());
        let key = match k {
            "codigo" => {
                f.set_sel("cat", 10);
                f.set_text("code", v);
                "code"
            }
            "categoria" => {
                let Some(i) = find(&CATS) else { return false };
                f.set_sel("cat", i);
                "cat"
            }
            "casas" => {
                f.set_text("dec", v);
                "dec"
            }
            "horizontal" => {
                let Some(i) = HALIGNS.iter().position(|x| x.0.to_lowercase().starts_with(&v.to_lowercase()))
                else {
                    return false;
                };
                f.set_sel("h", i);
                "h"
            }
            "vertical" => {
                let Some(i) = find(&VALIGNS.map(|x| x.0)) else { return false };
                f.set_sel("v", i);
                "v"
            }
            "recuo" => {
                f.set_text("indent", v);
                "indent"
            }
            "orientacao" => {
                f.set_text("angle", v);
                "angle"
            }
            "tamanho" => {
                f.set_text("size", v);
                "size"
            }
            "estilo" => {
                let Some(i) = find(&["Regular", "Itálico", "Negrito", "Negrito e itálico"]) else {
                    return false;
                };
                f.set_sel("face", i);
                "face"
            }
            "fonte" => {
                let Some(Control::List { items, .. }) = f.index("font").map(|i| &f.fields[i].control) else {
                    return false;
                };
                let Some(i) = items.iter().position(|x| x.eq_ignore_ascii_case(v)) else { return false };
                f.set_sel("font", i);
                "font"
            }
            "cor" | "fundo" => {
                let key = if k == "cor" { "color" } else { "fill" };
                let Some(Field { control: Control::Colors { sel, .. }, .. }) = f.field(key) else {
                    return false;
                };
                *sel = u32::from_str_radix(v, 16).ok();
                key
            }
            "quebrar" | "reduzir" | "mesclar" | "sublinhado" | "tachado" | "empilhado" | "milhar" => {
                let key = match k {
                    "quebrar" => "wrap",
                    "reduzir" => "shrink",
                    "mesclar" => "merge",
                    "sublinhado" => "underline",
                    "tachado" => "strike",
                    "milhar" => "thousands",
                    _ => "stacked",
                };
                let Some(Field { control: Control::Check(on), .. }) = f.field(key) else { return false };
                *on = v != "0";
                key
            }
            "linha" => {
                let Some(i) = find(&LINES.map(|l| l.0)) else { return false };
                f.set_sel("line", i);
                return true;
            }
            "corborda" => {
                let Some(Field { control: Control::Colors { sel, .. }, .. }) = f.field("bcolor") else {
                    return false;
                };
                *sel = u32::from_str_radix(v, 16).ok();
                return true;
            }
            "borda" => {
                let Some(i) = find(&["", "nenhuma", "contorno", "interna", "todas"]) else { return false };
                f.set_sel("preset", i);
                "preset"
            }
            _ => return false,
        };
        let Some(i) = f.index(key) else { return false };
        self.format_form_changed(f, i);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, select, style};

    #[test]
    fn codes_and_categories() {
        for (code, cat) in [
            ("General", 0),
            ("0.00", 1),
            ("#,##0.000", 1),
            ("#,##0;[Red]\\-#,##0", 1),
            ("\"R$ \"#,##0.00;\\-\"R$ \"#,##0.00", 2),
            ("_-\"R$ \"* #,##0.00_-;-\"R$ \"* #,##0.00_-;_-\"R$ \"* \"-\"??_-;_-@_-", 3),
            ("dd/mm/yyyy", 4),
            ("h:mm:ss", 5),
            ("0.00%", 6),
            ("# ??/??", 7),
            ("0.00E+00", 8),
            ("@", 9),
            ("0.0\" kg\"", 10),
        ] {
            let d = detect(code);
            assert_eq!(d.0, cat, "{code}");
            if cat != 10 {
                assert_eq!(code_of(d.0, d.1, d.2, d.3, d.4, d.5), code);
            }
        }
    }

    fn run(a: &mut SheetsApp, e: &str) {
        assert!(a.scripted_format(e));
    }

    #[test]
    fn number_alignment_font_fill() {
        let mut a = app(&[("P", &[("A1", "1234,5"), ("B1", "x")])]);
        run(
            &mut a,
            "formatar-celulas=codigo=#.##0,00\" kg\"|fonte=Arial|tamanho=14|estilo=Negrito|cor=FF0000|fundo=FFFF00|horizontal=Centro|orientacao=-45|quebrar=1>A1:B1",
        );
        let xf = style(&a, 0, "A1");
        let st = a.doc.as_ref().unwrap().styles.clone();
        assert_eq!(st.num_fmt_code(xf.num_fmt), "#,##0.00\" kg\"");
        let f = &st.fonts[xf.font as usize];
        assert_eq!(
            (f.name.as_str(), f.size, f.bold, f.color),
            ("Arial", 14.0, true, Color::Rgb(0xFFFF_0000))
        );
        assert_eq!(st.fills[xf.fill as usize].fg, Color::Rgb(0xFFFF_FF00));
        assert_eq!(
            (xf.alignment.horizontal, xf.alignment.rotation, xf.alignment.wrap),
            (HAlign::Center, 135, true)
        );
        assert_eq!(style(&a, 0, "B1"), xf);
        a.undo();
        assert_eq!(style(&a, 0, "A1"), Default::default());
        a.redo();
        assert_eq!(style(&a, 0, "B1"), xf);
        // Só o que foi mexido muda.
        run(&mut a, "formatar-celulas=estilo=Regular>A1:B1");
        let now = style(&a, 0, "A1");
        let st = a.doc.as_ref().unwrap().styles.clone();
        assert!(!st.fonts[now.font as usize].bold);
        assert_eq!(
            (now.num_fmt, now.fill, st.fonts[now.font as usize].name.as_str()),
            (xf.num_fmt, xf.fill, "Arial")
        );
    }

    #[test]
    fn borders_by_side() {
        let mut a = app(&[("P", &[])]);
        run(&mut a, "formatar-celulas=linha=Média|corborda=0000FF|borda=contorno>B2:D4");
        let b = |a: &SheetsApp, c: &str| {
            let x = style(a, 0, c).border;
            a.doc.as_ref().unwrap().styles.borders[x as usize]
        };
        assert_eq!(b(&a, "B2").left.style, BorderStyle::Medium);
        assert_eq!(b(&a, "B2").left.color, Color::Rgb(0xFF00_00FF));
        assert_eq!((b(&a, "B2").right.style, b(&a, "C3").top.style), (BorderStyle::None, BorderStyle::None));
        assert_eq!(b(&a, "D4").bottom.style, BorderStyle::Medium);
        run(&mut a, "formatar-celulas=linha=Pontilhada|borda=interna>B2:D4");
        assert_eq!(
            (b(&a, "B2").right.style, b(&a, "C3").top.style),
            (BorderStyle::Dotted, BorderStyle::Dotted)
        );
        assert_eq!(b(&a, "B2").left.style, BorderStyle::Medium);
        run(&mut a, "formatar-celulas=borda=nenhuma>B2:D4");
        assert!(b(&a, "C3").is_empty() && b(&a, "B2").is_empty());
        // O quadro: clicar põe, clicar de novo tira.
        select(&mut a, "F1");
        a.ask_format(3);
        let mut f = a.form.take().unwrap();
        a.format_form_border(&mut f, 0, Side::Top);
        assert_eq!(a.format_state.sides[2].unwrap().style, BorderStyle::Thin);
        a.format_form_border(&mut f, 0, Side::Top);
        assert_eq!(a.format_state.sides[2].unwrap().style, BorderStyle::None);
        // Sem várias colunas, não há borda interna.
        a.format_form_border(&mut f, 0, Side::InsideV);
        assert_eq!(a.format_state.sides[4], None);
    }

    #[test]
    fn errors_keep_the_box_open() {
        let mut a = app(&[("P", &[("A1", "1")])]);
        select(&mut a, "A1");
        a.ask_format(0);
        let mut f = a.form.take().unwrap();
        f.set_sel("cat", 10);
        f.set_text("code", "0\"aberto");
        a.format_form_changed(&mut f, 0);
        assert!(a.format_form_apply(&f).is_err());
        f.set_text("code", "0,0");
        assert!(a.format_form_apply(&f).is_ok());
        assert_eq!(a.doc.as_ref().unwrap().styles.num_fmt_code(style(&a, 0, "A1").num_fmt), "0.0");
        a.ask_format(1);
        let mut f = a.form.take().unwrap();
        f.set_text("angle", "120");
        let i = f.index("angle").unwrap();
        a.format_form_changed(&mut f, i);
        assert!(a.format_form_apply(&f).is_err());
    }

    #[test]
    fn several_areas_and_merge() {
        let mut a = app(&[("P", &[])]);
        run(&mut a, "formatar-celulas=categoria=Porcentagem|casas=1>A1:A2;C1");
        let st = |a: &SheetsApp, c: &str| {
            let id = style(a, 0, c).num_fmt;
            a.doc.as_ref().unwrap().styles.num_fmt_code(id).to_string()
        };
        assert_eq!(
            (st(&a, "A2"), st(&a, "C1"), st(&a, "B1")),
            ("0.0%".into(), "0.0%".into(), "General".into())
        );
        run(&mut a, "formatar-celulas=mesclar=1>E1:F2");
        assert_eq!(a.doc.as_ref().unwrap().current().unwrap().0.layout.merges.len(), 1);
        a.undo();
        assert!(a.doc.as_ref().unwrap().current().unwrap().0.layout.merges.is_empty());
    }
}
