//! Modelo de Dados (Power Pivot): as tabelas do modelo em colunas, as relações
//! entre elas e as medidas em DAX, para recalcular as tabelas dinâmicas sem o
//! Excel. Só o que as medidas usam é montado, e em paralelo.

pub mod cube;
pub mod eval;
pub mod model;
pub mod olap;
pub mod parse;

pub use eval::{Ctx, Eval};
pub use model::{Bits, Column, Kind, Model, Source, TableDef, Val};
