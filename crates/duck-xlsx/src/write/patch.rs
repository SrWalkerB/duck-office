//! Remendo de uma aba: o XML original passa por aqui em ordem e sai com só
//! o que mudou reescrito (ADR-006).
//!
//! - Linhas sem edição são copiadas byte a byte (em blocos, sem olhar as
//!   células, quando a próxima linha editada está além do pedaço).
//! - Numa linha editada, o `<row>` e os `<c>` não editados também são
//!   copiados; as células editadas são escritas a partir do modelo, na
//!   posição certa (as que não existiam são inseridas em ordem).
//! - Linhas editadas que não existiam entram entre as vizinhas.
//! - Antes de `<sheetData>`, só o `<dimension>` é ajustado.
//!
//! Com `rewrite_all`, toda célula é escrita a partir do modelo (para testar
//! o gravador no arquivo inteiro), mantendo os atributos que o modelo não
//! guarda (`cm`, `vm`, `ph`). Fórmulas de tabela de dados (atributos que o
//! modelo também não guarda) ficam como estavam.

use memchr::memmem;

use duck_core::{FormulaKind, Sheet, StringTable};

use super::cellxml::{cell_ref, write_cell};
use crate::sheet::{parse_ref, row_end};
use crate::xml::{attr, is_name_end, parse_u32, tag_end};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Head,
    Rows,
    Tail,
}

/// O elemento `<f …>…</f>` (ou `<f …/>`) de um `<c>` original.
fn formula_element(c: &[u8]) -> Option<&[u8]> {
    let mut from = 0;
    while let Some(k) = memmem::find(&c[from..], b"<f") {
        let i = from + k;
        if c.get(i + 2).is_some_and(|&b| is_name_end(b)) {
            let tend = tag_end(c, i)?;
            if c[tend - 2] == b'/' {
                return Some(&c[i..tend]);
            }
            let close = memmem::find(&c[tend..], b"</f>")?;
            return Some(&c[i..tend + close + 4]);
        }
        from = i + 2;
    }
    None
}

pub struct Patcher<'a> {
    sheet: &'a Sheet,
    shared: &'a StringTable,
    rewrite_all: bool,
    /// Linhas editadas, em ordem.
    dirty: Vec<Dirty>,
    next: usize,
    phase: Phase,
    last_row: Option<u32>,
    carry: Vec<u8>,
    /// Células escritas a partir do modelo (estatística).
    pub cells_written: u64,
}

impl<'a> Patcher<'a> {
    pub fn new(sheet: &'a Sheet, shared: &'a StringTable, rewrite_all: bool) -> Self {
        let e = sheet.edits();
        let mut rows: std::collections::BTreeMap<u32, Dirty> = std::collections::BTreeMap::new();
        for (&row, cols) in &e.cells {
            rows.insert(row, Dirty { row, cols: cols.iter().copied().collect(), height: false, full: false });
        }
        for &row in &e.rows {
            rows.entry(row).or_insert(Dirty { row, cols: Vec::new(), height: false, full: false }).height =
                true;
        }
        for &(a, b) in &e.rewrite {
            for row in a..=b {
                rows.entry(row).or_insert(Dirty { row, cols: Vec::new(), height: false, full: false }).full =
                    true;
            }
        }
        let dirty = rows.into_values().collect();
        Self {
            sheet,
            shared,
            rewrite_all,
            dirty,
            next: 0,
            phase: Phase::Head,
            last_row: None,
            carry: Vec::new(),
            cells_written: 0,
        }
    }

    /// Mais bytes do XML original (cortados em qualquer ponto).
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        let mut bytes = bytes;
        if !self.carry.is_empty() {
            let Some(f) = row_end(&self.carry, bytes) else {
                self.carry.extend_from_slice(bytes);
                return;
            };
            self.carry.extend_from_slice(&bytes[..f]);
            let seg = std::mem::take(&mut self.carry);
            self.segment(&seg, out, false);
            bytes = &bytes[f..];
        }
        match memmem::rfind(bytes, b"</row>") {
            Some(l) => {
                self.segment(&bytes[..l + 6], out, false);
                self.carry.extend_from_slice(&bytes[l + 6..]);
            }
            None => self.carry.extend_from_slice(bytes),
        }
    }

    /// Fim do XML original.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        let seg = std::mem::take(&mut self.carry);
        self.segment(&seg, out, true);
    }

    /// Um trecho com linhas inteiras (e o começo/fim do documento).
    fn segment(&mut self, seg: &[u8], out: &mut Vec<u8>, eof: bool) {
        let mut pos = 0;
        if self.phase == Phase::Head {
            let Some(tag) = crate::xml::find_tag(seg, 0, b"sheetData") else {
                // Sem `<sheetData>` (não deveria acontecer): copia como está.
                out.extend_from_slice(seg);
                return;
            };
            self.head(&seg[..tag.start], out);
            if tag.self_closing {
                out.extend_from_slice(b"<sheetData>");
                self.new_rows_before(u32::MAX, out);
                out.extend_from_slice(b"</sheetData>");
                self.phase = Phase::Tail;
            } else {
                out.extend_from_slice(&seg[tag.start..tag.end]);
                self.phase = Phase::Rows;
            }
            pos = tag.end;
        }
        while self.phase == Phase::Rows {
            let Some(k) = memchr::memchr(b'<', &seg[pos..]) else {
                out.extend_from_slice(&seg[pos..]);
                return;
            };
            let i = pos + k;
            out.extend_from_slice(&seg[pos..i]);
            let rest = &seg[i + 1..];
            if rest.starts_with(b"/sheetData>") {
                self.new_rows_before(u32::MAX, out);
                out.extend_from_slice(b"</sheetData>");
                pos = i + 12;
                self.phase = Phase::Tail;
                break;
            }
            if !(rest.starts_with(b"row") && rest.len() > 3 && is_name_end(rest[3])) {
                // Algo inesperado entre linhas: copia até o próximo `<`.
                out.push(b'<');
                pos = i + 1;
                continue;
            }
            // Bloco de linhas sem edição: copia tudo até a última linha do
            // trecho que vem antes da próxima editada.
            if !self.rewrite_all
                && let Some((end, last)) = self.bulk_end(seg, i)
            {
                out.extend_from_slice(&seg[i..end]);
                self.last_row = Some(last);
                pos = end;
                continue;
            }
            let Some(tend) = tag_end(seg, i) else {
                out.extend_from_slice(&seg[i..]);
                return;
            };
            let self_closing = seg[tend - 2] == b'/';
            let attrs = &seg[i + 4..if self_closing { tend - 2 } else { tend - 1 }];
            let r = match attr(attrs, b"r").and_then(parse_u32) {
                Some(r) if r > 0 => r - 1,
                _ => self.last_row.map_or(0, |r| r + 1),
            };
            let end = if self_closing {
                tend
            } else {
                memmem::find(&seg[tend..], b"</row>").map_or(seg.len(), |x| tend + x + 6)
            };
            self.new_rows_before(r, out);
            let dirty = if self.dirty.get(self.next).is_some_and(|d| d.row == r) {
                self.next += 1;
                Some(std::mem::take(&mut self.dirty[self.next - 1]))
            } else {
                None
            };
            if dirty.is_none() && !self.rewrite_all {
                out.extend_from_slice(&seg[i..end]);
            } else {
                let content_end = if self_closing { tend } else { end - 6 };
                let d = dirty.unwrap_or_default();
                if d.full {
                    self.row_full(r, attrs, d.height, out);
                } else {
                    self.row(r, attrs, &seg[tend..content_end], &d.cols, d.height, out);
                }
            }
            self.last_row = Some(r);
            pos = end;
        }
        // Depois de `</sheetData>`: copia o resto (com mesclagens e filtro novos).
        let e = self.sheet.edits();
        if self.phase == Phase::Tail && (e.merges || e.filter) {
            let mut tail = seg[pos..].to_vec();
            if e.merges {
                tail = replace_merges(&tail, &merges_xml(self.sheet));
            }
            if e.filter {
                tail = replace_autofilter(&tail, &autofilter_xml(self.sheet.layout.autofilter.as_ref()));
            }
            out.extend_from_slice(&tail);
        } else {
            out.extend_from_slice(&seg[pos..]);
        }
        let _ = eof;
    }

    /// Fim do bloco de linhas sem edição a partir da linha em `i`: a última
    /// linha do trecho, se ela (e portanto todas antes dela, que estão em
    /// ordem) vier antes da próxima linha editada.
    fn bulk_end(&self, seg: &[u8], i: usize) -> Option<(usize, u32)> {
        let close = memmem::rfind(&seg[i..], b"</row>")? + i + 6;
        let start = memmem::rfind(&seg[i..close], b"<row")? + i;
        let tend = tag_end(seg, start)?;
        let r = attr(&seg[start + 4..tend - 1], b"r").and_then(parse_u32)?.checked_sub(1)?;
        let next = self.dirty.get(self.next).map_or(u32::MAX, |d| d.row);
        (r < next && self.last_row.is_none_or(|l| l < r)).then_some((close, r))
    }

    /// Escreve as linhas editadas que não existiam no original, antes de `r`.
    fn new_rows_before(&mut self, r: u32, out: &mut Vec<u8>) {
        while let Some(d) = self.dirty.get(self.next)
            && d.row < r
        {
            let d = std::mem::take(&mut self.dirty[self.next]);
            self.next += 1;
            let start = out.len();
            out.extend_from_slice(b"<row r=\"");
            super::cellxml::push_u32(out, d.row + 1);
            out.push(b'"');
            if d.height {
                self.push_height(d.row, out);
            }
            out.push(b'>');
            let before = out.len();
            if d.full {
                for c in 0..self.sheet.cols() {
                    self.cell(d.row, c, b"", out);
                }
            }
            for c in d.cols {
                self.cell(d.row, c, b"", out);
            }
            let height = d.height && self.sheet.layout.row(d.row).is_some();
            if out.len() == before && !height {
                out.truncate(start);
            } else {
                out.extend_from_slice(b"</row>");
            }
        }
    }

    fn cell(&mut self, row: u32, col: u32, extra: &[u8], out: &mut Vec<u8>) {
        self.cells_written += 1;
        write_cell(out, self.sheet, self.shared, row, col, extra);
    }

    /// Célula editada que existia no XML original (`orig` = o `<c>…</c>`).
    fn cell_from(&mut self, row: u32, col: u32, extra: &[u8], orig: &[u8], out: &mut Vec<u8>) {
        self.cells_written += 1;
        super::cellxml::write_cell_with(out, self.sheet, self.shared, row, col, extra, formula_element(orig));
    }

    /// Uma linha que existia, escrita inteira a partir do modelo (o `<row>`
    /// original fica, sem `spans`).
    fn row_full(&mut self, r: u32, attrs: &[u8], height: bool, out: &mut Vec<u8>) {
        out.extend_from_slice(b"<row");
        let mut skip: Vec<&[u8]> = vec![b"spans"];
        if height {
            skip.extend([&b"ht"[..], b"customHeight", b"hidden"]);
        }
        push_attrs_except(attrs, &skip, out);
        if height {
            self.push_height(r, out);
        }
        out.push(b'>');
        for c in 0..self.sheet.cols() {
            self.cell(r, c, b"", out);
        }
        out.extend_from_slice(b"</row>");
    }

    /// Uma linha que existia, com células editadas (`cols`) ou reescrita inteira.
    fn row(&mut self, r: u32, attrs: &[u8], content: &[u8], cols: &[u32], height: bool, out: &mut Vec<u8>) {
        out.extend_from_slice(b"<row");
        // `spans` é só uma dica de quais colunas a linha usa; com células
        // novas pode ficar errado, então sai.
        let mut skip: Vec<&[u8]> = Vec::new();
        if !cols.is_empty() {
            skip.push(b"spans");
        }
        if height {
            skip.extend([&b"ht"[..], b"customHeight", b"hidden"]);
        }
        if skip.is_empty() {
            out.extend_from_slice(attrs);
        } else {
            push_attrs_except(attrs, &skip, out);
        }
        if height {
            self.push_height(r, out);
        }
        out.push(b'>');
        // Colunas editadas já escritas: um `<c>` original fora de ordem
        // (o Excel grava em ordem, outros geradores nem sempre) com uma
        // delas é descartado, não duplicado.
        let mut written = vec![false; cols.len()];
        let mut next_pending = 0;
        let mut pos = 0;
        let mut next_col = 0u32;
        while let Some(k) = memchr::memchr(b'<', &content[pos..]) {
            let i = pos + k;
            let rest = &content[i + 1..];
            if !(rest.first() == Some(&b'c') && rest.len() > 1 && is_name_end(rest[1])) {
                // Fim das células (`extLst`): o resto vai como está.
                break;
            }
            let Some(tend) = tag_end(content, i) else { break };
            let self_closing = content[tend - 2] == b'/';
            let cattrs = &content[i + 2..if self_closing { tend - 2 } else { tend - 1 }];
            let end = if self_closing {
                tend
            } else {
                memmem::find(&content[tend..], b"</c>").map_or(content.len(), |x| tend + x + 4)
            };
            let col = attr(cattrs, b"r").and_then(parse_ref).map_or(next_col, |(c, _)| c);
            while next_pending < cols.len() && cols[next_pending] < col {
                if !written[next_pending] {
                    written[next_pending] = true;
                    self.cell(r, cols[next_pending], b"", out);
                }
                next_pending += 1;
            }
            if let Ok(k) = cols.binary_search(&col) {
                if !written[k] {
                    written[k] = true;
                    self.cell_from(r, col, b"", &content[i..end], out);
                }
            } else if self.rewrite_all && !self.is_data_table(r, col) {
                let mut extra = Vec::new();
                push_attrs_except(cattrs, &[b"r", b"s", b"t"], &mut extra);
                self.cell(r, col, &extra, out);
            } else {
                out.extend_from_slice(&content[i..end]);
            }
            next_col = col + 1;
            pos = end;
        }
        for (k, &c) in cols.iter().enumerate() {
            if !written[k] {
                self.cell(r, c, b"", out);
            }
        }
        out.extend_from_slice(&content[pos..]);
        out.extend_from_slice(b"</row>");
    }

    /// ` ht="…" customHeight="1"` e ` hidden="1"` com a linha no modelo.
    fn push_height(&self, r: u32, out: &mut Vec<u8>) {
        if let Some(h) = self.sheet.row_height(r) {
            out.extend_from_slice(b" ht=\"");
            super::cellxml::push_number(out, h as f64);
            out.extend_from_slice(b"\" customHeight=\"1\"");
        }
        if self.sheet.row_hidden(r) {
            out.extend_from_slice(b" hidden=\"1\"");
        }
    }

    fn is_data_table(&self, r: u32, c: u32) -> bool {
        self.sheet.formula(r, c).is_some_and(|f| f.kind == FormulaKind::DataTable)
    }

    /// O que vem antes de `<sheetData>`, com o `<dimension>` e as larguras
    /// de coluna ajustados.
    fn head(&self, head: &[u8], out: &mut Vec<u8>) {
        if self.sheet.edits().pane {
            let mut tmp = Vec::with_capacity(head.len() + 256);
            self.head_cols(head, &mut tmp);
            out.extend_from_slice(&patch_pane(&tmp, self.sheet.layout.pane));
        } else {
            self.head_cols(head, out);
        }
    }

    fn head_cols(&self, head: &[u8], out: &mut Vec<u8>) {
        if self.sheet.edits().cols.is_empty() {
            return self.head_dimension(head, out);
        }
        let mut tmp = Vec::with_capacity(head.len() + 256);
        match crate::xml::find_tag(head, 0, b"cols") {
            Some(tag) => {
                let inner = crate::xml::inner(head, &tag, b"cols");
                let end = if tag.self_closing { tag.end } else { tag.end + inner.len() + 7 };
                tmp.extend_from_slice(&head[..tag.start]);
                self.cols(inner, &mut tmp);
                tmp.extend_from_slice(&head[end..]);
            }
            None => {
                // `<cols>` fica logo antes de `<sheetData>` (fim do cabeçalho).
                tmp.extend_from_slice(head);
                self.cols(b"", &mut tmp);
            }
        }
        self.head_dimension(&tmp, out);
    }

    /// `<cols>` com as colunas de largura mudada: os `<col>` originais que as
    /// cobrem são divididos (mantendo os outros atributos), e as que não
    /// tinham `<col>` ganham um.
    fn cols(&self, inner: &[u8], out: &mut Vec<u8>) {
        let changed = &self.sheet.edits().cols;
        // (primeira, última, atributos a manter, mudada)
        let mut items: Vec<(u32, u32, Vec<u8>, bool)> = Vec::new();
        let mut covered = std::collections::BTreeSet::new();
        crate::xml::for_each(inner, b"col", |a, _| {
            let (Some(min), Some(max)) =
                (attr(a, b"min").and_then(parse_u32), attr(a, b"max").and_then(parse_u32))
            else {
                return;
            };
            let (first, last) = (min.max(1) - 1, max.max(min) - 1);
            let mut keep = Vec::new();
            push_attrs_except(a, &[b"min", b"max"], &mut keep);
            let mut rest = Vec::new();
            push_attrs_except(a, &[b"min", b"max", b"width", b"customWidth", b"hidden"], &mut rest);
            let mut start = first;
            for &c in changed.range(first..=last) {
                if c > start {
                    items.push((start, c - 1, keep.clone(), false));
                }
                items.push((c, c, rest.clone(), true));
                covered.insert(c);
                start = c + 1;
            }
            if start <= last {
                items.push((start, last, keep, false));
            }
        });
        for &c in changed {
            if !covered.contains(&c) {
                items.push((c, c, Vec::new(), true));
            }
        }
        items.sort_by_key(|i| i.0);
        out.extend_from_slice(b"<cols>");
        for (first, last, attrs, edited) in items {
            out.extend_from_slice(b"<col min=\"");
            super::cellxml::push_u32(out, first + 1);
            out.extend_from_slice(b"\" max=\"");
            super::cellxml::push_u32(out, last + 1);
            out.push(b'"');
            if edited && self.sheet.col_hidden(first) {
                out.extend_from_slice(b" hidden=\"1\"");
            }
            if edited && let Some(w) = self.sheet.col_width(first) {
                out.extend_from_slice(b" width=\"");
                super::cellxml::push_number(out, w as f64);
                out.extend_from_slice(b"\" customWidth=\"1\"");
            }
            out.extend_from_slice(&attrs);
            out.extend_from_slice(b"/>");
        }
        out.extend_from_slice(b"</cols>");
    }

    fn head_dimension(&self, head: &[u8], out: &mut Vec<u8>) {
        let edited = !self.sheet.edits().cells.is_empty();
        let Some(tag) = crate::xml::find_tag(head, 0, b"dimension").filter(|_| edited) else {
            out.extend_from_slice(head);
            return;
        };
        let old = attr(tag.attrs, b"ref").and_then(crate::sheet::parse_range);
        let (r0, c0) = old.map_or((0, 0), |d| (d.first_row, d.first_col));
        let (mut r1, mut c1) = old.map_or((0, 0), |d| (d.last_row, d.last_col));
        if self.sheet.rows() > 0 {
            r1 = r1.max(self.sheet.rows() - 1);
            c1 = c1.max(self.sheet.cols().saturating_sub(1));
        }
        out.extend_from_slice(&head[..tag.start]);
        out.extend_from_slice(b"<dimension ref=\"");
        cell_ref(r0, c0, out);
        if (r0, c0) != (r1, c1) {
            out.push(b':');
            cell_ref(r1, c1, out);
        }
        out.extend_from_slice(b"\"/>");
        out.extend_from_slice(&head[tag.end..]);
    }
}

/// `<mergeCells>` com as mesclagens do modelo (vazio se não houver).
pub fn merges_xml(sheet: &Sheet) -> Vec<u8> {
    let m = &sheet.layout.merges;
    let mut out = Vec::new();
    if m.is_empty() {
        return out;
    }
    out.extend_from_slice(b"<mergeCells count=\"");
    super::cellxml::push_u32(&mut out, m.len() as u32);
    out.extend_from_slice(b"\">");
    for r in m {
        out.extend_from_slice(b"<mergeCell ref=\"");
        cell_ref(r.first_row, r.first_col, &mut out);
        if (r.first_row, r.first_col) != (r.last_row, r.last_col) {
            out.push(b':');
            cell_ref(r.last_row, r.last_col, &mut out);
        }
        out.extend_from_slice(b"\"/>");
    }
    out.extend_from_slice(b"</mergeCells>");
    out
}

/// Elementos que vêm depois de `<mergeCells>` na aba (ordem do esquema).
const AFTER_MERGES: [&[u8]; 23] = [
    b"phoneticPr",
    b"conditionalFormatting",
    b"dataValidations",
    b"hyperlinks",
    b"printOptions",
    b"pageMargins",
    b"pageSetup",
    b"headerFooter",
    b"rowBreaks",
    b"colBreaks",
    b"customProperties",
    b"cellWatches",
    b"ignoredErrors",
    b"smartTags",
    b"drawing",
    b"legacyDrawing",
    b"legacyDrawingHF",
    b"picture",
    b"oleObjects",
    b"controls",
    b"webPublishItems",
    b"tableParts",
    b"extLst",
];

/// O fim da aba (depois de `</sheetData>`) com `<mergeCells>` trocado por
/// `merges` (que pode ser vazio), no lugar certo do esquema.
pub fn replace_merges(tail: &[u8], merges: &[u8]) -> Vec<u8> {
    replace_element(tail, b"mergeCells", merges, &AFTER_MERGES)
}

/// Como [`replace_merges`], para `<autoFilter>` (que vem antes de `<mergeCells>`).
pub fn replace_autofilter(tail: &[u8], xml: &[u8]) -> Vec<u8> {
    let mut after: Vec<&[u8]> = vec![b"sortState", b"dataConsolidate", b"customSheetViews", b"mergeCells"];
    after.extend(AFTER_MERGES);
    replace_element(tail, b"autoFilter", xml, &after)
}

/// Tira o elemento `name` (o primeiro, no nível de cima) e põe `new` antes
/// do primeiro dos elementos de `after` que existir (ou antes do fechamento).
fn replace_element(tail: &[u8], name: &[u8], new: &[u8], after: &[&[u8]]) -> Vec<u8> {
    let mut t = tail.to_vec();
    if let Some(tag) = crate::xml::find_tag(&t, 0, name) {
        let end = if tag.self_closing {
            tag.end
        } else {
            let mut close = b"</".to_vec();
            close.extend_from_slice(name);
            close.push(b'>');
            memmem::find(&t[tag.end..], &close).map_or(t.len(), |x| tag.end + x + close.len())
        };
        t.drain(tag.start..end);
    }
    if new.is_empty() {
        return t;
    }
    let at = after
        .iter()
        .filter_map(|n| crate::xml::find_tag(&t, 0, n).map(|x| x.start))
        .min()
        .or_else(|| memmem::rfind(&t, b"</"))
        .unwrap_or(t.len());
    t.splice(at..at, new.iter().copied());
    t
}

/// `<autoFilter>` do modelo (vazio se não houver).
pub fn autofilter_xml(af: Option<&duck_core::AutoFilter>) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(af) = af else { return out };
    let r = af.range;
    out.extend_from_slice(b"<autoFilter ref=\"");
    cell_ref(r.first_row, r.first_col, &mut out);
    out.push(b':');
    cell_ref(r.last_row, r.last_col, &mut out);
    out.push(b'"');
    if af.columns.is_empty() {
        out.extend_from_slice(b"/>");
        return out;
    }
    out.push(b'>');
    for c in &af.columns {
        match &c.kind {
            duck_core::FilterKind::Values { values, blank } => {
                out.extend_from_slice(b"<filterColumn colId=\"");
                super::cellxml::push_u32(&mut out, c.col);
                out.extend_from_slice(b"\"><filters");
                if *blank {
                    out.extend_from_slice(b" blank=\"1\"");
                }
                if values.is_empty() {
                    out.extend_from_slice(b"/>");
                } else {
                    out.push(b'>');
                    for v in values {
                        out.extend_from_slice(b"<filter val=\"");
                        super::cellxml::escape_attr(v, &mut out);
                        out.extend_from_slice(b"\"/>");
                    }
                    out.extend_from_slice(b"</filters>");
                }
                out.extend_from_slice(b"</filterColumn>");
            }
            duck_core::FilterKind::Other(raw) => out.extend_from_slice(raw.as_bytes()),
        }
    }
    out.extend_from_slice(b"</autoFilter>");
    out
}

/// A parte de uma tabela com o intervalo, o filtro e as colunas do modelo.
pub fn patch_table(xml: &[u8], t: &duck_core::Table) -> Vec<u8> {
    let Some(tag) = crate::xml::find_tag(xml, 0, b"table") else { return xml.to_vec() };
    let mut out = Vec::with_capacity(xml.len() + 256);
    out.extend_from_slice(&xml[..tag.start]);
    let mut range = Vec::new();
    cell_ref(t.range.first_row, t.range.first_col, &mut range);
    range.push(b':');
    cell_ref(t.range.last_row, t.range.last_col, &mut range);
    out.extend_from_slice(b"<table");
    crate::xml::for_each_attr(tag.attrs, |k, whole| {
        out.push(b' ');
        if k == b"ref" {
            out.extend_from_slice(b"ref=\"");
            out.extend_from_slice(&range);
            out.push(b'"');
        } else {
            out.extend_from_slice(whole);
        }
    });
    out.push(b'>');
    // Corpo: filtro novo primeiro, colunas do modelo, o resto como está.
    let body = &xml[tag.end..];
    let mut rest = body.to_vec();
    rest = replace_element(&rest, b"autoFilter", b"", &[]);
    if let Some(c) = crate::xml::find_tag(&rest, 0, b"tableColumns") {
        let end = if c.self_closing {
            c.end
        } else {
            memmem::find(&rest[c.end..], b"</tableColumns>").map_or(rest.len(), |x| c.end + x + 15)
        };
        let mut cols = b"<tableColumns count=\"".to_vec();
        super::cellxml::push_u32(&mut cols, t.column_xml.len() as u32);
        cols.extend_from_slice(b"\">");
        for x in &t.column_xml {
            cols.extend_from_slice(x.as_bytes());
        }
        cols.extend_from_slice(b"</tableColumns>");
        rest.splice(c.start..end, cols);
    }
    out.extend_from_slice(&autofilter_xml(t.filter.as_ref()));
    out.extend_from_slice(&rest);
    out
}

/// O começo da aba com o `<pane>` do primeiro `<sheetView>` trocado pelos
/// painéis congelados do modelo (as seleções por painel saem: o Excel
/// recria).
pub fn patch_pane(head: &[u8], pane: Option<duck_core::Pane>) -> Vec<u8> {
    let mut xml = Vec::new();
    if let Some(p) = pane.filter(|p| p.frozen_rows > 0 || p.frozen_cols > 0) {
        let active: &[u8] = match (p.frozen_rows > 0, p.frozen_cols > 0) {
            (true, true) => b"bottomRight",
            (true, false) => b"bottomLeft",
            _ => b"topRight",
        };
        xml.extend_from_slice(b"<pane");
        if p.frozen_cols > 0 {
            xml.extend_from_slice(b" xSplit=\"");
            super::cellxml::push_u32(&mut xml, p.frozen_cols);
            xml.push(b'"');
        }
        if p.frozen_rows > 0 {
            xml.extend_from_slice(b" ySplit=\"");
            super::cellxml::push_u32(&mut xml, p.frozen_rows);
            xml.push(b'"');
        }
        xml.extend_from_slice(b" topLeftCell=\"");
        cell_ref(p.frozen_rows, p.frozen_cols, &mut xml);
        xml.extend_from_slice(b"\" activePane=\"");
        xml.extend_from_slice(active);
        xml.extend_from_slice(b"\" state=\"frozen\"/>");
    }
    let Some(view) = crate::xml::find_tag(head, 0, b"sheetView") else {
        // Sem `<sheetViews>`: entra antes de `<sheetFormatPr>`/`<cols>`, ou no fim.
        if xml.is_empty() {
            return head.to_vec();
        }
        let at = [&b"sheetFormatPr"[..], b"cols"]
            .iter()
            .filter_map(|n| crate::xml::find_tag(head, 0, n).map(|t| t.start))
            .min()
            .unwrap_or(head.len());
        let mut out = head[..at].to_vec();
        out.extend_from_slice(b"<sheetViews><sheetView workbookViewId=\"0\">");
        out.extend_from_slice(&xml);
        out.extend_from_slice(b"</sheetView></sheetViews>");
        out.extend_from_slice(&head[at..]);
        return out;
    };
    let mut out = head[..view.start].to_vec();
    if view.self_closing {
        out.extend_from_slice(&head[view.start..view.end - 2]);
        if xml.is_empty() {
            out.extend_from_slice(b"/>");
        } else {
            out.push(b'>');
            out.extend_from_slice(&xml);
            out.extend_from_slice(b"</sheetView>");
        }
        out.extend_from_slice(&head[view.end..]);
        return out;
    }
    let close = memmem::find(&head[view.end..], b"</sheetView>").map_or(head.len(), |x| view.end + x);
    out.extend_from_slice(&head[view.start..view.end]);
    out.extend_from_slice(&xml);
    // Filhos sem `<pane>` e `<selection>`.
    let inner = &head[view.end..close];
    let mut pos = 0;
    while let Some(k) = memchr::memchr(b'<', &inner[pos..]) {
        let i = pos + k;
        out.extend_from_slice(&inner[pos..i]);
        let end = tag_end(inner, i).unwrap_or(inner.len());
        let name = &inner[i + 1..];
        let drop = (name.starts_with(b"pane") && is_name_end(name[4]))
            || (name.starts_with(b"selection") && is_name_end(name[9]));
        if !drop {
            out.extend_from_slice(&inner[i..end]);
        } else if inner[end - 2] != b'/' {
            // `<selection>…</selection>` (não acontece no Excel): pula até o fim.
            let close_name: &[u8] = if name.starts_with(b"pane") { b"</pane>" } else { b"</selection>" };
            pos = memmem::find(&inner[end..], close_name).map_or(inner.len(), |x| end + x + close_name.len());
            continue;
        }
        pos = end;
    }
    out.extend_from_slice(&inner[pos..]);
    out.extend_from_slice(&head[close..]);
    out
}

/// Uma linha com algo a regravar.
#[derive(Default)]
struct Dirty {
    row: u32,
    /// Colunas editadas, em ordem.
    cols: Vec<u32>,
    /// A altura mudou.
    height: bool,
    /// A linha inteira é escrita a partir do modelo (classificar).
    full: bool,
}

/// Copia os atributos (` k="v"`), menos os de `skip`.
fn push_attrs_except(attrs: &[u8], skip: &[&[u8]], out: &mut Vec<u8>) {
    crate::xml::for_each_attr(attrs, |k, whole| {
        if !skip.contains(&k) {
            out.push(b' ');
            out.extend_from_slice(whole);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::CellValue;

    const XML: &str = concat!(
        r#"<?xml version="1.0"?><worksheet><dimension ref="A1:C3"/><sheetData>"#,
        r#"<row r="1" spans="1:3"><c r="A1" t="s"><v>0</v></c><c r="C1" vm="1"><v>5</v></c></row>"#,
        r#"<row r="3"><c r="B3"><f>A1+1</f><v>2</v></c></row>"#,
        r#"</sheetData><mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells></worksheet>"#,
    );

    fn patch(sheet: &Sheet, rewrite: bool, cut: usize) -> String {
        let shared = shared();
        let mut p = Patcher::new(sheet, &shared, rewrite);
        let mut out = Vec::new();
        for piece in XML.as_bytes().chunks(cut) {
            p.feed(piece, &mut out);
        }
        p.finish(&mut out);
        String::from_utf8(out).unwrap()
    }

    fn shared() -> StringTable {
        let mut s = StringTable::new();
        s.push("título");
        s
    }

    #[test]
    fn untouched_sheet_is_identical() {
        let sheet = crate::parse_sheet_xml(XML.as_bytes());
        for cut in [7, 50, 10_000] {
            assert_eq!(patch(&sheet, false, cut), XML);
        }
    }

    #[test]
    fn edits_are_spliced_in_place() {
        let mut sheet = crate::parse_sheet_xml(XML.as_bytes());
        sheet.set_cell(0, 1, CellValue::Text("a<b".into()), None, None); // B1, nova, entre A1 e C1
        sheet.set_cell(1, 0, CellValue::Number(0.5), None, Some(3)); // linha 2, nova
        sheet.set_cell(2, 1, CellValue::Empty, None, Some(0)); // B3 apagada
        sheet.set_cell(9, 2, CellValue::Bool(true), Some("1=1"), None); // depois do fim
        sheet.settle();
        let out = patch(&sheet, false, 13);
        assert_eq!(
            out,
            concat!(
                r#"<?xml version="1.0"?><worksheet><dimension ref="A1:C10"/><sheetData>"#,
                r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t xml:space="preserve">a&lt;b</t></is></c><c r="C1" vm="1"><v>5</v></c></row>"#,
                r#"<row r="2"><c r="A2" s="3"><v>0.5</v></c></row>"#,
                r#"<row r="3"></row>"#,
                r#"<row r="10"><c r="C10" t="b"><f>1=1</f><v>1</v></c></row>"#,
                r#"</sheetData><mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells></worksheet>"#,
            )
        );
        // Relido, dá o mesmo modelo.
        let back = crate::parse_sheet_xml(out.as_bytes());
        let sh = shared();
        for (r, c) in [(0, 0), (0, 1), (0, 2), (1, 0), (2, 1), (9, 2)] {
            assert_eq!(back.cell(r, c, &sh), sheet.cell(r, c, &sh), "{r},{c}");
        }
    }

    #[test]
    fn heights_and_widths() {
        let xml = concat!(
            r#"<worksheet><cols><col min="1" max="5" width="9" style="2" customWidth="1"/></cols>"#,
            r#"<sheetData><row r="2" ht="30" customHeight="1" x14ac:dyDescent="0.25"><c r="A2"><v>1</v></c></row></sheetData></worksheet>"#,
        );
        let mut sheet = crate::parse_sheet_xml(xml.as_bytes());
        sheet.set_col_width(2, Some(20.5));
        sheet.set_col_width(8, Some(3.0));
        sheet.set_row_height(1, None);
        sheet.set_row_height(4, Some(12.75));
        let sh = StringTable::new();
        let mut p = Patcher::new(&sheet, &sh, false);
        let mut out = Vec::new();
        p.feed(xml.as_bytes(), &mut out);
        p.finish(&mut out);
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            concat!(
                r#"<worksheet><cols><col min="1" max="2" width="9" style="2" customWidth="1"/>"#,
                r#"<col min="3" max="3" width="20.5" customWidth="1" style="2"/>"#,
                r#"<col min="4" max="5" width="9" style="2" customWidth="1"/>"#,
                r#"<col min="9" max="9" width="3" customWidth="1"/></cols>"#,
                r#"<sheetData><row r="2" x14ac:dyDescent="0.25"><c r="A2"><v>1</v></c></row>"#,
                r#"<row r="5" ht="12.75" customHeight="1"></row></sheetData></worksheet>"#,
            )
        );
        let back = crate::parse_sheet_xml(out.as_bytes());
        assert_eq!(back.col_width(2), Some(20.5));
        assert_eq!(back.col_width(0), Some(9.0));
        assert_eq!(back.row_height(4), Some(12.75));
        assert_eq!(back.row_height(1), None);
    }

    #[test]
    fn filter_sort_and_table() {
        let xml = concat!(
            r#"<worksheet><sheetData>"#,
            r#"<row r="1"><c r="A1" t="inlineStr"><is><t>UF</t></is></c><c r="B1" t="inlineStr"><is><t>Valor</t></is></c></row>"#,
            r#"<row r="2"><c r="A2" t="inlineStr"><is><t>SP</t></is></c><c r="B2"><v>3</v></c></row>"#,
            r#"<row r="3"><c r="A3" t="inlineStr"><is><t>RJ</t></is></c><c r="B3"><v>1</v></c></row>"#,
            r#"<row r="4"><c r="A4" t="inlineStr"><is><t>MG</t></is></c></row>"#,
            r#"</sheetData><autoFilter ref="A1:B4"/><pageMargins left="0.7"/></worksheet>"#,
        );
        let mut sheet = crate::parse_sheet_xml(xml.as_bytes());
        assert!(sheet.layout.autofilter.is_some());
        let mut af = sheet.layout.autofilter.clone().unwrap();
        af.columns.push(duck_core::FilterColumn {
            col: 0,
            kind: duck_core::FilterKind::Values { values: vec!["SP".into(), "RJ".into()], blank: false },
        });
        duck_core::Command::SetFilter { table: None, filter: Some(af) }.apply(&mut sheet);
        duck_core::Command::RowsHidden(vec![(3, true)]).apply(&mut sheet);
        // Ordena as linhas 2..4 pela coluna B: RJ(1), SP(3), MG(vazia).
        duck_core::Command::SortRows { r0: 1, c0: 0, c1: 1, perm: vec![1, 0, 2] }.apply(&mut sheet);
        let sh = StringTable::new();
        let mut p = Patcher::new(&sheet, &sh, false);
        let mut out = Vec::new();
        p.feed(xml.as_bytes(), &mut out);
        p.finish(&mut out);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains(r#"<autoFilter ref="A1:B4"><filterColumn colId="0"><filters><filter val="SP"/><filter val="RJ"/></filters></filterColumn></autoFilter><pageMargins"#), "{out}");
        let back = crate::parse_sheet_xml(out.as_bytes());
        assert_eq!(back.cell(1, 0, &sh).value, duck_core::Value::Text("RJ"));
        assert_eq!(back.cell(2, 1, &sh).value, duck_core::Value::Number(3.0));
        assert!(back.row_hidden(3));
        assert_eq!(back.layout.autofilter, sheet.layout.autofilter);

        // Parte da tabela: intervalo, filtro e coluna renomeada.
        let table_xml = br#"<table xmlns="x" id="1" name="T" displayName="T" ref="A1:B4"><autoFilter ref="A1:B4"/><tableColumns count="2"><tableColumn id="1" name="UF"/><tableColumn id="2" name="Valor"/></tableColumns><tableStyleInfo name="TableStyleLight1" showRowStripes="1"/></table>"#;
        let mut t = crate::filter::parse_table(table_xml).unwrap();
        t.range.last_row = 9;
        t.rename_column(1, "Total");
        t.filter = Some(duck_core::AutoFilter { range: t.range, columns: Vec::new() });
        let patched = String::from_utf8(patch_table(table_xml, &t)).unwrap();
        assert!(patched.contains(r#"ref="A1:B10"><autoFilter ref="A1:B10"/><tableColumns count="2"><tableColumn id="1" name="UF"/><tableColumn id="2" name="Total"/></tableColumns><tableStyleInfo"#), "{patched}");
    }

    #[test]
    fn merges_hidden_and_panes() {
        let xml = concat!(
            r#"<worksheet><sheetViews><sheetView workbookViewId="0"><selection activeCell="B2" sqref="B2"/></sheetView></sheetViews>"#,
            r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>"#,
            r#"<mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells><pageMargins left="0.7"/></worksheet>"#,
        );
        let mut sheet = crate::parse_sheet_xml(xml.as_bytes());
        duck_core::Command::Merges {
            add: vec![duck_core::CellRange { first_row: 2, first_col: 0, last_row: 3, last_col: 2 }],
            remove: vec![duck_core::CellRange { first_row: 0, first_col: 0, last_row: 0, last_col: 1 }],
        }
        .apply(&mut sheet);
        duck_core::Command::RowsHidden(vec![(0, true), (5, true)]).apply(&mut sheet);
        duck_core::Command::ColsHidden(vec![(3, true)]).apply(&mut sheet);
        duck_core::Command::Pane(Some(duck_core::Pane { frozen_rows: 1, frozen_cols: 0 })).apply(&mut sheet);
        let sh = StringTable::new();
        let mut p = Patcher::new(&sheet, &sh, false);
        let mut out = Vec::new();
        p.feed(xml.as_bytes(), &mut out);
        p.finish(&mut out);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains(r#"<sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView>"#), "{out}");
        assert!(out.contains(r#"<row r="1" hidden="1">"#), "{out}");
        assert!(out.contains(r#"<row r="6" hidden="1"></row>"#), "{out}");
        assert!(out.contains(r#"<col min="4" max="4" hidden="1"/>"#), "{out}");
        assert!(
            out.contains(r#"<mergeCells count="1"><mergeCell ref="A3:C4"/></mergeCells><pageMargins"#),
            "{out}"
        );
        let back = crate::parse_sheet_xml(out.as_bytes());
        assert_eq!(back.layout.merges, sheet.layout.merges);
        assert!(back.row_hidden(0) && back.row_hidden(5) && back.col_hidden(3));
        assert_eq!(back.layout.pane, sheet.layout.pane);
    }

    #[test]
    fn out_of_order_cells_are_not_duplicated() {
        let xml = r#"<worksheet><sheetData><row r="1"><c r="C1"><v>3</v></c><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#;
        let mut sheet = crate::parse_sheet_xml(xml.as_bytes());
        sheet.set_cell(0, 0, CellValue::Number(9.0), None, None);
        let sh = StringTable::new();
        let mut p = Patcher::new(&sheet, &sh, false);
        let mut out = Vec::new();
        p.feed(xml.as_bytes(), &mut out);
        p.finish(&mut out);
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.matches("r=\"A1\"").count(), 1, "{out}");
        assert_eq!(
            crate::parse_sheet_xml(out.as_bytes()).cell(0, 0, &sh).value,
            duck_core::Value::Number(9.0)
        );
    }

    #[test]
    fn rewrite_all_keeps_extra_attributes() {
        let sheet = crate::parse_sheet_xml(XML.as_bytes());
        let out = patch(&sheet, true, 9);
        assert!(out.contains(r#"<c r="C1" vm="1"><v>5</v></c>"#), "{out}");
        assert!(out.contains(r#"<c r="B3"><f>A1+1</f><v>2</v></c>"#), "{out}");
        assert!(out.contains(r#"<row r="1" spans="1:3">"#), "{out}");
    }
}
