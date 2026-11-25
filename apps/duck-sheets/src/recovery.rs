//! Cópias de recuperação: o que ainda não foi salvo não se perde se o programa
//! ou o computador fecharem sem aviso.
//!
//! Com alterações não salvas, depois de um tempo e com o usuário parado, uma
//! thread grava uma cópia inteira da planilha (pelo mesmo caminho de salvar) na
//! pasta do usuário. Salvar, descartar ou fechar normalmente apaga a cópia. Se
//! o programa quebrar, ele ainda grava uma última antes de sair. Na próxima
//! abertura, as que sobraram são oferecidas.
//!
//! Cada cópia tem três arquivos: `<id>.lock` (travado enquanto o programa está
//! vivo: quem não consegue travar sabe que a cópia tem dono), `<id>.txt` (de
//! que arquivo é e de quando) e a planilha.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use duck_core::{Sheet, StringTable, Styles};
use duck_xlsx::{SaveOptions, Source};

use crate::app::{Doc, SheetsApp};
use crate::dialog::{Dialog, Field, Kind};

/// Da primeira alteração até a cópia.
const EVERY: Duration = Duration::from_secs(45);
/// Espera o usuário parar.
const IDLE: Duration = Duration::from_secs(2);
/// Restos sem planilha mais velhos que isto são apagados.
const STALE: Duration = Duration::from_secs(24 * 3600);
/// Tamanho do registro de erros; passando, o anterior vira `erros-anterior.txt`.
const LOG_MAX: u64 = 256 * 1024;

/// Pasta das cópias e do registro de erros. `DUCK_RECUPERACAO` troca a pasta
/// (para testar sem mexer na de quem usa).
pub fn home() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DUCK_RECUPERACAO") {
        return Some(PathBuf::from(d));
    }
    if cfg!(test) {
        return None;
    }
    duck_platform::state_dir("duck-office")
}

/// Uma cópia na pasta, travada por este processo.
#[derive(Debug)]
pub struct Entry {
    dir: PathBuf,
    id: String,
    lock: File,
    /// Nome do arquivo da planilha (vazio = ainda não gravada).
    file: String,
    /// De onde veio (vazio = planilha nova).
    original: String,
    label: String,
    when: String,
}

impl Entry {
    fn create(dir: &Path) -> std::io::Result<Self> {
        private_dir(dir)?;
        let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
        let id = format!("{}-{}", since.as_millis(), std::process::id());
        let lock = File::create(dir.join(format!("{id}.lock")))?;
        lock.try_lock()?;
        Ok(Self {
            dir: dir.to_path_buf(),
            id,
            lock,
            file: String::new(),
            original: String::new(),
            label: String::new(),
            when: String::new(),
        })
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(&self.file)
    }

    /// Já há uma planilha gravada?
    fn written(&self) -> bool {
        !self.file.is_empty() && self.path().is_file()
    }

    pub fn original(&self) -> Option<&Path> {
        (!self.original.is_empty()).then(|| Path::new(&self.original))
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// Apaga a cópia.
    fn remove(self) {
        let Self { dir, id, lock, file, .. } = self;
        if !file.is_empty() {
            let _ = std::fs::remove_file(dir.join(file));
        }
        let _ = std::fs::remove_file(dir.join(format!("{id}.txt")));
        drop(lock);
        let _ = std::fs::remove_file(dir.join(format!("{id}.lock")));
    }

    fn write_info(&self) -> std::io::Result<()> {
        let line = |s: &str| s.replace(['\r', '\n'], " ");
        let text = format!(
            "arquivo={}\noriginal={}\nnome={}\nquando={}\n",
            self.file,
            line(&self.original),
            line(&self.label),
            self.when
        );
        std::fs::write(self.dir.join(format!("{}.txt", self.id)), text)
    }
}

/// Cria a pasta; onde há permissões, só o usuário entra.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

/// As cópias sem dono na pasta, da mais nova para a mais velha.
pub fn pending(dir: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".lock") else { continue };
        let Ok(lock) = File::options().write(true).open(e.path()) else { continue };
        if lock.try_lock().is_err() {
            continue;
        }
        let info = std::fs::read_to_string(dir.join(format!("{id}.txt"))).unwrap_or_default();
        let get = |k: &str| {
            info.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(x, _)| *x == k)
                .map(|(_, v)| v.to_string())
                .unwrap_or_default()
        };
        let entry = Entry {
            dir: dir.to_path_buf(),
            id: id.to_string(),
            lock,
            // Só o nome: nunca um caminho para fora da pasta.
            file: Path::new(&get("arquivo"))
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            original: get("original"),
            label: get("nome"),
            when: get("quando"),
        };
        if entry.written() {
            out.push(entry);
        } else if e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|d| d > STALE))
        {
            entry.remove();
        }
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// O que é preciso para gravar a planilha, sem segurar o documento.
struct Snapshot {
    sheets: Vec<Option<Arc<Sheet>>>,
    strings: Arc<StringTable>,
    styles: Arc<Styles>,
    path: PathBuf,
    /// Extensão da cópia.
    ext: String,
    opts: SaveOptions,
}

impl Snapshot {
    fn new(doc: &Doc) -> Self {
        let mut ext = doc.path.extension().and_then(|e| e.to_str()).unwrap_or("xlsx").to_ascii_lowercase();
        if ext == "xlsb" {
            let macros = doc.book().is_ok_and(|b| b.read_part("xl/vbaProject.bin").is_some());
            ext = if macros { "xlsm" } else { "xlsx" }.into();
        }
        Self {
            ext,
            sheets: doc.sheets.iter().map(|s| if s.preview { None } else { s.data.clone() }).collect(),
            strings: doc.strings.clone(),
            styles: doc.styles.clone(),
            path: doc.path.clone(),
            opts: SaveOptions {
                // Rápido: é uma cópia de passagem.
                level: 1,
                verify: false,
                tabs: Some(crate::tabs::plan(doc)),
                book_protection: doc.protection_changed.then(|| doc.protection.clone()),
                source: doc.source.clone(),
                ..Default::default()
            },
        }
    }

    fn write(&self, dest: &Path) -> Result<(), String> {
        if crate::loader::is_csv(&self.path) {
            let fmt =
                std::fs::read(&self.path).map(|b| duck_xlsx::csv::detect_format(&b)).unwrap_or_default();
            let sheet = self.sheets.first().and_then(|s| s.as_deref()).ok_or("sem dados")?;
            return duck_xlsx::csv::write_csv(dest, sheet, &self.strings, fmt).map_err(|e| e.to_string());
        }
        let refs: Vec<Option<&Sheet>> = self.sheets.iter().map(|s| s.as_deref()).collect();
        duck_xlsx::save_file(&self.path, dest, &refs, &self.strings, &self.styles, &self.opts)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Uma cópia sendo gravada. Vive em [`crate::calc::Calc`], que é parado antes
/// de qualquer edição (as abas não podem estar em uso).
pub struct Job {
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Job {
    /// Desiste e espera a thread soltar as abas (milissegundos).
    pub fn stop(self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

/// Estado das cópias nesta execução.
#[derive(Default)]
pub struct Recovery {
    dir: Option<PathBuf>,
    /// A cópia da planilha aberta.
    mine: Option<Entry>,
    /// A cópia de onde a planilha aberta foi recuperada.
    pub adopted: Option<Entry>,
    /// As oferecidas na abertura.
    pub found: Vec<Entry>,
    /// [`Doc::changes`] da última cópia, e da que está sendo gravada.
    saved: u64,
    saving: u64,
    result: Option<Receiver<Result<(), String>>>,
    /// Quando o despertador toca (`None` = desligado).
    alarm: Option<Instant>,
    last_input: Option<Instant>,
    /// Um erro ao gravar a cópia já foi avisado.
    warned: bool,
}

impl Recovery {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, ..Default::default() }
    }

    /// Houve uma tecla ou um clique.
    pub fn input(&mut self) {
        self.last_input = Some(Instant::now());
    }

    /// A planilha aberta veio desta cópia?
    pub fn is_adopted(&self, path: &Path) -> bool {
        self.adopted.as_ref().is_some_and(|e| e.path() == path)
    }

    /// A planilha foi salva ou descartada: as cópias dela não servem mais.
    pub fn forget(&mut self) {
        self.alarm = None;
        self.saved = 0;
        self.result = None;
        for e in [self.mine.take(), self.adopted.take()].into_iter().flatten() {
            e.remove();
        }
    }

    /// Salva: o que havia até `changes` está no arquivo.
    pub fn saved(&mut self, changes: u64) {
        self.forget();
        self.saved = changes;
    }

    /// A cópia (criada se preciso), com as informações de agora.
    fn entry(&mut self, doc: &Doc, untitled: bool, label: &str) -> Result<&mut Entry, String> {
        let dir = self.dir.clone().ok_or("sem pasta")?;
        if self.mine.is_none() {
            self.mine = Some(Entry::create(&dir).map_err(|e| e.to_string())?);
        }
        let original = match &self.adopted {
            Some(a) => a.original.clone(),
            None if untitled => String::new(),
            None => doc.path.to_string_lossy().into_owned(),
        };
        let label = self.adopted.as_ref().map_or(label, |a| a.label.as_str()).to_string();
        let e = self.mine.as_mut().ok_or("sem cópia")?;
        e.original = original;
        e.label = label;
        let t = duck_platform::local_time();
        e.when = format!("{:02}/{:02}/{} às {:02}:{:02}", t.day, t.month, t.year, t.hour, t.minute);
        Ok(e)
    }
}

impl SheetsApp {
    fn unsaved(&self) -> bool {
        self.recovery.dir.is_some()
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.dirty && d.finished.is_some() && d.changes != self.recovery.saved)
    }

    /// Liga o despertador, se há algo para guardar. Parado, o app continua
    /// sem gastar nada.
    pub fn recovery_arm(&mut self) {
        if self.recovery.alarm.is_none() && self.recovery.result.is_none() && self.unsaved() {
            self.recovery_alarm(EVERY);
        }
    }

    fn recovery_alarm(&mut self, after: Duration) {
        let Some(waker) = self.waker.clone() else { return };
        self.recovery.alarm = Some(Instant::now() + after);
        let _ = std::thread::Builder::new().name("duck-recuperar".into()).stack_size(64 * 1024).spawn(
            move || {
                std::thread::sleep(after + Duration::from_millis(5));
                waker.wake();
            },
        );
    }

    /// Acordou: vê se a cópia terminou e se está na hora de outra.
    pub fn recovery_poll(&mut self) {
        match self.recovery.result.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(r)) => {
                self.recovery.result = None;
                if let Some(doc) = &mut self.doc
                    && let Some(job) = doc.calc.snapshot.take()
                {
                    let _ = job.handle.join();
                }
                self.recovery_done(r);
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                // Parada no meio por uma edição: tenta de novo mais tarde.
                self.recovery.result = None;
                if let Some(doc) = &mut self.doc {
                    doc.calc.stop_snapshot();
                }
            }
            _ => {}
        }
        match self.recovery.alarm {
            Some(t) if t <= Instant::now() => self.recovery.alarm = None,
            Some(_) => return,
            None => {
                self.recovery_arm();
                return;
            }
        }
        if !self.unsaved() {
            return;
        }
        let busy = self.saving.is_some()
            || self.drag.is_some()
            || self.recovery.result.is_some()
            || self.recovery.last_input.is_some_and(|t| t.elapsed() < IDLE)
            || self.doc.as_ref().is_some_and(|d| d.calc.running());
        if busy {
            self.recovery_alarm(IDLE);
        } else {
            self.recovery_start();
        }
    }

    fn recovery_done(&mut self, r: Result<(), String>) {
        match r {
            Ok(()) => {
                self.recovery.saved = self.recovery.saving;
                if let Some(e) = self.recovery.mine.as_mut()
                    && let Err(e) = e.write_info()
                {
                    self.recovery_failed(&e.to_string());
                }
            }
            Err(e) => self.recovery_failed(&e),
        }
    }

    fn recovery_failed(&mut self, e: &str) {
        // A versão de agora não volta a ser tentada; a próxima alteração, sim.
        self.recovery.saved = self.recovery.saving;
        if !std::mem::replace(&mut self.recovery.warned, true) {
            self.message = format!("Não foi possível guardar a cópia de recuperação: {e}");
        }
    }

    /// Prepara a cópia: o que gravar e onde.
    fn recovery_prepare(&mut self) -> Result<(Snapshot, PathBuf, u64), String> {
        let label = self.doc.as_ref().map(|d| self.file_label(&d.path)).unwrap_or_default();
        let untitled = self.doc.as_ref().is_some_and(|d| self.untitled_path.as_ref() == Some(&d.path));
        let doc = self.doc.as_ref().ok_or("sem planilha")?;
        let snap = Snapshot::new(doc);
        let changes = doc.changes;
        let e = self.recovery.entry(doc, untitled, &label)?;
        if e.file.is_empty() {
            e.file = format!("{}.{}", e.id, snap.ext);
            // Antes da planilha: quebrando entre as duas, ela é encontrada.
            e.write_info().map_err(|e| e.to_string())?;
        }
        Ok((snap, e.path(), changes))
    }

    fn recovery_start(&mut self) {
        let Some(waker) = self.waker.clone() else { return };
        self.recovery_spawn(move || waker.wake());
    }

    /// Grava a cópia numa thread; `wake` avisa a janela.
    fn recovery_spawn(&mut self, wake: impl Fn() + Send + 'static) {
        let (mut snap, dest, changes) = match self.recovery_prepare() {
            Ok(x) => x,
            Err(e) => return self.recovery_failed(&e),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        snap.opts.cancel = Some(cancel.clone());
        let (tx, rx) = channel();
        let c = cancel.clone();
        let handle = std::thread::Builder::new().name("duck-recuperar".into()).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                duck_par::pool(0).install(|| snap.write(&dest))
            }))
            .unwrap_or_else(|_| Err("erro interno".into()));
            // As abas são soltas antes de avisar.
            drop(snap);
            if !c.load(Ordering::Relaxed) {
                let _ = tx.send(r);
            }
            drop(tx);
            wake();
        });
        let Ok(handle) = handle else { return };
        self.recovery.saving = changes;
        self.recovery.result = Some(rx);
        if let Some(doc) = &mut self.doc {
            doc.calc.snapshot = Some(Job { cancel, handle });
        }
    }

    /// O programa quebrou: uma última cópia, na própria thread. `true` = o que
    /// não estava salvo está guardado.
    pub fn recovery_emergency(&mut self) -> bool {
        let had = self.recovery.mine.as_ref().is_some_and(Entry::written);
        if !self.unsaved() {
            return had || self.recovery.adopted.is_some();
        }
        if let Some(doc) = &mut self.doc {
            doc.calc.cancel();
        }
        let Ok((mut snap, dest, _)) = self.recovery_prepare() else { return had };
        // O estado pode estar pela metade: só substitui a cópia anterior se a
        // nova conferir.
        snap.opts.verify = had;
        match snap.write(&dest) {
            Ok(()) => self.recovery.mine.as_ref().is_some_and(|e| e.write_info().is_ok()),
            Err(_) => had,
        }
    }

    /// Na abertura: oferece as cópias que sobraram.
    pub fn recovery_offer(&mut self) {
        let Some(dir) = &self.recovery.dir else { return };
        self.recovery.found = pending(dir);
        self.recovery_ask();
    }

    fn recovery_ask(&mut self) {
        let found = &self.recovery.found;
        let Some(first) = found.first() else { return };
        let name = |e: &Entry| if e.label.is_empty() { "Planilha".to_string() } else { e.label.clone() };
        let fields = if found.len() == 1 {
            vec![Field::info(&format!(
                "O duck-sheets fechou sem salvar \"{}\". Há uma cópia de {}, com as alterações até ali.",
                name(first),
                first.when
            ))]
        } else {
            let list: Vec<String> = found.iter().map(|e| format!("{} · {}", name(e), e.when)).collect();
            let list: Vec<&str> = list.iter().map(String::as_str).collect();
            vec![
                Field::info("O duck-sheets fechou sem salvar estas planilhas. Escolha uma cópia:"),
                Field::choice("Cópia", &list, 0),
            ]
        };
        let buttons = ["Recuperar", "Descartar", "Depois"];
        self.dialog = Some(Dialog::new(Kind::Recover, "Recuperar planilha", fields, &buttons));
    }

    /// Resposta à oferta (`None` = Esc: fica para a próxima abertura).
    pub fn recovery_answer(&mut self, d: &Dialog, button: Option<usize>) {
        let i = d.choice_of(1);
        if i >= self.recovery.found.len() {
            return;
        }
        match button {
            Some(0) => {
                let Some(waker) = self.waker.clone() else { return };
                let e = self.recovery.found.remove(i);
                let path = e.path();
                self.open(path.clone(), waker);
                // Trocar de planilha pode ter sido recusado (há alterações nela).
                if self.loader.as_ref().is_some_and(|l| l.path == path) {
                    self.recovery.adopted = Some(e);
                }
                // As outras ficam para a próxima abertura.
                self.recovery.found.clear();
            }
            Some(1) => {
                self.recovery.found.remove(i).remove();
                self.recovery_ask();
            }
            _ => self.recovery.found.clear(),
        }
    }

    /// A planilha recuperada terminou de carregar: ainda não está salva.
    pub fn recovery_loaded(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        if !self.recovery.is_adopted(&doc.path) {
            return;
        }
        doc.dirty = true;
        self.recovery.saved = doc.changes;
        self.message = "Planilha recuperada: salve para não perder".into();
        self.update_title();
    }
}

/// Arquivo com as falhas do programa.
pub fn log_file() -> Option<PathBuf> {
    home().map(|d| d.join("erros.txt"))
}

/// Registra as falhas (`panic`) num arquivo, além do que o Rust já mostra. Só
/// neste computador: nada é enviado.
pub fn install_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let Some(file) = log_file() else { return };
        let what: String = info.payload_as_str().unwrap_or("(sem mensagem)").chars().take(300).collect();
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current();
        let t = duck_platform::local_time();
        let text = format!(
            "{:02}/{:02}/{} {:02}:{:02}:{:02} duck-sheets {} ({} {})\n{what}\nem {at}, thread {}\n{}\n\n",
            t.day,
            t.month,
            t.year,
            t.hour,
            t.minute,
            t.second,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture()
        );
        let _ = append(&file, &text);
    }));
}

fn append(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = file.parent() {
        private_dir(dir)?;
    }
    if std::fs::metadata(file).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::rename(file, file.with_file_name("erros-anterior.txt"));
    }
    File::options().create(true).append(true).open(file)?.write_all(text.as_bytes())
}

/// Pasta das cópias.
pub fn dir() -> Option<PathBuf> {
    home().map(|d| d.join("recuperacao"))
}

/// O arquivo de origem de uma planilha recuperada, para a sugestão de onde
/// salvar: pasta e nome.
pub fn suggestion(e: &Entry) -> (Option<PathBuf>, String) {
    let ext = e.path().extension().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let base = e.original().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(e.label()));
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty());
    let name = format!("{}.{ext}", stem.unwrap_or_else(|| "Planilha".into()));
    (e.original().and_then(Path::parent).filter(|d| d.is_dir()).map(Path::to_path_buf), name)
}

/// A origem de uma planilha, para ler o original mesmo que ele saia do lugar.
pub fn reopen(path: &Path) -> Option<Arc<Source>> {
    Source::open(path).ok().map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, get, type_in};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("duck-recuperar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Um app com uma planilha em branco de verdade, no disco.
    fn on_disk(dir: &Path) -> SheetsApp {
        let path = dir.join("Contas.xlsx");
        duck_xlsx::write::write_blank(&path).unwrap();
        let mut a = app(&[("Planilha1", &[])]);
        let (wb, _) = duck_xlsx::open(&path, &Default::default()).unwrap();
        let book = duck_xlsx::Book::open(&path).unwrap();
        let doc = a.doc.as_mut().unwrap();
        doc.styles = Arc::new(wb.styles);
        doc.sheets[0].info = book.sheets()[0].clone();
        doc.sheets[0].origin = duck_xlsx::TabOrigin::File(book.sheets()[0].part.clone());
        doc.source = reopen(&path);
        doc.path = path;
        a.recovery = Recovery::new(Some(dir.join("copias")));
        a
    }

    #[test]
    fn crash_and_recover() {
        let d = temp("quebra");
        let mut a = on_disk(&d);
        assert!(!a.recovery_emergency(), "sem alterações não há o que guardar");
        assert!(pending(&d.join("copias")).is_empty());
        type_in(&mut a, "B2", "não salvo");
        assert!(a.recovery_emergency());
        // Enquanto este processo vive, a cópia tem dono.
        assert!(pending(&d.join("copias")).is_empty());
        // Quebrou: a trava some, a cópia fica (nada chama `forget`).
        drop(a);
        let found = pending(&d.join("copias"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label(), "Contas.xlsx");
        assert_eq!(found[0].original(), Some(d.join("Contas.xlsx").as_path()));
        assert_eq!(suggestion(&found[0]), (Some(d.clone()), "Contas.xlsx".into()));
        let (wb, _) = duck_xlsx::open(found[0].path(), &Default::default()).unwrap();
        let v = wb.sheets[0].cell(1, 1, &wb.shared_strings).value;
        assert!(matches!(v, duck_core::Value::Text("não salvo")), "{v:?}");
        // O original não foi tocado.
        let (wb, _) = duck_xlsx::open(d.join("Contas.xlsx"), &Default::default()).unwrap();
        assert!(matches!(wb.sheets[0].cell(1, 1, &wb.shared_strings).value, duck_core::Value::Empty));
        // Descartar apaga tudo.
        let mut b = app(&[("Planilha1", &[])]);
        drop(found);
        b.recovery = Recovery::new(Some(d.join("copias")));
        b.recovery_offer();
        assert!(matches!(b.dialog.as_ref().unwrap().kind, Kind::Recover));
        b.dialog_event(crate::dialog::Outcome::Button(1));
        assert!(b.dialog.is_none());
        assert_eq!(std::fs::read_dir(d.join("copias")).unwrap().count(), 0);
        drop(b);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A cópia sai em segundo plano, e uma edição no meio não espera por ela
    /// nem é recusada.
    #[test]
    fn in_the_background() {
        let d = temp("fundo");
        let mut a = on_disk(&d);
        type_in(&mut a, "A1", "um");
        a.recovery_spawn(|| ());
        // Edita com a gravação em andamento (ou já terminada).
        type_in(&mut a, "A2", "dois");
        assert_eq!(get(&a, 0, "A2"), "dois");
        for _ in 0..500 {
            a.recovery_poll();
            if a.recovery.result.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(a.recovery.result.is_none());
        assert!(a.unsaved(), "a segunda edição ainda não está na cópia");
        a.recovery_spawn(|| ());
        for _ in 0..500 {
            std::thread::sleep(Duration::from_millis(10));
            a.recovery_poll();
            if !a.unsaved() {
                break;
            }
        }
        assert!(!a.unsaved());
        assert!(a.doc.as_ref().unwrap().dirty, "a cópia não é salvar");
        assert!(a.doc.as_ref().unwrap().calc.snapshot.is_none());
        let path = a.recovery.mine.as_ref().unwrap().path();
        drop(a);
        let (wb, _) = duck_xlsx::open(path, &Default::default()).unwrap();
        assert!(matches!(wb.sheets[0].cell(1, 0, &wb.shared_strings).value, duck_core::Value::Text("dois")));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn saving_or_closing_removes_the_copy() {
        let d = temp("salva");
        let mut a = on_disk(&d);
        type_in(&mut a, "A1", "1");
        assert!(a.recovery_emergency());
        assert!(a.unsaved());
        let n = a.doc.as_ref().unwrap().changes;
        a.recovery.saved(n);
        assert!(!a.unsaved());
        assert_eq!(std::fs::read_dir(d.join("copias")).unwrap().count(), 0);
        type_in(&mut a, "A1", "2");
        assert!(a.unsaved());
        assert!(a.recovery_emergency());
        a.cleanup();
        assert_eq!(std::fs::read_dir(d.join("copias")).unwrap().count(), 0);
        drop(a);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Duas cópias: escolhe, e "Depois" deixa todas.
    #[test]
    fn several_copies() {
        let d = temp("varias");
        for text in ["um", "dois"] {
            let mut a = on_disk(&d);
            type_in(&mut a, "A1", text);
            assert!(a.recovery_emergency());
            drop(a);
            std::thread::sleep(Duration::from_millis(3));
        }
        let mut b = app(&[("Planilha1", &[])]);
        b.recovery = Recovery::new(Some(d.join("copias")));
        b.recovery_offer();
        assert_eq!(b.recovery.found.len(), 2);
        b.dialog_event(crate::dialog::Outcome::Cancel);
        assert!(b.dialog.is_none() && b.recovery.found.is_empty());
        let left = pending(&d.join("copias"));
        assert_eq!(left.len(), 2);
        // A mais nova primeiro.
        let (wb, _) = duck_xlsx::open(left[0].path(), &Default::default()).unwrap();
        assert!(matches!(wb.sheets[0].cell(0, 0, &wb.shared_strings).value, duck_core::Value::Text("dois")));
        drop((left, b));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Outro programa muda o arquivo: salvar pergunta, e nada é sobrescrito.
    #[test]
    fn changed_by_another_program() {
        let d = temp("mudou");
        let mut a = on_disk(&d);
        a.opts.save_to = None;
        let path = a.doc.as_ref().unwrap().path.clone();
        type_in(&mut a, "A1", "meu");
        std::fs::write(d.join("outro"), b"do outro programa").unwrap();
        std::fs::rename(d.join("outro"), &path).unwrap();
        a.save(false);
        assert!(a.saving.is_none());
        assert!(matches!(a.dialog.as_ref().unwrap().kind, Kind::Changed(_)));
        a.dialog_event(crate::dialog::Outcome::Cancel);
        assert_eq!(std::fs::read(&path).unwrap(), b"do outro programa");
        assert!(a.doc.as_ref().unwrap().dirty);
        // A cópia de recuperação ainda sai do original.
        assert!(a.recovery_emergency());
        assert_eq!(get(&a, 0, "A1"), "meu");
        drop(a);
        let found = pending(&d.join("copias"));
        let (wb, _) = duck_xlsx::open(found[0].path(), &Default::default()).unwrap();
        assert!(matches!(wb.sheets[0].cell(0, 0, &wb.shared_strings).value, duck_core::Value::Text("meu")));
        drop(found);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
