//! Tabelas dinâmicas no app (F5): clicar numa segmentação de tabela
//! dinâmica muda os itens escondidos das tabelas ligadas, recalcula cada uma
//! com o motor do `duck-calc` e reescreve as células (num passo de desfazer).
//! Fórmulas e gráficos que leem essas células acompanham pelo recálculo.

use std::collections::{HashMap, HashSet};

use duck_calc::pivot::{self as engine, PivotResult};
use duck_core::drawing::ObjectKind;
use duck_core::pivot::{PItem, PivotCache, PivotTable, Records};
use duck_core::{CellChange, CellRange, CellValue, Command, Sheet, StringTable};

/// Caches com os registros já lidos (sob demanda: só no primeiro clique).
#[derive(Default)]
pub struct PivotState {
    pub loaded: HashMap<u32, (PivotCache, Records)>,
    /// Abas com tabelas dinâmicas ou segmentações mudadas (para salvar).
    pub changed: HashSet<usize>,
}

/// Valor de célula de um item (rótulo).
fn item_value(it: &PItem) -> CellValue {
    match it {
        PItem::Num(x) | PItem::Date(x) => CellValue::Number(*x),
        PItem::Bool(b) => CellValue::Bool(*b),
        PItem::Str(s) => CellValue::Text(s.clone()),
        PItem::Err(_) => CellValue::Error(duck_core::CellError::NA),
        PItem::Missing => CellValue::Text("(vazio)".into()),
    }
}

fn label(t: &PivotTable, cache: &PivotCache, field: i32, pos: u32) -> CellValue {
    if field < 0 {
        return CellValue::Text(t.data_fields.get(pos as usize).map(|d| d.name.clone()).unwrap_or_default());
    }
    let f = &t.fields[field as usize];
    let Some(it) = f.items.get(pos as usize) else { return CellValue::Empty };
    if let Some(n) = &it.name {
        return CellValue::Text(n.clone());
    }
    let Some(x) = it.x else { return CellValue::Empty };
    cache
        .fields
        .get(field as usize)
        .and_then(|cf| cf.items.get(x as usize))
        .map_or(CellValue::Empty, item_value)
}

fn text_of(v: &CellValue) -> String {
    match v {
        CellValue::Text(s) => s.clone(),
        CellValue::Number(x) => {
            let mut s = String::new();
            duck_calc::value::num_to_text(*x, &mut s);
            s
        }
        _ => String::new(),
    }
}

/// Células do resultado: (linha, coluna, valor), e o novo intervalo.
pub fn render(
    t: &PivotTable,
    cache: &PivotCache,
    res: &PivotResult,
) -> (Vec<(u32, u32, CellValue)>, CellRange) {
    let mut out = Vec::new();
    let (r0, c0) = (t.range.first_row, t.range.first_col);
    let (dr, dc) = (r0 + t.first_data_row, c0 + t.first_data_col);
    let grand = || CellValue::Text(t.grand_caption.clone().unwrap_or_else(|| "Total Geral".into()));
    let label_cols = t.first_data_col.max(1);
    let row_fields: Vec<i32> = t.row_fields.clone();
    let mut path: Vec<u32> = Vec::new();
    for (i, it) in res.row_items.iter().enumerate() {
        let row = dr + i as u32;
        for k in 0..label_cols {
            out.push((row, c0 + k, CellValue::Empty));
        }
        match it.t.as_deref() {
            Some("grand") => out.push((row, c0, grand())),
            Some(_) => {
                // Subtotal: "<item> Total".
                let lvl = it.r as usize;
                path.truncate(lvl);
                path.extend(&it.x);
                let f = row_fields.get(lvl).copied().unwrap_or(-1);
                let name = text_of(&label(t, cache, f, *it.x.first().unwrap_or(&0)));
                let col = if label_cols > 1 { c0 + (lvl as u32).min(label_cols - 1) } else { c0 };
                out.push((row, col, CellValue::Text(format!("{name} Total"))));
            }
            None => {
                path.truncate(it.r as usize);
                path.extend(&it.x);
                if label_cols == 1 {
                    // Forma compacta: o item do nível mais fundo, numa coluna só.
                    let lvl = path.len().saturating_sub(1);
                    let f = row_fields.get(lvl).copied().unwrap_or(-1);
                    out.push((row, c0, label(t, cache, f, *path.last().unwrap_or(&0))));
                } else {
                    // Tabular/estrutura de tópicos: um campo por coluna, a partir do primeiro novo.
                    for (lvl, &p) in path.iter().enumerate().skip(it.r as usize) {
                        let f = row_fields.get(lvl).copied().unwrap_or(-1);
                        let col = c0 + (lvl as u32).min(label_cols - 1);
                        out.push((row, col, label(t, cache, f, p)));
                    }
                }
            }
        }
        for j in 0..res.col_items.len() {
            let v = engine::data_value(t, res, i, j).unwrap_or(CellValue::Empty);
            out.push((row, dc + j as u32, v));
        }
    }
    // Cabeçalho das colunas (quando há campos nas colunas).
    if t.col_fields.iter().any(|&f| f >= 0) && dr > r0 {
        let hr = dr - 1;
        let mut cpath: Vec<u32> = Vec::new();
        for (j, it) in res.col_items.iter().enumerate() {
            let col = dc + j as u32;
            let v = match it.t.as_deref() {
                Some("grand") => grand(),
                _ => {
                    cpath.truncate(it.r as usize);
                    cpath.extend(&it.x);
                    let lvl = cpath.len().saturating_sub(1);
                    let f = t.col_fields.get(lvl).copied().unwrap_or(-1);
                    label(t, cache, f, *cpath.last().unwrap_or(&0))
                }
            };
            out.push((hr, col, v));
        }
    }
    let rows = res.row_items.len().max(1) as u32;
    let cols = res.col_items.len().max(1) as u32;
    let range = CellRange { first_row: r0, first_col: c0, last_row: dr + rows - 1, last_col: dc + cols - 1 };
    (out, range)
}

/// Mudanças de célula para trocar o resultado antigo pelo novo: escreve o
/// novo, limpa o que sobrou do antigo; linhas novas copiam o estilo da última
/// linha antiga da mesma coluna.
pub fn changes(
    sheet: &Sheet,
    shared: &StringTable,
    old: CellRange,
    new_cells: Vec<(u32, u32, CellValue)>,
    new: CellRange,
) -> Vec<CellChange> {
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut out = Vec::new();
    for (r, c, v) in new_cells {
        if !seen.insert((r, c)) {
            out.retain(|x: &CellChange| (x.row, x.col) != (r, c));
        }
        let cur = sheet.cell(r, c, shared);
        let style = if r > old.last_row { Some(sheet.cell(old.last_row, c, shared).style) } else { None };
        let same = match (&v, cur.value) {
            (CellValue::Empty, duck_core::Value::Empty) => true,
            (CellValue::Number(a), duck_core::Value::Number(b)) => *a == b,
            (CellValue::Text(a), duck_core::Value::Text(b)) => a == b,
            _ => false,
        };
        if !same || style.is_some_and(|s| s != cur.style) {
            out.push(CellChange { row: r, col: c, value: v, formula: None, style });
        }
    }
    // Limpa o que sobrou do resultado antigo.
    for r in old.first_row..=old.last_row {
        for c in old.first_col..=old.last_col {
            if new.contains(r, c) || seen.contains(&(r, c)) {
                continue;
            }
            if sheet.has_value(r, c) {
                out.push(CellChange { row: r, col: c, value: CellValue::Empty, formula: None, style: None });
            }
        }
    }
    out
}

impl crate::app::SheetsApp {
    /// Clique numa segmentação de tabela dinâmica.
    pub fn pivot_slicer_click(&mut self, obj: usize, item: Option<usize>, ctrl: bool) {
        let Some(doc) = self.doc.as_ref() else { return };
        let active = doc.active;
        let Some((sheet, _)) = doc.current() else { return };
        let Some(ObjectKind::Slicer(s)) = sheet.layout.objects.get(obj).map(|o| o.kind.clone()) else {
            return;
        };
        let Some(link) = s.pivot.clone() else { return };
        // Nova seleção (como nas segmentações de tabela).
        let all = s.items.iter().all(|i| i.1);
        let mut chosen: Vec<bool> = s.items.iter().map(|i| i.1 && !all).collect();
        match item {
            None => chosen.iter_mut().for_each(|c| *c = false),
            Some(i) if ctrl => {
                if all {
                    chosen = s.items.iter().map(|_| true).collect();
                }
                if let Some(c) = chosen.get_mut(i) {
                    *c = !*c;
                }
            }
            Some(i) => chosen = (0..s.items.len()).map(|j| j == i).collect(),
        }
        if chosen.iter().all(|c| !*c) {
            chosen = vec![true; s.items.len()];
        }
        let visible: HashSet<u32> =
            link.item_x.iter().zip(&chosen).filter(|(_, c)| **c).map(|(x, _)| *x).collect();

        // Cache (com os registros, lidos uma vez).
        let Some(meta) = doc
            .calc
            .extras
            .pivot_caches
            .iter()
            .find(|c| c.x14_id.as_deref() == Some(link.cache_id.as_str()))
            .cloned()
        else {
            self.message = "Cache da tabela dinâmica não encontrado".into();
            return;
        };
        if meta.external {
            self.message = "Tabela dinâmica do Modelo de Dados: o filtro precisa do Excel".into();
            return;
        }
        let cache_id = meta.id;
        let path = doc.path.clone();
        if !doc.pivots.loaded.contains_key(&cache_id) {
            let mut c = meta.clone();
            let Some(rec) = duck_xlsx::Book::open(&path).ok().and_then(|b| b.read_pivot_records(&mut c))
            else {
                self.message = "Não foi possível ler os registros da tabela dinâmica".into();
                return;
            };
            if let Some(d) = self.doc.as_mut() {
                d.pivots.loaded.insert(cache_id, (c, rec));
            }
        }
        let Some(doc) = self.doc.as_ref() else { return };
        let (cache, rec) = &doc.pivots.loaded[&cache_id];
        let Some(field) = cache.fields.iter().position(|f| f.name.trim() == link.source.trim()) else {
            return;
        };
        let names: HashSet<&str> = link.tables.iter().map(|t| t.1.as_str()).collect();

        // Novas tabelas e as mudanças de célula, aba por aba.
        let mut step: crate::edit::Step = Vec::new();
        let mut new_tables: Vec<(usize, usize, PivotTable)> = Vec::new();
        for (si, slot) in doc.sheets.iter().enumerate() {
            let Some(sh) = &slot.data else { continue };
            let mut cells = Vec::new();
            for (ti, t) in sh.layout.pivots.iter().enumerate() {
                if t.cache_id != cache_id || !names.contains(t.name.as_str()) {
                    continue;
                }
                let mut nt = t.clone();
                if let Some(f) = nt.fields.get_mut(field) {
                    for it in &mut f.items {
                        if it.t.is_none()
                            && let Some(x) = it.x
                        {
                            it.hidden = !visible.contains(&x);
                        }
                    }
                }
                let res = engine::compute(&nt, cache, rec);
                let (out, range) = render(&nt, cache, &res);
                cells.extend(changes(sh, &doc.strings, t.range, out, range));
                nt.range = range;
                nt.row_items = res.row_items;
                nt.col_items = res.col_items;
                new_tables.push((si, ti, nt));
            }
            if !cells.is_empty() {
                step.push((si, Command::SetCells(cells)));
            }
        }
        // Grava: modelos das tabelas, seleção das segmentações e as células.
        let part = link.part.clone();
        let Some(doc) = self.doc.as_mut() else { return };
        for (si, ti, nt) in new_tables {
            if let Some(sh) = doc.sheets.get_mut(si).and_then(crate::edit::sheet_mut)
                && let Some(t) = sh.layout.pivots.get_mut(ti)
            {
                *t = nt;
                sh.mark_pivots();
            }
            doc.pivots.changed.insert(si);
        }
        for si in 0..doc.sheets.len() {
            let has = doc.sheets[si].data.as_ref().is_some_and(|d| {
                d.layout.objects.iter().any(|o| matches!(&o.kind, ObjectKind::Slicer(x) if x.pivot.as_ref().is_some_and(|p| p.part == part)))
            });
            if !has {
                continue;
            }
            if let Some(sh) = doc.sheets.get_mut(si).and_then(crate::edit::sheet_mut) {
                sh.mark_pivots();
                for o in &mut sh.layout.objects {
                    if let ObjectKind::Slicer(x) = &mut o.kind
                        && x.pivot.as_ref().is_some_and(|p| p.part == part)
                    {
                        for (it, c) in x.items.iter_mut().zip(&chosen) {
                            it.1 = *c;
                        }
                    }
                }
            }
            doc.pivots.changed.insert(si);
        }
        doc.dirty = true;
        let n = step.iter().map(|s| match &s.1 {
            Command::SetCells(c) => c.len(),
            _ => 0,
        });
        let total: usize = n.sum();
        if !step.is_empty() {
            self.run_step(step);
        }
        if self.doc.as_ref().is_some_and(|d| d.active != active) {
            self.switch_sheet(active);
        }
        self.message = format!("Tabelas dinâmicas atualizadas ({total} células)");
    }
}
