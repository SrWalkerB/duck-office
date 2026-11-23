//! `duck-bench regress [ARQUIVO...]`: confere de uma vez o que já funciona e
//! acusa regressões.
//!
//! Para cada arquivo, mede métricas (tempo de abrir, memória, fórmulas,
//! dinâmicas, gráficos...) e compara com a linha de base guardada em
//! `bench.baseline.txt` (fora do git: vem de planilhas com dados reais).
//! `--gravar` regrava a linha de base. Sem ARQUIVO, usa as linhas
//! `regress = "..."` de `bench.local.toml`. Nunca mostra conteúdo nem nomes.
//!
//! `--base CAMINHO` usa outra linha de base, só com o que não depende da
//! máquina (contagens e acertos): é a que o CI confere com a vitrine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use duck_core::drawing::ObjectKind;
use duck_core::{Sheet, Workbook};
use duck_xlsx::Options;

use crate::Res;

const BASELINE: &str = "bench.baseline.txt";

/// Como comparar uma métrica com a linha de base.
#[derive(Clone, Copy)]
enum Rule {
    /// Contagem: precisa ser igual.
    Same,
    /// Não pode diminuir (acertos).
    AtLeast,
    /// Tempo ou memória: no máximo esta folga (fração) acima.
    Slack(f64),
}

#[derive(Default)]
pub struct Metrics {
    list: Vec<(String, f64, Rule)>,
}

impl Metrics {
    fn add(&mut self, name: &str, v: f64, rule: Rule) {
        self.list.push((name.into(), v, rule));
    }
}

fn targets(args: &[String]) -> Result<Vec<PathBuf>, String> {
    let given: Vec<PathBuf> = (0..args.len())
        .filter(|&i| !args[i].starts_with("--") && (i == 0 || args[i - 1] != "--base"))
        .map(|i| PathBuf::from(&args[i]))
        .collect();
    if !given.is_empty() {
        return Ok(given);
    }
    let cfg = std::fs::read_to_string("bench.local.toml").map_err(|_| "sem bench.local.toml".to_string())?;
    let list: Vec<PathBuf> = cfg
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| k.trim() == "regress")
        .map(|(_, v)| PathBuf::from(v.trim().trim_matches('"')))
        .collect();
    if list.is_empty() {
        return Err("bench.local.toml sem linhas regress = \"caminho\"".into());
    }
    Ok(list)
}

/// Identifica o arquivo sem guardar o nome: tamanho + hash do diretório do zip.
fn key(path: &Path) -> String {
    let len = std::fs::metadata(path).map_or(0, |m| m.len());
    format!("arquivo-{len}")
}

fn read_baseline(path: &str) -> BTreeMap<String, f64> {
    parse_baseline(&std::fs::read_to_string(path).unwrap_or_default())
}

/// `nome = valor` por linha; o nome pode ter `=` (`salvar (1 = certo)`).
fn parse_baseline(text: &str) -> BTreeMap<String, f64> {
    text.lines()
        .filter_map(|l| l.rsplit_once('='))
        .filter_map(|(k, v)| Some((k.trim().to_string(), v.trim().parse().ok()?)))
        .collect()
}

impl Rule {
    fn ok(self, v: f64, old: Option<f64>) -> bool {
        match (old, self) {
            (None, _) => true,
            (Some(o), Rule::Same) => v == o,
            (Some(o), Rule::AtLeast) => v >= o,
            (Some(o), Rule::Slack(f)) => v <= o * (1.0 + f),
        }
    }
}

pub fn cmd(args: &[String]) -> Res {
    let save = args.iter().any(|a| a == "--gravar");
    let shared = crate::flag(args, "--base");
    let base_path = shared.clone().unwrap_or_else(|| BASELINE.to_string());
    let mut base = read_baseline(&base_path);
    let mut bad = 0;
    for (n, path) in targets(args)?.iter().enumerate() {
        // A base do repositório vale em qualquer máquina: o zip pode variar.
        let k = match &shared {
            Some(_) => path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            None => key(path),
        };
        println!("\n== planilha {} ==", n + 1);
        let m = measure(path)?;
        for (name, v, rule) in &m.list {
            if shared.is_some() && matches!(rule, Rule::Slack(_)) {
                continue;
            }
            let id = format!("{k}.{name}");
            let old = base.get(&id).copied();
            let ok = rule.ok(*v, old);
            let was = old.map_or("novo".to_string(), |o| format!("{o}"));
            println!("  {} {name:<38} {v:>14} (antes {was})", if ok { "ok " } else { "RUIM" });
            bad += !ok as u32;
            if save {
                base.insert(id, *v);
            }
        }
        // Uma métrica que sumiu também é regressão.
        let here = format!("{k}.");
        let gone =
            |id: &String| id.strip_prefix(&here).is_some_and(|n| m.list.iter().all(|(name, ..)| name != n));
        if save {
            base.retain(|id, _| !gone(id));
        } else {
            for id in base.keys().filter(|id| gone(id)) {
                println!("  RUIM {:<38} {:>14} (antes {})", &id[here.len()..], "sumiu", base[id]);
                bad += 1;
            }
        }
    }
    if save {
        let text: String = base.iter().map(|(k, v)| format!("{k} = {v}\n")).collect();
        std::fs::write(&base_path, text).map_err(|e| e.to_string())?;
        println!("\nlinha de base gravada em {base_path}");
        return Ok(());
    }
    if bad > 0 { Err(format!("{bad} métricas pioraram")) } else { Ok(()) }
}

fn measure(path: &Path) -> Result<Metrics, String> {
    let mut m = Metrics::default();
    let mut best = f64::MAX;
    let mut wb: Option<Workbook> = None;
    for _ in 0..3 {
        drop(wb.take());
        let t = Instant::now();
        let (w, _) = duck_xlsx::open(path, &Options::default()).map_err(|e| e.to_string())?;
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        wb = Some(w);
    }
    let wb = wb.unwrap();
    m.add("abrir (ms)", best.round(), Rule::Slack(0.35));
    m.add(
        "memória com tudo aberto (MB)",
        crate::mem::mb(crate::mem::process().current).round(),
        Rule::Slack(0.10),
    );
    objects(&wb, &mut m);
    charts(&wb, &mut m);
    conditional(&wb, &mut m);
    links(&wb, &mut m);
    macros(path, &mut m);
    drop(wb);
    others(path, &mut m)?;
    Ok(m)
}

/// Regras de formatação condicional lidas e as que o app sabe desenhar.
fn conditional(wb: &Workbook, m: &mut Metrics) {
    use duck_core::CfKind;
    let count = |f: &dyn Fn(&CfKind) -> bool| {
        wb.sheets.iter().flat_map(|s| &s.layout.cond).filter(|r| f(&r.kind)).count() as f64
    };
    m.add("formatação condicional: regras", count(&|_| true), Rule::Same);
    m.add("formatação condicional: entendidas", count(&|k| *k != CfKind::Unsupported), Rule::AtLeast);
    m.add(
        "formatação condicional: ícones próprios",
        count(&|k| matches!(k, CfKind::IconSet { icons, .. } if !icons.is_empty())),
        Rule::Same,
    );
    m.add(
        "formatação condicional: barras completas",
        count(&|k| matches!(k, CfKind::DataBar { look, .. } if **look != duck_core::BarLook::default())),
        Rule::Same,
    );
}

/// Links de objetos que levam a uma aba e célula que existem.
fn links(wb: &Workbook, m: &mut Metrics) {
    let names: Vec<String> = wb.sheets.iter().map(|s| s.name.to_lowercase()).collect();
    let (mut total, mut ok) = (0, 0);
    for o in wb.sheets.iter().flat_map(|s| &s.layout.objects) {
        let Some(loc) = o.link.as_deref().and_then(|l| l.strip_prefix('#')) else { continue };
        total += 1;
        let loc = loc.replace("%20", " ");
        let (sheet, cell) = match loc.rfind('!') {
            Some(i) => (loc[..i].trim_matches('\'').replace("''", "'").to_lowercase(), &loc[i + 1..]),
            None => (String::new(), loc.as_str()),
        };
        let cell_ok = cell.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'$');
        ok += (cell_ok && (sheet.is_empty() || names.contains(&sheet))) as u32;
    }
    if total > 0 {
        m.add("links internos", total as f64, Rule::Same);
        m.add("links internos que resolvem", ok as f64, Rule::AtLeast);
    }
}

/// Rotinas VBA e quantas o analisador entende.
fn macros(path: &Path, m: &mut Metrics) {
    let Some(bin) = duck_xlsx::Package::open(path).ok().and_then(|p| p.read("xl/vbaProject.bin").ok()) else {
        return;
    };
    let (mut procs, mut ok) = (0, 0);
    for md in duck_vba::modules(&bin) {
        for p in duck_vba::parse::parse_module(&md.name, &md.source).procs {
            procs += 1;
            ok += p.body.is_ok() as u32;
        }
    }
    m.add("macros: rotinas", procs as f64, Rule::Same);
    m.add("macros: rotinas entendidas", ok as f64, Rule::AtLeast);
}

/// Números de uma linha de saída, na ordem.
fn numbers(line: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in line.chars().chain([' ']) {
        if c.is_ascii_digit() || (c == '.' && !cur.is_empty()) {
            cur.push(c);
        } else if !cur.is_empty() {
            out.extend(cur.trim_end_matches('.').parse::<f64>());
            cur.clear();
        }
    }
    out
}

/// Os outros comandos, cada um num processo próprio (memória separada).
fn others(path: &Path, m: &mut Metrics) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let run = |args: &[&str]| -> Result<(bool, String), String> {
        let o = std::process::Command::new(&exe)
            .arg(args[0])
            .arg(path)
            .args(&args[1..])
            .output()
            .map_err(|e| e.to_string())?;
        Ok((o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned()))
    };
    let line = |out: &str, key: &str| out.lines().find(|l| l.contains(key)).map(numbers).unwrap_or_default();
    let (_, out) = run(&["calc"])?;
    if let [total, _, same, ..] = line(&out, "iguais ao Excel")[..] {
        m.add("fórmulas", total, Rule::Same);
        m.add("fórmulas iguais ao Excel", same, Rule::AtLeast);
    }
    let (_, out) = run(&["dinamicas"])?;
    if let [total, _, layout, _, same, _, ms, ..] = line(&out, "com o layout igual")[..] {
        m.add("dinâmicas", total, Rule::Same);
        m.add("dinâmicas: mesmo layout", layout, Rule::AtLeast);
        m.add("dinâmicas: células iguais", same, Rule::AtLeast);
        m.add("dinâmicas (ms)", ms, Rule::Slack(0.5));
    }
    let (_, out) = run(&["modelo"])?;
    if let [total, _, exact, same, _, _, ms, ..] = line(&out, "dinâmicas do modelo")[..]
        && total > 0.0
    {
        m.add("modelo: dinâmicas", total, Rule::Same);
        m.add("modelo: idênticas ao Excel", exact, Rule::AtLeast);
        m.add("modelo: valores iguais", same, Rule::AtLeast);
        m.add("modelo (ms)", ms, Rule::Slack(0.8));
    }
    for (name, args) in [
        ("salvar sem mudar", &["roundtrip"][..]),
        ("salvar com edições", &["roundtrip", "--edit", "200"]),
        ("salvar com linhas inseridas", &["roundtrip", "--estrutura"]),
    ] {
        let (ok, _) = run(args)?;
        m.add(&format!("{name} (1 = certo)"), ok as u32 as f64, Rule::AtLeast);
    }
    if let Some(ok) = move_objects(path)? {
        m.add("mover objetos (1 = certo)", ok as u32 as f64, Rule::AtLeast);
    }
    Ok(())
}

/// Move e estica todos os objetos de todas as abas, grava e relê: as âncoras
/// têm de voltar iguais. `None` = não há objetos.
fn move_objects(path: &Path) -> Result<Option<bool>, String> {
    use duck_core::drawing::{Anchor, Marker};
    let (mut wb, _) = duck_xlsx::open(path, &duck_xlsx::Options::default()).map_err(|e| e.to_string())?;
    let mark = |m: Marker| Marker { col: m.col + 1, col_off: 9525, row: m.row + 2, row_off: 19050 };
    let mut n = 0;
    for s in &mut wb.sheets {
        let list: Vec<(u32, Anchor)> = (s.layout.drawing_anchors.iter().enumerate())
            .filter(|(_, a)| !a.fallback)
            .map(|(i, a)| {
                let new = match a.anchor {
                    Anchor::Two { from, to } => Anchor::Two { from: mark(from), to: mark(to) },
                    Anchor::One { from, cx, cy } => Anchor::One { from: mark(from), cx: cx + 9525, cy },
                    Anchor::Abs { x, y, cx, cy } => Anchor::Abs { x: x + 9525, y, cx, cy: cy + 9525 },
                };
                (i as u32, new)
            })
            .collect();
        n += list.len();
        if !list.is_empty() {
            duck_core::Command::Anchors(list).apply(s);
        }
    }
    if n == 0 {
        return Ok(None);
    }
    let out = std::env::temp_dir().join(format!("duck-mover-{}.xlsx", std::process::id()));
    let sheets: Vec<Option<&Sheet>> = wb.sheets.iter().map(Some).collect();
    let opts = duck_xlsx::SaveOptions::default();
    let saved = duck_xlsx::save_file(path, &out, &sheets, &wb.shared_strings, &wb.styles, &opts);
    let same = saved.map_err(|e| e.to_string()).and_then(|_| {
        let (back, _) = duck_xlsx::open(&out, &duck_xlsx::Options::default()).map_err(|e| e.to_string())?;
        Ok(wb.sheets.iter().zip(&back.sheets).all(|(a, b)| {
            let pos = |s: &Sheet| s.layout.objects.iter().map(|o| (o.id, o.anchor)).collect::<Vec<_>>();
            a.layout.drawing_anchors == b.layout.drawing_anchors && pos(a) == pos(b)
        }))
    });
    let _ = std::fs::remove_file(&out);
    same.map(Some)
}

/// Quantos objetos de cada tipo o leitor encontra.
fn objects(wb: &Workbook, m: &mut Metrics) {
    let mut n: BTreeMap<&str, u32> = BTreeMap::new();
    for o in wb.sheets.iter().flat_map(|s| &s.layout.objects) {
        let k = match &o.kind {
            ObjectKind::Chart(_) => "gráficos",
            ObjectKind::Slicer(_) => "segmentações",
            ObjectKind::Timeline(_) => "linhas do tempo",
            ObjectKind::Shape(_) => "formas",
            ObjectKind::Control(_) => "controles",
            ObjectKind::Picture { .. } => "imagens",
        };
        *n.entry(k).or_default() += 1;
    }
    for (k, v) in n {
        m.add(&format!("objetos: {k}"), v as f64, Rule::Same);
    }
}

/// Séries relidas das células iguais ao cache que o Excel gravou.
fn charts(wb: &Workbook, m: &mut Metrics) {
    let refs: Vec<&Sheet> = wb.sheets.iter().collect();
    let book = duck_calc::Book::new(refs, &wb.shared_strings, &wb.extras, wb.date1904);
    let (mut total, mut same) = (0, 0);
    for (si, s) in wb.sheets.iter().enumerate() {
        for o in &s.layout.objects {
            let ObjectKind::Chart(c) = &o.kind else { continue };
            for s in c.plots.iter().flat_map(|p| &p.series) {
                total += 1;
                let up = duck_calc::chartdata::update(&book, s, si as u32, c.plot_hidden);
                // Só os valores: os textos do cache já vêm formatados.
                let eq = up.and_then(|u| u.vals).is_none_or(|v| {
                    let trim = |x: &[Option<f64>]| {
                        let n = x.iter().rposition(|v| v.is_some()).map_or(0, |i| i + 1);
                        x[..n].to_vec()
                    };
                    let (a, b) = (trim(&v), trim(&s.vals));
                    a.len() == b.len()
                        && a.iter().zip(&b).all(|(x, y)| match (x, y) {
                            (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(1.0),
                            (x, y) => x.is_none() == y.is_none(),
                        })
                });
                same += eq as u32;
            }
        }
    }
    m.add("gráficos: séries", total as f64, Rule::Same);
    m.add("gráficos: séries iguais ao Excel", same as f64, Rule::AtLeast);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_numbers() {
        assert_eq!(numbers("12 fórmulas: 3.5 (100.000%), fim 7."), [12.0, 3.5, 100.0, 7.0]);
    }

    #[test]
    fn baseline_names_may_have_equals() {
        let b = parse_baseline("v.salvar (1 = certo) = 1\nv.abrir (ms) = 12.5\nlixo\n");
        assert_eq!(b.get("v.salvar (1 = certo)"), Some(&1.0));
        assert_eq!(b.get("v.abrir (ms)"), Some(&12.5));
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn rules() {
        assert!(Rule::Same.ok(3.0, Some(3.0)) && !Rule::Same.ok(4.0, Some(3.0)));
        assert!(Rule::AtLeast.ok(4.0, Some(3.0)) && !Rule::AtLeast.ok(2.0, Some(3.0)));
        assert!(Rule::Slack(0.1).ok(109.0, Some(100.0)) && !Rule::Slack(0.1).ok(111.0, Some(100.0)));
        assert!(Rule::Same.ok(1.0, None));
    }
}
