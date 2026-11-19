//! `duck-bench`: mede a leitura de planilhas (ADR-012, F1).
//!
//! ```text
//! duck-bench open   [ARQUIVO] [--threads N] [--chunk-mb M]   lê e mostra tempo, memória e contagens
//! duck-bench sweep  [ARQUIVO] [--threads 1,2,4,...]          curva de ganho por threads (1 processo por rodada)
//! duck-bench verify [ARQUIVO]                                compara cada célula com o calamine
//! duck-bench gen    SAIDA.xlsx [--rows N] [--cols M]         gera planilha sintética
//! ```
//!
//! Sem ARQUIVO, usa `reference = "..."` de `bench.local.toml` (fora do git:
//! o arquivo de referência tem dados reais e nunca entra no repositório).

mod generate;
mod mem;
mod verify;

// Alocador com arenas por thread (ADR-012): menos contenção e menos memória
// presa depois que os pedaços de XML são liberados.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use duck_core::Workbook;
use duck_xlsx::{LoadReport, Options};
use mem::mb;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[args.len().min(1)..];
    let result = match cmd {
        "open" => cmd_open(rest),
        "sweep" => cmd_sweep(rest),
        "verify" => cmd_verify(rest),
        "gen" => cmd_gen(rest),
        _ => {
            eprintln!(
                "uso: duck-bench <open|sweep|verify|gen> [ARQUIVO] [opções]\n  veja o topo de crates/duck-bench/src/main.rs"
            );
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
}

type Res = Result<(), String>;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn positional(args: &[String]) -> Option<String> {
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = !matches!(a.as_str(), "--quiet" | "--machine");
            continue;
        }
        return Some(a.clone());
    }
    None
}

fn file_arg(args: &[String]) -> Result<PathBuf, String> {
    if let Some(p) = positional(args) {
        return Ok(PathBuf::from(p));
    }
    let cfg = std::fs::read_to_string("bench.local.toml")
        .map_err(|_| "informe o arquivo ou crie bench.local.toml com reference = \"caminho\"".to_string())?;
    cfg.lines()
        .find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == "reference").then(|| v.trim().trim_matches('"').to_string())
        })
        .map(PathBuf::from)
        .ok_or_else(|| "bench.local.toml sem a chave reference".to_string())
}

fn options(args: &[String]) -> Options {
    let mut o = Options::default();
    if let Some(mb) = flag(args, "--chunk-mb").and_then(|v| v.parse::<usize>().ok()) {
        o.chunk_bytes = mb * 1024 * 1024;
    }
    if let Some(n) = flag(args, "--in-flight").and_then(|v| v.parse::<usize>().ok()) {
        o.max_in_flight = n;
    }
    o
}

fn threads(args: &[String]) -> usize {
    flag(args, "--threads").and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn load(path: &Path, args: &[String]) -> Result<(Workbook, LoadReport), String> {
    let pool = duck_par::pool(threads(args));
    let opts = options(args);
    let r = pool.install(|| duck_xlsx::open(path, &opts)).map_err(|e| e.to_string());
    // Devolve ao SO a memória dos pedaços de XML já liberados: cada thread do
    // pool tem o próprio heap no mimalloc.
    pool.broadcast(|_| release_free_memory());
    release_free_memory();
    r
}

fn release_free_memory() {
    // SAFETY: função do mimalloc sem pré-condições; só compacta o heap da thread atual.
    unsafe { libmimalloc_sys::mi_collect(true) };
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------

fn cmd_open(args: &[String]) -> Res {
    let path = file_arg(args)?;
    let machine = args.iter().any(|a| a == "--machine");
    let file_size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    let (wb, rep) = load(&path, args)?;
    let m = mem::process();
    let cells: u64 = wb.sheets.iter().map(|s| s.counts().values()).sum();

    if machine {
        // Uma linha para o `sweep` ler: threads total_ms peak_bytes current_bytes cells
        println!("RESULT {} {:.3} {} {} {}", rep.threads, ms(rep.total), m.peak, m.current, cells);
        return Ok(());
    }

    println!("\nduck-bench open");
    println!("  arquivo: {:.1} MB no disco, {:.1} MB de XML nas abas", mb(file_size), mb(rep.xml_bytes()));
    println!("  threads: {}", rep.threads);
    println!("\nTempo total: {:.0} ms", ms(rep.total));
    println!("  pacote + workbook        {:>8.1} ms", ms(rep.package));
    println!(
        "  strings compartilhadas   {:>8.1} ms ({} strings)",
        ms(rep.shared_strings),
        wb.shared_strings.len()
    );
    println!(
        "  estilos                  {:>8.1} ms ({} estilos de célula)",
        ms(rep.styles),
        wb.styles.xfs.len()
    );

    let mut idx: Vec<usize> = (0..rep.sheets.len()).collect();
    idx.sort_by_key(|&i| std::cmp::Reverse(rep.sheets[i].xml_bytes));
    println!("\nAbas (maiores primeiro):");
    println!(
        "  {:>3} {:>9} {:>6} {:>9} {:>9} {:>9} {:>8} {:>11} {:>10}",
        "#", "XML MB", "pedaç.", "parede", "inflate", "parse*", "merge", "linhas×col", "valores"
    );
    for &i in idx.iter().take(8) {
        let s = &rep.sheets[i];
        let sh = &wb.sheets[i];
        println!(
            "  {:>3} {:>9.1} {:>6} {:>7.0}ms {:>7.0}ms {:>7.0}ms {:>6.0}ms {:>7}×{:<3} {:>10}{}",
            i,
            mb(s.xml_bytes),
            s.chunks,
            ms(s.wall),
            ms(s.inflate),
            ms(s.parse_cpu),
            ms(s.merge),
            sh.rows(),
            sh.cols(),
            sh.counts().values(),
            if s.sequential { "  (sequencial)" } else { "" }
        );
    }
    if idx.len() > 8 {
        println!("  ... e mais {} abas menores", idx.len() - 8);
    }
    println!("  * parse = soma do tempo de CPU de todos os pedaços (várias threads)");

    let mut total = duck_core::CellCounts::default();
    for s in &wb.sheets {
        total.add(&s.counts());
    }
    println!("\nCélulas:");
    println!("  com valor            {:>12}", total.values());
    println!("    números            {:>12}", total.numbers);
    println!("    strings compart.   {:>12}", total.shared_strings);
    println!("    strings locais     {:>12}", total.local_strings);
    println!("    booleanos / erros  {:>12} / {}", total.bools, total.errors);
    println!("  com fórmula          {:>12}", total.formulas);
    println!("  só estilo            {:>12}", total.styled_empty);

    println!("\nMemória:");
    println!("  dados do workbook    {:>9.1} MB", mb(wb.heap_bytes() as u64));
    println!("  processo agora       {:>9.1} MB", mb(m.current));
    println!("  pico do processo     {:>9.1} MB", mb(m.peak));
    Ok(())
}

fn cmd_sweep(args: &[String]) -> Res {
    let path = file_arg(args)?;
    let list: Vec<usize> = flag(args, "--threads")
        .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect())
        .unwrap_or_else(|| {
            let max = duck_par::available_threads();
            let mut v = vec![1, 2, 4, 8, 16, 32];
            v.retain(|&n| n <= max);
            if !v.contains(&max) {
                v.push(max);
            }
            v
        });
    let repeat: usize = flag(args, "--repeat").and_then(|v| v.parse().ok()).unwrap_or(2);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    println!("\nduck-bench sweep: {} rodadas por configuração (melhor tempo), 1 processo por rodada", repeat);
    println!("  {:>7} {:>10} {:>8} {:>10} {:>12}", "threads", "tempo", "ganho", "eficiência", "pico memória");
    let mut base = None;
    for &n in &list {
        let mut best: Option<(f64, u64)> = None;
        for _ in 0..repeat {
            let mut cmd = Command::new(&exe);
            cmd.arg("open").arg(&path).arg("--threads").arg(n.to_string()).arg("--machine");
            for f in ["--chunk-mb", "--in-flight"] {
                if let Some(v) = flag(args, f) {
                    cmd.arg(f).arg(v);
                }
            }
            let out = cmd.output().map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            let line = text.lines().find(|l| l.starts_with("RESULT")).ok_or_else(|| {
                format!("rodada com {n} threads falhou: {}", String::from_utf8_lossy(&out.stderr))
            })?;
            let f: Vec<&str> = line.split_whitespace().collect();
            let t: f64 = f[2].parse().unwrap_or(f64::MAX);
            let peak: u64 = f[3].parse().unwrap_or(0);
            if best.is_none_or(|b| t < b.0) {
                best = Some((t, peak));
            }
        }
        let (t, peak) = best.unwrap();
        let b = *base.get_or_insert(t);
        let speedup = b / t;
        println!(
            "  {:>7} {:>8.0}ms {:>7.2}× {:>9.0}% {:>9.0} MB",
            n,
            t,
            speedup,
            100.0 * speedup / n as f64,
            mb(peak)
        );
    }
    Ok(())
}

fn cmd_verify(args: &[String]) -> Res {
    let path = file_arg(args)?;
    let t = Instant::now();
    let (wb, rep) = load(&path, args)?;
    println!("duck-xlsx: {:.0} ms", ms(rep.total));
    let t2 = Instant::now();
    let out = verify::run(&path, &wb)?;
    println!("calamine + comparação: {:.0} ms", ms(t2.elapsed()));
    println!(
        "{} abas, {} células comparadas, {} divergências ({:.1} s no total)",
        out.sheets,
        out.compared,
        out.mismatches,
        t.elapsed().as_secs_f64()
    );
    for s in &out.samples {
        println!("  {s}");
    }
    if out.mismatches > 0 {
        return Err("o duck-xlsx leu algo diferente do calamine".into());
    }
    Ok(())
}

fn cmd_gen(args: &[String]) -> Res {
    let out = positional(args).ok_or("informe o arquivo de saída")?;
    let spec = generate::Spec {
        rows: flag(args, "--rows").and_then(|v| v.parse().ok()).unwrap_or(600_000),
        cols: flag(args, "--cols").and_then(|v| v.parse().ok()).unwrap_or(37),
        small_sheets: flag(args, "--small-sheets").and_then(|v| v.parse().ok()).unwrap_or(10),
    };
    let t = Instant::now();
    generate::write(Path::new(&out), &spec).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!(
        "{out}: {} linhas × {} colunas + {} abas pequenas, {:.1} MB, {:.1} s",
        spec.rows,
        spec.cols,
        spec.small_sheets,
        mb(size),
        t.elapsed().as_secs_f64()
    );
    Ok(())
}
