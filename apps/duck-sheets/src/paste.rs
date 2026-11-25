//! Copiar, recortar e colar; colar especial (valores, fórmulas, formatos,
//! larguras, transpor, operação) e o pincel de formatação.

use std::sync::Arc;

use duck_core::cells::CellOp;
use duck_core::input::{Input, parse_input};
use duck_core::{CellChange, CellError, CellRange, CellValue, CfCtx, Command, RichRun, Value};
use duck_platform::Context;

use crate::app::{MAX_COLS, MAX_ROWS, SheetsApp, fmt_int, selection_tsv};
use crate::edit::should_apply;
use crate::fill::{cells, range_of};
use crate::forms::{Field, Form, FormId};
use crate::look::Looks;

/// O que foi copiado dentro do app: além do texto (TSV) que foi para o
/// clipboard do sistema, as células como estão (fórmulas e estilos).
pub struct Clip {
    /// O texto posto no clipboard: se ainda for o mesmo ao colar, a cópia
    /// veio daqui e as fórmulas e estilos valem.
    pub text: String,
    pub sheet: usize,
    /// A origem, se é uma área só.
    pub src: Option<CellRange>,
    pub rows: u32,
    pub cols: u32,
    /// Linha a linha.
    pub cells: Vec<ClipCell>,
    /// Largura de cada coluna copiada (`None` = padrão).
    pub widths: Vec<Option<f32>>,
    /// Mesclagens inteiras, a partir do canto.
    pub merges: Vec<CellRange>,
    /// Recortar: a origem é movida ao colar (uma vez).
    pub cut: bool,
}

/// O que está copiado ou recortado, com a borda tracejada na tela. Sai com Esc,
/// com qualquer mudança na pasta e ao mover o que foi recortado.
#[derive(Clone, Debug, PartialEq)]
pub struct Marked {
    pub sheet: usize,
    pub areas: Vec<CellRange>,
}

#[derive(Clone)]
pub struct ClipCell {
    pub value: CellValue,
    pub formula: Option<String>,
    pub style: u16,
    pub rich: Option<Vec<RichRun>>,
    /// De que célula veio.
    pub at: (u32, u32),
}

/// O que colar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum What {
    #[default]
    All,
    Formulas,
    Values,
    Formats,
    AllButBorders,
    Widths,
    FormulasNum,
    ValuesNum,
}

const WHATS: [(&str, What); 8] = [
    ("Tudo", What::All),
    ("Fórmulas", What::Formulas),
    ("Valores", What::Values),
    ("Formatos", What::Formats),
    ("Tudo, exceto bordas", What::AllButBorders),
    ("Larguras das colunas", What::Widths),
    ("Fórmulas e formatos de número", What::FormulasNum),
    ("Valores e formatos de número", What::ValuesNum),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Op {
    #[default]
    None,
    Add,
    Sub,
    Mul,
    Div,
}

const OPS: [(&str, Op); 5] = [
    ("Nenhuma", Op::None),
    ("Adição", Op::Add),
    ("Subtração", Op::Sub),
    ("Multiplicação", Op::Mul),
    ("Divisão", Op::Div),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PasteOpts {
    pub what: What,
    pub op: Op,
    pub skip_blanks: bool,
    pub transpose: bool,
}

/// Formatos que o pincel leva.
#[derive(Clone)]
pub struct Painter {
    pub rows: u32,
    pub cols: u32,
    pub styles: Vec<u16>,
    /// Continua ligado depois de usar (duplo clique no botão).
    pub sticky: bool,
}

/// Máximo de células copiadas com fórmulas e estilos (acima disso, só o texto).
const MAX_CLIP: u64 = 1_000_000;
/// Máximo de células copiadas como texto.
const MAX_COPY: u64 = 5_000_000;
/// Máximo de células coladas de uma vez.
const MAX_PASTE: u64 = 2_000_000;

/// Lê o TSV do clipboard (como o Excel e o Google Planilhas gravam: campos
/// com tab, quebra ou aspas vêm entre aspas, com aspas dobradas).
pub fn parse_tsv(s: &str) -> Vec<Vec<String>> {
    let mut rows = vec![Vec::new()];
    let mut field = String::new();
    let mut chars = s.chars().peekable();
    let mut at_start = true;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if at_start => {
                // Campo entre aspas.
                while let Some(q) = chars.next() {
                    if q == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            field.push('"');
                        } else {
                            break;
                        }
                    } else {
                        field.push(q);
                    }
                }
                at_start = false;
            }
            '\t' => {
                rows.last_mut().unwrap().push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                rows.last_mut().unwrap().push(std::mem::take(&mut field));
                rows.push(Vec::new());
                at_start = true;
            }
            c => {
                field.push(c);
                at_start = false;
            }
        }
    }
    if !field.is_empty() || !rows.last().unwrap().is_empty() {
        rows.last_mut().unwrap().push(field);
    } else {
        rows.pop();
    }
    rows
}

pub fn value_of(v: Value) -> CellValue {
    match v {
        Value::Empty => CellValue::Empty,
        Value::Number(x) => CellValue::Number(x),
        Value::Text(t) => CellValue::Text(t.to_string()),
        Value::Bool(b) => CellValue::Bool(b),
        Value::Error(e) => CellValue::Error(e),
    }
}

/// Número como numa fórmula do arquivo (ponto decimal).
fn literal(v: &CellValue) -> Option<String> {
    match v {
        CellValue::Empty => Some("0".into()),
        CellValue::Number(x) => Some(format!("{x}")),
        _ => None,
    }
}

/// O valor (ou fórmula) da operação entre o que há no destino e o colado.
/// `None` = não dá para operar (texto): o destino fica como está.
fn operate(
    op: Op,
    old: (&CellValue, Option<&str>),
    new: (&CellValue, Option<&str>),
) -> Option<(CellValue, Option<String>)> {
    let sign = match op {
        Op::Add => "+",
        Op::Sub => "-",
        Op::Mul => "*",
        Op::Div => "/",
        Op::None => return Some((new.0.clone(), new.1.map(str::to_string))),
    };
    if old.1.is_some() || new.1.is_some() {
        let side = |x: (&CellValue, Option<&str>)| match x.1 {
            Some(f) => Some(format!("({f})")),
            None => literal(x.0),
        };
        return Some((CellValue::Empty, Some(format!("{}{sign}{}", side(old)?, side(new)?))));
    }
    let num = |v: &CellValue| match v {
        CellValue::Empty => Some(0.0),
        CellValue::Number(x) => Some(*x),
        _ => None,
    };
    let (a, b) = (num(old.0)?, num(new.0)?);
    let x = match op {
        Op::Add => a + b,
        Op::Sub => a - b,
        Op::Mul => a * b,
        _ if b == 0.0 => return Some((CellValue::Error(CellError::Div0), None)),
        _ => a / b,
    };
    Some((CellValue::Number(x), None))
}

/// Junta as áreas num bloco só, como o Excel: todas com as mesmas linhas
/// (lado a lado) ou todas com as mesmas colunas (uma embaixo da outra). O
/// `true` diz lado a lado. `None` = não dá para copiar.
fn arrange(areas: &[CellRange]) -> Option<(Vec<CellRange>, bool)> {
    let mut v = areas.to_vec();
    v.sort_by_key(|r| (r.first_row, r.first_col));
    v.dedup();
    let overlap = v
        .iter()
        .enumerate()
        .any(|(i, a)| v[i + 1..].iter().any(|b| duck_core::cells::intersect(a, b).is_some()));
    if overlap && v.len() > 1 {
        return None;
    }
    let same_rows = v.iter().all(|r| (r.first_row, r.last_row) == (v[0].first_row, v[0].last_row));
    let same_cols = v.iter().all(|r| (r.first_col, r.last_col) == (v[0].first_col, v[0].last_col));
    if same_cols {
        Some((v, false))
    } else if same_rows {
        v.sort_by_key(|r| r.first_col);
        Some((v, true))
    } else {
        None
    }
}

impl SheetsApp {
    // --- copiar -------------------------------------------------------------

    /// Ctrl+C e Ctrl+X: o texto da seleção para o clipboard (e guarda as
    /// células para colar aqui com fórmulas e estilos).
    pub(crate) fn copy_text(&mut self, cut: bool) -> Option<String> {
        let doc = self.doc.as_ref()?;
        let (sheet, v) = doc.current()?;
        let active = doc.active;
        let selected: Vec<CellRange> = v.areas().into_iter().map(range_of).collect();
        // Linhas e colunas inteiras: só até onde há dados.
        let areas: Vec<CellRange> = v
            .areas()
            .into_iter()
            .map(|(r0, c0, r1, c1)| {
                let r1 = r1.min(sheet.rows().max(r0 + 1) - 1).max(r0);
                let c1 = c1.min(sheet.cols().max(c0 + 1) - 1).max(c0);
                range_of((r0, c0, r1, c1))
            })
            .collect();
        if cut && areas.len() > 1 {
            self.message = "Não dá para recortar várias áreas de uma vez".into();
            return None;
        }
        let Some((areas, side)) = arrange(&areas) else {
            self.message = "Só dá para copiar várias áreas com as mesmas linhas ou as mesmas colunas".into();
            return None;
        };
        let total: u64 = areas.iter().map(cells).sum();
        if total > MAX_COPY {
            self.message = format!("Seleção grande demais para copiar ({} células)", fmt_int(total));
            return None;
        }
        let part = |r: &CellRange| {
            let sel = (r.first_row, r.first_col, r.last_row, r.last_col);
            selection_tsv(sheet, &doc.strings, &doc.looks, doc.date1904, sel)
        };
        let text = if side {
            // Lado a lado: junta as linhas.
            let mut rows: Vec<String> = Vec::new();
            for (k, r) in areas.iter().enumerate() {
                let sel = |row| (row, r.first_col, row, r.last_col);
                for (i, row) in (r.first_row..=r.last_row).enumerate() {
                    let t = selection_tsv(sheet, &doc.strings, &doc.looks, doc.date1904, sel(row));
                    let t = t.trim_end_matches("\r\n");
                    if k == 0 {
                        rows.push(t.to_string());
                    } else {
                        rows[i].push('\t');
                        rows[i].push_str(t);
                    }
                }
            }
            rows.iter().map(|r| format!("{r}\r\n")).collect()
        } else {
            areas.iter().map(part).collect::<String>()
        };
        self.clip = if total <= MAX_CLIP { self.capture(&areas, side, text.clone(), cut) } else { None };
        self.marked = Some(Marked { sheet: active, areas: selected });
        self.message = if cut {
            "Recortado: cole com Ctrl+V".into()
        } else {
            format!("{} células copiadas", fmt_int(total))
        };
        Some(text)
    }

    /// As células de um bloco da aba ativa.
    pub fn capture(&self, areas: &[CellRange], side: bool, text: String, cut: bool) -> Option<Clip> {
        let doc = self.doc.as_ref()?;
        let (sheet, v) = doc.current()?;
        let size = |r: &CellRange| (r.last_row - r.first_row + 1, r.last_col - r.first_col + 1);
        let (rows, cols) = if side {
            (size(&areas[0]).0, areas.iter().map(|r| size(r).1).sum())
        } else {
            (areas.iter().map(|r| size(r).0).sum(), size(&areas[0]).1)
        };
        // De onde vem cada linha e cada coluna do bloco.
        let (row_of, col_of): (Vec<u32>, Vec<u32>) = if side {
            (
                (areas[0].first_row..=areas[0].last_row).collect(),
                areas.iter().flat_map(|r| r.first_col..=r.last_col).collect(),
            )
        } else {
            (
                areas.iter().flat_map(|r| r.first_row..=r.last_row).collect(),
                (areas[0].first_col..=areas[0].last_col).collect(),
            )
        };
        let mut list = Vec::with_capacity((rows * cols) as usize);
        for &r in &row_of {
            for &c in &col_of {
                let cell = sheet.cell(r, c, &doc.strings);
                list.push(ClipCell {
                    value: value_of(cell.value),
                    formula: duck_calc::cell_formula(sheet, r, c).map(|f| f.into_owned()),
                    style: if cell.style == 0 { v.blank_style(r, c) } else { cell.style },
                    rich: sheet.rich_at(r, c, &doc.strings).map(<[_]>::to_vec),
                    at: (r, c),
                });
            }
        }
        let src = (areas.len() == 1).then(|| areas[0]);
        let merges = match &src {
            Some(a) => v
                .merges
                .in_rows(a.first_row, a.last_row)
                .filter(|m| duck_core::cells::intersect(m, a) == Some(**m))
                .map(|m| CellRange {
                    first_row: m.first_row - a.first_row,
                    last_row: m.last_row - a.first_row,
                    first_col: m.first_col - a.first_col,
                    last_col: m.last_col - a.first_col,
                })
                .collect(),
            None => Vec::new(),
        };
        let widths = col_of.iter().map(|&c| sheet.col_width(c)).collect();
        Some(Clip { text, sheet: doc.active, src, rows, cols, cells: list, widths, merges, cut })
    }

    // --- colar --------------------------------------------------------------

    /// Ctrl+V: cola na seleção. Uma cópia feita aqui leva fórmulas (com as
    /// referências deslocadas) e estilos; texto de outro programa é
    /// interpretado como se digitado. Um bloco menor que a seleção é
    /// repetido se couber um número inteiro de vezes (como no Excel).
    pub fn paste(&mut self, cx: &mut Context, opts: PasteOpts) {
        if let Some(text) = cx.clipboard().get_text() {
            self.paste_text(text, opts);
        }
    }

    /// Cola `text` (o conteúdo do clipboard; ver [`Self::paste`]).
    pub fn paste_text(&mut self, text: String, opts: PasteOpts) {
        self.commit_edit();
        if !self.editable() {
            return;
        }
        let Some(v) = self.doc.as_ref().and_then(|d| d.current()).map(|x| x.1) else { return };
        if v.multi() {
            self.message = "Escolha uma área só para colar".into();
            return;
        }
        let (r0, c0, r1, c1) = v.sel();
        let clip = self.clip.take();
        let marked = self.marked.take();
        match clip {
            Some(clip) if clip.text == text => {
                let done = self.paste_clip(&clip, range_of((r0, c0, r1, c1)), opts);
                // Depois de recortar, a origem já foi movida. Uma cópia pode
                // ser colada de novo: continua marcada.
                if !(done && clip.cut) {
                    self.clip = Some(clip);
                    self.marked = marked;
                }
            }
            // Veio de outro programa: o que estava marcado não vale mais.
            clip => {
                self.clip = clip.filter(|c| !c.cut);
                self.paste_foreign(&text, range_of((r0, c0, r1, c1)), opts);
            }
        }
    }

    /// Tira a marca do que está copiado. Um recorte é desfeito: a origem fica
    /// onde está.
    pub fn unmark(&mut self) {
        self.marked = None;
        if self.clip.as_ref().is_some_and(|c| c.cut) {
            self.clip = None;
        }
    }

    /// A área do destino: o bloco no canto da seleção, ou repetido nela.
    fn paste_area(&mut self, sel: CellRange, rows: u32, cols: u32) -> Option<CellRange> {
        let (sr, sc) = (sel.last_row - sel.first_row + 1, sel.last_col - sel.first_col + 1);
        let (reps_r, reps_c) =
            if sr.is_multiple_of(rows) && sc.is_multiple_of(cols) && (sr, sc) != (rows, cols) {
                (sr / rows, sc / cols)
            } else {
                (1, 1)
            };
        let total = rows as u64 * cols as u64 * reps_r as u64 * reps_c as u64;
        if total > MAX_PASTE {
            self.message = format!("Colagem grande demais ({} células)", fmt_int(total));
            return None;
        }
        let last = |a: u32, n: u32, max: u32| (a as u64 + n as u64 - 1).min(max as u64 - 1) as u32;
        Some(CellRange {
            first_row: sel.first_row,
            first_col: sel.first_col,
            last_row: last(sel.first_row, rows * reps_r, MAX_ROWS),
            last_col: last(sel.first_col, cols * reps_c, MAX_COLS),
        })
    }

    /// Cola uma cópia feita no app, com o canto em `sel`.
    pub fn paste_clip(&mut self, clip: &Clip, sel: CellRange, opts: PasteOpts) -> bool {
        let (rows, cols) = if opts.transpose { (clip.cols, clip.rows) } else { (clip.rows, clip.cols) };
        // Recortar e colar na mesma aba é mover: as referências acompanham.
        if clip.cut
            && opts == PasteOpts::default()
            && let Some(src) = clip.src
        {
            if self.doc.as_ref().is_some_and(|d| d.active == clip.sheet) {
                let dr = sel.first_row as i64 - src.first_row as i64;
                let dc = sel.first_col as i64 - src.first_col as i64;
                return self.run_cell_op(CellOp::Move { src, dr, dc });
            }
            self.message = "Recortar entre abas: colado como cópia".into();
        }
        let Some(area) = self.paste_area(sel, rows, cols) else { return false };
        let today = self.today;
        let Some(doc) = self.doc.as_mut() else { return false };
        let active = doc.active;
        if opts.what == What::Widths {
            let list: Vec<(u32, Option<f32>)> = (area.first_col..=area.last_col)
                .zip(clip.widths.iter().cycle())
                .map(|(c, w)| (c, *w))
                .collect();
            let ok = self.run_step(vec![(active, Command::ColWidths(list))]);
            if ok {
                self.message = "Larguras coladas".into();
            }
            return ok;
        }
        let mut styles = doc.styles.clone();
        let before = styles.xfs.len();
        let Some((sheet, v)) = doc.current() else { return false };
        let cfx = CfCtx { sheet, shared: &doc.strings, today };
        let mut changes: Vec<CellChange> = Vec::new();
        let mut only_style: Vec<(u32, u32, u16)> = Vec::new();
        let mut rich: Vec<(u32, u32, Vec<RichRun>)> = Vec::new();
        for r in area.first_row..=area.last_row {
            for c in area.first_col..=area.last_col {
                let (i, j) = ((r - area.first_row) % rows, (c - area.first_col) % cols);
                let k = if opts.transpose { j * clip.cols + i } else { i * clip.cols + j };
                let cell = &clip.cells[k as usize];
                let blank = matches!(cell.value, CellValue::Empty) && cell.formula.is_none();
                if opts.skip_blanks && blank {
                    continue;
                }
                let old_style = {
                    let s = sheet.raw(r, c).style;
                    if s == 0 { v.blank_style(r, c) } else { s }
                };
                let style = match opts.what {
                    What::All | What::Formats => Some(cell.style),
                    What::AllButBorders => {
                        let border = styles.xfs.get(old_style as usize).map_or(0, |x| x.border);
                        Some(Arc::make_mut(&mut styles).derive_xf(cell.style, |x| x.border = border))
                    }
                    What::FormulasNum | What::ValuesNum => {
                        let fmt = styles.xfs.get(cell.style as usize).map_or(0, |x| x.num_fmt);
                        Some(Arc::make_mut(&mut styles).derive_xf(old_style, |x| x.num_fmt = fmt))
                    }
                    _ => None,
                };
                if opts.what == What::Formats {
                    only_style.push((r, c, cell.style));
                    continue;
                }
                let formula = match &cell.formula {
                    Some(f) if !matches!(opts.what, What::Values | What::ValuesNum) => {
                        Some(if opts.transpose {
                            duck_calc::editing::transpose(f, cell.at, (r, c))
                        } else {
                            duck_calc::shift(f, r as i64 - cell.at.0 as i64, c as i64 - cell.at.1 as i64)
                        })
                    }
                    _ => None,
                };
                let old_value = value_of(sheet.cell(r, c, &doc.strings).value);
                let old_formula = match opts.op {
                    Op::None => None,
                    _ => duck_calc::cell_formula(sheet, r, c).map(|f| f.into_owned()),
                };
                let Some((value, formula)) =
                    operate(opts.op, (&old_value, old_formula.as_deref()), (&cell.value, formula.as_deref()))
                else {
                    continue;
                };
                let moved = (r, c) != cell.at || opts.op != Op::None;
                let value = match &formula {
                    Some(f) if moved => cfx.eval_formula(f, r, c).unwrap_or(CellValue::Empty),
                    _ => value,
                };
                if formula.is_none()
                    && opts.op == Op::None
                    && let Some(runs) = &cell.rich
                {
                    rich.push((r, c, runs.clone()));
                }
                changes.push(CellChange { row: r, col: c, value, formula, style });
            }
        }
        // Mesclagens: as do destino saem, as da cópia entram.
        let formats = matches!(opts.what, What::All | What::Formats | What::AllButBorders);
        let (mut add, mut remove) = (Vec::new(), Vec::new());
        if formats {
            remove = v
                .merges
                .in_rows(area.first_row, area.last_row)
                .filter(|m| duck_core::cells::intersect(m, &area).is_some())
                .copied()
                .collect();
            let mut r = area.first_row;
            while !clip.merges.is_empty() && r <= area.last_row {
                let mut c = area.first_col;
                while c <= area.last_col {
                    for m in &clip.merges {
                        let m = if opts.transpose {
                            CellRange {
                                first_row: m.first_col,
                                last_row: m.last_col,
                                first_col: m.first_row,
                                last_col: m.last_row,
                            }
                        } else {
                            *m
                        };
                        add.push(CellRange {
                            first_row: r + m.first_row,
                            last_row: r + m.last_row,
                            first_col: c + m.first_col,
                            last_col: c + m.last_col,
                        });
                    }
                    c += cols;
                }
                r += rows;
            }
        }
        let n = changes.len().max(only_style.len()) as u64;
        let mut cmds = Vec::new();
        if !changes.is_empty() {
            cmds.push(Command::SetCells(changes));
        }
        if !rich.is_empty() {
            cmds.push(Command::RichText(rich));
        }
        if !only_style.is_empty() {
            cmds.push(Command::SetStyles(only_style));
        }
        if add != remove {
            cmds.push(Command::Merges { add, remove });
        }
        if cmds.is_empty() {
            self.message = "Nada para colar".into();
            return false;
        }
        if styles.xfs.len() != before {
            doc.looks = Arc::new(Looks::new(&styles));
            doc.styles = styles;
            self.font_key = (0, 0);
        }
        if !self.run_step(vec![(active, Command::Group(cmds))]) {
            return false;
        }
        self.select_range(area);
        self.message = format!("{} células coladas", fmt_int(n));
        true
    }

    /// Cola texto de outro programa.
    fn paste_foreign(&mut self, text: &str, sel: CellRange, opts: PasteOpts) {
        if matches!(opts.what, What::Formats | What::Widths) {
            self.message = "A cópia não veio do Duck Sheets: não há formatos para colar".into();
            return;
        }
        let tsv = parse_tsv(text);
        let (mut rows, mut cols) = (tsv.len() as u32, tsv.iter().map(Vec::len).max().unwrap_or(0) as u32);
        if opts.transpose {
            std::mem::swap(&mut rows, &mut cols);
        }
        if rows == 0 || cols == 0 {
            return;
        }
        let Some(area) = self.paste_area(sel, rows, cols) else { return };
        let today = self.today;
        let Some(doc) = self.doc.as_mut() else { return };
        let active = doc.active;
        let mut styles = doc.styles.clone();
        let before = styles.xfs.len();
        let Some((sheet, v)) = doc.current() else { return };
        let cfx = CfCtx { sheet, shared: &doc.strings, today };
        let mut changes = Vec::new();
        // Com `=`, mas que não dá para ler como fórmula: fica como texto.
        let mut broken = 0u64;
        for r in area.first_row..=area.last_row {
            for c in area.first_col..=area.last_col {
                let (i, j) = ((r - area.first_row) % rows, (c - area.first_col) % cols);
                let (i, j) = if opts.transpose { (j, i) } else { (i, j) };
                let field =
                    tsv.get(i as usize).and_then(|row| row.get(j as usize)).map_or("", String::as_str);
                if opts.skip_blanks && field.is_empty() {
                    continue;
                }
                let current = sheet.cell(r, c, &doc.strings);
                let base = if current.style == 0 { v.blank_style(r, c) } else { current.style };
                let (value, formula, style) =
                    match parse_input(field, doc.date1904, duck_core::input::today()) {
                        Input::Formula(f) => match doc.file_formula(active, &f, r, c) {
                            Ok(f) => (CellValue::Empty, Some(f), base),
                            Err(_) => {
                                broken += 1;
                                (CellValue::Text(field.to_string()), None, base)
                            }
                        },
                        Input::Value(value, fmt) => {
                            let mut style = base;
                            let cur = styles.xfs.get(base as usize).map_or(0, |x| x.num_fmt);
                            if let Some(f) = fmt
                                && should_apply(&styles, cur, f)
                            {
                                style = Arc::make_mut(&mut styles).derive_xf(base, |x| x.num_fmt = f);
                            }
                            (value, None, style)
                        }
                    };
                let old = value_of(current.value);
                let old_formula = match opts.op {
                    Op::None => None,
                    _ => duck_calc::cell_formula(sheet, r, c).map(|f| f.into_owned()),
                };
                let Some((value, formula)) =
                    operate(opts.op, (&old, old_formula.as_deref()), (&value, formula.as_deref()))
                else {
                    continue;
                };
                let value = match &formula {
                    Some(f) => cfx.eval_formula(f, r, c).unwrap_or(CellValue::Empty),
                    None => value,
                };
                changes.push(CellChange { row: r, col: c, value, formula, style: Some(style) });
            }
        }
        if styles.xfs.len() != before {
            doc.looks = Arc::new(Looks::new(&styles));
            doc.styles = styles;
            self.font_key = (0, 0);
        }
        let n = changes.len() as u64;
        if self.run_step(vec![(active, Command::SetCells(changes))]) {
            self.select_range(area);
            self.message = format!("{} células coladas", fmt_int(n));
            if broken > 0 {
                let n = fmt_int(broken);
                self.message.push_str(&format!(" · {n} com erro na fórmula ficaram como texto"));
            }
        }
    }

    /// Seleciona um intervalo (a célula ativa fica no fim, como num arraste).
    pub fn select_range(&mut self, a: CellRange) {
        if let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) {
            v.single();
            v.anchor = (a.first_row, a.first_col);
            v.cur = (a.last_row.min(v.rows.len() - 1), a.last_col.min(v.cols.len() - 1));
        }
    }

    // --- colar especial -----------------------------------------------------

    /// Abre a caixa "Colar especial" com o texto do clipboard.
    pub fn ask_paste_special(&mut self, text: String) {
        self.commit_edit();
        let mine = self.clip.as_ref().is_some_and(|c| c.text == text);
        let names: Vec<&str> = WHATS.iter().map(|w| w.0).collect();
        let ops: Vec<&str> = OPS.iter().map(|o| o.0).collect();
        let mut fields = vec![
            Field::radio("what", "Colar", &names, 0).left(),
            Field::radio("op", "Operação", &ops, 0).right(),
            Field::check("skip", "Ignorar em branco", false),
            Field::check("transpose", "Transpor (linhas viram colunas)", false),
        ];
        if !mine {
            fields.push(Field::info("", "A cópia veio de outro programa: não há fórmulas nem formatos."));
        }
        self.paste_source = text;
        self.form = Some(Form::new(FormId::PasteSpecial, "Colar especial", fields, &["OK", "Cancelar"]));
    }

    /// OK na caixa "Colar especial".
    pub fn paste_form(&mut self, f: &Form) {
        let opts = PasteOpts {
            what: WHATS[f.sel("what").min(WHATS.len() - 1)].1,
            op: OPS[f.sel("op").min(OPS.len() - 1)].1,
            skip_blanks: f.on("skip"),
            transpose: f.on("transpose"),
        };
        let text = std::mem::take(&mut self.paste_source);
        self.paste_text(text, opts);
    }

    // --- pincel -------------------------------------------------------------

    /// Liga o pincel com o formato da seleção (ou desliga).
    pub fn toggle_painter(&mut self, sticky: bool) {
        if self.painter.is_some() && !sticky {
            self.painter = None;
            self.message.clear();
            return;
        }
        self.commit_edit();
        let Some((r0, c0, r1, c1)) = self.sel_cells(100_000) else { return };
        let Some(doc) = &self.doc else { return };
        let Some((sheet, v)) = doc.current() else { return };
        if v.multi() {
            self.message = "Escolha uma área só para copiar o formato".into();
            return;
        }
        let styles = (r0..=r1)
            .flat_map(|r| (c0..=c1).map(move |c| (r, c)))
            .map(|(r, c)| {
                let s = sheet.raw(r, c).style;
                if s == 0 { v.blank_style(r, c) } else { s }
            })
            .collect();
        self.painter = Some(Painter { rows: r1 - r0 + 1, cols: c1 - c0 + 1, styles, sticky });
        self.message = "Pincel: escolha as células que vão receber o formato (Esc sai)".into();
    }

    /// Aplica o pincel nas áreas selecionadas.
    pub fn apply_painter(&mut self) {
        let Some(p) = self.painter.clone() else { return };
        if !p.sticky {
            self.painter = None;
        }
        let Some(v) = self.doc.as_ref().and_then(|d| d.current()).map(|x| x.1) else { return };
        let mut list: Vec<(u32, u32, u16)> = Vec::new();
        let mut last = None;
        for (r0, c0, r1, c1) in v.areas() {
            // Uma célula recebe o bloco inteiro.
            let (r1, c1) = if (r0, c0) == (r1, c1) { (r0 + p.rows - 1, c0 + p.cols - 1) } else { (r1, c1) };
            let (r1, c1) = (r1.min(MAX_ROWS - 1), c1.min(MAX_COLS - 1));
            if (r1 - r0 + 1) as u64 * (c1 - c0 + 1) as u64 > MAX_PASTE {
                self.message = "Seleção grande demais para o pincel".into();
                return;
            }
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let k = ((r - r0) % p.rows) * p.cols + (c - c0) % p.cols;
                    list.push((r, c, p.styles[k as usize]));
                }
            }
            last = Some(range_of((r0, c0, r1, c1)));
        }
        let Some(active) = self.doc.as_ref().map(|d| d.active) else { return };
        if self.run_step(vec![(active, Command::SetStyles(list))]) {
            self.message = "Formato copiado".into();
            if let Some(a) = last
                && !self.doc.as_ref().and_then(|d| d.current()).is_some_and(|x| x.1.multi())
            {
                self.select_range(a);
            }
        }
    }

    /// `--edit` de copiar, colar e do pincel. `true` = era um deles.
    pub fn scripted_paste(&mut self, e: &str, clipboard: &mut String) -> bool {
        let Some((op, arg)) = e.split_once(':') else { return false };
        let opts = |what, transpose, op| PasteOpts { what, op, skip_blanks: false, transpose };
        let paste = match op {
            "colar" => Some(PasteOpts::default()),
            "colar-valores" => Some(opts(What::Values, false, Op::None)),
            "colar-formulas" => Some(opts(What::Formulas, false, Op::None)),
            "colar-formatos" => Some(opts(What::Formats, false, Op::None)),
            "colar-larguras" => Some(opts(What::Widths, false, Op::None)),
            "colar-sem-bordas" => Some(opts(What::AllButBorders, false, Op::None)),
            "colar-transpor" => Some(opts(What::All, true, Op::None)),
            "colar-somar" => Some(opts(What::Values, false, Op::Add)),
            "copiar" | "recortar" => None,
            "pincel" => {
                let Some((from, to)) = arg.split_once('>') else { return false };
                if self.scripted_select(from) {
                    self.toggle_painter(false);
                    if self.scripted_select(to) {
                        self.apply_painter();
                    }
                }
                eprintln!("--edit {e}: {}", self.message);
                return true;
            }
            _ => return false,
        };
        if !self.scripted_select(arg) {
            eprintln!("--edit: intervalo inválido: {arg}");
            return true;
        }
        match paste {
            Some(o) => self.paste_text(clipboard.clone(), o),
            None => *clipboard = self.copy_text(op == "recortar").unwrap_or_default(),
        }
        true
    }

    /// Seleciona `A1:B2` (várias áreas separadas por `;`).
    pub fn scripted_select(&mut self, s: &str) -> bool {
        let mut areas = Vec::new();
        for part in s.split(';') {
            let Some((a, b)) = crate::app::parse_range(part.trim()) else { return false };
            areas.push((a, b));
        }
        let Some(&(a, b)) = areas.last() else { return false };
        self.go_to(a.0, a.1);
        let Some(v) = self.doc.as_mut().and_then(|d| d.view_mut()) else { return false };
        v.anchor = a;
        v.cur = b;
        v.single();
        if areas.len() > 1 {
            v.extra = areas[..areas.len() - 1]
                .iter()
                .map(|(a, b)| (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)))
                .collect();
            v.extra_anchor = v.anchor;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, get, select, style};

    #[test]
    fn tsv_from_other_programs() {
        assert_eq!(parse_tsv("a\tb\r\n1\t2\r\n"), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(parse_tsv("\"x\ny\"\t\"di \"\"aspas\"\"\"\n"), vec![vec!["x\ny", "di \"aspas\""]]);
        assert_eq!(parse_tsv("só"), vec![vec!["só"]]);
        assert_eq!(parse_tsv("a\t\tc"), vec![vec!["a", "", "c"]]);
    }

    fn sample() -> SheetsApp {
        let mut a = app(&[(
            "P",
            &[("A1", "1"), ("B1", "2"), ("A2", "3"), ("B2", "=A1+B1"), ("D1", "10"), ("D2", "=D1")],
        )]);
        select(&mut a, "A1:B2");
        a.apply_fmt(crate::format::Fmt::Bold);
        a.doc.as_mut().unwrap().history = Default::default();
        a
    }

    fn paste(a: &mut SheetsApp, from: &str, to: &str, opts: PasteOpts) {
        select(a, from);
        let t = a.copy_text(false).unwrap();
        select(a, to);
        a.paste_text(t, opts);
    }

    fn bold(a: &SheetsApp, cell: &str) -> bool {
        let f = style(a, 0, cell).font;
        a.doc.as_ref().unwrap().styles.fonts[f as usize].bold
    }

    #[test]
    fn special() {
        let o = |what| PasteOpts { what, ..Default::default() };
        let mut a = sample();
        paste(&mut a, "A1:B2", "F1", o(What::Values));
        assert_eq!((get(&a, 0, "G2"), bold(&a, "G2")), ("3".into(), false));
        paste(&mut a, "A1:B2", "F5", o(What::Formulas));
        assert_eq!((get(&a, 0, "G6"), bold(&a, "G6")), ("=F5+G5".into(), false));
        paste(&mut a, "A1:B2", "F9", o(What::Formats));
        assert_eq!((get(&a, 0, "G10"), bold(&a, "G10")), ("".into(), true));
        paste(&mut a, "A1:B2", "F13", PasteOpts { transpose: true, ..Default::default() });
        assert_eq!((get(&a, 0, "F14"), get(&a, 0, "G13")), ("2".into(), "3".into()));
        assert_eq!(get(&a, 0, "G14"), "=F13+F14");
        paste(&mut a, "D1:D2", "A1", PasteOpts { what: What::Values, op: Op::Add, ..Default::default() });
        assert_eq!((get(&a, 0, "A1"), get(&a, 0, "A2")), ("11".into(), "13".into()));
        paste(&mut a, "D1", "B2", PasteOpts { what: What::Values, op: Op::Mul, ..Default::default() });
        assert_eq!(get(&a, 0, "B2"), "=(A1+B1)*10");
        for _ in 0..6 {
            a.undo();
        }
        assert_eq!(
            (get(&a, 0, "A1"), get(&a, 0, "B2"), get(&a, 0, "F13")),
            ("1".into(), "=A1+B1".into(), "".into())
        );
    }

    #[test]
    fn widths_and_blanks() {
        let mut a = sample();
        a.run_command(Command::ColWidths(vec![(0, Some(30.0))]));
        paste(&mut a, "A1:B1", "F1", PasteOpts { what: What::Widths, ..Default::default() });
        let w = |a: &SheetsApp, c| a.doc.as_ref().unwrap().current().unwrap().0.col_width(c);
        assert_eq!((w(&a, 5), w(&a, 6)), (Some(30.0), None));
        // Ignorar em branco não apaga o destino.
        paste(&mut a, "C1:D1", "A1", PasteOpts { skip_blanks: true, ..Default::default() });
        assert_eq!((get(&a, 0, "A1"), get(&a, 0, "B1")), ("1".into(), "10".into()));
    }

    #[test]
    fn cut_moves_references() {
        let mut a = sample();
        select(&mut a, "A1:A2");
        let t = a.copy_text(true).unwrap();
        select(&mut a, "F1");
        a.paste_text(t.clone(), PasteOpts::default());
        assert_eq!(
            (get(&a, 0, "A1"), get(&a, 0, "F1"), get(&a, 0, "B2")),
            ("".into(), "1".into(), "=F1+B1".into())
        );
        // Uma vez só.
        assert!(a.clip.is_none());
        a.undo();
        assert_eq!((get(&a, 0, "A1"), get(&a, 0, "B2")), ("1".into(), "=A1+B1".into()));
    }

    #[test]
    fn marks_what_is_copied() {
        let mark = |a: &SheetsApp| a.marked.as_ref().map(|m| m.areas.clone());
        let a1a2 = || Some(vec![range_of((0, 0, 1, 0))]);
        let mut a = sample();
        select(&mut a, "A1:A2");
        let t = a.copy_text(false).unwrap();
        assert_eq!(mark(&a), a1a2());
        // Colar não tira a marca: dá para colar de novo.
        for cell in ["F1", "G1"] {
            select(&mut a, cell);
            a.paste_text(t.clone(), PasteOpts::default());
            assert_eq!(get(&a, 0, cell), "1");
            assert_eq!(mark(&a), a1a2());
        }
        // Qualquer mudança tira, e a cópia continua valendo.
        crate::testing::type_in(&mut a, "H1", "x");
        assert!(a.marked.is_none() && a.clip.is_some());
        // A marca é a seleção toda, mesmo onde não há dados.
        select(&mut a, "A1:K30");
        a.copy_text(false).unwrap();
        assert_eq!(mark(&a), Some(vec![range_of((0, 0, 29, 10))]));
        // Texto de outro programa no clipboard: a marca sai.
        select(&mut a, "J1");
        a.paste_text("de fora".into(), PasteOpts::default());
        assert!(a.marked.is_none());
    }

    #[test]
    fn marks_what_is_cut() {
        let mut a = sample();
        select(&mut a, "A1:A2");
        let t = a.copy_text(true).unwrap();
        assert!(a.marked.is_some());
        // Esc: nada se move; o texto do clipboard ainda pode ser colado.
        a.unmark();
        assert!(a.marked.is_none() && a.clip.is_none());
        select(&mut a, "F1");
        a.paste_text(t, PasteOpts::default());
        assert_eq!((get(&a, 0, "A1"), get(&a, 0, "F1")), ("1".into(), "1".into()));
        // Uma mudança no meio também cancela: a origem pode ter andado.
        select(&mut a, "A1:A2");
        a.copy_text(true).unwrap();
        crate::testing::type_in(&mut a, "H1", "x");
        assert!(a.marked.is_none() && a.clip.is_none());
        // Colado: a marca sai com a origem.
        select(&mut a, "A1:A2");
        let t = a.copy_text(true).unwrap();
        select(&mut a, "F5");
        a.paste_text(t, PasteOpts::default());
        assert_eq!((get(&a, 0, "A1"), get(&a, 0, "F5")), ("".into(), "1".into()));
        assert!(a.marked.is_none());
    }

    #[test]
    fn several_areas() {
        let mut a = sample();
        assert!(a.scripted_select("A1:A2;D1:D2"));
        let t = a.copy_text(false).unwrap();
        assert_eq!(t, "1\t10\r\n3\t10\r\n");
        select(&mut a, "F1");
        a.paste_text(t, PasteOpts::default());
        assert_eq!((get(&a, 0, "F2"), get(&a, 0, "G2")), ("3".into(), "=G1".into()));
        assert!(a.scripted_select("A1:A2;D1:E1"));
        assert!(a.copy_text(false).is_none());
        assert!(a.scripted_select("A1;D1"));
        assert!(a.copy_text(true).is_none());
    }

    #[test]
    fn merges_and_text_from_outside() {
        let mut a = sample();
        select(&mut a, "A1:B1");
        a.merge(crate::ribbon::Merge::Cells);
        paste(&mut a, "A1:B2", "F1", PasteOpts::default());
        let m = &a.doc.as_ref().unwrap().current().unwrap().0.layout.merges;
        assert!(m.contains(&range_of((0, 5, 0, 6))), "{m:?}");
        select(&mut a, "I1");
        a.paste_text("1,5\t=A2*2\r\n22/11/2026\tx".into(), PasteOpts::default());
        assert_eq!((get(&a, 0, "I1"), get(&a, 0, "J1")), ("1,5".into(), "=A2*2".into()));
        assert_eq!(get(&a, 0, "I2"), "46348");
        select(&mut a, "L1");
        a.paste_text("a\tb".into(), PasteOpts { transpose: true, ..Default::default() });
        assert_eq!(get(&a, 0, "L2"), "b");
    }

    #[test]
    fn painter() {
        let mut a = sample();
        select(&mut a, "A1");
        a.toggle_painter(false);
        select(&mut a, "D1:D2");
        a.apply_painter();
        assert!(bold(&a, "D1") && bold(&a, "D2") && a.painter.is_none());
        a.undo();
        assert!(!bold(&a, "D1"));
        // Duplo clique: continua ligado; um bloco maior numa célula só.
        select(&mut a, "A1:B2");
        a.toggle_painter(true);
        select(&mut a, "H1");
        a.apply_painter();
        assert!(bold(&a, "I2") && a.painter.is_some());
    }
}
