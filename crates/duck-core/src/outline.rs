//! Agrupamento de linhas e colunas (níveis de tópico): intervalos seguidos
//! com o mesmo nível, em vez de um número por linha (uma aba de 1 milhão de
//! linhas com dois grupos guarda dois intervalos).

use std::collections::BTreeSet;

/// Nível de um eixo: (primeiro, último, nível 1–7), em ordem, sem sobrepor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Levels(pub Vec<(u32, u32, u8)>);

/// Um grupo: o intervalo de linhas (ou colunas) com nível ≥ `level`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub first: u32,
    pub last: u32,
    pub level: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outline {
    pub rows: Levels,
    pub cols: Levels,
    /// A linha de resumo (com o botão) fica embaixo (`summaryBelow`).
    pub below: bool,
    /// A coluna de resumo fica à direita (`summaryRight`).
    pub right: bool,
    /// Linhas e colunas de resumo com o grupo fechado (`collapsed`).
    pub closed_rows: BTreeSet<u32>,
    pub closed_cols: BTreeSet<u32>,
}

impl Default for Outline {
    fn default() -> Self {
        Self {
            rows: Levels::default(),
            cols: Levels::default(),
            below: true,
            right: true,
            closed_rows: BTreeSet::new(),
            closed_cols: BTreeSet::new(),
        }
    }
}

impl Levels {
    /// Acrescenta no fim da leitura (os índices chegam em ordem em cada pedaço).
    pub fn push(&mut self, first: u32, last: u32, level: u8) {
        if level == 0 {
            return;
        }
        if let Some(l) = self.0.last_mut()
            && l.2 == level
            && l.1.saturating_add(1) == first
        {
            l.1 = last;
            return;
        }
        self.0.push((first, last, level));
    }

    /// Depois de juntar os pedaços: em ordem e com os vizinhos iguais unidos.
    pub fn finish(&mut self) {
        self.0.sort_by_key(|l| l.0);
        let list = std::mem::take(&mut self.0);
        for (a, b, l) in list {
            self.push(a, b, l);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn max(&self) -> u8 {
        self.0.iter().map(|l| l.2).max().unwrap_or(0)
    }

    pub fn level(&self, i: u32) -> u8 {
        let k = self.0.partition_point(|l| l.1 < i);
        self.0.get(k).filter(|l| l.0 <= i).map_or(0, |l| l.2)
    }

    /// Todos os grupos, por nível e posição.
    pub fn groups(&self) -> Vec<Group> {
        let mut out = Vec::new();
        for level in 1..=self.max() {
            let mut cur: Option<Group> = None;
            for &(a, b, l) in &self.0 {
                if l < level {
                    out.extend(cur.take());
                    continue;
                }
                match &mut cur {
                    Some(g) if g.last + 1 == a => g.last = b,
                    _ => {
                        out.extend(cur.take());
                        cur = Some(Group { first: a, last: b, level });
                    }
                }
            }
            out.extend(cur);
        }
        out
    }

    /// Depois de inserir/apagar linhas ou colunas.
    pub fn shift(&mut self, s: crate::Shift) {
        let list = std::mem::take(&mut self.0);
        for (a, b, l) in list {
            if let Some((a, b)) = s.span(a, b) {
                self.push(a, b, l);
            }
        }
    }
}

impl Group {
    /// Onde fica o botão (a linha ou coluna de resumo). `None` = antes da
    /// primeira.
    pub fn button(&self, after: bool) -> Option<u32> {
        if after { Some(self.last + 1) } else { self.first.checked_sub(1) }
    }
}

impl Outline {
    /// Depois de inserir/apagar linhas ou colunas.
    pub fn shift(&mut self, s: crate::Shift) {
        let (levels, closed) = match s.axis {
            crate::Axis::Rows => (&mut self.rows, &mut self.closed_rows),
            crate::Axis::Cols => (&mut self.cols, &mut self.closed_cols),
        };
        levels.shift(s);
        *closed = closed.iter().filter_map(|&i| s.index(i)).collect();
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.cols.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_groups() {
        // Linhas 2–9 nível 1, com 4–6 no nível 2.
        let mut l = Levels::default();
        for i in 2..=9 {
            l.push(i, i, if (4..=6).contains(&i) { 2 } else { 1 });
        }
        assert_eq!(l.0, [(2, 3, 1), (4, 6, 2), (7, 9, 1)]);
        assert_eq!((l.level(1), l.level(5), l.level(9), l.level(10), l.max()), (0, 2, 1, 0, 2));
        let g = l.groups();
        assert_eq!(g, [Group { first: 2, last: 9, level: 1 }, Group { first: 4, last: 6, level: 2 }]);
        assert_eq!((g[0].button(true), g[0].button(false)), (Some(10), Some(1)));
    }

    #[test]
    fn unordered_parts_and_shift() {
        let mut l = Levels::default();
        l.push(10, 12, 1);
        l.push(5, 9, 1);
        l.finish();
        assert_eq!(l.0, [(5, 12, 1)]);
        let s = crate::Shift { axis: crate::Axis::Rows, at: 7, n: 2, insert: true };
        l.shift(s);
        assert_eq!(l.0, [(5, 14, 1)]);
        l.shift(crate::Shift { axis: crate::Axis::Rows, at: 0, n: 6, insert: false });
        assert_eq!(l.0, [(0, 8, 1)]);
    }
}
