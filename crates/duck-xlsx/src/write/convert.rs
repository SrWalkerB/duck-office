//! `.xlsb` → `.xlsx` / `.xlsm`: um pacote novo com o que a leitura entende.
//!
//! O binário não é regravado. Para salvar, o arquivo original vira primeiro um
//! pacote em XML (as partes que a camada do `.xlsb` apresenta, mais as
//! células, que saem do modelo), e as edições entram nele pelo mesmo gravador
//! dos outros arquivos. O que não é lido (tabelas dinâmicas, segmentações,
//! comentários...) não vai: [`crate::Book::left_out`] diz o que é.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::Path;
use std::sync::Mutex;

use super::deflate::{PartEncoder, compress_chunk};
use super::zipw::{DEFLATED, ZipWriter};
use super::{DYN_METADATA, cellxml, drop_tag, insert_before, regen, rels_path};
use crate::xlsb::Kind;
use crate::xml::{attr_str, for_each};
use crate::{Book, Error, Options};

const MACRO_WORKBOOK: &str = "application/vnd.ms-excel.sheet.macroEnabled.main+xml";
const VBA: &str = "application/vnd.ms-office.vbaProject";
const METADATA: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml";
const METADATA_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata";

/// Partes do pacote novo, a partir da raiz, seguindo os relacionamentos.
struct Parts {
    /// Em ordem, sem a raiz.
    names: Vec<String>,
    /// Parte (vazio = raiz) → relacionamentos dela.
    rels: BTreeMap<String, Vec<u8>>,
}

fn collect(book: &Book, macros: bool) -> Parts {
    let mut parts = Parts { names: Vec::new(), rels: BTreeMap::new() };
    let mut queue = vec![String::new()];
    while let Some(part) = queue.pop() {
        let path = if part.is_empty() { "_rels/.rels".to_string() } else { rels_path(&part) };
        let Ok(mut xml) = book.pkg.read(&path) else { continue };
        // O projeto VBA não leva a assinatura (as partes mudaram).
        if part.ends_with("vbaProject.bin") {
            continue;
        }
        let mut found = Vec::new();
        for (id, kind, target) in crate::parts::rel_targets_typed(&book.pkg, &part) {
            // Link para um lugar do próprio arquivo (`#Aba!A1`): não é parte.
            if kind.ends_with("/hyperlink") {
                continue;
            }
            let keep = macros || !kind.ends_with("/vbaProject");
            if !keep || book.pkg.entry(&target).is_none() {
                xml = drop_tag(&xml, b"Relationship", b"Id", &id);
            } else if !parts.names.contains(&target) {
                parts.names.push(target.clone());
                found.push(target);
            }
        }
        if !xml.is_empty() && memchr::memmem::find(&xml, b"<Relationship ").is_some() {
            parts.rels.insert(part, xml);
        }
        queue.extend(found.into_iter().rev());
    }
    parts
}

/// `[Content_Types].xml` do pacote novo.
fn content_types(book: &Book, parts: &Parts, macros: bool, metadata: Option<&str>) -> Vec<u8> {
    let (mut defaults, mut overrides) = (HashMap::new(), HashMap::new());
    if let Ok(xml) = book.pkg.read("[Content_Types].xml") {
        for_each(&xml, b"Default", |a, _| {
            if let (Some(e), Some(t)) = (attr_str(a, b"Extension"), attr_str(a, b"ContentType")) {
                defaults.insert(e.to_ascii_lowercase(), t);
            }
        });
        for_each(&xml, b"Override", |a, _| {
            if let (Some(p), Some(t)) = (attr_str(a, b"PartName"), attr_str(a, b"ContentType")) {
                overrides.insert(p, t);
            }
        });
    }
    let mut ext: BTreeMap<String, String> = BTreeMap::new();
    ext.insert("rels".into(), "application/vnd.openxmlformats-package.relationships+xml".into());
    ext.insert("xml".into(), "application/xml".into());
    let mut list = String::new();
    let mut add = |part: &str, kind: &str| {
        list.push_str(&format!("<Override PartName=\"/{part}\" ContentType=\"{kind}\"/>"));
    };
    for p in &parts.names {
        match book.pkg.xlsb().and_then(|x| x.kind(p)) {
            Some(Kind::Workbook) if macros => add(p, MACRO_WORKBOOK),
            Some(k) => add(p, k.content_type()),
            None if p.ends_with("vbaProject.bin") => add(p, VBA),
            None => match overrides.get(&format!("/{p}")) {
                Some(t) => add(p, t),
                None => {
                    let e = p.rsplit_once('.').map_or("", |x| x.1).to_ascii_lowercase();
                    if let Some(t) = defaults.get(&e) {
                        ext.entry(e).or_insert_with(|| t.clone());
                    }
                }
            },
        }
    }
    if let Some(p) = metadata {
        add(p, METADATA);
    }
    let defaults: String =
        ext.iter().map(|(e, t)| format!("<Default Extension=\"{e}\" ContentType=\"{t}\"/>")).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">{defaults}{list}</Types>"
    )
    .into_bytes()
}

fn put<W: Write + Seek>(zip: &mut ZipWriter<W>, name: &str, bytes: &[u8], level: u32) -> Result<(), Error> {
    let z = compress_chunk(bytes, level, true);
    let mut h = crc32fast::Hasher::new();
    h.update(bytes);
    zip.copy_raw(name, DEFLATED, h.finalize(), bytes.len() as u64, &z)?;
    Ok(())
}

/// Grava em `dest` o pacote em XML equivalente ao `.xlsb` aberto em `book`.
/// `macros` = com o projeto VBA (`.xlsm`).
pub fn to_xlsx(book: &Book, dest: &Path, macros: bool, level: u32) -> Result<(), Error> {
    let has_vba = book.pkg.entry("xl/vbaProject.bin").is_some();
    let macros = macros && has_vba;
    let layer = book.pkg.xlsb().ok_or_else(|| Error::Format("o arquivo não é um .xlsb".into()))?;
    let mut parts = collect(book, macros);
    let shared = book.read_shared_strings()?;
    let mut zip = ZipWriter::new(BufWriter::with_capacity(1 << 20, File::create(dest)?));
    // Matrizes dinâmicas: as células apontam para o `metadata.xml` padrão.
    let _cm = cellxml::dynamic_cm(1);
    let mut dynamic = false;
    for (index, name) in parts.names.iter().enumerate() {
        debug_assert!(!parts.names[..index].contains(name));
        match layer.kind(name) {
            Some(Kind::Sheet) => {
                let Some(i) = book.meta.sheets.iter().position(|m| m.path == *name) else { continue };
                dynamic |= sheet(book, i, name, &shared, &mut zip, level)?;
            }
            Some(Kind::Strings) => put(&mut zip, name, &crate::xlsb::strings_xml(&shared), level)?,
            Some(_) => put(&mut zip, name, &book.pkg.read(name)?, level)?,
            // Desenhos e gráficos podem ter sido limpos; o resto vai como está.
            None if name.ends_with(".xml") => put(&mut zip, name, &book.pkg.read(name)?, level)?,
            None => {
                let e = book.pkg.entry(name).ok_or_else(|| Error::Missing(name.clone()))?;
                zip.copy_raw(name, e.method_code(), e.crc32, e.size, book.pkg.compressed(e))?;
            }
        }
    }
    let metadata = dynamic.then(|| match layer.workbook().rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/metadata.xml"),
        None => "metadata.xml".to_string(),
    });
    if let Some(m) = &metadata {
        put(&mut zip, m, DYN_METADATA, level)?;
        if let Some(r) = parts.rels.get_mut(layer.workbook()) {
            let rel = format!("<Relationship Id=\"rIdDuckMeta\" Type=\"{METADATA_REL}\" Target=\"/{m}\"/>");
            *r = insert_before(r, b"</Relationships>", rel.as_bytes());
        }
    }
    for (part, xml) in &parts.rels {
        let path = if part.is_empty() { "_rels/.rels".to_string() } else { rels_path(part) };
        put(&mut zip, &path, xml, level)?;
    }
    put(&mut zip, "[Content_Types].xml", &content_types(book, &parts, macros, metadata.as_deref()), level)?;
    let (w, len) = zip.finish()?;
    let file = w.into_inner().map_err(|e| Error::Io(e.into_error()))?;
    file.set_len(len)?;
    Ok(())
}

/// Uma aba: começo e fim traduzidos, células do modelo. Uma por vez, para não
/// ter a pasta inteira duas vezes na memória. Devolve se há matriz dinâmica.
fn sheet<W: Write + Seek + Send>(
    book: &Book,
    index: usize,
    name: &str,
    shared: &duck_core::StringTable,
    zip: &mut ZipWriter<W>,
    level: u32,
) -> Result<bool, Error> {
    let loaded = Mutex::new(None);
    book.load_sheets(&[index], &Options::default(), &|_, r| *loaded.lock().unwrap() = Some(r));
    let (sheet, _) = loaded.into_inner().unwrap().unwrap_or_else(|| Err(Error::Missing(name.to_string())))?;
    let frame = book.pkg.read(name)?;
    let at = memchr::memmem::find(&frame, b"<sheetData/>").unwrap_or(frame.len());
    let rest = frame.get(at + b"<sheetData/>".len()..).unwrap_or_default();
    zip.begin_part(name, book.pkg.entry(name).is_some_and(|e| e.size >= 512 << 20))?;
    let mut enc = PartEncoder::new(level, 4 << 20);
    enc.write(zip, &frame[..at])?;
    regen::sheet_data(&sheet, shared, &mut |b| enc.write(zip, b))?;
    enc.write(zip, rest)?;
    let (crc, size) = enc.finish(zip)?;
    zip.end_part(crc, size)?;
    Ok(sheet.array_formulas().iter().any(|a| a.3))
}
