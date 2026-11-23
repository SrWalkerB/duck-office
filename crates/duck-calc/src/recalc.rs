//! Grafo de dependências e recálculo em paralelo (F4).
//!
//! - **Blocos**: células seguidas de uma coluna com a mesma fórmula (um
//!   modelo copiado para baixo, um grupo compartilhado) viram um nó só. O
//!   arquivo de referência tem 10 milhões de fórmulas e poucas dezenas de
//!   milhares de blocos.
//! - **Dependências por intervalo**: cada referência de um bloco vira um
//!   retângulo (as relativas andam com as linhas do bloco). Um retângulo que
//!   cobre muitos blocos de uma coluna passa por um nó de coluna, para o
//!   número de arestas não explodir.
//! - **Níveis**: blocos sem dependência entre si são calculados juntos, em
//!   paralelo, em pedaços de linhas. Um bloco que depende de si mesmo
//!   (saldo acumulado: `B3 = B2 + A3`) é calculado de cima para baixo.
//! - Os resultados vão para um [`Overlay`]: as abas não mudam durante o
//!   cálculo (a janela continua mostrando e pode cancelar); quem chama
//!   grava tudo no fim, só o que mudou.

use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use duck_core::{CellError, CellRange, CellValue, FormulaKind};
use rayon::prelude::*;

use crate::eval::{Book, Engine, Full, Overlay};

/// Matrizes que espalham, por (aba, linha, coluna) da fórmula: a área que
/// ocupam (as células dela, sem fórmula, são do resultado).
pub type Spills = HashMap<(u32, u32, u32), CellRange>;

/// Resultado de uma fórmula que espalha (ou que deixou de espalhar).
#[derive(Debug)]
pub struct SpillOut {
    pub sheet: u32,
    pub row: u32,
    pub col: u32,
    /// Área nova (`None` = não espalha: bloqueada ou resultado único).
    pub range: Option<CellRange>,
    /// Matriz antiga (CSE): área fixa.
    pub fixed: bool,
    /// Valores das células da área, sem a da fórmula.
    pub values: Vec<(u32, u32, CellValue)>,
}
use crate::parse::{self, Area, Formula, MAX_COL, MAX_ROW, Node, SheetSel, flag};
use crate::value::{Sid, V};

/// Linhas por tarefa ao calcular um bloco grande.
const CHUNK: u32 = 2048;
/// Acima disso, um retângulo depende da coluna inteira (nó de coluna).
const COL_NODE_OVER: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct Run {
    pub sheet: u32,
    pub col: u32,
    pub r0: u32,
    pub r1: u32,
    pub flags: u8,
    /// Depende de células do próprio bloco: calculado de cima para baixo.
    pub seq: bool,
}

/// Retângulo de precedentes (aba local, inclusivo).
#[derive(Clone, Copy, Debug)]
struct Rect {
    sheet: u32,
    r0: u32,
    r1: u32,
    c0: u32,
    c1: u32,
}

/// (aba, coluna) → (linha inicial, linha final, nó que observa).
type Watch = HashMap<(u32, u32), Vec<(u32, u32, u32)>>;

pub struct Graph {
    pub runs: Vec<Run>,
    /// Blocos de cada (aba, coluna), em ordem de linha.
    by_col: HashMap<(u32, u32), Vec<u32>>,
    /// Arestas (precedente → dependentes) em CSR, sobre nós = blocos + nós de coluna.
    adj_start: Vec<u32>,
    adj: Vec<u32>,
    /// Nós de coluna: (aba, coluna) de cada nó a partir de `runs.len()`.
    col_nodes: Vec<(u32, u32)>,
    /// Quem observa cada coluna: (linha inicial, linha final, nó dependente).
    watch: Watch,
    /// Blocos voláteis (HOJE, AGORA, ALEATÓRIO, INDIRETO, DESLOC).
    volatile: Vec<u32>,
    /// Áreas espalhadas: (bloco da fórmula, área).
    spill_rects: Vec<(u32, Rect)>,
}

/// O que recalcular.
pub enum Dirty {
    /// Tudo (abrir com recálculo, F9 completo).
    All,
    /// Células que mudaram (valor ou fórmula): seus dependentes, mais os voláteis.
    Cells(Vec<(u32, u32, u32)>),
    /// Só os voláteis e dependentes (F9).
    Volatile,
}

/// Resultado de um recálculo.
pub struct Outcome {
    /// Valores novos que diferem dos atuais.
    pub overlay: Overlay,
    /// Fórmulas que espalham.
    pub spills: Vec<SpillOut>,
    pub cells: u64,
    pub levels: usize,
    pub cancelled: bool,
}

impl Graph {
    pub fn build(book: &Book, eng: &Engine) -> Self {
        Self::build_with(book, eng, &Spills::new())
    }

    /// Com as áreas espalhadas (as fórmulas que leem essas células dependem
    /// de quem espalhou).
    pub fn build_with(book: &Book, eng: &Engine, spills: &Spills) -> Self {
        // Blocos, uma aba e coluna por vez (em paralelo por aba).
        let per_sheet: Vec<Vec<Run>> = book
            .sheets
            .par_iter()
            .enumerate()
            .map(|(s, sheet)| {
                let mut runs = Vec::new();
                for col in 0..sheet.cols() {
                    let mut cur: Option<(u32, u32, u32)> = None; // (id, r0, r1)
                    sheet.for_each_formula(col, |row, id| match &mut cur {
                        Some((cid, _, r1)) if *cid == id && *r1 + 1 == row => *r1 = row,
                        _ => {
                            if let Some((_, r0, r1)) = cur.take() {
                                runs.push(Run { sheet: s as u32, col, r0, r1, flags: 0, seq: false });
                            }
                            cur = Some((id, row, row));
                        }
                    });
                    if let Some((_, r0, r1)) = cur {
                        runs.push(Run { sheet: s as u32, col, r0, r1, flags: 0, seq: false });
                    }
                }
                runs
            })
            .collect();
        let mut runs: Vec<Run> = per_sheet.into_iter().flatten().collect();

        let mut by_col: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for (i, r) in runs.iter().enumerate() {
            by_col.entry((r.sheet, r.col)).or_default().push(i as u32);
        }

        // Precedentes de cada bloco (em paralelo).
        let precs: Vec<(Vec<Rect>, u8)> = runs
            .par_iter()
            .map(|r| match eng.formula(book, r.sheet, r.r0, r.col) {
                Some(f) => {
                    let mut out = Vec::new();
                    let mut fl = f.flags;
                    rects(book, eng, &f, r, &mut out, &mut fl, 0);
                    (out, fl)
                }
                None => (Vec::new(), 0),
            })
            .collect();

        let n_runs = runs.len();
        // Áreas espalhadas, com o bloco da fórmula: as do arquivo (matrizes)
        // e as anotadas pelo app.
        let mut spill_rects: Vec<(u32, Rect)> = Vec::new();
        {
            let mut add = |s: u32, row: u32, col: u32, rg: CellRange| {
                if (rg.first_row, rg.first_col) == (rg.last_row, rg.last_col) {
                    return;
                }
                let Some(list) = by_col.get(&(s, col)) else { return };
                let i = list.partition_point(|&k| runs[k as usize].r1 < row);
                if let Some(&k) = list.get(i)
                    && runs[k as usize].r0 <= row
                {
                    spill_rects.push((
                        k,
                        Rect {
                            sheet: s,
                            r0: rg.first_row,
                            r1: rg.last_row,
                            c0: rg.first_col,
                            c1: rg.last_col,
                        },
                    ));
                }
            };
            for (s, sheet) in book.sheets.iter().enumerate() {
                for (row, col, rg, _) in sheet.array_formulas() {
                    add(s as u32, row, col, rg);
                }
            }
            for (&(s, row, col), &rg) in spills {
                add(s, row, col, rg);
            }
        }
        let mut col_nodes: Vec<(u32, u32)> = Vec::new();
        let mut col_node_of: HashMap<(u32, u32), u32> = HashMap::new();
        let mut edges: Vec<(u32, u32)> = Vec::new();
        let mut watch: Watch = HashMap::new();
        let mut volatile = Vec::new();
        for (i, (rs, fl)) in precs.iter().enumerate() {
            let me = i as u32;
            runs[i].flags = *fl;
            if fl & (flag::VOLATILE | flag::DYNAMIC) != 0 {
                volatile.push(me);
            }
            for rc in rs {
                // Células espalhadas por outra fórmula: depende dela.
                for &(k, sr) in &spill_rects {
                    if k != me
                        && sr.sheet == rc.sheet
                        && sr.r0 <= rc.r1
                        && rc.r0 <= sr.r1
                        && sr.c0 <= rc.c1
                        && rc.c0 <= sr.c1
                    {
                        edges.push((k, me));
                    }
                }
                let used_cols = book.sheets.get(rc.sheet as usize).map_or(0, |s| s.cols());
                let c1 = rc.c1.min(used_cols.saturating_sub(1));
                if rc.c0 > c1 {
                    continue;
                }
                for col in rc.c0..=c1 {
                    watch.entry((rc.sheet, col)).or_default().push((rc.r0, rc.r1, me));
                    let Some(list) = by_col.get(&(rc.sheet, col)) else { continue };
                    // Blocos da coluna que cruzam as linhas do retângulo.
                    let start = list.partition_point(|&k| runs[k as usize].r1 < rc.r0);
                    let end = list.partition_point(|&k| runs[k as usize].r0 <= rc.r1);
                    if start >= end {
                        continue;
                    }
                    if end - start > COL_NODE_OVER {
                        let node = *col_node_of.entry((rc.sheet, col)).or_insert_with(|| {
                            col_nodes.push((rc.sheet, col));
                            (n_runs + col_nodes.len() - 1) as u32
                        });
                        edges.push((node, me));
                        continue;
                    }
                    for &k in &list[start..end] {
                        if k == me {
                            runs[i].seq = true;
                        } else {
                            edges.push((k, me));
                        }
                    }
                }
            }
        }
        // Nós de coluna dependem de todos os blocos da coluna.
        for (j, key) in col_nodes.iter().enumerate() {
            let node = (n_runs + j) as u32;
            for &k in by_col.get(key).map(Vec::as_slice).unwrap_or(&[]) {
                edges.push((k, node));
            }
        }
        edges.sort_unstable();
        edges.dedup();
        let n_nodes = n_runs + col_nodes.len();
        let mut adj_start = vec![0u32; n_nodes + 1];
        for &(a, _) in &edges {
            adj_start[a as usize + 1] += 1;
        }
        for i in 0..n_nodes {
            adj_start[i + 1] += adj_start[i];
        }
        let adj = edges.iter().map(|e| e.1).collect();
        for list in watch.values_mut() {
            list.sort_unstable();
        }
        Self { runs, by_col, adj_start, adj, col_nodes, watch, volatile, spill_rects }
    }

    pub fn nodes(&self) -> usize {
        self.runs.len() + self.col_nodes.len()
    }

    pub fn edges(&self) -> usize {
        self.adj.len()
    }

    fn deps(&self, n: u32) -> &[u32] {
        &self.adj[self.adj_start[n as usize] as usize..self.adj_start[n as usize + 1] as usize]
    }

    /// Bloco que contém a célula, se ela tem fórmula.
    pub fn run_at(&self, sheet: u32, row: u32, col: u32) -> Option<u32> {
        let list = self.by_col.get(&(sheet, col))?;
        let i = list.partition_point(|&k| self.runs[k as usize].r1 < row);
        let &k = list.get(i)?;
        (self.runs[k as usize].r0 <= row).then_some(k)
    }

    /// Nós a recalcular, em níveis (cada nível só depende dos anteriores).
    fn plan(&self, dirty: &Dirty) -> Vec<Vec<u32>> {
        let n = self.nodes();
        let mut mark = vec![false; n];
        let mut stack: Vec<u32> = Vec::new();
        let seed = |k: u32, mark: &mut Vec<bool>, stack: &mut Vec<u32>| {
            if !mark[k as usize] {
                mark[k as usize] = true;
                stack.push(k);
            }
        };
        match dirty {
            Dirty::All => {
                for k in 0..n as u32 {
                    seed(k, &mut mark, &mut stack);
                }
            }
            Dirty::Volatile | Dirty::Cells(_) => {
                for &k in &self.volatile {
                    seed(k, &mut mark, &mut stack);
                }
                if let Dirty::Cells(cells) = dirty {
                    for &(s, r, c) in cells {
                        if let Some(k) = self.run_at(s, r, c) {
                            seed(k, &mut mark, &mut stack);
                        }
                        // Mexeu na área de uma matriz que espalha: ela confere de novo.
                        for &(k, sr) in &self.spill_rects {
                            if sr.sheet == s && sr.r0 <= r && r <= sr.r1 && sr.c0 <= c && c <= sr.c1 {
                                seed(k, &mut mark, &mut stack);
                            }
                        }
                        if let Some(list) = self.watch.get(&(s, c)) {
                            let end = list.partition_point(|w| w.0 <= r);
                            for w in &list[..end] {
                                if w.1 >= r {
                                    seed(w.2, &mut mark, &mut stack);
                                }
                            }
                        }
                    }
                }
            }
        }
        // Fecho: tudo o que depende do que está marcado.
        while let Some(k) = stack.pop() {
            for &d in self.deps(k) {
                if !mark[d as usize] {
                    mark[d as usize] = true;
                    stack.push(d);
                }
            }
        }
        // Kahn sobre os marcados.
        let mut indeg = vec![0u32; n];
        for k in 0..n {
            if mark[k] {
                for &d in self.deps(k as u32) {
                    if mark[d as usize] {
                        indeg[d as usize] += 1;
                    }
                }
            }
        }
        let mut level: Vec<u32> =
            (0..n as u32).filter(|&k| mark[k as usize] && indeg[k as usize] == 0).collect();
        let mut levels = Vec::new();
        let mut done = 0usize;
        let total = mark.iter().filter(|&&m| m).count();
        while !level.is_empty() {
            done += level.len();
            let mut next = Vec::new();
            for &k in &level {
                for &d in self.deps(k) {
                    if mark[d as usize] {
                        indeg[d as usize] -= 1;
                        if indeg[d as usize] == 0 {
                            next.push(d);
                        }
                    }
                }
            }
            levels.push(std::mem::take(&mut level));
            level = next;
        }
        if done < total {
            // Referência circular: o que sobrou vai num nível final, calculado
            // uma vez em ordem de posição (o Excel avisaria e pararia).
            let mut rest: Vec<u32> =
                (0..n as u32).filter(|&k| mark[k as usize] && indeg[k as usize] > 0).collect();
            rest.sort_by_key(|&k| self.runs.get(k as usize).map(|r| (r.sheet, r.r0, r.col)));
            for k in rest {
                levels.push(vec![k]);
            }
        }
        levels
    }

    /// Recalcula. `cancel` pode interromper entre níveis.
    pub fn recalc(&self, book: &Book, eng: &Engine, dirty: &Dirty, cancel: &AtomicBool) -> Outcome {
        self.recalc_with(book, eng, dirty, cancel, &Spills::new())
    }

    /// Como [`Graph::recalc`], sabendo as áreas que cada fórmula já espalhou
    /// (podem ser reocupadas; o resto precisa estar vazio).
    pub fn recalc_with(
        &self,
        book: &Book,
        eng: &Engine,
        dirty: &Dirty,
        cancel: &AtomicBool,
        spills: &Spills,
    ) -> Outcome {
        let levels = self.plan(dirty);
        let mut overlay = Overlay::default();
        let mut spill_out: Vec<SpillOut> = Vec::new();
        let mut cells = 0u64;
        let n_levels = levels.len();
        for level in levels {
            if cancel.load(Ordering::Relaxed) {
                return Outcome { overlay, spills: spill_out, cells, levels: n_levels, cancelled: true };
            }
            // Tarefas: pedaços de linhas dos blocos do nível.
            let mut tasks: Vec<(u32, u32, u32)> = Vec::new();
            for &k in &level {
                let Some(r) = self.runs.get(k as usize) else { continue };
                if r.flags & flag::OPAQUE != 0 {
                    continue;
                }
                if r.seq {
                    tasks.push((k, r.r0, r.r1));
                } else {
                    let mut a = r.r0;
                    while a <= r.r1 {
                        let b = (a + CHUNK - 1).min(r.r1);
                        tasks.push((k, a, b));
                        a = b + 1;
                    }
                }
            }
            if tasks.is_empty() {
                continue;
            }
            let view = Book { overlay: Some(&overlay), ..book.clone() };
            type TaskOut = (Vec<(u32, u32, u32, CellValue)>, Vec<SpillOut>);
            let results: Vec<TaskOut> = tasks
                .par_iter()
                .map(|&(k, a, b)| {
                    let r = self.runs[k as usize];
                    let mut out = Vec::new();
                    let mut sp = Vec::new();
                    if cancel.load(Ordering::Relaxed) {
                        return (out, sp);
                    }
                    if r.seq {
                        // De cima para baixo, vendo os próprios resultados.
                        let local = RwLock::new(Overlay::default());
                        let seq_view = Book { local: Some(&local), ..view.clone() };
                        for row in a..=b {
                            if let Some(v) = eng.eval_cell(&seq_view, r.sheet, row, r.col)
                                && !same(&seq_view.get(Sid::Local(r.sheet), row, r.col), &v)
                            {
                                local.write().unwrap().set(r.sheet, row, r.col, v.clone());
                                out.push((r.sheet, row, r.col, v));
                            }
                        }
                    } else {
                        let sheet = &view.sheets[r.sheet as usize];
                        for row in a..=b {
                            // Espalham: fórmulas de matriz e as digitadas nesta sessão.
                            let spill_ok = matches!(
                                sheet.formula(row, r.col).map(|f| f.kind),
                                Some(FormulaKind::Array { .. })
                            ) || sheet.edits().has(row, r.col);
                            let v = match eng.eval_full(&view, r.sheet, row, r.col, spill_ok) {
                                None => continue,
                                Some(Full::One(v)) => {
                                    // Espalhava e agora é um valor só: a área volta a ser só ela.
                                    if spills.contains_key(&(r.sheet, row, r.col)) {
                                        sp.push(SpillOut {
                                            sheet: r.sheet,
                                            row,
                                            col: r.col,
                                            range: None,
                                            fixed: false,
                                            values: Vec::new(),
                                        });
                                    }
                                    v
                                }
                                Some(Full::Many { rows, cols, v }) => {
                                    let (master, s) =
                                        spill(&view, r.sheet, row, r.col, rows, cols, v, spills);
                                    for x in &s.values {
                                        out.push((r.sheet, x.0, x.1, x.2.clone()));
                                    }
                                    sp.push(s);
                                    master
                                }
                            };
                            if !same(&view.get(Sid::Local(r.sheet), row, r.col), &v) {
                                out.push((r.sheet, row, r.col, v));
                            }
                        }
                    }
                    (out, sp)
                })
                .collect();
            if cancel.load(Ordering::Relaxed) {
                return Outcome { overlay, spills: spill_out, cells, levels: n_levels, cancelled: true };
            }
            cells += tasks.iter().map(|t| (t.2 - t.1 + 1) as u64).sum::<u64>();
            for (list, sp) in results {
                for (s, r, c, v) in list {
                    overlay.set(s, r, c, v);
                }
                spill_out.extend(sp);
            }
        }
        Outcome { overlay, spills: spill_out, cells, levels: n_levels, cancelled: false }
    }
}

/// Uma fórmula com resultado matricial: a área que ocupa e os valores, ou
/// `#DESPEJAR!` se a área está ocupada por outra coisa. Matrizes antigas
/// (CSE) têm a área fixa (o que falta vira `#N/D`).
#[allow(clippy::too_many_arguments)]
fn spill(
    book: &Book,
    sheet: u32,
    row: u32,
    col: u32,
    rows: u32,
    cols: u32,
    v: Vec<CellValue>,
    spills: &Spills,
) -> (CellValue, SpillOut) {
    let fixed = match book.sheets[sheet as usize].formula(row, col).map(|f| f.kind) {
        Some(FormulaKind::Array { range, dynamic: false }) => Some(range),
        _ => None,
    };
    let range = fixed.unwrap_or(CellRange {
        first_row: row,
        first_col: col,
        last_row: row.saturating_add(rows - 1),
        last_col: col.saturating_add(cols - 1),
    });
    let mut out = SpillOut { sheet, row, col, range: None, fixed: fixed.is_some(), values: Vec::new() };
    if range.last_row >= MAX_ROW || range.last_col >= MAX_COL {
        return (CellValue::Error(CellError::Spill), out);
    }
    let owned = spills.get(&(sheet, row, col));
    if fixed.is_none() {
        let s = &book.sheets[sheet as usize];
        for rr in range.first_row..=range.last_row {
            for cc in range.first_col..=range.last_col {
                if (rr, cc) == (row, col) || owned.is_some_and(|o| o.contains(rr, cc)) {
                    continue;
                }
                let busy =
                    s.formula(rr, cc).is_some() || !matches!(book.get(Sid::Local(sheet), rr, cc), V::Empty);
                if busy {
                    return (CellValue::Error(CellError::Spill), out);
                }
            }
        }
    }
    let at = |i: u32, j: u32| -> CellValue {
        let i = if rows == 1 { 0 } else { i };
        let j = if cols == 1 { 0 } else { j };
        if i < rows && j < cols {
            v[(i * cols + j) as usize].clone()
        } else {
            CellValue::Error(CellError::NA)
        }
    };
    for rr in range.first_row..=range.last_row {
        for cc in range.first_col..=range.last_col {
            if (rr, cc) != (row, col) {
                out.values.push((rr, cc, at(rr - row, cc - col)));
            }
        }
    }
    out.range = Some(range);
    (at(0, 0), out)
}

/// O valor atual da célula já é o novo?
fn same(old: &V, new: &CellValue) -> bool {
    match (old, new) {
        // Diferença só além do 15º algarismo (ordem das somas): é o mesmo número.
        (V::Num(a), CellValue::Number(b)) => a == b || crate::index::round15(*a) == crate::index::round15(*b),
        (V::Str(a), CellValue::Text(b)) => a == b,
        (V::Empty, CellValue::Text(b)) => b.is_empty(),
        (V::Empty, CellValue::Empty) => true,
        (V::Bool(a), CellValue::Bool(b)) => a == b,
        (V::Err(a), CellValue::Error(b)) => a == b,
        _ => false,
    }
}

/// Retângulos de precedentes de uma fórmula para as linhas do bloco.
fn rects(book: &Book, eng: &Engine, f: &Formula, run: &Run, out: &mut Vec<Rect>, fl: &mut u8, depth: u32) {
    for n in &f.nodes {
        match n {
            Node::Area(a) => {
                if let Some(r) = area_rect(a, run) {
                    out.push(r);
                }
            }
            Node::Name(i) if depth < 8 => {
                if let Some(nf) = eng.name_formula(book, *i) {
                    *fl |= nf.flags;
                    rects(book, eng, nf, run, out, fl, depth + 1);
                }
            }
            _ => {}
        }
    }
    if *fl & flag::DYNAMIC != 0 {
        // INDIRETO/DESLOC: pode ler qualquer lugar das abas citadas (ou da própria).
        let mut sheets: Vec<u32> = out.iter().map(|r| r.sheet).collect();
        sheets.push(run.sheet);
        sheets.sort_unstable();
        sheets.dedup();
        for s in sheets {
            out.push(Rect { sheet: s, r0: 0, r1: MAX_ROW, c0: 0, c1: MAX_COL });
        }
    }
}

fn area_rect(a: &Area, run: &Run) -> Option<Rect> {
    let sheet = match a.sheet {
        SheetSel::This => run.sheet,
        SheetSel::Local(s) if s != u32::MAX => s,
        _ => return None,
    };
    let span = |c: parse::Coord, lo: u32, hi: u32| -> (i64, i64) {
        if c.abs { (c.v as i64, c.v as i64) } else { (lo as i64 + c.v as i64, hi as i64 + c.v as i64) }
    };
    let (a0, a1) = span(a.r0, run.r0, run.r1);
    let (b0, b1) = span(a.r1, run.r0, run.r1);
    let (c0, _) = span(a.c0, run.col, run.col);
    let (d0, _) = span(a.c1, run.col, run.col);
    let r0 = a0.min(b0).max(0);
    let r1 = a1.max(b1).min(MAX_ROW as i64);
    let (c0, c1) = (c0.min(d0).max(0), c0.max(d0).min(MAX_COL as i64));
    (r0 <= r1 && c0 <= c1).then_some(Rect {
        sheet,
        r0: r0 as u32,
        r1: r1 as u32,
        c0: c0 as u32,
        c1: c1 as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{BookExtras, CellValue, FormulaKind, Sheet, SheetPart, StringTable, kind};

    fn sheet() -> Sheet {
        let mut p = SheetPart::new();
        for r in 0..5u32 {
            p.set(r, 0, kind::NUMBER, 0, ((r + 1) as f64).to_bits());
            // B = A*2 (modelo relativo copiado), D = saldo acumulado.
            let t = crate::to_template(&format!("A{}*2", r + 1), r, 1).unwrap();
            p.set_formula_template(r, 1, &t);
            p.set(r, 1, kind::NUMBER | kind::FORMULA, 0, 0f64.to_bits());
            let d = if r == 0 { "A1".to_string() } else { format!("D{}+A{}", r, r + 1) };
            let t = crate::to_template(&d, r, 3).unwrap();
            p.set_formula_template(r, 3, &t);
            p.set(r, 3, kind::NUMBER | kind::FORMULA, 0, 0f64.to_bits());
        }
        p.set_formula(0, 2, "SUM(B1:B5)", FormulaKind::Normal);
        p.set(0, 2, kind::NUMBER | kind::FORMULA, 0, 0f64.to_bits());
        let mut s = Sheet::new("Dados");
        s.merge_part(p);
        s.finish();
        s
    }

    fn num(o: &Overlay, r: u32, c: u32) -> Option<f64> {
        match o.get(0, r, c)? {
            CellValue::Number(x) => Some(*x),
            _ => None,
        }
    }

    #[test]
    fn full_and_incremental() {
        let mut s = sheet();
        let shared = StringTable::new();
        let extras = BookExtras::default();
        let cancel = AtomicBool::new(false);
        let out = {
            let book = Book::new(vec![&s], &shared, &extras, false);
            let eng = Engine::new(&book);
            let g = Graph::build(&book, &eng);
            assert!(g.runs.iter().any(|r| r.seq), "saldo acumulado é sequencial");
            g.recalc(&book, &eng, &Dirty::All, &cancel)
        };
        assert_eq!(num(&out.overlay, 4, 1), Some(10.0));
        assert_eq!(num(&out.overlay, 0, 2), Some(30.0));
        assert_eq!(num(&out.overlay, 4, 3), Some(15.0));
        // Grava e edita A1.
        s.set_calc_values(out.overlay.drain().map(|(_, r, c, v)| (r, c, v)).collect());
        s.set_cell(0, 0, CellValue::Number(11.0), None, None);
        let book = Book::new(vec![&s], &shared, &extras, false);
        let eng = Engine::new(&book);
        let g = Graph::build(&book, &eng);
        let out = g.recalc(&book, &eng, &Dirty::Cells(vec![(0, 0, 0)]), &cancel);
        assert_eq!(num(&out.overlay, 0, 1), Some(22.0));
        assert_eq!(num(&out.overlay, 0, 2), Some(50.0));
        assert_eq!(num(&out.overlay, 4, 3), Some(25.0));
        // B2..B5 não mudaram: não estão no resultado.
        assert_eq!(num(&out.overlay, 3, 1), None);
    }

    #[test]
    fn dynamic_arrays_spill() {
        let mut s = sheet();
        let shared = StringTable::new();
        let extras = BookExtras::default();
        let cancel = AtomicBool::new(false);
        // Fórmula digitada (desta sessão): espalha. A1:A5 = 1..5.
        s.set_cell(0, 5, CellValue::Number(0.0), Some("_xlfn._xlws.FILTER(A1:A5,A1:A5>2)"), None);
        s.set_cell(0, 6, CellValue::Number(0.0), Some("_xlfn.LET(_xlpm.x,A2,_xlpm.x*10)"), None);
        s.set_cell(0, 7, CellValue::Number(0.0), Some("A1:A3*2"), None);
        let book = Book::new(vec![&s], &shared, &extras, false);
        let eng = Engine::new(&book);
        let g = Graph::build(&book, &eng);
        let out = g.recalc(&book, &eng, &Dirty::Cells(vec![(0, 0, 5), (0, 0, 6), (0, 0, 7)]), &cancel);
        assert_eq!(num(&out.overlay, 0, 5), Some(3.0));
        assert_eq!(num(&out.overlay, 0, 6), Some(20.0));
        let f = out.spills.iter().find(|x| x.col == 5).expect("FILTRO espalha");
        assert_eq!(f.range.map(|r| (r.first_row, r.last_row)), Some((0, 2)));
        assert_eq!(f.values.len(), 2);
        // Referência multiplicada (Excel atual): espalha 3 linhas.
        let m = out.spills.iter().find(|x| x.col == 7).expect("A1:A3*2 espalha");
        assert_eq!(
            m.values.iter().map(|v| v.2.clone()).collect::<Vec<_>>(),
            vec![CellValue::Number(4.0), CellValue::Number(6.0)]
        );
        // Área ocupada: #DESPEJAR!.
        s.set_cell(1, 5, CellValue::Text("x".into()), None, None);
        let book = Book::new(vec![&s], &shared, &extras, false);
        let eng = Engine::new(&book);
        let g = Graph::build(&book, &eng);
        let out = g.recalc(&book, &eng, &Dirty::Cells(vec![(0, 0, 5)]), &cancel);
        assert_eq!(out.overlay.get(0, 0, 5), Some(&CellValue::Error(duck_core::CellError::Spill)));
    }
}
