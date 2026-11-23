//! `duck-xlsx`: leitura de `.xlsx` / `.xlsm` em streaming e em paralelo (ADR-005).
//!
//! - Arquivo mapeado em memória; partes descompactadas direto do mapeamento.
//! - Strings compartilhadas, estilos e **todas as abas ao mesmo tempo**.
//! - Abas grandes cortadas em pedaços em fronteiras de `</row>` e lidas em
//!   várias threads; partes juntadas em ordem fixa (resultado determinístico).
//! - Sem DOM e sem um objeto por célula: cada pedaço escreve direto nos blocos
//!   colunares do `duck-core`.
//! - Valores em cache do arquivo são usados como estão: nada é recalculado.
//!
//! Use dentro de um pool (`duck_par::pool(n).install(|| duck_xlsx::open(...))`)
//! para escolher o número de threads; fora de um pool, usa o pool global.

mod cond;
pub mod csv;
pub mod drawing;
mod extras;
pub mod filter;
mod package;
mod parts;
pub mod pivot;
#[doc(hidden)]
pub mod sheet;
pub mod write;
pub mod xml;

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use duck_core::{Sheet, StringTable, Styles, Workbook};
use duck_par::InFlight;

pub use package::Package;
pub use sheet::SheetReport;
pub use write::{SaveOptions, SaveReport, save_file};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Zip(String),
    Missing(String),
    Format(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "erro de leitura: {e}"),
            Error::Zip(e) => write!(f, "zip inválido: {e}"),
            Error::Missing(p) => write!(f, "parte ausente no pacote: {p}"),
            Error::Format(e) => write!(f, "conteúdo inválido: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Tamanho alvo de cada pedaço de XML descompactado.
    pub chunk_bytes: usize,
    /// Máximo de pedaços em memória ao mesmo tempo (0 = 2 × threads + 2).
    pub max_in_flight: usize,
    /// Só estas abas (índices na ordem do workbook). `None` = todas.
    pub only_sheets: Option<Vec<usize>>,
    /// Inflate paralelo: partes (1 MB comprimido, ~15 MB descompactados)
    /// especuladas à frente. Cada uma também é lida (parse) na própria tarefa,
    /// então isto é quantas tarefas de "descompactar e ler" rodam juntas.
    pub inflate_lookahead: usize,
    /// Inflate paralelo: vetores descompactados em trânsito no parse.
    pub inflate_pieces: usize,
    /// Inflate paralelo: tamanho de cada parte, em bytes comprimidos.
    pub inflate_part_bytes: usize,
    /// Inflate paralelo: cada tarefa lê a própria parte em pedaços deste
    /// tamanho, à medida que descompacta (a parte inteira tem ~15 MB de XML).
    pub inflate_stream_bytes: usize,
}

impl Default for Options {
    fn default() -> Self {
        // Valores medidos no arquivo de referência: mais que isso não ganha
        // tempo (16 threads) e só aumenta o pico de memória.
        Self {
            chunk_bytes: 8 * 1024 * 1024,
            max_in_flight: 0,
            only_sheets: None,
            inflate_lookahead: 16,
            inflate_pieces: 8,
            inflate_part_bytes: 1 << 20,
            inflate_stream_bytes: 1 << 20,
        }
    }
}

/// O que aconteceu durante a leitura.
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub threads: usize,
    pub total: Duration,
    /// Abrir o zip e ler workbook + relacionamentos.
    pub package: Duration,
    pub shared_strings: Duration,
    pub styles: Duration,
    /// Na ordem do workbook.
    pub sheets: Vec<SheetReport>,
}

impl LoadReport {
    pub fn xml_bytes(&self) -> u64 {
        self.sheets.iter().map(|s| s.xml_bytes).sum()
    }
}

pub type SheetResult = Result<(Sheet, SheetReport), Error>;

/// Uma aba listada no workbook.
#[derive(Clone, Debug)]
pub struct SheetInfo {
    pub name: String,
    pub hidden: bool,
    /// Tamanho do XML descompactado.
    pub xml_bytes: u64,
    /// A aba tem gráficos, imagens, tabelas ou tabelas dinâmicas (partes
    /// ligadas a ela que o duck-sheets ainda não ajusta ao inserir linhas).
    pub objects: bool,
}

/// Um pacote aberto, para leitura em etapas (carga progressiva): primeiro o
/// que a primeira tela precisa (estilos, strings, prévia da aba ativa), depois
/// as abas inteiras.
pub struct Book {
    pkg: Package,
    meta: parts::WorkbookMeta,
    /// Caches das segmentações de dados (lidos uma vez, na primeira aba que tiver).
    slicer_caches: std::sync::OnceLock<drawing::SlicerCaches>,
}

impl Book {
    /// Abre o zip e lê `workbook.xml` (rápido: só o diretório central e uma parte pequena).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let pkg = Package::open(path.as_ref())?;
        let meta = parts::read_workbook(&pkg)?;
        Ok(Self { pkg, meta, slicer_caches: Default::default() })
    }

    pub fn sheets(&self) -> Vec<SheetInfo> {
        self.meta
            .sheets
            .iter()
            .map(|m| SheetInfo {
                name: m.name.clone(),
                hidden: m.hidden,
                xml_bytes: self.pkg.entry(&m.path).map(|e| e.size).unwrap_or(0),
                objects: parts::sheet_objects(&self.pkg, &m.path),
            })
            .collect()
    }

    /// Aba aberta quando o arquivo foi salvo (sempre uma aba visível, se houver).
    pub fn active_sheet(&self) -> usize {
        self.meta.active
    }

    pub fn date1904(&self) -> bool {
        self.meta.date1904
    }

    /// Nomes definidos, arquivos externos (com os valores guardados) e a
    /// data em que o arquivo foi salvo: o que o motor de cálculo precisa.
    pub fn read_extras(&self) -> duck_core::BookExtras {
        let mut e = extras::read_extras(&self.pkg, &self.meta);
        e.pivot_caches = pivot::read_caches(&self.pkg, &self.meta.path);
        e
    }

    /// Bytes de uma parte do pacote (imagens, por exemplo).
    pub fn read_part(&self, name: &str) -> Option<Vec<u8>> {
        self.pkg.read(name).ok()
    }

    /// Registros de um cache de tabela dinâmica (lidos só quando precisa).
    pub fn read_pivot_records(
        &self,
        cache: &mut duck_core::pivot::PivotCache,
    ) -> Option<duck_core::pivot::Records> {
        pivot::read_records(&self.pkg, cache)
    }

    /// Estilos e tema.
    pub fn read_styles(&self) -> Result<Styles, Error> {
        let mut st = match &self.meta.styles {
            Some(path) => parts::read_styles(&self.pkg, path)?,
            None => Styles::default(),
        };
        if let Some(path) = &self.meta.theme
            && let Ok(theme) = parts::read_theme(&self.pkg, path)
        {
            st.theme = theme;
        }
        Ok(st)
    }

    pub fn read_shared_strings(&self) -> Result<StringTable, Error> {
        match &self.meta.shared_strings {
            Some(path) => parts::read_shared_strings(&self.pkg, path),
            None => Ok(StringTable::new()),
        }
    }

    /// Prévia de uma aba: só os primeiros `max_xml_bytes` de XML, numa thread.
    /// Traz o cabeçalho da aba (colunas, painéis, visão) e as primeiras linhas;
    /// mesclagens (que ficam no fim do XML) não vêm.
    pub fn preview_sheet(&self, index: usize, max_xml_bytes: usize) -> Result<Sheet, Error> {
        let m = self.meta.sheets.get(index).ok_or_else(|| Error::Missing(format!("aba {index}")))?;
        let entry = self.pkg.entry(&m.path).ok_or_else(|| Error::Missing(m.path.clone()))?;
        let mut buf = Vec::with_capacity(max_xml_bytes.min(entry.size as usize) + 64 * 1024);
        let eof = self.pkg.inflater(entry).read_into(&mut buf, max_xml_bytes)?;
        let cut = if eof { buf.len() } else { duck_par::cut_after_last(&buf, b"</row>").unwrap_or(0) };
        let mut part = duck_core::SheetPart::new();
        let mut state = sheet::RowState::default();
        let mut scratch = sheet::Scratch::default();
        let _ = sheet::parse_rows(&buf[..cut], &mut part, &mut state, &mut scratch);
        let mut s = Sheet::new(m.name.clone());
        s.hidden = m.hidden;
        s.merge_part(part);
        s.finish();
        Ok(s)
    }

    /// Lê as abas `which` em paralelo (maiores primeiro) e entrega cada uma a
    /// `done` assim que fica pronta (de qualquer thread).
    pub fn load_sheets(&self, which: &[usize], opts: &Options, done: &(dyn Fn(usize, SheetResult) + Sync)) {
        let threads = rayon::current_num_threads();
        let limiter =
            InFlight::new(if opts.max_in_flight == 0 { threads * 2 + 2 } else { opts.max_in_flight });
        rayon::scope(|s| self.spawn_sheets(s, which, opts, &limiter, done));
    }

    fn spawn_sheets<'s>(
        &'s self,
        s: &rayon::Scope<'s>,
        which: &[usize],
        opts: &'s Options,
        limiter: &'s InFlight,
        done: &'s (dyn Fn(usize, SheetResult) + Sync),
    ) {
        let mut order: Vec<usize> = which.iter().copied().filter(|&i| i < self.meta.sheets.len()).collect();
        // Maiores primeiro: a descompactação da aba gigante (sequencial) começa logo.
        let size = |i: usize| self.pkg.entry(&self.meta.sheets[i].path).map(|e| e.size).unwrap_or(0);
        order.sort_by_key(|&i| std::cmp::Reverse(size(i)));
        let chunk = opts.chunk_bytes.max(64 * 1024);
        for i in order {
            s.spawn(move |_| {
                let m = &self.meta.sheets[i];
                let r = match self.pkg.entry(&m.path) {
                    Some(e) => {
                        sheet::load_sheet(&self.pkg, e, &m.name, chunk, limiter, opts).map(|(mut sh, rep)| {
                            sh.hidden = m.hidden;
                            parts::resolve_links(&self.pkg, &m.path, &mut sh.layout.links);
                            filter::read_tables(&self.pkg, &m.path, &mut sh.layout.tables);
                            let caches = self
                                .slicer_caches
                                .get_or_init(|| drawing::read_slicer_caches(&self.pkg, &self.meta.path));
                            sh.layout.objects = drawing::read_objects(&self.pkg, &m.path, caches);
                            sh.layout.pivots = pivot::read_pivot_tables(&self.pkg, &m.path);
                            (sh, rep)
                        })
                    }
                    None => Err(Error::Missing(m.path.clone())),
                };
                done(i, r);
            });
        }
    }
}

/// Lê um `.xlsx` / `.xlsm` inteiro: strings, estilos e todas as abas ao mesmo tempo.
pub fn open(path: impl AsRef<Path>, opts: &Options) -> Result<(Workbook, LoadReport), Error> {
    let t0 = Instant::now();
    let book = Book::open(path)?;
    let package_time = t0.elapsed();

    let threads = rayon::current_num_threads();
    let limiter = InFlight::new(if opts.max_in_flight == 0 { threads * 2 + 2 } else { opts.max_in_flight });
    let wanted: Vec<usize> = match &opts.only_sheets {
        Some(v) => v.clone(),
        None => (0..book.meta.sheets.len()).collect(),
    };

    let shared: Mutex<Option<Result<(StringTable, Duration), Error>>> = Mutex::new(None);
    let styles: Mutex<Option<Result<(Styles, Duration), Error>>> = Mutex::new(None);
    let results: Mutex<Vec<(usize, SheetResult)>> = Mutex::new(Vec::new());
    let done = |i: usize, r: SheetResult| results.lock().unwrap().push((i, r));

    rayon::scope(|s| {
        let (book, shared, styles) = (&book, &shared, &styles);
        s.spawn(move |_| {
            let t = Instant::now();
            let r = book.read_shared_strings().map(|st| (st, t.elapsed()));
            *shared.lock().unwrap() = Some(r);
        });
        s.spawn(move |_| {
            let t = Instant::now();
            let r = book.read_styles().map(|st| (st, t.elapsed()));
            *styles.lock().unwrap() = Some(r);
        });
        book.spawn_sheets(s, &wanted, opts, &limiter, &done);
    });

    let mut wb = Workbook { date1904: book.meta.date1904, extras: book.read_extras(), ..Default::default() };
    let mut report = LoadReport { threads, package: package_time, ..Default::default() };
    if let Some(r) = shared.into_inner().unwrap() {
        let (st, t) = r?;
        wb.shared_strings = st;
        report.shared_strings = t;
    }
    if let Some(r) = styles.into_inner().unwrap() {
        let (st, t) = r?;
        wb.styles = st;
        report.styles = t;
    }
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| r.0);
    for (_, r) in results {
        let (sheet, rep) = r?;
        wb.sheets.push(sheet);
        report.sheets.push(rep);
    }
    report.total = t0.elapsed();
    Ok((wb, report))
}

/// Lê o XML de uma aba inteiro, numa thread e num pedaço só. Para medições
/// do parse isolado (`duck-bench parse`) e testes.
#[doc(hidden)]
pub fn parse_sheet_xml(xml: &[u8]) -> Sheet {
    let mut part = duck_core::SheetPart::new();
    let mut state = sheet::RowState::default();
    let mut scratch = sheet::Scratch::default();
    let _ = sheet::parse_rows(xml, &mut part, &mut state, &mut scratch);
    let mut s = Sheet::new("bench");
    s.merge_part(part);
    s.finish();
    s
}

/// Nomes e tamanhos (XML descompactado) das abas, sem ler as células.
pub fn list_sheets(path: impl AsRef<Path>) -> Result<Vec<(String, u64)>, Error> {
    Ok(Book::open(path)?.sheets().into_iter().map(|s| (s.name, s.xml_bytes)).collect())
}
