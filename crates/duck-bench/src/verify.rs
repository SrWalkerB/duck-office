//! Confere célula a célula o que o `duck-xlsx` leu contra o `calamine`,
//! um leitor de xlsx independente e maduro.
//!
//! No `.xlsb` só os valores são comparados: o calamine não expande as fórmulas
//! compartilhadas e de matriz, perde o `$` das referências e troca funções (a
//! tabela dele está deslocada a partir da 469). As fórmulas do `.xlsb` são
//! conferidas pelo recálculo (`duck-bench calc`).

use calamine::{Data, Reader, Sheets, Xlsx, open_workbook, open_workbook_auto};
use duck_core::{Value, Workbook};

pub struct Outcome {
    /// O arquivo é um `.xlsb`.
    pub binary: bool,
    pub sheets: usize,
    pub compared: u64,
    pub mismatches: u64,
    pub formulas_compared: u64,
    pub formula_mismatches: u64,
    pub merges_compared: u64,
    pub merge_mismatches: u64,
    /// Erros de fórmula do `.xlsb`: o calamine pula esse registro (`BrtFmlaError`).
    pub formula_errors: u64,
    pub samples: Vec<String>,
}

/// Fórmula com o conteúdo dos textos literais escondido: referências e nomes
/// de função aparecem (para depurar), dados da planilha não.
fn mask(f: &str) -> String {
    let mut out = String::with_capacity(f.len());
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
    out
}

fn matches(ours: Value, theirs: &Data) -> bool {
    match (ours, theirs) {
        (Value::Number(a), Data::Float(b)) => a == *b || (a.is_nan() && b.is_nan()),
        (Value::Number(a), Data::Int(b)) => a == *b as f64,
        (Value::Number(a), Data::DateTime(d)) => a == d.as_f64(),
        (Value::Text(a), Data::String(b)) => a == b,
        (Value::Text(a), Data::DateTimeIso(b)) | (Value::Text(a), Data::DurationIso(b)) => a == b,
        (Value::Bool(a), Data::Bool(b)) => a == *b,
        (Value::Error(_), Data::Error(_)) => true,
        (Value::Empty, Data::Empty) => true,
        // calamine guarda texto vazio de fórmula como string vazia.
        (Value::Empty, Data::String(s)) => s.is_empty(),
        (Value::Text(s), Data::Empty) => s.is_empty(),
        _ => false,
    }
}

fn kind_ours(v: Value) -> &'static str {
    match v {
        Value::Empty => "vazio",
        Value::Number(_) => "número",
        Value::Text(_) => "texto",
        Value::Bool(_) => "booleano",
        Value::Error(_) => "erro",
    }
}

fn kind_theirs(d: &Data) -> &'static str {
    match d {
        Data::Int(_) | Data::Float(_) => "número",
        Data::DateTime(_) => "data",
        Data::String(_) | Data::DateTimeIso(_) | Data::DurationIso(_) => "texto",
        Data::Bool(_) => "booleano",
        Data::Error(_) => "erro",
        Data::Empty => "vazio",
    }
}

/// Descreve a diferença **sem mostrar o conteúdo** da célula (o arquivo de
/// referência tem dados reais): só tipos, tamanhos e o caractere divergente.
fn describe(ours: Value, theirs: &Data) -> String {
    if let (Value::Text(a), Data::String(b)) = (ours, theirs) {
        let pos = a
            .chars()
            .zip(b.chars())
            .position(|(x, y)| x != y)
            .unwrap_or(a.chars().count().min(b.chars().count()));
        let at = |s: &str| {
            s.chars().nth(pos).map(|c| format!("U+{:04X}", c as u32)).unwrap_or_else(|| "fim".into())
        };
        return format!(
            "texto de {} × {} caracteres, primeira diferença no caractere {pos}: {} × {}",
            a.chars().count(),
            b.chars().count(),
            at(a),
            at(b)
        );
    }
    if let (Value::Number(a), Data::Float(b)) = (ours, theirs) {
        return format!("número difere em {:e}", (a - b).abs());
    }
    format!("nosso {} × calamine {}", kind_ours(ours), kind_theirs(theirs))
}

pub fn run(path: &std::path::Path, wb: &Workbook) -> Result<Outcome, String> {
    let mut cal = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let binary = matches!(cal, Sheets::Xlsb(_));
    let mut out = Outcome {
        binary,
        sheets: 0,
        compared: 0,
        mismatches: 0,
        formulas_compared: 0,
        formula_mismatches: 0,
        merges_compared: 0,
        merge_mismatches: 0,
        formula_errors: 0,
        samples: Vec::new(),
    };
    for (si, sheet) in wb.sheets.iter().enumerate() {
        let range = cal.worksheet_range(&sheet.name).map_err(|e| format!("calamine, aba {si}: {e}"))?;
        out.sheets += 1;
        let (r0, c0) = range.start().unwrap_or((0, 0));
        let mut theirs_values = 0u64;
        for (r, c, d) in range.used_cells() {
            let (row, col) = (r0 + r as u32, c0 + c as u32);
            theirs_values += 1;
            out.compared += 1;
            let ours = wb.cell(si, row, col).value;
            if !matches(ours, d) {
                out.mismatches += 1;
                if out.samples.len() < 10 {
                    out.samples.push(format!(
                        "aba {si}, linha {}, coluna {}: {}",
                        row + 1,
                        col + 1,
                        describe(ours, d)
                    ));
                }
            }
        }
        // O contrário: nada que lemos pode faltar no calamine.
        let ours_values = sheet.counts().values();
        let diff = ours_values.abs_diff(theirs_values);
        if diff > 0 {
            // Diferenças aqui só contam se não forem strings vazias (que o calamine pode omitir).
            let mut extra = 0u64;
            for row in 0..sheet.rows() {
                for col in 0..sheet.cols() {
                    let v = wb.cell(si, row, col).value;
                    if matches!(v, Value::Empty) || matches!(v, Value::Text("")) {
                        continue;
                    }
                    let in_range = row >= r0 && col >= c0;
                    let theirs =
                        if in_range { range.get(((row - r0) as usize, (col - c0) as usize)) } else { None };
                    if theirs.is_none_or(|d| matches!(d, Data::Empty)) {
                        if binary && matches!(v, Value::Error(_)) && sheet.formula(row, col).is_some() {
                            out.formula_errors += 1;
                            continue;
                        }
                        extra += 1;
                        if out.samples.len() < 10 {
                            out.samples.push(format!(
                                "aba {si}, linha {}, coluna {}: só nós lemos um valor ({})",
                                row + 1,
                                col + 1,
                                kind_ours(v)
                            ));
                        }
                    }
                }
            }
            out.mismatches += extra;
        }
        if binary {
            continue;
        }
        // Fórmulas: o calamine expande as compartilhadas por conta própria.
        let fr =
            cal.worksheet_formula(&sheet.name).map_err(|e| format!("calamine, fórmulas da aba {si}: {e}"))?;
        let (fr0, fc0) = fr.start().unwrap_or((0, 0));
        let mut theirs_f = 0u64;
        for (r, c, f) in fr.used_cells() {
            if f.is_empty() {
                continue;
            }
            let (row, col) = (fr0 + r as u32, fc0 + c as u32);
            theirs_f += 1;
            out.formulas_compared += 1;
            let ours = duck_calc::cell_formula(sheet, row, col);
            if ours.as_deref() != Some(f.as_str()) {
                out.formula_mismatches += 1;
                if out.samples.len() < 10 {
                    out.samples.push(format!(
                        "aba {si}, linha {}, coluna {}: fórmula nossa {:?} × calamine {:?}",
                        row + 1,
                        col + 1,
                        ours.as_deref().map(mask),
                        mask(f)
                    ));
                }
            }
        }
        let ours_f = sheet.formula_cells();
        if ours_f != theirs_f {
            for (row, col) in sheet.formula_positions() {
                let theirs = if row >= fr0 && col >= fc0 {
                    fr.get(((row - fr0) as usize, (col - fc0) as usize)).filter(|f| !f.is_empty())
                } else {
                    None
                };
                if theirs.is_none() {
                    out.formula_mismatches += 1;
                    if out.samples.len() < 10 {
                        out.samples.push(format!(
                            "aba {si}, linha {}, coluna {}: só nós lemos uma fórmula",
                            row + 1,
                            col + 1
                        ));
                    }
                }
            }
        }
    }

    // Células mescladas: mesmo conjunto de intervalos em cada aba.
    if let Ok(mut x) = open_workbook::<Xlsx<_>, _>(path) {
        for (si, sheet) in wb.sheets.iter().enumerate() {
            let Ok(theirs) = x.merge_cells_by_sheet_name(&sheet.name) else { continue };
            let mut a: Vec<(u32, u32, u32, u32)> = sheet
                .layout
                .merges
                .iter()
                .map(|m| (m.first_row, m.first_col, m.last_row, m.last_col))
                .collect();
            let mut b: Vec<(u32, u32, u32, u32)> =
                theirs.iter().map(|d| (d.start.0, d.start.1, d.end.0, d.end.1)).collect();
            a.sort_unstable();
            b.sort_unstable();
            out.merges_compared += b.len() as u64;
            if a != b {
                out.merge_mismatches += a.len().abs_diff(b.len()).max(1) as u64;
                if out.samples.len() < 10 {
                    out.samples.push(format!(
                        "aba {si}: {} mesclagens nossas × {} do calamine",
                        a.len(),
                        b.len()
                    ));
                }
            }
        }
    }
    Ok(out)
}
