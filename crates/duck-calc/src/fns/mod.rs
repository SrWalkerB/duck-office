//! Funções por assunto (as básicas estão em [`crate::funcs`]).
//!
//! Cada módulo tem a lista das suas funções (`DEFS`); [`crate::funcs`] junta
//! todas numa tabela só.

use std::rc::Rc;

use duck_core::CellError;

use crate::eval::{Eval, Res};
use crate::value::{Arr, V};

pub mod coupon;
pub mod database;
pub mod dates;
pub mod dist;
pub mod engineering;
pub mod financial;
pub mod group;
pub mod info;
pub mod lambda;
pub mod math;
pub mod matrix;
pub mod regress;
pub mod special;
pub mod stats;
pub mod text;

/// Todos os argumentos se aplicam item a item ([`crate::funcs::FnDef::lift`]).
pub const ALL: u32 = u32::MAX;

/// Matriz `rows` × `cols` (vazia = `#CALC!`).
pub fn matrix<'e>(rows: u32, cols: u32, v: Vec<V<'e>>) -> Res<V<'e>> {
    if rows == 0 || cols == 0 || v.is_empty() {
        return Err(CellError::Calc);
    }
    Ok(V::Arr(Rc::new(Arr { rows, cols, v })))
}

/// Coluna de números.
pub fn column<'e>(v: impl IntoIterator<Item = f64>) -> Res<V<'e>> {
    let v: Vec<V> = v.into_iter().map(V::num).collect();
    matrix(v.len() as u32, 1, v)
}

/// Números de um argumento só, como nas funções de matriz do Excel: em
/// referências e matrizes só os números, com as contas item a item.
pub fn arr_nums<'e>(e: &mut Eval<'e>, n: u32) -> Res<Vec<f64>> {
    let saved = std::mem::replace(&mut e.array, true);
    let r = crate::funcs::num_list(e, &[n]);
    e.array = saved;
    r
}

/// Pares de números de duas matrizes do mesmo tamanho (os pares em que falta
/// um número não contam; tamanhos diferentes dão `#N/D`).
pub fn pairs<'e>(e: &mut Eval<'e>, a: u32, b: u32) -> Res<Vec<(f64, f64)>> {
    let x = e.arr(a)?;
    let y = e.arr(b)?;
    if x.v.len() != y.v.len() {
        return Err(CellError::NA);
    }
    let mut out = Vec::with_capacity(x.v.len());
    for (p, q) in x.v.iter().zip(&y.v) {
        match (p, q) {
            (V::Err(e), _) | (_, V::Err(e)) => return Err(*e),
            (V::Num(p), V::Num(q)) => out.push((*p, *q)),
            _ => {}
        }
    }
    Ok(out)
}

/// Função de um número: `num1!(nome, x, resultado)`.
macro_rules! num1 {
    ($name:ident, $x:ident, $body:expr) => {
        fn $name<'e>(e: &mut $crate::eval::Eval<'e>, a: &[u32]) -> $crate::eval::Res<$crate::value::V<'e>> {
            let $x = e.num(a[0])?;
            let r: $crate::eval::Res<f64> = $body;
            Ok($crate::value::V::num(r?))
        }
    };
}

/// Função de números (os que faltam valem o padrão): `nums!(nome, [x, y = 1.0], resultado)`.
macro_rules! nums {
    ($name:ident, [$($x:ident $(= $d:expr)?),+], $body:expr) => {
        fn $name<'e>(
            e: &mut $crate::eval::Eval<'e>,
            a: &[u32],
        ) -> $crate::eval::Res<$crate::value::V<'e>> {
            let mut _k = 0usize;
            $(
                #[allow(unused_assignments)]
                let $x = nums!(@arg e, a, _k $(, $d)?);
                _k += 1;
            )+
            let r: $crate::eval::Res<f64> = $body;
            Ok($crate::value::V::num(r?))
        }
    };
    (@arg $e:ident, $a:ident, $k:ident) => { $e.num($a[$k])? };
    (@arg $e:ident, $a:ident, $k:ident, $d:expr) => { $e.opt_num($a, $k, $d)? };
}

pub(crate) use {num1, nums};
