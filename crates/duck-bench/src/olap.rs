//! `duck-bench modelo ARQUIVO [--hoje AAAA-MM-DD]`: recalcula as dinâmicas do
//! Modelo de Dados e compara com o que o Excel gravou. Só contagens.

use std::sync::Arc;
use std::time::Instant;

use duck_core::{CellValue, Sheet, Value, Workbook};
use duck_dax::olap;
use duck_xlsx::Options;

use crate::Res;

#[derive(Default, Debug)]
pub struct Report {
    pub tables: u32,
    /// Não calculadas (medida não suportada, coluna fora do modelo...).
    pub failed: u32,
    pub layout_ok: u32,
    pub values_ok: u64,
    pub values_bad: u64,
    /// Tabelas com tudo igual.
    pub exact: u32,
    pub ms: f64,
    pub errors: Vec<String>,
}

/// Segmentações do modelo ligadas a uma dinâmica.
pub fn selections(sheets: &[Arc<Sheet>], tab: u32, t: &duck_core::pivot::PivotTable) -> Vec<olap::Selection> {
    let mut out: Vec<olap::Selection> = Vec::new();
    for o in sheets.iter().flat_map(|s| &s.layout.objects) {
        let duck_core::drawing::ObjectKind::Slicer(s) = &o.kind else { continue };
        let Some(link) = s.pivot.as_ref().filter(|l| l.is_cube()) else { continue };
        if !link.tables.iter().any(|x| x.0 == tab && x.1 == t.name) || s.items.iter().all(|i| i.1) {
            continue;
        }
        if !out.iter().any(|x| x.0 == link.source) {
            let chosen = link.members.iter().zip(&s.items).filter(|x| x.1.1).map(|x| x.0.clone());
            out.push((link.source.clone(), chosen.collect()));
        }
    }
    out
}

pub fn check(
    path: &std::path::Path,
    wb: Workbook,
    verbose: bool,
    today: Option<f64>,
) -> Result<Report, String> {
    let mut rep = Report::default();
    let book = duck_xlsx::Book::open(path).map_err(|e| e.to_string())?;
    let Some(def) = book.read_model() else { return Ok(rep) };
    let sheets: Vec<Arc<Sheet>> = wb.sheets.into_iter().map(Arc::new).collect();
    let strings = Arc::new(wb.shared_strings);
    let source = olap::SheetSource::new(&def, sheets.clone(), strings.clone());
    if verbose {
        println!("{} tabelas no modelo, {} sem origem", def.tables.len(), source.missing());
    }
    let now = today.or(wb.extras.saved_at).unwrap_or(0.0);
    let t0 = Instant::now();
    let model = olap::build(&def, Box::new(source), now);
    if verbose {
        println!(
            "{} medidas, {} com fórmula não entendida, {} relações",
            model.measures.len(),
            model.measures.values().filter(|m| m.body.is_err()).count(),
            model.rels.len()
        );
    }
    for (si, sheet) in sheets.iter().enumerate() {
        for t in &sheet.layout.pivots {
            let Some(cache) = wb.extras.pivot_caches.iter().find(|c| c.id == t.cache_id && c.external) else {
                continue;
            };
            let Some(cube) = book.read_cube_cache(&cache.part) else { continue };
            rep.tables += 1;
            let slicers = selections(&sheets, sheet.tab_id, t);
            let t1 = Instant::now();
            let c = match olap::compute(&model, &cube, cache, t, &slicers) {
                Ok(c) => c,
                Err(e) => {
                    rep.failed += 1;
                    rep.errors.push(e);
                    continue;
                }
            };
            let ms = t1.elapsed().as_secs_f64() * 1000.0;
            // Membros iguais aos do arquivo, na mesma ordem, e as mesmas linhas.
            let names =
                |c: &duck_core::pivot::PivotCache, t: &duck_core::pivot::PivotTable, f: i32| -> Vec<String> {
                    let f = f as usize;
                    let items = t.fields[f].items.iter().filter_map(|i| i.x);
                    items
                        .map(|x| {
                            c.fields[f]
                                .items
                                .get(x as usize)
                                .map(olap::item_key)
                                .unwrap_or_default()
                                .to_lowercase()
                        })
                        .collect()
                };
            let mut same = c.result.row_items.len() == t.row_items.len()
                && c.result.col_items.len() == t.col_items.len();
            let label = |c: &duck_core::pivot::PivotCache,
                         t: &duck_core::pivot::PivotTable,
                         items: &[duck_core::pivot::AxisItem],
                         fields: &[i32]|
             -> Vec<Vec<String>> {
                let mut path: Vec<u32> = Vec::new();
                items
                    .iter()
                    .map(|it| {
                        path.truncate(it.r as usize);
                        path.extend(&it.x);
                        if it.t.is_some() {
                            return vec![it.t.clone().unwrap_or_default()];
                        }
                        path.iter()
                            .zip(fields)
                            .map(|(p, f)| {
                                if *f < 0 {
                                    format!("#{p}")
                                } else {
                                    names(c, t, *f).get(*p as usize).cloned().unwrap_or_default()
                                }
                            })
                            .collect()
                    })
                    .collect()
            };
            let (ra, rb) = (
                label(&c.cache, &c.table, &c.result.row_items, &t.row_fields),
                label(cache, t, &t.row_items, &t.row_fields),
            );
            let (ca, cb) = (
                label(&c.cache, &c.table, &c.result.col_items, &t.col_fields),
                label(cache, t, &t.col_items, &t.col_fields),
            );
            same = same && ra == rb && ca == cb;
            rep.layout_ok += same as u32;
            // Valores: pelas linhas e colunas com o mesmo rótulo.
            let (r0, c0) = (t.range.first_row + t.first_data_row, t.range.first_col + t.first_data_col);
            let (mut ok, mut bad) = (0u64, 0u64);
            for (i, l) in rb.iter().enumerate() {
                let Some(ri) = ra.iter().position(|x| x == l) else {
                    bad += cb.len() as u64;
                    continue;
                };
                for (j, cl) in cb.iter().enumerate() {
                    let Some(ci) = ca.iter().position(|x| x == cl) else {
                        bad += 1;
                        continue;
                    };
                    let mine =
                        duck_calc::pivot::data_value(&c.table, &c.result, ri, ci).unwrap_or(CellValue::Empty);
                    let excel = sheet.cell(r0 + i as u32, c0 + j as u32, &strings).value;
                    let eq = match (&mine, excel) {
                        (CellValue::Empty, Value::Empty) => true,
                        (CellValue::Number(a), Value::Number(b)) => (a - b).abs() <= 1e-9 * a.abs().max(1.0),
                        (CellValue::Text(a), Value::Text(b)) => a == b,
                        _ => false,
                    };
                    if eq { ok += 1 } else { bad += 1 }
                }
            }
            rep.values_ok += ok;
            rep.values_bad += bad;
            rep.exact += (same && bad == 0) as u32;
            if verbose && (!same || bad > 0) {
                let common = rb.iter().filter(|l| ra.contains(l)).count();
                // Depuração sem conteúdo: só formatos (letra = x, número = 9).
                if std::env::var("DUCK_FORMAS").is_ok() {
                    let shape = |s: &str| -> String {
                        s.chars()
                            .take(14)
                            .map(|c| {
                                if c.is_alphabetic() {
                                    'x'
                                } else if c.is_numeric() {
                                    '9'
                                } else {
                                    c
                                }
                            })
                            .collect()
                    };
                    for (col, keys) in olap::filters(&model, &cube, t, &slicers).unwrap_or_default() {
                        let c = model.column(col.0, col.1).unwrap();
                        let def = &model.tables[col.0].columns[col.1];
                        let texts: Vec<String> = c.dict.iter().map(duck_dax::cube::member_caption).collect();
                        let hit = keys
                            .iter()
                            .filter(|k| texts.iter().any(|t| t.to_lowercase() == k.to_lowercase()))
                            .count();
                        println!(
                            "    filtro tipo {:?} calc {} dic {} linhas {}: {hit}/{} achadas; chaves {:?}; valores {:?}",
                            def.kind,
                            def.expr.is_some(),
                            c.dict.len(),
                            c.codes.len(),
                            keys.len(),
                            keys.iter().map(|k| shape(k)).collect::<Vec<_>>(),
                            texts.iter().take(4).map(|k| shape(k)).collect::<Vec<_>>()
                        );
                    }
                    for (col, keys) in olap::filters(&model, &cube, t, &slicers).unwrap_or_default() {
                        let c = model.column(col.0, col.1).unwrap();
                        let has = |c: &duck_dax::Column, k: &str| {
                            c.dict
                                .iter()
                                .any(|v| duck_dax::cube::member_caption(v).to_lowercase() == k.to_lowercase())
                        };
                        if keys.iter().all(|k| has(&c, k)) {
                            continue;
                        }
                        let name = &model.tables[col.0].columns[col.1].name;
                        let tb = sheets
                            .iter()
                            .flat_map(|s| &s.layout.tables)
                            .find(|x| Some(&x.name) == def.tables[col.0].source.as_ref());
                        let exact = tb.map(|x| x.columns.iter().filter(|c| *c == name).count());
                        let loose = tb.map(|x| {
                            x.columns.iter().filter(|c| c.trim().eq_ignore_ascii_case(name.trim())).count()
                        });
                        let pos = tb.and_then(|x| x.columns.iter().position(|c| c == name));
                        let with: Vec<usize> = (0..model.tables[col.0].columns.len())
                            .filter(|i| {
                                model.column(col.0, *i).is_ok_and(|c| keys.iter().any(|k| has(&c, k)))
                            })
                            .collect();
                        if let Some((sh, tb)) = sheets.iter().find_map(|s| {
                            s.layout
                                .tables
                                .iter()
                                .find(|x| Some(&x.name) == def.tables[col.0].source.as_ref())
                                .map(|t| (s, t))
                        }) {
                            let head = |i: usize| match sh
                                .cell(tb.range.first_row, tb.range.first_col + i as u32, &strings)
                                .value
                            {
                                Value::Text(x) => x.to_string(),
                                _ => String::new(),
                            };
                            let bad: Vec<usize> =
                                (0..tb.columns.len()).filter(|i| head(*i) != tb.columns[*i]).collect();
                            let w = tb.range.last_col - tb.range.first_col + 1;
                            println!(
                                "    cabeçalhos diferentes do nome da coluna: {bad:?}; largura {w}; nome da procurada em {:?}",
                                (0..w as usize).filter(|i| head(*i) == *name).collect::<Vec<_>>()
                            );
                            let c = model.column(col.0, col.1).unwrap();
                            let kinds: Vec<_> = (0..c.dict.len())
                                .map(|k| {
                                    (
                                        c.codes.iter().filter(|x| **x as usize == k).count(),
                                        duck_dax::cube::member_caption(&c.dict[k]).chars().count(),
                                    )
                                })
                                .collect();
                            println!(
                                "    valores (quantos, tamanho): {kinds:?}; fórmula {:?}",
                                duck_calc::cell_formula(
                                    sh,
                                    tb.range.first_row + 1,
                                    tb.range.first_col + pos.unwrap_or(0) as u32
                                )
                                .map(|f| f.len())
                            );
                        }
                        println!(
                            "    coluna {} de {} no modelo; na tabela: exata {exact:?} parecida {loose:?} posição {pos:?} de {:?}; colunas com a chave {with:?}; linhas {}",
                            col.1,
                            model.tables[col.0].columns.len(),
                            tb.map(|x| x.columns.len()),
                            model.rows(col.0)
                        );
                    }
                    let miss: Vec<_> = rb
                        .iter()
                        .filter(|l| !ra.contains(l))
                        .take(3)
                        .map(|l| l.iter().map(|x| shape(x)).collect::<Vec<_>>())
                        .collect();
                    let extra: Vec<_> = ra
                        .iter()
                        .filter(|l| !rb.contains(l))
                        .take(3)
                        .map(|l| l.iter().map(|x| shape(x)).collect::<Vec<_>>())
                        .collect();
                    println!("    só no Excel {miss:?}; só aqui {extra:?}");
                    if ca != cb {
                        let f = |v: &Vec<Vec<String>>| {
                            v.iter()
                                .take(13)
                                .map(|l| l.iter().map(|x| shape(x)).collect::<Vec<_>>().join("/"))
                                .collect::<Vec<_>>()
                        };
                        let pos: Vec<_> = cb.iter().map(|l| ca.iter().position(|x| x == l)).collect();
                        println!("    colunas Excel {:?} aqui {:?} posições {pos:?}", f(&cb), f(&ca));
                    }
                    if ra != rb && ra.len() == rb.len() {
                        let pos: Vec<_> =
                            rb.iter().map(|l| ra.iter().position(|x| x == l)).take(24).collect();
                        let fl = &t.fields[t.row_fields[0] as usize];
                        if fl.sort_by_data.is_some() {
                            let nat = |l: &Vec<String>| -> i64 {
                                let mut v: Vec<&Vec<String>> = rb.iter().collect();
                                v.sort_by(|a, b| duck_dax::model::collate(&a[0], &b[0]));
                                v.iter().position(|x| *x == l).map_or(-1, |x| x as i64)
                            };
                            let row: Vec<(u32, i64, u32)> = t.row_items.iter().enumerate().map(|(i, it)| {
                                let v = match sheet.cell(r0 + i as u32, c0, &strings).value { Value::Number(x) => x, _ => -1.0 };
                                let same = t.row_items.iter().enumerate().filter(|(k, _)| matches!(sheet.cell(r0 + *k as u32, c0, &strings).value, Value::Number(x) if x == v)).count() as u32;
                                (it.x.first().copied().unwrap_or(0), nat(&rb[i]), same)
                            }).collect();
                            println!("    (item, ordem natural, empatados) {row:?}");
                        }
                        println!(
                            "    linhas {pos:?} source_sort {} ordem {:?} por medida {:?}",
                            fl.source_sort, fl.sort, fl.sort_by_data
                        );
                    }
                    let mut per: std::collections::BTreeMap<(usize, &str), u32> =
                        std::collections::BTreeMap::new();
                    let (mut kinds, mut where_) =
                        (std::collections::BTreeMap::new(), std::collections::BTreeMap::new());
                    for (i, l) in rb.iter().enumerate() {
                        let Some(ri) = ra.iter().position(|x| x == l) else { continue };
                        for (j, cl) in cb.iter().enumerate() {
                            let Some(ci) = ca.iter().position(|x| x == cl) else { continue };
                            let mine = duck_calc::pivot::data_value(&c.table, &c.result, ri, ci)
                                .unwrap_or(CellValue::Empty);
                            let ex = sheet.cell(r0 + i as u32, c0 + j as u32, &strings).value;
                            let k = match (&mine, ex) {
                                (CellValue::Number(a), Value::Number(b))
                                    if (a - b).abs() <= 1e-9 * a.abs().max(1.0) =>
                                {
                                    continue;
                                }
                                (CellValue::Number(a), Value::Number(b)) => {
                                    if *a > b {
                                        "maior"
                                    } else {
                                        "menor"
                                    }
                                }
                                (CellValue::Empty, Value::Empty) => continue,
                                (CellValue::Empty, Value::Number(x)) => {
                                    if x == 0.0 {
                                        "vazio aqui, zero no Excel"
                                    } else {
                                        "vazio aqui, número no Excel"
                                    }
                                }
                                (CellValue::Empty, Value::Text(x)) => {
                                    if x.is_empty() {
                                        "vazio aqui, texto vazio"
                                    } else {
                                        "vazio aqui, texto no Excel"
                                    }
                                }
                                (CellValue::Empty, _) => "vazio aqui, erro",
                                (_, Value::Empty) => "vazio no Excel",
                                (CellValue::Text(a), Value::Text(b)) if a == b => continue,
                                (CellValue::Text(a), Value::Text(b)) => {
                                    let (mut x, mut y): (Vec<&str>, Vec<&str>) =
                                        (a.split(", ").collect(), b.split(", ").collect());
                                    let sorted =
                                        y.windows(2).all(|w| duck_dax::model::collate(w[0], w[1]).is_le());
                                    let rev =
                                        y.windows(2).all(|w| duck_dax::model::collate(w[0], w[1]).is_ge());
                                    let (nx, ny) = (x.len(), y.len());
                                    let cx = x.iter().filter(|v| y.contains(v)).count();
                                    let ex = y
                                        .iter()
                                        .map(|v| x.iter().position(|q| q == v).map_or(-1, |p| p as i32))
                                        .take(12)
                                        .collect::<Vec<_>>();
                                    println!(
                                        "      texto: {nx} aqui, {ny} no Excel, {cx} comuns; Excel em ordem {sorted} inversa {rev}; posições {ex:?}"
                                    );
                                    x.sort();
                                    y.sort();
                                    if a.len() == b.len() {
                                        "texto em outra ordem"
                                    } else {
                                        "texto diferente"
                                    }
                                }
                                _ => "tipo",
                            };
                            *kinds.entry(k).or_insert(0) += 1;
                            *per.entry((j, k)).or_insert(0) += 1;
                            *where_.entry((j, t.col_items[j].t.clone())).or_insert(0) += 1;
                        }
                    }
                    if t.data_fields.len() > 3 {
                        for ((j, k), n) in &per {
                            let d = t
                                .data_fields
                                .get(*j)
                                .map(|d| cube.fields[d.fld as usize].name.clone())
                                .unwrap_or_default();
                            let name = duck_core::cube::split_unique(&d).map(|x| x.1).unwrap_or_default();
                            let body = model
                                .measures
                                .get(&name.to_lowercase())
                                .map(|m| format!("{:?}", m.body))
                                .unwrap_or_default();
                            let funcs: Vec<&str> =
                                body.split("Func(\"").skip(1).filter_map(|x| x.split('"').next()).collect();
                            println!(
                                "      medida {j} {k} ×{n}: {funcs:?} {}",
                                if body.contains("Bracket") { "refs" } else { "" }
                            );
                        }
                    }
                    println!("    diferenças {kinds:?} colunas {where_:?}");
                }
                println!(
                    "  aba {} parte {}: linhas {}/{} (comuns {common}, mesma ordem {}), colunas {}/{}, valores {ok} iguais {bad} diferentes, filtros {:?}, ordenação {}, medidas {}, {ms:.0} ms",
                    si + 1,
                    t.part.chars().filter(|c| c.is_ascii_digit()).collect::<String>(),
                    ra.len(),
                    rb.len(),
                    rb.iter().filter(|l| ra.contains(l)).eq(ra.iter().filter(|l| rb.contains(l))),
                    ca.len(),
                    cb.len(),
                    olap::filters(&model, &cube, t, &slicers)
                        .map(|f| f.iter().map(|x| x.1.len()).collect::<Vec<_>>())
                        .unwrap_or_default(),
                    t.fields.iter().filter(|f| f.sort_by_data.is_some()).count(),
                    t.data_fields.len(),
                );
            }
        }
    }
    rep.ms = t0.elapsed().as_secs_f64() * 1000.0;
    Ok(rep)
}

pub fn cmd(args: &[String]) -> Res {
    let path = crate::file_arg(args)?;
    let (wb, _) = duck_xlsx::open(&path, &Options::default()).map_err(|e| e.to_string())?;
    let today = crate::flag(args, "--hoje").and_then(|d| {
        let p: Vec<i64> = d.split('-').filter_map(|x| x.parse().ok()).collect();
        (p.len() == 3).then(|| duck_core::input::serial(p[0] as i32, p[1] as u32, p[2] as u32))
    });
    let r = check(&path, wb, true, today)?;
    let mut errs: std::collections::BTreeMap<&str, u32> = Default::default();
    for e in &r.errors {
        *errs.entry(e).or_default() += 1;
    }
    println!(
        "{} dinâmicas do modelo: {} com o mesmo layout, {} idênticas, {} valores iguais, {} diferentes, {} não calculadas {:?}, {:.0} ms",
        r.tables, r.layout_ok, r.exact, r.values_ok, r.values_bad, r.failed, errs, r.ms
    );
    Ok(())
}
