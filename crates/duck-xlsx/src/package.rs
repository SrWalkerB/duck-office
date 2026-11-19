//! O pacote OOXML: um zip mapeado em memória.
//!
//! O crate `zip` só é usado para ler o diretório central (nomes, tamanhos,
//! posições). Os bytes comprimidos de cada parte são lidos direto do
//! mapeamento, sem cópia, e descompactados em streaming (`zlib-rs`).

use std::collections::HashMap;
use std::fs::File;
use std::io::Cursor;
use std::path::Path;

use flate2::{Decompress, FlushDecompress, Status};
use memmap2::Mmap;
use zip::CompressionMethod;

use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Stored,
    Deflated,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    data_offset: usize,
    compressed: usize,
    /// Tamanho descompactado declarado no zip.
    pub size: u64,
    method: Method,
}

pub struct Package {
    mmap: Mmap,
    entries: HashMap<String, Entry>,
}

impl Package {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let file = File::open(path)?;
        // SAFETY: o arquivo é só lido. Se outro processo alterá-lo durante a
        // leitura, o conteúdo lido pode ficar inconsistente (como em qualquer
        // leitor), mas não há escrita através do mapeamento.
        let mmap = unsafe { Mmap::map(&file)? };
        Self::from_mmap(mmap)
    }

    fn from_mmap(mmap: Mmap) -> Result<Self, Error> {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(&mmap[..])).map_err(|e| Error::Zip(e.to_string()))?;
        let mut entries = HashMap::with_capacity(archive.len());
        for i in 0..archive.len() {
            let f = archive.by_index_raw(i).map_err(|e| Error::Zip(e.to_string()))?;
            let name = f.name().map_err(|e| Error::Zip(e.to_string()))?.into_owned();
            let method = match f.compression() {
                CompressionMethod::Stored => Method::Stored,
                CompressionMethod::Deflated => Method::Deflated,
                // Métodos raros (bzip2, zstd...) não aparecem em planilhas do Office.
                _ => continue,
            };
            if f.encrypted() {
                continue;
            }
            let header = f.header_start() as usize;
            let data_offset = local_data_offset(&mmap, header)
                .ok_or_else(|| Error::Zip(format!("cabeçalho local inválido em {name}")))?;
            let compressed = f.compressed_size() as usize;
            if data_offset + compressed > mmap.len() {
                return Err(Error::Zip(format!("parte {name} ultrapassa o fim do arquivo")));
            }
            entries.insert(name.clone(), Entry { name, data_offset, compressed, size: f.size(), method });
        }
        Ok(Self { mmap, entries })
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.get(name)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// Lê uma parte inteira (para partes pequenas: workbook, rels, estilos...).
    pub fn read(&self, name: &str) -> Result<Vec<u8>, Error> {
        let e = self.entries.get(name).ok_or_else(|| Error::Missing(name.to_string()))?;
        let mut out = Vec::with_capacity(e.size.min(1 << 30) as usize);
        let mut inf = self.inflater(e);
        inf.read_into(&mut out, usize::MAX)?;
        Ok(out)
    }

    pub fn inflater(&self, e: &Entry) -> Inflater<'_> {
        Inflater {
            input: &self.mmap[e.data_offset..e.data_offset + e.compressed],
            pos: 0,
            d: (e.method == Method::Deflated).then(|| Decompress::new(false)),
            done: false,
        }
    }
}

/// Início dos dados de uma parte: depois do cabeçalho local de 30 bytes, do
/// nome e do campo extra (que podem diferir dos do diretório central).
fn local_data_offset(buf: &[u8], header: usize) -> Option<usize> {
    let h = buf.get(header..header + 30)?;
    if h[0..4] != [0x50, 0x4b, 0x03, 0x04] {
        return None;
    }
    let name_len = u16::from_le_bytes([h[26], h[27]]) as usize;
    let extra_len = u16::from_le_bytes([h[28], h[29]]) as usize;
    Some(header + 30 + name_len + extra_len)
}

/// Descompactação em streaming de uma parte.
pub struct Inflater<'a> {
    input: &'a [u8],
    pos: usize,
    d: Option<Decompress>,
    done: bool,
}

impl Inflater<'_> {
    /// Acrescenta bytes descompactados em `out` até ele ter `want` bytes ou a
    /// parte acabar. Retorna `true` quando a parte acabou.
    pub fn read_into(&mut self, out: &mut Vec<u8>, want: usize) -> Result<bool, Error> {
        if self.done {
            return Ok(true);
        }
        let Some(d) = self.d.as_mut() else {
            // Sem compressão: copia direto.
            let n = (want.saturating_sub(out.len())).min(self.input.len() - self.pos);
            out.extend_from_slice(&self.input[self.pos..self.pos + n]);
            self.pos += n;
            self.done = self.pos >= self.input.len();
            return Ok(self.done);
        };
        while out.len() < want {
            if out.capacity() == out.len() {
                let grow = (want - out.len()).clamp(64 * 1024, 16 * 1024 * 1024);
                out.reserve(grow);
            }
            let (in0, out0) = (d.total_in(), d.total_out());
            let status = d
                .decompress_vec(&self.input[self.pos..], out, FlushDecompress::None)
                .map_err(|e| Error::Format(format!("deflate inválido: {e}")))?;
            self.pos += (d.total_in() - in0) as usize;
            if status == Status::StreamEnd {
                self.done = true;
                return Ok(true);
            }
            if d.total_in() == in0 && d.total_out() == out0 {
                if self.pos >= self.input.len() {
                    return Err(Error::Format("parte compactada truncada".into()));
                }
                // Sem espaço de saída: o próximo loop reserva mais.
                out.reserve(64 * 1024);
            }
        }
        Ok(false)
    }
}
