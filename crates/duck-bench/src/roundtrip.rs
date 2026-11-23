//! `duck-bench roundtrip`: abre, (opcionalmente edita,) salva numa cópia e
//! confere (F3, ADR-006).
//!
//! 1. O salvamento já relê as abas regravadas e compara com o modelo
//!    (`duck_xlsx::save_file`).
//! 2. As partes não regravadas precisam ser idênticas byte a byte.
//! 3. A cópia é lida de novo e comparada célula a célula com o calamine.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use duck_core::{CellValue, FormulaKind, Sheet};
use duck_xlsx::{Package, SaveOptions};

use crate::mem::{self, mb};
use crate::{Res, file_arg, flag, load, ms, threads};

pub fn cmd(args: &[String]) -> Res {
    let src = file_arg(args)?;
    let rewrite_all = args.iter().any(|a| a == "--rewrite-all");
    let keep = args.iter().any(|a| a == "--keep");
    let edits: usize = flag(args, "--edit").and_then(|v| v.parse().ok()).unwrap_or(0);
    let structure = args.iter().any(|a| a == "--estrutura");
    let tabs = args.iter().any(|a| a == "--abas");
    let level: u32 = flag(args, "--level").and_then(|v| v.parse().ok()).unwrap_or(6);
    let out = match flag(args, "--out") {
        Some(o) => PathBuf::from(o),
        None => {
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("xlsx");
            std::env::temp_dir().join(format!("duck-roundtrip-{}.{ext}", std::process::id()))
        }
    };

    let (mut wb, rep) = load(&src, args)?;
    println!("lido em {:.0} ms ({} abas)", ms(rep.total), wb.sheets.len());
    if edits > 0 {
        let (sheet, n) = edit(&mut wb.sheets, &mut wb.styles, edits);
        println!("{n} edições na aba {} ({} linhas)", sheet + 1, wb.sheets[sheet].rows());
    }
    if structure {
        let (sheet, ms) = restructure(&mut wb);
        println!("linhas e colunas inseridas/apagadas na aba {} em {ms:.0} ms", sheet + 1);
    }

    if args.iter().any(|a| a == "--celulas") {
        println!("{}", shift_cells(&mut wb)?);
    }
    let mut opts = SaveOptions { level, rewrite_all, ..Default::default() };
    if tabs {
        let t = Instant::now();
        let (plan, what) = retab(&src, &mut wb)?;
        println!("abas: {what} em {:.0} ms", ms(t.elapsed()));
        opts.tabs = Some(plan);
    }
    let sheets: Vec<Option<&Sheet>> = wb.sheets.iter().map(Some).collect();
    let pool = duck_par::pool(threads(args));
    let before = mem::process();
    let t = Instant::now();
    let saved =
        pool.install(|| duck_xlsx::save_file(&src, &out, &sheets, &wb.shared_strings, &wb.styles, &opts));
    let wall = t.elapsed();
    let saved = saved.map_err(|e| format!("salvar: {e}"))?;
    let after = mem::process();
    println!(
        "salvo em {:.0} ms (gravação {:.0} ms + conferência {:.0} ms), {:.1} MB → {:.1} MB",
        ms(wall),
        ms(saved.wall),
        ms(saved.verify),
        mb(std::fs::metadata(&src).map_or(0, |m| m.len())),
        mb(saved.bytes),
    );
    println!(
        "{} partes copiadas, {} regravadas ({} abas, {} células escritas, XML {:.0} → {:.0} MB){}{}",
        saved.parts_copied,
        saved.parts_rewritten,
        saved.sheets_rewritten.len(),
        saved.cells_written,
        mb(saved.xml_in),
        mb(saved.xml_out),
        if saved.full_calc { ", recálculo ao abrir" } else { "" },
        if saved.calc_chain_removed { ", calcChain removido" } else { "" },
    );
    println!("memória: {:.0} MB antes, pico {:.0} MB", mb(before.current), mb(after.peak));

    let mut result = check(&src, &out, &saved, args);
    if tabs && result.is_ok() {
        result = check_tabs(&src, &out, &wb, args);
    }
    if !keep {
        let _ = std::fs::remove_file(&out);
    } else {
        println!("cópia mantida em {}", out.display());
    }
    result
}

fn check(src: &Path, out: &Path, saved: &duck_xlsx::SaveReport, args: &[String]) -> Res {
    // Partes copiadas: idênticas byte a byte.
    let a = Package::open(src).map_err(|e| e.to_string())?;
    let b = Package::open(out).map_err(|e| e.to_string())?;
    let names_b: HashSet<&str> = b.entries().map(|e| e.name.as_str()).collect();
    let (mut same, mut changed, mut missing) = (0, Vec::new(), Vec::<String>::new());
    for e in a.entries_in_order() {
        if !names_b.contains(e.name.as_str()) {
            missing.push(e.name.clone());
            continue;
        }
        let f = b.entry(&e.name).unwrap();
        if a.compressed(e) == b.compressed(f) && e.crc32 == f.crc32 {
            same += 1;
        } else {
            changed.push(e.name.clone());
        }
    }
    println!("partes idênticas: {same}; mudadas: {changed:?}; fora: {missing:?}");
    // Uma parte regravada pode sair idêntica (mesmo XML, mesmo deflate).
    if changed.len() > saved.parts_rewritten + saved.parts_added.len() {
        return Err(format!(
            "{} partes mudaram, mas {} foram regravadas",
            changed.len(),
            saved.parts_rewritten
        ));
    }
    missing.retain(|m| !saved.parts_removed.contains(m));
    if missing.len() != usize::from(saved.calc_chain_removed) {
        return Err("partes sumiram na cópia".into());
    }
    let added = b.entries().filter(|e| a.entry(&e.name).is_none()).count();
    if added > 0 || !saved.parts_removed.is_empty() {
        println!("partes novas: {added}; excluídas: {}", saved.parts_removed.len());
    }
    drop((a, b));
    let problems = crate::lint::check(out);
    let before = crate::lint::check(src);
    let new: Vec<&String> = problems.iter().filter(|p| !before.contains(p)).collect();
    if !new.is_empty() {
        new.iter().for_each(|p| println!("  pacote: {p}"));
        return Err("o pacote salvo tem problemas".into());
    }

    // A cópia lida pelo calamine.
    let t = Instant::now();
    let (wb, _) = load(out, args)?;
    let o = crate::verify::run(out, &wb)?;
    println!(
        "calamine na cópia: {} células, {} divergências; {} fórmulas, {} divergências ({:.0} ms)",
        o.compared,
        o.mismatches,
        o.formulas_compared,
        o.formula_mismatches,
        ms(t.elapsed())
    );
    for s in &o.samples {
        println!("  {s}");
    }
    if o.mismatches > 0 || o.formula_mismatches > 0 || o.merge_mismatches > 0 {
        return Err("a cópia salva não bate com o calamine".into());
    }
    println!("round-trip OK");
    Ok(())
}

/// Move um bloco, insere e exclui células (deslocando) e escreve texto rico,
/// em cada aba em que dá (as recusas, como cortar uma mesclagem, são
/// contadas). Confere que desfazer devolve tudo e refaz.
fn shift_cells(wb: &mut duck_core::Workbook) -> Result<String, String> {
    use duck_calc::cellops;
    use duck_core::cells::CellOp;
    use duck_core::{Axis, CellChange, CellRange, Command, RichRun, RunFont, Shift};
    let t = Instant::now();
    let (mut done, mut refused, mut cells) = (0, 0, 0usize);
    let range =
        |a: u32, b: u32, c: u32, d: u32| CellRange { first_row: a, first_col: b, last_row: c, last_col: d };
    for i in 0..wb.sheets.len() {
        let (rows, cols) = (wb.sheets[i].rows(), wb.sheets[i].cols());
        if rows < 3 || cols < 1 {
            continue;
        }
        let (r, c) = (rows / 2, cols / 2);
        // Perto do fim nas abas grandes: deslocar milhões de células é o
        // trabalho de "inserir linhas".
        let low = rows.saturating_sub(40);
        let ops = [
            CellOp::Move { src: range(r, c, r + 1, c + 1), dr: 3, dc: 2 },
            CellOp::Move { src: range(0, 0, 1, 0), dr: i64::from(rows) + 2, dc: 0 },
            CellOp::Shift { shift: Shift { axis: Axis::Rows, at: low, n: 2, insert: true }, lo: c, hi: c },
            CellOp::Shift { shift: Shift { axis: Axis::Rows, at: low, n: 1, insert: false }, lo: 0, hi: 0 },
            CellOp::Shift { shift: Shift { axis: Axis::Cols, at: c, n: 1, insert: true }, lo: r, hi: r },
        ];
        for op in ops {
            let refs: Vec<&Sheet> = wb.sheets.iter().collect();
            let (cmds, book_refs) = match cellops::plan(&refs, &wb.shared_strings, &wb.extras, i, op) {
                Ok(x) => x,
                Err(_) => {
                    refused += 1;
                    continue;
                }
            };
            let touched: Vec<(usize, u32, u32)> =
                cmds.iter().flat_map(|(k, c)| c.cells().into_iter().map(move |(r, c)| (*k, r, c))).collect();
            let snap = |wb: &duck_core::Workbook| -> Vec<String> {
                touched
                    .iter()
                    .map(|&(k, r, c)| {
                        let s = &wb.sheets[k];
                        let cell = s.cell(r, c, &wb.shared_strings);
                        format!("{:?}{:?}", cell, duck_calc::cell_formula(s, r, c))
                    })
                    .collect()
            };
            let before = snap(wb);
            let layouts: Vec<_> = wb.sheets.iter().map(|s| s.layout.clone()).collect();
            let undo: Vec<(usize, Command)> =
                cmds.into_iter().map(|(k, c)| (k, c.apply(&mut wb.sheets[k]))).collect();
            let after = snap(wb);
            let redo: Vec<(usize, Command)> =
                undo.into_iter().rev().map(|(k, c)| (k, c.apply(&mut wb.sheets[k]))).collect();
            if snap(wb) != before || wb.sheets.iter().zip(&layouts).any(|(s, l)| s.layout != *l) {
                return Err(format!("aba {}: desfazer {op:?} não devolveu tudo", i + 1));
            }
            for (k, c) in redo.into_iter().rev() {
                c.apply(&mut wb.sheets[k]);
            }
            if snap(wb) != after {
                return Err(format!("aba {}: refazer {op:?} deu outro resultado", i + 1));
            }
            book_refs.swap(&mut wb.extras);
            done += 1;
            cells += touched.len();
        }
        // Texto rico novo.
        let runs = vec![
            RichRun { start: 0, font: None },
            RichRun { start: 4, font: Some(RunFont { bold: true, ..Default::default() }) },
        ];
        let cell = CellChange {
            row: rows + 5,
            col: 0,
            value: CellValue::Text("aba \nrica <&>".into()),
            formula: None,
            style: None,
        };
        Command::Group(vec![Command::SetCells(vec![cell]), Command::RichText(vec![(rows + 5, 0, runs)])])
            .apply(&mut wb.sheets[i]);
    }
    Ok(format!(
        "células: {done} operações ({refused} recusadas), {cells} células, desfazer e refazer conferidos, em {:.0} ms",
        ms(t.elapsed())
    ))
}

/// Mexe nas abas como o app: renomeia a primeira, copia a maior e a primeira
/// com objetos, inclui uma nova, exclui a última visível, muda a ordem, a cor
/// e a visibilidade. Devolve o plano para gravar.
fn retab(src: &Path, wb: &mut duck_core::Workbook) -> Result<(duck_xlsx::TabPlan, String), String> {
    use duck_calc::tabs;
    use duck_xlsx::{TabOrigin, TabOut, TabPlan};
    let book = duck_xlsx::Book::open(src).map_err(|e| e.to_string())?;
    let mut origins: Vec<TabOrigin> = book.sheets().into_iter().map(|s| TabOrigin::File(s.part)).collect();
    drop(book);
    let next_id = |sheets: &[Sheet]| sheets.iter().map(|s| s.tab_id).max().unwrap_or(0) + 1;
    fn run(sheets: &mut [Sheet], cmds: Vec<(usize, duck_core::Command)>) {
        for (i, c) in cmds {
            c.apply(&mut sheets[i]);
        }
    }
    let mut what = Vec::new();
    // Renomear.
    let refs: Vec<&Sheet> = wb.sheets.iter().collect();
    let names: Vec<&str> = refs.iter().map(|s| s.name.as_str()).collect();
    let old = names[0].to_string();
    let new = duck_core::tabs::copy_name("Aba d'água nº 1", &names);
    let (cmds, book_refs) = tabs::rename(&refs, &wb.extras, &old, &new);
    what.push(format!("1 renomeada ({} abas com referências)", cmds.len()));
    run(&mut wb.sheets, cmds);
    book_refs.swap(&mut wb.extras);
    wb.sheets[0].name = new;
    // Copiar: a maior e a primeira com objetos.
    let big = (0..wb.sheets.len()).max_by_key(|&i| wb.sheets[i].rows() as u64 * wb.sheets[i].cols() as u64);
    let obj = (0..wb.sheets.len()).filter(|&i| {
        let l = &wb.sheets[i].layout;
        !l.objects.is_empty() || !l.pivots.is_empty() || !l.tables.is_empty()
    });
    let mut which: Vec<usize> = obj.collect();
    which.extend(big.filter(|&b| (wb.sheets[b].rows() as u64) < 200_000));
    which.sort_unstable();
    which.dedup();
    let mut copied = 0;
    for i in which.into_iter().rev() {
        let refs: Vec<&Sheet> = wb.sheets.iter().collect();
        let names: Vec<&str> = refs.iter().map(|s| s.name.as_str()).collect();
        let name = duck_core::tabs::copy_name(&refs[i].name, &names);
        let c = tabs::duplicate(&refs, &wb.extras, i, i + 1, &name, next_id(&wb.sheets));
        let TabOrigin::File(part) = origins[i].clone() else { continue };
        c.refs.swap(&mut wb.extras);
        wb.sheets.insert(i + 1, c.sheet);
        origins.insert(i + 1, TabOrigin::Copy { part, renamed: c.renamed });
        copied += 1;
    }
    what.push(format!("{copied} copiadas"));
    // Nova, com valores e uma fórmula que lê a renomeada.
    let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
    let mut s = Sheet::new(duck_core::tabs::next_name(&names));
    s.tab_id = next_id(&wb.sheets);
    s.set_cell(0, 0, CellValue::Text("aba nova <&>".into()), None, None);
    s.set_cell(1, 0, CellValue::Number(2.5), None, None);
    let first = duck_calc::sheetrefs::quoted(&wb.sheets[0].name);
    s.set_cell(2, 1, CellValue::Number(5.0), Some(&format!("A2*2+{first}!A1")), None);
    s.set_col_width(0, Some(22.0));
    s.layout.merges.push(duck_core::CellRange { first_row: 4, first_col: 0, last_row: 5, last_col: 2 });
    duck_core::Command::TabColor(Some(duck_core::Color::Rgb(0xFF00_B050))).apply(&mut s);
    duck_core::Command::Pane(Some(duck_core::Pane { frozen_rows: 1, frozen_cols: 0 })).apply(&mut s);
    s.settle();
    tabs::insert(&wb.extras, 1).swap(&mut wb.extras);
    wb.sheets.insert(1, s);
    origins.insert(1, TabOrigin::Blank);
    what.push("1 nova".into());
    // Excluir a última visível do arquivo (se sobra outra).
    let files = (0..wb.sheets.len())
        .filter(|&i| i > 0 && !wb.sheets[i].hidden && matches!(origins[i], TabOrigin::File(_)))
        .collect::<Vec<_>>();
    if let Some(i) = files.into_iter().rev().find(|&i| {
        let refs: Vec<&Sheet> = wb.sheets.iter().collect();
        tabs::delete(&refs, &wb.extras, i).is_ok()
    }) {
        let refs: Vec<&Sheet> = wb.sheets.iter().collect();
        let (cmds, book_refs) = tabs::delete(&refs, &wb.extras, i)?;
        run(&mut wb.sheets, cmds);
        book_refs.swap(&mut wb.extras);
        wb.sheets.remove(i);
        origins.remove(i);
        what.push("1 excluída".into());
    }
    // Mover a primeira para o fim, pintar e esconder a segunda.
    let n = wb.sheets.len();
    tabs::reorder(&wb.extras, 0, n - 1).swap(&mut wb.extras);
    wb.sheets[..n].rotate_left(1);
    origins[..n].rotate_left(1);
    duck_core::Command::TabColor(Some(duck_core::Color::Rgb(0xFFFF_0000))).apply(&mut wb.sheets[n - 1]);
    if n > 2 {
        wb.sheets[1].hidden = true;
        what.push("1 escondida".into());
    }
    what.push("1 movida".into());
    let plan = TabPlan {
        tabs: wb
            .sheets
            .iter()
            .zip(origins)
            .map(|(s, origin)| TabOut { origin, name: s.name.clone(), id: s.tab_id, hidden: s.hidden })
            .collect(),
        names: wb.extras.names.clone(),
    };
    Ok((plan, what.join(", ")))
}

/// A cópia tem as abas do modelo: nomes, ordem, cor, visibilidade, fórmulas,
/// nomes definidos, objetos e tabelas.
fn check_tabs(src: &Path, out: &Path, wb: &duck_core::Workbook, args: &[String]) -> Res {
    let (back, _) = load(out, args)?;
    let names = |w: &duck_core::Workbook| {
        w.sheets.iter().map(|s| (s.name.clone(), s.hidden, s.tab_id, s.tab_color())).collect::<Vec<_>>()
    };
    if names(wb) != names(&back) {
        return Err("as abas da cópia não são as do modelo".into());
    }
    let mut formulas = 0u64;
    for (i, (a, b)) in wb.sheets.iter().zip(&back.sheets).enumerate() {
        for (r, c) in a.formula_positions() {
            formulas += 1;
            if duck_calc::cell_formula(a, r, c) != duck_calc::cell_formula(b, r, c) {
                return Err(format!("aba {}: fórmula diferente na cópia", i + 1));
            }
        }
        let rich = |s: &Sheet, sh: &duck_core::StringTable| {
            let r = s.rows().saturating_sub(1);
            s.rich_at(r, 0, sh).map(<[_]>::to_vec)
        };
        if rich(a, &wb.shared_strings) != rich(b, &back.shared_strings) {
            return Err(format!("aba {}: texto rico diferente na cópia", i + 1));
        }
        let kinds = |s: &Sheet| {
            let l = &s.layout;
            let names: Vec<&str> = l.tables.iter().map(|t| t.name.as_str()).collect();
            (l.objects.len(), l.pivots.len(), names.join(","), l.cond.len(), l.links.len())
        };
        if kinds(a) != kinds(b) {
            return Err(format!("aba {}: objetos diferentes na cópia", i + 1));
        }
        let refs = |s: &Sheet| {
            let mut v = Vec::new();
            for mut o in s.layout.objects.clone() {
                match &mut o.kind {
                    duck_core::drawing::ObjectKind::Chart(c) => v.extend(c.refs_mut().map(|r| r.clone())),
                    duck_core::drawing::ObjectKind::Slicer(x) => {
                        v.push(x.name.clone());
                        v.extend(x.pivot.iter().flat_map(|p| p.tables.iter().map(|t| format!("{t:?}"))));
                        v.extend(x.table.iter().map(|t| t.0.clone()));
                    }
                    duck_core::drawing::ObjectKind::Timeline(x) => v.push(x.name.clone()),
                    duck_core::drawing::ObjectKind::Control(x) => v.extend(x.link.clone()),
                    _ => {}
                }
            }
            v.extend(s.layout.links.iter().filter_map(|l| l.location.clone()));
            v
        };
        if refs(a) != refs(b) {
            let (x, y) = (refs(a), refs(b));
            let n = x.iter().zip(&y).filter(|(p, q)| p != q).count() + x.len().abs_diff(y.len());
            return Err(format!("aba {}: {n} referências de objetos diferentes na cópia", i + 1));
        }
    }
    let key = |w: &duck_core::Workbook| {
        let mut v: Vec<_> = w
            .extras
            .names
            .iter()
            .filter(|n| n.formula != "#N/A")
            .map(|n| (n.name.to_lowercase(), n.sheet, n.formula.clone()))
            .collect();
        v.sort();
        v
    };
    if key(wb) != key(&back) {
        let (x, y) = (key(wb), key(&back));
        let diff = x.iter().filter(|n| !y.contains(n)).count() + y.iter().filter(|n| !x.contains(n)).count();
        return Err(format!("{diff} nomes definidos diferentes na cópia"));
    }
    // Salvar de novo, sem mexer, não muda nenhuma parte.
    let again = out.with_extension("2.xlsx");
    let sheets: Vec<Option<&Sheet>> = back.sheets.iter().map(Some).collect();
    let opts = SaveOptions::default();
    duck_xlsx::save_file(out, &again, &sheets, &back.shared_strings, &back.styles, &opts)
        .map_err(|e| format!("salvar de novo: {e}"))?;
    let _ = std::fs::remove_file(&again);
    let _ = src;
    println!(
        "abas conferidas: {} abas, {formulas} fórmulas, {} nomes definidos",
        back.sheets.len(),
        back.extras.names.len()
    );
    Ok(())
}

/// Insere e apaga linhas e colunas na maior aba (como o app faz).
fn restructure(wb: &mut duck_core::Workbook) -> (usize, f64) {
    let sheets = &wb.sheets;
    let (i, _) = sheets
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| s.rows() as u64 * s.cols() as u64)
        .unwrap_or((0, &sheets[0]));
    let t = Instant::now();
    // A maior, e as que têm validação ou notas (que andam junto).
    let rules = |s: &Sheet| !s.layout.validations.is_empty() || !s.layout.notes.is_empty();
    let list: Vec<usize> = (0..sheets.len()).filter(|&k| k == i || rules(&sheets[k])).collect();
    for k in list {
        restructure_sheet(wb, k);
    }
    (i, t.elapsed().as_secs_f64() * 1000.0)
}

fn restructure_sheet(wb: &mut duck_core::Workbook, k: usize) {
    use duck_core::{Axis, Command, OpsHandle, Shift};
    let s = &mut wb.sheets[k];
    let name = s.name.clone();
    let rows = s.rows().max(2);
    let steps = [
        Shift { axis: Axis::Rows, at: rows / 2, n: 3, insert: true },
        Shift { axis: Axis::Rows, at: 1, n: 1, insert: false },
        Shift { axis: Axis::Cols, at: 1, n: 2, insert: true },
        Shift { axis: Axis::Cols, at: 0, n: 1, insert: false },
    ];
    for shift in steps {
        let ops =
            OpsHandle(std::sync::Arc::new(duck_calc::ShiftOps { shift, sheet: name.clone(), own: true }));
        Command::Shift(shift, ops).apply(s);
        duck_calc::tabs::shift_names(&wb.extras, shift, &name).swap(&mut wb.extras);
    }
}

/// Uma nota, uma regra de validação e proteção novas na aba.
fn edit_rules(s: &mut Sheet) {
    use duck_core::{CellRange, Command, DataValidation, DvKind, Note, SheetProtection};
    let mut notes = s.layout.notes.clone();
    if duck_core::notes::at(&notes, 1, 1).is_none() {
        notes.push(Note::new(
            1,
            1,
            "duck-bench",
            "nota <nova> & \"teste\"
segunda linha",
        ));
    }
    let mut rules = s.layout.validations.clone();
    let mut v = DataValidation {
        ranges: vec![CellRange { first_row: 1, first_col: 1, last_row: 9, last_col: 1 }],
        kind: DvKind::List,
        prompt: "Escolha <um> & \"só um\"".into(),
        ..Default::default()
    };
    v.set_literal_items(&["um".into(), "dois, ou três".into()]);
    rules.push(v);
    Command::Group(vec![
        Command::Notes(notes),
        Command::Validations(rules),
        Command::Protect(Some(Box::new(SheetProtection {
            password: duck_xlsx::protect::new_password("teste"),
            ..Default::default()
        }))),
    ])
    .apply(s);
}

/// Edições sintéticas na maior aba: números, texto, fórmulas, apagar, uma
/// célula além do fim e (se houver) a mestre de uma fórmula compartilhada.
fn edit(sheets: &mut [Sheet], styles: &mut duck_core::Styles, n: usize) -> (usize, usize) {
    let (i, _) = sheets
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| s.rows() as u64 * s.cols() as u64)
        .unwrap_or((0, &sheets[0]));
    let s = &mut sheets[i];
    let (rows, cols) = (s.rows().max(1), s.cols().max(1));
    for k in 0..n {
        let r = ((k as u64 * 7919) % rows as u64) as u32;
        let c = (k as u32 * 3) % cols;
        match k % 4 {
            0 => s.set_cell(r, c, CellValue::Number(k as f64 + 0.25), None, None),
            1 => s.set_cell(r, c, CellValue::Text(format!("editado {k} <&>")), None, None),
            2 => s.set_cell(r, c, CellValue::Number(1.0 + k as f64), Some(&format!("1+{k}")), None),
            _ => s.set_cell(r, c, CellValue::Empty, None, Some(0)),
        }
    }
    s.set_cell(rows + 2, 0, CellValue::Text("depois do fim".into()), None, None);
    // Estilo novo (data), largura e altura.
    let base = s.cell(0, 0, &duck_core::StringTable::new()).style;
    let date = styles.derive_xf(base, |x| x.num_fmt = 14);
    s.set_cell(rows + 3, 1, CellValue::Number(46348.0), None, Some(date));
    s.set_col_width(1, Some(18.5));
    s.set_row_height(rows + 3, Some(25.5));
    let master = s.formula_positions().take(2_000_000).find(|&(r, c)| {
        s.formula(r, c).is_some_and(|f| {
            matches!(f.kind, FormulaKind::Shared { .. }) && (f.anchor_row, f.anchor_col) == (r, c)
        })
    });
    let mut count = n + 1;
    if let Some((r, c)) = master {
        s.set_cell(r, c, CellValue::Number(42.0), None, None);
        count += 1;
    }
    s.settle();
    edit_rules(s);
    (i, count)
}
