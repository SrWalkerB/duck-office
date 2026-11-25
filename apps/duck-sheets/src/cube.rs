//! Tabelas dinâmicas do Modelo de Dados no app: o modelo é montado das
//! tabelas das abas na primeira vez que algo precisa dele (não na abertura) e
//! guardado até os dados dessas abas mudarem.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use duck_core::cube::{CubeCache, ModelDef};
use duck_core::drawing::ObjectKind;
use duck_core::pivot::{Axis, PivotField, PivotTable};
use duck_core::{Command, Sheet};
use duck_dax::olap::{self, Computed, Selection, SheetSource};
use duck_vba::excel::Field;

use crate::app::{Doc, SheetsApp};
use crate::pivot::{changes, render};

/// Modelo e definições já lidas.
#[derive(Default)]
pub struct CubeState {
    def: Option<Arc<ModelDef>>,
    caches: HashMap<u32, Arc<CubeCache>>,
    model: Option<Ready>,
}

struct Ready {
    model: Arc<duck_dax::Model>,
    /// Versão dos dados das abas de origem quando o modelo foi montado.
    version: Vec<(usize, u64)>,
}

type R<T> = Result<T, String>;

fn sheets_of(doc: &Doc) -> R<Vec<Arc<Sheet>>> {
    doc.sheets
        .iter()
        .map(|s| s.data.clone().ok_or_else(|| "Aguarde terminar de carregar".to_string()))
        .collect()
}

/// Segmentações do modelo ligadas a uma dinâmica, com a seleção delas.
fn selections(sheets: &[Arc<Sheet>], tab: u32, t: &PivotTable) -> Vec<Selection> {
    let mut out: Vec<Selection> = Vec::new();
    for o in sheets.iter().flat_map(|s| &s.layout.objects) {
        let ObjectKind::Slicer(s) = &o.kind else { continue };
        let Some(link) = s.pivot.as_ref().filter(|l| l.is_cube()) else { continue };
        if !link.tables.iter().any(|x| x.0 == tab && x.1 == t.name) || out.iter().any(|x| x.0 == link.source)
        {
            continue;
        }
        if s.items.iter().all(|i| i.1) {
            continue;
        }
        let chosen = link.members.iter().zip(&s.items).filter(|x| x.1.1).map(|x| x.0.clone());
        out.push((link.source.clone(), chosen.collect()));
    }
    out
}

impl SheetsApp {
    fn cube_cache(&mut self, id: u32) -> R<Arc<CubeCache>> {
        let doc = self.doc.as_mut().ok_or("sem planilha")?;
        if let Some(c) = doc.pivots.cube.caches.get(&id) {
            return Ok(c.clone());
        }
        let part = doc
            .calc
            .extras
            .pivot_caches
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.part.clone())
            .ok_or("Cache da tabela dinâmica não encontrado")?;
        let c = doc
            .book()
            .ok()
            .and_then(|b| b.read_cube_cache(&part))
            .ok_or("A tabela dinâmica não é do Modelo de Dados")?;
        let c = Arc::new(c);
        doc.pivots.cube.caches.insert(id, c.clone());
        Ok(c)
    }

    /// O modelo pronto para contas, com os dados de agora. Chame
    /// [`duck_dax::Model::detach`] no fim: a origem segura as abas.
    fn cube_model(&mut self) -> R<Arc<duck_dax::Model>> {
        let doc = self.doc.as_mut().ok_or("sem planilha")?;
        let sheets = sheets_of(doc)?;
        if doc.pivots.cube.def.is_none() {
            let def = doc
                .book()
                .ok()
                .and_then(|b| b.read_model())
                .ok_or("O arquivo não tem um Modelo de Dados que o duck-sheets leia")?;
            doc.pivots.cube.def = Some(Arc::new(def));
        }
        let def = doc.pivots.cube.def.clone().expect("modelo");
        let source = SheetSource::new(&def, sheets, doc.strings.clone());
        let now = duck_calc::eval::now_serial();
        let version: Vec<(usize, u64)> =
            source.sheets_used().into_iter().map(|s| (s, doc.calc.version(s))).collect();
        if let Some(r) = &doc.pivots.cube.model
            && r.version == version
            && r.model.today == now.floor()
        {
            r.model.attach(Box::new(source));
            return Ok(r.model.clone());
        }
        let model = Arc::new(olap::build(&def, Box::new(source), now));
        doc.pivots.cube.model = Some(Ready { model: model.clone(), version });
        Ok(model)
    }

    /// Recalcula dinâmicas do modelo (aba, índice, definição).
    fn cube_compute(
        &mut self,
        list: &[(usize, usize, PivotTable)],
        slicers: &[Selection],
    ) -> R<Vec<Computed>> {
        let model = self.cube_model()?;
        let mut r = Ok(Vec::with_capacity(list.len()));
        for (si, _, t) in list {
            let one = self.cube_cache(t.cache_id).and_then(|cube| {
                let doc = self.doc.as_ref().ok_or("sem planilha")?;
                let sheets = sheets_of(doc)?;
                let cache = doc.calc.extras.pivot_caches.iter().find(|c| c.id == t.cache_id);
                let cache = cache.ok_or("Cache da tabela dinâmica não encontrado")?;
                let mut sel = selections(&sheets, sheets[*si].tab_id, t);
                for s in slicers {
                    sel.retain(|x| x.0 != s.0);
                    sel.push(s.clone());
                }
                olap::compute(&model, &cube, cache, t, &sel)
            });
            match (one, &mut r) {
                (Ok(c), Ok(v)) => v.push(c),
                (Err(e), _) => {
                    r = Err(format!("Modelo de Dados: {e}"));
                    break;
                }
                _ => {}
            }
        }
        model.detach();
        r
    }

    /// Passo que aplica dinâmicas recalculadas (células e layout), com os
    /// objetos ajustados por `objects` em todas as abas.
    pub(crate) fn cube_apply(
        &mut self,
        list: &[(usize, usize, PivotTable)],
        slicers: &[Selection],
        objects: &mut dyn FnMut(&mut ObjectKind),
    ) -> R<usize> {
        let done = self.cube_compute(list, slicers)?;
        let doc = self.doc.as_ref().ok_or("sem planilha")?;
        let mut step: crate::edit::Step = Vec::new();
        let mut total = 0;
        for (si, slot) in doc.sheets.iter().enumerate() {
            let Some(sh) = &slot.data else { continue };
            let mut layout = sh.layout.clone();
            let mut cells = Vec::new();
            for ((_, pi, _), c) in list.iter().zip(&done).filter(|x| x.0.0 == si) {
                let mut nt = c.table.clone();
                nt.first_data_col = duck_calc::pivot::label_cols(&nt);
                let (out, range) = render(&nt, &c.cache, &c.result);
                let old = sh.layout.pivots.get(*pi).map_or(range, |o| o.range);
                cells.extend(changes(sh, &doc.strings, old, out, range));
                nt.range = range;
                nt.row_items = c.result.row_items.clone();
                nt.col_items = c.result.col_items.clone();
                layout.pivots[*pi] = nt;
            }
            for o in &mut layout.objects {
                objects(&mut o.kind);
            }
            if layout == sh.layout && cells.is_empty() {
                continue;
            }
            total += cells.len();
            let mut cmds = Vec::new();
            if !cells.is_empty() {
                cmds.push(Command::SetCells(cells));
            }
            if layout != sh.layout {
                cmds.push(Command::Layout(Box::new(layout)));
            }
            step.push((si, Command::Group(cmds)));
        }
        if !step.is_empty() && !self.run_step(step) {
            return Err("A planilha não pode ser editada agora".into());
        }
        Ok(total)
    }

    /// Clique numa segmentação do Modelo de Dados: `chosen` por item.
    pub fn cube_slicer(&mut self, obj: usize, chosen: &[bool]) -> R<usize> {
        let doc = self.doc.as_ref().ok_or("sem planilha")?;
        let (sheet, _) = doc.current().ok_or("sem planilha")?;
        let Some(ObjectKind::Slicer(s)) = sheet.layout.objects.get(obj).map(|o| &o.kind) else {
            return Err("segmentação".into());
        };
        let link = s.pivot.clone().ok_or("segmentação")?;
        let all = chosen.iter().all(|c| *c);
        let members: Vec<String> =
            link.members.iter().zip(chosen).filter(|x| *x.1).map(|x| x.0.clone()).collect();
        let sel: Selection = (link.source.clone(), if all { Vec::new() } else { members.clone() });
        // Dinâmicas ligadas, com o filtro novo.
        let mut list = Vec::new();
        for (si, slot) in doc.sheets.iter().enumerate() {
            let Some(sh) = &slot.data else { continue };
            for (pi, t) in sh.layout.pivots.iter().enumerate() {
                if t.cube.is_some() && link.tables.iter().any(|x| x.0 == sh.tab_id && x.1 == t.name) {
                    list.push((si, pi, t.clone()));
                }
            }
        }
        for (_, _, t) in &mut list {
            let cache = self.cube_cache(t.cache_id)?;
            let h = cache.hierarchies.iter().position(|h| h.unique == link.source);
            if let (Some(h), Some(c)) = (h, &mut t.cube) {
                c.choose(h as u32, &link.source, (!all).then(|| members.clone()));
            }
        }
        let with_data = self.cube_slicer_data(&link.source, &link.members, &sel)?;
        self.cube_apply(&list, std::slice::from_ref(&sel), &mut |kind| {
            let ObjectKind::Slicer(x) = kind else { return };
            let Some(p) = &x.pivot else { return };
            if p.part == link.part {
                for (it, c) in x.items.iter_mut().zip(chosen) {
                    it.1 = *c;
                }
            } else if let Some(data) = with_data.get(&p.source) {
                for (m, it) in p.members.iter().zip(&mut x.items) {
                    it.2 = data.contains(m);
                }
            }
        })
    }

    /// Membros com dados nas outras segmentações do modelo, depois da escolha
    /// `sel` (as da mesma tabela do modelo se filtram).
    fn cube_slicer_data(
        &mut self,
        source: &str,
        _members: &[String],
        sel: &Selection,
    ) -> R<HashMap<String, BTreeSet<String>>> {
        let model = self.cube_model()?;
        let doc = self.doc.as_ref().ok_or("sem planilha")?;
        let mut others: Vec<(String, Vec<String>)> = vec![sel.clone()];
        let mut sources: BTreeSet<String> = BTreeSet::new();
        for s in doc.sheets.iter().filter_map(|s| s.data.as_ref()) {
            for o in &s.layout.objects {
                let ObjectKind::Slicer(x) = &o.kind else { continue };
                let Some(p) = x.pivot.as_ref().filter(|p| p.is_cube()) else { continue };
                if p.source == source || !sources.insert(p.source.clone()) {
                    continue;
                }
                if !x.items.iter().all(|i| i.1) {
                    let chosen = p.members.iter().zip(&x.items).filter(|x| x.1.1).map(|x| x.0.clone());
                    others.push((p.source.clone(), chosen.collect()));
                }
            }
        }
        let filters: Vec<((usize, usize), Vec<String>)> = others
            .iter()
            .filter(|o| !o.1.is_empty())
            .filter_map(|(h, list)| {
                let (t, c) = duck_core::cube::split_unique(h)?;
                let col = model.find(&t, &c)?;
                Some((col, list.iter().filter_map(|u| duck_dax::cube::key_of_unique(u)).collect()))
            })
            .collect();
        let mut out = HashMap::new();
        for h in sources {
            if let Ok(items) = olap::slicer_items(&model, &h, &filters) {
                out.insert(h, items.into_iter().filter(|i| i.2).map(|i| i.0).collect());
            }
        }
        model.detach();
        Ok(out)
    }

    // --- o que as macros veem ------------------------------------------------

    /// Campos (hierarquias e medidas) de uma dinâmica do Modelo de Dados.
    pub fn cube_fields(&mut self, s: usize, p: usize) -> Vec<Field> {
        let id = self
            .doc
            .as_ref()
            .and_then(|d| d.sheets.get(s)?.data.as_ref()?.layout.pivots.get(p).map(|t| t.cache_id));
        let Some(cache) = id.and_then(|id| self.cube_cache(id).ok()) else { return Vec::new() };
        cache
            .hierarchies
            .iter()
            .enumerate()
            .map(|(id, h)| Field { id, name: h.unique.clone(), caption: h.caption.clone() })
            .collect()
    }

    /// Campos da tabela → hierarquias.
    pub fn cube_ids(&mut self, t: &PivotTable, fields: &[usize]) -> Vec<usize> {
        let Ok(cache) = self.cube_cache(t.cache_id) else { return Vec::new() };
        let now = t.cube.as_ref().map(|c| cache.with_new(c));
        let c = now.as_ref().unwrap_or(&cache);
        fields.iter().filter_map(|f| c.fields.get(*f)?.hierarchy.map(|h| h as usize)).collect()
    }

    /// Hierarquia → campo da tabela.
    pub fn cube_field_of(&mut self, t: &PivotTable, h: usize) -> Option<usize> {
        let cache = self.cube_cache(t.cache_id).ok()?;
        match &t.cube {
            Some(c) => cache.with_new(c).field_of(h),
            None => cache.field_of(h),
        }
    }

    pub fn cube_set_axis(&mut self, t: &mut PivotTable, h: usize, axis: Axis) -> R<()> {
        let cache = self.cube_cache(t.cache_id)?;
        if cache.hierarchies.get(h).is_none_or(|x| x.measure) {
            return Err("O campo é uma medida".into());
        }
        let field = match self.cube_field_of(t, h) {
            Some(f) => f,
            None if axis == Axis::None => return Ok(()),
            None => {
                // Campo novo, com as opções que o Excel dá aos campos do modelo.
                let like = t.fields.iter().find(|f| f.axis == Axis::Row);
                t.fields.push(PivotField {
                    show_all: false,
                    compact: like.is_none_or(|f| f.compact),
                    outline: like.is_none_or(|f| f.outline),
                    subtotal_top: false,
                    default_subtotal: false,
                    source_sort: true,
                    ..Default::default()
                });
                let c = t.cube.get_or_insert_with(Default::default);
                c.new_fields.push(h as u32);
                t.fields.len() - 1
            }
        };
        let id = field as i32;
        t.row_fields.retain(|&f| f != id);
        t.col_fields.retain(|&f| f != id);
        match axis {
            Axis::Row => t.row_fields.push(id),
            Axis::Col => t.col_fields.push(id),
            Axis::Page => return Err("Filtro de página por macro ainda não dá".into()),
            _ => {}
        }
        t.fields[field].axis = axis;
        if axis == Axis::None {
            t.fields[field].items.clear();
        }
        Ok(())
    }
}
