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
mod cells;
mod cellxml;
mod cube;
mod deflate;
mod objects;
mod patch;
mod pivot;
mod regen;
mod styles;
mod workbook;
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
use workbook::Work;
pub use workbook::{TabOrigin, TabOut, TabPlan};

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
    /// As abas do modelo, se podem não ser as do arquivo (criadas, copiadas,
    /// excluídas, renomeadas, movidas, escondidas).
    pub tabs: Option<TabPlan>,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { level: 6, chunk_bytes: 4 << 20, rewrite_all: false, verify: true, tabs: None }
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
    /// Partes criadas e tiradas (abas novas, copiadas ou excluídas).
    pub parts_added: Vec<String>,
    pub parts_removed: Vec<String>,
    /// Parte de cada aba no arquivo salvo.
    pub sheet_parts: Vec<String>,
    /// Partes das cópias de abas: (aba, nome provisório, nome no arquivo).
    pub part_renames: Vec<(usize, String, String)>,
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
        let tabs = match &opts.tabs {
            Some(p) => workbook::prepare(self, p, sheets)?,
            None => None,
        };
        let tabs_changed = tabs.is_some();
        let mut work = tabs.unwrap_or_else(|| Work::identity(self, sheets.len()));
        let edited = |s: &&Sheet| opts.rewrite_all || !s.edits().sheet_untouched();
        let mut by_path: HashMap<&str, (usize, &Sheet)> = HashMap::new();
        for (i, path) in work.parts.iter().enumerate() {
            if let Some(Some(s)) = sheets.get(i)
                && work.new[i].is_none()
                && (edited(s) || work.deselect.contains(&i) || self.retext_needed(&work, path))
            {
                by_path.insert(path.as_str(), (i, *s));
            }
        }
        // Layout de cada aba (nas cópias, com as partes definitivas).
        let layouts: Vec<Option<&duck_core::Layout>> = sheets
            .iter()
            .zip(&work.layouts)
            .map(|(s, l)| l.as_ref().or_else(|| s.map(|s| &s.layout)))
            .collect();
        let views = || sheets.iter().zip(&layouts).filter_map(|(s, l)| Some(((*s)?, (*l)?)));
        let new_styles = styles.has_new();
        let values = sheets.iter().flatten().any(|s| s.edits().values || s.edits().formulas);
        let moved = sheets.iter().flatten().any(|s| !s.edits().cell_ops.is_empty());
        let values = values || work.structure || moved;
        let formulas = sheets.iter().flatten().any(|s| s.edits().formulas) || work.structure;
        let calc_chain = self.meta.calc_chain.as_deref().filter(|_| formulas);

        // Partes pequenas remendadas: workbook, content types e rels.
        let mut small: HashMap<String, Vec<u8>> = std::mem::take(&mut work.small);
        if values {
            let mut wb = work.read(&self.pkg, &small, &self.meta.path)?;
            // Nomes definidos (áreas de impressão, intervalos nomeados) andam
            // com as linhas/colunas inseridas ou apagadas.
            // (com abas mudadas, isso já foi feito na preparação).
            let changes: Vec<(&str, &duck_core::Edits)> = self
                .meta
                .sheets
                .iter()
                .zip(sheets)
                .filter(|_| !tabs_changed)
                .filter_map(|(m, s)| Some((m.name.as_str(), (*s)?.edits())))
                .collect();
            wb = workbook::moved_names(wb, &changes);
            small.insert(self.meta.path.clone(), force_full_calc(&wb));
            rep.full_calc = true;
        }
        // Tabelas com filtro, intervalo ou colunas mudadas.
        for (s, l) in views() {
            for &t in &s.edits().tables {
                if let Some(table) = l.tables.get(t as usize)
                    && !table.part.is_empty()
                    && let Ok(xml) = work.read(&self.pkg, &small, &table.part)
                {
                    small.insert(table.part.clone(), patch::patch_table(&xml, table));
                }
            }
        }
        // Linhas/colunas inseridas ou apagadas: âncoras dos desenhos e fontes
        // dos caches de tabela dinâmica; gráficos com referências mudadas.
        let mut charts_done: std::collections::HashSet<&str> = Default::default();
        for (s, l) in views() {
            if s.edits().structure || s.edits().drawings || !s.edits().cell_ops.is_empty() {
                let mut parts: Vec<(&str, Vec<duck_core::drawing::Anchor>)> = Vec::new();
                for a in &l.drawing_anchors {
                    match parts.iter_mut().find(|p| p.0 == a.part) {
                        Some(p) => p.1.push(a.anchor),
                        None => parts.push((&a.part, vec![a.anchor])),
                    }
                }
                for (part, list) in parts {
                    if let Ok(xml) = work.read(&self.pkg, &small, part) {
                        small.insert(part.to_string(), objects::patch_anchors(&xml, &list));
                    }
                }
                for (part, r) in l.pivot_sources.iter().filter(|_| !s.edits().is_drawings_only()) {
                    if let Ok(xml) = work.read(&self.pkg, &small, part) {
                        small.insert(part.clone(), objects::patch_cache_source(&xml, *r));
                    }
                }
            }
            if s.edits().structure || s.edits().pivots {
                for o in &l.objects {
                    if let duck_core::drawing::ObjectKind::Chart(c) = &o.kind
                        && !c.part.is_empty()
                        && charts_done.insert(&c.part)
                        && let Ok(xml) = work.read(&self.pkg, &small, &c.part)
                        && let Some(new) = objects::patch_chart_refs(&xml, c)
                    {
                        small.insert(c.part.clone(), new);
                    }
                }
            }
        }
        // Botões de opção e caixas de seleção marcados ou desmarcados.
        for (i, (s, l)) in
            sheets.iter().zip(&layouts).enumerate().filter_map(|(i, (s, l))| Some((i, ((*s)?, (*l)?))))
        {
            if !s.edits().pivots {
                continue;
            }
            let mut marks: HashMap<u32, bool> = HashMap::new();
            for o in &l.objects {
                // Célula ligada que mudou de lugar ou de aba.
                if let duck_core::drawing::ObjectKind::Control(c) = &o.kind
                    && let Some(link) = &c.link
                    && let Ok(xml) = work.read(&self.pkg, &small, &c.part)
                    && let Some(new) = workbook::patch_control_link(&xml, link)
                {
                    small.insert(c.part.clone(), new);
                }
                if let duck_core::drawing::ObjectKind::Control(c) = &o.kind
                    && matches!(
                        c.kind,
                        duck_core::drawing::ControlKind::Radio | duck_core::drawing::ControlKind::Check
                    )
                    && let Ok(xml) = work.read(&self.pkg, &small, &c.part)
                {
                    marks.insert(o.id, c.checked);
                    if let Some(new) = objects::patch_control(&xml, c.checked) {
                        small.insert(c.part.clone(), new);
                    }
                }
            }
            if marks.is_empty() {
                continue;
            }
            let vml = match (&work.new[i], small.get(&rels_path(&work.parts[i]))) {
                // Cópia: as relações dela já foram refeitas.
                (Some(_), Some(rels)) => workbook::vml_parts(rels, &work.parts[i]),
                _ => crate::parts::rel_targets_typed(&self.pkg, &work.parts[i])
                    .into_iter()
                    .filter(|r| r.1.ends_with("/vmlDrawing"))
                    .map(|r| r.2)
                    .collect(),
            };
            for part in vml {
                if let Ok(xml) = work.read(&self.pkg, &small, &part)
                    && let Some(new) = objects::patch_vml_checked(&xml, &marks)
                {
                    small.insert(part, new);
                }
            }
        }
        // Tabelas dinâmicas e segmentações de dados mudadas.
        for (s, l) in views() {
            if !s.edits().pivots {
                continue;
            }
            for t in &l.pivots {
                if !t.part.is_empty()
                    && let Ok(xml) = work.read(&self.pkg, &small, &t.part)
                {
                    let mut new = pivot::patch_pivot_table(&xml, t);
                    if t.cube.is_some() {
                        let source = work.added.get(&t.part).unwrap_or(&t.part);
                        new = self.patch_cube(&xml, new, t, source, &mut small);
                    }
                    small.insert(t.part.clone(), new);
                }
            }
            let mut caches_done: std::collections::HashSet<&str> = Default::default();
            for o in &l.objects {
                if let duck_core::drawing::ObjectKind::Slicer(sl) = &o.kind
                    && let Some(link) = &sl.pivot
                    && caches_done.insert(&link.part)
                    && let Ok(xml) = work.read(&self.pkg, &small, &link.part)
                {
                    if link.is_cube() {
                        small.insert(link.part.clone(), cube::patch_slicer(&xml, sl, link));
                        continue;
                    }
                    let sel: std::collections::HashMap<u32, bool> =
                        link.item_x.iter().zip(&sl.items).map(|(x, it)| (*x, it.1)).collect();
                    small.insert(link.part.clone(), pivot::patch_slicer_cache(&xml, &sel));
                }
                if let duck_core::drawing::ObjectKind::Timeline(tl) = &o.kind
                    && !tl.part.is_empty()
                    && caches_done.insert(&tl.part)
                    && let Ok(xml) = work.read(&self.pkg, &small, &tl.part)
                {
                    small.insert(tl.part.clone(), pivot::patch_timeline_cache(&xml, tl.selection));
                }
            }
        }
        if new_styles && let Some(path) = &self.meta.styles {
            let xml = self.pkg.read(path)?;
            small.insert(path.clone(), styles::patch_styles(&xml, styles));
        }
        // Matrizes dinâmicas: as abas regravadas apontam para os metadados
        // `XLDAPR`; o arquivo que não tem `metadata.xml` ganha um.
        let mut new_meta: Option<(String, Vec<u8>)> = None;
        let new_sheets = || sheets.iter().zip(&work.new).filter(|x| x.1.is_some()).filter_map(|x| *x.0);
        let has_dyn =
            by_path.values().map(|x| x.1).chain(new_sheets()).any(|s| s.array_formulas().iter().any(|a| a.3));
        let mut cm = 0;
        if has_dyn {
            let dir = self.meta.path.rsplit_once('/').map_or("", |p| p.0);
            let meta =
                if dir.is_empty() { "metadata.xml".to_string() } else { format!("{dir}/metadata.xml") };
            match self.pkg.read(&meta) {
                Ok(x) => cm = xldapr_cm(&x).unwrap_or(0),
                Err(_) => {
                    new_meta = Some((meta, DYN_METADATA.to_vec()));
                    cm = 1;
                }
            }
        }
        cellxml::DYN_CM.store(cm, std::sync::atomic::Ordering::Relaxed);

        if let Some(cc) = calc_chain {
            let ct = work.read(&self.pkg, &small, "[Content_Types].xml")?;
            small.insert(
                "[Content_Types].xml".into(),
                drop_tag(&ct, b"Override", b"PartName", &format!("/{cc}")),
            );
            let rels = rels_path(&self.meta.path);
            if let Ok(r) = work.read(&self.pkg, &small, &rels) {
                small.insert(rels, drop_calc_chain_rel(&r));
            }
            rep.calc_chain_removed = true;
        }
        if let Some((meta, _)) = &new_meta {
            let ct = match small.get("[Content_Types].xml") {
                Some(b) => b.clone(),
                None => self.pkg.read("[Content_Types].xml")?,
            };
            let ov = format!(
                r#"<Override PartName="/{meta}" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"/>"#
            );
            small.insert("[Content_Types].xml".into(), insert_before(&ct, b"</Types>", ov.as_bytes()));
            let rels = rels_path(&self.meta.path);
            let r = match small.get(&rels) {
                Some(b) => b.clone(),
                None => self.pkg.read(&rels)?,
            };
            let rel = br#"<Relationship Id="rIdDuckMeta" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata" Target="metadata.xml"/>"#;
            small.insert(rels, insert_before(&r, b"</Relationships>", rel));
        }

        let file = File::create(out)?;
        let mut zip = ZipWriter::new(BufWriter::with_capacity(1 << 20, file));
        for e in self.pkg.entries_in_order() {
            if Some(e.name.as_str()) == calc_chain {
                continue;
            }
            if work.removed.contains(&e.name) {
                rep.parts_removed.push(e.name.clone());
                continue;
            }
            if let Some((i, sheet)) = by_path.get(e.name.as_str()) {
                let fix = TabFix {
                    map: work.sheet_map(*i, ""),
                    deselect: work.deselect.contains(i),
                    copy: false,
                    name: self.file_name(&e.name),
                };
                if sheet.edits().structure {
                    let name = self.file_name(&e.name);
                    self.write_sheet_regen(
                        &mut zip,
                        Some(e),
                        &e.name,
                        sheet,
                        shared,
                        &name,
                        &fix,
                        opts,
                        &mut rep,
                    )?;
                } else {
                    self.write_sheet(&mut zip, e, sheet, shared, &fix, opts, &mut rep)?;
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
        // Abas novas e copiadas (o `<sheetData>` sai do modelo).
        for (i, new) in work.new.iter().enumerate() {
            let (Some(new), Some(Some(sheet))) = (new, sheets.get(i)) else { continue };
            let name = opts.tabs.as_ref().map_or("", |p| p.tabs[i].name.as_str());
            let fix =
                TabFix { map: work.sheet_map(i, name), deselect: false, copy: true, name: String::new() };
            let source = new.source.as_ref().and_then(|s| self.pkg.entry(&s.0));
            let from = new.source.as_ref().map_or("", |s| s.1.as_str());
            self.write_sheet_regen(
                &mut zip,
                source,
                &work.parts[i],
                sheet,
                shared,
                from,
                &fix,
                opts,
                &mut rep,
            )?;
            rep.sheets_rewritten.push(i);
            rep.parts_added.push(work.parts[i].clone());
        }
        // As outras partes novas: copiadas do arquivo ou já remendadas.
        for (dst, src) in &work.added {
            if !small.contains_key(dst)
                && let Some(e) = self.pkg.entry(src)
            {
                zip.copy_raw(dst, e.method_code(), e.crc32, e.size, self.pkg.compressed(e))?;
                rep.parts_added.push(dst.clone());
            }
        }
        let mut fresh: Vec<&String> = small.keys().filter(|p| self.pkg.entry(p).is_none()).collect();
        fresh.sort();
        for p in fresh {
            let bytes = &small[p];
            let z = compress_chunk(bytes, opts.level, true);
            let mut h = crc32fast::Hasher::new();
            h.update(bytes);
            zip.copy_raw(p, zipw::DEFLATED, h.finalize(), bytes.len() as u64, &z)?;
            rep.parts_added.push(p.clone());
        }
        if let Some((meta, bytes)) = &new_meta {
            let z = compress_chunk(bytes, opts.level, true);
            let mut h = crc32fast::Hasher::new();
            h.update(bytes);
            zip.copy_raw(meta, zipw::DEFLATED, h.finalize(), bytes.len() as u64, &z)?;
            rep.parts_rewritten += 1;
        }
        for (i, r) in work.renames.iter().enumerate() {
            rep.part_renames.extend(r.iter().map(|(a, b)| (i, a.clone(), b.clone())));
        }
        rep.part_renames.sort();
        rep.sheet_parts = work.parts.clone();
        let (w, len) = zip.finish()?;
        let file = w.into_inner().map_err(|e| Error::Io(e.into_error()))?;
        file.set_len(len)?;
        file.sync_all()?;
        rep.sheets_rewritten.sort_unstable();
        rep.bytes = len;
        rep.wall = t0.elapsed();
        Ok(rep)
    }

    /// Nome no arquivo da aba da parte `path`.
    fn file_name(&self, path: &str) -> String {
        self.meta.sheets.iter().find(|m| m.path == path).map(|m| m.name.clone()).unwrap_or_default()
    }

    /// O XML da aba cita, fora das células, alguma aba que mudou de nome ou
    /// saiu? (As células com essas referências já estão marcadas.)
    fn retext_needed(&self, work: &Work, path: &str) -> bool {
        if work.map.is_empty() {
            return false;
        }
        self.pkg
            .entry(path)
            .and_then(|e| self.head_and_tail(e).ok())
            .is_some_and(|(_, tail)| workbook::retext_tail(&tail, &work.map).is_some())
    }

    /// Dinâmica do Modelo de Dados: a tabela (já com o remendo comum) e o
    /// cache dela, se foi recalculada ou mudou de filtro.
    fn patch_cube(
        &self,
        orig: &[u8],
        new: Vec<u8>,
        t: &duck_core::pivot::PivotTable,
        source: &str,
        small: &mut HashMap<String, Vec<u8>>,
    ) -> Vec<u8> {
        let same = crate::cube::parse_cube_pivot(orig).map(Box::new) == t.cube;
        let part = crate::parts::rel_targets_typed(&self.pkg, source)
            .into_iter()
            .find(|r| r.1.ends_with("/pivotCacheDefinition"))
            .map(|r| r.2);
        let Some((part, cache)) = part.filter(|_| !same).and_then(|p| {
            let c = crate::cube::read_cube_cache(&self.pkg, &p)?;
            Some((p, c))
        }) else {
            return new;
        };
        if let Ok(xml) = self.pkg.read(&part) {
            small.insert(part, cube::patch_cache(&xml, t, &cache));
        }
        cube::patch_table(&new, t, &cache)
    }

    /// Aba com mudança de estrutura: começo e fim do XML original
    /// (ajustados) e o `<sheetData>` inteiro a partir do modelo.
    #[allow(clippy::too_many_arguments)]
    fn write_sheet_regen<W: Write + Seek + Send>(
        &self,
        zip: &mut ZipWriter<W>,
        e: Option<&crate::package::Entry>,
        part: &str,
        sheet: &Sheet,
        shared: &StringTable,
        name: &str,
        fix: &TabFix,
        opts: &SaveOptions,
        rep: &mut SaveReport,
    ) -> Result<(), Error> {
        let size = e.map_or(0, |e| e.size);
        let (head, tail) = match e {
            Some(e) => self.head_and_tail(e)?,
            None => {
                let (h, t) = workbook::blank_head();
                (h.to_vec(), t.to_vec())
            }
        };
        let shifts = &sheet.edits().shifts;
        zip.begin_part(part, size >= 2 << 30)?;
        let mut enc = PartEncoder::new(opts.level, opts.chunk_bytes);
        let mut h = regen::head(&head, sheet, shifts);
        if sheet.edits().pane || e.is_none() {
            h = patch::patch_pane(&h, sheet.layout.pane);
        }
        h = fix.head(h, sheet);
        enc.write(zip, &h)?;
        let mut xml_out = h.len() as u64;
        let cells = regen::sheet_data(sheet, shared, &mut |b| {
            xml_out += b.len() as u64;
            enc.write(zip, b)
        })?;
        // Inserir/apagar linhas e mover células, na ordem em que foram feitos.
        let mut t = tail;
        let mut done = 0;
        for (at, op) in &sheet.edits().cell_ops {
            if *at > done {
                t = regen::tail(&t, sheet, name, &shifts[done..*at]);
                done = *at;
            }
            t = cells::tail(&t, op, name);
        }
        let t = fix.tail(regen::tail(&t, sheet, name, &shifts[done..]));
        xml_out += t.len() as u64;
        enc.write(zip, &t)?;
        let (crc, size) = enc.finish(zip)?;
        zip.end_part(crc, size)?;
        rep.cells_written += cells;
        rep.xml_in += size;
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

    #[allow(clippy::too_many_arguments)]
    fn write_sheet<W: Write + Seek + Send>(
        &self,
        zip: &mut ZipWriter<W>,
        e: &crate::package::Entry,
        sheet: &Sheet,
        shared: &StringTable,
        fix: &TabFix,
        opts: &SaveOptions,
        rep: &mut SaveReport,
    ) -> Result<(), Error> {
        let big = e.size >= 2 << 30;
        let parallel = e.is_deflated()
            && e.compressed_size() >= PARALLEL_INFLATE_MIN
            && rayon::current_num_threads() >= 4;
        if parallel {
            zip.begin_part(&e.name, big)?;
            let mut patcher = patch::Patcher::new(sheet, shared, opts.rewrite_all).with_fix(fix);
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
        let mut patcher = patch::Patcher::new(sheet, shared, opts.rewrite_all).with_fix(fix);
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

/// O que muda no XML de uma aba por causa das abas (além das células).
pub(super) struct TabFix {
    /// Nomes de aba trocados.
    map: Option<duck_calc::sheetrefs::SheetMap>,
    /// Tira a marca de aba selecionada.
    deselect: bool,
    /// Aba nova ou copiada.
    copy: bool,
    /// Nome da aba no arquivo.
    name: String,
}

impl TabFix {
    fn head(&self, h: Vec<u8>, sheet: &Sheet) -> Vec<u8> {
        if self.deselect || self.copy || sheet.edits().tab_color {
            return workbook::patch_head(&h, sheet, self.deselect, self.copy);
        }
        h
    }

    /// O remendo da aba muda o fim do XML dela?
    fn changes_tail(&self, sheet: &Sheet) -> bool {
        self.map.is_some() || !sheet.edits().cell_ops.is_empty()
    }

    /// O fim do XML no remendo (sem linhas ou colunas inseridas ou apagadas).
    fn patched_tail(&self, mut t: Vec<u8>, sheet: &Sheet) -> Vec<u8> {
        for (_, op) in &sheet.edits().cell_ops {
            t = cells::tail(&t, op, &self.name);
        }
        self.tail(t)
    }

    fn tail(&self, t: Vec<u8>) -> Vec<u8> {
        self.map.as_ref().and_then(|m| workbook::retext_tail(&t, m)).unwrap_or(t)
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
/// Metadados mínimos das matrizes dinâmicas (`cm="1"` = `XLDAPR`).
const DYN_METADATA: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray"><metadataTypes count="1"><metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata></metadata>"#;

/// O `cm` das matrizes dinâmicas num `metadata.xml` existente: o bloco de
/// `cellMetadata` que aponta para o tipo `XLDAPR`.
fn xldapr_cm(x: &[u8]) -> Option<u32> {
    let mut t = None;
    let mut k = 0u32;
    crate::xml::for_each(x, b"metadataType", |a, _| {
        k += 1;
        if attr(a, b"name") == Some(b"XLDAPR") && t.is_none() {
            t = Some(k);
        }
    });
    let t = t?;
    let cm = find_tag(x, 0, b"cellMetadata")?;
    let body = crate::xml::inner(x, &cm, b"cellMetadata");
    let mut idx = 0u32;
    let mut found = None;
    crate::xml::for_each(body, b"bk", |_, bk| {
        idx += 1;
        if found.is_none()
            && let Some(rc) = find_tag(bk, 0, b"rc")
            && attr(rc.attrs, b"t").and_then(crate::xml::parse_u32) == Some(t)
        {
            found = Some(idx);
        }
    });
    found
}

/// Insere `ins` antes da última ocorrência de `before`.
fn insert_before(xml: &[u8], before: &[u8], ins: &[u8]) -> Vec<u8> {
    match memchr::memmem::rfind(xml, before) {
        Some(i) => [&xml[..i], ins, &xml[i..]].concat(),
        None => xml.to_vec(),
    }
}

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
