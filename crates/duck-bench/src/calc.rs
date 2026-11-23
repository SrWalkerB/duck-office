//! `duck-bench calc [ARQUIVO] [--hoje AAAA-MM-DD] [--exemplos N]`: calcula
//! cada fórmula do arquivo com os valores que o Excel guardou e compara o
//! resultado com o valor guardado (F4).
//!
//! É a conferência do motor: cada fórmula vira um teste. O relatório mostra
//! só contagens, nomes de função, tipos de diferença e posições, nunca o
//! conteúdo das células (o arquivo de referência tem dados reais).

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use duck_calc::parse::Node;
use duck_calc::{Book, Engine, funcs};
use duck_core::{CellValue, Sheet, Value};
use duck_xlsx::Options;
use rayon::prelude::*;

use crate::Res;

#[derive(Default)]
struct Stats {
    total: u64,
    equal: u64,
    opaque: u64,
    /// (função mais externa, tipo da diferença) → quantidade.
    diffs: HashMap<(String, String), u64>,
    /// Função usada em fórmulas que divergem → quantidade.
    by_func: HashMap<String, (u64, u64)>,
    examples: Vec<(u32, u32, u32, String, String)>,
}

impl Stats {
    fn merge(mut self, o: Stats) -> Stats {
        self.total += o.total;
        self.equal += o.equal;
        self.opaque += o.opaque;
        for (k, v) in o.diffs {
            *self.diffs.entry(k).or_default() += v;
        }
        for (k, (a, b)) in o.by_func {
            let e = self.by_func.entry(k).or_default();
            e.0 += a;
            e.1 += b;
        }
        self.examples.extend(o.examples);
        self
    }
}

fn kind(v: &CellValue) -> &'static str {
    match v {
        CellValue::Empty => "vazio",
        CellValue::Number(_) => "número",
        CellValue::Text(_) => "texto",
        CellValue::Bool(_) => "lógico",
        CellValue::Error(_) => "erro",
    }
}

fn cached(v: Value) -> CellValue {
    match v {
        Value::Empty => CellValue::Empty,
        Value::Number(x) => CellValue::Number(x),
        Value::Text(s) => CellValue::Text(s.to_string()),
        Value::Bool(b) => CellValue::Bool(b),
        Value::Error(e) => CellValue::Error(e),
    }
}

/// Tipo da diferença (sem mostrar os valores).
fn diff_kind(want: &CellValue, got: &CellValue) -> Option<String> {
    match (want, got) {
        (CellValue::Number(a), CellValue::Number(b)) => {
            if a == b || duck_calc::index::round15(*a) == duck_calc::index::round15(*b) {
                return None;
            }
            let rel = (a - b).abs() / a.abs().max(b.abs()).max(1e-300);
            if rel < 1e-12 {
                return None;
            }
            Some(if rel < 1e-6 {
                "número: diferença pequena".into()
            } else if (a - b).abs() == 1.0 {
                "número: diferença de 1".into()
            } else {
                "número: diferente".into()
            })
        }
        (CellValue::Text(a), CellValue::Text(b)) => {
            if a == b {
                None
            } else if a.to_lowercase() == b.to_lowercase() {
                Some("texto: só maiúsculas".into())
            } else if a.trim() == b.trim() {
                Some("texto: espaços".into())
            } else if a.chars().count() == b.chars().count() {
                Some("texto: mesmo tamanho".into())
            } else {
                Some("texto: diferente".into())
            }
        }
        (CellValue::Bool(a), CellValue::Bool(b)) => (a != b).then(|| "lógico: diferente".into()),
        (CellValue::Error(a), CellValue::Error(b)) => {
            (a != b).then(|| format!("erro: esperado {} obtido {}", a.as_str(), b.as_str()))
        }
        (CellValue::Empty, CellValue::Text(s)) if s.is_empty() => None,
        (CellValue::Empty, CellValue::Number(x)) if *x == 0.0 => None,
        (CellValue::Text(s), CellValue::Number(x)) if s.is_empty() && *x == 0.0 => {
            Some("tipo: esperado texto vazio, obtido 0".into())
        }
        (CellValue::Error(e), g) => Some(format!("tipo: esperado {} obtido {}", e.as_str(), kind(g))),
        (w, CellValue::Error(e)) => Some(format!("tipo: esperado {} obtido {}", kind(w), e.as_str())),
        (w, g) => Some(format!("tipo: esperado {} obtido {}", kind(w), kind(g))),
    }
}

fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    c += 1;
    while c > 0 {
        s.push(b'A' + ((c - 1) % 26) as u8);
        c = (c - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

pub fn cmd(args: &[String]) -> Res {
    let path = crate::file_arg(args)?;
    let max_examples: usize = crate::flag(args, "--exemplos").and_then(|v| v.parse().ok()).unwrap_or(10);
    // `--tipo TEXTO`: só exemplos cujo tipo de diferença contém TEXTO.
    let filter = crate::flag(args, "--tipo");
    let show_values = args.iter().any(|a| a == "--formas-valor");
    let t0 = Instant::now();
    let (wb, _) = duck_xlsx::open(&path, &Options::default()).map_err(|e| e.to_string())?;
    let load = t0.elapsed();

    let sheets: Vec<&Sheet> = wb.sheets.iter().collect();
    let mut book = Book::new(sheets, &wb.shared_strings, &wb.extras, wb.date1904);
    if let Some(d) = crate::flag(args, "--hoje") {
        let p: Vec<i32> = d.split('-').filter_map(|x| x.parse().ok()).collect();
        if p.len() == 3 {
            book.set_now(duck_core::input::serial(p[0], p[1] as u32, p[2] as u32) + 0.5);
        }
    } else if let Some(t) = wb.extras.saved_at {
        // O arquivo guarda a hora em UTC; o Brasil está 3 horas atrás.
        book.set_now(t - 3.0 / 24.0);
    }
    println!(
        "arquivo lido em {:.2} s; {} abas, {} nomes definidos, {} arquivos externos",
        load.as_secs_f64(),
        wb.sheets.len(),
        wb.extras.names.len(),
        wb.extras.externals.len()
    );

    // Posições das fórmulas.
    let mut cells: Vec<(u32, u32, u32)> = Vec::new();
    for (s, sh) in wb.sheets.iter().enumerate() {
        cells.extend(sh.formula_positions().map(|(r, c)| (s as u32, r, c)));
    }
    let eng = Engine::new(&book);

    // Compilação (uma por definição) separada da avaliação para medir.
    let t1 = Instant::now();
    cells.par_chunks(4096).for_each(|ch| {
        for &(s, r, c) in ch {
            let _ = eng.formula(&book, s, r, c);
        }
    });
    let compile = t1.elapsed();

    let t2 = Instant::now();
    let stats =
        cells
            .par_chunks(2048)
            .map(|ch| {
                let mut st = Stats::default();
                for &(s, r, c) in ch {
                    st.total += 1;
                    let Some(got) = eng.eval_cell(&book, s, r, c) else {
                        st.opaque += 1;
                        continue;
                    };
                    let want = cached(wb.sheets[s as usize].cell(r, c, &wb.shared_strings).value);
                    let Some(kind) = diff_kind(&want, &got) else {
                        st.equal += 1;
                        continue;
                    };
                    let f = eng.formula(&book, s, r, c).unwrap();
                    let outer = match f.node(f.root) {
                        Node::Call { f: id, .. } => funcs::def(*id).name.to_string(),
                        _ => "(expressão)".to_string(),
                    };
                    *st.diffs.entry((outer, kind.clone())).or_default() += 1;
                    let mut seen: Vec<&str> = Vec::new();
                    for n in &f.nodes {
                        if let Node::Call { f: id, .. } = n {
                            let name = funcs::def(*id).name;
                            if !seen.contains(&name) {
                                seen.push(name);
                            }
                        }
                    }
                    for name in seen {
                        st.by_func.entry(name.to_string()).or_default().0 += 1;
                    }
                    if st.examples.len() < max_examples && filter.as_deref().is_none_or(|f| kind.contains(f))
                    {
                        let text = duck_calc::cell_formula(&wb.sheets[s as usize], r, c).unwrap_or_default();
                        let mut shape = formula_shape(&text);
                        if show_values {
                            shape.push_str(&format!(
                                "   [guardado: {}  calculado: {}]",
                                value_shape(&want),
                                value_shape(&got)
                            ));
                            // Formas dos valores das células simples citadas.
                            for n in &f.nodes {
                                if let Node::Area(a) = n
                                    && a.r0 == a.r1
                                    && a.c0 == a.c1
                                    && let duck_calc::parse::SheetSel::This
                                    | duck_calc::parse::SheetSel::Local(_) = a.sheet
                                {
                                    let sh = match a.sheet {
                                        duck_calc::parse::SheetSel::Local(i) => i,
                                        _ => s,
                                    };
                                    let (rr, cc) = (a.r0.at(r), a.c0.at(c));
                                    if rr >= 0
                                        && cc >= 0
                                        && let Some(sheet) = wb.sheets.get(sh as usize)
                                    {
                                        let v = cached(
                                            sheet.cell(rr as u32, cc as u32, &wb.shared_strings).value,
                                        );
                                        shape.push_str(&format!(
                                            " {}{}={}",
                                            col_name(cc as u32),
                                            rr + 1,
                                            value_shape(&v)
                                        ));
                                    }
                                }
                            }
                        }
                        st.examples.push((s, r, c, kind, shape));
                    }
                }
                st
            })
            .reduce(Stats::default, Stats::merge);
    let eval = t2.elapsed();

    if args.iter().any(|a| a == "--recalc") {
        recalc(&book, &eng, &wb);
    }
    let checked = stats.total - stats.opaque;
    println!(
        "{} fórmulas: {} calculadas, {} iguais ao Excel ({:.3}%), {} com o valor guardado mantido (função não suportada)",
        stats.total,
        checked,
        stats.equal,
        100.0 * stats.equal as f64 / checked.max(1) as f64,
        stats.opaque
    );
    println!(
        "compilação {:.0} ms ({} definições), cálculo {:.0} ms ({:.1} M fórmulas/s, {} threads), {} índices de busca",
        compile.as_secs_f64() * 1000.0,
        wb.sheets.iter().map(|s| s.formula_texts_stored()).sum::<usize>(),
        eval.as_secs_f64() * 1000.0,
        checked as f64 / eval.as_secs_f64() / 1e6,
        rayon::current_num_threads(),
        eng.index.len()
    );
    if !stats.diffs.is_empty() {
        println!("\ndiferenças (função mais externa, tipo):");
        let sorted: BTreeMap<_, _> = stats.diffs.iter().map(|(k, v)| (std::cmp::Reverse(*v), k)).collect();
        for (std::cmp::Reverse(n), (f, k)) in sorted.iter().take(40) {
            println!("{n:>10}  {f:<14} {k}");
        }
        println!("\nfunções presentes nas fórmulas que divergem:");
        let mut fs: Vec<_> = stats.by_func.iter().collect();
        fs.sort_by_key(|a| std::cmp::Reverse(a.1.0));
        for (f, (n, _)) in fs.iter().take(25) {
            println!("{n:>10}  {f}");
        }
        println!("\nexemplos (aba, célula):");
        for (s, r, c, k, shape) in stats.examples.iter().take(max_examples) {
            println!(
                "  aba {} {}{}  {k}
      {shape}",
                s + 1,
                col_name(*c),
                r + 1
            );
        }
    }
    Ok(())
}

/// Forma de uma fórmula sem dados: funções e referências A1 ficam; textos,
/// nomes de aba, de tabela e de coluna viram marcadores.
fn formula_shape(f: &str) -> String {
    let b: Vec<char> = f.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            '"' => {
                let mut j = i + 1;
                let mut n = 0;
                while j < b.len() {
                    if b[j] == '"' {
                        if b.get(j + 1) == Some(&'"') {
                            j += 2;
                            n += 1;
                            continue;
                        }
                        break;
                    }
                    j += 1;
                    n += 1;
                }
                out.push_str(&format!(
                    "\"{}\"",
                    value_shape_str(&b[i + 1..j.min(b.len())].iter().collect::<String>(), n)
                ));
                i = j + 1;
            }
            '\'' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != '\'' {
                    j += 1;
                }
                let inner: String = b[i + 1..j.min(b.len())].iter().collect();
                if let Some(rest) = inner.strip_prefix('[') {
                    let book: String = rest.chars().take_while(|c| *c != ']').collect();
                    out.push_str(&format!("'[{book}]ABA'"));
                } else {
                    out.push_str("'ABA'");
                }
                i = j + 1;
            }
            '[' => {
                // Conteúdo de referência estruturada / arquivo externo.
                let mut depth = 0;
                let mut j = i;
                while j < b.len() {
                    match b[j] {
                        '[' => {
                            depth += 1;
                            out.push('[');
                        }
                        ']' => {
                            depth -= 1;
                            out.push(']');
                            if depth == 0 {
                                break;
                            }
                        }
                        '\'' => {
                            j += 1;
                        }
                        ch if ch == '#'
                            || ch == '@'
                            || ch == ','
                            || ch == ':'
                            || ch == ' '
                            || ch.is_ascii_digit() =>
                        {
                            // Palavras-chave (#This Row) ficam.
                            if ch == '#' {
                                let mut k = j;
                                while k < b.len() && b[k] != ']' && b[k] != ',' {
                                    out.push(b[k]);
                                    k += 1;
                                }
                                j = k;
                                continue;
                            }
                            out.push(ch);
                        }
                        _ => {
                            if !out.ends_with('c') {
                                out.push('c');
                            }
                        }
                    }
                    j += 1;
                }
                i = j + 1;
            }
            c if c.is_alphanumeric() || c == '_' || c == '$' || c == '.' || c == '\\' => {
                let mut j = i;
                while j < b.len() && (b[j].is_alphanumeric() || matches!(b[j], '_' | '$' | '.' | '\\' | '?'))
                {
                    j += 1;
                }
                let w: String = b[i..j].iter().collect();
                let next = b.get(j).copied();
                if next == Some('(') {
                    out.push_str(&w);
                } else if next == Some('!') || next == Some('[') {
                    out.push_str(if next == Some('!') { "ABA" } else { "TAB" });
                } else if duck_calc::parse::parse_cell(&w).is_some()
                    || w.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '$')
                    || w.trim_start_matches('$').chars().all(|c| c.is_ascii_uppercase()) && w.len() <= 4
                    || matches!(w.as_str(), "TRUE" | "FALSE")
                {
                    out.push_str(&w);
                } else {
                    out.push_str("NOME");
                }
                i = j;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn value_shape_str(s: &str, max: usize) -> String {
    s.chars()
        .take(max.min(24))
        .map(|c| {
            if c.is_ascii_digit() {
                '9'
            } else if c.is_alphabetic() {
                'a'
            } else {
                c
            }
        })
        .collect()
}

fn value_shape(v: &CellValue) -> String {
    match v {
        CellValue::Empty => "(vazio)".into(),
        CellValue::Number(x) => format!("número{}", if x.fract() != 0.0 { " fracionário" } else { "" }),
        CellValue::Text(s) => format!("\"{}\"", value_shape_str(s, 24)),
        CellValue::Bool(_) => "lógico".into(),
        CellValue::Error(e) => e.as_str().into(),
    }
}

/// Recálculo completo pela ordem das dependências e recálculo dos voláteis.
fn recalc(book: &Book, eng: &Engine, wb: &duck_core::Workbook) {
    use std::sync::atomic::AtomicBool;
    let t = Instant::now();
    let g = duck_calc::Graph::build(book, eng);
    let build = t.elapsed();
    println!(
        "grafo: {} blocos ({} sequenciais), {} nós, {} arestas, montado em {:.0} ms",
        g.runs.len(),
        g.runs.iter().filter(|r| r.seq).count(),
        g.nodes(),
        g.edges(),
        build.as_secs_f64() * 1000.0
    );
    let cancel = AtomicBool::new(false);
    for (name, dirty) in [("completo", duck_calc::Dirty::All), ("voláteis", duck_calc::Dirty::Volatile)] {
        eng.index.clear();
        let t = Instant::now();
        let out = g.recalc(book, eng, &dirty, &cancel);
        println!(
            "recálculo {name}: {} células em {} níveis, {:.0} ms; {} valores mudaram",
            out.cells,
            out.levels,
            t.elapsed().as_secs_f64() * 1000.0,
            out.overlay.len()
        );
        let mut by_sheet: BTreeMap<u32, u64> = BTreeMap::new();
        let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
        for (s, r, c, v) in out.overlay.drain() {
            *by_sheet.entry(s).or_default() += 1;
            let old = cached(wb.sheets[s as usize].cell(r, c, &wb.shared_strings).value);
            *kinds.entry(format!("{} → {}", value_shape(&old), value_shape(&v))).or_default() += 1;
        }
        for (k, n) in kinds.iter().take(8) {
            println!("    {n} × {k}");
        }
        for (s, n) in by_sheet.iter().take(8) {
            println!("    aba {}: {n} mudaram", s + 1);
        }
    }
    let _ = wb;
}

/// `duck-bench objetos [ARQUIVO]`: gráficos, segmentações e formas lidos de
/// cada aba (só tipos e contagens).
pub fn cmd_objects(args: &[String]) -> Res {
    use duck_core::drawing::{ObjectKind, PlotKind};
    let path = crate::file_arg(args)?;
    let t = Instant::now();
    let (wb, _) = duck_xlsx::open(&path, &Options::default()).map_err(|e| e.to_string())?;
    println!("lido em {:.2} s", t.elapsed().as_secs_f64());
    for (i, s) in wb.sheets.iter().enumerate() {
        if s.layout.objects.is_empty() {
            continue;
        }
        let mut kinds: BTreeMap<String, u32> = BTreeMap::new();
        for o in &s.layout.objects {
            let k = match &o.kind {
                ObjectKind::Chart(c) => {
                    let p: Vec<String> = c
                        .plots
                        .iter()
                        .map(|p| {
                            format!(
                                "{}{}x{}",
                                match p.kind {
                                    PlotKind::Bar => "barras",
                                    PlotKind::BarH => "barras-h",
                                    PlotKind::Line => "linha",
                                    PlotKind::Area => "área",
                                    PlotKind::Pie => "pizza",
                                    PlotKind::Doughnut => "rosca",
                                    PlotKind::Scatter => "dispersão",
                                },
                                if p.secondary { "(2º eixo)" } else { "" },
                                p.series.len()
                            )
                        })
                        .collect();
                    let pts: usize = c.plots.iter().flat_map(|p| &p.series).map(|s| s.vals.len()).sum();
                    format!(
                        "gráfico [{}] {} pontos{}",
                        p.join(" + "),
                        pts,
                        if c.pivot { " (dinâmico)" } else { "" }
                    )
                }
                ObjectKind::Slicer(sl) => format!(
                    "segmentação {} itens, {} colunas{}",
                    sl.items.len(),
                    sl.columns,
                    if sl.table.is_some() { " (de tabela)" } else { "" }
                ),
                ObjectKind::Shape { .. } => "forma".into(),
                ObjectKind::Picture { .. } => "imagem".into(),
            };
            *kinds.entry(k).or_default() += 1;
        }
        println!("aba {}: {} objetos", i + 1, s.layout.objects.len());
        for (k, n) in kinds {
            println!("   {n} × {k}");
        }
    }
    Ok(())
}
