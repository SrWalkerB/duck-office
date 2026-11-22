//! Escritor de zip mínimo para o pacote OOXML.
//!
//! - Partes não tocadas entram com os **bytes comprimidos originais**
//!   ([`ZipWriter::copy_raw`]): mesmo CRC, sem descompactar.
//! - Partes regravadas são escritas em fluxo: o cabeçalho local vai com
//!   tamanhos zerados e é corrigido no fim ([`ZipWriter::end_part`]), sem
//!   descritor de dados.
//! - Zip64 só quando precisa (parte ≥ 4 GB, arquivo ≥ 4 GB ou > 65535 partes).
//! - Data fixa (1/1/1980, como o Excel grava).

use std::io::{self, Seek, SeekFrom, Write};

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
const LOCATOR64_SIG: u32 = 0x0706_4b50;
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = (1 << 5) | 1;
const MAX32: u64 = 0xFFFF_FFFF;

pub const DEFLATED: u16 = 8;

struct CdEntry {
    name: String,
    method: u16,
    crc: u32,
    csize: u64,
    size: u64,
    offset: u64,
}

pub struct ZipWriter<W: Write + Seek> {
    out: W,
    pos: u64,
    entries: Vec<CdEntry>,
    /// Parte em andamento: (offset do cabeçalho local, cabeçalho com zip64).
    open: Option<(u64, bool)>,
}

fn flags(name: &str) -> u16 {
    if name.is_ascii() { 0 } else { 1 << 11 }
}

impl<W: Write + Seek> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out, pos: 0, entries: Vec::new(), open: None }
    }

    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        self.out.write_all(b)?;
        self.pos += b.len() as u64;
        Ok(())
    }

    fn local_header(&mut self, e: &CdEntry, zip64: bool) -> io::Result<()> {
        let mut h = Vec::with_capacity(30 + e.name.len() + 20);
        h.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        h.extend_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes());
        h.extend_from_slice(&flags(&e.name).to_le_bytes());
        h.extend_from_slice(&e.method.to_le_bytes());
        h.extend_from_slice(&DOS_TIME.to_le_bytes());
        h.extend_from_slice(&DOS_DATE.to_le_bytes());
        h.extend_from_slice(&e.crc.to_le_bytes());
        if zip64 {
            h.extend_from_slice(&u32::MAX.to_le_bytes());
            h.extend_from_slice(&u32::MAX.to_le_bytes());
        } else {
            h.extend_from_slice(&(e.csize as u32).to_le_bytes());
            h.extend_from_slice(&(e.size as u32).to_le_bytes());
        }
        h.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        h.extend_from_slice(&(if zip64 { 20u16 } else { 0u16 }).to_le_bytes());
        h.extend_from_slice(e.name.as_bytes());
        if zip64 {
            h.extend_from_slice(&1u16.to_le_bytes());
            h.extend_from_slice(&16u16.to_le_bytes());
            h.extend_from_slice(&e.size.to_le_bytes());
            h.extend_from_slice(&e.csize.to_le_bytes());
        }
        self.put(&h)
    }

    /// Copia uma parte já comprimida (bytes, CRC e tamanhos do zip original).
    pub fn copy_raw(&mut self, name: &str, method: u16, crc: u32, size: u64, data: &[u8]) -> io::Result<()> {
        let e =
            CdEntry { name: name.to_string(), method, crc, csize: data.len() as u64, size, offset: self.pos };
        let zip64 = e.size >= MAX32 || e.csize >= MAX32;
        self.local_header(&e, zip64)?;
        self.put(data)?;
        self.entries.push(e);
        Ok(())
    }

    /// Começa uma parte escrita em fluxo (comprimida com deflate). `zip64`
    /// reserva espaço para tamanhos de 64 bits (partes que podem passar de 4 GB).
    pub fn begin_part(&mut self, name: &str, zip64: bool) -> io::Result<()> {
        assert!(self.open.is_none(), "parte anterior não terminada");
        let e =
            CdEntry { name: name.to_string(), method: DEFLATED, crc: 0, csize: 0, size: 0, offset: self.pos };
        self.open = Some((self.pos, zip64));
        self.local_header(&e, zip64)?;
        self.entries.push(e);
        Ok(())
    }

    /// Dados comprimidos da parte em andamento.
    pub fn write_data(&mut self, b: &[u8]) -> io::Result<()> {
        self.put(b)
    }

    /// Termina a parte em andamento, corrigindo o cabeçalho local.
    pub fn end_part(&mut self, crc: u32, size: u64) -> io::Result<()> {
        let (offset, zip64) = self.open.take().expect("nenhuma parte aberta");
        let e = self.entries.last_mut().expect("parte aberta");
        let name_len = e.name.len() as u64;
        e.crc = crc;
        e.size = size;
        e.csize = self.pos - offset - 30 - name_len - if zip64 { 20 } else { 0 };
        if !zip64 && (e.size >= MAX32 || e.csize >= MAX32) {
            return Err(io::Error::other(format!("parte {} passou de 4 GB sem zip64", e.name)));
        }
        let (csize, size) = (e.csize, e.size);
        let end = self.pos;
        self.out.seek(SeekFrom::Start(offset + 14))?;
        self.out.write_all(&crc.to_le_bytes())?;
        if zip64 {
            self.out.seek(SeekFrom::Start(offset + 30 + name_len + 4))?;
            self.out.write_all(&size.to_le_bytes())?;
            self.out.write_all(&csize.to_le_bytes())?;
        } else {
            self.out.write_all(&(csize as u32).to_le_bytes())?;
            self.out.write_all(&(size as u32).to_le_bytes())?;
        }
        self.out.seek(SeekFrom::Start(end))?;
        Ok(())
    }

    /// Desiste da parte em andamento: a próxima escrita começa no lugar dela.
    pub fn abort_part(&mut self) -> io::Result<()> {
        let (offset, _) = self.open.take().expect("nenhuma parte aberta");
        self.entries.pop();
        self.out.seek(SeekFrom::Start(offset))?;
        self.pos = offset;
        Ok(())
    }

    /// Escreve o diretório central e devolve o destino e o tamanho final
    /// (que pode ser menor que o já escrito, depois de um `abort_part`).
    pub fn finish(mut self) -> io::Result<(W, u64)> {
        assert!(self.open.is_none(), "parte não terminada");
        let cd_start = self.pos;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let mut extra = Vec::new();
            if e.size >= MAX32 {
                extra.extend_from_slice(&e.size.to_le_bytes());
            }
            if e.csize >= MAX32 {
                extra.extend_from_slice(&e.csize.to_le_bytes());
            }
            if e.offset >= MAX32 {
                extra.extend_from_slice(&e.offset.to_le_bytes());
            }
            let zip64 = !extra.is_empty();
            let mut h = Vec::with_capacity(46 + e.name.len() + 4 + extra.len());
            h.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            h.extend_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes());
            h.extend_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes());
            h.extend_from_slice(&flags(&e.name).to_le_bytes());
            h.extend_from_slice(&e.method.to_le_bytes());
            h.extend_from_slice(&DOS_TIME.to_le_bytes());
            h.extend_from_slice(&DOS_DATE.to_le_bytes());
            h.extend_from_slice(&e.crc.to_le_bytes());
            h.extend_from_slice(&(e.csize.min(MAX32) as u32).to_le_bytes());
            h.extend_from_slice(&(e.size.min(MAX32) as u32).to_le_bytes());
            h.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            h.extend_from_slice(&(if zip64 { extra.len() as u16 + 4 } else { 0 }).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // comentário
            h.extend_from_slice(&0u16.to_le_bytes()); // disco
            h.extend_from_slice(&0u16.to_le_bytes()); // atributos internos
            h.extend_from_slice(&0u32.to_le_bytes()); // atributos externos
            h.extend_from_slice(&(e.offset.min(MAX32) as u32).to_le_bytes());
            h.extend_from_slice(e.name.as_bytes());
            if zip64 {
                h.extend_from_slice(&1u16.to_le_bytes());
                h.extend_from_slice(&(extra.len() as u16).to_le_bytes());
                h.extend_from_slice(&extra);
            }
            self.put(&h)?;
        }
        let cd_size = self.pos - cd_start;
        let n = entries.len() as u64;
        if n > 0xFFFF || cd_start >= MAX32 || cd_size >= MAX32 {
            let eocd64 = self.pos;
            let mut h = Vec::with_capacity(56 + 20);
            h.extend_from_slice(&EOCD64_SIG.to_le_bytes());
            h.extend_from_slice(&44u64.to_le_bytes());
            h.extend_from_slice(&45u16.to_le_bytes());
            h.extend_from_slice(&45u16.to_le_bytes());
            h.extend_from_slice(&0u32.to_le_bytes());
            h.extend_from_slice(&0u32.to_le_bytes());
            h.extend_from_slice(&n.to_le_bytes());
            h.extend_from_slice(&n.to_le_bytes());
            h.extend_from_slice(&cd_size.to_le_bytes());
            h.extend_from_slice(&cd_start.to_le_bytes());
            h.extend_from_slice(&LOCATOR64_SIG.to_le_bytes());
            h.extend_from_slice(&0u32.to_le_bytes());
            h.extend_from_slice(&eocd64.to_le_bytes());
            h.extend_from_slice(&1u32.to_le_bytes());
            self.put(&h)?;
        }
        let mut h = Vec::with_capacity(22);
        h.extend_from_slice(&EOCD_SIG.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&(n.min(0xFFFF) as u16).to_le_bytes());
        h.extend_from_slice(&(n.min(0xFFFF) as u16).to_le_bytes());
        h.extend_from_slice(&(cd_size.min(MAX32) as u32).to_le_bytes());
        h.extend_from_slice(&(cd_start.min(MAX32) as u32).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        self.put(&h)?;
        self.out.flush()?;
        Ok((self.out, self.pos))
    }
}
