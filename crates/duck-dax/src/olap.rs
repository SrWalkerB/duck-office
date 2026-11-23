//! Ponte entre as dinâmicas do arquivo e o modelo: monta o [`Model`] das
//! tabelas das abas e recalcula uma tabela dinâmica inteira (campos, itens,
//! linhas, colunas e valores).

use std::collections::HashMap;
use std::sync::Arc;

use duck_calc::pivot::PivotResult;
use duck_core::cube::{CubeCache, ModelDef, ModelKind, split_unique};
use duck_core::pivot::{AxisItem, FieldItem, PItem, PivotCache, PivotTable, SortType};
use duck_core::{CellRange, CellValue, Sheet, StringTable, Value};

use crate::cube::{Level, Query, Sort, key_of_unique, member_caption, member_key};
use crate::model::{Kind, Model, Source, TableDef, Val};

type Res<T> = Result<T, String>;

/// Aba, área dos dados e coluna da aba de cada coluna do modelo.
type Place = (usize, CellRange, Vec<Option<u32>>);

/// Filtros por coluna: chaves dos membros escolhidos.
pub type Filters = Vec<((usize, usize), Vec<String>)>;

/// Dados das tabelas do modelo nas abas.
pub struct SheetSource {
    sheets: Vec<Arc<Sheet>>,
    strings: Arc<StringTable>,
    /// Por tabela do modelo: (aba, área dos dados, coluna de cada coluna).
    tables: Vec<Option<Place>>,
}

impl SheetSource {
    pub fn new(def: &ModelDef, sheets: Vec<Arc<Sheet>>, strings: Arc<StringTable>) -> Self {
        let tables = def
            .tables
            .iter()
            .map(|t| {
                let src = t.source.as_deref()?.to_lowercase();
                let (si, tb) = sheets.iter().enumerate().find_map(|(i, s)| {
                    s.layout.tables.iter().find(|x| x.name.to_lowercase() == src).map(|x| (i, x))
                })?;
                let body = CellRange {
                    first_row: tb.range.first_row + tb.header_rows,
                    last_row: tb.range.last_row.checked_sub(tb.totals_rows)?,
                    ..tb.range
                };
                let cols = t
                    .columns
                    .iter()
                    .map(|c| {
                        // Nomes que só mudam num espaço são colunas diferentes.
                        let n = c.name.trim().to_lowercase();
                        let same = |f: &dyn Fn(&str) -> bool| tb.columns.iter().position(|x| f(x));
                        let k = same(&|x| x == c.name)
                            .or_else(|| same(&|x| x.to_lowercase() == c.name.to_lowercase()))
                            .or_else(|| same(&|x| x.trim().to_lowercase() == n))?;
                        Some(tb.range.first_col + k as u32)
                    })
                    .collect();
                Some((si, body, cols))
            })
            .collect();
        SheetSource { sheets, strings, tables }
    }

    /// Abas de onde vêm os dados.
    pub fn sheets_used(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.tables.iter().flatten().map(|t| t.0).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Tabelas do modelo sem origem numa aba.
    pub fn missing(&self) -> usize {
        self.tables.iter().filter(|t| t.is_none()).count()
    }
}

impl Source for SheetSource {
    fn rows(&self, t: usize) -> usize {
        match &self.tables[t] {
            Some((_, r, _)) if r.last_row >= r.first_row => (r.last_row - r.first_row + 1) as usize,
            _ => 0,
        }
    }

    fn column(&self, t: usize, c: usize) -> Vec<Val> {
        use rayon::prelude::*;
        let n = self.rows(t);
        let Some((si, r, cols)) = &self.tables[t] else { return Vec::new() };
        let Some(Some(col)) = cols.get(c) else { return vec![Val::Blank; n] };
        let sheet = &self.sheets[*si];
        (0..n as u32)
            .into_par_iter()
            .with_min_len(8192)
            .map(|i| match sheet.cell(r.first_row + i, *col, &self.strings).value {
                Value::Number(x) => Val::Num(x),
                Value::Text(s) => Val::str(s),
                Value::Bool(b) => Val::Bool(b),
                // Erros entram como o texto deles (em colunas de texto).
                Value::Error(e) => Val::str(error_text(e)),
                Value::Empty => Val::Blank,
            })
            .collect()
    }
}

fn error_text(e: duck_core::CellError) -> &'static str {
    use duck_core::CellError as E;
    match e {
        E::Null => "#NULO!",
        E::Div0 => "#DIV/0!",
        E::Value => "#VALOR!",
        E::Ref => "#REF!",
        E::Name => "#NOME?",
        E::Num => "#NÚM!",
        E::NA => "#N/D",
        e => e.as_str(),
    }
}

/// Monta o modelo (as colunas são lidas quando uma conta precisa).
pub fn build(def: &ModelDef, source: Box<dyn Source>, now: f64) -> Model {
    let tables: Vec<TableDef> = def
        .tables
        .iter()
        .map(|t| {
            let cols = t
                .columns
                .iter()
                .map(|c| {
                    let kind = match c.kind {
                        ModelKind::Any => Kind::Any,
                        ModelKind::Text => Kind::Text,
                        ModelKind::Number => Kind::Number,
                        ModelKind::Date => Kind::Date,
                        ModelKind::Bool => Kind::Bool,
                    };
                    (c.name.clone(), kind, c.expr.clone())
                })
                .collect();
            (t.name.clone(), cols)
        })
        .collect();
    // Medidas soltas ou o script inteiro do modelo.
    let measures = def
        .measures
        .iter()
        .flat_map(|(t, n, e)| {
            if n.is_empty() { crate::parse::measures(e) } else { vec![(t.clone(), n.clone(), e.clone())] }
        })
        .collect();
    let mut m = Model::new(tables, &def.rels, measures, source, now);
    for t in &def.tables {
        for c in &t.columns {
            if let Some(by) = &c.sort_by {
                m.sort_by(&t.name, &c.name, by);
            }
        }
    }
    m
}

/// Coluna do modelo de um campo da dinâmica.
fn column(m: &Model, c: &CubeCache, field: usize) -> Res<(usize, usize)> {
    let h = c.hierarchy_of(field).ok_or("campo sem hierarquia")?;
    let (t, col) = split_unique(&h.unique).ok_or("nome de hierarquia")?;
    m.find(&t, &col).ok_or_else(|| "coluna fora do modelo".to_string())
}

/// Nome da medida de um campo de valores.
fn measure(c: &CubeCache, field: usize) -> Res<String> {
    let f = c.fields.get(field).ok_or("campo de valores")?;
    let unique = c.hierarchy_of(field).map_or(f.name.as_str(), |h| h.unique.as_str());
    match split_unique(unique) {
        Some((m, n)) if m == "Measures" => Ok(n),
        _ => Err("campo de valores sem medida".into()),
    }
}

/// Seleção de uma segmentação: hierarquia e nomes únicos escolhidos.
pub type Selection = (String, Vec<String>);

/// Filtros da dinâmica: membros escolhidos de cada hierarquia, filtros de
/// página e segmentações ligadas (que valem por cima).
pub fn filters(m: &Model, c: &CubeCache, t: &PivotTable, slicers: &[Selection]) -> Res<Filters> {
    let Some(cube) = &t.cube else { return Ok(Vec::new()) };
    let col_of = |h: usize| -> Res<(usize, usize)> {
        let h = c.hierarchies.get(h).ok_or("hierarquia")?;
        let (t, col) = split_unique(&h.unique).ok_or("nome de hierarquia")?;
        m.find(&t, &col).ok_or_else(|| "coluna fora do modelo".to_string())
    };
    let mut out = Vec::new();
    for (h, list) in &cube.members {
        // `[All]` na lista = sem filtro.
        let keys: Option<Vec<String>> = list.iter().map(|u| key_of_unique(u)).collect();
        if let Some(keys) = keys {
            out.push((col_of(*h as usize)?, keys));
        }
    }
    for (_, h, name) in &cube.pages {
        if *h < 0 || cube.members.iter().any(|x| x.0 as i32 == *h) {
            continue;
        }
        if let Some(k) = name.as_deref().and_then(key_of_unique) {
            out.push((col_of(*h as usize)?, vec![k]));
        }
    }
    for (h, chosen) in slicers {
        let (t, col) = split_unique(h).ok_or("hierarquia da segmentação")?;
        let Some(col) = m.find(&t, &col) else { continue };
        out.retain(|f| f.0 != col);
        let keys: Option<Vec<String>> = chosen.iter().map(|u| key_of_unique(u)).collect();
        if let Some(keys) = keys.filter(|k| !k.is_empty()) {
            out.push((col, keys));
        }
    }
    Ok(out)
}

fn item(v: &Val, date: bool) -> PItem {
    match v {
        Val::Blank => PItem::Missing,
        Val::Num(n) if date => PItem::Date(*n),
        Val::Num(n) => PItem::Num(*n),
        Val::Bool(b) => PItem::Bool(*b),
        Val::Str(s) => PItem::Str(s.to_string()),
    }
}

fn cell(v: &Val) -> CellValue {
    match v {
        Val::Blank => CellValue::Empty,
        Val::Num(n) => CellValue::Number(*n),
        Val::Bool(b) => CellValue::Bool(*b),
        Val::Str(s) => CellValue::Text(s.to_string()),
    }
}

/// O cache com os membros recalculados desta tabela e os campos novos.
pub fn effective_cache(cache: &PivotCache, t: &PivotTable) -> PivotCache {
    let mut c = cache.clone();
    let Some(p) = &t.cube else { return c };
    while c.fields.len() < t.fields.len() {
        c.fields.push(Default::default());
    }
    for (f, items) in &p.items {
        if let Some(cf) = c.fields.get_mut(*f as usize) {
            cf.items = items.clone();
        }
    }
    c
}

/// Uma dinâmica recalculada: a tabela e o cache com os itens novos, e o
/// resultado no formato do motor de dinâmicas.
pub struct Computed {
    pub table: PivotTable,
    pub cache: PivotCache,
    pub result: PivotResult,
}

pub fn compute(
    m: &Model,
    cube: &CubeCache,
    cache: &PivotCache,
    t: &PivotTable,
    slicers: &[Selection],
) -> Res<Computed> {
    let cube_now = t.cube.as_ref().map(|p| cube.with_new(p));
    let cube = cube_now.as_ref().unwrap_or(cube);
    let cache = &effective_cache(cache, t);
    let mut q = Query { filters: filters(m, cube, t, slicers)?, ..Default::default() };
    for d in &t.data_fields {
        q.data.push(measure(cube, d.fld as usize)?);
    }
    // Chave do membro na posição `pos` de um campo (para as ordenações).
    let key_at = |field: usize, pos: u32| -> Option<String> {
        let x = t.fields.get(field)?.items.get(pos as usize)?.x?;
        Some(item_key(cache.fields.get(field)?.items.get(x as usize)?))
    };
    let level = |f: i32| -> Res<Level> {
        let fi = f as usize;
        let mut l = Level::new(column(m, cube, fi)?);
        let pf = t.fields.get(fi).ok_or("campo da dinâmica")?;
        l.descending = pf.sort == SortType::Descending && pf.sort_by_data.is_none();
        if pf.sort != SortType::Manual
            && let Some(d) = pf.sort_by_data
        {
            let mut at = Vec::new();
            for (rf, pos) in &pf.sort_refs {
                if let (Ok(col), Some(k)) = (column(m, cube, *rf as usize), key_at(*rf as usize, *pos)) {
                    at.push((col, k));
                }
            }
            l.sort = Some(Sort { data: d as usize, descending: pf.sort == SortType::Descending, at });
        }
        if !pf.source_sort {
            // A ordem que está na tela (a ordenação por valor desempata pelos
            // itens); membros novos vão no fim.
            let mut order: Vec<u32> = Vec::new();
            if pf.sort == SortType::Manual {
                for (fields, items) in [(&t.row_fields, &t.row_items), (&t.col_fields, &t.col_items)] {
                    if let Some(lv) = fields.iter().position(|x| *x == f) {
                        order.extend(shown(items, lv));
                    }
                }
            }
            let rest: Vec<u32> = (0..pf.items.len() as u32).filter(|p| !order.contains(p)).collect();
            order.extend(rest);
            l.manual = order.into_iter().filter_map(|p| key_at(fi, p)).collect();
            l.manual.extend(custom_list(&l.manual));
        }
        l.collapsed = pf
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.collapsed)
            .filter_map(|(p, _)| key_at(fi, p as u32))
            .collect();
        Ok(l)
    };
    let axis_fields = |l: &[i32]| l.iter().copied().filter(|f| *f >= 0).collect::<Vec<_>>();
    let (rf, cf) = (axis_fields(&t.row_fields), axis_fields(&t.col_fields));
    q.rows = rf.iter().map(|f| level(*f)).collect::<Res<_>>()?;
    q.cols = cf.iter().map(|f| level(*f)).collect::<Res<_>>()?;
    let a = crate::cube::run(m, &q)?;

    // Itens novos dos campos dos eixos.
    let mut nt = t.clone();
    let mut nc = cache.clone();
    for (fields, ax) in [(&rf, &a.rows), (&cf, &a.cols)] {
        for (f, members) in fields.iter().zip(&ax.members) {
            let fi = *f as usize;
            let col = column(m, cube, fi)?;
            let date = m.tables[col.0].columns[col.1].kind == Kind::Date;
            let old = &t.fields[fi].items;
            let collapsed: Vec<String> = old
                .iter()
                .enumerate()
                .filter(|(_, i)| i.collapsed)
                .filter_map(|(p, _)| key_at(fi, p as u32))
                .collect();
            // Todos recolhidos antes: os membros novos também entram recolhidos.
            let all = !old.is_empty() && old.iter().filter(|i| i.t.is_none()).all(|i| i.collapsed);
            let items: Vec<PItem> = members.iter().map(|v| item(v, date)).collect();
            nt.fields[fi].items = items
                .iter()
                .enumerate()
                .map(|(x, it)| FieldItem {
                    x: Some(x as u32),
                    collapsed: all || collapsed.contains(&item_key(it)),
                    ..Default::default()
                })
                .collect();
            if let Some(p) = &mut nt.cube {
                p.items.retain(|x| x.0 != fi as u32);
                p.items.push((fi as u32, items.clone()));
            }
            if let Some(cf) = nc.fields.get_mut(fi) {
                cf.items = items;
            }
        }
    }

    // Linhas e colunas no formato do arquivo.
    let n_data = t.data_fields.len();
    let build = |fields: &[i32], paths: &[Vec<u32>], grand: bool| -> (Vec<AxisItem>, Vec<Vec<u32>>) {
        let data_at = fields.iter().position(|f| *f == -2);
        let many = data_at.is_some() && n_data > 1;
        let mut items = Vec::new();
        let mut keys = Vec::new();
        let mut prev: Vec<u32> = Vec::new();
        let depth = fields.len();
        let mut push = |full: Vec<u32>, i: u32, t: Option<&str>, key: Vec<u32>| {
            let r = if t.is_some() {
                0
            } else {
                full.iter().zip(&prev).take_while(|(a, b)| a == b).count().min(full.len().saturating_sub(1))
            };
            items.push(AxisItem { t: t.map(Into::into), r: r as u32, i, x: full[r..].to_vec() });
            keys.push(key);
            if t.is_none() {
                prev = full;
            }
        };
        if n_data == 0 && fields.is_empty() {
            return (items, keys);
        }
        if paths.len() == 1 && paths[0].is_empty() {
            // Sem campos: uma linha (ou uma por medida).
            if many {
                for k in 0..n_data as u32 {
                    push(vec![k], k, None, Vec::new());
                }
            } else {
                push(Vec::new(), 0, None, Vec::new());
            }
            return (items, keys);
        }
        for p in paths {
            if many {
                let at = data_at.unwrap_or(0).min(p.len());
                for k in 0..n_data as u32 {
                    let mut full = p.clone();
                    full.insert(at, k);
                    push(full, k, None, p.clone());
                }
            } else {
                push(p.clone(), 0, None, p.clone());
            }
        }
        let _ = depth;
        if grand && !paths.is_empty() {
            let n = if many { n_data as u32 } else { 1 };
            for k in 0..n {
                push(vec![0], k, Some("grand"), Vec::new());
            }
        }
        (items, keys)
    };
    // `rowGrandTotals` é a linha de total geral; `colGrandTotals`, a coluna.
    let row_empty = a.rows.members.is_empty();
    let col_empty = a.cols.members.is_empty();
    let (row_items, row_paths) = build(&t.row_fields, &a.rows.paths, t.row_grand && !row_empty);
    let (col_items, col_paths) = build(&t.col_fields, &a.cols.paths, t.col_grand && !col_empty);
    let mut values: HashMap<(Vec<u32>, Vec<u32>, u32), CellValue> = HashMap::with_capacity(a.values.len());
    for ((r, c, k), v) in &a.values {
        values.insert((r.clone(), c.clone(), *k), cell(v));
    }
    let result =
        PivotResult { row_items, col_items, row_paths, col_paths, cube: Some(values), ..Default::default() };
    Ok(Computed { table: nt, cache: nc, result })
}

/// Posições de um nível na ordem em que aparecem num eixo.
fn shown(items: &[AxisItem], level: usize) -> Vec<u32> {
    let mut path: Vec<u32> = Vec::new();
    let mut out = Vec::new();
    for it in items.iter().filter(|i| i.t.is_none()) {
        path.truncate(it.r as usize);
        path.extend(&it.x);
        if let Some(&p) = path.get(level)
            && !out.contains(&p)
        {
            out.push(p);
        }
    }
    out
}

/// Meses e dias da semana: listas próprias do Excel, na ordem do calendário.
/// Devolve a lista que contém os membros (para os que aparecerem depois).
fn custom_list(members: &[String]) -> Vec<String> {
    const LISTS: [&[&str]; 4] = [
        &["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"],
        &[
            "janeiro",
            "fevereiro",
            "março",
            "abril",
            "maio",
            "junho",
            "julho",
            "agosto",
            "setembro",
            "outubro",
            "novembro",
            "dezembro",
        ],
        &["dom", "seg", "ter", "qua", "qui", "sex", "sáb"],
        &["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"],
    ];
    let inside = |l: &&&[&str]| {
        !members.is_empty()
            && members.iter().filter(|m| !m.is_empty()).all(|m| l.contains(&m.to_lowercase().as_str()))
    };
    LISTS.iter().find(inside).map(|l| l.iter().map(|s| s.to_string()).collect()).unwrap_or_default()
}

/// Chave de membro de um item do cache.
pub fn item_key(i: &PItem) -> String {
    match i {
        PItem::Str(s) => s.clone(),
        PItem::Num(n) => member_key(&Val::Num(*n), false),
        PItem::Date(n) => member_key(&Val::Num(*n), true),
        PItem::Bool(b) => member_key(&Val::Bool(*b), false),
        PItem::Err(e) => e.clone(),
        PItem::Missing => String::new(),
    }
}

/// Itens de uma segmentação: (nome único, rótulo, tem dados).
pub fn slicer_items(
    m: &Model,
    hierarchy: &str,
    filters: &[((usize, usize), Vec<String>)],
) -> Res<Vec<(String, String, bool)>> {
    let (t, c) = split_unique(hierarchy).ok_or("hierarquia da segmentação")?;
    let col = m.find(&t, &c).ok_or("coluna fora do modelo")?;
    let date = m.tables[col.0].columns[col.1].kind == Kind::Date;
    let others: Vec<_> = filters.iter().filter(|f| f.0 != col).cloned().collect();
    Ok(crate::cube::members(m, col, &others)?
        .into_iter()
        .map(|(v, has)| {
            let key = member_key(&v, date);
            let unique = match v {
                Val::Blank => format!("{hierarchy}.&"),
                _ => format!("{hierarchy}.&[{}]", key.replace(']', "]]")),
            };
            (unique, member_caption(&v), has)
        })
        .collect())
}
