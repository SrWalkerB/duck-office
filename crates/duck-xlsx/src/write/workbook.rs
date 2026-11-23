//! Abas criadas, copiadas, excluídas, renomeadas, movidas e ocultas: o que
//! muda no pacote além do XML das próprias abas.
//!
//! - `workbook.xml`: `<sheets>` na ordem do modelo, nomes definidos (índice
//!   da aba e texto), aba ativa e as listas de caches.
//! - Relações e tipos das partes que entram; as que só a aba excluída usava
//!   saem (o que não se alcança mais pelas relações).
//! - Uma aba copiada leva cópias das partes dela (desenhos, gráficos, tabelas,
//!   comentários...); imagens e caches de dinâmica continuam os mesmos.
//! - Textos que citam abas fora das células (validação, formatação
//!   condicional, gráficos, controles, links) trocam de nome.
//!
//! [`prepare`] monta tudo isso antes de gravar; o resto do gravador trabalha
//! com as partes já no lugar ([`Work`]).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use duck_calc::sheetrefs::SheetMap;
use duck_calc::tabs::source_part;
use duck_core::{DefinedName, Layout, Sheet};
use memchr::memmem;

use super::cellxml::{escape_attr, escape_text};
use super::{insert_before, rels_path};
use crate::parts::{percent_decode, rel_targets_typed, resolve};
use crate::xml::{
    attr, attr_str, find_tag, for_each, for_each_attr, inner, is_name_end, parse_u32, tag_end, unescape_into,
};
use crate::{Book, Error, Package};

/// De onde vem o XML de uma aba.
#[derive(Clone, Debug, PartialEq)]
pub enum TabOrigin {
    /// Parte do arquivo.
    File(String),
    /// Cópia de outra aba do arquivo (parte dela) e os nomes novos das
    /// segmentações e linhas do tempo.
    Copy { part: String, renamed: Vec<(String, String)> },
    /// Aba nova, escrita só a partir do modelo.
    Blank,
}

/// Uma aba a gravar.
#[derive(Clone, Debug)]
pub struct TabOut {
    pub origin: TabOrigin,
    pub name: String,
    /// `sheetId`.
    pub id: u32,
    pub hidden: bool,
}

/// As abas do modelo, na ordem. Sem mudança em relação ao arquivo, o gravador
/// não mexe em nada além das abas editadas.
#[derive(Clone, Debug, Default)]
pub struct TabPlan {
    pub tabs: Vec<TabOut>,
    /// Nomes definidos do modelo (índice local = aba do modelo): os que o
    /// arquivo não tem são acrescentados.
    pub names: Vec<DefinedName>,
}

/// Aba que não está no arquivo.
pub(super) struct NewTab {
    /// Parte de origem (`None` = em branco) e o nome da aba de origem.
    pub source: Option<(String, String)>,
}

/// O que [`prepare`] decidiu.
#[derive(Default)]
pub(super) struct Work {
    /// Parte de cada aba do modelo.
    pub parts: Vec<String>,
    pub new: Vec<Option<NewTab>>,
    /// Abas do arquivo que saíram ou mudaram de nome.
    pub map: SheetMap,
    /// Layout com as partes definitivas (só nas cópias).
    pub layouts: Vec<Option<Layout>>,
    /// Partes renomeadas de cada cópia: provisória → definitiva.
    pub renames: Vec<HashMap<String, String>>,
    /// Partes novas: (nome, parte do arquivo de onde vem).
    pub added: BTreeMap<String, String>,
    pub removed: HashSet<String>,
    /// Abas escondidas que o arquivo tinha como selecionadas.
    pub deselect: HashSet<usize>,
    /// Partes (que não são de abas) com texto trocado.
    pub small: HashMap<String, Vec<u8>>,
    /// Abas entraram ou saíram.
    pub structure: bool,
}

impl Work {
    /// Nenhuma mudança nas abas: cada uma na sua parte.
    pub fn identity(book: &Book, n: usize) -> Self {
        Work {
            parts: book.meta.sheets.iter().map(|m| m.path.clone()).take(n).collect(),
            new: (0..n).map(|_| None).collect(),
            layouts: (0..n).map(|_| None).collect(),
            renames: (0..n).map(|_| HashMap::new()).collect(),
            ..Default::default()
        }
    }

    /// Bytes atuais de uma parte: já remendada, copiada ou do arquivo.
    pub fn read(
        &self,
        pkg: &Package,
        small: &HashMap<String, Vec<u8>>,
        part: &str,
    ) -> Result<Vec<u8>, Error> {
        if let Some(b) = small.get(part) {
            return Ok(b.clone());
        }
        pkg.read(self.added.get(part).map_or(part, String::as_str))
    }

    /// O texto do XML da aba `i` depois de `</sheetData>` precisa trocar nomes
    /// de aba? (nome da aba no arquivo, trocas).
    pub fn sheet_map(&self, i: usize, name: &str) -> Option<SheetMap> {
        let own = self.new.get(i).and_then(|n| n.as_ref()).and_then(|n| n.source.as_ref());
        let mut map = self.map.clone();
        if let Some((_, from)) = own {
            map.add(from, Some(name));
        }
        (!map.is_empty()).then_some(map)
    }
}

impl TabPlan {
    /// O plano muda alguma coisa no arquivo?
    fn changes(&self, book: &Book) -> bool {
        let m = &book.meta.sheets;
        let same = self.tabs.len() == m.len()
            && self.tabs.iter().zip(m).all(|(t, m)| {
                t.origin == TabOrigin::File(m.path.clone()) && t.name == m.name && t.hidden == m.hidden
            });
        !same
    }
}

/// Os nomes definidos do `workbook.xml` ajustados às linhas e colunas
/// inseridas ou apagadas e às células movidas, na ordem em que foi feito.
pub(super) fn moved_names(wb: Vec<u8>, sheets: &[(&str, &duck_core::Edits)]) -> Vec<u8> {
    let mut wb = wb;
    for (name, e) in sheets {
        let mut done = 0;
        for (at, op) in &e.cell_ops {
            if *at > done {
                wb = super::regen::defined_names(&wb, &[(name, &e.shifts[done..*at])]);
                done = *at;
            }
            let moved = |t: &str| duck_calc::cellops::adjust(t, op, false, name);
            wb = map_texts(&wb, &|n| n == b"definedName", &moved).unwrap_or(wb);
        }
        if done < e.shifts.len() {
            wb = super::regen::defined_names(&wb, &[(name, &e.shifts[done..])]);
        }
    }
    wb
}

/// Desenhos VML das relações de uma aba.
pub(super) fn vml_parts(rels: &[u8], part: &str) -> Vec<String> {
    parse_rels(rels, part).into_iter().filter(|r| r.kind.ends_with("/vmlDrawing")).map(|r| r.part).collect()
}

/// Um relacionamento como está no arquivo.
struct Rel {
    id: String,
    kind: String,
    /// Como está escrito.
    target: String,
    /// Parte (vazio se externo).
    part: String,
}

fn parse_rels(xml: &[u8], part: &str) -> Vec<Rel> {
    let dir = part.rsplit_once('/').map_or("", |p| p.0);
    let mut out = Vec::new();
    for_each(xml, b"Relationship", |a, _| {
        let (Some(id), Some(kind), Some(target)) =
            (attr_str(a, b"Id"), attr_str(a, b"Type"), attr_str(a, b"Target"))
        else {
            return;
        };
        let external = attr(a, b"TargetMode") == Some(b"External");
        let part = if external { String::new() } else { resolve(dir, &target) };
        out.push(Rel { id, kind, target, part });
    });
    out
}

/// Tira os relacionamentos que `drop` escolher.
fn drop_rels(xml: &[u8], drop: &dyn Fn(&str) -> bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut pos = 0;
    while let Some(t) = find_tag(xml, pos, b"Relationship") {
        out.extend_from_slice(&xml[pos..t.start]);
        let end = element_end(xml, &t, b"Relationship");
        if !attr_str(t.attrs, b"Id").is_some_and(|id| drop(&id)) {
            out.extend_from_slice(&xml[t.start..end]);
        }
        pos = end;
    }
    out.extend_from_slice(&xml[pos..]);
    out
}

fn element_end(xml: &[u8], t: &crate::xml::Tag, name: &[u8]) -> usize {
    if t.self_closing {
        return t.end;
    }
    let close = [b"</", name, b">"].concat();
    memmem::find(&xml[t.end..], &close).map_or(t.end, |k| t.end + k + close.len())
}

/// Reescreve as tags cujo nome `want` aceita; `f` recebe cada atributo
/// (nome, valor decodificado) e devolve o valor novo. `None` = nada mudou.
fn map_tags(
    xml: &[u8],
    want: &dyn Fn(&[u8]) -> bool,
    f: &mut dyn FnMut(&[u8], &str) -> Option<String>,
) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(xml.len());
    let mut changed = false;
    let mut pos = 0;
    while let Some(lt) = memchr::memchr(b'<', &xml[pos..]).map(|k| pos + k) {
        let Some(end) = tag_end(xml, lt) else { break };
        let n = xml[lt + 1..end].iter().position(|&c| is_name_end(c)).unwrap_or(0);
        let name = &xml[lt + 1..lt + 1 + n];
        out.extend_from_slice(&xml[pos..lt]);
        if name.is_empty() || matches!(name[0], b'/' | b'?' | b'!') || !want(name) {
            out.extend_from_slice(&xml[lt..end]);
            pos = end;
            continue;
        }
        let close: &[u8] = if xml[end - 2] == b'/' { b"/>" } else { b">" };
        let attrs = &xml[lt + 1 + n..end - close.len()];
        let mut tag = xml[lt..lt + 1 + n].to_vec();
        let mut hit = false;
        for_each_attr(attrs, |k, whole| {
            tag.push(b' ');
            let mut v = String::new();
            let eq = whole.iter().position(|&c| c == b'=').unwrap_or(k.len());
            let q = whole[eq..].iter().position(|&c| c == b'"' || c == b'\'').map_or(eq, |x| eq + x);
            unescape_into(&whole[(q + 1).min(whole.len())..whole.len() - 1], &mut v);
            match f(k, &v) {
                Some(new) => {
                    hit = true;
                    tag.extend_from_slice(k);
                    tag.extend_from_slice(b"=\"");
                    escape_attr(&new, &mut tag);
                    tag.push(b'"');
                }
                None => tag.extend_from_slice(whole),
            }
        });
        if hit {
            changed = true;
            out.extend_from_slice(&tag);
            out.extend_from_slice(close);
        } else {
            out.extend_from_slice(&xml[lt..end]);
        }
        pos = end;
    }
    out.extend_from_slice(&xml[pos..]);
    changed.then_some(out)
}

/// Reescreve o texto dos elementos (sem filhos) cujo nome `want` aceita.
fn map_texts(
    xml: &[u8],
    want: &dyn Fn(&[u8]) -> bool,
    f: &dyn Fn(&str) -> Option<String>,
) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(xml.len());
    let mut changed = false;
    let mut pos = 0;
    while let Some(lt) = memchr::memchr(b'<', &xml[pos..]).map(|k| pos + k) {
        let Some(end) = tag_end(xml, lt) else { break };
        let n = xml[lt + 1..end].iter().position(|&c| is_name_end(c)).unwrap_or(0);
        let name = &xml[lt + 1..lt + 1 + n];
        out.extend_from_slice(&xml[pos..end]);
        pos = end;
        if name.is_empty() || matches!(name[0], b'/' | b'?' | b'!') || !want(name) || xml[end - 2] == b'/' {
            continue;
        }
        let Some(close) = memchr::memchr(b'<', &xml[end..]).map(|k| end + k) else { break };
        let mut text = String::new();
        unescape_into(&xml[end..close], &mut text);
        match f(&text) {
            Some(new) => {
                changed = true;
                escape_text(&new, &mut out);
            }
            None => out.extend_from_slice(&xml[end..close]),
        }
        pos = close;
    }
    out.extend_from_slice(&xml[pos..]);
    changed.then_some(out)
}

/// Nome sem o prefixo (`c:f` → `f`).
fn local(name: &[u8]) -> &[u8] {
    name.iter().rposition(|&c| c == b':').map_or(name, |k| &name[k + 1..])
}

/// Controle com a célula ligada do modelo. `None` = já era essa.
pub(super) fn patch_control_link(xml: &[u8], link: &str) -> Option<Vec<u8>> {
    map_tags(xml, &|n| n == b"formControlPr", &mut |k, v| {
        (k == b"fmlaLink" && v != link).then(|| link.to_string())
    })
}

/// O fim do XML de uma aba (depois de `</sheetData>`) com os nomes de aba
/// trocados: formatação condicional, validação, minigráficos, links e
/// controles. `None` = nada mudou.
pub(super) fn retext_tail(tail: &[u8], map: &SheetMap) -> Option<Vec<u8>> {
    let formulas: &[&[u8]] = &[b"formula", b"formula1", b"formula2", b"f"];
    let a = map_texts(tail, &|n| formulas.contains(&local(n)), &|t| map.apply(t));
    let b = map_tags(
        a.as_deref().unwrap_or(tail),
        &|n| matches!(local(n), b"hyperlink" | b"controlPr" | b"oleObject" | b"objectPr" | b"dataRef"),
        &mut |k, v| match k {
            b"location" => map.apply_location(v),
            b"linkedCell" | b"listFillRange" | b"link" => map.apply(v),
            b"sheet" => map.name(v).flatten().map(str::to_string),
            _ => None,
        },
    );
    b.or(a)
}

/// O começo do XML da aba com a cor da guia do modelo, se mudou. `deselect`
/// tira a marca de aba selecionada; `copy` tira também o nome no VBA (cada
/// aba tem o seu) e o identificador da original.
pub(super) fn patch_head(head: &[u8], sheet: &Sheet, deselect: bool, copy: bool) -> Vec<u8> {
    let mut h = head.to_vec();
    if deselect || copy {
        h = drop_attr(&h, b"sheetView", b"tabSelected");
    }
    if copy {
        h = drop_attr(&h, b"sheetPr", b"codeName");
        h = drop_attr(&h, b"worksheet", b"xr:uid");
    }
    if !sheet.edits().tab_color {
        return h;
    }
    let mut color = Vec::new();
    if let Some(c) = sheet.tab_color() {
        super::styles::color_xml(b"tabColor", c, &mut color);
    }
    // `<sheetPr>` é o primeiro filho de `<worksheet>`, e `<tabColor>` o dele.
    match find_tag(&h, 0, b"sheetPr") {
        Some(t) if t.self_closing => {
            if color.is_empty() {
                return h;
            }
            let mut out = h[..t.end - 2].to_vec();
            out.push(b'>');
            out.extend_from_slice(&color);
            out.extend_from_slice(b"</sheetPr>");
            out.extend_from_slice(&h[t.end..]);
            out
        }
        Some(t) => {
            let end = element_end(&h, &t, b"sheetPr");
            let mut body = h[t.end..end].to_vec();
            if let Some(c) = find_tag(&body, 0, b"tabColor") {
                let e = element_end(&body, &c, b"tabColor");
                let s = c.start;
                body.drain(s..e);
            }
            [&h[..t.end], &color[..], &body[..], &h[end..]].concat()
        }
        None if color.is_empty() => h,
        None => {
            let Some(w) = find_tag(&h, 0, b"worksheet") else { return h };
            [&h[..w.end], b"<sheetPr>", &color[..], b"</sheetPr>", &h[w.end..]].concat()
        }
    }
}

/// Tira o atributo `key` da primeira tag `name`.
fn drop_attr(xml: &[u8], name: &[u8], key: &[u8]) -> Vec<u8> {
    let Some(t) = find_tag(xml, 0, name) else { return xml.to_vec() };
    if attr(t.attrs, key).is_none() {
        return xml.to_vec();
    }
    let mut out = xml[..t.start].to_vec();
    out.push(b'<');
    out.extend_from_slice(name);
    for_each_attr(t.attrs, |k, whole| {
        if k != key {
            out.push(b' ');
            out.extend_from_slice(whole);
        }
    });
    out.extend_from_slice(if t.self_closing { b"/>" } else { b">" });
    out.extend_from_slice(&xml[t.end..]);
    out
}

/// Começo e fim de uma aba em branco.
pub(super) fn blank_head() -> (&'static [u8], &'static [u8]) {
    (
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><dimension ref="A1"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/>"#,
        br#"<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#,
    )
}

/// `dir/nome12.xml` → (`dir/nome`, `.xml`).
fn split_number(part: &str) -> (&str, &str) {
    let dot = part.rfind('.').unwrap_or(part.len());
    let stem = part[..dot].trim_end_matches(|c: char| c.is_ascii_digit());
    (stem, &part[dot..])
}

/// Monta o trabalho das abas.
struct Builder<'a> {
    book: &'a Book,
    pkg: &'a Package,
    work: Work,
    /// Nomes de parte em uso.
    used: HashSet<String>,
    /// Content types a acrescentar: (parte nova, parte modelo).
    types: Vec<(String, String)>,
    wb: Vec<u8>,
    wb_rels: Vec<u8>,
    rel_ids: HashSet<String>,
}

impl Builder<'_> {
    fn free_part(&mut self, like: &str) -> String {
        let (stem, ext) = split_number(like);
        let p = (1..).map(|n| format!("{stem}{n}{ext}")).find(|p| !self.used.contains(p)).unwrap_or_default();
        self.used.insert(p.clone());
        p
    }

    fn free_rel(&mut self) -> String {
        let id = (1..).map(|n| format!("rIdAba{n}")).find(|i| !self.rel_ids.contains(i)).unwrap_or_default();
        self.rel_ids.insert(id.clone());
        id
    }

    /// Acrescenta uma relação ao workbook. Devolve o id.
    fn add_wb_rel(&mut self, kind: &str, part: &str) -> String {
        let id = self.free_rel();
        let dir = self.book.meta.path.rsplit_once('/').map_or("", |p| p.0);
        let target = match part.strip_prefix(&format!("{dir}/")) {
            Some(t) if !dir.is_empty() => t.to_string(),
            _ => format!("/{part}"),
        };
        let rel = format!(r#"<Relationship Id="{id}" Type="{kind}" Target="{target}"/>"#);
        self.wb_rels = insert_before(&self.wb_rels, b"</Relationships>", rel.as_bytes());
        id
    }

    /// Copia uma parte e, se as relações dela pedirem, as que ela usa.
    /// `map` junta parte antiga → nova.
    fn clone_part(&mut self, src: &str, map: &mut HashMap<String, String>) -> String {
        if let Some(d) = map.get(src) {
            return d.clone();
        }
        let dst = self.free_part(src);
        map.insert(src.to_string(), dst.clone());
        self.clone_into(src, &dst, map);
        dst
    }

    fn clone_into(&mut self, src: &str, dst: &str, map: &mut HashMap<String, String>) {
        self.work.added.insert(dst.to_string(), src.to_string());
        self.types.push((dst.to_string(), src.to_string()));
        let Ok(rels) = self.pkg.read(&rels_path(src)) else { return };
        // Imagens, caches e arquivos embutidos são usados pelas duas.
        const SHARED: [&str; 7] = [
            "/image",
            "/pivotCacheDefinition",
            "/hyperlink",
            "/theme",
            "/package",
            "/externalLink",
            "/person",
        ];
        let list = parse_rels(&rels, src);
        let mut targets: HashMap<String, String> = HashMap::new();
        for r in &list {
            if r.part.is_empty()
                || SHARED.iter().any(|s| r.kind.ends_with(s))
                || self.pkg.entry(&r.part).is_none()
            {
                continue;
            }
            let new = self.clone_part(&r.part, map);
            // Mesma pasta: só o nome do arquivo muda.
            let file = new.rsplit_once('/').map_or(new.as_str(), |p| p.1);
            let target = match r.target.rsplit_once('/') {
                Some((d, _)) => format!("{d}/{file}"),
                None => file.to_string(),
            };
            targets.insert(r.id.clone(), target);
        }
        let new = map_tags(&rels, &|n| n == b"Relationship", &mut |k, v| {
            if k == b"Target" {
                let r = list.iter().find(|r| r.target == v && targets.contains_key(&r.id))?;
                return targets.get(&r.id).cloned();
            }
            None
        });
        self.work.small.insert(rels_path(dst), new.unwrap_or(rels));
    }

    /// Abas novas e copiadas: partes, relações e tipos.
    fn add_tabs(&mut self, plan: &TabPlan, sheets: &[Option<&Sheet>]) -> Result<(), Error> {
        let sheet_kind = parse_rels(&self.wb_rels, &self.book.meta.path)
            .into_iter()
            .find(|r| r.kind.ends_with("/worksheet"))
            .map(|r| r.kind)
            .unwrap_or_else(|| {
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet".into()
            });
        let like =
            self.book.meta.sheets.first().map_or("xl/worksheets/sheet1.xml".to_string(), |m| m.path.clone());
        for (i, t) in plan.tabs.iter().enumerate() {
            let source = match &t.origin {
                TabOrigin::File(p) => {
                    self.work.parts.push(p.clone());
                    self.work.new.push(None);
                    self.work.layouts.push(None);
                    self.work.renames.push(HashMap::new());
                    continue;
                }
                TabOrigin::Copy { part, renamed } => Some((part.clone(), renamed)),
                TabOrigin::Blank => None,
            };
            let part = self.free_part(&like);
            self.add_wb_rel(&sheet_kind, &part);
            self.work.parts.push(part.clone());
            let Some((src, renamed)) = source else {
                self.types.push((part, like.clone()));
                self.work.new.push(Some(NewTab { source: None }));
                self.work.layouts.push(None);
                self.work.renames.push(HashMap::new());
                continue;
            };
            let meta = self
                .book
                .meta
                .sheets
                .iter()
                .find(|m| m.path == src)
                .ok_or_else(|| Error::Missing(src.clone()))?;
            let (src_name, src_id) = (meta.name.clone(), meta.id);
            let mut map = HashMap::new();
            map.insert(src.clone(), part.clone());
            self.clone_into(&src, &part, &mut map);
            // A aba em si é escrita a partir do modelo.
            self.work.added.remove(&part);
            let sheet = sheets.get(i).copied().flatten();
            self.finish_copy(&map, renamed, sheet, (src_id, t.id))?;
            let mut own = self.work.map.clone();
            own.add(&src_name, Some(&t.name));
            for dst in map.values() {
                self.retext_part(dst, &own);
            }
            // O modelo com as partes definitivas.
            let mut renames = HashMap::new();
            let layout = sheet.map(|s| {
                let mut l = s.layout.clone();
                l.map_parts(&mut |p| {
                    let (from, copy) = source_part(p);
                    if !copy {
                        return None;
                    }
                    let to = map.get(from)?.clone();
                    renames.insert(p.to_string(), to.clone());
                    Some(to)
                });
                l
            });
            self.work.layouts.push(layout);
            self.work.renames.push(renames);
            self.work.new.push(Some(NewTab { source: Some((src, src_name)) }));
        }
        Ok(())
    }

    /// Tabelas, segmentações e linhas do tempo de uma cópia: nomes, ids e
    /// caches próprios.
    fn finish_copy(
        &mut self,
        map: &HashMap<String, String>,
        renamed: &[(String, String)],
        sheet: Option<&Sheet>,
        (old_id, new_id): (u32, u32),
    ) -> Result<(), Error> {
        let pairs: Vec<(String, String)> = map.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
        // Tabelas.
        let mut table_ids: HashMap<String, String> = HashMap::new();
        for t in sheet.map(|s| &s.layout.tables[..]).unwrap_or(&[]) {
            let (from, _) = source_part(&t.part);
            let Some(dst) = map.get(from) else { continue };
            let Ok(xml) = self.pkg.read(from) else { continue };
            if let Some(id) = find_tag(&xml, 0, b"table").and_then(|x| attr_str(x.attrs, b"id")) {
                table_ids.insert(id, t.id.to_string());
            }
            let xml = super::patch::patch_table(&xml, t);
            let xml = map_tags(&xml, &|n| n == b"table", &mut |k, _| match k {
                b"id" => Some(t.id.to_string()),
                b"name" | b"displayName" => Some(t.name.clone()),
                _ => None,
            })
            .unwrap_or(xml);
            self.work.small.insert(dst.clone(), xml);
        }
        let new_name = |n: &str| renamed.iter().find(|r| r.0 == n).map(|r| r.1.clone());
        // Segmentações e linhas do tempo: caches do workbook por nome.
        let wb_rels = parse_rels(&self.wb_rels, &self.book.meta.path);
        let mut caches: HashMap<String, (String, String, String)> = HashMap::new();
        for r in &wb_rels {
            let tag: &[u8] = if r.kind.ends_with("/slicerCache") {
                b"slicerCacheDefinition"
            } else if r.kind.ends_with("/timelineCache") {
                b"timelineCacheDefinition"
            } else {
                continue;
            };
            let Ok(x) = self.work.read(self.pkg, &self.work.small, &r.part) else { continue };
            if let Some(n) = find_tag(&x, 0, tag).and_then(|t| attr_str(t.attrs, b"name")) {
                caches.insert(n, (r.id.clone(), r.kind.clone(), r.part.clone()));
            }
        }
        let mut cloned: HashMap<String, String> = HashMap::new();
        for (from, dst) in &pairs {
            let Ok(xml) = self.pkg.read(from) else { continue };
            let is_view =
                find_tag(&xml, 0, b"slicers").is_some() || find_tag(&xml, 0, b"timelines").is_some();
            if is_view {
                let mut used: Vec<String> = Vec::new();
                for tag in [&b"slicer"[..], b"timeline"] {
                    for_each(&xml, tag, |a, _| used.extend(attr_str(a, b"cache")));
                }
                for c in used {
                    if cloned.contains_key(&c) {
                        continue;
                    }
                    let Some((rid, kind, part)) = caches.get(&c).cloned() else { continue };
                    if let Some(n) = self.clone_cache(&c, &rid, &kind, &part, (old_id, new_id), &table_ids)? {
                        cloned.insert(c, n);
                    }
                }
                let new = map_tags(&xml, &|n| matches!(n, b"slicer" | b"timeline"), &mut |k, v| match k {
                    b"name" => new_name(v),
                    b"cache" => cloned.get(v).cloned(),
                    _ => None,
                });
                if let Some(x) = new {
                    self.work.small.insert(dst.clone(), x);
                }
            } else if find_tag(&xml, 0, b"xdr:wsDr").is_some() && !renamed.is_empty() {
                // `<sle:slicer name>` / `<tsle:timeslicer name>` no desenho.
                let new = map_tags(
                    &xml,
                    &|n| matches!(local(n), b"slicer" | b"timeslicer" | b"timeline"),
                    &mut |k, v| if k == b"name" { new_name(v) } else { None },
                );
                if let Some(x) = new {
                    self.work.small.insert(dst.clone(), x);
                }
            }
        }
        Ok(())
    }

    /// Cache próprio para a cópia de uma segmentação, se ele depende da aba
    /// copiada. Devolve o nome novo.
    fn clone_cache(
        &mut self,
        name: &str,
        rid: &str,
        kind: &str,
        part: &str,
        (old_id, new_id): (u32, u32),
        table_ids: &HashMap<String, String>,
    ) -> Result<Option<String>, Error> {
        let xml = self.work.read(self.pkg, &self.work.small, part)?;
        let mut linked = false;
        for_each(&xml, b"pivotTable", |a, _| {
            linked |= attr(a, b"tabId").and_then(parse_u32) == Some(old_id);
        });
        for_each(&xml, b"x15:tableSlicerCache", |a, _| {
            linked |= attr_str(a, b"tableId").is_some_and(|t| table_ids.contains_key(&t));
        });
        if !linked {
            return Ok(None);
        }
        let base = name.trim_end_matches(|c: char| c.is_ascii_digit());
        let taken = self.cache_names();
        let new = (1..).map(|n| format!("{base}{n}")).find(|n| !taken.contains(&n.to_lowercase()));
        let new = new.unwrap_or_default();
        let dst = self.free_part(part);
        self.work.added.insert(dst.clone(), part.to_string());
        self.types.push((dst.clone(), part.to_string()));
        if let Ok(r) = self.pkg.read(&rels_path(part)) {
            self.work.small.insert(rels_path(&dst), r);
        }
        // Só as dinâmicas da aba copiada.
        let mut x = keep_pivots(&xml, &|id| id == old_id);
        x = map_tags(
            &x,
            &|n| {
                matches!(
                    local(n),
                    b"slicerCacheDefinition"
                        | b"timelineCacheDefinition"
                        | b"pivotTable"
                        | b"tableSlicerCache"
                )
            },
            &mut |k, v| match k {
                b"name" if v == name => Some(new.clone()),
                b"tabId" => Some(new_id.to_string()),
                b"tableId" => table_ids.get(v).cloned(),
                _ => None,
            },
        )
        .unwrap_or(x);
        self.work.small.insert(dst.clone(), x);
        // Workbook: relação, entrada na lista da extensão e o nome `#N/A`.
        let id = self.add_wb_rel(kind, &dst);
        let wb = std::mem::take(&mut self.wb);
        self.wb = duplicate_ref(&wb, rid, &id);
        if let Some(t) = defined_name(&self.wb, name) {
            let tag = format!("<definedName name=\"{}\">#N/A</definedName>", esc(&new));
            self.wb = [&self.wb[..t], tag.as_bytes(), &self.wb[t..]].concat();
        }
        Ok(Some(new))
    }

    /// Nomes em uso: definidos e de caches.
    fn cache_names(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        for_each(&self.wb, b"definedName", |a, _| out.extend(attr_str(a, b"name").map(|n| n.to_lowercase())));
        for (p, x) in &self.work.small {
            if p.contains("Cache") {
                for tag in [&b"slicerCacheDefinition"[..], b"timelineCacheDefinition"] {
                    if let Some(t) = find_tag(x, 0, tag) {
                        out.extend(attr_str(t.attrs, b"name").map(|n| n.to_lowercase()));
                    }
                }
            }
        }
        for r in parse_rels(&self.wb_rels, &self.book.meta.path) {
            if r.kind.ends_with("/slicerCache") || r.kind.ends_with("/timelineCache") {
                let Ok(x) = self.pkg.read(&r.part) else { continue };
                for tag in [&b"slicerCacheDefinition"[..], b"timelineCacheDefinition"] {
                    if let Some(t) = find_tag(&x, 0, tag) {
                        out.extend(attr_str(t.attrs, b"name").map(|n| n.to_lowercase()));
                    }
                }
            }
        }
        out
    }

    /// Abas excluídas: relações, caches de segmentação e de dinâmica que só
    /// elas usavam. As partes que sobram soltas saem no fim.
    fn drop_tabs(&mut self, gone: &[(String, u32)]) {
        let alive: Vec<String> = self
            .book
            .meta
            .sheets
            .iter()
            .map(|m| m.path.clone())
            .filter(|p| !gone.iter().any(|g| g.0 == *p))
            .collect();
        let rels = parse_rels(&self.wb_rels, &self.book.meta.path);
        let mut drop: HashSet<String> =
            rels.iter().filter(|r| gone.iter().any(|g| g.0 == r.part)).map(|r| r.id.clone()).collect();
        // O que as abas que ficam usam.
        let mut used_caches: HashSet<String> = HashSet::new();
        let mut used_pivots: HashSet<String> = HashSet::new();
        let mut gone_pivots: HashSet<String> = HashSet::new();
        let mut gone_views = false;
        let scan = |sheet: &str, views: &mut HashSet<String>, pivots: &mut HashSet<String>| -> bool {
            let mut any = false;
            for (_, kind, part) in rel_targets_typed(self.pkg, sheet) {
                if kind.ends_with("/slicer") || kind.ends_with("/timeline") {
                    any = true;
                    if let Ok(x) = self.pkg.read(&part) {
                        for tag in [&b"slicer"[..], b"timeline"] {
                            for_each(&x, tag, |a, _| views.extend(attr_str(a, b"cache")));
                        }
                    }
                } else if kind.ends_with("/pivotTable") {
                    for (_, k, cache) in rel_targets_typed(self.pkg, &part) {
                        if k.ends_with("/pivotCacheDefinition") {
                            pivots.insert(cache);
                        }
                    }
                }
            }
            any
        };
        for s in &alive {
            scan(s, &mut used_caches, &mut used_pivots);
        }
        let mut gone_caches = HashSet::new();
        for (s, _) in gone {
            gone_views |= scan(s, &mut gone_caches, &mut gone_pivots);
        }
        // Cópias em andamento usam caches da original.
        for x in self.work.small.values() {
            for tag in [&b"slicer"[..], b"timeline"] {
                for_each(x, tag, |a, _| used_caches.extend(attr_str(a, b"cache")));
            }
        }
        for (dst, src) in &self.work.added {
            if dst.contains("pivotTable") {
                for (_, k, cache) in rel_targets_typed(self.pkg, src) {
                    if k.ends_with("/pivotCacheDefinition") {
                        used_pivots.insert(cache);
                    }
                }
            }
        }
        let mut slicer_pivot_ids: HashSet<String> = HashSet::new();
        let mut drop_names: Vec<String> = Vec::new();
        for r in &rels {
            let tag: &[u8] = if r.kind.ends_with("/slicerCache") {
                b"slicerCacheDefinition"
            } else if r.kind.ends_with("/timelineCache") {
                b"timelineCacheDefinition"
            } else {
                continue;
            };
            let Ok(x) = self.work.read(self.pkg, &self.work.small, &r.part) else { continue };
            let name = find_tag(&x, 0, tag).and_then(|t| attr_str(t.attrs, b"name")).unwrap_or_default();
            if gone_views && !used_caches.contains(&name) {
                drop.insert(r.id.clone());
                drop_names.push(name);
                continue;
            }
            for t in [&b"tabular"[..], b"olap", b"state"] {
                for_each(&x, t, |a, _| slicer_pivot_ids.extend(attr_str(a, b"pivotCacheId")));
            }
            let new = keep_pivots(&x, &|id| !gone.iter().any(|g| g.1 == id));
            if new != x {
                self.work.small.insert(r.part.clone(), new);
            }
        }
        for r in &rels {
            if !r.kind.ends_with("/pivotCacheDefinition")
                || used_pivots.contains(&r.part)
                || !gone_pivots.contains(&r.part)
            {
                continue;
            }
            // Uma segmentação ainda aponta para o cache.
            let x14 = self.pkg.read(&r.part).ok().and_then(|x| {
                find_tag(&x, 0, b"x14:pivotCacheDefinition").and_then(|t| attr_str(t.attrs, b"pivotCacheId"))
            });
            if x14.is_some_and(|id| slicer_pivot_ids.contains(&id)) {
                continue;
            }
            drop.insert(r.id.clone());
        }
        self.wb_rels = drop_rels(&self.wb_rels, &|id| drop.contains(id));
        let wb = std::mem::take(&mut self.wb);
        self.wb = drop_refs(&wb, &drop);
        for n in drop_names {
            while let Some(at) = defined_name(&self.wb, &n) {
                let Some(t) = find_tag(&self.wb, at, b"definedName") else { break };
                let end = element_end(&self.wb, &t, b"definedName");
                let s = t.start;
                self.wb.drain(s..end);
            }
        }
    }

    /// Troca nomes de aba numa parte que não é de aba.
    fn retext_part(&mut self, part: &str, map: &SheetMap) {
        if map.is_empty() || part.ends_with(".rels") {
            return;
        }
        let Ok(x) = self.work.read(self.pkg, &self.work.small, part) else { return };
        let formula = |t: &str| map.apply(t);
        let new = if memmem::find(&x, b"wsDr").is_some() {
            map_tags(&x, &|n| matches!(local(n), b"sp" | b"cameraTool"), &mut |k, v| match k {
                b"textlink" | b"cellRange" => map.apply(v),
                _ => None,
            })
        } else if memmem::find(&x, b"chartSpace").is_some() {
            // Gráfico dinâmico: `[arquivo]Aba!Tabela` na fonte.
            let source = |t: &str| {
                let (a, rest) = t.strip_prefix('[')?.split_once(']')?;
                let (sheet, name) = rest.rsplit_once('!')?;
                let n = map.name(sheet)??;
                Some(format!("[{a}]{n}!{name}"))
            };
            map_texts(&x, &|n| matches!(local(n), b"f" | b"name"), &|t| {
                if t.starts_with('[') { source(t) } else { map.apply(t) }
            })
        } else if find_tag(&x, 0, b"formControlPr").is_some() {
            map_tags(&x, &|n| n == b"formControlPr", &mut |k, v| {
                if k.starts_with(b"fmla") { map.apply(v) } else { None }
            })
        } else if part.ends_with(".vml") {
            map_texts(&x, &|n| n.starts_with(b"x:Fmla"), &formula)
        } else if find_tag(&x, 0, b"pivotCacheDefinition").is_some() {
            map_tags(&x, &|n| n == b"worksheetSource", &mut |k, v| {
                if k == b"sheet" { map.name(v).flatten().map(str::to_string) } else { None }
            })
        } else {
            None
        };
        if let Some(n) = new {
            self.work.small.insert(part.to_string(), n);
        }
        // Links dos desenhos (`#Aba!A1`).
        let rp = rels_path(part);
        let Ok(r) = self.work.read(self.pkg, &self.work.small, &rp) else { return };
        let new = map_tags(&r, &|n| n == b"Relationship", &mut |k, v| {
            if k != b"Target" || !v.starts_with('#') {
                return None;
            }
            map.apply_location(&percent_decode(v)).map(|n| n.replace(' ', "%20"))
        });
        if let Some(n) = new {
            self.work.small.insert(rp, n);
        }
    }

    /// `<sheets>`, nomes definidos e a aba ativa.
    fn workbook(&mut self, plan: &TabPlan, edits: &[(&str, &duck_core::Edits)]) {
        let wb = std::mem::take(&mut self.wb);
        let meta = &self.book.meta.sheets;
        let rels = parse_rels(&self.wb_rels, &self.book.meta.path);
        let rid_of =
            |part: &str| rels.iter().find(|r| r.part == part).map(|r| r.id.clone()).unwrap_or_default();
        // Tags `<sheet>` do arquivo, na ordem; as que não são planilhas
        // (gráficos) andam com a planilha de antes delas.
        struct Raw {
            tag: Vec<u8>,
            /// Planilha do modelo, se for uma que ficou.
            model: Option<usize>,
            /// Outro tipo de aba.
            other: bool,
        }
        let Some(list) = find_tag(&wb, 0, b"sheets") else {
            self.wb = wb;
            return;
        };
        let body = inner(&wb, &list, b"sheets");
        let body_start = list.end;
        let list_end = body_start + body.len();
        let mut raws: Vec<Raw> = Vec::new();
        let mut pos = 0;
        while let Some(t) = find_tag(body, pos, b"sheet") {
            let end = element_end(body, &t, b"sheet");
            let tab = raws.len();
            let file = meta.iter().position(|m| m.tab == tab);
            let model = file.and_then(|f| {
                plan.tabs.iter().position(|x| x.origin == TabOrigin::File(meta[f].path.clone()))
            });
            raws.push(Raw { tag: body[t.start..end].to_vec(), model, other: file.is_none() });
            pos = end;
        }
        // Âncora das outras abas: a planilha viva de antes.
        let mut head: Vec<usize> = Vec::new();
        let mut after: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut last: Option<usize> = None;
        for (k, r) in raws.iter().enumerate() {
            if r.other {
                match last {
                    Some(m) => after.entry(m).or_default().push(k),
                    None => head.push(k),
                }
            } else if r.model.is_some() {
                last = r.model;
            }
        }
        let mut out: Vec<Vec<u8>> = Vec::new();
        let mut new_index: HashMap<usize, usize> = HashMap::new(); // tab antigo -> novo
        let mut model_index: Vec<usize> = Vec::new();
        for &k in &head {
            new_index.insert(k, out.len());
            out.push(raws[k].tag.clone());
        }
        for (i, t) in plan.tabs.iter().enumerate() {
            model_index.push(out.len());
            match raws.iter().position(|r| r.model == Some(i)) {
                Some(k) => {
                    new_index.insert(k, out.len());
                    let was = meta.iter().find(|m| m.tab == k);
                    out.push(sheet_tag(&raws[k].tag, t, was.is_some_and(|m| m.hidden != t.hidden)));
                }
                None => {
                    let mut tag = b"<sheet name=\"".to_vec();
                    escape_attr(&t.name, &mut tag);
                    tag.extend_from_slice(format!("\" sheetId=\"{}\"", t.id).as_bytes());
                    if t.hidden {
                        tag.extend_from_slice(b" state=\"hidden\"");
                    }
                    tag.extend_from_slice(format!(" r:id=\"{}\"/>", rid_of(&self.work.parts[i])).as_bytes());
                    out.push(tag);
                }
            }
            for &k in after.get(&i).into_iter().flatten() {
                new_index.insert(k, out.len());
                out.push(raws[k].tag.clone());
            }
        }
        let mut new = wb[..body_start].to_vec();
        for t in &out {
            new.extend_from_slice(t);
        }
        new.extend_from_slice(&wb[list_end..]);
        // Aba ativa: a mesma do arquivo, se continua visível; senão, a primeira visível.
        let visible = |k: usize| {
            out.get(k).is_some_and(|t| {
                !matches!(find_tag(t, 0, b"sheet").and_then(|t| attr(t.attrs, b"state")), Some(s) if s != b"visible")
            })
        };
        new = map_active(&new, &mut |a| {
            let same = new_index.get(&(a as usize)).copied().filter(|&k| visible(k));
            same.or_else(|| (0..out.len()).find(|&k| visible(k))).unwrap_or(0) as u32
        });
        // A aba que estava selecionada e foi escondida perde a marca.
        for m in meta {
            if let Some(i) = plan.tabs.iter().position(|t| t.origin == TabOrigin::File(m.path.clone()))
                && plan.tabs[i].hidden
                && !m.hidden
            {
                self.work.deselect.insert(i);
            }
        }
        // Nomes definidos.
        new = moved_names(new, edits);
        self.wb = self.names(&new, plan, &new_index, &model_index);
    }

    fn names(
        &self,
        wb: &[u8],
        plan: &TabPlan,
        new_index: &HashMap<usize, usize>,
        model_index: &[usize],
    ) -> Vec<u8> {
        let mut present: HashSet<(String, Option<usize>)> = HashSet::new();
        let mut items: Vec<u8> = Vec::new();
        let list = find_tag(wb, 0, b"definedNames");
        let body: &[u8] = list.as_ref().map_or(&[], |t| inner(wb, t, b"definedNames"));
        let mut pos = 0;
        while let Some(t) = find_tag(body, pos, b"definedName") {
            let end = element_end(body, &t, b"definedName");
            pos = end;
            let local = attr(t.attrs, b"localSheetId").and_then(parse_u32).map(|x| x as usize);
            let at = match local {
                Some(l) => match new_index.get(&l) {
                    Some(&n) => Some(n),
                    None => continue, // a aba saiu
                },
                None => None,
            };
            let name = attr_str(t.attrs, b"name").unwrap_or_default();
            present.insert((name.to_lowercase(), at));
            items.extend_from_slice(b"<definedName");
            for_each_attr(t.attrs, |k, whole| {
                items.push(b' ');
                match (k, at) {
                    (b"localSheetId", Some(n)) => {
                        items.extend_from_slice(format!("localSheetId=\"{n}\"").as_bytes())
                    }
                    _ => items.extend_from_slice(whole),
                }
            });
            items.push(b'>');
            let raw = if t.self_closing { &[][..] } else { &body[t.end..end - b"</definedName>".len()] };
            let mut text = String::new();
            unescape_into(raw, &mut text);
            match self.work.map.apply(&text) {
                Some(n) => escape_text(&n, &mut items),
                None => items.extend_from_slice(raw),
            }
            items.extend_from_slice(b"</definedName>");
        }
        for n in &plan.names {
            let at = match n.sheet {
                Some(s) => match model_index.get(s as usize) {
                    Some(&k) => Some(k),
                    None => continue,
                },
                None => None,
            };
            if !present.insert((n.name.to_lowercase(), at)) {
                continue;
            }
            items.extend_from_slice(b"<definedName name=\"");
            escape_attr(&n.name, &mut items);
            items.push(b'"');
            if let Some(k) = at {
                items.extend_from_slice(format!(" localSheetId=\"{k}\"").as_bytes());
            }
            if n.hidden {
                items.extend_from_slice(b" hidden=\"1\"");
            }
            items.push(b'>');
            escape_text(&n.formula, &mut items);
            items.extend_from_slice(b"</definedName>");
        }
        let whole = if items.is_empty() {
            Vec::new()
        } else {
            [b"<definedNames>".as_slice(), &items, b"</definedNames>"].concat()
        };
        if let Some(t) = list {
            let end = element_end(wb, &t, b"definedNames");
            return [&wb[..t.start], &whole[..], &wb[end..]].concat();
        }
        if whole.is_empty() {
            return wb.to_vec();
        }
        const AFTER: [&[u8]; 10] = [
            b"calcPr",
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
            .filter_map(|n| find_tag(wb, 0, n).map(|t| t.start))
            .min()
            .or_else(|| memmem::rfind(wb, b"</"))
            .unwrap_or(wb.len());
        [&wb[..at], &whole[..], &wb[at..]].concat()
    }

    /// Partes alcançadas pelas relações, a partir da raiz.
    fn reachable(&self, current: bool) -> HashSet<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::from([String::new()]);
        while let Some(p) = queue.pop_front() {
            let rp = rels_path(&p);
            let xml = if current {
                if rp == rels_path(&self.book.meta.path) {
                    Some(self.wb_rels.clone())
                } else {
                    let src = self.work.added.get(&p).map(|s| rels_path(s)).unwrap_or(rp.clone());
                    self.work.small.get(&rp).cloned().or_else(|| self.pkg.read(&src).ok())
                }
            } else {
                self.pkg.read(&rp).ok()
            };
            let Some(xml) = xml else { continue };
            for r in parse_rels(&xml, &p) {
                if !r.part.is_empty() && seen.insert(r.part.clone()) {
                    queue.push_back(r.part);
                }
            }
        }
        seen
    }

    /// `[Content_Types].xml` com as partes que entraram e sem as que saíram.
    fn content_types(&mut self) -> Result<(), Error> {
        let mut ct = self.pkg.read("[Content_Types].xml")?;
        let mut add = Vec::new();
        for (dst, src) in &self.types {
            let mut kind = None;
            for_each(&ct, b"Override", |a, _| {
                if attr_str(a, b"PartName").is_some_and(|p| p.trim_start_matches('/') == src) {
                    kind = attr(a, b"ContentType").map(<[u8]>::to_vec);
                }
            });
            if let Some(k) = kind {
                add.extend_from_slice(b"<Override PartName=\"/");
                escape_attr(dst, &mut add);
                add.extend_from_slice(b"\" ContentType=\"");
                add.extend_from_slice(&k);
                add.extend_from_slice(b"\"/>");
            }
        }
        if !self.work.removed.is_empty() {
            let mut out = Vec::with_capacity(ct.len());
            let mut pos = 0;
            while let Some(t) = find_tag(&ct, pos, b"Override") {
                out.extend_from_slice(&ct[pos..t.start]);
                let end = element_end(&ct, &t, b"Override");
                let gone = attr_str(t.attrs, b"PartName")
                    .is_some_and(|p| self.work.removed.contains(p.trim_start_matches('/')));
                if !gone {
                    out.extend_from_slice(&ct[t.start..end]);
                }
                pos = end;
            }
            out.extend_from_slice(&ct[pos..]);
            ct = out;
        }
        if !add.is_empty() {
            ct = insert_before(&ct, b"</Types>", &add);
        }
        self.work.small.insert("[Content_Types].xml".into(), ct);
        Ok(())
    }
}

/// Prepara o pacote para as abas de `plan`. `None` = nada muda.
pub(super) fn prepare(book: &Book, plan: &TabPlan, sheets: &[Option<&Sheet>]) -> Result<Option<Work>, Error> {
    if plan.tabs.len() != sheets.len() {
        return Err(Error::Format("plano de abas diferente do modelo".into()));
    }
    if !plan.changes(book) {
        return Ok(None);
    }
    let pkg = &book.pkg;
    let meta = &book.meta;
    let order: Vec<&str> = meta.sheets.iter().map(|m| m.name.as_str()).collect();
    let mut map = SheetMap::with_order(&order);
    let mut gone = Vec::new();
    for m in &meta.sheets {
        match plan.tabs.iter().find(|t| t.origin == TabOrigin::File(m.path.clone())) {
            Some(t) => map.add(&m.name, Some(&t.name)),
            None => {
                map.add(&m.name, None);
                gone.push((m.path.clone(), m.id));
            }
        }
    }
    for t in &plan.tabs {
        if let TabOrigin::File(p) = &t.origin
            && !meta.sheets.iter().any(|m| m.path == *p)
        {
            return Err(Error::Missing(p.clone()));
        }
    }
    let wb = pkg.read(&meta.path)?;
    let wb_rels = pkg.read(&rels_path(&meta.path))?;
    let rel_ids = parse_rels(&wb_rels, &meta.path).into_iter().map(|r| r.id).collect();
    let mut b = Builder {
        book,
        pkg,
        work: Work { map, ..Default::default() },
        used: pkg.entries().map(|e| e.name.clone()).collect(),
        types: Vec::new(),
        wb,
        wb_rels,
        rel_ids,
    };
    b.add_tabs(plan, sheets)?;
    b.work.structure = !gone.is_empty() || b.work.new.iter().any(Option::is_some);
    if !gone.is_empty() {
        b.drop_tabs(&gone);
    }
    // Mudanças de linhas e colunas nas abas do arquivo (nomes definidos).
    let edits: Vec<(&str, &duck_core::Edits)> = plan
        .tabs
        .iter()
        .zip(sheets)
        .filter_map(|(t, s)| {
            let TabOrigin::File(p) = &t.origin else { return None };
            let m = meta.sheets.iter().find(|m| m.path == *p)?;
            s.map(|s| (m.name.as_str(), s.edits()))
        })
        .collect();
    b.workbook(plan, &edits);
    if !gone.is_empty() {
        let before = b.reachable(false);
        let after = b.reachable(true);
        let mut removed: HashSet<String> = before.difference(&after).cloned().collect();
        let rels: Vec<String> =
            removed.iter().map(|p| rels_path(p)).filter(|p| pkg.entry(p).is_some()).collect();
        removed.extend(rels);
        b.work.small.retain(|p, _| !removed.contains(p));
        b.work.removed = removed;
    }
    // Textos que citam abas, nas partes que ficam.
    if !b.work.map.is_empty() {
        let sheet_parts: HashSet<&str> = meta.sheets.iter().map(|m| m.path.as_str()).collect();
        let copies: HashSet<String> = b.work.added.keys().cloned().collect();
        let parts: Vec<String> = b
            .reachable(true)
            .into_iter()
            .filter(|p| !sheet_parts.contains(p.as_str()) && !copies.contains(p) && *p != meta.path)
            .filter(|p| {
                pkg.entry(p).is_some()
                    && ["drawing", "chart", "ctrlProp", "pivotCacheDefinition"].iter().any(|k| p.contains(k))
            })
            .collect();
        let map = b.work.map.clone();
        for p in parts {
            b.retext_part(&p, &map);
        }
    }
    b.content_types()?;
    // As propriedades do documento listam as abas: o Excel refaz a lista.
    if let Ok(app) = pkg.read("docProps/app.xml") {
        let mut x = app;
        for name in [&b"HeadingPairs"[..], b"TitlesOfParts"] {
            if let Some(t) = find_tag(&x, 0, name) {
                let (s, e) = (t.start, element_end(&x, &t, name));
                x.drain(s..e);
            }
        }
        b.work.small.insert("docProps/app.xml".into(), x);
    }
    let Builder { mut work, wb, wb_rels, .. } = b;
    work.small.insert(meta.path.clone(), wb);
    work.small.insert(rels_path(&meta.path), wb_rels);
    Ok(Some(work))
}

/// Uma tag `<sheet>` do arquivo com o nome (e, se mudou, o estado) do modelo.
fn sheet_tag(tag: &[u8], t: &TabOut, state_changed: bool) -> Vec<u8> {
    let Some(x) = find_tag(tag, 0, b"sheet") else { return tag.to_vec() };
    let mut out = b"<sheet".to_vec();
    for_each_attr(x.attrs, |k, whole| {
        if k == b"state" && state_changed {
            return;
        }
        out.push(b' ');
        if k == b"name" {
            out.extend_from_slice(b"name=\"");
            escape_attr(&t.name, &mut out);
            out.push(b'"');
        } else if k == b"r:id" && state_changed && t.hidden {
            out.extend_from_slice(b"state=\"hidden\" ");
            out.extend_from_slice(whole);
        } else {
            out.extend_from_slice(whole);
        }
    });
    out.extend_from_slice(if x.self_closing { b"/>" } else { b">" });
    out.extend_from_slice(&tag[x.end..]);
    out
}

/// Troca o `activeTab` de cada `<workbookView>` (padrão 0) e tira o
/// `firstSheet` (pode apontar para uma aba que saiu).
fn map_active(wb: &[u8], f: &mut dyn FnMut(u32) -> u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(wb.len());
    let mut pos = 0;
    while let Some(t) = find_tag(wb, pos, b"workbookView") {
        out.extend_from_slice(&wb[pos..t.start]);
        let old = attr(t.attrs, b"activeTab").and_then(parse_u32).unwrap_or(0);
        let new = f(old);
        out.extend_from_slice(b"<workbookView");
        for_each_attr(t.attrs, |k, whole| {
            if !matches!(k, b"activeTab" | b"firstSheet") {
                out.push(b' ');
                out.extend_from_slice(whole);
            }
        });
        if new > 0 {
            out.extend_from_slice(format!(" activeTab=\"{new}\"").as_bytes());
        }
        out.extend_from_slice(if t.self_closing { b"/>" } else { b">" });
        pos = t.end;
    }
    out.extend_from_slice(&wb[pos..]);
    out
}

/// Tira os `<pivotTable tabId>` (de um cache de segmentação) que `keep`
/// recusar; a lista vazia sai inteira.
fn keep_pivots(xml: &[u8], keep: &dyn Fn(u32) -> bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut pos = 0;
    while let Some(t) = find_tag(xml, pos, b"pivotTable") {
        out.extend_from_slice(&xml[pos..t.start]);
        let end = element_end(xml, &t, b"pivotTable");
        if attr(t.attrs, b"tabId").and_then(parse_u32).is_none_or(keep) {
            out.extend_from_slice(&xml[t.start..end]);
        }
        pos = end;
    }
    out.extend_from_slice(&xml[pos..]);
    if let Some(t) = find_tag(&out, 0, b"pivotTables")
        && inner(&out, &t, b"pivotTables").iter().all(u8::is_ascii_whitespace)
    {
        let (s, e) = (t.start, element_end(&out, &t, b"pivotTables"));
        out.drain(s..e);
    }
    out
}

/// Repete, logo depois, o elemento vazio que aponta para a relação `rid`,
/// com o id `new` (`<x14:slicerCache r:id="rId9"/>`).
fn duplicate_ref(xml: &[u8], rid: &str, new: &str) -> Vec<u8> {
    let pat = format!("r:id=\"{rid}\"");
    let Some(k) = memmem::find(xml, pat.as_bytes()) else { return xml.to_vec() };
    let Some(lt) = memchr::memrchr(b'<', &xml[..k]) else { return xml.to_vec() };
    let Some(end) = tag_end(xml, lt) else { return xml.to_vec() };
    if xml[end - 2] != b'/' {
        return xml.to_vec();
    }
    let tag = String::from_utf8_lossy(&xml[lt..end]).replace(&pat, &format!("r:id=\"{new}\""));
    [&xml[..end], tag.as_bytes(), &xml[end..]].concat()
}

/// Tira os elementos que apontam para relações que saíram (abas, caches) e
/// as listas que ficaram vazias.
fn drop_refs(xml: &[u8], drop: &HashSet<String>) -> Vec<u8> {
    let mut out = xml.to_vec();
    for id in drop {
        let pat = format!("r:id=\"{id}\"");
        let mut from = 0;
        while let Some(k) = memmem::find(&out[from..], pat.as_bytes()).map(|k| from + k) {
            let Some(lt) = memchr::memrchr(b'<', &out[..k]) else { break };
            let Some(end) = tag_end(&out, lt) else { break };
            // `<sheet>` é refeito por quem chama: aqui só o que aponta para caches.
            if out[lt..].starts_with(b"<sheet ") {
                from = end;
                continue;
            }
            from = lt;
            let end = if out[end - 2] == b'/' {
                end
            } else {
                let n = out[lt + 1..end].iter().position(|&c| is_name_end(c)).unwrap_or(0);
                let close = [b"</", &out[lt + 1..lt + 1 + n], b">"].concat();
                memmem::find(&out[end..], &close).map_or(end, |x| end + x + close.len())
            };
            out.drain(lt..end);
        }
    }
    for name in
        ["pivotCaches", "x14:slicerCaches", "x15:slicerCaches", "x15:timelineCacheRefs", "x14:pivotCaches"]
    {
        let n = name.as_bytes();
        while let Some(t) = find_tag(&out, 0, n) {
            if t.self_closing || !inner(&out, &t, n).iter().all(u8::is_ascii_whitespace) {
                break;
            }
            let (mut s, mut e) = (t.start, element_end(&out, &t, n));
            // Dentro de `<ext>` sozinho: sai o `<ext>` também.
            if let Some(lt) = memchr::memrchr(b'<', &out[..s])
                && out[lt..].starts_with(b"<ext ")
                && tag_end(&out, lt) == Some(s)
                && out[e..].starts_with(b"</ext>")
            {
                s = lt;
                e += b"</ext>".len();
            }
            out.drain(s..e);
        }
    }
    if let Some(t) = find_tag(&out, 0, b"extLst")
        && !t.self_closing
        && inner(&out, &t, b"extLst").iter().all(u8::is_ascii_whitespace)
    {
        let (s, e) = (t.start, element_end(&out, &t, b"extLst"));
        out.drain(s..e);
    }
    out
}

/// Posição do `<definedName>` (do arquivo todo) com este nome.
fn defined_name(wb: &[u8], name: &str) -> Option<usize> {
    let mut pos = 0;
    while let Some(t) = find_tag(wb, pos, b"definedName") {
        if attr_str(t.attrs, b"name").as_deref() == Some(name) {
            return Some(t.start);
        }
        pos = t.end;
    }
    None
}

fn esc(s: &str) -> String {
    let mut v = Vec::new();
    escape_attr(s, &mut v);
    String::from_utf8(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{CellValue, Value};

    #[test]
    fn pieces() {
        let x = br#"<c><pivotTables><pivotTable tabId="3" name="a"/><pivotTable tabId="4" name="b"/></pivotTables></c>"#;
        assert_eq!(
            keep_pivots(x, &|id| id == 4),
            br#"<c><pivotTables><pivotTable tabId="4" name="b"/></pivotTables></c>"#
        );
        assert_eq!(keep_pivots(x, &|_| false), b"<c></c>");

        let wb = br#"<workbook><sheets><sheet name="A" r:id="rId1"/></sheets><pivotCaches><pivotCache cacheId="1" r:id="rId9"/></pivotCaches><extLst><ext uri="{x}" xmlns:x14="y"><x14:slicerCaches><x14:slicerCache r:id="rId7"/></x14:slicerCaches></ext></extLst></workbook>"#;
        let two = duplicate_ref(wb, "rId7", "rIdAba1");
        let both = r#"<x14:slicerCache r:id="rId7"/><x14:slicerCache r:id="rIdAba1"/>"#;
        assert!(String::from_utf8_lossy(&two).contains(both));
        let drop = ["rId7", "rId9", "rId1"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            String::from_utf8(drop_refs(wb, &drop)).unwrap(),
            r#"<workbook><sheets><sheet name="A" r:id="rId1"/></sheets></workbook>"#
        );

        let t = TabOut { origin: TabOrigin::Blank, name: "A & B".into(), id: 1, hidden: true };
        assert_eq!(
            sheet_tag(br#"<sheet name="x" sheetId="1" r:id="rId1"/>"#, &t, true),
            br#"<sheet name="A &amp; B" sheetId="1" state="hidden" r:id="rId1"/>"#
        );
        let t = TabOut { hidden: false, ..t };
        assert_eq!(
            sheet_tag(br#"<sheet name="x" sheetId="1" state="hidden" r:id="rId1"/>"#, &t, true),
            br#"<sheet name="A &amp; B" sheetId="1" r:id="rId1"/>"#
        );
    }

    #[test]
    fn tail_and_head() {
        let m = SheetMap::rename("Velha", "Nova aba");
        let x = br#"<conditionalFormatting sqref="A1"><cfRule><formula>Velha!$A$1&gt;0</formula></cfRule></conditionalFormatting><dataValidations><dataValidation sqref="A1"><formula1>Velha!$A$1:$A$3</formula1></dataValidation></dataValidations><hyperlinks><hyperlink ref="A1" location="Velha!B2" display="x"/></hyperlinks><ext><x14:sparkline><xm:f>Velha!A1:C1</xm:f></x14:sparkline></ext></worksheet>"#;
        let out = String::from_utf8(retext_tail(x, &m).unwrap()).unwrap();
        assert_eq!(out.matches("'Nova aba'!").count(), 4, "{out}");
        assert!(!out.contains("Velha"));
        assert!(retext_tail(b"<pageMargins/></worksheet>", &m).is_none());

        let mut s = Sheet::new("t");
        duck_core::Command::TabColor(Some(duck_core::Color::Rgb(0xFF00_FF00))).apply(&mut s);
        let h = patch_head(br#"<worksheet xmlns="m"><dimension ref="A1"/>"#, &s, false, false);
        assert_eq!(
            String::from_utf8(h).unwrap(),
            r#"<worksheet xmlns="m"><sheetPr><tabColor rgb="FF00FF00"/></sheetPr><dimension ref="A1"/>"#
        );
        let h = patch_head(
            br#"<worksheet xmlns="m" xr:uid="{1}"><sheetPr codeName="Plan1"><tabColor theme="4"/><outlinePr/></sheetPr><sheetViews><sheetView tabSelected="1" workbookViewId="0"/></sheetViews>"#,
            &s,
            false,
            true,
        );
        assert_eq!(
            String::from_utf8(h).unwrap(),
            r#"<worksheet xmlns="m"><sheetPr><tabColor rgb="FF00FF00"/><outlinePr/></sheetPr><sheetViews><sheetView workbookViewId="0"/></sheetViews>"#
        );
        duck_core::Command::TabColor(None).apply(&mut s);
        let h = patch_head(br#"<worksheet><sheetPr><tabColor theme="4"/></sheetPr>"#, &s, false, false);
        assert_eq!(h, b"<worksheet><sheetPr></sheetPr>");
    }

    /// Cria, copia, renomeia, move, esconde e exclui; salva e relê.
    #[test]
    fn tabs_round_trip() {
        let dir = std::env::temp_dir().join(format!("duck-abas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (p, q, r) = (dir.join("a.xlsx"), dir.join("b.xlsx"), dir.join("c.xlsx"));
        super::super::write_blank(&p).unwrap();
        let (mut wb, _) = crate::open(&p, &crate::Options::default()).unwrap();
        let mut one = wb.sheets.remove(0);
        one.set_cell(0, 0, CellValue::Number(7.0), None, None);
        one.name = "Dados <1>".into();
        let mut two = Sheet::new("Conta");
        two.tab_id = 2;
        two.set_cell(0, 0, CellValue::Number(14.0), Some("'Dados <1>'!A1*2"), None);
        let mut three = one.clone();
        three.name = "Dados <1> (2)".into();
        three.tab_id = 3;
        three.hidden = true;
        let part = "xl/worksheets/sheet1.xml".to_string();
        let tab = |origin, s: &Sheet| TabOut { origin, name: s.name.clone(), id: s.tab_id, hidden: s.hidden };
        let names = vec![
            DefinedName { name: "Taxa".into(), sheet: None, formula: "Conta!$A$1".into(), hidden: false },
            DefinedName { name: "Local".into(), sheet: Some(0), formula: "Conta!$A$2".into(), hidden: false },
        ];
        let plan = TabPlan {
            tabs: vec![
                tab(TabOrigin::Blank, &two),
                tab(TabOrigin::Copy { part: part.clone(), renamed: Vec::new() }, &three),
                tab(TabOrigin::File(part), &one),
            ],
            names,
        };
        let opts = crate::SaveOptions { tabs: Some(plan), ..Default::default() };
        let sheets = [Some(&two), Some(&three), Some(&one)];
        let rep = crate::save_file(&p, &q, &sheets, &wb.shared_strings, &wb.styles, &opts).unwrap();
        assert_eq!(rep.sheet_parts.len(), 3);
        let (back, _) = crate::open(&q, &crate::Options::default()).unwrap();
        let got: Vec<_> = back.sheets.iter().map(|s| (s.name.as_str(), s.hidden, s.tab_id)).collect();
        assert_eq!(got, vec![("Conta", false, 2), ("Dados <1> (2)", true, 3), ("Dados <1>", false, 1)]);
        assert_eq!(duck_calc::cell_formula(&back.sheets[0], 0, 0).unwrap(), "'Dados <1>'!A1*2");
        assert_eq!(back.cell(1, 0, 0).value, Value::Number(7.0));
        assert_eq!(back.extras.names.len(), 2);
        assert_eq!(back.extras.names[1].sheet, Some(0));
        // Agora exclui a primeira e troca o nome da segunda.
        let mut keep: Vec<Sheet> = back.sheets.into_iter().skip(1).collect();
        keep[0].hidden = false;
        keep[0].name = "Só".into();
        let plan = TabPlan {
            tabs: keep
                .iter()
                .zip(&rep.sheet_parts[1..])
                .map(|(s, p)| tab(TabOrigin::File(p.clone()), s))
                .collect(),
            names: Vec::new(),
        };
        let opts = crate::SaveOptions { tabs: Some(plan), ..Default::default() };
        let sheets: Vec<Option<&Sheet>> = keep.iter().map(Some).collect();
        let rep = crate::save_file(&q, &r, &sheets, &back.shared_strings, &back.styles, &opts).unwrap();
        assert!(rep.parts_removed.iter().any(|p| p.contains("worksheets/sheet")));
        let (back, _) = crate::open(&r, &crate::Options::default()).unwrap();
        let got: Vec<_> = back.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(got, vec!["Só", "Dados <1>"]);
        // `Taxa` fica (agora #REF!); `Local` saiu com a aba.
        assert_eq!(back.extras.names.len(), 1);
        assert_eq!(back.extras.names[0].formula, "#REF!");
        // Sem mudança nas abas, o plano não regrava nada.
        let plan = TabPlan {
            tabs: back
                .sheets
                .iter()
                .zip(&rep.sheet_parts)
                .map(|(s, p)| tab(TabOrigin::File(p.clone()), s))
                .collect(),
            names: Vec::new(),
        };
        let opts = crate::SaveOptions { tabs: Some(plan), ..Default::default() };
        let sheets: Vec<Option<&Sheet>> = back.sheets.iter().map(Some).collect();
        let rep = crate::save_file(&r, &q, &sheets, &back.shared_strings, &back.styles, &opts).unwrap();
        assert_eq!((rep.parts_rewritten, rep.parts_added.len()), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
