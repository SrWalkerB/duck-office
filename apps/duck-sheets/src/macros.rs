//! Macros VBA: o interpretador (`duck-vba`) sobre a planilha aberta. As
//! mudanças de células e de dinâmicas são juntadas e aplicadas quando a macro
//! lê algo que depende delas (ou no fim), e a macro inteira é um passo de
//! desfazer. A macro só enxerga a planilha: não abre arquivos nem programas.

use std::collections::{BTreeMap, HashMap};

use duck_core::pivot::{Axis, PivotTable, SortType};
use duck_core::{CellChange, CellRange, CellValue, Command, Value};
use duck_vba::excel::{Backend, Excel, Field, Orient, Table};
use duck_vba::{Interp, VbErr};

use crate::app::SheetsApp;

/// Células que uma macro pode trocar de uma vez.
const MAX_CELLS: u64 = 2_000_000;

type R<T> = Result<T, String>;

struct Ctx<'a> {
    app: &'a mut SheetsApp,
    /// Células escritas e ainda não aplicadas, por aba.
    cells: HashMap<usize, BTreeMap<(u32, u32), CellChange>>,
    /// Dinâmicas mudadas e ainda não recalculadas.
    pivots: Vec<(usize, usize, PivotTable)>,
    /// Linhas acrescentadas a tabelas e ainda não aplicadas: (aba, tabela) → n.
    grown: BTreeMap<(usize, usize), u32>,
}

impl SheetsApp {
    /// Executa a macro de um objeto (`[0]!Nome`, `Arquivo.xlsm!Módulo.Nome`).
    pub fn run_macro(&mut self, name: &str) {
        let name = name.rsplit(['!', '.']).next().unwrap_or(name).to_string();
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.finished.is_none() || doc.sheets.iter().any(|s| s.data.is_none()) {
            self.message = "Aguarde terminar de carregar".into();
            return;
        }
        if doc.macros.is_none() {
            let bin = duck_xlsx::Package::open(&doc.path).ok().and_then(|p| p.read("xl/vbaProject.bin").ok());
            doc.macros = Some(bin.map(|b| duck_vba::modules(&b)).unwrap_or_default());
        }
        let modules = doc.macros.clone().unwrap_or_default();
        if modules.is_empty() {
            self.message = "O arquivo não tem macros".into();
            return;
        }
        // Uma pergunta por arquivo, como o aviso de segurança do Excel.
        if !doc.macros_ok {
            if !self.ask(
                "Este objeto executa uma macro gravada no arquivo. Ela só mexe nesta planilha \
                 (o duck-sheets não dá às macros acesso a arquivos, à internet ou a outros programas). \
                 Executar as macros deste arquivo?",
            ) {
                return;
            }
            if let Some(d) = self.doc.as_mut() {
                d.macros_ok = true;
            }
        }
        let units = modules.iter().map(|m| duck_vba::parse::parse_module(&m.name, &m.source)).collect();
        let before = self.doc.as_ref().map_or(0, |d| d.history.undo.len());
        let mut ctx = Ctx { app: self, cells: HashMap::new(), pivots: Vec::new(), grown: BTreeMap::new() };
        let mut r = {
            let mut host = Excel::new(&mut ctx);
            Interp::new(&mut host, units).run(&name)
        };
        // O que ficou pendente entra mesmo se a macro parou no meio.
        if let Err(e) = ctx.flush()
            && r.is_ok()
        {
            r = Err(VbErr::new(1004, e));
        }
        self.merge_undo(before);
        match r {
            Ok(()) => self.message = "Macro executada".into(),
            Err(e) if e.number == 0 => {}
            Err(e) => {
                self.message = format!("Erro na macro: {}", e.text);
                self.macro_message(&format!("Erro em tempo de execução '{}':\n\n{}", e.number, e.text), 16);
            }
        }
    }

    /// Junta os passos feitos depois de `from` num só.
    fn merge_undo(&mut self, from: usize) {
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.history.undo.len() > from + 1 {
            let steps = doc.history.undo.split_off(from);
            // Do mais novo para o mais velho.
            let merged = steps.into_iter().rev().flatten().collect();
            doc.history.undo.push(merged);
        }
    }

    /// Caixa de mensagem de uma macro (nos testes, vai para o terminal).
    fn macro_message(&mut self, text: &str, buttons: i32) -> i32 {
        let scripted = self.opts.save_to.is_some() || self.opts.screenshot.is_some() || self.opts.bench;
        // 3 e 4 = sim/não; 1 = OK/cancelar.
        let kind = buttons & 7;
        if scripted {
            // Só os textos do próprio duck-sheets: o resto pode ter dados.
            let own = text.find("O duck-sheets").map_or("", |i| &text[i..]);
            eprintln!("--macro: mensagem ({} caracteres) {own}", text.chars().count());
            return if kind >= 3 { 6 } else { 1 };
        }
        match kind {
            0 => {
                match buttons & 0x70 {
                    16 => duck_platform::message_box("Macro", text),
                    _ => duck_platform::info_box("Macro", text),
                }
                1
            }
            k => {
                let yes = self.window.as_ref().is_none_or(|w| w.confirm("Macro", text));
                match (k, yes) {
                    (1, true) => 1,
                    (1, false) => 2,
                    (_, true) => 6,
                    (_, false) => 7,
                }
            }
        }
    }
}

impl Ctx<'_> {
    fn doc(&self) -> Option<&crate::app::Doc> {
        self.app.doc.as_ref()
    }

    fn sheet(&self, s: usize) -> R<&duck_core::Sheet> {
        self.doc()
            .and_then(|d| d.sheets.get(s))
            .and_then(|s| s.data.as_deref())
            .ok_or_else(|| "Aba não encontrada".to_string())
    }

    fn flush_cells(&mut self) -> R<()> {
        if self.cells.is_empty() && self.grown.is_empty() {
            return Ok(());
        }
        let mut step: crate::edit::Step = Vec::new();
        // Tabelas que cresceram: um layout por aba.
        let mut layouts: BTreeMap<usize, duck_core::Layout> = BTreeMap::new();
        for ((s, t), n) in std::mem::take(&mut self.grown) {
            if let std::collections::btree_map::Entry::Vacant(e) = layouts.entry(s) {
                e.insert(self.sheet(s)?.layout.clone());
            }
            let l = layouts.get_mut(&s).expect("layout");
            l.tables[t].range.last_row += n;
            if let Some(f) = &mut l.tables[t].filter {
                f.range.last_row += n;
            }
        }
        step.extend(layouts.into_iter().map(|(s, l)| (s, Command::Layout(Box::new(l)))));
        step.extend(
            std::mem::take(&mut self.cells)
                .into_iter()
                .map(|(s, m)| (s, Command::SetCells(m.into_values().collect()))),
        );
        if self.app.run_step(step) { Ok(()) } else { Err("A planilha não pode ser editada agora".into()) }
    }

    /// A dinâmica para mudar (a pendente, se já foi mudada).
    fn pivot_mut(&mut self, s: usize, p: usize) -> R<&mut PivotTable> {
        if let Some(i) = self.pivots.iter().position(|x| (x.0, x.1) == (s, p)) {
            return Ok(&mut self.pivots[i].2);
        }
        let t = self.sheet(s)?.layout.pivots.get(p).cloned().ok_or("Tabela dinâmica não encontrada")?;
        self.pivots.push((s, p, t));
        Ok(&mut self.pivots.last_mut().expect("dinâmica").2)
    }

    fn pivot(&self, s: usize, p: usize) -> Option<&PivotTable> {
        self.pivots
            .iter()
            .find(|x| (x.0, x.1) == (s, p))
            .map(|x| &x.2)
            .or_else(|| self.sheet(s).ok()?.layout.pivots.get(p))
    }

    fn cache_of(&self, s: usize, p: usize) -> Option<&duck_core::pivot::PivotCache> {
        let id = self.pivot(s, p)?.cache_id;
        let d = self.doc()?;
        d.pivots
            .loaded
            .get(&id)
            .map(|c| &c.0)
            .or_else(|| d.calc.extras.pivot_caches.iter().find(|c| c.id == id))
    }
}

impl Backend for Ctx<'_> {
    fn workbook_name(&self) -> String {
        self.doc().map(|d| self.app.file_label(&d.path)).unwrap_or_default()
    }

    fn sheets(&self) -> Vec<String> {
        self.doc().map(|d| d.sheets.iter().map(|s| s.info.name.clone()).collect()).unwrap_or_default()
    }

    fn active_sheet(&self) -> usize {
        self.doc().map_or(0, |d| d.active)
    }

    fn activate(&mut self, sheet: usize) {
        self.app.switch_sheet(sheet);
    }

    fn value(&mut self, s: usize, row: u32, col: u32) -> CellValue {
        if let Some(c) = self.cells.get(&s).and_then(|m| m.get(&(row, col))) {
            return c.value.clone();
        }
        let Some(doc) = self.doc() else { return CellValue::Empty };
        let Ok(sh) = self.sheet(s) else { return CellValue::Empty };
        match sh.cell(row, col, &doc.strings).value {
            Value::Empty => CellValue::Empty,
            Value::Number(n) => CellValue::Number(n),
            Value::Text(t) => CellValue::Text(t.to_string()),
            Value::Bool(b) => CellValue::Bool(b),
            Value::Error(e) => CellValue::Error(e),
        }
    }

    fn set_values(&mut self, s: usize, r: CellRange, vals: Vec<CellValue>) -> R<()> {
        if vals.len() as u64 > MAX_CELLS {
            return Err("Intervalo grande demais para uma macro".into());
        }
        let mut m = self.cells.remove(&s).unwrap_or_default();
        let sh = self.sheet(s).ok().zip(self.doc());
        let w = (r.last_col - r.first_col + 1) as usize;
        for (i, value) in vals.into_iter().enumerate() {
            let (row, col) = (r.first_row + (i / w) as u32, r.first_col + (i % w) as u32);
            // Limpar o que já está vazio não é mudança (um `ClearContents` numa
            // faixa grande criaria centenas de milhares).
            if value == CellValue::Empty
                && !m.contains_key(&(row, col))
                && sh.is_some_and(|(sh, d)| {
                    sh.formula_id(row, col) == 0
                        && matches!(sh.cell(row, col, &d.strings).value, Value::Empty)
                })
            {
                continue;
            }
            let (value, formula) = match value {
                CellValue::Text(t) if t.starts_with('=') && t.len() > 1 => {
                    (CellValue::Empty, Some(t[1..].to_string()))
                }
                v => (v, None),
            };
            m.insert((row, col), CellChange { row, col, value, formula, style: None });
        }
        if !m.is_empty() {
            self.cells.insert(s, m);
        }
        Ok(())
    }

    fn used_range(&self, s: usize) -> Option<CellRange> {
        let sh = self.sheet(s).ok()?;
        let pend = self.cells.get(&s);
        let (mut rows, mut cols) = (sh.rows(), sh.cols());
        for (r, c) in pend.into_iter().flat_map(|m| m.keys()) {
            rows = rows.max(r + 1);
            cols = cols.max(c + 1);
        }
        (rows > 0 && cols > 0).then(|| CellRange {
            first_row: 0,
            first_col: 0,
            last_row: rows - 1,
            last_col: cols - 1,
        })
    }

    fn select(&mut self, s: usize, r: CellRange) {
        if self.active_sheet() != s {
            return;
        }
        self.app.go_to(r.first_row, r.first_col);
        if let Some(v) = self.app.doc.as_mut().and_then(|d| d.view_mut()) {
            v.anchor = (r.last_row.min(v.rows.len() - 1), r.last_col.min(v.cols.len() - 1));
        }
    }

    /// Classificar. Dentro de uma dinâmica, ordena o campo de linha pelo valor
    /// da coluna, como no Excel.
    fn sort(&mut self, s: usize, r: CellRange, key: u32, desc: bool) -> R<()> {
        self.flush()?;
        let sheet = self.sheet(s)?;
        let inside = sheet.layout.pivots.iter().position(|t| {
            t.range.contains(r.first_row, r.first_col) && t.range.contains(r.last_row, r.last_col)
        });
        if let Some(p) = inside {
            let t = &sheet.layout.pivots[p];
            let Some(&field) = t.row_fields.iter().rev().find(|f| **f >= 0) else { return Ok(()) };
            let dc = t.range.first_col + t.first_data_col;
            let ci = key.checked_sub(dc).ok_or("A chave não é uma coluna de valores")? as usize;
            let item = t.col_items.get(ci).cloned().unwrap_or_default();
            // Coluna de um item: o total é o dessa coluna.
            let mut refs = Vec::new();
            if item.t.is_none() && t.col_fields.iter().any(|&f| f >= 0) {
                let mut path: Vec<u32> = Vec::new();
                for it in &t.col_items[..=ci] {
                    path.truncate(it.r as usize);
                    path.extend(&it.x);
                }
                refs.extend(
                    t.col_fields.iter().zip(&path).filter(|(f, _)| **f >= 0).map(|(f, x)| (*f as u32, *x)),
                );
            }
            let t = self.pivot_mut(s, p)?;
            let f = &mut t.fields[field as usize];
            f.sort = if desc { SortType::Descending } else { SortType::Ascending };
            f.sort_by_data = Some(item.i);
            f.sort_refs = refs;
            return self.flush();
        }
        let (doc, sheet) = (self.doc().ok_or("sem planilha")?, self.sheet(s)?);
        let last = r.last_row.min(sheet.rows().max(1) - 1);
        let n = (last + 1).saturating_sub(r.first_row);
        let rank = |v: &Value| match v {
            Value::Number(_) => 0,
            Value::Text(_) => 1,
            Value::Bool(_) => 2,
            Value::Error(_) => 3,
            Value::Empty => 4,
        };
        let mut perm: Vec<u32> = (0..n).collect();
        perm.sort_by(|&a, &b| {
            let va = sheet.cell(r.first_row + a, key, &doc.strings).value;
            let vb = sheet.cell(r.first_row + b, key, &doc.strings).value;
            // Vazias sempre no fim.
            match (matches!(va, Value::Empty), matches!(vb, Value::Empty)) {
                (true, true) => return std::cmp::Ordering::Equal,
                (true, false) => return std::cmp::Ordering::Greater,
                (false, true) => return std::cmp::Ordering::Less,
                _ => {}
            }
            let o = match (&va, &vb) {
                (Value::Number(x), Value::Number(y)) => x.total_cmp(y),
                (Value::Text(x), Value::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
                (x, y) => rank(x).cmp(&rank(y)),
            };
            if desc { o.reverse() } else { o }
        });
        let cmd = Command::SortRows { r0: r.first_row, c0: r.first_col, c1: r.last_col, perm };
        if self.app.run_step(vec![(s, cmd)]) { Ok(()) } else { Err("Não foi possível classificar".into()) }
    }

    fn freeze_values(&mut self, s: usize, r: CellRange) -> R<()> {
        self.flush()?;
        let (doc, sheet) = (self.doc().ok_or("sem planilha")?, self.sheet(s)?);
        let mut list = Vec::new();
        for row in r.first_row..=r.last_row.min(sheet.rows().max(1) - 1) {
            for col in r.first_col..=r.last_col.min(sheet.cols().max(1) - 1) {
                if sheet.formula(row, col).is_none() {
                    continue;
                }
                let value = match sheet.cell(row, col, &doc.strings).value {
                    Value::Empty => CellValue::Empty,
                    Value::Number(n) => CellValue::Number(n),
                    Value::Text(t) => CellValue::Text(t.to_string()),
                    Value::Bool(b) => CellValue::Bool(b),
                    Value::Error(e) => CellValue::Error(e),
                };
                list.push(CellChange { row, col, value, formula: None, style: None });
                if list.len() as u64 > MAX_CELLS {
                    return Err("Fórmulas demais para trocar por valores numa macro".into());
                }
            }
        }
        if !list.is_empty() {
            self.app.run_step(vec![(s, Command::SetCells(list))]);
        }
        Ok(())
    }

    fn set_hidden(&mut self, s: usize, rows: bool, first: u32, last: u32, hidden: bool) -> R<()> {
        let sheet = self.sheet(s)?;
        let list: Vec<(u32, bool)> = if hidden {
            (first..=last.min(first + 1_048_575)).map(|i| (i, true)).collect()
        } else if rows {
            sheet.layout.rows_in(first, last).iter().filter(|r| r.hidden).map(|r| (r.row, false)).collect()
        } else {
            (first..=last.min(16_383)).filter(|&c| sheet.col_hidden(c)).map(|c| (c, false)).collect()
        };
        if list.is_empty() {
            return Ok(());
        }
        let cmd = if rows { Command::RowsHidden(list) } else { Command::ColsHidden(list) };
        self.app.run_step(vec![(s, cmd)]);
        Ok(())
    }

    fn delete_sheet(&mut self, _: usize) -> R<()> {
        Err("O duck-sheets ainda não exclui abas por macro".into())
    }

    fn tables(&self, s: usize) -> Vec<Table> {
        let Ok(sh) = self.sheet(s) else { return Vec::new() };
        sh.layout
            .tables
            .iter()
            .enumerate()
            .map(|(i, t)| Table {
                name: t.name.clone(),
                range: CellRange {
                    last_row: t.range.last_row + self.grown.get(&(s, i)).copied().unwrap_or(0),
                    ..t.range
                },
                header_rows: t.header_rows,
                totals_rows: t.totals_rows,
                columns: t.columns.clone(),
            })
            .collect()
    }

    /// Uma linha no fim da tabela: as colunas calculadas repetem a fórmula da
    /// linha de cima (ajustada) e todas copiam o estilo dela.
    fn add_table_row(&mut self, s: usize, name: &str) -> R<u32> {
        // A tabela cresce e as células entram quando a macro precisar delas.
        let sheet = self.sheet(s)?;
        let ti = sheet
            .layout
            .tables
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(name))
            .ok_or("Tabela não encontrada")?;
        let t = &sheet.layout.tables[ti];
        if t.totals_rows > 0 {
            return Err("Tabela com linha de totais: acrescentar linhas por macro ainda não dá".into());
        }
        let grown = self.grown.get(&(s, ti)).copied().unwrap_or(0);
        let (r, above) = (t.range, t.range.last_row + grown);
        let row = above + 1;
        if (r.first_col..=r.last_col).any(|c| sheet.has_value(row, c)) {
            return Err("Há dados logo abaixo da tabela".into());
        }
        let mut new = Vec::new();
        for col in r.first_col..=r.last_col {
            let pend = self.cells.get(&s).and_then(|m| m.get(&(above, col)));
            let formula = match pend {
                Some(p) => p.formula.clone(),
                None => duck_calc::cell_formula(sheet, above, col).map(|f| f.into_owned()),
            };
            let style = match pend.and_then(|p| p.style) {
                Some(st) => Some(st),
                None => Some(sheet.cell(above, col, &self.doc().ok_or("sem planilha")?.strings).style),
            };
            let formula = formula.map(|f| duck_calc::shift(&f, 1, 0));
            new.push(CellChange { row, col, value: CellValue::Empty, formula, style });
        }
        *self.grown.entry((s, ti)).or_default() += 1;
        let m = self.cells.entry(s).or_default();
        for c in new {
            m.insert((c.row, c.col), c);
        }
        Ok(row)
    }

    fn pivots(&self, s: usize) -> Vec<String> {
        self.sheet(s).map(|sh| sh.layout.pivots.iter().map(|p| p.name.clone()).collect()).unwrap_or_default()
    }

    fn pivot_fields(&mut self, s: usize, p: usize) -> Vec<Field> {
        if self.pivot_is_cube(s, p) {
            return self.app.cube_fields(s, p);
        }
        let Some(t) = self.pivot(s, p) else { return Vec::new() };
        let Some(c) = self.cache_of(s, p) else { return Vec::new() };
        t.fields
            .iter()
            .zip(&c.fields)
            .enumerate()
            .map(|(id, (f, cf))| Field {
                id,
                name: f.name.clone().unwrap_or_else(|| cf.name.clone()),
                caption: cf.name.clone(),
            })
            .collect()
    }

    fn pivot_axis(&mut self, s: usize, p: usize, axis: Orient) -> Vec<usize> {
        let cube = self.pivot_is_cube(s, p);
        let Some(t) = self.pivot(s, p) else { return Vec::new() };
        let ids: Vec<usize> = match axis {
            Orient::Row => t.row_fields.iter().filter(|f| **f >= 0).map(|f| *f as usize).collect(),
            Orient::Col => t.col_fields.iter().filter(|f| **f >= 0).map(|f| *f as usize).collect(),
            Orient::Page => t.page_fields.iter().map(|f| f.0 as usize).collect(),
            _ => Vec::new(),
        };
        if !cube {
            return ids;
        }
        match self.pivot(s, p).cloned() {
            Some(t) => self.app.cube_ids(&t, &ids),
            None => Vec::new(),
        }
    }

    fn pivot_is_cube(&self, s: usize, p: usize) -> bool {
        let Some(id) = self.pivot(s, p).map(|t| t.cache_id) else { return false };
        self.doc()
            .and_then(|d| d.calc.extras.pivot_caches.iter().find(|c| c.id == id))
            .is_some_and(|c| c.external)
    }

    fn set_orientation(&mut self, s: usize, p: usize, field: usize, o: Orient) -> R<()> {
        let axis = match o {
            Orient::Hidden => Axis::None,
            Orient::Row => Axis::Row,
            Orient::Col => Axis::Col,
            Orient::Page => Axis::Page,
            Orient::Data => return Err("Mudar campos de valores por macro ainda não dá".into()),
        };
        if self.pivot_is_cube(s, p) {
            let mut t = self.pivot(s, p).cloned().ok_or("Tabela dinâmica não encontrada")?;
            self.app.cube_set_axis(&mut t, field, axis)?;
            *self.pivot_mut(s, p)? = t;
            return Ok(());
        }
        let id = self.pivot(s, p).map(|t| t.cache_id).ok_or("Tabela dinâmica não encontrada")?;
        self.app.ensure_pivot_cache(id)?;
        let cache = self.cache_of(s, p).cloned().ok_or("Cache da tabela dinâmica não encontrado")?;
        if axis != Axis::None && cache.fields.get(field).is_none_or(|c| c.items.is_empty()) {
            return Err("O campo não tem itens no cache da tabela dinâmica".into());
        }
        let t = self.pivot_mut(s, p)?;
        duck_calc::pivot::set_axis(t, &cache, field, axis);
        Ok(())
    }

    fn set_position(&mut self, s: usize, p: usize, field: usize, pos: usize) -> R<()> {
        let field = if self.pivot_is_cube(s, p) {
            let t = self.pivot(s, p).cloned().ok_or("Tabela dinâmica não encontrada")?;
            self.app.cube_field_of(&t, field).ok_or("O campo não está na tabela")?
        } else {
            field
        };
        let t = self.pivot_mut(s, p)?;
        for list in [&mut t.row_fields, &mut t.col_fields] {
            if let Some(i) = list.iter().position(|f| *f == field as i32) {
                let f = list.remove(i);
                // "Valores" não conta nas posições dos campos.
                let at =
                    list.iter().enumerate().filter(|(_, f)| **f >= 0).nth(pos).map_or(list.len(), |(k, _)| k);
                list.insert(at, f);
                return Ok(());
            }
        }
        Err("O campo não está na tabela".into())
    }

    fn set_show_detail(&mut self, s: usize, p: usize, field: usize, show: bool) -> R<()> {
        let field = if self.pivot_is_cube(s, p) {
            let t = self.pivot(s, p).cloned().ok_or("Tabela dinâmica não encontrada")?;
            self.app.cube_field_of(&t, field).ok_or("O campo não está na tabela")?
        } else {
            field
        };
        let t = self.pivot_mut(s, p)?;
        if let Some(f) = t.fields.get_mut(field) {
            f.items.iter_mut().filter(|i| i.t.is_none()).for_each(|i| i.collapsed = !show);
        }
        Ok(())
    }

    fn refresh_pivot(&mut self, s: usize, p: usize) -> R<()> {
        self.flush()?;
        let t = self.pivot(s, p).cloned().ok_or("Tabela dinâmica não encontrada")?;
        self.pivots.push((s, p, t));
        self.flush()
    }

    fn pivot_ranges(&mut self, s: usize, p: usize) -> R<(CellRange, Option<CellRange>, CellRange)> {
        let t = self.pivot(s, p).ok_or("Tabela dinâmica não encontrada")?;
        let r = t.range;
        let (dr, dc) = (r.first_row + t.first_data_row, r.first_col + t.first_data_col);
        let body = (!t.data_fields.is_empty() && dr <= r.last_row && dc <= r.last_col).then_some(CellRange {
            first_row: dr,
            first_col: dc,
            ..r
        });
        // Rótulos de linha, com o cabeçalho.
        let rows = CellRange {
            first_row: dr.saturating_sub(1).max(r.first_row),
            last_col: (dc.max(r.first_col + 1) - 1).min(r.last_col),
            ..r
        };
        Ok((r, body, rows))
    }

    fn flush(&mut self) -> R<()> {
        self.flush_cells()?;
        if self.pivots.is_empty() {
            return Ok(());
        }
        let list = std::mem::take(&mut self.pivots);
        self.app.apply_pivots(list).map(|_| ())
    }

    fn calculate(&mut self) {
        let _ = self.flush();
        if let Some(doc) = &mut self.app.doc {
            doc.calc.request(crate::calc::Pending::All);
        }
    }

    fn msgbox(&mut self, text: &str, buttons: i32, _: &str) -> i32 {
        self.app.macro_message(text, buttons)
    }

    fn status(&mut self, text: Option<&str>) {
        if let Some(t) = text {
            self.app.message = t.to_string();
        }
    }
}
