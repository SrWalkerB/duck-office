//! Leitura de uma aba (`sheetN.bin`), com as células lidas em pedaços e em
//! paralelo, como nas abas em XML.
//!
//! Registros não têm marca de sincronismo: para cortar o fluxo é preciso
//! percorrê-lo do começo. Uma thread faz só isso ([`Feeder`]): lê o cabeçalho
//! de cada registro, corta em começos de linha e guarda as fórmulas
//! compartilhadas, que as linhas seguintes citam pela célula-mestre. O resto
//! (valores, fórmulas) fica para as tarefas.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use duck_core::{CellRange, FormulaKind, RowSpec, Sheet, SheetPart, kind};
use duck_par::InFlight;

use super::frame as frame_xml;
use super::ptg::{self, Names};
use super::rec::{self, Cur, Head, Records, id, push_utf16};
use crate::Error;
use crate::package::{Entry, Package};
use crate::sheet::{Merger, RowState, Scratch, SheetReport, parse_rows};

/// Fórmula compartilhada, já normalizada (vale para todas as células dela).
/// `None` = não deu para ler.
type Shared = Option<Arc<Vec<u8>>>;

/// Fórmulas compartilhadas da aba, pela célula-mestre. O `Feeder` guarda cada
/// uma antes de entregar o pedaço em que ela aparece, então quem lê um pedaço
/// sempre encontra as que ele cita.
#[derive(Default)]
struct Masters(RwLock<HashMap<(u32, u32), Shared>>);

impl Masters {
    /// `BrtShrFmla` logo depois da fórmula da célula `at`.
    fn add(&self, at: (u32, u32), b: &[u8]) {
        let normalized = || {
            let mut c = Cur::new(b);
            c.skip(16)?;
            let (rgce, rgcb) = c.formula()?;
            let mut norm = Vec::with_capacity(rgce.len() + 16);
            ptg::normalize(rgce, rgcb, at, &mut norm)?;
            Some(Arc::new(norm))
        };
        self.0.write().unwrap().insert(at, normalized());
    }

    fn get(&self, at: (u32, u32)) -> Option<Shared> {
        self.0.read().unwrap().get(&at).cloned()
    }
}

/// O que os pedaços de uma aba compartilham.
struct Env<'a> {
    names: &'a Names,
    masters: &'a Masters,
    /// Altura padrão das linhas (1/20 de ponto).
    default_height: u16,
    /// Fórmulas que não deu para ler.
    unread: &'a AtomicU64,
}

/// Fórmula já decodificada no pedaço: o id dela, ou `None` se não deu.
type Known = Option<u32>;

/// Memória de trabalho de um pedaço.
#[derive(Default)]
struct Work {
    norm: Vec<u8>,
    text: String,
    /// Fórmula normalizada → id na parte.
    known: HashMap<Box<[u8]>, Known>,
    /// Última fórmula de cada coluna: as cópias para baixo nem passam pelo mapa.
    last: Vec<(Vec<u8>, Known)>,
    shared: HashMap<(u32, u32), Known>,
}

impl Work {
    /// Id da fórmula que está em `norm`, decodificada na primeira vez.
    fn formula(&mut self, part: &mut SheetPart, row: u32, col: u32, env: &Env) -> Known {
        let c = col as usize;
        if let Some((key, id)) = self.last.get(c)
            && *key == self.norm
        {
            return *id;
        }
        let id = match self.known.get(self.norm.as_slice()) {
            Some(id) => *id,
            None => {
                let id = ptg::decode(&self.norm, env.names).map(|t| part.set_formula_template(row, col, &t));
                self.known.insert(self.norm.as_slice().into(), id);
                id
            }
        };
        if self.last.len() <= c {
            self.last.resize_with(c + 1, Default::default);
        }
        let last = &mut self.last[c];
        last.0.clear();
        last.0.extend_from_slice(&self.norm);
        last.1 = id;
        id
    }

    /// Id da fórmula compartilhada da mestre `at`, se for uma.
    fn shared(&mut self, part: &mut SheetPart, at: (u32, u32), cell: (u32, u32), env: &Env) -> Option<Known> {
        if let Some(id) = self.shared.get(&at) {
            return Some(*id);
        }
        let id = env.masters.get(at)?.and_then(|norm| {
            self.norm.clear();
            self.norm.extend_from_slice(&norm);
            self.formula(part, cell.0, cell.1, env)
        });
        self.shared.insert(at, id);
        Some(id)
    }
}

/// Última célula com fórmula, para o registro que a completa (matriz, tabela
/// de dados).
#[derive(Clone, Copy)]
struct Last {
    row: u32,
    col: u32,
    kind: u8,
    style: u16,
    payload: u64,
    /// Veio com metadados: matriz dinâmica.
    dynamic: bool,
}

/// Lê as linhas de `buf` (registros inteiros, a partir de um começo de linha).
fn parse(buf: &[u8], part: &mut SheetPart, env: &Env) {
    let mut w = Work::default();
    let mut row = None;
    let mut next_col = 0u32;
    let mut meta = false;
    let mut last: Option<Last> = None;
    for (k, b) in Records::new(buf) {
        match k {
            id::ROW => {
                next_col = 0;
                meta = false;
                last = None;
                row = row_header(b, part, env);
            }
            id::CELL_BLANK..=id::FMLA_ERROR | id::CELL_RSTRING | id::SHORT_BLANK..=id::SHORT_ISST => {
                let Some(r) = row else { continue };
                let mut c = Cur::new(b);
                let short = (id::SHORT_BLANK..=id::SHORT_ISST).contains(&k);
                let col = if short { Some(next_col) } else { c.u32() };
                let (Some(col), Some(style)) = (col, c.u32()) else { continue };
                if col > rec::LAST_COL {
                    continue;
                }
                next_col = col + 1;
                let style = (style & 0xFF_FFFF).min(u16::MAX as u32) as u16;
                let k = if short { k - id::SHORT_BLANK + id::CELL_BLANK } else { k };
                let dynamic = std::mem::take(&mut meta);
                last = cell(k, &mut c, (r, col), style, part, &mut w, env).map(|mut l| {
                    l.dynamic = dynamic;
                    l
                });
            }
            id::CELL_META => meta = true,
            id::ARR_FMLA => {
                if let Some(l) = last
                    && array(b, l, part, env).is_none()
                {
                    env.unread.fetch_add(1, Ordering::Relaxed);
                }
            }
            id::TABLE => {
                if let Some(l) = last {
                    part.set(l.row, l.col, l.kind | kind::FORMULA, l.style, l.payload);
                    part.set_formula(l.row, l.col, "", FormulaKind::DataTable);
                }
            }
            _ => {}
        }
    }
}

/// `BrtRowHdr`: número, altura, visibilidade, estilo e nível de tópico.
fn row_header(b: &[u8], part: &mut SheetPart, env: &Env) -> Option<u32> {
    let mut c = Cur::new(b);
    let (row, xf, height) = (c.u32()?, c.u32()?, c.u16()?);
    let (_, bits) = (c.u8()?, c.u8()?);
    if row > rec::LAST_ROW {
        return None;
    }
    let (level, collapsed) = (bits & 7, bits & 8 != 0);
    let (hidden, own_height, own_style) = (bits & 0x10 != 0, bits & 0x20 != 0, bits & 0x40 != 0);
    let height = (own_height || height != env.default_height).then(|| height as f32 / 20.0);
    let style = own_style.then(|| xf.min(u16::MAX as u32) as u16);
    if height.is_some() || hidden || style.is_some() {
        part.layout.rows.push(RowSpec { row, height, hidden, style });
    }
    if level > 0 {
        part.layout.outline.rows.push(row, row, level);
    }
    if collapsed {
        part.layout.outline.closed_rows.insert(row);
    }
    Some(row)
}

/// Uma célula (`k` de [`id::CELL_BLANK`] a [`id::FMLA_ERROR`], ou texto rico).
/// Devolve a célula se ela tem uma fórmula que depende do registro seguinte.
#[inline]
fn cell(
    k: u16,
    c: &mut Cur,
    (row, col): (u32, u32),
    style: u16,
    part: &mut SheetPart,
    w: &mut Work,
    env: &Env,
) -> Option<Last> {
    let mut text = |part: &mut SheetPart, wide: &[u8]| {
        w.text.clear();
        push_utf16(wide, &mut w.text);
        part.strings.push(&w.text) as u64
    };
    let (t, payload) = match k {
        id::CELL_BLANK => {
            if style != 0 {
                part.set(row, col, kind::EMPTY, style, 0);
            }
            return None;
        }
        id::CELL_RK => (kind::NUMBER, rec::rk(c.u32()?).to_bits()),
        id::CELL_REAL | id::FMLA_NUM => (kind::NUMBER, c.f64()?.to_bits()),
        id::CELL_BOOL | id::FMLA_BOOL => (kind::BOOL, (c.u8()? != 0) as u64),
        id::CELL_ERROR | id::FMLA_ERROR => (kind::ERROR, rec::error(c.u8()?) as u64),
        id::CELL_ISST => (kind::SHARED_STR, c.u32()? as u64),
        id::CELL_ST | id::FMLA_STRING => (kind::LOCAL_STR, text(part, c.wide()?)),
        _ => {
            let _ = c.u8()?;
            (kind::LOCAL_STR, text(part, c.wide()?))
        }
    };
    if !(id::FMLA_STRING..=id::FMLA_ERROR).contains(&k) {
        part.set(row, col, t, style, payload);
        return None;
    }
    let formula = c.u16().and_then(|_| c.formula());
    let mut cell = Last { row, col, kind: t, style, payload, dynamic: false };
    let known = match formula {
        None => None,
        // `PtgExp`: a fórmula está na célula-mestre (compartilhada ou matriz).
        Some((&[0x01, a, b, c, d], extra)) => {
            let master = (u32::from_le_bytes([a, b, c, d]), Cur::new(extra).u32().unwrap_or(col));
            match w.shared(part, master, (row, col), env) {
                Some(id) => id,
                None => {
                    // Matriz: só a mestre tem fórmula, que vem no registro seguinte.
                    part.set(row, col, t, style, payload);
                    return (master == (row, col)).then_some(cell);
                }
            }
        }
        // `PtgTbl`: tabela de dados.
        Some((&[0x02, ..], _)) => {
            part.set(row, col, t, style, payload);
            return Some(cell);
        }
        Some((rgce, rgcb)) => {
            w.norm.clear();
            match ptg::normalize(rgce, rgcb, (row, col), &mut w.norm) {
                Some(()) => w.formula(part, row, col, env),
                None => None,
            }
        }
    };
    match known {
        Some(id) => {
            cell.kind |= kind::FORMULA;
            part.reuse_formula(row, col, id);
        }
        // Não deu para ler: a célula fica com o valor gravado.
        None => {
            env.unread.fetch_add(1, Ordering::Relaxed);
        }
    }
    part.set(row, col, cell.kind, style, payload);
    None
}

/// `BrtArrFmla`: fórmula de matriz da célula anterior.
fn array(b: &[u8], l: Last, part: &mut SheetPart, env: &Env) -> Option<()> {
    let mut c = Cur::new(b);
    let range: CellRange = c.range()?;
    c.u8()?;
    let (rgce, rgcb) = c.formula()?;
    let text = ptg::text(rgce, rgcb, (l.row, l.col), env.names)?;
    part.set(l.row, l.col, l.kind | kind::FORMULA, l.style, l.payload);
    part.set_formula(l.row, l.col, &text, FormulaKind::Array { range, dynamic: l.dynamic });
    Some(())
}

// ---------------------------------------------------------------------------
// Fluxo
// ---------------------------------------------------------------------------

enum Stage {
    Head,
    Rows,
    Tail,
}

/// Percorre o fluxo de registros e o separa em começo, pedaços de linhas e fim.
struct Feeder<'a> {
    buf: Vec<u8>,
    /// Até onde os registros já foram percorridos.
    pos: usize,
    /// Começo do pedaço em andamento e último começo de linha visto.
    base: usize,
    cut: usize,
    stage: Stage,
    row: u32,
    /// Coluna da última fórmula.
    col: u32,
    default_height: u16,
    masters: &'a Masters,
    chunk: usize,
}

/// O que o [`Feeder`] achou.
enum Found {
    /// O começo da aba (registros antes das células).
    Head(Vec<u8>),
    /// Linhas inteiras: o vetor e onde elas começam nele.
    Rows(Vec<u8>, usize),
}

impl<'a> Feeder<'a> {
    fn new(masters: &'a Masters, chunk: usize) -> Self {
        Self {
            buf: Vec::new(),
            pos: 0,
            base: 0,
            cut: 0,
            stage: Stage::Head,
            row: 0,
            col: 0,
            default_height: 300,
            masters,
            chunk,
        }
    }

    /// Percorre o que houver de novo em `buf` e entrega o que ficou inteiro.
    fn scan(&mut self, sink: &mut dyn FnMut(Found, u16)) {
        if matches!(self.stage, Stage::Tail) {
            return;
        }
        while let Head::Record(k, s, e) = rec::head(&self.buf, self.pos) {
            let at = self.pos;
            self.pos = e;
            match k {
                id::ROW => {
                    self.cut = at;
                    self.row = Cur::new(&self.buf[s..e]).u32().unwrap_or(self.row);
                }
                id::FMLA_STRING..=id::FMLA_ERROR => {
                    self.col = Cur::new(&self.buf[s..e]).u32().unwrap_or(self.col);
                }
                id::SHR_FMLA => self.masters.add((self.row, self.col), &self.buf[s..e]),
                id::WS_FMT_INFO => {
                    let mut c = Cur::new(&self.buf[s..e]);
                    if let Some(h) = c.skip(6).and_then(|_| c.u16()) {
                        self.default_height = h;
                    }
                }
                id::BEGIN_SHEET_DATA => {
                    sink(Found::Head(self.buf[..at].to_vec()), self.default_height);
                    self.stage = Stage::Rows;
                    (self.base, self.cut) = (e, e);
                }
                id::END_SHEET_DATA => {
                    let tail = self.buf.split_off(e);
                    let mut rows = std::mem::replace(&mut self.buf, tail);
                    rows.truncate(at);
                    if at > self.base {
                        sink(Found::Rows(rows, self.base), self.default_height);
                    }
                    self.stage = Stage::Tail;
                    return;
                }
                _ => {}
            }
        }
        if matches!(self.stage, Stage::Rows) && self.cut - self.base >= self.chunk {
            let rest = self.buf.split_off(self.cut);
            let rows = std::mem::replace(&mut self.buf, rest);
            self.buf.reserve(self.chunk);
            sink(Found::Rows(rows, self.base), self.default_height);
            self.pos -= self.cut;
            (self.base, self.cut) = (0, 0);
        }
    }

    /// Fim do fluxo: os registros depois das células. Erro se ele parou no meio.
    fn finish(self) -> Result<Vec<u8>, Error> {
        match self.stage {
            Stage::Tail => Ok(self.buf),
            _ => Err(Error::Format("aba binária truncada".into())),
        }
    }
}

/// Lê uma aba inteira. Chamado de dentro de um pool rayon.
///
/// Sem o inflate paralelo das abas em XML: medido, ele piora o binário (600 mil
/// linhas × 37 colunas: 1,1–1,3 s contra 0,6 s, com o dobro de memória). As
/// repetições do binário apontam sempre para perto do fim da janela, e quase
/// todo marcador precisa ser resolvido em série.
pub fn load(
    pkg: &Package,
    entry: &Entry,
    name: &str,
    limiter: &InFlight,
    opts: &crate::Options,
) -> Result<(Sheet, SheetReport), Error> {
    let layer = pkg.xlsb().ok_or_else(|| Error::Format("o pacote não é um .xlsb".into()))?;
    let names = layer.names(pkg);
    let t0 = Instant::now();
    let mut report = SheetReport { name: name.to_string(), xml_bytes: entry.size, ..Default::default() };
    // Uma célula ocupa no binário um quarto do que ocupa em XML, e cada pedaço
    // deve dar o mesmo trabalho.
    let chunk = (opts.chunk_bytes / 4).max(64 * 1024);
    let masters = Masters::default();
    let unread = AtomicU64::new(0);
    let merger = Merger::new(name);
    let mut chunks = 0usize;
    let mut inflate = Duration::ZERO;
    let mut result = Ok(Vec::new());
    rayon::scope(|s| {
        let (merger, masters, unread) = (&merger, &masters, &unread);
        let mut feeder = Feeder::new(masters, chunk);
        let mut dispatch = |found: Found, default_height: u16| {
            let my = chunks;
            chunks += 1;
            match found {
                Found::Head(bin) => merger.add(my, layout_part(&frame_xml::head(&bin)), Duration::ZERO),
                Found::Rows(buf, from) => {
                    limiter.acquire();
                    s.spawn(move |_| {
                        let t = Instant::now();
                        let mut part = SheetPart::new();
                        parse(&buf[from..], &mut part, &Env { names, masters, default_height, unread });
                        drop(buf);
                        limiter.release();
                        part.pack_inner();
                        merger.add(my, part, t.elapsed());
                    });
                }
            }
        };
        let mut inf = pkg.inflater(entry);
        loop {
            let t = Instant::now();
            let want = feeder.buf.len() + chunk;
            let eof = inf.read_into(&mut feeder.buf, want);
            inflate += t.elapsed();
            match eof {
                Ok(eof) => {
                    feeder.scan(&mut dispatch);
                    pkg.release(entry, inf.consumed().saturating_sub(64 * 1024));
                    if eof {
                        break;
                    }
                }
                Err(e) => {
                    result = Err(e);
                    return;
                }
            }
        }
        result = feeder.finish();
    });
    let tail = result?;
    pkg.release(entry, usize::MAX);
    report.inflate = inflate;

    let t = Instant::now();
    let mut n = unread.into_inner();
    merger.add(chunks, layout_part(&frame_xml::tail(&tail, names, &mut n)), Duration::ZERO);
    layer.count_unread(n);
    let (mut sheet, parse_cpu, merge, complete) = merger.finish();
    if !complete {
        return Err(Error::Format("aba binária incompleta".into()));
    }
    sheet.finish_with(|cols| {
        use rayon::prelude::*;
        cols.par_iter_mut().for_each(duck_core::Column::pack);
    });
    report.chunks = chunks;
    report.parse_cpu = parse_cpu;
    report.merge = merge + t.elapsed();
    report.wall = t0.elapsed();
    Ok((sheet, report))
}

/// O layout do começo ou do fim da aba, pelo leitor do XML.
fn layout_part(xml: &str) -> SheetPart {
    let mut part = SheetPart::new();
    let _ = parse_rows(xml.as_bytes(), &mut part, &mut RowState::default(), &mut Scratch::default());
    part
}

/// Começo e fim da aba em XML, sem as células.
pub fn frame(pkg: &Package, real: &str, names: &Names, unread: &mut u64) -> Result<(String, String), Error> {
    let entry = pkg.entry(real).ok_or_else(|| Error::Missing(real.to_string()))?;
    let masters = Masters::default();
    let mut feeder = Feeder::new(&masters, 1 << 20);
    let mut head = Vec::new();
    let mut inf = pkg.inflater(entry);
    loop {
        let want = feeder.buf.len() + (1 << 20);
        let eof = inf.read_into(&mut feeder.buf, want)?;
        feeder.scan(&mut |f, _| {
            if let Found::Head(h) = f {
                head = h;
            }
        });
        if eof {
            break;
        }
    }
    let tail = feeder.finish()?;
    Ok((frame_xml::head(&head), frame_xml::tail(&tail, names, unread)))
}

/// Prévia: o começo da aba e as primeiras linhas, numa thread.
pub fn preview(pkg: &Package, entry: &Entry, max_bytes: usize, sheet: &mut Sheet) -> Result<(), Error> {
    let layer = pkg.xlsb().ok_or_else(|| Error::Format("o pacote não é um .xlsb".into()))?;
    let names = layer.names(pkg);
    let masters = Masters::default();
    let unread = AtomicU64::new(0);
    let mut feeder = Feeder::new(&masters, usize::MAX);
    let eof = pkg.inflater(entry).read_into(&mut feeder.buf, max_bytes)?;
    let mut parts = Vec::new();
    let rows = |buf: &[u8], default_height: u16| {
        let mut part = SheetPart::new();
        parse(buf, &mut part, &Env { names, masters: &masters, default_height, unread: &unread });
        part
    };
    feeder.scan(&mut |f, height| match f {
        Found::Head(h) => parts.push(layout_part(&frame_xml::head(&h))),
        Found::Rows(buf, from) => parts.push(rows(&buf[from..], height)),
    });
    if !eof && matches!(feeder.stage, Stage::Rows) && feeder.cut > feeder.base {
        parts.push(rows(&feeder.buf[feeder.base..feeder.cut], feeder.default_height));
    }
    for p in parts {
        sheet.merge_part(p);
    }
    Ok(())
}
