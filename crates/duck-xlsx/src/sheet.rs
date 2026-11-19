//! Leitura de uma aba (`xl/worksheets/sheetN.xml`) com parse paralelo em pedaços.
//!
//! ```text
//!                             ┌─▶ parse (thread 1) ─▶ SheetPart 0 ─┐
//! inflate ──pedaços de 8 MB──┼─▶ parse (thread 2) ─▶ SheetPart 1 ─┼─▶ merge em ordem ─▶ Sheet
//! (cortes após </row>)       └─▶ parse (thread N) ─▶ SheetPart k ─┘
//! ```
//!
//! Cada pedaço começa numa fronteira de linha, e as linhas trazem o próprio
//! número (`<row r="12345">`), então cada pedaço é montado sem depender dos
//! outros. Se uma aba não tiver os números (raro), ela é relida em sequência.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use duck_core::{CellError, CellRange, ColSpec, FormulaKind, Pane, RowSpec, Sheet, SheetPart, kind};
use duck_par::{InFlight, cut_after_last};
use memchr::{memchr, memmem};

use crate::Error;
use crate::package::{Entry, Package};
use crate::xml::{attr, is_name_end, parse_u32, rich_text_into, tag_end, unescape_into};

/// Estatísticas da leitura de uma aba.
#[derive(Clone, Debug, Default)]
pub struct SheetReport {
    pub name: String,
    pub xml_bytes: u64,
    pub chunks: usize,
    /// Tempo de parede desde o início da aba até ela estar montada.
    pub wall: Duration,
    /// Tempo de parede da descompactação (em paralelo com o parse).
    pub inflate: Duration,
    /// Soma do tempo de parse de todos os pedaços (várias threads).
    pub parse_cpu: Duration,
    pub merge: Duration,
    /// A aba precisou ser lida em sequência (linhas sem número).
    pub sequential: bool,
    /// A descompactação usou várias threads (`duck-deflate`).
    pub parallel_inflate: bool,
}

/// O pedaço não pode ser lido isoladamente (linha/célula sem número).
#[derive(Debug)]
pub(crate) struct NeedsSequential;

/// Atributos crus, conteúdo cru e id de uma fórmula que não depende da posição.
type CachedFormula = (Vec<u8>, Vec<u8>, u32);

/// Memória de trabalho de um leitor de pedaço.
#[derive(Default)]
pub(crate) struct Scratch {
    text: String,
    /// Por coluna: atributos e texto crus da última fórmula que não depende da
    /// posição, e o id dela. Colunas calculadas de tabela e fórmulas só com
    /// referências absolutas repetem o mesmo texto em todas as linhas; com
    /// isso, cada uma é decodificada e deduplicada uma vez por coluna.
    last_formula: Vec<Option<CachedFormula>>,
}

/// Estado de leitura que atravessa pedaços no modo sequencial.
#[derive(Clone, Copy, Default)]
pub(crate) struct RowState {
    /// Linha atual (0-based); `None` se ainda desconhecida.
    row: Option<u32>,
    next_col: u32,
}

#[inline]
fn starts_tag(rest: &[u8], name: &[u8]) -> bool {
    rest.len() > name.len() && &rest[..name.len()] == name && is_name_end(rest[name.len()])
}

#[inline]
fn attr_f32(attrs: &[u8], key: &[u8]) -> Option<f32> {
    attr(attrs, key).and_then(|v| fast_float2::parse::<f32, _>(v.trim_ascii()).ok())
}

#[inline]
fn attr_true(attrs: &[u8], key: &[u8]) -> bool {
    matches!(attr(attrs, key), Some(b"1") | Some(b"true"))
}

/// "A1:B2" ou "A1" → intervalo (base 0).
pub(crate) fn parse_range(v: &[u8]) -> Option<CellRange> {
    let (a, b) = match memchr(b':', v) {
        Some(i) => (&v[..i], &v[i + 1..]),
        None => (v, v),
    };
    let (c0, r0) = parse_ref(a)?;
    let (c1, r1) = parse_ref(b)?;
    Some(CellRange { first_row: r0?, first_col: c0, last_row: r1?, last_col: c1 })
}

/// Lê as linhas de `buf` para `part`, além do layout que aparecer no pedaço
/// (`<col>`, `<mergeCell>`, `<pane>`, `<sheetFormatPr>`, `<dimension>`).
pub(crate) fn parse_rows(
    buf: &[u8],
    part: &mut SheetPart,
    state: &mut RowState,
    scratch: &mut Scratch,
) -> Result<(), NeedsSequential> {
    let close_c = memmem::Finder::new(b"</c>");
    let mut pos = 0;
    while let Some(off) = memchr(b'<', &buf[pos..]) {
        let i = pos + off;
        let rest = &buf[i + 1..];
        // <c ...>: o caso mais comum, primeiro.
        if rest.len() > 1 && rest[0] == b'c' && is_name_end(rest[1]) {
            let Some(end) = tag_end(buf, i) else { break };
            let self_closing = buf[end - 2] == b'/';
            let attrs = &buf[i + 2..if self_closing { end - 2 } else { end - 1 }];
            let (mut r_attr, mut s_attr, mut t_attr) = (None, None, None);
            scan_cell_attrs(attrs, &mut r_attr, &mut s_attr, &mut t_attr);

            let (col, row) = match r_attr.and_then(parse_ref) {
                Some((c, Some(r))) => (c, r),
                Some((c, None)) => (c, state.row.ok_or(NeedsSequential)?),
                None => (state.next_col, state.row.ok_or(NeedsSequential)?),
            };
            state.row = Some(row);
            state.next_col = col + 1;
            let style = s_attr.and_then(parse_u32).unwrap_or(0).min(u16::MAX as u32) as u16;

            if self_closing {
                if style != 0 {
                    part.set(row, col, kind::EMPTY, style, 0);
                }
                pos = end;
                continue;
            }
            let close = match close_c.find(&buf[end..]) {
                Some(k) => end + k,
                None => buf.len(),
            };
            let content = &buf[end..close];
            store_cell(part, row, col, style, t_attr, content, scratch);
            pos = (close + 4).min(buf.len());
            continue;
        }
        // <row ...>
        if starts_tag(rest, b"row") {
            let Some(end) = tag_end(buf, i) else { break };
            let attrs = &buf[i + 4..end - 1];
            state.row = match attr(attrs, b"r").and_then(parse_u32) {
                Some(r) if r > 0 => Some(r - 1),
                _ => state.row.map(|r| r + 1),
            };
            state.next_col = 0;
            // Altura própria (ou calculada pelo Excel, ex.: texto quebrado) e linhas ocultas.
            let height = attr_f32(attrs, b"ht");
            let hidden = attr_true(attrs, b"hidden");
            if let Some(row) = state.row
                && (height.is_some() || hidden)
            {
                part.layout.rows.push(RowSpec { row, height, hidden });
            }
            pos = end;
            continue;
        }
        if rest.first() == Some(&b'/') {
            pos = i + 1;
            continue;
        }
        // Layout (cabeçalho e rodapé da aba).
        if starts_tag(rest, b"col") {
            let Some(end) = tag_end(buf, i) else { break };
            let attrs = &buf[i + 4..end - 1];
            if let (Some(min), Some(max)) =
                (attr(attrs, b"min").and_then(parse_u32), attr(attrs, b"max").and_then(parse_u32))
                && min >= 1
            {
                part.layout.cols.push(ColSpec {
                    first: min - 1,
                    last: max.max(min) - 1,
                    width: attr_f32(attrs, b"width"),
                    hidden: attr_true(attrs, b"hidden"),
                    style: attr(attrs, b"style").and_then(parse_u32).unwrap_or(0).min(u16::MAX as u32) as u16,
                });
            }
            pos = end;
            continue;
        }
        if starts_tag(rest, b"mergeCell") {
            let Some(end) = tag_end(buf, i) else { break };
            if let Some(r) = attr(&buf[i + 10..end - 1], b"ref").and_then(parse_range) {
                part.layout.merges.push(r);
            }
            pos = end;
            continue;
        }
        if starts_tag(rest, b"pane") {
            let Some(end) = tag_end(buf, i) else { break };
            let attrs = &buf[i + 5..end - 1];
            if matches!(attr(attrs, b"state"), Some(b"frozen") | Some(b"frozenSplit")) {
                let n = |k: &[u8]| attr_f32(attrs, k).map(|v| v.max(0.0) as u32).unwrap_or(0);
                part.layout.pane = Some(Pane { frozen_rows: n(b"ySplit"), frozen_cols: n(b"xSplit") });
            }
            pos = end;
            continue;
        }
        if starts_tag(rest, b"sheetFormatPr") {
            let Some(end) = tag_end(buf, i) else { break };
            let attrs = &buf[i + 14..end - 1];
            part.layout.default_row_height = attr_f32(attrs, b"defaultRowHeight");
            part.layout.default_col_width =
                attr_f32(attrs, b"defaultColWidth").or_else(|| attr_f32(attrs, b"baseColWidth"));
            pos = end;
            continue;
        }
        if starts_tag(rest, b"dimension") {
            let Some(end) = tag_end(buf, i) else { break };
            part.layout.dimension = attr(&buf[i + 10..end - 1], b"ref").and_then(parse_range);
            pos = end;
            continue;
        }
        pos = i + 1;
    }
    Ok(())
}

/// Lê `r`, `s` e `t` numa passada.
#[inline]
fn scan_cell_attrs<'a>(
    attrs: &'a [u8],
    r: &mut Option<&'a [u8]>,
    s: &mut Option<&'a [u8]>,
    t: &mut Option<&'a [u8]>,
) {
    let mut i = 0;
    let n = attrs.len();
    while i < n {
        while i < n && attrs[i].is_ascii_whitespace() {
            i += 1;
        }
        let k0 = i;
        while i < n && attrs[i] != b'=' {
            i += 1;
        }
        let key = &attrs[k0..i];
        i += 1;
        while i < n && attrs[i] != b'"' && attrs[i] != b'\'' {
            i += 1;
        }
        if i >= n {
            return;
        }
        let q = attrs[i];
        let v0 = i + 1;
        let Some(len) = memchr(q, &attrs[v0..]) else { return };
        let v = &attrs[v0..v0 + len];
        match key.trim_ascii() {
            b"r" => *r = Some(v),
            b"s" => *s = Some(v),
            b"t" => *t = Some(v),
            _ => {}
        }
        i = v0 + len + 1;
    }
}

/// "AB12" → (coluna 27, Some(linha 11)); "AB" → (27, None). Aceita `$`. Base 0.
#[inline]
pub(crate) fn parse_ref(r: &[u8]) -> Option<(u32, Option<u32>)> {
    let mut i = 0;
    if r.first() == Some(&b'$') {
        i += 1;
    }
    let mut col: u32 = 0;
    while i < r.len() && r[i].is_ascii_alphabetic() {
        col = col.checked_mul(26)? + (r[i].to_ascii_uppercase() - b'A') as u32 + 1;
        i += 1;
    }
    if col == 0 {
        return None;
    }
    if r.get(i) == Some(&b'$') {
        i += 1;
    }
    let row = if i < r.len() { Some(parse_u32(&r[i..])?.checked_sub(1)?) } else { None };
    Some((col - 1, row))
}

/// Um filho `<name ...>...</name>` dentro do conteúdo de uma célula.
struct Child<'a> {
    attrs: &'a [u8],
    content: &'a [u8],
}

#[inline]
fn find_child<'a>(content: &'a [u8], name: &[u8]) -> Option<Child<'a>> {
    let mut pos = 0;
    while let Some(off) = memchr(b'<', &content[pos..]) {
        let i = pos + off;
        let rest = &content[i + 1..];
        if starts_tag(rest, name) {
            let end = tag_end(content, i)?;
            if content[end - 2] == b'/' {
                return Some(Child { attrs: &content[i + 1 + name.len()..end - 2], content: &[] });
            }
            let attrs = &content[i + 1 + name.len()..end - 1];
            let mut close = Vec::with_capacity(name.len() + 3);
            close.extend_from_slice(b"</");
            close.extend_from_slice(name);
            close.push(b'>');
            let k = memmem::find(&content[end..], &close).unwrap_or(content.len() - end);
            return Some(Child { attrs, content: &content[end..end + k] });
        }
        pos = i + 1;
    }
    None
}

/// Guarda a fórmula `<f>` da célula, se houver. Retorna o bit de fórmula.
#[inline]
fn store_formula(part: &mut SheetPart, row: u32, col: u32, content: &[u8], sc: &mut Scratch) -> u8 {
    let Some(f) = find_child(content, b"f") else { return 0 };
    if let Some(Some((attrs, raw, id))) = sc.last_formula.get(col as usize)
        && raw.as_slice() == f.content
        && attrs.as_slice() == f.attrs
    {
        part.reuse_formula(row, col, *id);
        return kind::FORMULA;
    }
    let scratch = &mut sc.text;
    let t = attr(f.attrs, b"t");
    if t == Some(b"shared")
        && let Some(si) = attr(f.attrs, b"si").and_then(parse_u32)
    {
        if f.content.is_empty() {
            part.set_shared_formula(row, col, si);
        } else {
            scratch.clear();
            unescape_into(f.content, scratch);
            part.set_formula(row, col, scratch, FormulaKind::Shared { si });
        }
        return kind::FORMULA;
    }
    scratch.clear();
    unescape_into(f.content, scratch);
    let k = match t {
        Some(b"array") => match attr(f.attrs, b"ref").and_then(parse_range) {
            Some(r) => FormulaKind::Array(r),
            None => FormulaKind::Normal,
        },
        Some(b"dataTable") => FormulaKind::DataTable,
        _ => {
            // Fórmula normal: guarda como modelo relativo deduplicado (fórmulas
            // copiadas para baixo viram uma entrada só).
            if let Some(t) = duck_calc::to_template(scratch, row, col) {
                let id = part.set_formula_template(row, col, &t);
                if duck_calc::is_position_independent(&t) {
                    // Vale para qualquer linha desta coluna.
                    let c = col as usize;
                    if sc.last_formula.len() <= c {
                        sc.last_formula.resize_with(c + 1, || None);
                    }
                    sc.last_formula[c] = Some((f.attrs.to_vec(), f.content.to_vec(), id));
                }
                return kind::FORMULA;
            }
            FormulaKind::Normal
        }
    };
    part.set_formula(row, col, scratch, k);
    kind::FORMULA
}

#[inline]
fn store_cell(
    part: &mut SheetPart,
    row: u32,
    col: u32,
    style: u16,
    t: Option<&[u8]>,
    content: &[u8],
    sc: &mut Scratch,
) {
    let formula = store_formula(part, row, col, content, sc);
    let scratch = &mut sc.text;
    let t = t.unwrap_or(b"n");
    if t == b"inlineStr" {
        scratch.clear();
        if let Some(is) = find_child(content, b"is") {
            rich_text_into(is.content, scratch);
        }
        let id = part.strings.push(scratch);
        part.set(row, col, kind::LOCAL_STR | formula, style, id as u64);
        return;
    }
    let v = match find_child(content, b"v") {
        Some(v) => v.content,
        None => {
            if style != 0 || formula != 0 {
                part.set(row, col, kind::EMPTY | formula, style, 0);
            }
            return;
        }
    };
    match t {
        b"s" => match parse_u32(v.trim_ascii()) {
            Some(i) => part.set(row, col, kind::SHARED_STR | formula, style, i as u64),
            None => part.set(row, col, kind::EMPTY | formula, style, 0),
        },
        b"b" => part.set(row, col, kind::BOOL | formula, style, (v.trim_ascii() == b"1") as u64),
        b"e" => part.set(row, col, kind::ERROR | formula, style, CellError::parse(v.trim_ascii()) as u64),
        b"str" | b"d" => {
            scratch.clear();
            unescape_into(v, scratch);
            let id = part.strings.push(scratch);
            part.set(row, col, kind::LOCAL_STR | formula, style, id as u64);
        }
        _ => match fast_float2::parse::<f64, _>(v.trim_ascii()) {
            Ok(x) => part.set(row, col, kind::NUMBER | formula, style, x.to_bits()),
            Err(_) => {
                // Número malformado: guarda o texto para não perder o conteúdo.
                scratch.clear();
                unescape_into(v, scratch);
                let id = part.strings.push(scratch);
                part.set(row, col, kind::LOCAL_STR | formula, style, id as u64);
            }
        },
    }
}

/// Um pedaço de XML para uma tarefa de parse: próprio, ou um intervalo de um
/// vetor compartilhado (saída do inflate paralelo, sem cópia).
enum Chunk {
    Owned(Vec<u8>),
    Shared(Arc<Piece>, std::ops::Range<usize>),
}

impl Chunk {
    fn bytes(&self) -> &[u8] {
        match self {
            Chunk::Owned(v) => v,
            Chunk::Shared(v, r) => &v.buf[r.clone()],
        }
    }
}

/// Junta as partes à aba na ordem dos pedaços, à medida que ficam prontas.
struct Merger {
    sheet: Sheet,
    next: usize,
    pending: std::collections::BTreeMap<usize, SheetPart>,
    parse_cpu: Duration,
    merge: Duration,
}

impl Merger {
    fn new(name: &str) -> Self {
        Self {
            sheet: Sheet::new(name),
            next: 0,
            pending: Default::default(),
            parse_cpu: Duration::ZERO,
            merge: Duration::ZERO,
        }
    }

    fn add(&mut self, idx: usize, part: SheetPart, cpu: Duration) {
        self.parse_cpu += cpu;
        self.pending.insert(idx, part);
        let t = Instant::now();
        while let Some(p) = self.pending.remove(&self.next) {
            self.sheet.merge_part(p);
            self.next += 1;
        }
        self.merge += t.elapsed();
    }
}

/// Um vetor descompactado compartilhado pelas tarefas de parse. Ocupa uma vaga
/// do limitador de vetores até a última tarefa que o usa terminar: é o que
/// limita a memória em trânsito (um pedaço pequeno segura o vetor inteiro).
struct Piece {
    buf: duck_deflate::Buffer,
    slots: Arc<InFlight>,
}

impl Drop for Piece {
    fn drop(&mut self) {
        self.slots.release();
    }
}

/// Partes de XML a partir das quais vale usar o inflate paralelo.
const PARALLEL_INFLATE_MIN: usize = 8 * 1024 * 1024;

/// Lê uma aba inteira. Chamado de dentro de um pool rayon.
pub(crate) fn load_sheet(
    pkg: &Package,
    entry: &Entry,
    name: &str,
    chunk_bytes: usize,
    limiter: &InFlight,
    opts: &crate::Options,
) -> Result<(Sheet, SheetReport), Error> {
    let parallel = entry.is_deflated()
        && entry.compressed_size() >= PARALLEL_INFLATE_MIN
        && rayon::current_num_threads() >= 4;
    if parallel {
        match load_sheet_with(pkg, entry, name, chunk_bytes, limiter, Some(opts)) {
            Ok(r) => return Ok(r),
            // Inflate paralelo não conferiu (CRC, deflate inesperado): relê pelo
            // caminho sequencial do zlib-rs, que é a referência.
            Err(Error::Format(_)) => {}
            Err(e) => return Err(e),
        }
    }
    load_sheet_with(pkg, entry, name, chunk_bytes, limiter, None)
}

fn load_sheet_with(
    pkg: &Package,
    entry: &Entry,
    name: &str,
    chunk_bytes: usize,
    limiter: &InFlight,
    // `Some` = inflate paralelo com essas opções.
    parallel: Option<&crate::Options>,
) -> Result<(Sheet, SheetReport), Error> {
    let parallel_inflate = parallel.is_some();
    let t0 = Instant::now();
    let mut report = SheetReport { name: name.to_string(), xml_bytes: entry.size, ..Default::default() };
    report.parallel_inflate = parallel_inflate;

    // Merge em fluxo: cada parte entra na aba assim que todas as anteriores
    // entraram (ordem fixa = resultado determinístico) e sua memória é liberada.
    let merger = Mutex::new(Merger::new(name));
    let needs_seq = std::sync::atomic::AtomicBool::new(false);
    let mut inflate_time = Duration::ZERO;
    let mut io_error = None;

    rayon::scope(|s| {
        let mut idx = 0usize;
        // Entrega um pedaço de XML (cortado depois de um `</row>`) para uma thread.
        let mut dispatch = |chunk: Chunk| {
            limiter.acquire();
            let (merger, needs_seq) = (&merger, &needs_seq);
            let my = idx;
            s.spawn(move |_| {
                let tp = Instant::now();
                let mut part = SheetPart::new();
                let mut state = RowState::default();
                let mut scratch = Scratch::default();
                if parse_rows(chunk.bytes(), &mut part, &mut state, &mut scratch).is_err() {
                    needs_seq.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                drop(chunk);
                limiter.release();
                let cpu = tp.elapsed();
                merger.lock().unwrap().add(my, part, cpu);
            });
            idx += 1;
        };

        if let Some(o) = parallel {
            // A saída chega em vetores grandes, em ordem e por posse. Cada um vira
            // um `Arc` e as tarefas leem intervalos dele, sem copiar; só a linha
            // que atravessa dois vetores (`carry`) é copiada.
            let mut carry: Vec<u8> = Vec::new();
            let piece_slots = Arc::new(InFlight::new(o.inflate_pieces.max(1)));
            let ti = Instant::now();
            let r = duck_deflate::inflate_parallel(
                pkg.compressed(entry),
                Some(entry.crc32),
                duck_deflate::Options { part_bytes: 1 << 20, lookahead: o.inflate_lookahead.max(1) },
                |buf| {
                    // A entrada comprimida já consumida não é mais lida (com margem).
                    pkg.release(entry, buf.input_consumed().saturating_sub(64 * 1024));
                    let bytes = buf.bytes();
                    let (Some(first), Some(last)) =
                        (memmem::find(bytes, b"</row>"), memmem::rfind(bytes, b"</row>"))
                    else {
                        carry.extend_from_slice(bytes);
                        return Ok(());
                    };
                    let (first, last) = (first + 6, last + 6);
                    // Emenda: o fim do pedaço anterior + o começo deste, até o primeiro `</row>`.
                    carry.extend_from_slice(&bytes[..first]);
                    let joined = std::mem::take(&mut carry);
                    carry.extend_from_slice(&bytes[last..]);
                    piece_slots.acquire();
                    let shared = Arc::new(Piece { buf, slots: piece_slots.clone() });
                    dispatch(Chunk::Owned(joined));
                    let mut pos = first;
                    while pos < last {
                        let target = pos + chunk_bytes;
                        let cut = if target >= last {
                            last
                        } else {
                            match memmem::rfind(&shared.buf[pos..target], b"</row>") {
                                Some(i) => pos + i + 6,
                                None => memmem::find(&shared.buf[target..last], b"</row>")
                                    .map_or(last, |i| target + i + 6),
                            }
                        };
                        dispatch(Chunk::Shared(shared.clone(), pos..cut));
                        pos = cut;
                    }
                    Ok(())
                },
            );
            inflate_time = ti.elapsed();
            match r {
                Ok(_) => {
                    if !carry.is_empty() {
                        dispatch(Chunk::Owned(carry));
                    }
                }
                Err(e) => io_error = Some(Error::Format(format!("inflate paralelo: {e}"))),
            }
        } else {
            let mut inf = pkg.inflater(entry);
            let mut buf: Vec<u8> = Vec::with_capacity(chunk_bytes + 64 * 1024);
            loop {
                let ti = Instant::now();
                let want = buf.len() + chunk_bytes;
                let eof = match inf.read_into(&mut buf, want) {
                    Ok(e) => e,
                    Err(e) => {
                        io_error = Some(e);
                        break;
                    }
                };
                inflate_time += ti.elapsed();
                let cut = if eof {
                    buf.len()
                } else {
                    match cut_after_last(&buf, b"</row>") {
                        Some(c) => c,
                        // Linha maior que o pedaço: lê mais antes de cortar.
                        None => continue,
                    }
                };
                let tail = buf.split_off(cut);
                let chunk = std::mem::replace(&mut buf, tail);
                buf.reserve(chunk_bytes);
                dispatch(Chunk::Owned(chunk));
                if eof {
                    break;
                }
            }
        }
        report.chunks = idx;
    });
    if let Some(e) = io_error {
        return Err(e);
    }
    pkg.release(entry, usize::MAX);
    report.inflate = inflate_time;

    let tm = Instant::now();
    let m = merger.into_inner().unwrap();
    report.parse_cpu = m.parse_cpu;
    let mut sheet = m.sheet;
    if needs_seq.into_inner() || !m.pending.is_empty() {
        report.sequential = true;
        sheet = Sheet::new(name);
        sequential(pkg, entry, chunk_bytes, &mut sheet)?;
    }
    sheet.finish();
    report.merge = m.merge + tm.elapsed();
    report.wall = t0.elapsed();
    Ok((sheet, report))
}

/// Leitura em sequência, carregando o número da linha entre pedaços.
fn sequential(pkg: &Package, entry: &Entry, chunk_bytes: usize, sheet: &mut Sheet) -> Result<(), Error> {
    let mut inf = pkg.inflater(entry);
    let mut buf = Vec::with_capacity(chunk_bytes);
    let mut state = RowState::default();
    let mut scratch = Scratch::default();
    loop {
        let want = buf.len() + chunk_bytes;
        let eof = inf.read_into(&mut buf, want)?;
        let cut = if eof {
            buf.len()
        } else {
            match cut_after_last(&buf, b"</row>") {
                Some(c) => c,
                None => continue,
            }
        };
        let tail = buf.split_off(cut);
        let mut part = SheetPart::new();
        // Linhas/células sem número continuam a contagem do pedaço anterior.
        let _ = parse_rows(&buf, &mut part, &mut state, &mut scratch);
        sheet.merge_part(part);
        buf = tail;
        if eof {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{StringTable, Value};

    fn parse_all(xml: &[u8]) -> Sheet {
        let mut part = SheetPart::new();
        let mut st = RowState::default();
        let mut s = Scratch::default();
        parse_rows(xml, &mut part, &mut st, &mut s).unwrap();
        let mut sheet = Sheet::new("t");
        sheet.merge_part(part);
        sheet
    }

    #[test]
    fn refs() {
        assert_eq!(parse_ref(b"A1"), Some((0, Some(0))));
        assert_eq!(parse_ref(b"AB12"), Some((27, Some(11))));
        assert_eq!(parse_ref(b"XFD1048576"), Some((16383, Some(1048575))));
        assert_eq!(parse_ref(b"C"), Some((2, None)));
        assert_eq!(parse_ref(b"12"), None);
    }

    #[test]
    fn cell_types() {
        let xml = br#"<sheetData>
<row r="1"><c r="A1" t="s"><v>1</v></c><c r="B1"><v>3.25</v></c><c r="C1" t="b"><v>1</v></c>
<c r="D1" t="e"><v>#N/A</v></c><c r="E1" t="str"><f>A1&amp;"x"</f><v>a &amp; b</v></c>
<c r="F1" t="inlineStr"><is><t>inline</t></is></c><c r="G1" s="7"/><c r="H1" s="2"><f>1/0</f><v>1E-3</v></c></row>
<row r="5"><c><v>10</v></c><c><v>11</v></c></row>
</sheetData><cols><col min="1" max="2"/></cols><conditionalFormatting><cfRule><formula>1</formula></cfRule></conditionalFormatting>"#;
        let sheet = parse_all(xml);
        let mut shared = StringTable::new();
        shared.push("zero");
        shared.push("um");
        let c = |r, c| sheet.cell(r, c, &shared);
        assert_eq!(c(0, 0).value, Value::Text("um"));
        assert_eq!(c(0, 1).value, Value::Number(3.25));
        assert_eq!(c(0, 2).value, Value::Bool(true));
        assert_eq!(c(0, 3).value, Value::Error(CellError::NA));
        assert_eq!(c(0, 4).value, Value::Text("a & b"));
        assert!(c(0, 4).formula);
        assert_eq!(c(0, 5).value, Value::Text("inline"));
        assert_eq!(c(0, 6).value, Value::Empty);
        assert_eq!(c(0, 6).style, 7);
        assert_eq!(c(0, 7).value, Value::Number(0.001));
        assert!(c(0, 7).formula);
        // Células sem `r` seguem a ordem da linha.
        assert_eq!(c(4, 0).value, Value::Number(10.0));
        assert_eq!(c(4, 1).value, Value::Number(11.0));
        assert_eq!((sheet.rows(), sheet.cols()), (5, 8));
    }

    #[test]
    fn layout_and_formulas() {
        let xml = br#"<worksheet><dimension ref="A1:C4"/><sheetViews><sheetView><pane xSplit="1" ySplit="2" topLeftCell="B3" state="frozen"/></sheetView></sheetViews>
<sheetFormatPr defaultRowHeight="15"/><cols><col min="1" max="2" width="12.5" customWidth="1"/><col min="3" max="3" width="9" hidden="1"/></cols>
<sheetData>
<row r="1" ht="30" customHeight="1"><c r="A1"><f t="shared" ref="A1:A3" si="0">B1*2</f><v>2</v></c><c r="B1"><v>1</v></c></row>
<row r="2" hidden="1"><c r="A2"><f t="shared" si="0"/><v>4</v></c><c r="B2"><f>SUM($B$1:B1)</f><v>1</v></c></row>
<row r="3"><c r="A3"><f t="shared" si="0"/><v>6</v></c><c r="B3"><f>SUM($B$1:B2)</f><v>2</v></c><c r="C3"><f t="array" ref="C3:C4">B1:B2*2</f><v>2</v></c></row>
</sheetData><mergeCells count="1"><mergeCell ref="B4:C4"/></mergeCells></worksheet>"#;
        let mut sheet = parse_all(xml);
        sheet.finish();
        let l = &sheet.layout;
        assert_eq!(l.pane, Some(Pane { frozen_rows: 2, frozen_cols: 1 }));
        assert_eq!(l.default_row_height, Some(15.0));
        assert_eq!(l.col(1).and_then(|c| c.width), Some(12.5));
        assert!(l.col(2).unwrap().hidden);
        assert_eq!(l.row(0).and_then(|r| r.height), Some(30.0));
        assert!(l.row(1).unwrap().hidden);
        assert_eq!(l.merges, vec![CellRange { first_row: 3, first_col: 1, last_row: 3, last_col: 2 }]);
        assert_eq!(l.dimension, Some(CellRange { first_row: 0, first_col: 0, last_row: 3, last_col: 2 }));

        let f = |r, c| duck_calc::cell_formula(&sheet, r, c).map(|s| s.into_owned());
        assert_eq!(f(0, 0).as_deref(), Some("B1*2"));
        assert_eq!(f(1, 0).as_deref(), Some("B2*2"));
        assert_eq!(f(2, 0).as_deref(), Some("B3*2"));
        assert_eq!(f(1, 1).as_deref(), Some("SUM($B$1:B1)"));
        assert_eq!(f(2, 1).as_deref(), Some("SUM($B$1:B2)"));
        assert_eq!(f(2, 2).as_deref(), Some("B1:B2*2"));
        assert_eq!(f(0, 1), None);
        // As duas fórmulas "SUM($B$1:Bn)" viram um modelo só.
        assert_eq!(sheet.formula_texts_stored(), 3);
    }

    #[test]
    fn chunk_without_row_numbers_asks_for_sequential() {
        let mut part = SheetPart::new();
        let mut st = RowState::default();
        let mut s = Scratch::default();
        let r = parse_rows(b"<row><c><v>1</v></c></row>", &mut part, &mut st, &mut s);
        assert!(r.is_err());
    }
}
