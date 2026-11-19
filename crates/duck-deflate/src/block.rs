//! Cabeçalhos e decodificação de blocos deflate (RFC 1951).

use std::sync::OnceLock;

use crate::bits::Bits;
use crate::huffman::{CL_BITS, DIST_BITS, LIT_BITS, Table};

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
const LEN_EXTRA: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Janela de referências do deflate.
pub(crate) const WINDOW: usize = 32 * 1024;
/// Valores ≥ `MARKER` na saída de 16 bits apontam para a janela desconhecida.
pub(crate) const MARKER: u16 = 256;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Corrupt;

pub(crate) enum Kind {
    Stored,
    Fixed,
    Dynamic(Box<(Table, Table)>),
}

pub(crate) struct Header {
    pub last: bool,
    pub kind: Kind,
}

fn fixed() -> &'static (Table, Table) {
    static FIXED: OnceLock<(Table, Table)> = OnceLock::new();
    FIXED.get_or_init(|| {
        let mut lit = [0u8; 288];
        lit[..144].fill(8);
        lit[144..256].fill(9);
        lit[256..280].fill(7);
        lit[280..].fill(8);
        (Table::build(&lit, LIT_BITS, false).unwrap(), Table::build(&[5u8; 32], DIST_BITS, false).unwrap())
    })
}

pub(crate) fn read_header(b: &mut Bits) -> Result<Header, Corrupt> {
    b.refill();
    let last = b.read(1) == 1;
    let kind = match b.read(2) {
        0 => Kind::Stored,
        1 => Kind::Fixed,
        2 => Kind::Dynamic(Box::new(read_dynamic_tables(b)?)),
        _ => return Err(Corrupt),
    };
    Ok(Header { last, kind })
}

/// Lê as tabelas de um bloco dinâmico (depois dos 3 bits de cabeçalho).
pub(crate) fn read_dynamic_tables(b: &mut Bits) -> Result<(Table, Table), Corrupt> {
    b.refill();
    let hlit = b.read(5) as usize + 257;
    let hdist = b.read(5) as usize + 1;
    let hclen = b.read(4) as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(Corrupt);
    }
    let mut cl = [0u8; 19];
    for &i in &CL_ORDER[..hclen] {
        cl[i] = b.read(3) as u8;
    }
    let clt = Table::build(&cl, CL_BITS, false).map_err(|_| Corrupt)?;
    let mut lens = [0u8; 286 + 30];
    let total = hlit + hdist;
    let mut i = 0;
    while i < total {
        b.refill();
        let sym = clt.decode(b).ok_or(Corrupt)?;
        match sym {
            0..=15 => {
                lens[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err(Corrupt);
                }
                let prev = lens[i - 1];
                let n = 3 + b.read(2) as usize;
                if i + n > total {
                    return Err(Corrupt);
                }
                lens[i..i + n].fill(prev);
                i += n;
            }
            17 | 18 => {
                let n = if sym == 17 { 3 + b.read(3) as usize } else { 11 + b.read(7) as usize };
                if i + n > total {
                    return Err(Corrupt);
                }
                i += n;
            }
            _ => return Err(Corrupt),
        }
    }
    if lens[256] == 0 {
        return Err(Corrupt);
    }
    let lit = Table::build(&lens[..hlit], LIT_BITS, false).map_err(|_| Corrupt)?;
    let dist = Table::build(&lens[hlit..total], DIST_BITS, true).map_err(|_| Corrupt)?;
    if b.overrun() {
        return Err(Corrupt);
    }
    Ok((lit, dist))
}

/// Destino da saída decodificada.
pub(crate) trait Out {
    fn lit(&mut self, b: u8);
    fn copy(&mut self, dist: usize, len: usize) -> Result<(), Corrupt>;
    fn bytes(&mut self, s: &[u8]);
}

/// Cópia LZ77 dentro de um vetor (`dist` ≤ tamanho atual), incluindo
/// sobreposição: a região de `start` até o fim é periódica com período
/// `dist`, então cada rodada pode copiar o dobro da anterior.
#[inline(always)]
fn copy_within<T: Copy>(v: &mut Vec<T>, dist: usize, len: usize) {
    let start = v.len() - dist;
    if dist >= len {
        v.extend_from_within(start..start + len);
        return;
    }
    v.reserve(len);
    let mut left = len;
    while left > 0 {
        let k = left.min(v.len() - start);
        v.extend_from_within(start..start + k);
        left -= k;
    }
}

/// Saída em bytes: o vetor já contém a janela anterior (até 32 KB).
impl Out for Vec<u8> {
    #[inline(always)]
    fn lit(&mut self, b: u8) {
        self.push(b);
    }

    #[inline(always)]
    fn copy(&mut self, dist: usize, len: usize) -> Result<(), Corrupt> {
        if dist > self.len() {
            return Err(Corrupt);
        }
        copy_within(self, dist, len);
        Ok(())
    }

    fn bytes(&mut self, s: &[u8]) {
        self.extend_from_slice(s);
    }
}

/// Saída em símbolos de 16 bits para quando a janela anterior é desconhecida:
/// referências a antes do início viram marcadores `MARKER + índice na janela`.
impl Out for Vec<u16> {
    #[inline(always)]
    fn lit(&mut self, b: u8) {
        self.push(b as u16);
    }

    #[inline(always)]
    fn copy(&mut self, dist: usize, len: usize) -> Result<(), Corrupt> {
        if dist > WINDOW {
            return Err(Corrupt);
        }
        if dist <= self.len() {
            copy_within(self, dist, len);
            return Ok(());
        }
        let n = self.len() as isize;
        for i in 0..len as isize {
            let p = n + i - dist as isize;
            let v = if p < 0 { MARKER + (WINDOW as isize + p) as u16 } else { self[p as usize] };
            self.push(v);
        }
        Ok(())
    }

    fn bytes(&mut self, s: &[u8]) {
        self.extend(s.iter().map(|&b| b as u16));
    }
}

/// Decodifica um bloco (cabeçalho já lido) para `out`.
pub(crate) fn decode_block<O: Out>(b: &mut Bits, h: &Header, out: &mut O) -> Result<(), Corrupt> {
    let (lit, dist) = match &h.kind {
        Kind::Stored => {
            b.align();
            let len = b.read(16);
            let nlen = b.read(16);
            if len != !nlen & 0xffff {
                return Err(Corrupt);
            }
            let s = b.take_bytes(len as usize).ok_or(Corrupt)?;
            out.bytes(s);
            return Ok(());
        }
        Kind::Fixed => {
            let f = fixed();
            (&f.0, &f.1)
        }
        Kind::Dynamic(t) => (&t.0, &t.1),
    };
    loop {
        // 56 bits cobrem o pior caso: 15 (símbolo) + 5 + 15 (distância) + 13.
        b.refill();
        if b.overrun() {
            return Err(Corrupt);
        }
        let sym = lit.decode(b).ok_or(Corrupt)?;
        if sym < 256 {
            out.lit(sym as u8);
            continue;
        }
        if sym == 256 {
            break;
        }
        let li = sym as usize - 257;
        if li >= 29 {
            return Err(Corrupt);
        }
        let len = LEN_BASE[li] as usize + b.peek(LEN_EXTRA[li] as u32) as usize;
        b.consume(LEN_EXTRA[li] as u32);
        let ds = dist.decode(b).ok_or(Corrupt)? as usize;
        if ds >= 30 {
            return Err(Corrupt);
        }
        let d = DIST_BASE[ds] as usize + b.peek(DIST_EXTRA[ds] as u32) as usize;
        b.consume(DIST_EXTRA[ds] as u32);
        out.copy(d, len)?;
    }
    if b.overrun() {
        return Err(Corrupt);
    }
    Ok(())
}
