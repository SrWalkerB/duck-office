//! Pedaços de XML.

use duck_core::CellRange;

use super::rec::{LAST_COL, LAST_ROW};
use crate::write::cellxml;

/// ` nome="valor"`, escapado.
pub fn attr(name: &str, value: &str) -> String {
    let mut v = format!(" {name}=\"").into_bytes();
    cellxml::escape_attr(value, &mut v);
    v.push(b'"');
    String::from_utf8(v).unwrap_or_default()
}

/// Como [`attr`], com os caracteres de controle como `_xHHHH_` (`ST_Xstring`),
/// como o Excel grava nomes e textos.
pub fn xattr(name: &str, value: &str) -> String {
    if !value.bytes().any(|b| b < 0x20) {
        return attr(name, value);
    }
    let mut s = String::with_capacity(value.len() + 16);
    for ch in value.chars() {
        match ch as u32 {
            c @ 0..0x20 => s.push_str(&format!("_x{c:04x}_")),
            _ => s.push(ch),
        }
    }
    attr(name, &s)
}

/// ` nome="1"` se ligado.
pub fn flag(name: &str, on: bool) -> String {
    if on { format!(" {name}=\"1\"") } else { String::new() }
}

/// Texto escapado para dentro de um elemento.
pub fn text(s: &str) -> String {
    let mut v = Vec::with_capacity(s.len());
    cellxml::escape_text(s, &mut v);
    String::from_utf8(v).unwrap_or_default()
}

/// Referência A1. Fora da aba (arquivo corrompido), a célula da ponta.
pub fn cell(row: u32, col: u32) -> String {
    let (row, col) = (row.min(LAST_ROW), col.min(LAST_COL));
    let mut v = Vec::with_capacity(8);
    cellxml::cell_ref(row, col, &mut v);
    String::from_utf8(v).unwrap_or_default()
}

pub fn range(r: &CellRange) -> String {
    let a = cell(r.first_row, r.first_col);
    if (r.first_row, r.first_col) == (r.last_row, r.last_col) {
        return a;
    }
    format!("{a}:{}", cell(r.last_row, r.last_col))
}

pub fn ranges(list: &[CellRange]) -> String {
    list.iter().map(range).collect::<Vec<_>>().join(" ")
}
