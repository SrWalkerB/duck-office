//! Confere célula a célula o que o `duck-xlsx` leu contra o `calamine`,
//! um leitor de xlsx independente e maduro.

use calamine::{Data, Reader, open_workbook_auto};
use duck_core::{Value, Workbook};

pub struct Outcome {
    pub sheets: usize,
    pub compared: u64,
    pub mismatches: u64,
    pub samples: Vec<String>,
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
    let mut out = Outcome { sheets: 0, compared: 0, mismatches: 0, samples: Vec::new() };
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
    }
    Ok(out)
}
