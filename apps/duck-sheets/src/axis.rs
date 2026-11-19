//! Eixo esparso de linhas ou colunas: tamanho padrão + exceções.
//!
//! Uma aba de 1 milhão de linhas em que só 15 mil têm altura própria ocupa
//! ~15 mil entradas, não 1 milhão (ADR-007): `offset(i)` e `index_at(pos)`
//! saem de buscas binárias nas exceções e de somas prefixadas das diferenças.

/// Tamanhos em pixels lógicos a 100% de zoom (96 DPI).
#[derive(Clone, Debug, Default)]
pub struct Axis {
    len: u32,
    default: u32,
    /// Índices com tamanho diferente do padrão, em ordem.
    idx: Vec<u32>,
    size: Vec<u32>,
    /// `prefix[k]` = soma de `size[j] - default` para `j < k`.
    prefix: Vec<i64>,
}

impl Axis {
    /// `overrides`: (índice, tamanho), em qualquer ordem; o último vence.
    pub fn new(len: u32, default: u32, overrides: impl IntoIterator<Item = (u32, u32)>) -> Self {
        let mut ov: Vec<(u32, u32)> = overrides.into_iter().filter(|&(i, _)| i < len).collect();
        // Ordena mantendo a ordem de chegada entre iguais; depois fica o último.
        ov.sort_by_key(|&(i, _)| i);
        let mut idx = Vec::with_capacity(ov.len());
        let mut size: Vec<u32> = Vec::with_capacity(ov.len());
        for (i, s) in ov {
            if idx.last() == Some(&i) {
                *size.last_mut().unwrap() = s;
            } else {
                idx.push(i);
                size.push(s);
            }
        }
        // Exceções iguais ao padrão não precisam ficar.
        let keep: Vec<bool> = size.iter().map(|&s| s != default).collect();
        let mut k = keep.iter();
        idx.retain(|_| *k.next().unwrap());
        let mut k = keep.iter();
        size.retain(|_| *k.next().unwrap());
        idx.shrink_to_fit();
        size.shrink_to_fit();
        let mut prefix = Vec::with_capacity(idx.len() + 1);
        let mut acc = 0i64;
        prefix.push(0);
        for &s in &size {
            acc += s as i64 - default as i64;
            prefix.push(acc);
        }
        Self { len, default, idx, size, prefix }
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn default_size(&self) -> u32 {
        self.default
    }

    pub fn size(&self, i: u32) -> u32 {
        match self.idx.binary_search(&i) {
            Ok(k) => self.size[k],
            Err(_) => self.default,
        }
    }

    /// Soma dos tamanhos de `[0, i)`.
    pub fn offset(&self, i: u32) -> i64 {
        let i = i.min(self.len);
        let k = self.idx.partition_point(|&x| x < i);
        i as i64 * self.default as i64 + self.prefix[k]
    }

    pub fn total(&self) -> i64 {
        self.offset(self.len)
    }

    /// Item que contém a posição `pos` (o último cujo início é ≤ `pos`, então
    /// itens escondidos — tamanho 0 — nunca são escolhidos no meio).
    pub fn index_at(&self, pos: i64) -> u32 {
        if self.len == 0 || pos <= 0 {
            return self.first_visible(0);
        }
        let (mut lo, mut hi) = (0u32, self.len - 1);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if self.offset(mid) <= pos {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// Primeiro item visível a partir de `i` (ou `i`, se nenhum).
    pub fn first_visible(&self, i: u32) -> u32 {
        let mut j = i;
        while j < self.len && self.size(j) == 0 {
            j += 1;
        }
        if j < self.len { j } else { i.min(self.len.saturating_sub(1)) }
    }

    /// Próximo item visível depois de `i` (ou `i`, se não houver).
    pub fn next_visible(&self, i: u32) -> u32 {
        let mut j = i + 1;
        while j < self.len {
            if self.size(j) > 0 {
                return j;
            }
            j += 1;
        }
        i
    }

    /// Item visível anterior a `i` (ou `i`, se não houver).
    pub fn prev_visible(&self, i: u32) -> u32 {
        let mut j = i;
        while j > 0 {
            j -= 1;
            if self.size(j) > 0 {
                return j;
            }
        }
        i
    }

    #[cfg(test)]
    pub fn heap_bytes(&self) -> usize {
        self.idx.capacity() * 4 + self.size.capacity() * 4 + self.prefix.capacity() * 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_dense_sums() {
        let len = 5000u32;
        let sizes: Vec<u32> = (0..len)
            .map(|i| match i % 97 {
                0 => 0, // escondida
                5 => 40,
                _ => 20,
            })
            .collect();
        let ov: Vec<(u32, u32)> = sizes.iter().enumerate().map(|(i, &s)| (i as u32, s)).collect();
        let axis = Axis::new(len, 20, ov);
        assert!(axis.heap_bytes() < 5000 * 4, "deveria guardar só as exceções");
        let mut acc = 0i64;
        for i in 0..len {
            assert_eq!(axis.offset(i), acc, "offset({i})");
            assert_eq!(axis.size(i), sizes[i as usize]);
            if sizes[i as usize] > 0 {
                assert_eq!(axis.index_at(acc), i);
                assert_eq!(axis.index_at(acc + sizes[i as usize] as i64 - 1), i);
            }
            acc += sizes[i as usize] as i64;
        }
        assert_eq!(axis.total(), acc);
        assert_eq!(axis.index_at(acc + 100), len - 1);
        // Linha 0 escondida: a primeira visível é a 1.
        assert_eq!(axis.index_at(0), 1);
        assert_eq!(axis.next_visible(96), 98);
        assert_eq!(axis.prev_visible(98), 96);
    }

    #[test]
    fn last_override_wins() {
        let a = Axis::new(10, 20, [(3, 30), (3, 50), (4, 20)]);
        assert_eq!(a.size(3), 50);
        assert_eq!(a.size(4), 20);
        assert_eq!(a.total(), 10 * 20 + 30);
    }
}
