//! Texto: DIVIDIRTEXTO e MOEDA.

use duck_core::{CellError, NumFmt};

use super::{ALL, matrix};
use crate::eval::{Eval, Res, to_text};
use crate::funcs::{FnDef, n, round_digits};
use crate::value::V;

pub const DEFS: &[FnDef] = &[
    n(
        "TEXTSPLIT",
        "DIVIDIRTEXTO",
        2,
        6,
        textsplit,
        "texto; delimitador_de_coluna; [delimitador_de_linha]; [ignorar_vazio]; [modo_de_correspondência]; [preencher_com]",
        "O texto dividido em colunas e linhas",
    ),
    n("DOLLAR", "MOEDA", 1, 2, dollar, "núm; [decimais]", "O número como texto, no formato de moeda")
        .lift(ALL),
];

/// Delimitadores de um argumento (um ou vários), os mais longos primeiro.
fn delimiters<'e>(e: &mut Eval<'e>, a: &[u32], i: usize) -> Res<Vec<Vec<char>>> {
    let Some(&n) = a.get(i) else { return Ok(Vec::new()) };
    if e.is_missing(n) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for v in &e.arr(n)?.v {
        let t: Vec<char> = to_text(v.clone())?.chars().collect();
        if t.is_empty() {
            return Err(CellError::Value);
        }
        out.push(t);
    }
    out.sort_by_key(|d| std::cmp::Reverse(d.len()));
    Ok(out)
}

/// Partes de `s` entre os delimitadores.
fn split(s: &[char], delims: &[Vec<char>], fold: bool, skip_empty: bool) -> Vec<Vec<char>> {
    if delims.is_empty() {
        return vec![s.to_vec()];
    }
    let same = |a: char, b: char| a == b || (fold && a.to_lowercase().eq(b.to_lowercase()));
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let hit = delims
            .iter()
            .find(|d| s.len() - i >= d.len() && d.iter().zip(&s[i..]).all(|(a, b)| same(*a, *b)));
        match hit {
            Some(d) => {
                out.push(std::mem::take(&mut cur));
                i += d.len();
            }
            None => {
                cur.push(s[i]);
                i += 1;
            }
        }
    }
    out.push(cur);
    if skip_empty {
        out.retain(|p| !p.is_empty());
    }
    out
}

fn textsplit<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s: Vec<char> = e.text(a[0])?.chars().collect();
    let cols = delimiters(e, a, 1)?;
    let rows = delimiters(e, a, 2)?;
    let skip = e.opt_bool(a, 3, false)?;
    let fold = e.opt_num(a, 4, 0.0)? != 0.0;
    if cols.is_empty() && rows.is_empty() {
        return Err(CellError::Value);
    }
    let pad = match a.get(5) {
        Some(&n) if !e.is_missing(n) => e.scalar(n),
        _ => V::Err(CellError::NA),
    };
    let mut lines: Vec<Vec<Vec<char>>> =
        split(&s, &rows, fold, skip).into_iter().map(|l| split(&l, &cols, fold, skip)).collect();
    if skip {
        lines.retain(|l| !l.is_empty());
    }
    let width = lines.iter().map(Vec::len).max().unwrap_or(0);
    let mut v = Vec::with_capacity(lines.len() * width);
    for l in &lines {
        for c in 0..width {
            v.push(match l.get(c) {
                Some(p) => V::text(p.iter().collect::<String>()),
                None => pad.clone(),
            });
        }
    }
    matrix(lines.len() as u32, width as u32, v)
}

fn dollar<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let d = e.opt_num(a, 1, 2.0)?.trunc();
    if d > 127.0 {
        return Err(CellError::Value);
    }
    let r = round_digits(x, d as i64, 0);
    let code = if d > 0.0 { format!("#,##0.{}", "0".repeat(d as usize)) } else { "#,##0".to_string() };
    let mut out = String::new();
    NumFmt::parse(&code).format_number(r.abs(), false, &mut out);
    Ok(V::text(format!("{}R$ {out}", if r < 0.0 { "-" } else { "" })))
}
