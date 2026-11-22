//! Edição de uma aba (F3): escrita direta nos blocos, rastreio do que mudou
//! (para o gravador remendar só isso) e comandos com inverso (ADR-009).
//!
//! - Escrever numa célula descompacta o bloco dela; [`Sheet::settle`]
//!   recompacta os blocos tocados no fim de cada comando.
//! - O texto digitado vai para uma tabela de strings própria das edições.
//!   Tabelas e fórmulas só crescem: um [`RawCell`] guardado para desfazer
//!   continua válido para sempre.

use std::collections::{BTreeMap, BTreeSet};

use crate::{BLOCK_ROWS, CellError, FormulaDef, FormulaKind, SHARED_BIT, Sheet, StringTable, kind};

/// Uma célula como está guardada (tipo, estilo, payload e id de fórmula).
/// Serve para desfazer: restaurar um `RawCell` devolve a célula exatamente
/// como era.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawCell {
    pub kind: u8,
    pub style: u16,
    pub payload: u64,
    pub formula: u32,
}

impl RawCell {
    pub fn is_blank(&self) -> bool {
        self.kind == kind::EMPTY && self.style == 0 && self.formula == 0
    }
}

/// Valor novo de uma célula.
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(CellError),
}

/// O que mudou numa aba desde o último salvamento.
#[derive(Clone, Debug, Default)]
pub struct Edits {
    /// Células editadas, por linha.
    pub cells: BTreeMap<u32, BTreeSet<u32>>,
    /// Linhas com altura ou visibilidade mudada.
    pub rows: BTreeSet<u32>,
    /// Colunas com largura mudada (`<cols>` é remendado).
    pub cols: BTreeSet<u32>,
    /// Algum valor mudou (o Excel precisa recalcular ao abrir).
    pub values: bool,
    /// Alguma fórmula mudou (o `calcChain.xml` deixa de valer).
    pub formulas: bool,
    /// Linhas ou colunas inseridas/apagadas: a aba é regravada inteira a
    /// partir do modelo, e as referências no resto do XML dela andam com
    /// `shifts` (em ordem).
    pub structure: bool,
    pub shifts: Vec<crate::Shift>,
    /// Mesclagens mudadas (`<mergeCells>` é regravado).
    pub merges: bool,
    /// Painéis congelados mudados (`<pane>` do `<sheetView>`).
    pub pane: bool,
}

impl Edits {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
            && self.rows.is_empty()
            && self.cols.is_empty()
            && !self.values
            && !self.formulas
            && !self.structure
            && !self.merges
            && !self.pane
    }

    fn mark(&mut self, row: u32, col: u32) {
        self.cells.entry(row).or_default().insert(col);
    }

    /// A célula foi editada.
    pub fn has(&self, row: u32, col: u32) -> bool {
        self.cells.get(&row).is_some_and(|c| c.contains(&col))
    }
}

impl Sheet {
    /// O que mudou desde o último salvamento.
    pub fn edits(&self) -> &Edits {
        &self.edits
    }

    /// Esquece as mudanças (depois de salvar).
    pub fn clear_edits(&mut self) {
        self.edits = Edits::default();
    }

    /// Marca a célula como editada sem mudar o conteúdo (ex.: para o
    /// gravador escrevê-la a partir do modelo).
    pub fn mark_edited(&mut self, row: u32, col: u32) {
        self.edits.mark(row, col);
    }

    /// A célula como está guardada.
    pub fn raw(&self, row: u32, col: u32) -> RawCell {
        let Some(column) = self.columns.get(col as usize) else { return RawCell::default() };
        let formula = column.formula_id(row as usize);
        match column.get(row as usize) {
            Some((b, i)) => RawCell { kind: b.kind(i), style: b.style(i), payload: b.payload(i), formula },
            None => RawCell { formula, ..Default::default() },
        }
    }

    /// Grava a célula exatamente como `cell` (ver [`RawCell`]).
    pub fn set_raw(&mut self, row: u32, col: u32, cell: RawCell) {
        let old = self.raw(row, col);
        if old == cell {
            return;
        }
        self.unshare_if_master(old.formula, cell.formula);
        let (c, r) = (col as usize, row as usize);
        let bi = r / BLOCK_ROWS;
        let i = r % BLOCK_ROWS;
        let exists = self.columns.get(c).is_some_and(|col| col.get(r).is_some());
        if exists || cell.kind != kind::EMPTY || cell.style != 0 {
            if c >= self.columns.len() {
                self.columns.resize_with(c + 1, Default::default);
            }
            let b = self.columns[c].block_mut(bi);
            b.kind[i] = cell.kind;
            b.style[i] = cell.style;
            b.payload[i] = cell.payload;
        }
        let has_ids = self.columns.get(c).is_some_and(|col| matches!(col.formulas.get(bi), Some(Some(_))));
        if has_ids || cell.formula != 0 {
            if c >= self.columns.len() {
                self.columns.resize_with(c + 1, Default::default);
            }
            self.columns[c].formula_block_mut(bi)[i] = cell.formula;
        }
        if !cell.is_blank() {
            self.rows = self.rows.max(row + 1);
            self.cols = self.cols.max(col + 1);
        }
        self.touched.push((c as u32, bi as u32));
        self.edits.mark(row, col);
        if old.kind & kind::TYPE_MASK != cell.kind & kind::TYPE_MASK || old.payload != cell.payload {
            self.edits.values = true;
        }
        if old.formula != cell.formula || (old.kind ^ cell.kind) & kind::FORMULA != 0 {
            self.edits.formulas = true;
        }
    }

    /// Grava um valor (e uma fórmula, sem o `=`). `style = None` mantém o
    /// estilo atual. O valor de uma fórmula é o resultado (sem motor de
    /// cálculo, quem chama decide; `Empty` = ainda sem resultado).
    pub fn set_cell(
        &mut self,
        row: u32,
        col: u32,
        value: CellValue,
        formula: Option<&str>,
        style: Option<u16>,
    ) {
        let old = self.raw(row, col);
        let (mut k, payload) = match value {
            CellValue::Empty => (kind::EMPTY, 0),
            CellValue::Number(x) => (kind::NUMBER, x.to_bits()),
            CellValue::Text(s) => {
                let (t, i) = self.push_edit_string(&s);
                (kind::LOCAL_STR, (t << 32) | i as u64)
            }
            CellValue::Bool(b) => (kind::BOOL, b as u64),
            CellValue::Error(e) => (kind::ERROR, e as u64),
        };
        let formula_id = match formula {
            Some(text) => {
                k |= kind::FORMULA;
                let t = self.formula_texts.push(text);
                self.formula_defs.push(FormulaDef {
                    text: t,
                    kind: FormulaKind::Normal,
                    row,
                    col,
                    template: false,
                });
                self.formula_defs.len() as u32
            }
            None => 0,
        };
        let style = style.unwrap_or(old.style);
        self.set_raw(row, col, RawCell { kind: k, style, payload, formula: formula_id });
    }

    /// Troca só o estilo da célula (valor e fórmula ficam).
    pub fn set_style(&mut self, row: u32, col: u32, style: u16) {
        let mut raw = self.raw(row, col);
        raw.style = style;
        self.set_raw(row, col, raw);
    }

    /// Recompacta os blocos tocados desde a última chamada (fim de um comando).
    pub fn settle(&mut self) {
        let mut touched = std::mem::take(&mut self.touched);
        touched.sort_unstable();
        touched.dedup();
        for (c, bi) in touched {
            if let Some(col) = self.columns.get_mut(c as usize) {
                let bi = bi as usize;
                col.pack_range(bi..bi + 1);
            }
        }
    }

    /// Tabela de strings das edições (criada na primeira edição de texto).
    fn push_edit_string(&mut self, s: &str) -> (u64, u32) {
        let t = match self.edit_table {
            Some(t) => t,
            None => {
                self.strings.push(StringTable::new());
                let t = self.strings.len() - 1;
                self.edit_table = Some(t);
                t
            }
        };
        (t as u64, self.strings[t].push(s))
    }

    /// Se a célula era a mestre de um grupo de fórmulas compartilhadas e a
    /// fórmula dela mudou, as outras células do grupo passam a ser gravadas
    /// com a fórmula própria (o texto do grupo só existe na mestre).
    fn unshare_if_master(&mut self, old: u32, new: u32) {
        if old == 0 || old == new || old & SHARED_BIT != 0 {
            return;
        }
        let Some(def) = self.formula_defs.get(old as usize - 1) else { return };
        let FormulaKind::Shared { si } = def.kind else { return };
        let member = SHARED_BIT | (si & !SHARED_BIT);
        let mut found = Vec::new();
        for (c, col) in self.columns.iter().enumerate() {
            for (bi, ids) in col.formulas.iter().enumerate() {
                let Some(ids) = ids else { continue };
                for i in 0..BLOCK_ROWS {
                    if ids.get(i) == member {
                        found.push(((bi * BLOCK_ROWS + i) as u32, c as u32));
                    }
                }
            }
        }
        for (r, c) in found {
            self.edits.mark(r, c);
        }
    }
}

// ---------------------------------------------------------------------------
// Larguras e alturas
// ---------------------------------------------------------------------------

impl Sheet {
    /// Largura da coluna em caracteres (`None` = padrão da aba).
    pub fn col_width(&self, col: u32) -> Option<f32> {
        self.layout.cols.iter().find(|s| s.first <= col && col <= s.last).and_then(|s| s.width)
    }

    /// Muda a largura de uma coluna (caracteres; `None` = padrão da aba).
    pub fn set_col_width(&mut self, col: u32, width: Option<f32>) {
        self.change_col(col, |c| c.width = width);
    }

    /// A coluna está oculta.
    pub fn col_hidden(&self, col: u32) -> bool {
        self.layout.cols.iter().find(|s| s.first <= col && col <= s.last).is_some_and(|s| s.hidden)
    }

    pub fn set_col_hidden(&mut self, col: u32, hidden: bool) {
        self.change_col(col, |c| c.hidden = hidden);
    }

    /// A linha está oculta.
    pub fn row_hidden(&self, row: u32) -> bool {
        self.layout.row(row).is_some_and(|r| r.hidden)
    }

    pub fn set_row_hidden(&mut self, row: u32, hidden: bool) {
        let rows = &mut self.layout.rows;
        match rows.binary_search_by_key(&row, |r| r.row) {
            Ok(i) => {
                rows[i].hidden = hidden;
                if !hidden && rows[i].height.is_none() && rows[i].style.is_none() {
                    rows.remove(i);
                }
            }
            Err(i) if hidden => rows.insert(i, crate::RowSpec { row, height: None, hidden, style: None }),
            Err(_) => {}
        }
        self.edits.rows.insert(row);
    }

    /// Muda a especificação de uma coluna, separando-a do `<col>` que a cobre.
    fn change_col(&mut self, col: u32, f: impl FnOnce(&mut crate::ColSpec)) {
        let cols = &mut self.layout.cols;
        match cols.iter().position(|s| s.first <= col && col <= s.last) {
            Some(i) => {
                let s = cols[i];
                let mut mid = s;
                mid.first = col;
                mid.last = col;
                f(&mut mid);
                let mut repl = Vec::with_capacity(3);
                if s.first < col {
                    repl.push(crate::ColSpec { last: col - 1, ..s });
                }
                repl.push(mid);
                if col < s.last {
                    repl.push(crate::ColSpec { first: col + 1, ..s });
                }
                cols.splice(i..i + 1, repl);
            }
            None => {
                let at = cols.partition_point(|s| s.first < col);
                let mut spec = crate::ColSpec { first: col, last: col, width: None, hidden: false, style: 0 };
                f(&mut spec);
                cols.insert(at, spec);
            }
        }
        self.edits.cols.insert(col);
    }

    /// Altura da linha em pontos (`None` = automática).
    pub fn row_height(&self, row: u32) -> Option<f32> {
        let rows = &self.layout.rows;
        rows.binary_search_by_key(&row, |r| r.row).ok().and_then(|i| rows[i].height)
    }

    /// Muda a altura de uma linha (pontos; `None` = automática).
    pub fn set_row_height(&mut self, row: u32, height: Option<f32>) {
        let rows = &mut self.layout.rows;
        match rows.binary_search_by_key(&row, |r| r.row) {
            Ok(i) => {
                rows[i].height = height;
                // Sem nada de especial, a linha sai da lista (como na leitura).
                if height.is_none() && !rows[i].hidden && rows[i].style.is_none() {
                    rows.remove(i);
                }
            }
            Err(i) if height.is_some() => {
                rows.insert(i, crate::RowSpec { row, height, hidden: false, style: None })
            }
            Err(_) => {}
        }
        self.edits.rows.insert(row);
    }
}

// ---------------------------------------------------------------------------
// Comandos (ADR-009)
// ---------------------------------------------------------------------------

/// Conteúdo novo de uma célula.
#[derive(Clone, Debug, PartialEq)]
pub struct CellChange {
    pub row: u32,
    pub col: u32,
    pub value: CellValue,
    /// Fórmula sem o `=` (o valor é o resultado, se conhecido).
    pub formula: Option<String>,
    /// `None` mantém o estilo atual.
    pub style: Option<u16>,
}

/// As operações de fórmula de uma mudança de estrutura (ver [`crate::FormulaOps`]).
#[derive(Clone)]
pub struct OpsHandle(pub std::sync::Arc<dyn crate::FormulaOps>);

impl std::fmt::Debug for OpsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FormulaOps")
    }
}

/// Uma mudança numa aba. [`Command::apply`] devolve o comando que desfaz.
#[derive(Clone, Debug)]
pub enum Command {
    SetCells(Vec<CellChange>),
    /// Células de volta exatamente como eram (o inverso de mudar células).
    Restore(Vec<(u32, u32, RawCell)>),
    /// Larguras em caracteres (`None` = padrão).
    ColWidths(Vec<(u32, Option<f32>)>),
    /// Alturas em pontos (`None` = automática).
    RowHeights(Vec<(u32, Option<f32>)>),
    /// Estilos de células (formatação).
    SetStyles(Vec<(u32, u32, u16)>),
    /// Mesclagens: as de `add` entram, as de `remove` saem.
    Merges {
        add: Vec<crate::CellRange>,
        remove: Vec<crate::CellRange>,
    },
    /// Linhas ocultas (`true`) ou reexibidas.
    RowsHidden(Vec<(u32, bool)>),
    ColsHidden(Vec<(u32, bool)>),
    /// Painéis congelados (`None` = sem).
    Pane(Option<crate::Pane>),
    /// Inserir ou apagar linhas/colunas.
    Shift(crate::Shift, OpsHandle),
    /// O layout inteiro de volta (desfazer de uma mudança de estrutura).
    Layout(Box<crate::Layout>),
    /// Vários, em ordem (o inverso vem na ordem contrária).
    Group(Vec<Command>),
}

impl Command {
    /// Aplica e devolve o inverso.
    pub fn apply(self, sheet: &mut Sheet) -> Command {
        let inverse = match self {
            Command::SetCells(changes) => {
                let mut old = Vec::with_capacity(changes.len());
                for ch in changes {
                    old.push((ch.row, ch.col, sheet.raw(ch.row, ch.col)));
                    sheet.set_cell(ch.row, ch.col, ch.value, ch.formula.as_deref(), ch.style);
                }
                old.reverse();
                Command::Restore(old)
            }
            Command::Restore(cells) => {
                let mut old = Vec::with_capacity(cells.len());
                for (r, c, raw) in cells {
                    old.push((r, c, sheet.raw(r, c)));
                    sheet.set_raw(r, c, raw);
                }
                old.reverse();
                Command::Restore(old)
            }
            Command::ColWidths(list) => {
                let mut old = Vec::with_capacity(list.len());
                for (c, w) in list {
                    old.push((c, sheet.col_width(c)));
                    sheet.set_col_width(c, w);
                }
                old.reverse();
                Command::ColWidths(old)
            }
            Command::Shift(s, ops) => {
                let undo = sheet.apply_shift(s, &*ops.0);
                let back = OpsHandle(ops.0.with_shift(s.inverse()));
                Command::Group(vec![
                    Command::Shift(s.inverse(), back),
                    Command::Restore(undo.lost),
                    Command::Layout(undo.layout),
                ])
            }
            Command::Layout(l) => {
                let old = std::mem::replace(&mut sheet.layout, *l);
                Command::Layout(Box::new(old))
            }
            Command::Group(list) => {
                let mut inv: Vec<Command> = list.into_iter().map(|c| c.apply(sheet)).collect();
                inv.reverse();
                Command::Group(inv)
            }
            Command::Merges { add, remove } => {
                let m = &mut sheet.layout.merges;
                m.retain(|r| !remove.contains(r));
                for r in &add {
                    if !m.contains(r) {
                        m.push(*r);
                    }
                }
                m.sort_by_key(|r| (r.first_row, r.first_col));
                sheet.edits.merges = true;
                Command::Merges { add: remove, remove: add }
            }
            Command::RowsHidden(list) => {
                let mut old = Vec::with_capacity(list.len());
                for (r, h) in list {
                    old.push((r, sheet.row_hidden(r)));
                    sheet.set_row_hidden(r, h);
                }
                old.reverse();
                Command::RowsHidden(old)
            }
            Command::ColsHidden(list) => {
                let mut old = Vec::with_capacity(list.len());
                for (c, h) in list {
                    old.push((c, sheet.col_hidden(c)));
                    sheet.set_col_hidden(c, h);
                }
                old.reverse();
                Command::ColsHidden(old)
            }
            Command::Pane(p) => {
                let old = std::mem::replace(&mut sheet.layout.pane, p);
                sheet.edits.pane = true;
                Command::Pane(old)
            }
            Command::SetStyles(list) => {
                let mut old = Vec::with_capacity(list.len());
                for (r, c, st) in list {
                    old.push((r, c, sheet.raw(r, c).style));
                    sheet.set_style(r, c, st);
                }
                old.reverse();
                Command::SetStyles(old)
            }
            Command::RowHeights(list) => {
                let mut old = Vec::with_capacity(list.len());
                for (r, h) in list {
                    old.push((r, sheet.row_height(r)));
                    sheet.set_row_height(r, h);
                }
                old.reverse();
                Command::RowHeights(old)
            }
        };
        sheet.settle();
        inverse
    }

    /// Células tocadas pelo comando (para atualizar a tela).
    pub fn cells(&self) -> Vec<(u32, u32)> {
        match self {
            Command::SetCells(v) => v.iter().map(|c| (c.row, c.col)).collect(),
            Command::Restore(v) => v.iter().map(|c| (c.0, c.1)).collect(),
            Command::SetStyles(v) => v.iter().map(|c| (c.0, c.1)).collect(),
            Command::Group(v) => v.iter().flat_map(Command::cells).collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SheetPart, Value};

    #[test]
    fn commands_undo_and_redo() {
        let shared = StringTable::new();
        let mut s = sample();
        let snap = |s: &Sheet| {
            (0..12)
                .flat_map(|r| (0..4).map(move |c| (r, c)))
                .map(|(r, c)| {
                    (format!("{:?}", s.cell(r, c, &shared)), s.formula(r, c).map(|f| f.text.to_string()))
                })
                .collect::<Vec<_>>()
        };
        let before = snap(&s);
        let cmd = Command::SetCells(vec![
            CellChange { row: 1, col: 0, value: CellValue::Text("x".into()), formula: None, style: None },
            CellChange {
                row: 1,
                col: 0,
                value: CellValue::Number(2.0),
                formula: Some("1+1".into()),
                style: Some(4),
            },
            CellChange { row: 6, col: 1, value: CellValue::Empty, formula: None, style: None },
        ]);
        let undo = cmd.apply(&mut s);
        let after = snap(&s);
        assert_ne!(after, before);
        assert_eq!(s.formula(1, 0).unwrap().text, "1+1");
        let redo = undo.apply(&mut s);
        assert_eq!(snap(&s), before);
        redo.apply(&mut s);
        assert_eq!(snap(&s), after);

        let w = Command::ColWidths(vec![(2, Some(20.0))]).apply(&mut s);
        assert_eq!(s.col_width(2), Some(20.0));
        w.apply(&mut s);
        assert_eq!(s.col_width(2), None);
        let h = Command::RowHeights(vec![(3, Some(30.0))]).apply(&mut s);
        assert_eq!(s.row_height(3), Some(30.0));
        h.apply(&mut s);
        assert_eq!(s.row_height(3), None);
        assert!(s.edits().cols.contains(&2) && s.edits().rows.contains(&3));
    }

    #[test]
    fn column_specs_split() {
        let mut s = Sheet::new("t");
        s.layout.cols.push(crate::ColSpec { first: 0, last: 9, width: Some(5.0), hidden: false, style: 3 });
        s.set_col_width(4, Some(12.0));
        let spans: Vec<_> = s.layout.cols.iter().map(|c| (c.first, c.last, c.width, c.style)).collect();
        assert_eq!(spans, vec![(0, 3, Some(5.0), 3), (4, 4, Some(12.0), 3), (5, 9, Some(5.0), 3)]);
        s.set_col_width(20, Some(1.0));
        assert_eq!(s.layout.cols.last().unwrap().first, 20);
    }

    fn sample() -> Sheet {
        let mut p = SheetPart::new();
        for r in 0..3000u32 {
            p.set(r, 0, kind::NUMBER, 2, (r as f64).to_bits());
        }
        p.set(5, 1, kind::NUMBER | kind::FORMULA, 0, 10f64.to_bits());
        p.set_formula(5, 1, "A6*2", FormulaKind::Shared { si: 0 });
        p.set(6, 1, kind::NUMBER | kind::FORMULA, 0, 12f64.to_bits());
        p.set_shared_formula(6, 1, 0);
        let mut s = Sheet::new("t");
        s.merge_part(p);
        s.finish();
        s
    }

    #[test]
    fn set_and_restore_cells() {
        let shared = StringTable::new();
        let mut s = sample();
        let before = s.raw(10, 0);
        s.set_cell(10, 0, CellValue::Text("olá".into()), None, None);
        s.set_cell(2000, 3, CellValue::Number(1.5), Some("A1+0,5"), Some(7));
        s.settle();
        assert_eq!(s.cell(10, 0, &shared).value, Value::Text("olá"));
        assert_eq!(s.cell(10, 0, &shared).style, 2);
        let c = s.cell(2000, 3, &shared);
        assert_eq!((c.value, c.style, c.formula), (Value::Number(1.5), 7, true));
        assert_eq!(s.formula(2000, 3).unwrap().text, "A1+0,5");
        assert_eq!(s.cols(), 4);
        assert!(s.edits().has(10, 0) && s.edits().has(2000, 3));
        assert!(s.edits().values && s.edits().formulas);
        // Desfazer: o RawCell antigo devolve a célula exatamente.
        s.set_raw(10, 0, before);
        s.settle();
        assert_eq!(s.cell(10, 0, &shared).value, Value::Number(10.0));
        // Apagar.
        s.set_cell(11, 0, CellValue::Empty, None, Some(0));
        assert_eq!(s.cell(11, 0, &shared).value, Value::Empty);
        assert!(s.raw(11, 0).is_blank());
    }

    #[test]
    fn editing_shared_master_marks_members() {
        let mut s = sample();
        s.set_cell(5, 1, CellValue::Number(1.0), None, None);
        assert!(s.edits().has(6, 1), "a outra célula do grupo precisa ser regravada");
        // O membro continua com a fórmula do grupo (texto deslocado da mestre).
        assert_eq!(s.formula(6, 1).unwrap().text, "A6*2");
    }
}
