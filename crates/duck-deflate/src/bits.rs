//! Leitor de bits do deflate (LSB primeiro), a partir de qualquer posição de bit.

pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Próximo byte a carregar no buffer.
    pos: usize,
    buf: u64,
    /// Bits válidos em `buf`.
    n: u32,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8], bit: u64) -> Self {
        let mut b = Self { data, pos: (bit / 8) as usize, buf: 0, n: 0 };
        b.refill();
        b.consume((bit % 8) as u32);
        b
    }

    /// Garante pelo menos 56 bits no buffer (completa com zeros além do fim;
    /// [`Self::overrun`] detecta isso).
    #[inline(always)]
    pub fn refill(&mut self) {
        if self.n > 56 {
            return;
        }
        if self.pos + 8 <= self.data.len() {
            let v = u64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().unwrap());
            self.buf |= v << self.n;
            let k = (63 - self.n) >> 3;
            self.pos += k as usize;
            self.n += k * 8;
        } else {
            while self.n <= 56 {
                let b = self.data.get(self.pos).copied().unwrap_or(0);
                self.buf |= (b as u64) << self.n;
                self.pos += 1;
                self.n += 8;
            }
        }
    }

    #[inline(always)]
    pub fn peek(&self, k: u32) -> u32 {
        (self.buf & ((1u64 << k) - 1)) as u32
    }

    #[inline(always)]
    pub fn consume(&mut self, k: u32) {
        debug_assert!(k <= self.n);
        self.buf >>= k;
        self.n -= k;
    }

    /// Lê `k` bits (k ≤ 32), recarregando se preciso.
    #[inline(always)]
    pub fn read(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.refill();
        }
        let v = self.peek(k);
        self.consume(k);
        v
    }

    /// Posição atual em bits desde o início dos dados.
    #[inline]
    pub fn bit_pos(&self) -> u64 {
        self.pos as u64 * 8 - self.n as u64
    }

    /// Leu além do fim dos dados?
    #[inline]
    pub fn overrun(&self) -> bool {
        self.bit_pos() > self.data.len() as u64 * 8
    }

    /// Pula até o próximo byte inteiro.
    pub fn align(&mut self) {
        let r = self.n % 8;
        self.consume(r);
    }

    /// Copia `len` bytes alinhados (bloco sem compressão) para `out`.
    pub fn take_bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        debug_assert_eq!(self.n % 8, 0);
        let start = (self.bit_pos() / 8) as usize;
        let end = start.checked_add(len)?;
        if end > self.data.len() {
            return None;
        }
        self.pos = end;
        self.buf = 0;
        self.n = 0;
        self.refill();
        Some(&self.data[start..end])
    }
}
