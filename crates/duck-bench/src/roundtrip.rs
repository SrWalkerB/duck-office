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
        let (sheet, ms) = restructure(&mut wb.sheets);
        println!("linhas e colunas inseridas/apagadas na aba {} em {ms:.0} ms", sheet + 1);
    }

    let opts = SaveOptions { level, rewrite_all, ..Default::default() };
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

    let result = check(&src, &out, &saved, args);
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
    let (mut same, mut changed, mut missing) = (0, Vec::new(), Vec::new());
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
    if changed.len() > saved.parts_rewritten {
        return Err(format!(
            "{} partes mudaram, mas {} foram regravadas",
            changed.len(),
            saved.parts_rewritten
        ));
    }
    if missing.len() != usize::from(saved.calc_chain_removed) {
        return Err("partes sumiram na cópia".into());
    }
    drop((a, b));

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

/// Insere e apaga linhas e colunas na maior aba (como o app faz).
fn restructure(sheets: &mut [Sheet]) -> (usize, f64) {
    use duck_core::{Axis, Command, OpsHandle, Shift};
    let (i, _) = sheets
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| s.rows() as u64 * s.cols() as u64)
        .unwrap_or((0, &sheets[0]));
    let s = &mut sheets[i];
    let name = s.name.clone();
    let t = Instant::now();
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
    }
    (i, t.elapsed().as_secs_f64() * 1000.0)
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
    (i, count)
}
