//! Recálculo em segundo plano (F4).
//!
//! Depois de abrir (voláteis, como o Excel) e de cada edição, uma thread
//! recalcula os dependentes com o motor do `duck-calc`, lendo as abas por
//! `Arc` (a janela continua desenhando). O resultado volta como um conjunto
//! de valores novos, gravados de uma vez na janela. Uma edição nova cancela
//! o cálculo em andamento (que para em milissegundos) e junta o que faltava
//! ao próximo.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use duck_calc::{Book, Dirty, Engine, Graph, Overlay};
use duck_core::{BookExtras, Command, Sheet, StringTable};

/// O que falta recalcular.
#[derive(Clone, Debug, Default)]
pub enum Pending {
    #[default]
    None,
    Volatile,
    Cells(Vec<(u32, u32, u32)>),
    All,
}

impl Pending {
    pub fn add(&mut self, other: Pending) {
        *self = match (std::mem::take(self), other) {
            (Pending::All, _) | (_, Pending::All) => Pending::All,
            (Pending::None, o) | (o, Pending::None) => o,
            (Pending::Volatile, o) | (o, Pending::Volatile) => match o {
                Pending::Cells(c) => Pending::Cells(c),
                o => o,
            },
            (Pending::Cells(mut a), Pending::Cells(b)) => {
                a.extend(b);
                if a.len() > 200_000 { Pending::All } else { Pending::Cells(a) }
            }
        };
    }

    fn dirty(&self) -> Dirty {
        match self {
            Pending::All => Dirty::All,
            Pending::Cells(c) => Dirty::Cells(c.clone()),
            _ => Dirty::Volatile,
        }
    }
}

pub struct Done {
    pub engine: Arc<Engine>,
    pub graph: Arc<Graph>,
    pub overlay: Overlay,
    pub cells: u64,
    pub elapsed: Duration,
    pub cancelled: bool,
}

struct Job {
    cancel: Arc<AtomicBool>,
    rx: Receiver<Done>,
    handle: std::thread::JoinHandle<()>,
    /// O que este cálculo cobre (volta para `pending` se for cancelado).
    covers: Pending,
}

#[derive(Default)]
pub struct Calc {
    pub extras: Arc<BookExtras>,
    engine: Option<Arc<Engine>>,
    graph: Option<Arc<Graph>>,
    /// Fórmulas ou estrutura mudaram: motor e grafo são refeitos antes do próximo cálculo.
    stale: bool,
    pending: Pending,
    job: Option<Job>,
    /// Último cálculo (para a barra de status).
    pub last: Option<(Duration, u64, usize)>,
}

impl Calc {
    pub fn new(extras: Arc<BookExtras>) -> Self {
        Self { extras, stale: true, pending: Pending::Volatile, ..Default::default() }
    }

    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// Anota o que um comando mudou.
    pub fn note(&mut self, sheet: usize, cmd: &Command) {
        let s = sheet as u32;
        match cmd {
            Command::SetCells(_) | Command::Restore(_) => {
                self.stale = true;
                self.pending.add(Pending::Cells(cmd.cells().into_iter().map(|(r, c)| (s, r, c)).collect()));
            }
            Command::Shift(..) | Command::SortRows { .. } => {
                self.stale = true;
                self.pending.add(Pending::All);
            }
            Command::Group(v) => v.iter().for_each(|c| self.note(sheet, c)),
            // Formatação, tamanhos, filtro, mesclagens: nada a recalcular.
            _ => {}
        }
    }

    /// Pede um recálculo (F9 = voláteis, Ctrl+Alt+F9 = tudo).
    pub fn request(&mut self, p: Pending) {
        if matches!(p, Pending::All) {
            self.stale = true;
        }
        self.pending.add(p);
    }

    /// Para o cálculo em andamento (antes de editar ou salvar): o que ele
    /// cobria volta para a fila.
    pub fn cancel(&mut self) {
        let Some(job) = self.job.take() else { return };
        job.cancel.store(true, Ordering::Relaxed);
        let _ = job.handle.join();
        self.pending.add(job.covers);
    }

    /// Começa um cálculo se há algo pendente e nenhum em andamento.
    pub fn start(
        &mut self,
        sheets: Vec<Arc<Sheet>>,
        strings: Arc<StringTable>,
        date1904: bool,
        waker: duck_platform::Waker,
    ) {
        if self.job.is_some() || matches!(self.pending, Pending::None) {
            return;
        }
        let covers = std::mem::take(&mut self.pending);
        let dirty = covers.dirty();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let extras = self.extras.clone();
        let reuse = if self.stale { None } else { self.engine.clone().zip(self.graph.clone()) };
        self.stale = false;
        let c = cancel.clone();
        let handle = std::thread::Builder::new()
            .name("duck-calc".into())
            .spawn(move || {
                let t = Instant::now();
                let done = {
                    let refs: Vec<&Sheet> = sheets.iter().map(|s| &**s).collect();
                    let book = Book::new(refs, &strings, &extras, date1904);
                    let (engine, graph) = match reuse {
                        Some((e, g)) => {
                            if let Dirty::Cells(cells) = &dirty {
                                for &(s, _, col) in cells {
                                    e.index.invalidate(duck_calc::value::Sid::Local(s), col, col);
                                }
                            }
                            (e, g)
                        }
                        None => {
                            let e = Engine::new(&book);
                            let g = Graph::build(&book, &e);
                            (Arc::new(e), Arc::new(g))
                        }
                    };
                    let out = graph.recalc(&book, &engine, &dirty, &c);
                    Done {
                        engine,
                        graph,
                        overlay: out.overlay,
                        cells: out.cells,
                        elapsed: t.elapsed(),
                        cancelled: out.cancelled,
                    }
                };
                // As abas são soltas antes de avisar: a janela já pode escrever nelas.
                drop(sheets);
                let _ = tx.send(done);
                waker.wake();
            })
            .expect("thread de cálculo");
        self.job = Some(Job { cancel, rx, handle, covers });
    }

    /// Resultado de um cálculo que terminou (`block` = espera ele terminar).
    pub fn poll(&mut self, block: bool) -> Option<Done> {
        let job = self.job.as_ref()?;
        let done = if block { job.rx.recv().ok()? } else { job.rx.try_recv().ok()? };
        let job = self.job.take().unwrap();
        let _ = job.handle.join();
        self.engine = Some(done.engine.clone());
        self.graph = Some(done.graph.clone());
        if done.cancelled {
            self.pending.add(job.covers);
            return None;
        }
        // Os índices de busca das colunas que mudaram ficam velhos.
        let mut cols: Vec<(u32, u32)> = done.overlay.columns().collect();
        cols.sort_unstable();
        cols.dedup();
        for (s, c) in cols {
            done.engine.index.invalidate(duck_calc::value::Sid::Local(s), c, c);
        }
        self.last = Some((done.elapsed, done.cells, done.overlay.len()));
        Some(done)
    }
}

impl crate::app::SheetsApp {
    /// Começa o recálculo pendente, se o arquivo já carregou e não está salvando.
    pub fn calc_kick(&mut self) {
        if self.saving.is_some() {
            return;
        }
        let Some(waker) = self.waker.clone() else { return };
        let Some(doc) = &mut self.doc else { return };
        if doc.finished.is_none() || doc.sheets.iter().any(|s| s.data.is_none() || s.preview) {
            return;
        }
        let sheets: Vec<Arc<Sheet>> = doc.sheets.iter().filter_map(|s| s.data.clone()).collect();
        let (strings, date1904) = (doc.strings.clone(), doc.date1904);
        doc.calc.start(sheets, strings, date1904, waker);
    }

    /// Termina todo o recálculo pendente antes de seguir (edições de teste
    /// antes de `--save-to`).
    pub fn calc_finish(&mut self) {
        for _ in 0..16 {
            self.calc_kick();
            let Some(doc) = &mut self.doc else { return };
            let Some(done) = doc.calc.poll(true) else {
                if doc.calc.running() {
                    continue;
                }
                return;
            };
            self.apply_calc(done);
        }
    }

    /// Grava os resultados de um recálculo que terminou. `true` = redesenhar.
    pub fn poll_calc(&mut self) -> bool {
        let Some(doc) = &mut self.doc else { return false };
        let Some(done) = doc.calc.poll(false) else {
            if !doc.calc.running() {
                // Cancelado (ou nada): pode haver mais na fila.
                self.calc_kick();
            }
            return false;
        };
        self.apply_calc(done);
        self.calc_kick();
        true
    }

    fn apply_calc(&mut self, done: Done) {
        let Some(doc) = &mut self.doc else { return };
        let changed = done.overlay.len();
        let mut per_sheet: Vec<Vec<(u32, u32, duck_core::CellValue)>> = vec![Vec::new(); doc.sheets.len()];
        for (s, r, c, v) in done.overlay.drain() {
            if let Some(list) = per_sheet.get_mut(s as usize) {
                list.push((r, c, v));
            }
        }
        let mut touched = Vec::new();
        for (i, vals) in per_sheet.into_iter().enumerate() {
            if vals.is_empty() {
                continue;
            }
            let Some(sheet) = doc.sheets.get_mut(i).and_then(crate::edit::sheet_mut) else { continue };
            sheet.set_calc_values(vals);
            touched.push(i);
        }
        for i in touched {
            // A formatação condicional depende dos valores.
            let n = doc.sheets[i].data.as_ref().map_or(0, |s| s.layout.cond.len());
            if let Some(v) = doc.sheets[i].view.as_mut() {
                v.cf = (0..n).map(|_| duck_core::CfState::default()).collect();
            }
        }
        self.stats = None;
        if changed > 0 {
            self.refresh_charts();
        }
        if changed > 0 {
            self.message = format!(
                "Calculado: {} fórmulas em {} ms, {} valores mudaram",
                done.cells,
                done.elapsed.as_millis(),
                changed
            );
        }
    }
}
