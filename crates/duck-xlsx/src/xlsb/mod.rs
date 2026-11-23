//! Leitura de `.xlsb` (pasta de trabalho binária, MS-XLSB / BIFF12).
//!
//! O pacote é o mesmo zip do `.xlsx`, mas a pasta, as abas, os estilos, as
//! tabelas e os arquivos externos são fluxos de registros binários (`.bin`);
//! desenhos, gráficos, tema e relacionamentos continuam XML. A leitura é em
//! duas partes:
//!
//! - **Células** ([`sheet`]): do binário direto para os blocos colunares do
//!   `duck-core`, em pedaços e em paralelo, como nas abas em XML. As fórmulas
//!   ([`ptg`]) viram modelos relativos, sem passar pelo texto A1.
//! - **Todo o resto**: o [`Layer`] apresenta as partes `.bin` como se fossem
//!   as `.xml` equivalentes (traduzidas na hora em que são lidas). Os leitores
//!   de estilos, tabelas, formatação condicional, desenhos... são os mesmos do
//!   `.xlsx`, e "salvar como `.xlsx`" (`write/convert.rs`) grava essas partes.
//!
//! Partes sem tradução (tabelas dinâmicas, segmentações, comentários...) ficam
//! fora dos relacionamentos, e [`Layer::left_out`] diz quais são.

mod book;
mod frame;
mod ftab;
mod out;
mod ptg;
mod rec;
pub(crate) mod sheet;
mod strings;
mod styles;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::Error;
use crate::package::Package;
use crate::parts;
use crate::xml::{attr as xml_attr, attr_str, find_tag, for_each, inner, tag_end};

pub(crate) use strings::{read as read_strings, to_xml as strings_xml};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Workbook,
    Sheet,
    Styles,
    Strings,
    Table,
    External,
}

impl Kind {
    /// Tipo de conteúdo da parte em XML.
    pub fn content_type(self) -> &'static str {
        match self {
            Kind::Workbook => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            Kind::Sheet => "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml",
            Kind::Styles => "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml",
            Kind::Strings => "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml",
            Kind::Table => "application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml",
            Kind::External => "application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml",
        }
    }

    /// Relacionamentos da parte que têm leitura.
    fn keeps(self, rel: &str) -> bool {
        let last = rel.rsplit('/').next().unwrap_or(rel);
        match self {
            Kind::Workbook => matches!(
                last,
                "worksheet" | "sharedStrings" | "styles" | "theme" | "externalLink" | "vbaProject"
            ),
            Kind::Sheet => matches!(last, "drawing" | "table" | "hyperlink"),
            Kind::External => last.starts_with("externalLinkPath") || last.starts_with("xlExternalLinkPath"),
            _ => false,
        }
    }
}

/// Um relacionamento como está no arquivo.
struct Rel {
    id: String,
    kind: String,
    /// Alvo resolvido (parte do pacote) ou o endereço externo.
    target: String,
    external: bool,
}

/// As partes `.bin` de um `.xlsb`, apresentadas como `.xml`.
pub struct Layer {
    /// Nome apresentado (`xl/workbook.xml`) → parte real e o que ela é.
    parts: HashMap<String, (String, Kind)>,
    /// Parte real → nome apresentado.
    shown: HashMap<String, String>,
    /// Nome apresentado da pasta.
    workbook: String,
    names: OnceLock<ptg::Names>,
    /// Fórmulas e regras que não deu para ler.
    unread: AtomicU64,
    /// Os nomes definidos já entraram na conta (a pasta é traduzida várias vezes).
    counted: AtomicBool,
}

fn rels_of(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// Parte dona de um `.rels` (`xl/_rels/a.bin.rels` → `xl/a.bin`; vazio = o pacote).
fn owner_of(rels: &str) -> Option<String> {
    let (dir, file) = rels.rsplit_once("_rels/")?;
    Some(format!("{dir}{}", file.strip_suffix(".rels")?))
}

fn raw_rels(pkg: &Package, part: &str) -> Vec<Rel> {
    let dir = part.rsplit_once('/').map_or("", |p| p.0);
    let path = if part.is_empty() { "_rels/.rels".to_string() } else { rels_of(part) };
    let Ok(buf) = pkg.read_raw(&path) else { return Vec::new() };
    let mut out = Vec::new();
    for_each(&buf, b"Relationship", |a, _| {
        if let (Some(id), Some(kind), Some(target)) =
            (attr_str(a, b"Id"), attr_str(a, b"Type"), attr_str(a, b"Target"))
        {
            let external = xml_attr(a, b"TargetMode") == Some(b"External");
            let target = if external { target } else { parts::resolve(dir, &target) };
            out.push(Rel { id, kind, target, external });
        }
    });
    out
}

impl Layer {
    /// A camada do pacote, se ele for um `.xlsb`.
    pub fn new(pkg: &Package) -> Option<Self> {
        let root = raw_rels(pkg, "");
        let main = &root.iter().find(|r| r.kind.ends_with("/officeDocument"))?.target;
        if !main.to_ascii_lowercase().ends_with(".bin") {
            return None;
        }
        let mut layer = Self {
            parts: HashMap::new(),
            shown: HashMap::new(),
            workbook: String::new(),
            names: OnceLock::new(),
            unread: AtomicU64::new(0),
            counted: AtomicBool::new(false),
        };
        layer.workbook = layer.add(pkg, main, Kind::Workbook);
        for r in raw_rels(pkg, main) {
            let kind = match r.kind.rsplit('/').next() {
                Some("worksheet") => Kind::Sheet,
                Some("styles") => Kind::Styles,
                Some("sharedStrings") => Kind::Strings,
                Some("externalLink") => Kind::External,
                _ => continue,
            };
            layer.add(pkg, &r.target, kind);
            if kind == Kind::Sheet {
                for t in raw_rels(pkg, &r.target).iter().filter(|t| t.kind.ends_with("/table")) {
                    layer.add(pkg, &t.target, Kind::Table);
                }
            }
        }
        Some(layer)
    }

    fn add(&mut self, pkg: &Package, real: &str, kind: Kind) -> String {
        if let Some(s) = self.shown.get(real) {
            return s.clone();
        }
        let stem = real.strip_suffix(".bin").unwrap_or(real);
        let mut name = format!("{stem}.xml");
        while pkg.has_raw(&name) || self.parts.contains_key(&name) {
            name.insert(name.len() - 4, '_');
        }
        self.parts.insert(name.clone(), (real.to_string(), kind));
        self.shown.insert(real.to_string(), name.clone());
        name
    }

    /// Parte real de um nome apresentado.
    pub fn real(&self, name: &str) -> Option<&str> {
        self.parts.get(name).map(|p| p.0.as_str())
    }

    pub fn kind(&self, name: &str) -> Option<Kind> {
        self.parts.get(name).map(|p| p.1)
    }

    pub fn workbook(&self) -> &str {
        &self.workbook
    }

    /// Conteúdo de uma parte apresentada. `None` = a parte é a do arquivo.
    pub fn read(&self, pkg: &Package, name: &str) -> Option<Result<Vec<u8>, Error>> {
        if let Some((real, kind)) = self.parts.get(name) {
            return Some(self.translate(pkg, real, *kind));
        }
        if let Some(owner) = owner_of(name) {
            return self.rels(pkg, &owner).map(Ok);
        }
        // Desenhos e gráficos apontam para o que não tem leitura.
        if name.contains("/drawings/") && name.ends_with(".xml") {
            return Some(pkg.read_raw(name).map(|x| drawing(&x)));
        }
        if name.contains("/charts/") && name.ends_with(".xml") {
            return Some(pkg.read_raw(name).map(|x| chart(&x)));
        }
        None
    }

    fn translate(&self, pkg: &Package, real: &str, kind: Kind) -> Result<Vec<u8>, Error> {
        let bin = || pkg.read_raw(real);
        let mut unread = 0;
        let xml = match kind {
            Kind::Workbook => {
                let bin = bin()?;
                let sheets = self.sheet_ids(pkg);
                book::Book::parse(&bin).to_xml(self.names(pkg), |t| sheets.contains(&t.rid), &mut unread)
            }
            Kind::Styles => styles::to_xml(&bin()?),
            Kind::Table => book::table_xml(&bin()?, self.names(pkg), &mut unread)
                .ok_or_else(|| Error::Format(format!("tabela inválida em {real}")))?,
            Kind::External => book::external_xml(&bin()?),
            Kind::Strings => strings::to_xml(&strings::read(pkg, real)?),
            Kind::Sheet => {
                let (head, tail) = sheet::frame(pkg, real, self.names(pkg), &mut unread)?;
                [head, "<sheetData/>".into(), tail].concat().into_bytes()
            }
        };
        let first = match kind {
            Kind::Workbook => !self.counted.swap(true, Ordering::Relaxed),
            // As da aba são contadas na leitura das células.
            k => k != Kind::Sheet,
        };
        if first {
            self.count_unread(unread);
        }
        Ok(xml)
    }

    /// `r:id` das abas de planilha.
    fn sheet_ids(&self, pkg: &Package) -> Vec<String> {
        let real = self.real(&self.workbook).unwrap_or_default();
        raw_rels(pkg, real).into_iter().filter(|r| r.kind.ends_with("/worksheet")).map(|r| r.id).collect()
    }

    /// Os relacionamentos de `owner` (nome apresentado; vazio = o pacote), só
    /// com o que tem leitura. `None` = os do arquivo, sem mudança.
    fn rels(&self, pkg: &Package, owner: &str) -> Option<Vec<u8>> {
        let (real, kind) = match self.parts.get(owner) {
            Some((real, kind)) => (real.as_str(), Some(*kind)),
            None if owner.is_empty() => ("", None),
            None => return None,
        };
        let mut s = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for r in raw_rels(pkg, real) {
            let keep = match kind {
                Some(k) => k.keeps(&r.kind),
                None => matches!(
                    r.kind.rsplit('/').next(),
                    Some("officeDocument" | "core-properties" | "extended-properties" | "custom-properties")
                ),
            };
            if !keep {
                continue;
            }
            s.push_str("<Relationship");
            s.push_str(&out::attr("Id", &r.id));
            s.push_str(&out::attr("Type", &r.kind));
            if r.external {
                s.push_str(&out::attr("Target", &r.target));
                s.push_str(" TargetMode=\"External\"/>");
                continue;
            }
            let shown = self.shown.get(&r.target).unwrap_or(&r.target);
            s.push_str(&out::attr("Target", &format!("/{shown}")));
            s.push_str("/>");
        }
        s.push_str("</Relationships>");
        Some(s.into_bytes())
    }

    /// O que as fórmulas citam por número (lido uma vez).
    pub(crate) fn names(&self, pkg: &Package) -> &ptg::Names {
        self.names.get_or_init(|| {
            let real = self.real(&self.workbook).unwrap_or_default();
            let Ok(bin) = pkg.read_raw(real) else { return Default::default() };
            let rels = raw_rels(pkg, real);
            let sheets = self.sheet_ids(pkg);
            let tables = self
                .parts
                .values()
                .filter(|p| p.1 == Kind::Table)
                .filter_map(|p| book::table_columns(&pkg.read_raw(&p.0).ok()?))
                .collect();
            book::Book::parse(&bin).names(
                tables,
                |t| sheets.contains(&t.rid),
                |rid| {
                    rels.iter()
                        .find(|r| r.id == rid)
                        .and_then(|r| pkg.read_raw(&r.target).ok())
                        .map(|b| book::external_names(&b))
                        .unwrap_or_default()
                },
            )
        })
    }

    pub(crate) fn count_unread(&self, n: u64) {
        self.unread.fetch_add(n, Ordering::Relaxed);
    }

    /// Fórmulas e regras lidas até agora que não deu para entender (as
    /// células ficam com o valor gravado).
    pub fn unread(&self) -> u64 {
        self.unread.load(Ordering::Relaxed)
    }

    /// O que o arquivo tem e não é lido: (nome, quantidade).
    pub fn left_out(&self, pkg: &Package) -> Vec<(&'static str, usize)> {
        let mut v: Vec<(&'static str, usize)> = [
            ("tabelas dinâmicas (só os valores)", "/pivotTables/"),
            ("segmentações", "/slicers/"),
            ("linhas do tempo", "/timelines/"),
            ("comentários", "/comments"),
            ("comentários encadeados", "/threadedComments/"),
            ("controles", "/ctrlProps/"),
            ("abas de gráfico", "/chartsheets/"),
            ("abas de macro", "/macrosheets/"),
            ("objetos incorporados", "/embeddings/"),
            ("modelo de dados", "/model/"),
            ("conexões", "/connections"),
        ]
        .into_iter()
        .map(|(label, at)| {
            let n = pkg.raw_names().filter(|n| n.contains(at) && !n.contains("/_rels/")).count();
            (label, n)
        })
        .collect();
        v.retain(|x| x.1 > 0);
        v
    }
}

/// Desenho sem o que aponta para partes que não são lidas: segmentações,
/// linhas do tempo e controles (a forma deles está no VML).
fn drawing(xml: &[u8]) -> Vec<u8> {
    const OUT: [&[u8]; 3] = [b"/drawing/2010/slicer", b"/drawing/2012/timeslicer", b"compatExt"];
    let Some(root) = find_tag(xml, 0, b"xdr:wsDr") else { return xml.to_vec() };
    if root.self_closing {
        return xml.to_vec();
    }
    let mut out = xml[..root.end].to_vec();
    // Filhos diretos da raiz: do `<` de abertura (profundidade 0) ao `>` que fecha.
    let (mut depth, mut start, mut kept) = (0, root.end, root.end);
    let mut i = root.end;
    while let Some(lt) = memchr::memchr(b'<', &xml[i..]).map(|k| i + k) {
        let Some(end) = tag_end(xml, lt) else { break };
        i = end;
        match xml.get(lt + 1) {
            Some(b'!' | b'?') => continue,
            Some(b'/') if depth == 0 => break,
            Some(b'/') => depth -= 1,
            _ => {
                if depth == 0 {
                    start = lt;
                }
                depth += usize::from(xml[end - 2] != b'/');
            }
        }
        if depth == 0 && OUT.iter().any(|n| memchr::memmem::find(&xml[start..end], n).is_some()) {
            out.extend_from_slice(&xml[kept..start]);
            kept = end;
        }
    }
    out.extend_from_slice(&xml[kept..]);
    out
}

/// Gráfico sem a ligação com a tabela dinâmica (que não é lida): sem ela, o
/// Excel mostraria um gráfico dinâmico quebrado.
///
/// Os intervalos de um gráfico dinâmico vão até o fim da tabela, com a linha de
/// total geral, que ele não mostra (fica fora dos valores guardados). Como
/// gráfico comum, eles terminam no último ponto guardado.
fn chart(xml: &[u8]) -> Vec<u8> {
    let Some(t) = find_tag(xml, 0, b"c:pivotSource") else { return xml.to_vec() };
    let end = if t.self_closing {
        t.end
    } else {
        t.end + inner(xml, &t, b"c:pivotSource").len() + b"</c:pivotSource>".len()
    };
    let rest = xml.get(end..).unwrap_or_default();
    let mut out = xml[..t.start].to_vec();
    let mut pos = 0;
    while let Some(f) = find_tag(rest, pos, b"c:f") {
        let text = inner(rest, &f, b"c:f");
        let at = f.end + text.len();
        // O contador de pontos vem logo depois, antes da referência seguinte.
        let next = find_tag(rest, at, b"c:f").map_or(rest.len(), |n| n.start);
        let points = find_tag(&rest[..next], at, b"c:ptCount")
            .and_then(|c| xml_attr(c.attrs, b"val"))
            .and_then(crate::xml::parse_u32);
        out.extend_from_slice(&rest[pos..f.end]);
        match points.and_then(|n| shorten(text, n)) {
            Some(r) => out.extend_from_slice(&r),
            None => out.extend_from_slice(text),
        }
        pos = at;
    }
    out.extend_from_slice(&rest[pos..]);
    out
}

/// `Aba!$B$5:$B$12` com `points` linhas, se tiver mais que isso.
fn shorten(f: &[u8], points: u32) -> Option<Vec<u8>> {
    let last = memchr::memrchr(b'$', f)?;
    let colon = memchr::memrchr(b':', f)?;
    let first = memchr::memrchr(b'$', &f[..colon])?;
    let row = |b: &[u8]| std::str::from_utf8(b).ok()?.parse::<u32>().ok();
    let (r0, r1) = (row(&f[first + 1..colon])?, row(&f[last + 1..])?);
    (points > 1 && r1 >= r0 && r1 - r0 + 1 > points)
        .then(|| [&f[..=last], (r0 + points - 1).to_string().as_bytes()].concat())
}
