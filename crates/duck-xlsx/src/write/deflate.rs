//! Compressão paralela de uma parte (como o `pigz --independent`).
//!
//! A saída é cortada em pedaços; cada pedaço é comprimido numa thread, sem
//! depender dos outros, e termina com um flush de sincronização (bloco vazio
//! que alinha no byte), então os pedaços comprimidos podem ser concatenados.
//! Só o último fecha o stream. O CRC de cada pedaço também é calculado na
//! thread dele, e os CRCs são combinados em ordem.

use std::io::{self, Seek, Write};

use flate2::{Compress, Compression, FlushCompress, Status};
use rayon::prelude::*;

use super::zipw::ZipWriter;

/// Comprime `input` inteiro. `last` fecha o stream deflate.
pub fn compress_chunk(input: &[u8], level: u32, last: bool) -> Vec<u8> {
    let mut c = Compress::new(Compression::new(level), false);
    let mut out = Vec::with_capacity(input.len() / 4 + 1024);
    let flush = if last { FlushCompress::Finish } else { FlushCompress::Sync };
    loop {
        if out.capacity() - out.len() < 64 * 1024 {
            out.reserve(out.capacity().max(256 * 1024));
        }
        let consumed = c.total_in() as usize;
        let st = c.compress_vec(&input[consumed..], &mut out, flush).expect("deflate em memória não falha");
        if last {
            if st == Status::StreamEnd {
                break;
            }
        } else if c.total_in() as usize == input.len() && out.len() < out.capacity() {
            break;
        }
    }
    out
}

/// Recebe os bytes de uma parte em ordem e escreve a parte comprimida no zip.
pub struct PartEncoder {
    level: u32,
    chunk: usize,
    /// Pedaços cheios esperando a próxima rodada de compressão.
    pending: Vec<Vec<u8>>,
    cur: Vec<u8>,
    batch: usize,
    crc: crc32fast::Hasher,
    size: u64,
}

impl PartEncoder {
    pub fn new(level: u32, chunk: usize) -> Self {
        let batch = rayon::current_num_threads().max(1) * 2;
        Self {
            level,
            chunk: chunk.max(64 * 1024),
            pending: Vec::new(),
            cur: Vec::with_capacity(chunk),
            batch,
            crc: crc32fast::Hasher::new(),
            size: 0,
        }
    }

    pub fn write<W: Write + Seek>(&mut self, zip: &mut ZipWriter<W>, mut b: &[u8]) -> io::Result<()> {
        while !b.is_empty() {
            let room = self.chunk - self.cur.len();
            let n = room.min(b.len());
            self.cur.extend_from_slice(&b[..n]);
            b = &b[n..];
            if self.cur.len() == self.chunk {
                let full = std::mem::replace(&mut self.cur, Vec::with_capacity(self.chunk));
                self.pending.push(full);
                if self.pending.len() >= self.batch {
                    self.flush(zip, false)?;
                }
            }
        }
        Ok(())
    }

    /// Comprime os pedaços pendentes em paralelo e escreve em ordem. Com
    /// `last`, inclui o pedaço atual e fecha o stream.
    fn flush<W: Write + Seek>(&mut self, zip: &mut ZipWriter<W>, last: bool) -> io::Result<()> {
        let mut chunks = std::mem::take(&mut self.pending);
        if last {
            chunks.push(std::mem::take(&mut self.cur));
        }
        let n = chunks.len();
        let level = self.level;
        let done: Vec<(Vec<u8>, crc32fast::Hasher, usize)> = chunks
            .par_iter()
            .enumerate()
            .map(|(i, c)| {
                let mut h = crc32fast::Hasher::new();
                h.update(c);
                (compress_chunk(c, level, last && i + 1 == n), h, c.len())
            })
            .collect();
        for (z, h, len) in done {
            zip.write_data(&z)?;
            self.crc.combine(&h);
            self.size += len as u64;
        }
        Ok(())
    }

    /// Termina a parte: devolve (CRC, tamanho descompactado).
    pub fn finish<W: Write + Seek>(mut self, zip: &mut ZipWriter<W>) -> io::Result<(u32, u64)> {
        self.flush(zip, true)?;
        Ok((self.crc.finalize(), self.size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_chunks_concatenate() {
        let text: Vec<u8> =
            (0..200_000u32).flat_map(|i| format!("<c r=\"A{i}\"><v>{i}</v></c>").into_bytes()).collect();
        let mut z = Vec::new();
        for (i, c) in text.chunks(100_000).enumerate() {
            z.extend(compress_chunk(c, 6, i + 1 == text.len().div_ceil(100_000)));
        }
        let back = duck_deflate::inflate(&z).unwrap();
        assert_eq!(back, text);
        // Vazio fecha o stream também.
        assert!(duck_deflate::inflate(&compress_chunk(b"", 6, true)).unwrap().is_empty());
    }
}
