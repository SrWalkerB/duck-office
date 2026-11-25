//! O arquivo aberto, como estava na abertura.
//!
//! Salvar por remendo (ADR-014) copia do original tudo o que não mudou, e as
//! dinâmicas, imagens e macros são lidas dele sob demanda. Se outro programa
//! trocar o arquivo no meio do caminho, o resultado seria uma mistura dos
//! dois. Por isso o app segura o arquivo aberto: quem salva trocando o arquivo
//! (renomeando um temporário por cima, como quase todos) deixa o de antes
//! legível por aqui, e dá para avisar antes de salvar por cima.
//!
//! No Windows os outros programas também não conseguem escrever dentro do
//! arquivo enquanto ele está aberto, como acontece com o Excel.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::{Book, Error, Package};

/// Tamanho e data: o bastante para saber que o arquivo não é mais o mesmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(m: &std::fs::Metadata) -> Self {
        Self { len: m.len(), modified: m.modified().ok() }
    }
}

/// O que está no disco, comparado com o que foi aberto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDisk {
    Same,
    Changed,
    Missing,
}

#[derive(Debug)]
pub struct Source {
    path: PathBuf,
    /// `None` só enquanto o arquivo é trocado onde isso exige fechá-lo.
    file: Mutex<Option<File>>,
    stamp: Stamp,
}

const REWRITTEN: &str = "Outro programa regravou o arquivo enquanto ele estava aberto aqui, e o duck-sheets precisa do original para salvar. Copie o que mudou para outra planilha e abra o arquivo de novo.";

impl Source {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = open_shared(path)?;
        let stamp = Stamp::of(&file.metadata()?);
        Ok(Self { path: path.to_path_buf(), file: Mutex::new(Some(file)), stamp })
    }

    /// O mesmo arquivo, renomeado para `path`.
    pub(crate) fn moved(mut self, path: &Path) -> Self {
        self.path = path.to_path_buf();
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Como está agora o arquivo nesse caminho.
    pub fn on_disk(&self) -> OnDisk {
        match std::fs::metadata(&self.path) {
            Ok(m) if Stamp::of(&m) == self.stamp => OnDisk::Same,
            Err(e) if e.kind() == io::ErrorKind::NotFound => OnDisk::Missing,
            // Sem conseguir olhar (rede fora do ar), quem dirá é o salvamento.
            Err(_) => OnDisk::Same,
            Ok(_) => OnDisk::Changed,
        }
    }

    /// `dest` é este mesmo arquivo?
    pub fn is(&self, dest: &Path) -> bool {
        let full = |p: &Path| p.canonicalize().ok();
        dest == self.path || full(dest).is_some_and(|d| full(&self.path) == Some(d))
    }

    /// O conteúdo de quando foi aberto.
    pub fn package(&self) -> Result<Package, Error> {
        let guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let Some(file) = guard.as_ref() else { return Err(Error::Unsupported(REWRITTEN)) };
        if Stamp::of(&file.metadata()?) != self.stamp {
            return Err(Error::Unsupported(REWRITTEN));
        }
        Package::from_file(file)
    }

    pub fn book(&self) -> Result<Book, Error> {
        Book::from_package(self.package()?)
    }

    /// Troca `dest` por `tmp`. Onde não dá para trocar um arquivo aberto
    /// (algumas unidades de rede), fecha este e tenta de novo.
    pub(crate) fn replace(&self, tmp: &Path, dest: &Path) -> io::Result<()> {
        let first = match std::fs::rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(e) if !self.is(dest) => return Err(e),
            Err(e) => e,
        };
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
        let out = std::fs::rename(tmp, dest);
        if out.is_err() {
            // Continua com o de antes, se ele ainda é o mesmo.
            *guard = open_shared(&self.path)
                .ok()
                .filter(|f| f.metadata().is_ok_and(|m| Stamp::of(&m) == self.stamp));
        }
        out.map_err(|_| first)
    }
}

/// Abre para leitura deixando os outros lerem e trocarem o arquivo, mas (no
/// Windows) não escreverem dentro dele. Se alguém já o tem aberto para
/// escrita (o Excel), abre sem essa garantia.
fn open_shared(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_DELETE.
        if let Ok(f) = OpenOptions::new().read(true).share_mode(1 | 4).open(path) {
            return Ok(f);
        }
    }
    OpenOptions::new().read(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("duck-source-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Outro programa troca o arquivo: dá para saber, e o de antes continua
    /// legível.
    #[test]
    fn replaced_by_another_program() {
        let d = dir("troca");
        let p = d.join("a.xlsx");
        crate::write::write_blank(&p).unwrap();
        let s = Source::open(&p).unwrap();
        assert_eq!(s.on_disk(), OnDisk::Same);
        assert_eq!(s.book().unwrap().sheets().len(), 1);
        std::fs::write(d.join("b"), b"outro conteudo, que nem zip e").unwrap();
        std::fs::rename(d.join("b"), &p).unwrap();
        assert_eq!(s.on_disk(), OnDisk::Changed);
        assert_eq!(s.book().unwrap().sheets().len(), 1, "o de antes");
        std::fs::remove_file(&p).unwrap();
        assert_eq!(s.on_disk(), OnDisk::Missing);
        assert_eq!(s.book().unwrap().sheets().len(), 1);
        drop(s);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Escrito por dentro: no Windows não deixa; nos outros, o original se
    /// perdeu e nada é lido pela metade.
    #[test]
    fn rewritten_in_place() {
        let d = dir("dentro");
        let p = d.join("a.xlsx");
        crate::write::write_blank(&p).unwrap();
        let s = Source::open(&p).unwrap();
        let w = std::fs::write(&p, b"lixo");
        if cfg!(windows) {
            assert!(w.is_err());
            assert!(s.book().is_ok());
        } else {
            assert_eq!(s.on_disk(), OnDisk::Changed);
            assert!(s.book().is_err());
        }
        drop(s);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Salvar por cima do próprio arquivo, com ele aberto, e não salvar se
    /// outro programa o trocou.
    #[test]
    fn save_over_itself() {
        use std::sync::Arc;
        let d = dir("salvar");
        let p = d.join("a.xlsx");
        crate::write::write_blank(&p).unwrap();
        let (wb, _) = crate::open(&p, &Default::default()).unwrap();
        let s = Arc::new(Source::open(&p).unwrap());
        let sheets: Vec<Option<&duck_core::Sheet>> = wb.sheets.iter().map(Some).collect();
        let save = |s: &Arc<Source>, over: bool| {
            let opts = crate::SaveOptions {
                source: Some(s.clone()),
                overwrite: over,
                rewrite_all: true,
                ..Default::default()
            };
            crate::save_file(&p, &p, &sheets, &wb.shared_strings, &wb.styles, &opts)
        };
        let s2 = save(&s, false).unwrap().source.unwrap();
        assert_eq!(s2.on_disk(), OnDisk::Same);
        // O de antes sabe que mudou (o tamanho ou a data).
        crate::write::write_blank(&d.join("b.xlsx")).unwrap();
        let f = File::options().write(true).open(d.join("b.xlsx")).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000)).unwrap();
        drop(f);
        std::fs::rename(d.join("b.xlsx"), &p).unwrap();
        assert_eq!(s2.on_disk(), OnDisk::Changed);
        assert!(save(&s2, false).is_err());
        assert_eq!(s2.on_disk(), OnDisk::Changed, "não mexeu");
        assert!(save(&s2, true).is_ok());
        assert!(crate::open(&p, &Default::default()).is_ok());
        drop((s, s2));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
