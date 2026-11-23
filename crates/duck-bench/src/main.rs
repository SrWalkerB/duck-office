//! `duck-bench`: mede a leitura de planilhas (ADR-012, F1).
//!
//! ```text
//! duck-bench open   [ARQUIVO] [--threads N] [--chunk-mb M]   lê e mostra tempo, memória e contagens
//! duck-bench sweep  [ARQUIVO] [--threads 1,2,4,...]          curva de ganho por threads (1 processo por rodada)
//! duck-bench verify [ARQUIVO]                                compara cada célula com o calamine
//! duck-bench gen    SAIDA.xlsx [--rows N] [--cols M]         gera planilha sintética
//! duck-bench preview [ARQUIVO] [--mb N]                      mede o caminho da primeira tela (F2)
//! duck-bench roundtrip [ARQUIVO] [--rewrite-all] [--edit N] [--estrutura] [--abas] [--celulas] [--level L] [--keep]  salva numa cópia e confere (F3)
//! duck-bench gen-showcase SAIDA.xlsx                         planilha de vitrine (formatos, bordas, painéis)
//! duck-bench calc [ARQUIVO] [--hoje AAAA-MM-DD]              calcula cada fórmula e compara com o Excel (F4)
//! ```
//!
//! Sem ARQUIVO, usa `reference = "..."` de `bench.local.toml` (fora do git:
//! o arquivo de referência tem dados reais e nunca entra no repositório).

mod calc;
mod emf;
mod generate;
mod lint;
mod mem;
mod olap;
mod regress;
mod roundtrip;
mod showcase;
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

/// Opções do mimalloc (v3, a versão que o `libmimalloc-sys` compila). Medidas
/// no arquivo de referência (ver docs/ARCHITECTURE.md, status da F2):
/// - `arena_eager_commit = 0`: o padrão compromete a arena inteira de saída em
///   sistemas com overcommit (Linux), inflando o RSS em ~250 MB;
/// - `purge_delay = 1 ms`: devolve logo ao SO a memória liberada (o padrão,
///   10 ms, deixa o pico da leitura passar de 1 GB).
const MI_OPTION_ARENA_EAGER_COMMIT: libmimalloc_sys::mi_option_t = 4;
const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;

extern "C" fn configure_allocator() {
    // SAFETY: só ajusta opções do mimalloc; roda antes de qualquer outra thread.
    unsafe {
        libmimalloc_sys::mi_option_set(MI_OPTION_ARENA_EAGER_COMMIT, 0);
        libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, 1);
    }
}

// O runtime do Rust já aloca antes do `main`, e o mimalloc lê
// `arena_eager_commit` ao criar a primeira arena: a opção precisa ser
// definida antes disso. No Linux, um construtor em `.init_array` roda antes do
// runtime. (No Windows a opção não muda nada: lá não há overcommit.)
#[cfg(target_os = "linux")]
#[used]
#[unsafe(link_section = ".init_array")]
static CONFIGURE_ALLOCATOR: extern "C" fn() = configure_allocator;

fn main() {
    configure_allocator();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[args.len().min(1)..];
    let result = match cmd {
        "open" => cmd_open(rest),
        "sweep" => cmd_sweep(rest),
        "verify" => cmd_verify(rest),
        "gen" => cmd_gen(rest),
        "formula-stats" => cmd_formula_stats(rest),
        "inflate" => cmd_inflate(rest),
        "parse" => cmd_parse(rest),
        "preview" => cmd_preview(rest),
        "roundtrip" => roundtrip::cmd(rest),
        "conferir-pacote" => match rest.first() {
            Some(p) => {
                let bad = lint::check(Path::new(p));
                bad.iter().for_each(|b| println!("{b}"));
                if bad.is_empty() { Ok(()) } else { Err(format!("{} problemas no pacote", bad.len())) }
            }
            None => Err("uso: duck-bench conferir-pacote ARQUIVO.xlsx".to_string()),
        },
        "regress" => regress::cmd(rest),
        "calc" => calc::cmd(rest),
        "objetos" => calc::cmd_objects(rest),
        "emf" => emf::cmd_emf(rest),
        "dinamicas" => calc::cmd_pivots(rest),
        "modelo" => olap::cmd(rest),
        "grupos" => calc::cmd_pivot_groups(rest),
        "celula" => calc::cmd_cell(rest),
        "gen-showcase" => match rest.first() {
            Some(p) => showcase::write(Path::new(p))
                .map_err(|e| e.to_string())
                .map(|_| println!("vitrine salva em {p}")),
            None => Err("uso: duck-bench gen-showcase SAIDA.xlsx".to_string()),
        },
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
            skip = !matches!(
                a.as_str(),
                "--quiet"
                    | "--machine"
                    | "--mem-trace"
                    | "--rewrite-all"
                    | "--keep"
                    | "--estrutura"
                    | "--abas"
                    | "--celulas"
                    | "--formas-valor"
                    | "--recalc"
                    | "--detalhes"
            );
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
    if let Some(list) = flag(args, "--sheets") {
        o.only_sheets = Some(list.split(',').filter_map(|v| v.trim().parse().ok()).collect());
    }
    if let Some(n) = flag(args, "--lookahead").and_then(|v| v.parse::<usize>().ok()) {
        o.inflate_lookahead = n;
    }
    if let Some(n) = flag(args, "--pieces").and_then(|v| v.parse::<usize>().ok()) {
        o.inflate_pieces = n;
    }
    if let Some(kb) = flag(args, "--part-kb").and_then(|v| v.parse::<usize>().ok()) {
        o.inflate_part_bytes = kb * 1024;
    }
    if let Some(kb) = flag(args, "--stream-kb").and_then(|v| v.parse::<usize>().ok()) {
        o.inflate_stream_bytes = kb * 1024;
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
    // `--mem-trace`: amostra a memória do processo a cada 10 ms durante a leitura.
    let trace = args.iter().any(|a| a == "--mem-trace");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sampler = trace.then(|| {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let mut v = Vec::new();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                v.push((t.elapsed().as_millis(), mem::process().current));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            v
        })
    });
    let cpu0 = mem::cpu_seconds();
    let (wb, rep) = load(&path, args)?;
    let cpu = mem::cpu_seconds() - cpu0;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(h) = sampler {
        let v = h.join().unwrap_or_default();
        println!("memória durante a leitura (ms → MB):");
        for (t, b) in v.iter().step_by((v.len() / 25).max(1)) {
            println!("  {t:>5} ms  {:>6.0} MB  {}", mb(*b), "#".repeat((mb(*b) / 40.0) as usize));
        }
    }
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
    println!(
        "\nTempo total: {:.0} ms (CPU: {:.2} s, ~{:.1} núcleos ocupados em média)",
        ms(rep.total),
        cpu,
        cpu / rep.total.as_secs_f64()
    );
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
    for (i, s) in rep.sheets.iter().enumerate() {
        if let Some(r) = s.inflate_report {
            println!(
                "  inflate paralelo da aba {i}: {} partes ({} especuladas, {} já resolvidas no pool); entrega: espera {:.0} ms, marcadores {:.0} ms, parse/vagas {:.0} ms, sequencial {:.0} ms",
                r.parts,
                r.speculative,
                r.pre_resolved,
                ms(r.wait),
                ms(r.resolve),
                ms(r.sink),
                ms(r.sequential_time)
            );
        }
    }

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
    let texts: usize = wb.sheets.iter().map(|s| s.formula_texts_stored()).sum();
    println!("    entradas guardadas {:>12} (modelos deduplicados + mestres de grupos)", texts);
    println!("  só estilo            {:>12}", total.styled_empty);
    let lay = |f: fn(&duck_core::Layout) -> usize| wb.sheets.iter().map(|s| f(&s.layout)).sum::<usize>();
    println!(
        "
Layout:"
    );
    println!("  colunas com largura  {:>12}", lay(|l| l.cols.len()));
    println!("  linhas com altura    {:>12}", lay(|l| l.rows.len()));
    println!("  células mescladas    {:>12}", lay(|l| l.merges.len()));
    println!(
        "  abas com painel congelado {:>7}",
        wb.sheets.iter().filter(|s| s.layout.pane.is_some()).count()
    );

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
            for f in ["--chunk-mb", "--in-flight", "--lookahead", "--pieces", "--sheets", "--part-kb"] {
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
    println!("{} fórmulas comparadas, {} divergências", out.formulas_compared, out.formula_mismatches);
    println!("{} mesclagens comparadas, {} divergências", out.merges_compared, out.merge_mismatches);
    for s in &out.samples {
        println!("  {s}");
    }
    if out.mismatches > 0 || out.formula_mismatches > 0 || out.merge_mismatches > 0 {
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

/// Diagnóstico: quantas fórmulas viram modelo relativo e quão repetidos são.
/// Mostra só a estrutura (textos literais escondidos).
fn cmd_formula_stats(args: &[String]) -> Res {
    use std::collections::HashMap;
    let path = file_arg(args)?;
    let (wb, _) = load(&path, args)?;
    let sheet = wb.sheets.iter().max_by_key(|s| s.formula_cells()).ok_or("sem abas")?;
    let mut by_template: HashMap<String, u64> = HashMap::new();
    let (mut total, mut templ, mut shared, mut raw) = (0u64, 0u64, 0u64, 0u64);
    let mut raw_samples = Vec::new();
    for (r, c) in sheet.formula_positions() {
        let Some(f) = sheet.formula(r, c) else { continue };
        total += 1;
        if f.template {
            templ += 1;
            *by_template.entry(f.text.to_string()).or_default() += 1;
        } else if matches!(f.kind, duck_core::FormulaKind::Shared { .. }) {
            shared += 1;
        } else {
            raw += 1;
            if raw_samples.len() < 5 {
                raw_samples.push(mask_strings(f.text));
            }
        }
    }
    println!("fórmulas na maior aba: {total}");
    println!("  modelos relativos: {templ} células, {} modelos distintos", by_template.len());
    println!("  compartilhadas:    {shared}");
    println!("  texto cru:         {raw}");
    let mut top: Vec<_> = by_template.into_iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.1));
    println!("modelos mais usados:");
    for (t, n) in top.iter().take(5) {
        println!(
            "  {n:>9}× {}",
            mask_strings(
                &t.replace(['\u{F000}', '\u{F001}', '\u{F002}'], "⟨")
                    .replace('\u{F003}', ",")
                    .replace('\u{F004}', "⟩")
            )
        );
    }
    println!("cauda: {} modelos usados só 1 vez", top.iter().filter(|t| t.1 == 1).count());
    for s in raw_samples {
        println!("  cru: {s}");
    }
    Ok(())
}

fn mask_strings(f: &str) -> String {
    let mut out = String::new();
    let mut in_str = false;
    for ch in f.chars() {
        if ch == '"' {
            in_str = !in_str;
            out.push(ch);
        } else if in_str {
            if !out.ends_with('…') {
                out.push('…');
            }
        } else {
            out.push(ch);
        }
    }
    out.chars().take(160).collect()
}

/// Descompacta a maior parte do zip com o zlib-rs (sequencial) e com o
/// `duck-deflate` em paralelo, e compara bytes, CRC e tempo.
fn cmd_inflate(args: &[String]) -> Res {
    use std::io::Read;
    let path = file_arg(args)?;
    let pkg = duck_xlsx::Package::open(&path).map_err(|e| e.to_string())?;
    let e = pkg
        .entries()
        .filter(|e| e.is_deflated())
        .max_by_key(|e| e.size)
        .ok_or("zip sem partes compactadas")?
        .clone();
    let data = pkg.compressed(&e);
    println!("maior parte: {:.1} MB comprimidos → {:.1} MB", mb(data.len() as u64), mb(e.size));

    let t = Instant::now();
    let mut reference = Vec::with_capacity(e.size as usize);
    flate2::read::DeflateDecoder::new(data).read_to_end(&mut reference).map_err(|e| e.to_string())?;
    let t_zlib = t.elapsed();
    println!(
        "  zlib-rs (1 thread)          {:>7.0} ms  ({:.0} MB/s)",
        ms(t_zlib),
        mb(e.size) / t_zlib.as_secs_f64()
    );

    let t = Instant::now();
    let ours = duck_deflate::inflate(data).map_err(|e| e.to_string())?;
    let t_seq = t.elapsed();
    println!(
        "  duck-deflate (1 thread)     {:>7.0} ms  ({:.0} MB/s)",
        ms(t_seq),
        mb(e.size) / t_seq.as_secs_f64()
    );
    if ours != reference {
        return Err("duck-deflate sequencial difere do zlib-rs".into());
    }
    drop(ours);

    let part_mb: usize = flag(args, "--part-mb").and_then(|v| v.parse().ok()).unwrap_or(1);
    let lookahead: usize = flag(args, "--lookahead").and_then(|v| v.parse().ok()).unwrap_or(16);
    let pool = duck_par::pool(threads(args));
    let mut pos = 0usize;
    let mut same = true;
    let t = Instant::now();
    let rep = pool
        .install(|| {
            duck_deflate::inflate_parallel(
                data,
                Some(e.crc32),
                duck_deflate::Options { part_bytes: part_mb << 20, lookahead, ..Default::default() },
                |b| {
                    let piece = b.bytes();
                    same &= reference.get(pos..pos + piece.len()) == Some(piece);
                    pos += piece.len();
                    Ok(())
                },
            )
        })
        .map_err(|e| e.to_string())?;
    let t_par = t.elapsed();
    println!(
        "  duck-deflate paralelo ({} thr) {:>5.0} ms  ({:.0} MB/s) — {:.1}× o zlib-rs",
        pool.current_num_threads(),
        ms(t_par),
        mb(e.size) / t_par.as_secs_f64(),
        t_zlib.as_secs_f64() / t_par.as_secs_f64()
    );
    println!(
        "  partes: {} ({} especuladas, {} em sequência: {} sem bloco, {} desalinhadas, {} sem janela; {} puladas)",
        rep.parts, rep.speculative, rep.sequential, rep.no_block, rep.misaligned, rep.unresolved, rep.skipped
    );
    println!("  saída resolvida em série (fase de marcadores): {:.1} MB", mb(rep.head_bytes));
    let [f, h, t, c] = rep.spec_times;
    println!(
        "  especulações (soma): busca {:.0} ms, fase 16 bits {:.0} ms, bytes {:.0} ms, CRC {:.0} ms",
        ms(f),
        ms(h),
        ms(t),
        ms(c)
    );
    println!(
        "  CRC32 confere e bytes {} ao zlib-rs",
        if same && pos == reference.len() { "idênticos" } else { "DIFERENTES" }
    );
    if !(same && pos == reference.len()) {
        return Err("saída paralela difere".into());
    }
    // Sem a comparação (que é serial): só o custo de descompactar e entregar.
    let mut best = f64::MAX;
    let mut best_cpu = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        let c0 = mem::cpu_seconds();
        let mut total = 0usize;
        pool.install(|| {
            duck_deflate::inflate_parallel(
                data,
                Some(e.crc32),
                duck_deflate::Options { part_bytes: part_mb << 20, lookahead, ..Default::default() },
                |b| {
                    total += b.len();
                    Ok(())
                },
            )
        })
        .map_err(|e| e.to_string())?;
        best = best.min(ms(t.elapsed()));
        best_cpu = best_cpu.min(mem::cpu_seconds() - c0);
    }
    println!(
        "  só descompactar (melhor de 3): {best:.0} ms — {:.1}× o zlib-rs (CPU {:.2} s; zlib-rs: {:.2} s)",
        ms(t_zlib) / best,
        best_cpu,
        t_zlib.as_secs_f64()
    );
    Ok(())
}

/// Mede só o parse da maior aba, numa thread: descompacta uma vez e cronometra
/// o leitor de linhas sobre o XML em memória.
fn cmd_parse(args: &[String]) -> Res {
    use std::io::Read;
    let path = file_arg(args)?;
    let pkg = duck_xlsx::Package::open(&path).map_err(|e| e.to_string())?;
    let e = pkg
        .entries()
        .filter(|e| e.name.starts_with("xl/worksheets/"))
        .max_by_key(|e| e.size)
        .ok_or("sem abas")?
        .clone();
    let mut xml = Vec::with_capacity(e.size as usize);
    flate2::read::DeflateDecoder::new(pkg.compressed(&e)).read_to_end(&mut xml).map_err(|e| e.to_string())?;
    let repeat: usize = flag(args, "--repeat").and_then(|v| v.parse().ok()).unwrap_or(3);
    let mut best = f64::MAX;
    let mut cells = 0u64;
    for _ in 0..repeat {
        let t = Instant::now();
        let sheet = duck_xlsx::parse_sheet_xml(&xml);
        best = best.min(t.elapsed().as_secs_f64());
        let c = sheet.counts();
        cells = c.values() + c.styled_empty;
    }
    println!(
        "parse de {:.0} MB de XML, 1 thread: {:.0} ms ({:.0} MB/s, {:.0} ns por célula, {} células)",
        mb(xml.len() as u64),
        best * 1000.0,
        mb(xml.len() as u64) / best,
        best * 1e9 / cells as f64,
        cells
    );
    Ok(())
}

/// O que o duck-sheets faz antes da primeira tela: abrir o pacote, ler estilos
/// e strings e uma prévia da aba ativa (os primeiros N MB de XML).
fn cmd_preview(args: &[String]) -> Res {
    let path = file_arg(args)?;
    let mb: usize = flag(args, "--mb").and_then(|v| v.parse().ok()).unwrap_or(4);
    let t0 = Instant::now();
    let book = duck_xlsx::Book::open(&path).map_err(|e| e.to_string())?;
    let t_open = t0.elapsed();
    let active = book.active_sheet();
    let (styles, (strings, preview)) = rayon::join(
        || {
            let t = Instant::now();
            (book.read_styles(), t.elapsed())
        },
        || {
            rayon::join(
                || {
                    let t = Instant::now();
                    (book.read_shared_strings(), t.elapsed())
                },
                || {
                    let t = Instant::now();
                    (book.preview_sheet(active, mb << 20), t.elapsed())
                },
            )
        },
    );
    let strings = (strings.0.map_err(|e| e.to_string())?, strings.1);
    let total = t0.elapsed();
    let st = styles.0.map_err(|e| e.to_string())?;
    let sh = preview.0.map_err(|e| e.to_string())?;
    println!(
        "
duck-bench preview ({mb} MB de XML da aba ativa)"
    );
    println!("  pacote + workbook   {:>7.1} ms", t_open.as_secs_f64() * 1000.0);
    println!("  estilos + tema      {:>7.1} ms ({} estilos)", styles.1.as_secs_f64() * 1000.0, st.xfs.len());
    println!(
        "  strings             {:>7.1} ms ({} strings)",
        strings.1.as_secs_f64() * 1000.0,
        strings.0.len()
    );
    println!(
        "  prévia da aba #{active:<4} {:>7.1} ms ({} linhas, {} valores)",
        preview.1.as_secs_f64() * 1000.0,
        sh.rows(),
        sh.counts().values()
    );
    println!("  total (paralelo)    {:>7.1} ms", total.as_secs_f64() * 1000.0);
    Ok(())
}
