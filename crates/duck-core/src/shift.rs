//! Inserir e apagar linhas e colunas (F3.5).
//!
//! - As células andam no modelo (coluna por coluna, em paralelo); as que
//!   ficam numa linha apagada saem e são guardadas para desfazer.
//! - Toda fórmula da aba é conferida: referências às linhas/colunas que
//!   andaram são ajustadas, as que apontavam para o que foi apagado viram
//!   `#REF!` (o ajuste de texto fica no `duck-calc`, via [`FormulaOps`]).
//!   Grupos de fórmulas compartilhadas viram modelos relativos.
//! - Larguras, alturas, mesclagens, formatação condicional e links andam
//!   junto.
//! - O gravador regrava a aba inteira a partir do modelo (`Edits::structure`).

use std::collections::HashMap;
use std::sync::Arc;

use crate::edit::RawCell;
use crate::{BLOCK_ROWS, CellRange, Column, FormulaDef, FormulaKind, Layout, SHARED_BIT, Sheet};

/// Linhas e colunas do Excel.
pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Rows,
    Cols,
}

/// Inserir (`insert`) ou apagar `n` linhas/colunas a partir de `at` (base 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shift {
    pub axis: Axis,
    pub at: u32,
    pub n: u32,
    pub insert: bool,
}

impl Shift {
    pub fn inverse(&self) -> Shift {
        Shift { insert: !self.insert, ..*self }
    }

    fn max(&self) -> u32 {
        match self.axis {
            Axis::Rows => MAX_ROWS,
            Axis::Cols => MAX_COLS,
        }
    }

    /// Nova posição (base 0) de uma linha/coluna; `None` se foi apagada (ou
    /// empurrada para fora da planilha).
    pub fn index(&self, i: u32) -> Option<u32> {
        if i < self.at {
            return Some(i);
        }
        if self.insert {
            let j = i as u64 + self.n as u64;
            (j < self.max() as u64).then_some(j as u32)
        } else if i < self.at + self.n {
            None
        } else {
            Some(i - self.n)
        }
    }

    /// Onde estava, antes da mudança, o que agora está em `i` (`None` = nova).
    pub fn source(&self, i: u32) -> Option<u32> {
        if i < self.at {
            return Some(i);
        }
        if self.insert {
            (i >= self.at + self.n).then(|| i - self.n)
        } else {
            let j = i as u64 + self.n as u64;
            (j < self.max() as u64).then_some(j as u32)
        }
    }

    /// Novo intervalo `[a, b]` no eixo: inserir dentro dele o aumenta; apagar
    /// uma parte o encolhe; `None` se foi apagado inteiro.
    pub fn span(&self, a: u32, b: u32) -> Option<(u32, u32)> {
        if self.insert {
            let a2 = self.index(a)?;
            let b2 = self.index(b).unwrap_or(self.max() - 1);
            Some((a2, b2))
        } else {
            let end = self.at + self.n; // primeira depois do apagado
            if a >= self.at && b < end {
                return None;
            }
            let a2 = if a < self.at {
                a
            } else if a < end {
                self.at
            } else {
                a - self.n
            };
            let b2 = if b < self.at {
                b
            } else if b < end {
                self.at - 1
            } else {
                b - self.n
            };
            Some((a2, b2))
        }
    }

    /// O intervalo de células depois da mudança.
    pub fn range(&self, r: CellRange) -> Option<CellRange> {
        Some(match self.axis {
            Axis::Rows => {
                let (a, b) = self.span(r.first_row, r.last_row)?;
                CellRange { first_row: a, last_row: b, ..r }
            }
            Axis::Cols => {
                let (a, b) = self.span(r.first_col, r.last_col)?;
                CellRange { first_col: a, last_col: b, ..r }
            }
        })
    }
}

/// Ajuste de fórmulas para uma mudança de estrutura (implementado no
/// `duck-calc`, que conhece a sintaxe das fórmulas).
pub trait FormulaOps: Send + Sync {
    /// A fórmula (A1) com as referências ajustadas, se mudar; e se o ajuste
    /// perdeu informação (`#REF!`, intervalo encolhido), para o desfazer
    /// guardar a original.
    fn adjust(&self, a1: &str) -> Option<(String, bool)>;
    fn make_template(&self, a1: &str, row: u32, col: u32) -> Option<String>;
    fn expand_template(&self, template: &str, row: u32, col: u32) -> String;
    /// Desloca as referências relativas (fórmulas compartilhadas).
    fn shift_a1(&self, a1: &str, drow: i64, dcol: i64) -> String;
    /// As mesmas operações para outra mudança (desfazer).
    fn with_shift(&self, s: Shift) -> Arc<dyn FormulaOps>;
    /// As referências de um modelo relativo no eixo (`rows`: linhas; senão
    /// colunas): (absoluta, valor ou deslocamento, base 1).
    fn axis_refs(&self, template: &str, rows: bool) -> Vec<(bool, i64)>;
}

/// O modelo continua valendo na célula que andou de `p_old` para `p_new`
/// (base 1, no eixo)? Sim se cada referência for para onde foi a célula
/// que ela apontava.
fn unchanged(refs: &[(bool, i64)], s: Shift, p_old: i64, p_new: i64) -> bool {
    refs.iter().all(|&(abs, v)| {
        let t = if abs { v } else { p_old + v };
        if t < 1 {
            return true; // já era `#REF!`
        }
        let Some(nt) = s.index((t - 1) as u32).map(|x| x as i64 + 1) else { return false };
        if abs { nt == t } else { nt - p_new == v }
    })
}

/// Colunas deslocadas por uma thread e as células que se perderam nelas.
type ShiftedColumns = (Vec<Column>, Vec<(u32, u32, RawCell)>);

/// O que o desfazer precisa depois de [`Sheet::apply_shift`].
pub struct ShiftUndo {
    /// Células que se perderam (apagadas ou com fórmula que perdeu
    /// informação), na posição de antes.
    pub lost: Vec<(u32, u32, RawCell)>,
    pub layout: Box<Layout>,
}

impl Sheet {
    /// Texto A1 da fórmula de id `id` se ela estivesse em (`row`, `col`).
    fn formula_a1(&self, id: u32, row: u32, col: u32, ops: &dyn FormulaOps) -> Option<(String, FormulaKind)> {
        let def = if id & SHARED_BIT != 0 {
            let idx = *self.shared_groups.get(&(id & !SHARED_BIT))?;
            self.formula_defs.get(idx as usize)?
        } else {
            self.formula_defs.get(id as usize - 1)?
        };
        let text = self.formula_texts.get(def.text)?;
        let a1 = if def.template {
            ops.expand_template(text, row, col)
        } else if matches!(def.kind, FormulaKind::Shared { .. }) && (row, col) != (def.row, def.col) {
            ops.shift_a1(text, row as i64 - def.row as i64, col as i64 - def.col as i64)
        } else {
            text.to_string()
        };
        let kind = match def.kind {
            FormulaKind::Shared { .. } => FormulaKind::Normal,
            k => k,
        };
        Some((a1, kind))
    }

    /// Insere ou apaga linhas/colunas. Devolve o que o desfazer precisa.
    pub fn apply_shift(&mut self, s: Shift, ops: &dyn FormulaOps) -> ShiftUndo {
        let layout = Box::new(self.layout.clone());
        let mut lost = Vec::new();

        // 1. As células andam.
        match s.axis {
            Axis::Cols => {
                let at = s.at as usize;
                if s.insert {
                    if at < self.columns.len() {
                        let fill: Vec<Column> = (0..s.n).map(|_| Column::default()).collect();
                        self.columns.splice(at..at, fill);
                        let max = MAX_COLS as usize;
                        if self.columns.len() > max {
                            self.columns.truncate(max);
                        }
                    }
                } else if at < self.columns.len() {
                    let end = (at + s.n as usize).min(self.columns.len());
                    for (k, col) in self.columns[at..end].iter().enumerate() {
                        collect_cells(col, (at + k) as u32, &mut lost);
                    }
                    self.columns.drain(at..end);
                }
            }
            Axis::Rows => {
                let cols = std::mem::take(&mut self.columns);
                let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
                let chunk = cols.len().div_ceil(threads).max(1);
                let mut parts: Vec<ShiftedColumns> = Vec::new();
                let mut cols = cols;
                let mut groups: Vec<Vec<Column>> = Vec::new();
                while !cols.is_empty() {
                    let rest = cols.split_off(chunk.min(cols.len()));
                    groups.push(std::mem::replace(&mut cols, rest));
                }
                std::thread::scope(|sc| {
                    let handles: Vec<_> = groups
                        .into_iter()
                        .enumerate()
                        .map(|(g, group)| {
                            sc.spawn(move || {
                                let mut lost = Vec::new();
                                let out = group
                                    .into_iter()
                                    .enumerate()
                                    .map(|(k, col)| {
                                        shift_column_rows(col, (g * chunk + k) as u32, s, &mut lost)
                                    })
                                    .collect::<Vec<_>>();
                                (out, lost)
                            })
                        })
                        .collect();
                    for h in handles {
                        parts.push(h.join().expect("thread de deslocamento"));
                    }
                });
                for (cols, l) in parts {
                    self.columns.extend(cols);
                    lost.extend(l);
                }
            }
        }

        // 2. Fórmulas. A maioria usa um modelo relativo que continua valendo
        // depois de andar (as referências andam junto com a célula): essas
        // só são conferidas pelos deslocamentos das referências, sem texto.
        // O resto é refeito pelo texto A1 no lugar antigo, ajustado.
        let rows_axis = s.axis == Axis::Rows;
        let mut templates: HashMap<String, u32> = HashMap::new();
        let mut refs: HashMap<u32, Vec<(bool, i64)>> = HashMap::new();
        for (i, d) in self.formula_defs.iter().enumerate() {
            if d.template
                && let Some(t) = self.formula_texts.get(d.text)
            {
                templates.insert(t.to_string(), i as u32 + 1);
                refs.insert(i as u32 + 1, ops.axis_refs(t, rows_axis));
            }
        }
        // Grupos compartilhados viram um modelo (o texto só existe na mestre,
        // que pode andar ou mudar).
        let mut group_tpl: HashMap<u32, u32> = HashMap::new(); // si -> id do modelo
        let mut master_tpl: HashMap<u32, u32> = HashMap::new(); // id da mestre -> id do modelo
        let groups: Vec<(u32, u32)> = self.shared_groups.iter().map(|(&si, &idx)| (si, idx)).collect();
        for (si, idx) in groups {
            let Some(d) = self.formula_defs.get(idx as usize).copied() else { continue };
            let Some(text) = self.formula_texts.get(d.text).map(str::to_string) else { continue };
            let Some(t) = ops.make_template(&text, d.row, d.col) else { continue };
            let id = match templates.get(&t) {
                Some(&id) => id,
                None => {
                    let tt = self.formula_texts.push(&t);
                    self.formula_defs.push(FormulaDef {
                        text: tt,
                        kind: FormulaKind::Normal,
                        row: 0,
                        col: 0,
                        template: true,
                    });
                    let id = self.formula_defs.len() as u32;
                    refs.insert(id, ops.axis_refs(&t, rows_axis));
                    templates.insert(t, id);
                    id
                }
            };
            group_tpl.insert(si, id);
            master_tpl.insert(idx + 1, id);
        }
        enum Action {
            SetId(u32, u32, u32),
            Text(u32, u32, String, FormulaKind),
        }
        let mut actions: Vec<Action> = Vec::new();
        let mut lossy: Vec<(u32, u32, RawCell)> = Vec::new();
        {
            let this = &*self;
            let (refs, group_tpl, master_tpl) = (&refs, &group_tpl, &master_tpl);
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
            let ncols = this.columns.len();
            let chunk = ncols.div_ceil(threads).max(1);
            std::thread::scope(|sc| {
                let handles: Vec<_> = (0..ncols)
                    .step_by(chunk)
                    .map(|c0| {
                        sc.spawn(move || {
                            let mut out = Vec::new();
                            let mut bad = Vec::new();
                            for c in c0..(c0 + chunk).min(ncols) {
                                let col = &this.columns[c];
                                for (bi, ids) in col.formulas.iter().enumerate() {
                                    let Some(ids) = ids else { continue };
                                    for i in 0..BLOCK_ROWS {
                                        let id = ids.get(i);
                                        if id == 0 {
                                            continue;
                                        }
                                        let (r, c) = ((bi * BLOCK_ROWS + i) as u32, c as u32);
                                        let (or, oc) = match s.axis {
                                            Axis::Rows => (s.source(r), Some(c)),
                                            Axis::Cols => (Some(r), s.source(c)),
                                        };
                                        let (Some(or), Some(oc)) = (or, oc) else { continue };
                                        let (p_old, p_new) = if rows_axis {
                                            (or as i64 + 1, r as i64 + 1)
                                        } else {
                                            (oc as i64 + 1, c as i64 + 1)
                                        };
                                        let shared = id & SHARED_BIT != 0;
                                        let tpl = if shared {
                                            group_tpl.get(&(id & !SHARED_BIT)).copied()
                                        } else if let Some(&t) = master_tpl.get(&id) {
                                            Some(t)
                                        } else {
                                            refs.contains_key(&id).then_some(id)
                                        };
                                        if let Some(t) = tpl
                                            && let Some(rf) = refs.get(&t)
                                            && unchanged(rf, s, p_old, p_new)
                                        {
                                            if t != id {
                                                out.push(Action::SetId(r, c, t));
                                            }
                                            continue;
                                        }
                                        let Some((a1, kind)) = this.formula_a1(id, or, oc, ops) else {
                                            continue;
                                        };
                                        let positional =
                                            tpl.is_some() || shared || master_tpl.contains_key(&id);
                                        match ops.adjust(&a1) {
                                            Some((new, loses)) => {
                                                if loses {
                                                    bad.push((or, oc, r, c));
                                                }
                                                out.push(Action::Text(r, c, new, kind));
                                            }
                                            None if positional => out.push(Action::Text(r, c, a1, kind)),
                                            None => {}
                                        }
                                    }
                                }
                            }
                            (out, bad)
                        })
                    })
                    .collect();
                for h in handles {
                    let (out, bad) = h.join().expect("thread de fórmulas");
                    actions.extend(out);
                    for (or, oc, r, c) in bad {
                        // A célula de antes, para o desfazer (no lugar antigo).
                        lossy.push((or, oc, this.raw(r, c)));
                    }
                }
            });
        }
        for a in actions {
            let (r, c, id) = match a {
                Action::SetId(r, c, id) => (r, c, id),
                Action::Text(r, c, a1, kind) => {
                    let id = match (kind, ops.make_template(&a1, r, c)) {
                        (FormulaKind::Normal, Some(t)) => match templates.get(&t) {
                            Some(&id) => id,
                            None => {
                                let text = self.formula_texts.push(&t);
                                self.formula_defs.push(FormulaDef {
                                    text,
                                    kind,
                                    row: 0,
                                    col: 0,
                                    template: true,
                                });
                                let id = self.formula_defs.len() as u32;
                                templates.insert(t, id);
                                id
                            }
                        },
                        (kind, _) => {
                            let kind = match kind {
                                FormulaKind::Array { range, dynamic } => {
                                    FormulaKind::Array { range: s.range(range).unwrap_or(range), dynamic }
                                }
                                k => k,
                            };
                            let text = self.formula_texts.push(&a1);
                            self.formula_defs.push(FormulaDef {
                                text,
                                kind,
                                row: r,
                                col: c,
                                template: false,
                            });
                            self.formula_defs.len() as u32
                        }
                    };
                    (r, c, id)
                }
            };
            let bi = r as usize / BLOCK_ROWS;
            self.columns[c as usize].formula_block_mut(bi)[r as usize % BLOCK_ROWS] = id;
            self.touched.push((c, bi as u32));
        }
        lost.extend(lossy);

        // 3. Layout.
        shift_layout(&mut self.layout, s);
        // Referências das séries dos gráficos (sempre com o nome da aba).
        for o in &mut self.layout.objects {
            if let crate::drawing::ObjectKind::Chart(c) = &mut o.kind {
                for r in c.refs_mut() {
                    if let Some((new, _)) = ops.adjust(r) {
                        *r = new;
                    }
                }
            }
        }
        if !self.layout.objects.is_empty() || !self.layout.pivots.is_empty() {
            self.mark_pivots();
        }
        // Colunas novas dentro de uma tabela: o cabeçalho recebe o nome da
        // coluna (o Excel exige o texto do cabeçalho igual ao nome).
        if s.axis == Axis::Cols && s.insert {
            let heads: Vec<(u32, u32, String)> = self
                .layout
                .tables
                .iter()
                .filter_map(|t| Some((t, t.header_row()?)))
                .flat_map(|(t, hr)| {
                    (s.at..s.at + s.n)
                        .filter(|c| (t.range.first_col..=t.range.last_col).contains(c))
                        .filter_map(move |c| {
                            Some((hr, c, t.columns.get((c - t.range.first_col) as usize)?.clone()))
                        })
                })
                .collect();
            for (r, c, name) in heads {
                self.set_cell(r, c, crate::CellValue::Text(name), None, None);
            }
        }

        // 4. Tamanho e o que o gravador precisa saber.
        let count = match s.axis {
            Axis::Rows => &mut self.rows,
            Axis::Cols => &mut self.cols,
        };
        if *count > s.at {
            *count = if s.insert { (*count + s.n).min(s.max()) } else { *count - s.n.min(*count - s.at) };
        }
        self.edits.structure = true;
        self.edits.shifts.push(s);
        self.edits.filter |= self.layout.autofilter.is_some();
        self.edits.tables.extend(0..self.layout.tables.len() as u32);
        self.edits.values = true;
        self.edits.formulas = true;
        self.settle();
        ShiftUndo { lost, layout }
    }
}

/// Todas as células de uma coluna (para o desfazer de apagar colunas).
fn collect_cells(col: &Column, c: u32, out: &mut Vec<(u32, u32, RawCell)>) {
    for (bi, slot) in col.blocks.iter().enumerate() {
        for i in 0..BLOCK_ROWS {
            let r = bi * BLOCK_ROWS + i;
            let formula = col.formula_id(r);
            let raw = match slot {
                Some(b) => RawCell { kind: b.kind(i), style: b.style(i), payload: b.payload(i), formula },
                None => RawCell { formula, ..Default::default() },
            };
            if !raw.is_blank() {
                out.push((r as u32, c, raw));
            }
        }
    }
}

/// Uma coluna com as linhas deslocadas. Blocos inteiros antes de `at` só
/// mudam de dono; o resto é copiado célula a célula.
fn shift_column_rows(old: Column, c: u32, s: Shift, lost: &mut Vec<(u32, u32, RawCell)>) -> Column {
    let mut new = Column::default();
    let first_moved = s.at as usize / BLOCK_ROWS;
    let Column { blocks, formulas } = old;
    let mut formulas: Vec<Option<crate::Ids>> = formulas;
    for (bi, slot) in blocks.into_iter().enumerate() {
        let ids = formulas.get_mut(bi).and_then(Option::take);
        if bi < first_moved {
            if new.blocks.len() <= bi {
                new.blocks.resize_with(bi + 1, || None);
                new.formulas.resize_with(bi + 1, || None);
            }
            new.blocks[bi] = slot;
            new.formulas[bi] = ids;
            continue;
        }
        for i in 0..BLOCK_ROWS {
            let r = (bi * BLOCK_ROWS + i) as u32;
            let formula = ids.as_ref().map_or(0, |x| x.get(i));
            let raw = match &slot {
                Some(b) => RawCell { kind: b.kind(i), style: b.style(i), payload: b.payload(i), formula },
                None => RawCell { formula, ..Default::default() },
            };
            if raw.is_blank() {
                continue;
            }
            match s.index(r) {
                Some(nr) => {
                    let nr = nr as usize;
                    let b = new.block_mut(nr / BLOCK_ROWS);
                    let j = nr % BLOCK_ROWS;
                    b.kind[j] = raw.kind;
                    b.style[j] = raw.style;
                    b.payload[j] = raw.payload;
                    if raw.formula != 0 {
                        new.formula_block_mut(nr / BLOCK_ROWS)[j] = raw.formula;
                    }
                }
                None => lost.push((r, c, raw)),
            }
        }
    }
    // Blocos de ids sem células (fórmulas sem valor) também andam.
    for (bi, ids) in formulas.into_iter().enumerate() {
        let Some(ids) = ids else { continue };
        for i in 0..BLOCK_ROWS {
            let id = ids.get(i);
            let r = (bi * BLOCK_ROWS + i) as u32;
            if id != 0
                && let Some(nr) = s.index(r)
            {
                let nr = nr as usize;
                new.formula_block_mut(nr / BLOCK_ROWS)[nr % BLOCK_ROWS] = id;
            }
        }
    }
    new.pack();
    new
}

/// Larguras/alturas, mesclagens, formatação condicional, links e dimensão.
pub fn shift_layout(l: &mut Layout, s: Shift) {
    l.outline.shift(s);
    match s.axis {
        Axis::Rows => {
            l.rows = l
                .rows
                .iter()
                .filter_map(|r| s.index(r.row).map(|row| crate::RowSpec { row, ..*r }))
                .collect();
        }
        Axis::Cols => {
            l.cols = l
                .cols
                .iter()
                .filter_map(|c| {
                    let (first, last) = s.span(c.first, c.last)?;
                    Some(crate::ColSpec { first, last, ..*c })
                })
                .collect();
            // Colunas inseridas no meio de um `<col>` herdam ele (como no Excel).
        }
    }
    l.merges = l
        .merges
        .iter()
        .filter_map(|m| s.range(*m))
        .filter(|m| m.first_row != m.last_row || m.first_col != m.last_col)
        .collect();
    for rule in &mut l.cond {
        rule.ranges = rule.ranges.iter().filter_map(|r| s.range(*r)).collect();
    }
    l.cond.retain(|r| !r.ranges.is_empty());
    l.links = l
        .links
        .iter()
        .filter_map(|h| s.range(h.range).map(|range| crate::Hyperlink { range, ..h.clone() }))
        .collect();
    l.dimension = l.dimension.and_then(|d| s.range(d));
    let shift_filter = |f: &mut Option<crate::AutoFilter>| {
        if let Some(af) = f.take() {
            *f = s.range(af.range).map(|range| {
                let mut af = af;
                af.range = range;
                af
            });
        }
    };
    shift_filter(&mut l.autofilter);
    let mut tables = Vec::with_capacity(l.tables.len());
    for mut t in std::mem::take(&mut l.tables) {
        let Some(range) = s.range(t.range) else { continue };
        if s.axis == Axis::Cols {
            // Colunas inseridas/apagadas dentro da tabela: a lista de colunas acompanha.
            let (c0, c1) = (t.range.first_col, t.range.last_col);
            if s.insert && s.at > c0 && s.at <= c1 {
                let k = (s.at - c0) as usize;
                let next_id = t
                    .column_xml
                    .iter()
                    .filter_map(|x| x.split("id=\"").nth(1)?.split('"').next()?.parse::<u32>().ok())
                    .max()
                    .unwrap_or(0)
                    + 1;
                for j in 0..s.n as usize {
                    let id = next_id + j as u32;
                    t.columns.insert(k + j, String::new());
                    t.column_xml.insert(k + j, format!("<tableColumn id=\"{id}\" name=\"\"/>"));
                    t.rename_column(k + j, "");
                }
            } else if !s.insert {
                let lo = s.at.max(c0);
                let hi = (s.at + s.n - 1).min(c1);
                if lo <= hi {
                    let (a, b) = ((lo - c0) as usize, (hi - c0) as usize + 1);
                    t.columns.drain(a..b.min(t.columns.len()));
                    t.column_xml.drain(a..b.min(t.column_xml.len()));
                }
            }
        }
        t.range = range;
        shift_filter(&mut t.filter);
        if let Some(f) = &mut t.filter {
            // O filtro da tabela cobre a tabela sem a linha de totais.
            f.range = CellRange { last_row: range.last_row.saturating_sub(t.totals_rows), ..range };
        }
        tables.push(t);
    }
    l.tables = tables;
    // Desenhos, tabelas dinâmicas e as fontes dos caches.
    for o in &mut l.objects {
        o.anchor = o.anchor.shifted(s);
    }
    for a in &mut l.drawing_anchors {
        a.anchor = a.anchor.shifted(s);
    }
    for p in &mut l.pivots {
        if let Some(r) = s.range(p.range) {
            p.range = r;
        }
    }
    l.pivot_sources.retain_mut(|(_, r)| match s.range(*r) {
        Some(n) => {
            *r = n;
            true
        }
        None => false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_move() {
        use crate::drawing::{Anchor, Marker};
        let m = |row| Marker { col: 1, col_off: 5, row, row_off: 7 };
        let a = Anchor::Two { from: m(3), to: m(8) };
        // Inserir acima move; inserir dentro estica.
        let ins = Shift { axis: Axis::Rows, at: 2, n: 2, insert: true };
        assert_eq!(a.shifted(ins), Anchor::Two { from: m(5), to: m(10) });
        let ins = Shift { axis: Axis::Rows, at: 5, n: 2, insert: true };
        assert_eq!(a.shifted(ins), Anchor::Two { from: m(3), to: m(10) });
        // Apagar o começo: o canto fica no começo do trecho apagado.
        let del = Shift { axis: Axis::Rows, at: 2, n: 3, insert: false };
        let Anchor::Two { from, to } = a.shifted(del) else { panic!() };
        assert_eq!((from.row, from.row_off, to.row), (2, 0, 5));
    }

    #[test]
    fn indexes_and_spans() {
        let ins = Shift { axis: Axis::Rows, at: 5, n: 2, insert: true };
        assert_eq!((ins.index(4), ins.index(5), ins.index(9)), (Some(4), Some(7), Some(11)));
        assert_eq!((ins.source(5), ins.source(6), ins.source(7)), (None, None, Some(5)));
        assert_eq!(ins.span(2, 4), Some((2, 4)));
        assert_eq!(ins.span(2, 5), Some((2, 7)), "inserir dentro aumenta");
        assert_eq!(ins.span(5, 6), Some((7, 8)));
        let del = ins.inverse();
        assert_eq!((del.index(4), del.index(5), del.index(6), del.index(7)), (Some(4), None, None, Some(5)));
        assert_eq!(del.span(5, 6), None);
        assert_eq!(del.span(3, 9), Some((3, 7)));
        assert_eq!(del.span(6, 9), Some((5, 7)));
        assert_eq!(del.span(2, 5), Some((2, 4)));
    }
}
