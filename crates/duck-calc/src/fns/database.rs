//! Funções de banco de dados (BDSOMA e companhia): uma tabela com cabeçalho,
//! um campo e um intervalo de critérios.

use duck_core::CellError;

use crate::eval::{Eval, Res, to_bool};
use crate::funcs::{Acc, FnDef, n, variance};
use crate::index::Crit;
use crate::value::{Rng, Sid, V, text_eq};

const ARGS: &str = "banco_dados; campo; critérios";

pub const DEFS: &[FnDef] = &[
    n("DSUM", "BDSOMA", 3, 3, dsum, ARGS, "A soma de um campo dos registros que atendem aos critérios"),
    n("DCOUNT", "BDCONTAR", 3, 3, dcount, ARGS, "Conta os números de um campo"),
    n("DCOUNTA", "BDCONTARA", 3, 3, dcounta, ARGS, "Conta os valores de um campo"),
    n("DAVERAGE", "BDMÉDIA", 3, 3, daverage, ARGS, "A média de um campo"),
    n("DMAX", "BDMÁX", 3, 3, dmax, ARGS, "O maior número de um campo"),
    n("DMIN", "BDMÍN", 3, 3, dmin, ARGS, "O menor número de um campo"),
    n("DGET", "BDEXTRAIR", 3, 3, dget, ARGS, "O valor do único registro que atende aos critérios"),
    n("DPRODUCT", "BDMULTIPL", 3, 3, dproduct, ARGS, "O produto de um campo"),
    n("DSTDEV", "BDEST", 3, 3, dstdev, ARGS, "O desvio padrão de uma amostra"),
    n("DSTDEVP", "BDDESVPA", 3, 3, dstdevp, ARGS, "O desvio padrão da população"),
    n("DVAR", "BDVAREST", 3, 3, dvar, ARGS, "A variância de uma amostra"),
    n("DVARP", "BDVARP", 3, 3, dvarp, ARGS, "A variância da população"),
];

fn ref_arg(e: &mut Eval, n: u32) -> Res<Rng> {
    match e.eval(n) {
        V::Ref(r) => Ok(r),
        V::Err(x) => Err(x),
        _ => Err(CellError::Value),
    }
}

/// Condição de um critério numa coluna.
enum Test {
    Field(u32, Crit),
    /// Fórmula (célula dos critérios) avaliada para cada registro.
    Formula(u32, u32),
    /// Constante debaixo de um título que não é um campo.
    Fixed(bool),
}

/// Critério de texto puro vale para o começo do campo ("Ma" acha "Maçã").
fn criterion(v: &V, date1904: bool) -> Crit {
    match v {
        V::Str(s)
            if !s.is_empty()
                && !s.starts_with(['=', '<', '>'])
                && crate::value::parse_number(s, date1904).is_none() =>
        {
            Crit::parse(&V::text(format!("{s}*")), date1904)
        }
        v => Crit::parse(v, date1904),
    }
}

/// Valores do campo nos registros que passam (com o campo omitido, todos os
/// registros contam como números).
fn records<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<Vec<V<'e>>> {
    let db = ref_arg(e, a[0])?;
    let field = if e.is_missing(a[1]) { None } else { Some(e.scalar(a[1])) };
    let crit = ref_arg(e, a[2])?;
    let book = e.book;
    let used = book.used_rows(db.sheet).saturating_sub(1);
    let last = db.r1.min(used.max(db.r0));
    let titles: Vec<V> = (db.c0..=db.c1).map(|c| book.get(db.sheet, db.r0, c)).collect();
    let title = |s: &str| {
        titles.iter().position(|t| matches!(t, V::Str(x) if text_eq(x.trim(), s.trim()))).map(|k| k as u32)
    };
    let col = match &field {
        None | Some(V::Empty) => None,
        Some(V::Err(x)) => return Err(*x),
        Some(V::Str(s)) => Some(title(s).ok_or(CellError::Value)?),
        Some(v) => {
            let k = e.to_num(v)?.trunc();
            if k < 1.0 || k > db.cols() as f64 {
                return Err(CellError::Value);
            }
            Some(k as u32 - 1)
        }
    };
    // Cada linha de critérios é um "E" das colunas; as linhas são um "OU".
    let mut any: Vec<Vec<Test>> = Vec::new();
    for row in crit.r0 + 1..=crit.r1 {
        let mut all = Vec::new();
        for c in crit.c0..=crit.c1 {
            let v = book.get(crit.sheet, row, c);
            if matches!(v, V::Empty) {
                continue;
            }
            let head = match book.get(crit.sheet, crit.r0, c) {
                V::Str(s) => title(&s),
                V::Num(x) => titles.iter().position(|t| *t == V::Num(x)).map(|k| k as u32),
                _ => None,
            };
            all.push(match head {
                Some(k) => Test::Field(k, criterion(&v, book.date1904)),
                None => match crit.sheet {
                    Sid::Local(s) if e.eng.formula(book, s, row, c).is_some() => Test::Formula(row, c),
                    _ => Test::Fixed(to_bool(&v).unwrap_or(false)),
                },
            });
        }
        // Linha vazia: todos os registros passam.
        let blank = all.is_empty();
        any.push(all);
        if blank {
            break;
        }
    }
    let mut out = Vec::new();
    for row in db.r0 + 1..=last {
        let pass = any.iter().any(|all| {
            all.iter().all(|t| match t {
                Test::Field(k, c) => c.matches(&book.get(db.sheet, row, db.c0 + k)),
                Test::Fixed(b) => *b,
                Test::Formula(r, c) => computed(e, crit.sheet, *r, *c, row - db.r0 - 1),
            })
        });
        if pass {
            out.push(match col {
                Some(k) => book.get(db.sheet, row, db.c0 + k),
                None => V::Num(1.0),
            });
        }
    }
    Ok(out)
}

/// Critério calculado: a fórmula fala do primeiro registro e anda `down` linhas.
fn computed(e: &Eval, sheet: Sid, row: u32, col: u32, down: u32) -> bool {
    let Sid::Local(s) = sheet else { return false };
    let Some(f) = e.eng.formula(e.book, s, row, col) else { return false };
    let mut sub = Eval::new(e.book, e.eng, &f, s, row + down, col);
    let v = sub.eval(f.root);
    to_bool(&sub.to_scalar(v)).unwrap_or(false)
}

fn numbers(e: &mut Eval, a: &[u32]) -> Res<Vec<f64>> {
    let mut out = Vec::new();
    for v in records(e, a)? {
        match v {
            V::Num(x) => out.push(x),
            V::Err(x) => return Err(x),
            _ => {}
        }
    }
    Ok(out)
}

fn dsum<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut acc = Acc::default();
    numbers(e, a)?.into_iter().for_each(|x| acc.add(x));
    Ok(V::num(acc.get()))
}

fn dcount<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(records(e, a)?.iter().filter(|v| matches!(v, V::Num(_))).count() as f64))
}

fn dcounta<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(records(e, a)?.iter().filter(|v| !matches!(v, V::Empty)).count() as f64))
}

fn daverage<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = numbers(e, a)?;
    if v.is_empty() { Err(CellError::Div0) } else { Ok(V::num(super::stats::mean(&v))) }
}

fn dmax<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(numbers(e, a)?.into_iter().reduce(f64::max).unwrap_or(0.0)))
}

fn dmin<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(numbers(e, a)?.into_iter().reduce(f64::min).unwrap_or(0.0)))
}

fn dget<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut v = records(e, a)?;
    match v.len() {
        0 => Err(CellError::Value),
        1 => Ok(v.remove(0)),
        _ => Err(CellError::Num),
    }
}

fn dproduct<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = numbers(e, a)?;
    Ok(V::num(if v.is_empty() { 0.0 } else { v.iter().product() }))
}

fn dstdev<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&numbers(e, a)?, true)?.sqrt()))
}

fn dstdevp<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&numbers(e, a)?, false)?.sqrt()))
}

fn dvar<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&numbers(e, a)?, true)?))
}

fn dvarp<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&numbers(e, a)?, false)?))
}
