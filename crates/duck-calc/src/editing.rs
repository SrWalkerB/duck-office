//! Ajudas da edição: o que a AutoSoma soma e a fórmula de uma cópia transposta.

use duck_core::{CellRange, Sheet, StringTable, Value};

use crate::{Piece, push_cell, push_col, push_raw, walk};

/// Fórmulas da AutoSoma para a seleção `sel`: (linha, coluna, fórmula sem `=`).
///
/// - Uma célula: os números logo acima dela; se não há, os da esquerda.
/// - Um bloco: se a última linha (ou coluna) está vazia, as somas vão nela;
///   senão, na linha de baixo (uma linha só: na célula à direita).
pub fn autosum(sheet: &Sheet, shared: &StringTable, sel: CellRange) -> Vec<(u32, u32, String)> {
    let num = |r: u32, c: u32| matches!(sheet.cell(r, c, shared).value, Value::Number(_));
    let empty = |r: u32, c: u32| !sheet.has_value(r, c) && sheet.formula(r, c).is_none();
    let text = |a: (u32, u32), b: (u32, u32)| {
        let mut s = String::from("SUM(");
        push_cell(&mut s, a.1 as i64 + 1, a.0 as i64 + 1, false, false);
        if a != b {
            s.push(':');
            push_cell(&mut s, b.1 as i64 + 1, b.0 as i64 + 1, false, false);
        }
        s.push(')');
        s
    };
    let (r0, c0) = (sel.first_row, sel.first_col);
    // Seleções de linha ou coluna inteira: só até onde há dados.
    let r1 = sel.last_row.min(sheet.rows().max(r0 + 1));
    let c1 = sel.last_col.min(sheet.cols().max(c0 + 1));
    if (r0, c0) == (r1, c1) {
        // Para cima: pula os vazios até o bloco de números mais perto.
        let mut r = r0;
        while r > 0 && empty(r - 1, c0) && r0 - r < 100 {
            r -= 1;
        }
        if r > 0 && num(r - 1, c0) {
            let mut top = r - 1;
            while top > 0 && num(top - 1, c0) {
                top -= 1;
            }
            return vec![(r0, c0, text((top, c0), (r0 - 1, c0)))];
        }
        let mut c = c0;
        while c > 0 && empty(r0, c - 1) && c0 - c < 100 {
            c -= 1;
        }
        if c > 0 && num(r0, c - 1) {
            let mut left = c - 1;
            while left > 0 && num(r0, left - 1) {
                left -= 1;
            }
            return vec![(r0, c0, text((r0, left), (r0, c0 - 1)))];
        }
        return vec![(r0, c0, "SUM()".into())];
    }
    let blank_row = (c0..=c1).all(|c| empty(r1, c));
    let blank_col = (r0..=r1).all(|r| empty(r, c1));
    let mut out = Vec::new();
    if r0 == r1 {
        // Uma linha: soma na última vazia ou na célula à direita.
        let (last, at) = if blank_col { (c1 - 1, c1) } else { (c1, c1 + 1) };
        out.push((r0, at, text((r0, c0), (r0, last))));
        return out;
    }
    if blank_row || !blank_col {
        let (last, at) = if blank_row { (r1 - 1, r1) } else { (r1, r1 + 1) };
        let cols = if blank_col && c1 > c0 { c1 - 1 } else { c1 };
        for c in c0..=cols {
            out.push((at, c, text((r0, c), (last, c))));
        }
    }
    if blank_col && c1 > c0 {
        let rows = if blank_row { r1 - 1 } else { r1 };
        for r in r0..=rows {
            out.push((r, c1, text((r, c0), (r, c1 - 1))));
        }
        if blank_row {
            out.push((r1, c1, text((r0, c1), (r1 - 1, c1))));
        }
    }
    out
}

/// A fórmula da célula `from` colada transposta em `to`: os deslocamentos das
/// referências relativas trocam de eixo; as absolutas (`$A$1`) ficam.
pub fn transpose(formula: &str, from: (u32, u32), to: (u32, u32)) -> String {
    let mut out = String::with_capacity(formula.len() + 8);
    let (fr, fc) = (from.0 as i64 + 1, from.1 as i64 + 1);
    let (tr, tc) = (to.0 as i64 + 1, to.1 as i64 + 1);
    walk(formula, |p| match p {
        Piece::Cell(r, _) => {
            let (row, col) = match (r.abs_row, r.abs_col) {
                (true, true) => (r.row, r.col),
                (false, false) => (tr + (r.col - fc), tc + (r.row - fr)),
                // Misto: a parte relativa anda como numa cópia comum.
                (true, false) => (r.row, tc + (r.col - fc)),
                (false, true) => (tr + (r.row - fr), r.col),
            };
            push_cell(&mut out, col, row, r.abs_col, r.abs_row);
        }
        // Colunas inteiras viram linhas inteiras, e o contrário.
        Piece::Col { col, abs: false, .. } => {
            let row = tr + (col - fc);
            if row >= 1 {
                out.push_str(&row.to_string());
            } else {
                out.push_str("#REF!");
            }
        }
        Piece::Row { row, abs: false, .. } => {
            let col = tc + (row - fr);
            if (1..=16_384).contains(&col) {
                push_col(&mut out, col);
            } else {
                out.push_str("#REF!");
            }
        }
        p => push_raw(&mut out, p),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::CellValue;

    fn rg(a: u32, b: u32, c: u32, d: u32) -> CellRange {
        CellRange { first_row: a, first_col: b, last_row: c, last_col: d }
    }

    #[test]
    fn sums() {
        let sh = StringTable::new();
        let mut s = Sheet::new("t");
        // A1 título, A2:B4 números.
        s.set_cell(0, 0, CellValue::Text("título".into()), None, None);
        for r in 1..4 {
            for c in 0..2 {
                s.set_cell(r, c, CellValue::Number(1.0), None, None);
            }
        }
        let t = |sel| {
            autosum(&s, &sh, sel).into_iter().map(|(r, c, f)| format!("{r},{c}={f}")).collect::<Vec<_>>()
        };
        assert_eq!(t(rg(4, 0, 4, 0)), ["4,0=SUM(A2:A4)"]);
        // Com um vazio no meio, a soma vai até ele.
        assert_eq!(t(rg(5, 0, 5, 0)), ["5,0=SUM(A2:A5)"]);
        assert_eq!(t(rg(2, 2, 2, 2)), ["2,2=SUM(A3:B3)"]);
        assert_eq!(t(rg(0, 4, 0, 4)), ["0,4=SUM()"]);
        assert_eq!(t(rg(1, 0, 3, 1)), ["4,0=SUM(A2:A4)", "4,1=SUM(B2:B4)"]);
        assert_eq!(t(rg(1, 0, 4, 1)), ["4,0=SUM(A2:A4)", "4,1=SUM(B2:B4)"]);
        assert_eq!(t(rg(1, 0, 3, 2)), ["1,2=SUM(A2:B2)", "2,2=SUM(A3:B3)", "3,2=SUM(A4:B4)"]);
        assert_eq!(
            t(rg(1, 0, 4, 2)),
            [
                "4,0=SUM(A2:A4)",
                "4,1=SUM(B2:B4)",
                "1,2=SUM(A2:B2)",
                "2,2=SUM(A3:B3)",
                "3,2=SUM(A4:B4)",
                "4,2=SUM(C2:C4)"
            ]
        );
        assert_eq!(t(rg(1, 0, 1, 1)), ["1,2=SUM(A2:B2)"]);
    }

    #[test]
    fn transposed() {
        // C1 (=A1+B1) colada em A3: pega A1 e A2 pelas linhas.
        assert_eq!(transpose("A1+B1", (0, 2), (2, 0)), "A1+A2");
        assert_eq!(transpose("SUM($A$1:$B$2)+D1", (0, 2), (2, 0)), "SUM($A$1:$B$2)+A4");
        assert_eq!(transpose("SUM(A:A)", (0, 2), (4, 0)), "SUM(3:3)");
        assert_eq!(transpose("B1", (1, 0), (0, 0)), "#REF!");
    }
}
