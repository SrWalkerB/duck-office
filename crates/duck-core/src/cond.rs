//! Formatação condicional: as regras lidas do arquivo e a avaliação por célula.
//!
//! A avaliação é feita na hora de desenhar, só para as células visíveis. O que
//! depende do intervalo inteiro (mínimo e máximo das escalas de cor, os N
//! maiores, a média, os valores repetidos) é calculado uma vez por regra, na
//! primeira vez que uma célula dela aparece, e guardado em [`CfState`].
//!
//! Fórmulas das regras (`expression`, `cellIs`, limites `formula`) passam por
//! um avaliador pequeno: números, textos, lógicos, referências a células da
//! própria aba, operadores e algumas funções (`AND`, `OR`, `NOT`, `IF`,
//! `IS*`, `LEN`, `MOD`, `ROW`, `COLUMN`, `TODAY`...). Uma regra com algo fora
//! disso (outra aba, intervalos, funções desconhecidas) simplesmente não se
//! aplica, sem errar o resto.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::{CellRange, Color, Dxf, Sheet, StringTable, Value};

/// Uma regra (`<cfRule>`) com os intervalos do bloco a que pertence.
#[derive(Clone, Debug, PartialEq)]
pub struct CfRule {
    pub ranges: Vec<CellRange>,
    /// Menor = vale antes.
    pub priority: i32,
    pub stop: bool,
    /// Índice em [`crate::Styles::dxfs`].
    pub dxf: Option<u32>,
    /// Formato da própria regra (regras da extensão do Excel 2010).
    pub own: Option<Box<Dxf>>,
    /// Ligação entre a regra comum e a metade dela na extensão (só na leitura).
    pub ext: Option<Box<ExtId>>,
    pub kind: CfKind,
}

/// `id` em comum das duas metades de uma regra.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtId {
    pub id: String,
    /// A metade que está na extensão (`<x14:cfRule>`).
    pub extension: bool,
}

/// Junta as metades (barras de dados: o arquivo guarda a versão de 2007 na
/// regra comum e o resto na extensão).
pub fn join_extensions(rules: &mut Vec<CfRule>) {
    if rules.iter().all(|r| r.ext.is_none()) {
        return;
    }
    let mut halves: Vec<(String, CfRule)> = Vec::new();
    let mut k = 0;
    while k < rules.len() {
        if rules[k].ext.as_ref().is_some_and(|e| e.extension) {
            let mut r = rules.remove(k);
            let id = r.ext.take().map(|e| e.id).unwrap_or_default();
            halves.push((id, r));
        } else {
            k += 1;
        }
    }
    for r in rules.iter_mut() {
        let Some(e) = r.ext.take() else { continue };
        let Some(i) = halves.iter().position(|h| h.0 == e.id) else { continue };
        let half = halves.remove(i).1;
        if let (
            CfKind::DataBar { color, show_value, .. },
            CfKind::DataBar { min, max, color: c2, show_value: s2, look },
        ) = (&r.kind, half.kind)
        {
            // A cor da barra fica na regra comum, se a extensão não disser.
            let color = if c2 == Color::Auto { *color } else { c2 };
            r.kind = CfKind::DataBar { min, max, color, show_value: *show_value && s2, look };
        }
    }
    // As que só existem na extensão (ícones personalizados, fórmulas com
    // outras abas...) são regras inteiras. Sem prioridade é metade sem par.
    rules.extend(halves.into_iter().map(|h| h.1).filter(|r| r.priority != i32::MAX));
}

/// Aparência de uma barra de dados (a de 2007 é o [`Default`]).
#[derive(Clone, Debug, PartialEq)]
pub struct BarLook {
    /// Menor e maior comprimento, em fração da célula.
    pub min_len: f32,
    pub max_len: f32,
    pub gradient: bool,
    pub border: Option<Color>,
    /// Cor e contorno das barras negativas (`None` = iguais às positivas).
    pub negative: Option<Color>,
    pub negative_border: Option<Color>,
    /// Onde fica o zero quando há negativos.
    pub axis: BarAxis,
    pub axis_color: Option<Color>,
    /// Da direita para a esquerda.
    pub rtl: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAxis {
    /// Proporcional aos limites.
    Auto,
    Middle,
    /// Sem eixo: as negativas vão para o mesmo lado.
    None,
}

impl Default for BarLook {
    fn default() -> Self {
        Self {
            min_len: 0.1,
            max_len: 0.9,
            gradient: true,
            border: None,
            negative: None,
            negative_border: None,
            axis: BarAxis::None,
            axis_color: None,
            rtl: false,
        }
    }
}

/// Uma barra de dados numa célula: de `from` a `to` (frações da largura; a cor
/// é mais forte em `from`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub from: f32,
    pub to: f32,
    pub color: Color,
    pub gradient: bool,
    pub border: Option<Color>,
    /// Linha do zero.
    pub axis: Option<(f32, Color)>,
}

impl BarLook {
    /// A barra do valor `x` entre os limites `lo` e `hi`.
    fn bar(&self, x: f64, lo: f64, hi: f64, color: Color) -> Bar {
        let (lo, hi) = if lo > hi { (hi, lo) } else { (lo, hi) };
        let neg = x < 0.0;
        let fill = if neg { self.negative.unwrap_or(color) } else { color };
        let border = match (self.border, neg) {
            (Some(b), true) => Some(self.negative_border.unwrap_or(b)),
            (b, _) => b,
        };
        let unit = |v: f64, full: f64| if full > 0.0 { (v / full).clamp(0.0, 1.0) as f32 } else { 1.0 };
        let (mut from, mut axis) = (0.0, None);
        let mut to;
        if lo < 0.0 && self.axis != BarAxis::None {
            let (zero, left, right) = match self.axis {
                BarAxis::Middle => {
                    let m = lo.abs().max(hi.abs());
                    (0.5, m, m)
                }
                _ if hi <= 0.0 => (1.0, -lo, 0.0),
                _ => ((-lo / (hi - lo)) as f32, -lo, hi),
            };
            from = zero;
            to = match neg {
                true => zero - zero * unit(-x, left) * self.max_len,
                false if right > 0.0 => zero + (1.0 - zero) * unit(x, right) * self.max_len,
                false => zero,
            };
            axis = Some((zero, self.axis_color.unwrap_or(Color::Rgb(0xFF00_0000))));
        } else {
            let t = if hi > lo { unit(x - lo, hi - lo) } else { 1.0 };
            to = self.min_len + (self.max_len - self.min_len) * t;
        }
        if self.rtl {
            (from, to) = (1.0 - from, 1.0 - to);
            axis = axis.map(|(a, c)| (1.0 - a, c));
        }
        Bar { from, to, color: fill, gradient: self.gradient, border, axis }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    Between,
    NotBetween,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextOp {
    Contains,
    NotContains,
    Begins,
    Ends,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Today,
    Yesterday,
    Tomorrow,
    Last7Days,
    ThisWeek,
    LastWeek,
    NextWeek,
    ThisMonth,
    LastMonth,
    NextMonth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfvoKind {
    Min,
    Max,
    /// O menor valor, ou zero se todos são positivos (barras da extensão).
    AutoMin,
    /// O maior valor, ou zero se todos são negativos.
    AutoMax,
    Num,
    Percent,
    Percentile,
    Formula,
}

/// Um limite de escala de cor, barra de dados ou conjunto de ícones.
#[derive(Clone, Debug, PartialEq)]
pub struct Cfvo {
    pub kind: CfvoKind,
    /// Número ou fórmula, como no arquivo.
    pub val: String,
    /// Ícones: o limite vale com `>=` (senão `>`).
    pub gte: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CfKind {
    CellIs {
        op: CfOp,
        f1: String,
        f2: Option<String>,
    },
    Expression(String),
    ColorScale(Vec<(Cfvo, Color)>),
    DataBar {
        min: Cfvo,
        max: Cfvo,
        color: Color,
        show_value: bool,
        look: Box<BarLook>,
    },
    IconSet {
        set: String,
        cfvos: Vec<Cfvo>,
        /// Ícones personalizados (conjunto e posição), um por limite.
        icons: Vec<(String, u8)>,
        reverse: bool,
        show_value: bool,
    },
    Top10 {
        rank: u32,
        percent: bool,
        bottom: bool,
    },
    AboveAverage {
        above: bool,
        equal: bool,
        std_dev: u32,
    },
    /// `true` = valores repetidos; `false` = valores únicos.
    Duplicate(bool),
    Text {
        op: TextOp,
        text: String,
    },
    /// `true` = vazias; `false` = não vazias.
    Blanks(bool),
    /// `true` = com erro; `false` = sem erro.
    Errors(bool),
    TimePeriod(Period),
    Unsupported,
}

impl CfRule {
    pub fn covers(&self, row: u32, col: u32) -> bool {
        self.ranges.iter().any(|r| r.contains(row, col))
    }

    /// A regra toca o retângulo `[r0, r1] × [c0, c1]`.
    pub fn touches(&self, r0: u32, r1: u32, c0: u32, c1: u32) -> bool {
        self.ranges
            .iter()
            .any(|r| r.last_row >= r0 && r.first_row <= r1 && r.last_col >= c0 && r.first_col <= c1)
    }

    /// Primeira célula do primeiro intervalo: base das referências relativas.
    fn base(&self) -> (u32, u32) {
        self.ranges.first().map_or((0, 0), |r| (r.first_row, r.first_col))
    }
}

/// O que uma regra faz numa célula.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CfApply<'a> {
    /// Formato diferencial (`dxf`).
    Dxf(u32),
    /// Formato diferencial da própria regra.
    Own(&'a Dxf),
    /// Escala de cor: entre `lo` e `hi`, na posição `t` (0–1).
    Scale {
        lo: Color,
        hi: Color,
        t: f32,
    },
    Bar {
        bar: Bar,
        show_value: bool,
    },
    /// Ícone `idx` (0 = o mais baixo) de um conjunto de `n`.
    Icon {
        set: &'a str,
        idx: usize,
        n: usize,
        show_value: bool,
    },
}

/// Dados de uma regra calculados uma vez (ver o módulo).
#[derive(Default)]
pub struct CfState {
    stats: OnceLock<Stats>,
}

#[derive(Default)]
struct Stats {
    /// Números do intervalo, em ordem.
    sorted: Vec<f64>,
    mean: f64,
    std: f64,
    /// Contagem de cada valor (duplicados/únicos).
    counts: HashMap<Key, u32>,
    /// Fórmulas já interpretadas (`None` = fora do que o avaliador entende).
    f1: Option<Expr>,
    f2: Option<Expr>,
    /// Limites das escalas, barras e ícones, já resolvidos.
    limits: Vec<f64>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Num(u64),
    Text(String),
    Bool(bool),
}

/// Data de hoje como número de série do Excel (UTC).
pub fn today_serial() -> f64 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    (secs / 86_400) as f64 + 25_569.0
}

/// Onde e com o que a regra é avaliada.
pub struct CfCtx<'a> {
    pub sheet: &'a Sheet,
    pub shared: &'a StringTable,
    pub today: f64,
}

impl CfCtx<'_> {
    /// Avalia `rule` na célula `(row, col)` de valor `value`.
    pub fn apply<'r>(
        &self,
        rule: &'r CfRule,
        state: &CfState,
        row: u32,
        col: u32,
        value: Value,
    ) -> Option<CfApply<'r>> {
        let st = state.stats.get_or_init(|| self.stats(rule));
        let dxf = |hit: bool| match &rule.own {
            _ if !hit => None,
            Some(d) => Some(CfApply::Own(d)),
            None => rule.dxf.map(CfApply::Dxf),
        };
        match &rule.kind {
            CfKind::CellIs { op, .. } => {
                let (base_r, base_c) = rule.base();
                let at = Pos { row, col, dr: row as i64 - base_r as i64, dc: col as i64 - base_c as i64 };
                let a = self.eval(st.f1.as_ref()?, at)?;
                let v = V::from_value(value);
                let hit = match op {
                    CfOp::Between | CfOp::NotBetween => {
                        let b = self.eval(st.f2.as_ref()?, at)?;
                        let (lo, hi) =
                            if compare(&a, &b) == std::cmp::Ordering::Greater { (b, a) } else { (a, b) };
                        let inside = compare(&v, &lo).is_ge() && compare(&v, &hi).is_le();
                        inside == (*op == CfOp::Between)
                    }
                    op => {
                        // Célula vazia não entra em "é igual a" texto/número (como o Excel trata "").
                        let o = compare(&v, &a);
                        match op {
                            CfOp::Lt => o.is_lt(),
                            CfOp::Le => o.is_le(),
                            CfOp::Gt => o.is_gt(),
                            CfOp::Ge => o.is_ge(),
                            CfOp::Eq => o.is_eq(),
                            _ => o.is_ne(),
                        }
                    }
                };
                dxf(hit)
            }
            CfKind::Expression(_) => {
                let (base_r, base_c) = rule.base();
                let at = Pos { row, col, dr: row as i64 - base_r as i64, dc: col as i64 - base_c as i64 };
                dxf(self.eval(st.f1.as_ref()?, at)?.truthy()?)
            }
            CfKind::ColorScale(stops) => {
                let x = number(value)?;
                if st.sorted.is_empty() || stops.len() < 2 {
                    return None;
                }
                let lim = &st.limits;
                let k = (1..lim.len()).find(|&k| x <= lim[k]).unwrap_or(lim.len() - 1);
                let (a, b) = (lim[k - 1], lim[k]);
                let t = if b > a {
                    ((x - a) / (b - a)).clamp(0.0, 1.0)
                } else if x >= b {
                    1.0
                } else {
                    0.0
                };
                Some(CfApply::Scale { lo: stops[k - 1].1, hi: stops[k].1, t: t as f32 })
            }
            CfKind::DataBar { color, show_value, look, .. } => {
                let x = number(value)?;
                let (lo, hi) = (*st.limits.first()?, *st.limits.get(1)?);
                Some(CfApply::Bar { bar: look.bar(x, lo, hi, *color), show_value: *show_value })
            }
            CfKind::IconSet { set, cfvos, icons, reverse, show_value } => {
                let x = number(value)?;
                let n = cfvos.len().max(1);
                let mut idx = 0;
                for (k, c) in cfvos.iter().enumerate().skip(1) {
                    let lim = *st.limits.get(k)?;
                    if (c.gte && x >= lim) || (!c.gte && x > lim) {
                        idx = k;
                    }
                }
                if *reverse {
                    idx = n - 1 - idx;
                }
                if let Some((set, k)) = icons.get(idx) {
                    // `3Triangles` tem 3 ícones; `NoIcons`, nenhum.
                    let n = set.bytes().next().filter(u8::is_ascii_digit).map_or(0, |d| (d - b'0') as usize);
                    return Some(CfApply::Icon { set, idx: *k as usize, n, show_value: *show_value });
                }
                Some(CfApply::Icon { set, idx, n, show_value: *show_value })
            }
            CfKind::Top10 { rank, percent, bottom } => {
                let x = number(value)?;
                let len = st.sorted.len();
                if len == 0 {
                    return None;
                }
                let k = if *percent {
                    ((len as f64 * *rank as f64 / 100.0).floor() as usize).max(1)
                } else {
                    *rank as usize
                };
                let k = k.clamp(1, len);
                let hit = if *bottom { x <= st.sorted[k - 1] } else { x >= st.sorted[len - k] };
                dxf(hit)
            }
            CfKind::AboveAverage { above, equal, std_dev } => {
                let x = number(value)?;
                if st.sorted.is_empty() {
                    return None;
                }
                let d = *std_dev as f64 * st.std;
                let hit = match (above, equal) {
                    (true, false) => x > st.mean + d,
                    (true, true) => x >= st.mean + d,
                    (false, false) => x < st.mean - d,
                    (false, true) => x <= st.mean - d,
                };
                dxf(hit)
            }
            CfKind::Duplicate(dup) => {
                let k = key(value)?;
                let n = st.counts.get(&k).copied().unwrap_or(0);
                dxf(if *dup { n > 1 } else { n == 1 })
            }
            CfKind::Text { op, text } => {
                let mut s = String::new();
                let hay = match value {
                    Value::Text(t) => t.to_lowercase(),
                    Value::Number(n) => {
                        crate::numfmt::format_general(n, 15, &mut s);
                        s.to_lowercase()
                    }
                    Value::Bool(b) => (if b { "verdadeiro" } else { "falso" }).to_string(),
                    _ => String::new(),
                };
                let needle = text.to_lowercase();
                dxf(match op {
                    TextOp::Contains => hay.contains(&needle),
                    TextOp::NotContains => !hay.contains(&needle),
                    TextOp::Begins => hay.starts_with(&needle),
                    TextOp::Ends => hay.ends_with(&needle),
                })
            }
            CfKind::Blanks(blank) => {
                let is_blank = match value {
                    Value::Empty => true,
                    Value::Text(t) => t.trim().is_empty(),
                    _ => false,
                };
                dxf(is_blank == *blank)
            }
            CfKind::Errors(err) => dxf(matches!(value, Value::Error(_)) == *err),
            CfKind::TimePeriod(p) => {
                let x = number(value)?.floor();
                let t = self.today;
                // Semana de domingo a sábado; 1 = domingo no Excel (série 1 = 01/01/1900, domingo).
                let week_start = t - ((t as i64 - 1).rem_euclid(7)) as f64;
                let (y, m) = year_month(t);
                let month = |dy: i32| {
                    let (yy, mm) = add_months(y, m, dy);
                    (serial(yy, mm, 1), serial(add_months(yy, mm, 1).0, add_months(yy, mm, 1).1, 1))
                };
                let hit = match p {
                    Period::Today => x == t,
                    Period::Yesterday => x == t - 1.0,
                    Period::Tomorrow => x == t + 1.0,
                    Period::Last7Days => x > t - 7.0 && x <= t,
                    Period::ThisWeek => x >= week_start && x < week_start + 7.0,
                    Period::LastWeek => x >= week_start - 7.0 && x < week_start,
                    Period::NextWeek => x >= week_start + 7.0 && x < week_start + 14.0,
                    Period::ThisMonth | Period::LastMonth | Period::NextMonth => {
                        let d = match p {
                            Period::LastMonth => -1,
                            Period::NextMonth => 1,
                            _ => 0,
                        };
                        let (a, b) = month(d);
                        x >= a && x < b
                    }
                };
                dxf(hit)
            }
            CfKind::Unsupported => None,
        }
    }

    fn stats(&self, rule: &CfRule) -> Stats {
        let mut st = Stats::default();
        let needs_numbers = matches!(
            rule.kind,
            CfKind::ColorScale(_)
                | CfKind::DataBar { .. }
                | CfKind::IconSet { .. }
                | CfKind::Top10 { .. }
                | CfKind::AboveAverage { .. }
        );
        if needs_numbers {
            for r in &rule.ranges {
                for c in r.first_col..=r.last_col {
                    self.sheet.for_each_value(
                        c,
                        r.first_row..r.last_row.saturating_add(1),
                        self.shared,
                        |_, cell| {
                            if let Value::Number(x) = cell.value {
                                st.sorted.push(x);
                            }
                            true
                        },
                    );
                }
            }
            st.sorted.sort_by(f64::total_cmp);
            let n = st.sorted.len() as f64;
            if n > 0.0 {
                st.mean = st.sorted.iter().sum::<f64>() / n;
                // Desvio padrão amostral, como DESVPAD.
                if n > 1.0 {
                    st.std =
                        (st.sorted.iter().map(|x| (x - st.mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
                }
            }
        }
        if matches!(rule.kind, CfKind::Duplicate(_)) {
            for r in &rule.ranges {
                for c in r.first_col..=r.last_col {
                    self.sheet.for_each_value(
                        c,
                        r.first_row..r.last_row.saturating_add(1),
                        self.shared,
                        |_, cell| {
                            if let Some(k) = key(cell.value) {
                                *st.counts.entry(k).or_insert(0) += 1;
                            }
                            true
                        },
                    );
                }
            }
        }
        match &rule.kind {
            CfKind::CellIs { f1, f2, .. } => {
                st.f1 = parse(f1);
                st.f2 = f2.as_deref().and_then(parse);
            }
            CfKind::Expression(f) => st.f1 = parse(f),
            _ => {}
        }
        let cfvos: Vec<&Cfvo> = match &rule.kind {
            CfKind::ColorScale(stops) => stops.iter().map(|s| &s.0).collect(),
            CfKind::DataBar { min, max, .. } => vec![min, max],
            CfKind::IconSet { cfvos, .. } => cfvos.iter().collect(),
            _ => Vec::new(),
        };
        let (min, max) =
            (st.sorted.first().copied().unwrap_or(0.0), st.sorted.last().copied().unwrap_or(0.0));
        let base = rule.base();
        for c in cfvos {
            let num = || -> Option<f64> {
                let e = parse(&c.val)?;
                self.eval(&e, Pos { row: base.0, col: base.1, dr: 0, dc: 0 })?.number()
            };
            let v = match c.kind {
                CfvoKind::Min => min,
                CfvoKind::Max => max,
                CfvoKind::AutoMin => min.min(0.0),
                CfvoKind::AutoMax => max.max(0.0),
                CfvoKind::Num | CfvoKind::Formula => num().unwrap_or(min),
                CfvoKind::Percent => min + (max - min) * num().unwrap_or(0.0) / 100.0,
                CfvoKind::Percentile => percentile(&st.sorted, num().unwrap_or(0.0) / 100.0),
            };
            st.limits.push(v);
        }
        st
    }

    /// Valor de uma fórmula digitada (sem o `=`) na célula `(row, col)`, se
    /// for do tipo que este avaliador entende (aritmética, comparações,
    /// referências a células da aba e as funções da lista). Enquanto o motor
    /// de cálculo (F4) não existe, é o que dá valor às fórmulas novas.
    pub fn eval_formula(&self, text: &str, row: u32, col: u32) -> Option<crate::CellValue> {
        let e = parse(text)?;
        Some(match self.eval(&e, Pos { row, col, dr: 0, dc: 0 })? {
            V::Num(x) if x.is_finite() => crate::CellValue::Number(x),
            V::Num(_) => crate::CellValue::Error(crate::CellError::Num),
            V::Text(t) => crate::CellValue::Text(t.into_owned()),
            V::Bool(b) => crate::CellValue::Bool(b),
            V::Err => crate::CellValue::Error(crate::CellError::Value),
            V::Empty => crate::CellValue::Number(0.0),
        })
    }

    fn eval(&self, e: &Expr, at: Pos) -> Option<V<'_>> {
        eval(e, self, at)
    }
}

fn number(v: Value) -> Option<f64> {
    match v {
        Value::Number(x) => Some(x),
        _ => None,
    }
}

fn key(v: Value) -> Option<Key> {
    match v {
        Value::Number(x) => Some(Key::Num(x.to_bits())),
        Value::Text(t) if !t.is_empty() => Some(Key::Text(t.to_lowercase())),
        Value::Bool(b) => Some(Key::Bool(b)),
        _ => None,
    }
}

/// Percentil com interpolação linear (como PERCENTIL.INC).
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let p = p.clamp(0.0, 1.0);
    let pos = p * (sorted.len() - 1) as f64;
    let (i, f) = (pos.floor() as usize, pos.fract());
    let a = sorted[i];
    let b = sorted.get(i + 1).copied().unwrap_or(a);
    a + (b - a) * f
}

// --- datas ------------------------------------------------------------------

/// Número de série (sistema 1900) → (ano, mês).
fn year_month(serial: f64) -> (i32, u32) {
    // Dias desde 1970-01-01.
    let days = serial as i64 - 25_569;
    let (y, m, _) = civil(days);
    (y, m)
}

fn civil(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

fn serial(y: i32, m: u32, d: u32) -> f64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468 + 25_569) as f64
}

fn add_months(y: i32, m: u32, d: i32) -> (i32, u32) {
    let t = y * 12 + m as i32 - 1 + d;
    (t.div_euclid(12), (t.rem_euclid(12) + 1) as u32)
}

// --- avaliador de fórmulas --------------------------------------------------

/// Valor durante a avaliação.
#[derive(Clone, Debug, PartialEq)]
enum V<'a> {
    Num(f64),
    Text(std::borrow::Cow<'a, str>),
    Bool(bool),
    Err,
    Empty,
}

impl<'a> V<'a> {
    fn from_value(v: Value<'a>) -> Self {
        match v {
            Value::Empty => V::Empty,
            Value::Number(x) => V::Num(x),
            Value::Text(t) => V::Text(t.into()),
            Value::Bool(b) => V::Bool(b),
            Value::Error(_) => V::Err,
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            V::Num(x) => Some(*x),
            V::Bool(b) => Some(*b as u8 as f64),
            V::Empty => Some(0.0),
            V::Text(t) => t.trim().parse().ok(),
            V::Err => None,
        }
    }

    fn truthy(&self) -> Option<bool> {
        match self {
            V::Bool(b) => Some(*b),
            V::Num(x) => Some(*x != 0.0),
            V::Empty => Some(false),
            V::Text(t) => match t.to_ascii_uppercase().as_str() {
                "TRUE" | "VERDADEIRO" => Some(true),
                "FALSE" | "FALSO" => Some(false),
                _ => None,
            },
            V::Err => None,
        }
    }

    fn text(&self) -> String {
        match self {
            V::Num(x) => {
                let mut s = String::new();
                crate::numfmt::format_general(*x, 15, &mut s);
                s
            }
            V::Text(t) => t.to_string(),
            V::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).into(),
            V::Err | V::Empty => String::new(),
        }
    }
}

/// Comparação do Excel: números < textos < lógicos; texto sem diferenciar
/// maiúsculas; vazio vale 0 ou "".
fn compare(a: &V, b: &V) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let rank = |v: &V| match v {
        V::Num(_) => 0,
        V::Text(_) => 1,
        V::Bool(_) => 2,
        V::Err => 3,
        V::Empty => 0,
    };
    match (a, b) {
        (V::Empty, V::Text(t)) => "".cmp(t.to_lowercase().as_str()),
        (V::Text(t), V::Empty) => t.to_lowercase().as_str().cmp(""),
        (V::Empty, V::Empty) => Ordering::Equal,
        (V::Empty | V::Num(_), V::Empty | V::Num(_)) => {
            a.number().unwrap_or(0.0).partial_cmp(&b.number().unwrap_or(0.0)).unwrap_or(Ordering::Equal)
        }
        (V::Text(x), V::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (V::Bool(x), V::Bool(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)),
    }
}

#[derive(Clone, Copy)]
struct Pos {
    row: u32,
    col: u32,
    /// Deslocamento desde a base da regra (referências relativas andam junto).
    dr: i64,
    dc: i64,
}

#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Num(f64),
    Text(String),
    Bool(bool),
    Ref { row: u32, col: u32, abs_row: bool, abs_col: bool },
    Neg(Box<Expr>),
    Pct(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Cat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Interpreta uma fórmula de regra (sem o `=` inicial, como no arquivo).
fn parse(src: &str) -> Option<Expr> {
    let src = src.trim();
    let src = src.strip_prefix('=').unwrap_or(src);
    let mut p = Parser { s: src.as_bytes(), i: 0 };
    let e = p.compare()?;
    p.ws();
    (p.i == p.s.len()).then_some(e)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, t: &[u8]) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t) {
            self.i += t.len();
            true
        } else {
            false
        }
    }

    fn compare(&mut self) -> Option<Expr> {
        let mut a = self.concat()?;
        loop {
            let op = if self.eat(b"<>") {
                Op::Ne
            } else if self.eat(b"<=") {
                Op::Le
            } else if self.eat(b">=") {
                Op::Ge
            } else if self.eat(b"=") {
                Op::Eq
            } else if self.eat(b"<") {
                Op::Lt
            } else if self.eat(b">") {
                Op::Gt
            } else {
                return Some(a);
            };
            let b = self.concat()?;
            a = Expr::Bin(op, a.into(), b.into());
        }
    }

    fn concat(&mut self) -> Option<Expr> {
        let mut a = self.add()?;
        while self.eat(b"&") {
            let b = self.add()?;
            a = Expr::Bin(Op::Cat, a.into(), b.into());
        }
        Some(a)
    }

    fn add(&mut self) -> Option<Expr> {
        let mut a = self.mul()?;
        loop {
            let op = if self.eat(b"+") {
                Op::Add
            } else if self.eat(b"-") {
                Op::Sub
            } else {
                return Some(a);
            };
            let b = self.mul()?;
            a = Expr::Bin(op, a.into(), b.into());
        }
    }

    fn mul(&mut self) -> Option<Expr> {
        let mut a = self.pow()?;
        loop {
            let op = if self.eat(b"*") {
                Op::Mul
            } else if self.eat(b"/") {
                Op::Div
            } else {
                return Some(a);
            };
            let b = self.pow()?;
            a = Expr::Bin(op, a.into(), b.into());
        }
    }

    fn pow(&mut self) -> Option<Expr> {
        let mut a = self.unary()?;
        while self.eat(b"^") {
            let b = self.unary()?;
            a = Expr::Bin(Op::Pow, a.into(), b.into());
        }
        Some(a)
    }

    fn unary(&mut self) -> Option<Expr> {
        if self.eat(b"-") {
            return Some(Expr::Neg(self.unary()?.into()));
        }
        if self.eat(b"+") {
            return self.unary();
        }
        let mut e = self.primary()?;
        while self.eat(b"%") {
            e = Expr::Pct(e.into());
        }
        Some(e)
    }

    fn primary(&mut self) -> Option<Expr> {
        self.ws();
        let c = *self.s.get(self.i)?;
        if c == b'(' {
            self.i += 1;
            let e = self.compare()?;
            return self.eat(b")").then_some(e);
        }
        if c == b'"' {
            self.i += 1;
            let mut out = Vec::new();
            loop {
                let ch = *self.s.get(self.i)?;
                self.i += 1;
                if ch == b'"' {
                    if self.s.get(self.i) == Some(&b'"') {
                        out.push(b'"');
                        self.i += 1;
                        continue;
                    }
                    break;
                }
                out.push(ch);
            }
            return Some(Expr::Text(String::from_utf8(out).ok()?));
        }
        if c.is_ascii_digit() || c == b'.' {
            let start = self.i;
            while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                self.i += 1;
            }
            if self.i < self.s.len() && (self.s[self.i] == b'E' || self.s[self.i] == b'e') {
                self.i += 1;
                if self.i < self.s.len() && (self.s[self.i] == b'+' || self.s[self.i] == b'-') {
                    self.i += 1;
                }
                while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                    self.i += 1;
                }
            }
            return std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok().map(Expr::Num);
        }
        // Nome: referência, lógico ou função.
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'$' | b'_' | b'.'))
        {
            self.i += 1;
        }
        if self.i == start {
            return None;
        }
        let name = std::str::from_utf8(&self.s[start..self.i]).ok()?.to_ascii_uppercase();
        // Outra aba ou intervalo: fora do avaliador.
        if matches!(self.s.get(self.i), Some(b'!') | Some(b':')) {
            return None;
        }
        if self.eat(b"(") {
            let name = name.strip_prefix("_XLFN.").unwrap_or(&name).to_string();
            let mut args = Vec::new();
            if !self.eat(b")") {
                loop {
                    args.push(self.compare()?);
                    if self.eat(b")") {
                        break;
                    }
                    if !self.eat(b",") {
                        return None;
                    }
                }
            }
            return Some(Expr::Call(name, args));
        }
        match name.as_str() {
            "TRUE" => return Some(Expr::Bool(true)),
            "FALSE" => return Some(Expr::Bool(false)),
            _ => {}
        }
        parse_cell_ref(&name)
    }
}

fn parse_cell_ref(s: &str) -> Option<Expr> {
    let b = s.as_bytes();
    let mut i = 0;
    let abs_col = b.first() == Some(&b'$');
    if abs_col {
        i += 1;
    }
    let mut col: u32 = 0;
    let c0 = i;
    while i < b.len() && b[i].is_ascii_uppercase() {
        col = col.checked_mul(26)? + (b[i] - b'A') as u32 + 1;
        i += 1;
    }
    if i == c0 || i - c0 > 3 {
        return None;
    }
    let abs_row = b.get(i) == Some(&b'$');
    if abs_row {
        i += 1;
    }
    let row: u32 = std::str::from_utf8(&b[i..]).ok()?.parse().ok()?;
    if row == 0 {
        return None;
    }
    Some(Expr::Ref { row: row - 1, col: col - 1, abs_row, abs_col })
}

fn eval<'a>(e: &Expr, cx: &CfCtx<'a>, at: Pos) -> Option<V<'a>> {
    Some(match e {
        Expr::Num(x) => V::Num(*x),
        Expr::Text(t) => V::Text(t.clone().into()),
        Expr::Bool(b) => V::Bool(*b),
        Expr::Ref { row, col, abs_row, abs_col } => {
            let r = if *abs_row { *row as i64 } else { *row as i64 + at.dr };
            let c = if *abs_col { *col as i64 } else { *col as i64 + at.dc };
            if r < 0 || c < 0 {
                return None;
            }
            V::from_value(cx.sheet.cell(r as u32, c as u32, cx.shared).value)
        }
        Expr::Neg(a) => V::Num(-eval(a, cx, at)?.number()?),
        Expr::Pct(a) => V::Num(eval(a, cx, at)?.number()? / 100.0),
        Expr::Bin(op, a, b) => {
            let (x, y) = (eval(a, cx, at)?, eval(b, cx, at)?);
            if matches!(x, V::Err) || matches!(y, V::Err) {
                return Some(V::Err);
            }
            match op {
                Op::Cat => V::Text((x.text() + &y.text()).into()),
                Op::Eq => V::Bool(compare(&x, &y).is_eq()),
                Op::Ne => V::Bool(compare(&x, &y).is_ne()),
                Op::Lt => V::Bool(compare(&x, &y).is_lt()),
                Op::Le => V::Bool(compare(&x, &y).is_le()),
                Op::Gt => V::Bool(compare(&x, &y).is_gt()),
                Op::Ge => V::Bool(compare(&x, &y).is_ge()),
                _ => {
                    let (Some(p), Some(q)) = (x.number(), y.number()) else { return Some(V::Err) };
                    V::Num(match op {
                        Op::Add => p + q,
                        Op::Sub => p - q,
                        Op::Mul => p * q,
                        Op::Div => {
                            if q == 0.0 {
                                return Some(V::Err);
                            }
                            p / q
                        }
                        _ => p.powf(q),
                    })
                }
            }
        }
        Expr::Call(name, args) => {
            let arg = |k: usize| args.get(k).and_then(|a| eval(a, cx, at));
            match (name.as_str(), args.len()) {
                ("AND", n) if n > 0 => {
                    let mut all = true;
                    for k in 0..n {
                        all &= arg(k)?.truthy()?;
                    }
                    V::Bool(all)
                }
                ("OR", n) if n > 0 => {
                    let mut any = false;
                    for k in 0..n {
                        any |= arg(k)?.truthy()?;
                    }
                    V::Bool(any)
                }
                ("NOT", 1) => V::Bool(!arg(0)?.truthy()?),
                ("IF", 2 | 3) => {
                    if arg(0)?.truthy()? {
                        arg(1)?
                    } else if args.len() == 3 {
                        arg(2)?
                    } else {
                        V::Bool(false)
                    }
                }
                ("ISBLANK", 1) => V::Bool(matches!(arg(0)?, V::Empty)),
                ("ISERROR" | "ISERR", 1) => V::Bool(matches!(arg(0)?, V::Err)),
                ("ISNUMBER", 1) => V::Bool(matches!(arg(0)?, V::Num(_))),
                ("ISTEXT", 1) => V::Bool(matches!(arg(0)?, V::Text(_))),
                ("LEN", 1) => V::Num(arg(0)?.text().chars().count() as f64),
                ("TRIM", 1) => {
                    V::Text(arg(0)?.text().split_whitespace().collect::<Vec<_>>().join(" ").into())
                }
                ("UPPER", 1) => V::Text(arg(0)?.text().to_uppercase().into()),
                ("LOWER", 1) => V::Text(arg(0)?.text().to_lowercase().into()),
                ("ABS", 1) => V::Num(arg(0)?.number()?.abs()),
                ("INT", 1) => V::Num(arg(0)?.number()?.floor()),
                ("MOD", 2) => {
                    let (a, b) = (arg(0)?.number()?, arg(1)?.number()?);
                    if b == 0.0 { V::Err } else { V::Num(a - b * (a / b).floor()) }
                }
                ("ROW", 0) => V::Num(at.row as f64 + 1.0),
                ("COLUMN", 0) => V::Num(at.col as f64 + 1.0),
                ("TODAY", 0) => V::Num(cx.today),
                _ => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SheetPart, kind};

    fn sheet(vals: &[(u32, u32, f64)], texts: &[(u32, u32, u32)]) -> Sheet {
        let mut p = SheetPart::new();
        for &(r, c, v) in vals {
            p.set(r, c, kind::NUMBER, 0, v.to_bits());
        }
        for &(r, c, i) in texts {
            p.set(r, c, kind::SHARED_STR, 0, i as u64);
        }
        let mut s = Sheet::new("t");
        s.merge_part(p);
        s.finish();
        s
    }

    fn range(a: &str) -> CellRange {
        let parts: Vec<&str> = a.split(':').collect();
        let one = |s: &str| match parse_cell_ref(s).unwrap() {
            Expr::Ref { row, col, .. } => (row, col),
            _ => unreachable!(),
        };
        let (r0, c0) = one(parts[0]);
        let (r1, c1) = one(parts.get(1).copied().unwrap_or(parts[0]));
        CellRange { first_row: r0, first_col: c0, last_row: r1, last_col: c1 }
    }

    fn rule(r: &str, kind: CfKind) -> CfRule {
        CfRule { ranges: vec![range(r)], priority: 1, stop: false, dxf: Some(7), own: None, ext: None, kind }
    }

    #[test]
    fn parses_and_evaluates_expressions() {
        let s = sheet(&[(0, 0, 5.0), (1, 0, 15.0), (0, 1, 1.0)], &[(1, 1, 0)]);
        let mut shared = StringTable::new();
        shared.push("Sim");
        let cx = CfCtx { sheet: &s, shared: &shared, today: 45_000.0 };
        // Referência relativa: anda com a célula; absoluta: fica.
        let r = rule("A1:A2", CfKind::Expression("AND($A1>10,B$1=1)".into()));
        let st = CfState::default();
        assert_eq!(cx.apply(&r, &st, 0, 0, Value::Number(5.0)), None);
        assert_eq!(cx.apply(&r, &st, 1, 0, Value::Number(15.0)), Some(CfApply::Dxf(7)));
        let r = rule("B1:B2", CfKind::Expression(r#"=B1="sim""#.into()));
        let st = CfState::default();
        assert_eq!(cx.apply(&r, &st, 1, 1, Value::Text("Sim")), Some(CfApply::Dxf(7)));
        assert_eq!(cx.apply(&r, &st, 0, 1, Value::Number(1.0)), None);
        // Zebra.
        let r = rule("A1:A9", CfKind::Expression("MOD(ROW(),2)=0".into()));
        let st = CfState::default();
        assert!(cx.apply(&r, &st, 1, 0, Value::Empty).is_some());
        assert!(cx.apply(&r, &st, 2, 0, Value::Empty).is_none());
        // Fora do avaliador: não se aplica.
        assert_eq!(parse("COUNTIF($A:$A,A1)>1"), None);
        assert_eq!(parse("Plan2!A1>1"), None);
        assert_eq!(parse("1+2*3^2-4/2"), Some(parse("1+((2*(3^2)))-(4/2)").unwrap()));
    }

    #[test]
    fn cell_is_and_text_rules() {
        let s = sheet(&[], &[]);
        let shared = StringTable::new();
        let cx = CfCtx { sheet: &s, shared: &shared, today: 45_000.0 };
        let st = CfState::default();
        let r = rule("A1:A9", CfKind::CellIs { op: CfOp::Between, f1: "10".into(), f2: Some("1".into()) });
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(5.0)).is_some());
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(11.0)).is_none());
        let st = CfState::default();
        let r = rule("A1:A9", CfKind::CellIs { op: CfOp::Eq, f1: r#""ok""#.into(), f2: None });
        assert!(cx.apply(&r, &st, 0, 0, Value::Text("OK")).is_some());
        assert!(cx.apply(&r, &st, 0, 0, Value::Empty).is_none());
        let st = CfState::default();
        let r = rule("A1:A9", CfKind::Text { op: TextOp::Contains, text: "ATR".into() });
        assert!(cx.apply(&r, &st, 0, 0, Value::Text("em atraso")).is_some());
        let r = rule("A1:A9", CfKind::Blanks(true));
        assert!(cx.apply(&r, &st, 0, 0, Value::Empty).is_some());
        assert!(cx.apply(&r, &st, 0, 0, Value::Text("x")).is_none());
    }

    #[test]
    fn range_rules() {
        let vals: Vec<(u32, u32, f64)> = (0..10).map(|r| (r, 0, r as f64 * 10.0)).collect();
        let mut s = sheet(&vals, &[(0, 1, 0), (1, 1, 1), (2, 1, 0)]);
        s.finish();
        let mut shared = StringTable::new();
        shared.push("a");
        shared.push("b");
        let cx = CfCtx { sheet: &s, shared: &shared, today: 45_000.0 };
        let min = Cfvo { kind: CfvoKind::Min, val: String::new(), gte: true };
        let max = Cfvo { kind: CfvoKind::Max, val: String::new(), gte: true };
        let r = rule(
            "A1:A10",
            CfKind::ColorScale(vec![(min.clone(), Color::Rgb(0)), (max.clone(), Color::Rgb(1))]),
        );
        let st = CfState::default();
        match cx.apply(&r, &st, 0, 0, Value::Number(45.0)) {
            Some(CfApply::Scale { t, .. }) => assert!((t - 0.5).abs() < 1e-6),
            x => panic!("{x:?}"),
        }
        let r = rule("A1:A10", CfKind::Top10 { rank: 2, percent: false, bottom: false });
        let st = CfState::default();
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(80.0)).is_some());
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(70.0)).is_none());
        let r = rule("A1:A10", CfKind::AboveAverage { above: true, equal: false, std_dev: 0 });
        let st = CfState::default();
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(50.0)).is_some());
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(40.0)).is_none());
        let r = rule("B1:B3", CfKind::Duplicate(true));
        let st = CfState::default();
        assert!(cx.apply(&r, &st, 0, 1, Value::Text("A")).is_some());
        assert!(cx.apply(&r, &st, 1, 1, Value::Text("b")).is_none());
        let pct = |v: &str| Cfvo { kind: CfvoKind::Percent, val: v.into(), gte: true };
        let r = rule(
            "A1:A10",
            CfKind::IconSet {
                set: "3Arrows".into(),
                cfvos: vec![pct("0"), pct("33"), pct("67")],
                icons: Vec::new(),
                reverse: false,
                show_value: true,
            },
        );
        let st = CfState::default();
        let icon = |v| match cx.apply(&r, &st, 0, 0, Value::Number(v)) {
            Some(CfApply::Icon { idx, n: 3, .. }) => idx,
            x => panic!("{x:?}"),
        };
        assert_eq!((icon(0.0), icon(40.0), icon(90.0)), (0, 1, 2));
        let r = rule(
            "A1:A10",
            CfKind::DataBar { min, max, color: Color::Rgb(0), show_value: true, look: Box::default() },
        );
        let st = CfState::default();
        match cx.apply(&r, &st, 0, 0, Value::Number(90.0)) {
            Some(CfApply::Bar { bar, .. }) => assert!((bar.to - 0.9).abs() < 1e-6),
            x => panic!("{x:?}"),
        }
    }

    /// Ícones personalizados: cada limite com o ícone dele (menor que zero,
    /// zero, maior que zero).
    #[test]
    fn custom_icons() {
        let s = sheet(&[], &[]);
        let shared = StringTable::new();
        let cx = CfCtx { sheet: &s, shared: &shared, today: 45_000.0 };
        let at = |kind, v: &str, gte| Cfvo { kind, val: v.into(), gte };
        let r = rule(
            "A1:A3",
            CfKind::IconSet {
                set: "3Arrows".into(),
                cfvos: vec![
                    at(CfvoKind::Percent, "0", true),
                    at(CfvoKind::Num, "0", true),
                    at(CfvoKind::Num, "0", false),
                ],
                icons: vec![("3TrafficLights1".into(), 2), ("3Triangles".into(), 1), ("NoIcons".into(), 0)],
                reverse: false,
                show_value: false,
            },
        );
        let st = CfState::default();
        let icon = |v| match cx.apply(&r, &st, 0, 0, Value::Number(v)) {
            Some(CfApply::Icon { set, idx, n, show_value: false }) => (set.to_string(), idx, n),
            x => panic!("{x:?}"),
        };
        assert_eq!(icon(-0.1), ("3TrafficLights1".into(), 2, 3));
        assert_eq!(icon(0.0), ("3Triangles".into(), 1, 3));
        assert_eq!(icon(2.0), ("NoIcons".into(), 0, 0));
        assert_eq!(cx.apply(&r, &st, 0, 0, Value::Text("x")), None);
    }

    #[test]
    fn data_bars_of_the_extension() {
        let c = Color::Rgb(1);
        let look = BarLook {
            min_len: 0.0,
            max_len: 1.0,
            gradient: false,
            negative: Some(Color::Rgb(2)),
            axis: BarAxis::Auto,
            ..Default::default()
        };
        let b = look.bar(0.25, 0.0, 1.0, c);
        assert_eq!((b.from, b.to, b.color, b.gradient, b.axis), (0.0, 0.25, c, false, None));
        // Fora dos limites: cheia ou vazia.
        assert_eq!((look.bar(3.0, 0.0, 1.0, c).to, look.bar(0.0, 0.0, 1.0, c).to), (1.0, 0.0));
        // Com negativos: o zero fica na proporção dos limites.
        let b = look.bar(-5.0, -10.0, 30.0, c);
        assert_eq!((b.from, b.to, b.color), (0.25, 0.125, Color::Rgb(2)));
        let b = look.bar(15.0, -10.0, 30.0, c);
        assert_eq!((b.from, b.to, b.axis.map(|a| a.0)), (0.25, 0.625, Some(0.25)));
        // Só negativos: do fim para o começo.
        let b = look.bar(-5.0, -10.0, 0.0, c);
        assert_eq!((b.from, b.to), (1.0, 0.5));
        let mid = BarLook { axis: BarAxis::Middle, ..look.clone() };
        assert_eq!((mid.bar(5.0, -10.0, 10.0, c).from, mid.bar(5.0, -10.0, 10.0, c).to), (0.5, 0.75));
        let rtl = BarLook { rtl: true, ..look };
        assert_eq!((rtl.bar(0.25, 0.0, 1.0, c).from, rtl.bar(0.25, 0.0, 1.0, c).to), (1.0, 0.75));
        // A de 2007 vai de 10% a 90%.
        assert_eq!(BarLook::default().bar(0.0, 0.0, 1.0, c).to, 0.1);
    }

    #[test]
    fn joins_the_two_halves() {
        let cfvo = |v: &str| Cfvo { kind: CfvoKind::Num, val: v.into(), gte: true };
        let bar = |color, look| CfKind::DataBar {
            min: cfvo("0"),
            max: cfvo("1"),
            color,
            show_value: true,
            look: Box::new(look),
        };
        let id = |extension| Some(Box::new(ExtId { id: "{A}".into(), extension }));
        let mut common = rule("A1:A9", bar(Color::Rgb(5), BarLook::default()));
        common.ext = id(false);
        let mut half = rule("A1:A9", bar(Color::Auto, BarLook { gradient: false, ..Default::default() }));
        (half.ext, half.priority) = (id(true), i32::MAX);
        let mut alone = rule("B1", CfKind::Blanks(true));
        alone.ext = Some(Box::new(ExtId { id: "{B}".into(), extension: true }));
        let mut rules = vec![common, half, alone];
        join_extensions(&mut rules);
        assert_eq!(rules.len(), 2);
        match &rules[0].kind {
            CfKind::DataBar { color: Color::Rgb(5), look, .. } => assert!(!look.gradient),
            k => panic!("{k:?}"),
        }
        assert_eq!(rules[1].kind, CfKind::Blanks(true));
        assert!(rules.iter().all(|r| r.ext.is_none()));
    }

    #[test]
    fn dates() {
        assert_eq!(serial(2023, 3, 15), 45_000.0);
        assert_eq!(year_month(45_000.0), (2023, 3));
        assert_eq!(add_months(2023, 12, 1), (2024, 1));
        assert_eq!(add_months(2023, 1, -1), (2022, 12));
        let s = sheet(&[], &[]);
        let shared = StringTable::new();
        let cx = CfCtx { sheet: &s, shared: &shared, today: 45_000.0 };
        let st = CfState::default();
        let r = rule("A1", CfKind::TimePeriod(Period::ThisMonth));
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(serial(2023, 3, 1))).is_some());
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(serial(2023, 4, 1))).is_none());
        let r = rule("A1", CfKind::TimePeriod(Period::LastMonth));
        assert!(cx.apply(&r, &st, 0, 0, Value::Number(serial(2023, 2, 28))).is_some());
    }
}
