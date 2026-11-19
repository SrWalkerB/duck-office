//! Tabelas de Huffman canônicas com consulta em dois níveis.

use crate::bits::Bits;

/// Bits da tabela primária.
pub(crate) const LIT_BITS: u32 = 10;
pub(crate) const DIST_BITS: u32 = 8;
pub(crate) const CL_BITS: u32 = 7;

const LINK: u32 = 1 << 31;

/// Entrada: folha = `símbolo << 4 | tamanho`; ligação = `LINK | deslocamento << 4 | bits da subtabela`;
/// 0 = código inválido.
pub(crate) struct Table {
    t: Vec<u32>,
    bits: u32,
}

#[derive(Debug)]
pub(crate) struct BadCode;

impl Table {
    /// Monta a tabela a partir dos tamanhos de código. Como o zlib, exige
    /// código completo, exceto o caso de um único código de tamanho 1
    /// (`allow_single`, válido para distâncias). Tamanhos todos zero = tabela vazia.
    pub fn build(lengths: &[u8], bits: u32, allow_single: bool) -> Result<Self, BadCode> {
        let mut count = [0u32; 16];
        for &l in lengths {
            if l > 15 {
                return Err(BadCode);
            }
            count[l as usize] += 1;
        }
        count[0] = 0;
        let max_len = (1..16).rev().find(|&l| count[l] > 0).unwrap_or(0) as u32;
        let mut t = vec![0u32; 1 << bits];
        if max_len == 0 {
            return Ok(Self { t, bits });
        }
        let mut left: i32 = 1;
        for &c in &count[1..16] {
            left <<= 1;
            left -= c as i32;
            if left < 0 {
                return Err(BadCode);
            }
        }
        if left > 0 {
            let total: u32 = count.iter().sum();
            if !(allow_single && total == 1 && max_len == 1) {
                return Err(BadCode);
            }
        }
        let mut next = [0u32; 16];
        let mut code = 0u32;
        for l in 1..16 {
            code = (code + count[l - 1]) << 1;
            next[l] = code;
        }
        let sub_bits = max_len.saturating_sub(bits);
        let mask = (1u32 << bits) - 1;
        for (sym, &l) in lengths.iter().enumerate() {
            if l == 0 {
                continue;
            }
            let l = l as u32;
            let c = next[l as usize];
            next[l as usize] += 1;
            let rev = c.reverse_bits() >> (32 - l);
            if l <= bits {
                let mut i = rev;
                while i < (1 << bits) {
                    t[i as usize] = ((sym as u32) << 4) | l;
                    i += 1 << l;
                }
            } else {
                let prefix = (rev & mask) as usize;
                if t[prefix] == 0 {
                    let off = t.len() as u32;
                    t.resize(t.len() + (1 << sub_bits), 0);
                    t[prefix] = LINK | (off << 4) | sub_bits;
                }
                let off = (t[prefix] & !LINK) >> 4;
                let rem = l - bits;
                let mut i = rev >> bits;
                while i < (1 << sub_bits) {
                    t[(off + i) as usize] = ((sym as u32) << 4) | rem;
                    i += 1 << rem;
                }
            }
        }
        Ok(Self { t, bits })
    }

    /// Decodifica um símbolo. Exige pelo menos 15 bits no buffer.
    #[inline(always)]
    pub fn decode(&self, b: &mut Bits) -> Option<u32> {
        let e = self.t[b.peek(self.bits) as usize];
        if e & LINK == 0 {
            let len = e & 15;
            if len == 0 {
                return None;
            }
            b.consume(len);
            return Some(e >> 4);
        }
        b.consume(self.bits);
        let sub = e & 15;
        let off = (e & !LINK) >> 4;
        let e2 = self.t[(off + b.peek(sub)) as usize];
        let len = e2 & 15;
        if len == 0 {
            return None;
        }
        b.consume(len);
        Some(e2 >> 4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversubscribed_and_incomplete() {
        assert!(Table::build(&[1, 1, 1], 7, false).is_err());
        assert!(Table::build(&[1, 2], 7, false).is_err());
        assert!(Table::build(&[1], 7, true).is_ok());
        assert!(Table::build(&[1, 1], 7, false).is_ok());
    }
}
