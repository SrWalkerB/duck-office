//! Macros VBA das planilhas: lê o código do `vbaProject.bin` e executa num
//! interpretador que só conhece a planilha aberta (células, tabelas,
//! dinâmicas). Não há acesso a arquivos, à rede nem a outros programas.

pub mod excel;
pub mod interp;
pub mod parse;
pub mod project;

pub use interp::{Interp, VbErr};
pub use project::{Module, modules};
