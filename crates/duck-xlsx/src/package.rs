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
    /// Método que não lemos (ou parte criptografada): só copiada ao salvar.
    Other(u16),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    data_offset: usize,
    compressed: usize,
    /// Tamanho descompactado declarado no zip.
    pub size: u64,
    /// CRC32 dos dados descompactados, declarado no zip.
    pub crc32: u32,
    method: Method,
}

impl Entry {
    pub fn is_deflated(&self) -> bool {
        self.method == Method::Deflated
    }

    pub fn compressed_size(&self) -> usize {
        self.compressed
    }

    /// Código do método de compressão no zip.
    pub fn method_code(&self) -> u16 {
        match self.method {
            Method::Stored => 0,
            Method::Deflated => 8,
            Method::Other(m) => m,
        }
    }

    /// A parte pode ser lida (não é de um método desconhecido nem criptografada).
    pub fn readable(&self) -> bool {
        !matches!(self.method, Method::Other(_))
    }
}

pub struct Package {
    mmap: Mmap,
    entries: HashMap<String, Entry>,
    /// `.xlsb`: as partes binárias, apresentadas como XML.
    xlsb: Option<Box<crate::xlsb::Layer>>,
}

impl Package {
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::from_file(&File::open(path)?)
    }

    pub fn from_file(file: &File) -> Result<Self, Error> {
        // SAFETY: o arquivo é só lido. Se outro processo alterá-lo durante a
        // leitura, o conteúdo lido pode ficar inconsistente (como em qualquer
        // leitor), mas não há escrita através do mapeamento.
        let mmap = unsafe { Mmap::map(file)? };
        Self::from_mmap(mmap)
    }

    fn from_mmap(mmap: Mmap) -> Result<Self, Error> {
        // Contêiner OLE: o Excel antigo e os arquivos com senha.
        if mmap.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            return Err(Error::Unsupported(
                "Arquivo do Excel antigo (.xls) ou protegido com senha, que o duck-sheets ainda não lê. No Excel, tire a senha e salve como .xlsx ou .xlsm.",
            ));
        }
        let mut archive =
            zip::ZipArchive::new(Cursor::new(&mmap[..])).map_err(|e| Error::Zip(e.to_string()))?;
        let mut entries = HashMap::with_capacity(archive.len());
        for i in 0..archive.len() {
            let f = archive.by_index_raw(i).map_err(|e| Error::Zip(e.to_string()))?;
            let name = f.name().map_err(|e| Error::Zip(e.to_string()))?.into_owned();
            // Métodos raros (bzip2, zstd...) e partes criptografadas não são
            // lidos, mas ficam na lista para serem copiados ao salvar.
            let header = f.header_start() as usize;
            let code = || mmap.get(header + 8..header + 10).map_or(99, |b| u16::from_le_bytes([b[0], b[1]]));
            let method = match f.compression() {
                _ if f.encrypted() => Method::Other(code()),
                CompressionMethod::Stored => Method::Stored,
                CompressionMethod::Deflated => Method::Deflated,
                _ => Method::Other(code()),
            };
            let data_offset = local_data_offset(&mmap, header)
                .ok_or_else(|| Error::Zip(format!("cabeçalho local inválido em {name}")))?;
            let compressed = f.compressed_size() as usize;
            if data_offset + compressed > mmap.len() {
                return Err(Error::Zip(format!("parte {name} ultrapassa o fim do arquivo")));
            }
            entries.insert(
                name.clone(),
                Entry { name, data_offset, compressed, size: f.size(), crc32: f.crc32(), method },
            );
        }
        let mut pkg = Self { mmap, entries, xlsb: None };
        pkg.xlsb = crate::xlsb::Layer::new(&pkg).map(Box::new);
        Ok(pkg)
    }

    /// A camada do `.xlsb`, se o pacote for um.
    pub(crate) fn xlsb(&self) -> Option<&crate::xlsb::Layer> {
        self.xlsb.as_deref()
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        match &self.xlsb {
            Some(x) => self.entries.get(x.real(name).unwrap_or(name)),
            None => self.entries.get(name),
        }
    }

    /// A parte existe no zip (sem a camada do `.xlsb`).
    pub(crate) fn has_raw(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub(crate) fn raw_names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// As partes na ordem em que estão no arquivo.
    pub fn entries_in_order(&self) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.entries.values().collect();
        v.sort_by_key(|e| e.data_offset);
        v
    }

    /// Lê uma parte inteira (para partes pequenas: workbook, rels, estilos...).
    pub fn read(&self, name: &str) -> Result<Vec<u8>, Error> {
        match self.xlsb.as_ref().and_then(|x| x.read(self, name)) {
            Some(r) => r,
            None => self.read_raw(name),
        }
    }

    /// Como [`Package::read`], sem a camada do `.xlsb`.
    pub(crate) fn read_raw(&self, name: &str) -> Result<Vec<u8>, Error> {
        let e = self.entries.get(name).ok_or_else(|| Error::Missing(name.to_string()))?;
        if !e.readable() {
            return Err(Error::Zip(format!("parte {name} usa compressão ou criptografia não suportada")));
        }
        let mut out = Vec::with_capacity(e.size.min(1 << 30) as usize);
        let mut inf = self.inflater(e);
        inf.read_into(&mut out, usize::MAX)?;
        Ok(out)
    }

    /// Lê uma parte em pedaços, na ordem, descomprimindo em paralelo se ela
    /// for grande.
    pub fn read_pieces(&self, name: &str, mut sink: impl FnMut(&[u8]) + Send) -> Result<(), Error> {
        /// Menor que isso, uma thread basta.
        const PARALLEL: u64 = 4 << 20;
        let e = self.entries.get(name).ok_or_else(|| Error::Missing(name.to_string()))?;
        if !e.readable() {
            return Err(Error::Zip(format!("parte {name} usa compressão ou criptografia não suportada")));
        }
        if e.is_deflated() && e.size >= PARALLEL && rayon::current_num_threads() >= 4 {
            // Um erro no meio não pode repetir o que já foi entregue.
            let mut sent = 0usize;
            let r = duck_deflate::inflate_parallel(
                self.compressed(e),
                Some(e.crc32),
                // Partes pequenas: estes XML comprimem ~25 vezes.
                duck_deflate::Options { part_bytes: 128 << 10, lookahead: 32, stream_bytes: 1 << 20 },
                |b| {
                    sent += b.bytes().len();
                    sink(b.bytes());
                    Ok(())
                },
            );
            match r {
                Ok(_) => return Ok(()),
                Err(_) if sent == 0 => {}
                Err(_) => return Err(Error::Zip(format!("parte {name} corrompida"))),
            }
        }
        let mut inf = self.inflater(e);
        let mut buf = Vec::with_capacity(4 << 20);
        loop {
            buf.clear();
            let eof = inf.read_into(&mut buf, 4 << 20)?;
            sink(&buf);
            if eof {
                return Ok(());
            }
        }
    }

    /// Bytes comprimidos da parte, direto do arquivo mapeado.
    pub fn compressed(&self, e: &Entry) -> &[u8] {
        &self.mmap[e.data_offset..e.data_offset + e.compressed]
    }

    /// Avisa o sistema que os bytes comprimidos de `e` até `upto` não serão
    /// mais lidos: as páginas do arquivo mapeado saem da memória do processo
    /// (contam no RSS / working set) e voltam do disco se forem lidas de novo.
    /// A aba de 1,6 GB de XML tem ~150 MB compactados: sem isso, ficariam
    /// todos na memória do processo até o fim da leitura.
    pub fn release(&self, e: &Entry, upto: usize) {
        const PAGE: usize = 4096;
        let start = e.data_offset.div_ceil(PAGE) * PAGE;
        let end = ((e.data_offset + upto.min(e.compressed)) / PAGE) * PAGE;
        if end <= start {
            return;
        }
        #[cfg(unix)]
        {
            // SAFETY: mapeamento só de leitura de um arquivo que não é
            // alterado por nós: depois do MADV_DONTNEED, uma leitura traz
            // de volta o mesmo conteúdo do arquivo.
            let _ = unsafe {
                self.mmap.unchecked_advise_range(memmap2::UncheckedAdvice::DontNeed, start, end - start)
            };
        }
        #[cfg(windows)]
        {
            // `VirtualUnlock` num intervalo que não está travado tira as
            // páginas do working set (e retorna "erro": ERROR_NOT_LOCKED,
            // o esperado). Elas continuam no cache de arquivos do sistema.
            // SAFETY: o intervalo está dentro do mapeamento, que vive enquanto `self`.
            unsafe {
                windows_sys::Win32::System::Memory::VirtualUnlock(
                    self.mmap.as_ptr().add(start).cast(),
                    end - start,
                );
            }
        }
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
    /// Bytes comprimidos já lidos.
    pub fn consumed(&self) -> usize {
        self.pos
    }

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
