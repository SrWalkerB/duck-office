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

mod package;
mod parts;
#[doc(hidden)]
pub mod sheet;
pub mod xml;

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use duck_core::{Sheet, StringTable, Styles, Workbook};
use duck_par::InFlight;

pub use package::Package;
pub use sheet::SheetReport;

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
}

impl Default for Options {
    fn default() -> Self {
        // Valores medidos no arquivo de referência: mais que isso quase não
        // ganha tempo e passa de 1 GB de pico.
        Self {
            chunk_bytes: 8 * 1024 * 1024,
            max_in_flight: 0,
            only_sheets: None,
            inflate_lookahead: 16,
            inflate_pieces: 8,
            inflate_part_bytes: 1 << 20,
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

type SheetResult = Result<(Sheet, SheetReport), Error>;

/// Lê um `.xlsx` / `.xlsm`.
pub fn open(path: impl AsRef<Path>, opts: &Options) -> Result<(Workbook, LoadReport), Error> {
    let t0 = Instant::now();
    let pkg = Package::open(path.as_ref())?;
    let meta = parts::read_workbook(&pkg)?;
    let package_time = t0.elapsed();

    let threads = rayon::current_num_threads();
    let limiter = InFlight::new(if opts.max_in_flight == 0 { threads * 2 + 2 } else { opts.max_in_flight });

    let wanted: Vec<usize> = match &opts.only_sheets {
        Some(v) => v.iter().copied().filter(|&i| i < meta.sheets.len()).collect(),
        None => (0..meta.sheets.len()).collect(),
    };
    // Maiores primeiro: a descompactação da aba gigante (sequencial) começa logo.
    let mut order = wanted.clone();
    let size = |i: usize| pkg.entry(&meta.sheets[i].path).map(|e| e.size).unwrap_or(0);
    order.sort_by_key(|&i| std::cmp::Reverse(size(i)));

    let shared: Mutex<Option<Result<(StringTable, Duration), Error>>> = Mutex::new(None);
    let styles: Mutex<Option<Result<(Styles, Duration), Error>>> = Mutex::new(None);
    let results: Mutex<Vec<(usize, SheetResult)>> = Mutex::new(Vec::new());

    rayon::scope(|s| {
        if let Some(path) = &meta.shared_strings {
            let (pkg, shared) = (&pkg, &shared);
            s.spawn(move |_| {
                let t = Instant::now();
                let r = parts::read_shared_strings(pkg, path).map(|st| (st, t.elapsed()));
                *shared.lock().unwrap() = Some(r);
            });
        }
        if let Some(path) = &meta.styles {
            let (pkg, styles) = (&pkg, &styles);
            s.spawn(move |_| {
                let t = Instant::now();
                let r = parts::read_styles(pkg, path).map(|st| (st, t.elapsed()));
                *styles.lock().unwrap() = Some(r);
            });
        }
        for &i in &order {
            let (pkg, meta, results, limiter) = (&pkg, &meta, &results, &limiter);
            let chunk = opts.chunk_bytes.max(64 * 1024);
            s.spawn(move |_| {
                let m = &meta.sheets[i];
                let r = match pkg.entry(&m.path) {
                    Some(e) => {
                        sheet::load_sheet(pkg, e, &m.name, chunk, limiter, opts).map(|(mut sh, rep)| {
                            sh.hidden = m.hidden;
                            (sh, rep)
                        })
                    }
                    None => Err(Error::Missing(m.path.clone())),
                };
                results.lock().unwrap().push((i, r));
            });
        }
    });

    let mut wb = Workbook { date1904: meta.date1904, ..Default::default() };
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
    let pkg = Package::open(path.as_ref())?;
    let meta = parts::read_workbook(&pkg)?;
    Ok(meta
        .sheets
        .iter()
        .map(|m| (m.name.clone(), pkg.entry(&m.path).map(|e| e.size).unwrap_or(0)))
        .collect())
}
