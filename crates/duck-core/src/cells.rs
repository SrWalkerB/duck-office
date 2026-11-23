//! Mover um bloco de células e inserir ou excluir só células (deslocando as
//! vizinhas): onde cada célula, referência e intervalo vai parar.
//!
//! O conteúdo das células anda por comandos comuns; aqui fica a geometria, e o
//! registro para o gravador ajustar o que está no XML da aba fora das células
//! (formatação condicional, validação, links).

use crate::{Axis, CellRange, Sheet, Shift};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellOp {
    /// Move as células de `src` `dr` linhas e `dc` colunas.
    Move { src: CellRange, dr: i64, dc: i64 },
    /// Insere ou exclui células: só as colunas (ou linhas) `lo..=hi` andam.
    Shift { shift: Shift, lo: u32, hi: u32 },
}

fn inside(a: &CellRange, b: &CellRange) -> bool {
    a.first_row >= b.first_row
        && a.last_row <= b.last_row
        && a.first_col >= b.first_col
        && a.last_col <= b.last_col
}

/// Interseção de dois intervalos.
pub fn intersect(a: &CellRange, b: &CellRange) -> Option<CellRange> {
    let r = CellRange {
        first_row: a.first_row.max(b.first_row),
        first_col: a.first_col.max(b.first_col),
        last_row: a.last_row.min(b.last_row),
        last_col: a.last_col.min(b.last_col),
    };
    (r.first_row <= r.last_row && r.first_col <= r.last_col).then_some(r)
}

/// `a` sem a parte em `b`, em até quatro pedaços.
pub fn subtract(a: &CellRange, b: &CellRange) -> Vec<CellRange> {
    let Some(x) = intersect(a, b) else { return vec![*a] };
    let mut out = Vec::new();
    if a.first_row < x.first_row {
        out.push(CellRange { last_row: x.first_row - 1, ..*a });
    }
    if x.last_row < a.last_row {
        out.push(CellRange { first_row: x.last_row + 1, ..*a });
    }
    if a.first_col < x.first_col {
        out.push(CellRange { first_row: x.first_row, last_row: x.last_row, last_col: x.first_col - 1, ..*a });
    }
    if x.last_col < a.last_col {
        out.push(CellRange { first_row: x.first_row, last_row: x.last_row, first_col: x.last_col + 1, ..*a });
    }
    out
}

fn translate(r: &CellRange, dr: i64, dc: i64) -> Option<CellRange> {
    let row = |x: u32| u32::try_from(x as i64 + dr).ok().filter(|&x| x < crate::shift::MAX_ROWS);
    let col = |x: u32| u32::try_from(x as i64 + dc).ok().filter(|&x| x < crate::shift::MAX_COLS);
    Some(CellRange {
        first_row: row(r.first_row)?,
        first_col: col(r.first_col)?,
        last_row: row(r.last_row)?,
        last_col: col(r.last_col)?,
    })
}

impl CellOp {
    /// A operação que desfaz (o que foi coberto ou excluído não volta: o
    /// desfazer guarda as células).
    pub fn inverse(&self) -> CellOp {
        match *self {
            CellOp::Move { src, dr, dc } => {
                CellOp::Move { src: translate(&src, dr, dc).unwrap_or(src), dr: -dr, dc: -dc }
            }
            CellOp::Shift { shift, lo, hi } => CellOp::Shift { shift: shift.inverse(), lo, hi },
        }
    }

    /// Para onde vai o bloco movido.
    pub fn target(&self) -> Option<CellRange> {
        match self {
            CellOp::Move { src, dr, dc } => translate(src, *dr, *dc),
            CellOp::Shift { .. } => None,
        }
    }

    /// Tudo o que muda de lugar, sai ou é coberto.
    pub fn area(&self) -> CellRange {
        match *self {
            CellOp::Move { src, .. } => {
                let t = self.target().unwrap_or(src);
                CellRange {
                    first_row: src.first_row.min(t.first_row),
                    first_col: src.first_col.min(t.first_col),
                    last_row: src.last_row.max(t.last_row),
                    last_col: src.last_col.max(t.last_col),
                }
            }
            CellOp::Shift { shift, lo, hi } => match shift.axis {
                Axis::Rows => CellRange {
                    first_row: shift.at,
                    last_row: crate::shift::MAX_ROWS - 1,
                    first_col: lo,
                    last_col: hi,
                },
                Axis::Cols => CellRange {
                    first_row: lo,
                    last_row: hi,
                    first_col: shift.at,
                    last_col: crate::shift::MAX_COLS - 1,
                },
            },
        }
    }

    /// A parte do eixo que cruza a faixa (`None` em [`CellOp::Move`]).
    fn in_band(&self, r: &CellRange) -> Option<bool> {
        let CellOp::Shift { shift, lo, hi } = self else { return None };
        let (a, b) = match shift.axis {
            Axis::Rows => (r.first_col, r.last_col),
            Axis::Cols => (r.first_row, r.last_row),
        };
        Some(a >= *lo && b <= *hi)
    }

    /// Posição nova de uma célula (`None` = excluída ou coberta).
    pub fn cell(&self, row: u32, col: u32) -> Option<(u32, u32)> {
        match self {
            CellOp::Move { src, dr, dc } => {
                if src.contains(row, col) {
                    return Some(((row as i64 + dr) as u32, (col as i64 + dc) as u32));
                }
                if self.target().is_some_and(|t| t.contains(row, col)) {
                    return None;
                }
                Some((row, col))
            }
            CellOp::Shift { shift, .. } => {
                let one = CellRange { first_row: row, first_col: col, last_row: row, last_col: col };
                if self.in_band(&one) != Some(true) {
                    return Some((row, col));
                }
                match shift.axis {
                    Axis::Rows => shift.index(row).map(|r| (r, col)),
                    Axis::Cols => shift.index(col).map(|c| (row, c)),
                }
            }
        }
    }

    /// Referência de fórmula a um intervalo: anda inteira, cresce ou encolhe
    /// por um lado, ou fica. `None` = tudo o que ela apontava saiu.
    pub fn reference(&self, r: CellRange) -> Option<CellRange> {
        match self {
            CellOp::Move { src, dr, dc } => {
                if inside(&r, src) {
                    return translate(&r, *dr, *dc);
                }
                if self.target().is_some_and(|t| inside(&r, &t)) {
                    return None;
                }
                let mut out = r;
                let add = |x: u32, d: i64| (x as i64 + d).max(0) as u32;
                // Uma ponta do intervalo foi movida na direção dele.
                if *dc == 0 && r.first_col >= src.first_col && r.last_col <= src.last_col {
                    let (a, b) = (
                        (src.first_row..=src.last_row).contains(&r.first_row),
                        (src.first_row..=src.last_row).contains(&r.last_row),
                    );
                    if a && !b {
                        out.first_row = add(r.first_row, *dr);
                    } else if b && !a {
                        out.last_row = add(r.last_row, *dr);
                    }
                }
                if *dr == 0 && r.first_row >= src.first_row && r.last_row <= src.last_row {
                    let (a, b) = (
                        (src.first_col..=src.last_col).contains(&r.first_col),
                        (src.first_col..=src.last_col).contains(&r.last_col),
                    );
                    if a && !b {
                        out.first_col = add(r.first_col, *dc);
                    } else if b && !a {
                        out.last_col = add(r.last_col, *dc);
                    }
                }
                Some(if out.first_row <= out.last_row && out.first_col <= out.last_col { out } else { r })
            }
            CellOp::Shift { shift, .. } => {
                if self.in_band(&r) == Some(true) {
                    shift.range(r)
                } else {
                    Some(r)
                }
            }
        }
    }

    /// Intervalo com formato (formatação condicional, validação, link): a
    /// parte que anda se separa da que fica.
    pub fn split(&self, r: CellRange) -> Vec<CellRange> {
        match self {
            CellOp::Move { src, dr, dc } => {
                let mut out = Vec::new();
                for p in subtract(&r, src) {
                    match self.target() {
                        Some(t) => out.extend(subtract(&p, &t)),
                        None => out.push(p),
                    }
                }
                if let Some(x) = intersect(&r, src)
                    && let Some(m) = translate(&x, *dr, *dc)
                {
                    out.push(m);
                }
                out
            }
            CellOp::Shift { shift, lo, hi } => {
                let band = match shift.axis {
                    Axis::Rows => CellRange {
                        first_row: 0,
                        last_row: crate::shift::MAX_ROWS - 1,
                        first_col: *lo,
                        last_col: *hi,
                    },
                    Axis::Cols => CellRange {
                        first_row: *lo,
                        last_row: *hi,
                        first_col: 0,
                        last_col: crate::shift::MAX_COLS - 1,
                    },
                };
                let Some(x) = intersect(&r, &band) else { return vec![r] };
                let new = shift.range(x);
                // Nada andou: o intervalo fica inteiro.
                if new == Some(x) {
                    return vec![r];
                }
                let mut out = subtract(&r, &band);
                out.extend(new);
                out
            }
        }
    }
}

impl Sheet {
    /// Registra a operação (para o gravador) ou, ao desfazer, tira o registro.
    pub(crate) fn note_cell_op(&mut self, op: CellOp, undo: bool) {
        let at = self.edits.shifts.len();
        let list = &mut self.edits.cell_ops;
        if undo {
            if list.last() == Some(&(at, op)) {
                list.pop();
                return;
            }
            // Já foi salvo: o arquivo está com a operação feita.
            list.push((at, op.inverse()));
            return;
        }
        list.push((at, op));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rg(a: u32, b: u32, c: u32, d: u32) -> CellRange {
        CellRange { first_row: a, first_col: b, last_row: c, last_col: d }
    }

    #[test]
    fn moves() {
        // B2:C3 vai para E5:F6.
        let m = CellOp::Move { src: rg(1, 1, 2, 2), dr: 3, dc: 3 };
        assert_eq!(m.cell(1, 1), Some((4, 4)));
        assert_eq!(m.cell(0, 0), Some((0, 0)));
        assert_eq!(m.cell(5, 5), None);
        assert_eq!(m.reference(rg(1, 1, 2, 1)), Some(rg(4, 4, 5, 4)));
        assert_eq!(m.reference(rg(4, 4, 5, 5)), None);
        assert_eq!(m.reference(rg(0, 0, 9, 9)), Some(rg(0, 0, 9, 9)));
        let parts = m.split(rg(0, 1, 3, 1));
        assert_eq!(parts, vec![rg(0, 1, 0, 1), rg(3, 1, 3, 1), rg(4, 4, 5, 4)]);
        assert_eq!(m.inverse(), CellOp::Move { src: rg(4, 4, 5, 5), dr: -3, dc: -3 });
        // A última célula de A1:A5 desce duas linhas: o intervalo cresce.
        let m = CellOp::Move { src: rg(4, 0, 4, 0), dr: 2, dc: 0 };
        assert_eq!(m.reference(rg(0, 0, 4, 0)), Some(rg(0, 0, 6, 0)));
    }

    #[test]
    fn shifts() {
        // Insere 2 células em B3:C3, para baixo.
        let s = Shift { axis: Axis::Rows, at: 2, n: 2, insert: true };
        let op = CellOp::Shift { shift: s, lo: 1, hi: 2 };
        assert_eq!(op.cell(2, 1), Some((4, 1)));
        assert_eq!(op.cell(2, 3), Some((2, 3)));
        assert_eq!(op.reference(rg(0, 1, 9, 2)), Some(rg(0, 1, 11, 2)));
        assert_eq!(op.reference(rg(0, 0, 9, 2)), Some(rg(0, 0, 9, 2)));
        assert_eq!(op.split(rg(5, 0, 5, 3)), vec![rg(5, 0, 5, 0), rg(5, 3, 5, 3), rg(7, 1, 7, 2)]);
        assert_eq!(op.split(rg(0, 0, 1, 3)), vec![rg(0, 0, 1, 3)]);
        // Exclui B3:B4, para cima.
        let s = Shift { axis: Axis::Rows, at: 2, n: 2, insert: false };
        let op = CellOp::Shift { shift: s, lo: 1, hi: 1 };
        assert_eq!(op.cell(3, 1), None);
        assert_eq!(op.cell(6, 1), Some((4, 1)));
        assert_eq!(op.reference(rg(2, 1, 3, 1)), None);
        assert_eq!(op.reference(rg(0, 1, 9, 1)), Some(rg(0, 1, 7, 1)));
        // Para a direita.
        let s = Shift { axis: Axis::Cols, at: 1, n: 1, insert: true };
        let op = CellOp::Shift { shift: s, lo: 4, hi: 4 };
        assert_eq!(op.cell(4, 1), Some((4, 2)));
        assert_eq!(op.cell(5, 1), Some((5, 1)));
    }
}
