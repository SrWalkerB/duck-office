//! Salvar `.xlsx` / `.xlsm` por remendo (ADR-006, F3).
//!
//! - Partes não tocadas: bytes comprimidos originais, copiados.
//! - Abas editadas: XML original relido em fluxo (inflate paralelo nas
//!   grandes), remendado só onde mudou ([`patch`]) e comprimido em paralelo
//!   ([`deflate`]).
//! - Valores mudados: `fullCalcOnLoad` no `workbook.xml` (o Excel recalcula
//!   ao abrir; o motor de cálculo próprio vem na F4). Fórmulas mudadas: o
//!   `calcChain.xml` sai (desatualizado, faria o Excel acusar erro).
//! - [`save_file`]: grava num temporário, **relê e confere** as abas
//!   regravadas contra o modelo e só então substitui o destino.

mod blank;
mod cellxml;
mod deflate;
mod patch;
mod regen;
mod styles;
mod zipw;

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use duck_core::{Sheet, StringTable, Styles};
use rayon::prelude::*;

use crate::xml::{attr, find_tag, for_each_attr};
use crate::{Book, Error};
use deflate::PartEncoder;
use zipw::ZipWriter;

pub use blank::write_blank;
pub use deflate::compress_chunk;

#[derive(Clone, Debug)]
pub struct SaveOptions {
    /// Nível do deflate nas partes regravadas (1 = rápido, 6 = padrão do zlib).
    pub level: u32,
    /// Tamanho dos pedaços comprimidos em paralelo.
    pub chunk_bytes: usize,
    /// Escreve toda célula de toda aba a partir do modelo (teste do gravador).
    pub rewrite_all: bool,
    /// [`save_file`]: relê e confere as abas regravadas antes de substituir.
    pub verify: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { level: 6, chunk_bytes: 4 << 20, rewrite_all: false, verify: true }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SaveReport {
    pub wall: Duration,
    pub parts_copied: usize,
    pub parts_rewritten: usize,
    /// Abas regravadas (índices na ordem do workbook).
    pub sheets_rewritten: Vec<usize>,
    pub cells_written: u64,
    /// XML original lido das abas regravadas.
    pub xml_in: u64,
    /// XML escrito nas abas regravadas.
    pub xml_out: u64,
    pub bytes: u64,
    pub calc_chain_removed: bool,
    pub full_calc: bool,
    /// Releitura e conferência ([`save_file`]).
    pub verify: Duration,
}

/// Abas grandes usam o inflate paralelo para reler o original.
const PARALLEL_INFLATE_MIN: usize = 8 * 1024 * 1024;

impl Book {
    /// Grava o pacote com as abas de `sheets` (na ordem do workbook; `None`
    /// = aba não carregada, copiada como está) em `out`.
    pub fn save(
        &self,
        sheets: &[Option<&Sheet>],
        shared: &StringTable,
        styles: &Styles,
        out: &Path,
        opts: &SaveOptions,
    ) -> Result<SaveReport, Error> {
        let t0 = Instant::now();
        let mut rep = SaveReport::default();
        let edited = |s: &&Sheet| opts.rewrite_all || !s.edits().is_empty();
        let mut by_path: HashMap<&str, (usize, &Sheet)> = HashMap::new();
        for (i, m) in self.meta.sheets.iter().enumerate() {
            if let Some(Some(s)) = sheets.get(i)
                && edited(s)
            {
                by_path.insert(m.path.as_str(), (i, *s));
            }
        }
        let new_styles = styles.has_new();
        let values = sheets.iter().flatten().any(|s| s.edits().values || s.edits().formulas);
        let formulas = sheets.iter().flatten().any(|s| s.edits().formulas);
        let calc_chain = self.meta.calc_chain.as_deref().filter(|_| formulas);

        // Partes pequenas remendadas: workbook, content types e rels.
        let mut small: HashMap<String, Vec<u8>> = HashMap::new();
        if values {
            let mut wb = self.pkg.read(&self.meta.path)?;
            // Nomes definidos (áreas de impressão, intervalos nomeados) andam
            // com as linhas/colunas inseridas ou apagadas.
            let changes: Vec<(&str, &[duck_core::Shift])> = self
                .meta
                .sheets
                .iter()
                .zip(sheets)
                .filter_map(|(m, s)| {
                    s.filter(|s| s.edits().structure).map(|s| (m.name.as_str(), &s.edits().shifts[..]))
                })
                .collect();
            if !changes.is_empty() {
                wb = regen::defined_names(&wb, &changes);
            }
            small.insert(self.meta.path.clone(), force_full_calc(&wb));
            rep.full_calc = true;
        }
        // Tabelas com filtro, intervalo ou colunas mudadas.
        for s in sheets.iter().flatten() {
            for &t in &s.edits().tables {
                if let Some(table) = s.layout.tables.get(t as usize)
                    && !table.part.is_empty()
                    && let Ok(xml) = self.pkg.read(&table.part)
                {
                    small.insert(table.part.clone(), patch::patch_table(&xml, table));
                }
            }
        }
        if new_styles && let Some(path) = &self.meta.styles {
            let xml = self.pkg.read(path)?;
            small.insert(path.clone(), styles::patch_styles(&xml, styles));
        }
        if let Some(cc) = calc_chain {
            let ct = self.pkg.read("[Content_Types].xml")?;
            small.insert(
                "[Content_Types].xml".into(),
                drop_tag(&ct, b"Override", b"PartName", &format!("/{cc}")),
            );
            let rels = rels_path(&self.meta.path);
            if let Ok(r) = self.pkg.read(&rels) {
                small.insert(rels, drop_calc_chain_rel(&r));
            }
            rep.calc_chain_removed = true;
        }

        let file = File::create(out)?;
        let mut zip = ZipWriter::new(BufWriter::with_capacity(1 << 20, file));
        for e in self.pkg.entries_in_order() {
            if Some(e.name.as_str()) == calc_chain {
                continue;
            }
            if let Some((i, sheet)) = by_path.get(e.name.as_str()) {
                if sheet.edits().structure {
                    let name = self.meta.sheets[*i].name.clone();
                    self.write_sheet_regen(&mut zip, e, sheet, shared, &name, opts, &mut rep)?;
                } else {
                    self.write_sheet(&mut zip, e, sheet, shared, opts, &mut rep)?;
                }
                rep.sheets_rewritten.push(*i);
                rep.parts_rewritten += 1;
            } else if let Some(bytes) = small.get(&e.name) {
                let z = compress_chunk(bytes, opts.level, true);
                let mut h = crc32fast::Hasher::new();
                h.update(bytes);
                zip.copy_raw(&e.name, zipw::DEFLATED, h.finalize(), bytes.len() as u64, &z)?;
                rep.parts_rewritten += 1;
            } else {
                zip.copy_raw(&e.name, e.method_code(), e.crc32, e.size, self.pkg.compressed(e))?;
                rep.parts_copied += 1;
            }
        }
        let (w, len) = zip.finish()?;
        let file = w.into_inner().map_err(|e| Error::Io(e.into_error()))?;
        file.set_len(len)?;
        file.sync_all()?;
        rep.sheets_rewritten.sort_unstable();
        rep.bytes = len;
        rep.wall = t0.elapsed();
        Ok(rep)
    }

    /// Aba com mudança de estrutura: começo e fim do XML original
    /// (ajustados) e o `<sheetData>` inteiro a partir do modelo.
    #[allow(clippy::too_many_arguments)]
    fn write_sheet_regen<W: Write + Seek + Send>(
        &self,
        zip: &mut ZipWriter<W>,
        e: &crate::package::Entry,
        sheet: &Sheet,
        shared: &StringTable,
        name: &str,
        opts: &SaveOptions,
        rep: &mut SaveReport,
    ) -> Result<(), Error> {
        let (head, tail) = self.head_and_tail(e)?;
        let shifts = &sheet.edits().shifts;
        zip.begin_part(&e.name, e.size >= 2 << 30)?;
        let mut enc = PartEncoder::new(opts.level, opts.chunk_bytes);
        let mut h = regen::head(&head, sheet, shifts);
        if sheet.edits().pane {
            h = patch::patch_pane(&h, sheet.layout.pane);
        }
        enc.write(zip, &h)?;
        let mut xml_out = h.len() as u64;
        let cells = regen::sheet_data(sheet, shared, &mut |b| {
            xml_out += b.len() as u64;
            enc.write(zip, b)
        })?;
        let t = regen::tail(&tail, sheet, name, shifts);
        xml_out += t.len() as u64;
        enc.write(zip, &t)?;
        let (crc, size) = enc.finish(zip)?;
        zip.end_part(crc, size)?;
        rep.cells_written += cells;
        rep.xml_in += e.size;
        rep.xml_out += xml_out;
        Ok(())
    }

    /// O XML da aba antes de `<sheetData>` e depois de `</sheetData>`
    /// (o meio é lido e descartado).
    fn head_and_tail(&self, e: &crate::package::Entry) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let mut sc = HeadTail::default();
        let parallel = e.is_deflated()
            && e.compressed_size() >= PARALLEL_INFLATE_MIN
            && rayon::current_num_threads() >= 4;
        if parallel {
            let r = duck_deflate::inflate_parallel(
                self.pkg.compressed(e),
                Some(e.crc32),
                duck_deflate::Options::default(),
                |b| {
                    sc.feed(b.bytes());
                    Ok(())
                },
            );
            if r.is_ok() {
                return Ok((sc.head, sc.tail));
            }
            sc = HeadTail::default();
        }
        let mut inf = self.pkg.inflater(e);
        let mut buf = Vec::with_capacity(4 << 20);
        loop {
            buf.clear();
            let eof = inf.read_into(&mut buf, 4 << 20)?;
            sc.feed(&buf);
            if eof {
                break;
            }
        }
        Ok((sc.head, sc.tail))
    }

    fn write_sheet<W: Write + Seek + Send>(
        &self,
        zip: &mut ZipWriter<W>,
        e: &crate::package::Entry,
        sheet: &Sheet,
        shared: &StringTable,
        opts: &SaveOptions,
        rep: &mut SaveReport,
    ) -> Result<(), Error> {
        let big = e.size >= 2 << 30;
        let parallel = e.is_deflated()
            && e.compressed_size() >= PARALLEL_INFLATE_MIN
            && rayon::current_num_threads() >= 4;
        if parallel {
            zip.begin_part(&e.name, big)?;
            let mut patcher = patch::Patcher::new(sheet, shared, opts.rewrite_all);
            let mut enc = PartEncoder::new(opts.level, opts.chunk_bytes);
            let mut buf = Vec::new();
            let mut xml_out = 0u64;
            let mut io_err = None;
            let r = duck_deflate::inflate_parallel(
                self.pkg.compressed(e),
                Some(e.crc32),
                duck_deflate::Options::default(),
                |b| {
                    patcher.feed(b.bytes(), &mut buf);
                    xml_out += buf.len() as u64;
                    if let Err(err) = enc.write(zip, &buf) {
                        io_err = Some(err);
                        return Err("escrita".into());
                    }
                    buf.clear();
                    Ok(())
                },
            );
            if let Some(err) = io_err {
                return Err(Error::Io(err));
            }
            if r.is_ok() {
                patcher.finish(&mut buf);
                xml_out += buf.len() as u64;
                enc.write(zip, &buf)?;
                let (crc, size) = enc.finish(zip)?;
                zip.end_part(crc, size)?;
                rep.cells_written += patcher.cells_written;
                rep.xml_in += e.size;
                rep.xml_out += xml_out;
                return Ok(());
            }
            // O inflate paralelo não conferiu: refaz pelo caminho sequencial.
            zip.abort_part()?;
        }
        zip.begin_part(&e.name, big)?;
        let mut patcher = patch::Patcher::new(sheet, shared, opts.rewrite_all);
        let mut enc = PartEncoder::new(opts.level, opts.chunk_bytes);
        let mut inf = self.pkg.inflater(e);
        let mut input = Vec::with_capacity(opts.chunk_bytes);
        let mut buf = Vec::new();
        loop {
            input.clear();
            let eof = inf.read_into(&mut input, opts.chunk_bytes)?;
            patcher.feed(&input, &mut buf);
            if eof {
                patcher.finish(&mut buf);
            }
            rep.xml_out += buf.len() as u64;
            enc.write(zip, &buf)?;
            buf.clear();
            if eof {
                break;
            }
        }
        let (crc, size) = enc.finish(zip)?;
        zip.end_part(crc, size)?;
        rep.cells_written += patcher.cells_written;
        rep.xml_in += e.size;
        Ok(())
    }
}

/// Separa o começo e o fim do XML de uma aba, lendo em pedaços.
#[derive(Default)]
struct HeadTail {
    head: Vec<u8>,
    tail: Vec<u8>,
    /// 0 = começo, 1 = dentro de `<sheetData>`, 2 = fim.
    phase: u8,
    /// Últimos bytes do pedaço anterior (marcas cortadas no meio).
    carry: Vec<u8>,
}

impl HeadTail {
    fn feed(&mut self, b: &[u8]) {
        match self.phase {
            0 => {
                self.head.extend_from_slice(b);
                let Some(k) = memchr::memmem::find(&self.head, b"<sheetData") else { return };
                let Some(gt) = memchr::memchr(b'>', &self.head[k..]).map(|x| k + x) else { return };
                let rest = self.head.split_off(gt + 1);
                let closed = self.head[gt - 1] == b'/';
                self.head.truncate(k);
                if closed {
                    self.phase = 2;
                    self.tail = rest;
                } else {
                    self.phase = 1;
                    self.feed_body(&rest);
                }
            }
            1 => self.feed_body(b),
            _ => self.tail.extend_from_slice(b),
        }
    }

    fn feed_body(&mut self, b: &[u8]) {
        const END: &[u8] = b"</sheetData>";
        let mut joint = std::mem::take(&mut self.carry);
        let skip = joint.len();
        joint.extend_from_slice(&b[..b.len().min(END.len())]);
        if let Some(k) = memchr::memmem::find(&joint, END) {
            self.phase = 2;
            let after = k + END.len() - skip;
            self.tail.extend_from_slice(&b[after.min(b.len())..]);
            return;
        }
        if let Some(k) = memchr::memmem::find(b, END) {
            self.phase = 2;
            self.tail.extend_from_slice(&b[k + END.len()..]);
            return;
        }
        let keep = b.len().min(END.len() - 1);
        self.carry = b[b.len() - keep..].to_vec();
    }
}

/// `dir/_rels/arquivo.rels` de uma parte.
fn rels_path(part: &str) -> String {
    match part.rfind('/') {
        Some(i) => format!("{}/_rels/{}.rels", &part[..i], &part[i + 1..]),
        None => format!("_rels/{part}.rels"),
    }
}

/// `<calcPr fullCalcOnLoad="1">` no `workbook.xml`: o Excel recalcula tudo ao abrir.
fn force_full_calc(xml: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len() + 32);
    if let Some(tag) = find_tag(xml, 0, b"calcPr") {
        if attr(tag.attrs, b"fullCalcOnLoad").is_some_and(|v| v == b"1" || v == b"true") {
            return xml.to_vec();
        }
        out.extend_from_slice(&xml[..tag.start]);
        out.extend_from_slice(b"<calcPr");
        for_each_attr(tag.attrs, |k, whole| {
            if k != b"fullCalcOnLoad" {
                out.push(b' ');
                out.extend_from_slice(whole);
            }
        });
        out.extend_from_slice(b" fullCalcOnLoad=\"1\"");
        out.extend_from_slice(if tag.self_closing { b"/>" } else { b">" });
        out.extend_from_slice(&xml[tag.end..]);
        return out;
    }
    // Sem `<calcPr>`: entra antes dos elementos que vêm depois dele no esquema.
    const AFTER: [&[u8]; 9] = [
        b"oleSize",
        b"customWorkbookViews",
        b"pivotCaches",
        b"smartTagPr",
        b"smartTagTypes",
        b"webPublishing",
        b"fileRecoveryPr",
        b"webPublishObjects",
        b"extLst",
    ];
    let at = AFTER
        .iter()
        .filter_map(|n| find_tag(xml, 0, n).map(|t| t.start))
        .min()
        .or_else(|| memchr::memmem::rfind(xml, b"</"));
    match at {
        Some(at) => {
            out.extend_from_slice(&xml[..at]);
            out.extend_from_slice(b"<calcPr fullCalcOnLoad=\"1\"/>");
            out.extend_from_slice(&xml[at..]);
            out
        }
        None => xml.to_vec(),
    }
}

/// Tira os elementos `<name …/>` cujo atributo `key` é `value`.
fn drop_tag(xml: &[u8], name: &[u8], key: &[u8], value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut pos = 0;
    while let Some(tag) = find_tag(xml, pos, name) {
        let end = if tag.self_closing {
            tag.end
        } else {
            let mut close = b"</".to_vec();
            close.extend_from_slice(name);
            close.push(b'>');
            memchr::memmem::find(&xml[tag.end..], &close).map_or(tag.end, |x| tag.end + x + close.len())
        };
        out.extend_from_slice(&xml[pos..tag.start]);
        if attr(tag.attrs, key) != Some(value.as_bytes()) {
            out.extend_from_slice(&xml[tag.start..end]);
        }
        pos = end;
    }
    out.extend_from_slice(&xml[pos..]);
    out
}

/// Tira o relacionamento do `calcChain` dos rels do workbook.
fn drop_calc_chain_rel(xml: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut pos = 0;
    while let Some(tag) = find_tag(xml, pos, b"Relationship") {
        out.extend_from_slice(&xml[pos..tag.start]);
        if !attr(tag.attrs, b"Type").is_some_and(|t| t.ends_with(b"/calcChain")) {
            out.extend_from_slice(&xml[tag.start..tag.end]);
        }
        pos = tag.end;
    }
    out.extend_from_slice(&xml[pos..]);
    out
}

/// Diferença entre duas abas (valores, estilos, fórmulas e layout), se houver.
pub fn compare_sheets(a: &Sheet, sa: &StringTable, b: &Sheet, sb: &StringTable) -> Result<(), String> {
    let cols = a.cols().max(b.cols());
    let rows = a.rows().max(b.rows());
    let diff = (0..cols).into_par_iter().find_map_any(|c| {
        for r in 0..rows {
            let (x, y) = (a.cell(r, c, sa), b.cell(r, c, sb));
            let same_value = match (x.value, y.value) {
                (duck_core::Value::Number(p), duck_core::Value::Number(q)) => p.to_bits() == q.to_bits(),
                (p, q) => p == q,
            };
            if !same_value || x.style != y.style || x.formula != y.formula {
                return Some(format!("célula {}: {:?} × {:?}", name(r, c), x, y));
            }
            if x.formula {
                let (f, g) = (duck_calc::cell_formula(a, r, c), duck_calc::cell_formula(b, r, c));
                if f != g {
                    return Some(format!("fórmula em {}: {:?} × {:?}", name(r, c), f, g));
                }
            }
        }
        None
    });
    if let Some(d) = diff {
        return Err(d);
    }
    let (la, lb) = (&a.layout, &b.layout);
    if la.merges != lb.merges {
        return Err(format!("mesclagens: {} × {}", la.merges.len(), lb.merges.len()));
    }
    if la.cols != lb.cols {
        return Err("larguras de coluna diferentes".into());
    }
    if la.rows != lb.rows {
        return Err("alturas de linha diferentes".into());
    }
    Ok(())
}

fn name(r: u32, c: u32) -> String {
    let mut v = Vec::new();
    cellxml::cell_ref(r, c, &mut v);
    String::from_utf8(v).unwrap_or_default()
}

/// Arquivo temporário ao lado do destino.
fn temp_path(dest: &Path) -> PathBuf {
    let name =
        dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "planilha".into());
    dest.with_file_name(format!("~{name}.duck-salvando"))
}

/// Salva `sheets` (ver [`Book::save`]) lendo as partes intactas de `src` e
/// gravando em `dest` (pode ser o próprio `src`). Grava num temporário, relê
/// as abas regravadas e confere com o modelo antes de substituir `dest`:
/// um defeito do gravador nunca destrói o arquivo original.
pub fn save_file(
    src: &Path,
    dest: &Path,
    sheets: &[Option<&Sheet>],
    shared: &StringTable,
    styles: &Styles,
    opts: &SaveOptions,
) -> Result<SaveReport, Error> {
    let tmp = temp_path(dest);
    let result = (|| {
        let book = Book::open(src)?;
        let mut rep = book.save(sheets, shared, styles, &tmp, opts)?;
        // O original precisa estar fechado (sem mapeamento) para ser substituído.
        drop(book);
        if opts.verify {
            let t = Instant::now();
            verify(&tmp, sheets, shared, styles, &rep.sheets_rewritten)?;
            rep.verify = t.elapsed();
        }
        if let Ok(meta) = std::fs::metadata(dest) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, dest)?;
        Ok(rep)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Relê `path` e confere as abas `which` contra o modelo.
pub fn verify(
    path: &Path,
    sheets: &[Option<&Sheet>],
    shared: &StringTable,
    styles: &Styles,
    which: &[usize],
) -> Result<(), Error> {
    let book = Book::open(path)?;
    if book.sheets().len() != sheets.len() {
        return Err(Error::Format(format!(
            "conferência: {} abas no arquivo salvo, {} no modelo",
            book.sheets().len(),
            sheets.len()
        )));
    }
    let shared2 = book.read_shared_strings()?;
    if styles.has_new() {
        let back = book.read_styles()?;
        if back.xfs != styles.xfs
            || back.fonts != styles.fonts
            || back.fills != styles.fills
            || back.borders != styles.borders
        {
            return Err(Error::Format("conferência: estilos diferentes no arquivo salvo".into()));
        }
    }
    let failure = std::sync::Mutex::new(None);
    let done = |i: usize, r: crate::SheetResult| {
        let msg = match r {
            Ok((back, _)) => match sheets[i] {
                Some(s) => compare_sheets(s, shared, &back, &shared2).err(),
                None => None,
            },
            Err(e) => Some(e.to_string()),
        };
        if let Some(m) = msg {
            failure.lock().unwrap().get_or_insert(format!("conferência da aba {}: {m}", i + 1));
        }
    };
    book.load_sheets(which, &crate::Options::default(), &done);
    match failure.into_inner().unwrap() {
        Some(m) => Err(Error::Format(m)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbook_patches() {
        let wb = br#"<workbook><sheets/><calcPr calcId="191029"/><extLst/></workbook>"#;
        assert_eq!(
            force_full_calc(wb),
            br#"<workbook><sheets/><calcPr calcId="191029" fullCalcOnLoad="1"/><extLst/></workbook>"#
        );
        let wb = br#"<workbook><sheets/><pivotCaches/></workbook>"#;
        assert_eq!(
            force_full_calc(wb),
            br#"<workbook><sheets/><calcPr fullCalcOnLoad="1"/><pivotCaches/></workbook>"#
        );
        let ct = br#"<Types><Override PartName="/xl/calcChain.xml" ContentType="x"/><Override PartName="/xl/a.xml" ContentType="y"/></Types>"#;
        assert_eq!(
            drop_tag(ct, b"Override", b"PartName", "/xl/calcChain.xml"),
            br#"<Types><Override PartName="/xl/a.xml" ContentType="y"/></Types>"#
        );
        let rels = br#"<Relationships><Relationship Id="rId1" Type="http://x/calcChain" Target="calcChain.xml"/><Relationship Id="rId2" Type="http://x/styles" Target="styles.xml"/></Relationships>"#;
        assert_eq!(
            drop_calc_chain_rel(rels),
            br#"<Relationships><Relationship Id="rId2" Type="http://x/styles" Target="styles.xml"/></Relationships>"#
        );
    }
}
