//! Informação sobre células, fórmulas, abas e o programa.

use duck_core::{CellError, FormulaKind};

use super::matrix;
use crate::eval::{Eval, Res};
use crate::funcs::{FnDef, col_letters, n};
use crate::value::{Rng, Sid, V, text_eq};

pub const DEFS: &[FnDef] = &[
    n("FORMULATEXT", "FÓRMULATEXTO", 1, 1, formulatext, "ref", "A fórmula de uma célula, como texto"),
    n("ISFORMULA", "ÉFÓRMULA", 1, 1, isformula, "ref", "VERDADEIRO se a célula tem uma fórmula"),
    n("SHEET", "PLAN", 0, 1, sheet, "[valor]", "O número de uma aba"),
    n("SHEETS", "PLANS", 0, 1, sheets, "[ref]", "Quantas abas o arquivo tem"),
    n("CELL", "CÉL", 1, 2, cell, "tipo_info; [ref]", "Uma informação sobre uma célula").volatile(),
    n("INFO", "INFORMAÇÃO", 1, 1, info, "tipo_texto", "Uma informação sobre o programa").volatile(),
];

/// Referência de um argumento.
fn ref_arg(e: &mut Eval, n: u32) -> Res<Rng> {
    match e.eval(n) {
        V::Ref(r) => Ok(r),
        V::Err(x) => Err(x),
        _ => Err(CellError::Value),
    }
}

/// `f` de cada célula da referência (só a primeira fora das matrizes).
fn per_cell<'e>(e: &mut Eval<'e>, r: Rng, f: impl Fn(&Eval<'e>, u32, u32) -> V<'e>) -> Res<V<'e>> {
    if !e.array || r.is_cell() {
        return Ok(f(e, r.r0, r.c0));
    }
    let r1 = r.r1.min(r.r0.max(e.book.max_rows.saturating_sub(1)));
    let c1 = r.c1.min(r.c0.max(e.book.used_cols(r.sheet).saturating_sub(1)));
    let mut v = Vec::new();
    for row in r.r0..=r1 {
        v.extend((r.c0..=c1).map(|col| f(e, row, col)));
    }
    matrix(r1 - r.r0 + 1, c1 - r.c0 + 1, v)
}

fn formulatext<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let r = match ref_arg(e, a[0]) {
        Ok(r) => r,
        Err(CellError::Value) => return Err(CellError::NA),
        Err(x) => return Err(x),
    };
    per_cell(e, r, |e, row, col| {
        let Sid::Local(s) = r.sheet else { return V::Err(CellError::NA) };
        let Some(sheet) = e.book.sheets.get(s as usize) else { return V::Err(CellError::NA) };
        let Some(f) = crate::cell_formula(sheet, row, col) else { return V::Err(CellError::NA) };
        let text = format!("={}", crate::locale::to_pt(&f));
        let cse = matches!(
            sheet.formula(row, col).map(|f| f.kind),
            Some(FormulaKind::Array { dynamic: false, .. })
        );
        V::text(if cse { format!("{{{text}}}") } else { text })
    })
}

fn isformula<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let r = ref_arg(e, a[0])?;
    per_cell(e, r, |e, row, col| {
        let Sid::Local(s) = r.sheet else { return V::Bool(false) };
        V::Bool(e.book.sheets.get(s as usize).is_some_and(|x| x.formula(row, col).is_some()))
    })
}

fn sheet<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let Some(&n) = a.first() else { return Ok(V::Num(e.sheet as f64 + 1.0)) };
    match e.eval(n) {
        V::Ref(Rng { sheet: Sid::Local(s), .. }) => Ok(V::Num(s as f64 + 1.0)),
        V::Str(name) => {
            let i = e.book.sheets.iter().position(|s| text_eq(&s.name, &name));
            i.map(|i| V::Num(i as f64 + 1.0)).ok_or(CellError::NA)
        }
        V::Err(x) => Err(x),
        _ => Err(CellError::NA),
    }
}

fn sheets<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match a.first() {
        None => Ok(V::Num(e.book.sheets.len() as f64)),
        // Referências a várias abas não são aceitas: é sempre uma.
        Some(&n) => ref_arg(e, n).map(|_| V::Num(1.0)),
    }
}

/// Minúsculas sem acentos (o tipo pode vir em português ou inglês).
fn plain(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'ã' | 'â' | 'à' => 'a',
            'é' | 'ê' => 'e',
            'í' => 'i',
            'ó' | 'ô' | 'õ' => 'o',
            'ú' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

fn quoted(name: &str) -> String {
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// CÉL: o que depende só da célula. Formatação (cor, formato, prefixo,
/// proteção) e o nome do arquivo quando não se sabe ficam com o valor guardado.
fn cell<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let kind = plain(&e.text(a[0])?);
    let here = Rng { sheet: Sid::Local(e.sheet), r0: e.row, r1: e.row, c0: e.col, c1: e.col };
    let r = match a.get(1) {
        Some(&n) if !e.is_missing(n) => ref_arg(e, n)?,
        _ => here,
    };
    let Sid::Local(s) = r.sheet else { return Err(CellError::Value) };
    let Some(sheet) = e.book.sheets.get(s as usize).copied() else { return Err(CellError::Ref) };
    Ok(match kind.as_str() {
        "address" | "endereco" => {
            let own = format!("${}${}", col_letters(r.c0), r.r0 + 1);
            V::text(if s == e.sheet { own } else { format!("{}!{own}", quoted(&sheet.name)) })
        }
        "col" => V::Num(r.c0 as f64 + 1.0),
        "row" | "lin" => V::Num(r.r0 as f64 + 1.0),
        "contents" | "conteudo" => match e.book.get(r.sheet, r.r0, r.c0) {
            V::Empty => V::Num(0.0),
            v => v,
        },
        "type" | "tipo" => V::text(match e.book.get(r.sheet, r.r0, r.c0) {
            V::Empty => "b",
            V::Str(_) => "l",
            _ => "v",
        }),
        "width" | "largura" => {
            let own = sheet.col_width(r.c0);
            let w = if sheet.col_hidden(r.c0) {
                0.0
            } else {
                own.or(sheet.layout.default_col_width).unwrap_or(8.43) as f64
            };
            return matrix(1, 2, vec![V::Num(w.round()), V::Bool(own.is_none())]);
        }
        "filename" | "nome.arquivo" => match &e.book.path {
            Some(p) if p.is_empty() => V::text(""),
            Some(p) => {
                let cut = p.rfind(['/', '\\']).map_or(0, |i| i + 1);
                V::text(format!("{}[{}]{}", &p[..cut], &p[cut..], sheet.name))
            }
            None => {
                e.opaque = true;
                return Err(CellError::Value);
            }
        },
        "color" | "cor" | "format" | "formato" | "parentheses" | "parenteses" | "prefix" | "prefixo"
        | "protect" | "proteger" | "protecao" => {
            e.opaque = true;
            return Err(CellError::Value);
        }
        _ => return Err(CellError::Value),
    })
}

fn info<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let kind = plain(&e.text(a[0])?);
    Ok(match kind.as_str() {
        "directory" | "diretorio" => match &e.book.path {
            Some(p) => V::text(p[..p.rfind(['/', '\\']).map_or(0, |i| i + 1)].to_string()),
            None => {
                e.opaque = true;
                return Err(CellError::Value);
            }
        },
        "numfile" | "numarquivo" => V::Num(e.book.sheets.len() as f64),
        "origin" | "origem" => V::text("$A:$A$1"),
        "osversion" | "versaoso" => V::text(match std::env::consts::OS {
            "windows" => "Windows (64-bit) NT :.00",
            "macos" => "Macintosh",
            _ => "Linux",
        }),
        "recalc" | "recalcular" => V::text("Automático"),
        "release" | "versao" => V::text("16.0"),
        "system" | "sistema" => V::text(if cfg!(target_os = "macos") { "mac" } else { "pcdos" }),
        "memavail" | "memused" | "totmem" => return Err(CellError::NA),
        _ => return Err(CellError::Value),
    })
}
