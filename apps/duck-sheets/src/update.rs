//! Atualização automática.
//!
//! Uma thread olha de tempos em tempos um **manifesto** (um texto pequeno com a
//! versão mais nova e o tamanho e o SHA-256 de cada instalador), assinado com a
//! chave do projeto. Havendo versão mais nova, baixa o instalador em segundo
//! plano e confere; ele é aplicado quando a última janela fecha, sem interromper
//! ninguém. Nada que não tenha a assinatura é executado, venha de onde vier.
//!
//! Quem baixa é o `curl` do sistema (o Windows e o macOS trazem um), para não
//! carregar uma pilha de TLS. O pedido não leva nada de quem usa.
//!
//! O que dá para trocar sozinho: a instalação do Windows só para o usuário, o
//! `.app` do macOS e o AppImage do Linux. Nos outros casos (pacote do sistema,
//! versão portátil, instalação para todos) só avisa que há versão nova.

use std::cmp::Ordering;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as Atomic};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use sha2::Digest;

use crate::app::SheetsApp;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const REPO: &str = "https://github.com/SrWalkerB/duck-office";
const MANIFESTS: &str = "https://raw.githubusercontent.com/SrWalkerB/duck-office/atualizacoes";

/// Chave pública (minisign). Sem ela o programa não se atualiza. A compilação
/// pode trocá-la para ensaiar a atualização.
const KEY_FILE: &str = include_str!("../assets/atualizacao.pub");
const KEY_OVERRIDE: Option<&str> = option_env!("DUCK_UPDATE_PUBKEY");

/// Depois de abrir, e entre uma olhada e outra.
const FIRST: Duration = Duration::from_secs(8);
const EVERY: Duration = Duration::from_secs(6 * 3600);
/// Abrir o programa várias vezes seguidas não repete o pedido.
const FRESH: Duration = Duration::from_secs(3600);
const MANIFEST_MAX: u64 = 64 * 1024;
const FILE_MAX: u64 = 512 * 1024 * 1024;
/// Aplicar a mesma versão sem sucesso mais que isto: passa a só avisar.
const TRIES: u32 = 2;

// ---------------------------------------------------------------------------
// Versões e manifesto
// ---------------------------------------------------------------------------

/// Compara versões (`1.0.0-beta.2` < `1.0.0-beta.10` < `1.0.0`). `None` = não é
/// uma versão.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let parse = |v: &str| -> Option<([u64; 3], Vec<String>)> {
        let v = v.split('+').next()?;
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(str::to_string).collect()),
            None => (v, Vec::new()),
        };
        let n: Vec<u64> = core.split('.').map(|x| x.parse().ok()).collect::<Option<_>>()?;
        Some((n.try_into().ok()?, pre))
    };
    let ((na, pa), (nb, pb)) = (parse(a)?, parse(b)?);
    Some(na.cmp(&nb).then_with(|| match (pa.is_empty(), pb.is_empty()) {
        // Sem sufixo é a versão final, depois de todas as prévias.
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for (x, y) in pa.iter().zip(&pb) {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    _ => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
            pa.len().cmp(&pb.len())
        }
    }))
}

/// Um arquivo do manifesto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    assets: Vec<(String, Asset)>,
}

impl Manifest {
    /// ```text
    /// duck-sheets 1
    /// versao=1.0.0
    /// arquivo=windows-instalador|duck-sheets-1.0.0-….exe|123|<sha256>
    /// ```
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines().map(str::trim);
        if lines.next()? != "duck-sheets 1" {
            return None;
        }
        let mut version = String::new();
        let mut assets = Vec::new();
        for l in lines {
            match l.split_once('=') {
                Some(("versao", v)) => version = v.to_string(),
                Some(("arquivo", v)) => {
                    let p: Vec<&str> = v.split('|').collect();
                    let [kind, name, size, sha] = p[..] else { return None };
                    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
                    // O nome vira caminho e endereço: nada além disto.
                    let ok = !name.is_empty() && !name.starts_with('.') && name.chars().all(plain);
                    let hex = sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit());
                    if !ok || !hex {
                        return None;
                    }
                    let size = size.parse().ok().filter(|s| (1..=FILE_MAX).contains(s))?;
                    let asset = Asset { name: name.into(), size, sha256: sha.to_ascii_lowercase() };
                    assets.push((kind.to_string(), asset));
                }
                // Linhas novas não atrapalham as versões antigas.
                _ => {}
            }
        }
        compare(&version, "0.0.0")?;
        version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')).then_some(())?;
        Some(Self { version, assets })
    }

    fn asset(&self, kind: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.0 == kind).map(|a| &a.1)
    }

    fn newer(&self) -> bool {
        compare(&self.version, VERSION) == Some(Ordering::Greater)
    }
}

/// A linha da chave (a outra é um comentário).
fn key_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|l| l.len() > 40 && !l.contains(' '))
}

fn key() -> Option<&'static str> {
    KEY_OVERRIDE.filter(|k| !k.is_empty()).or_else(|| key_line(KEY_FILE))
}

fn verify(content: &[u8], signature: &str, key: &str) -> Result<(), String> {
    let bad = |e: minisign_verify::Error| format!("assinatura da atualização: {e}");
    let key = minisign_verify::PublicKey::from_base64(key).map_err(bad)?;
    let sig = minisign_verify::Signature::decode(signature).map_err(bad)?;
    key.verify(content, &sig, false).map_err(bad)
}

fn intact(file: &Path, a: &Asset) -> bool {
    if std::fs::metadata(file).map(|m| m.len()).ok() != Some(a.size) {
        return false;
    }
    let Ok(mut f) = File::open(file) else { return false };
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0; 1 << 20];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(_) => return false,
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>() == a.sha256
}

// ---------------------------------------------------------------------------
// Como este programa foi instalado
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// Instalador do Windows, só para o usuário (a pasta).
    Windows(PathBuf),
    /// O `.app` do macOS.
    MacApp(PathBuf),
    AppImage(PathBuf),
    /// Quem atualiza é outro (o sistema, o administrador ou a pessoa).
    Other,
}

impl Install {
    pub fn detect() -> Self {
        let Ok(exe) = std::env::current_exe() else { return Self::Other };
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
        Self::of(&exe, appimage.as_deref())
    }

    fn of(exe: &Path, appimage: Option<&Path>) -> Self {
        let dir = exe.parent().unwrap_or(Path::new(""));
        if cfg!(windows) {
            if dir.join("unins000.exe").is_file() && writable(dir) {
                return Self::Windows(dir.to_path_buf());
            }
        } else if cfg!(target_os = "macos") {
            // X.app/Contents/MacOS/duck-sheets
            let app = exe.ancestors().nth(3).filter(|a| a.extension().is_some_and(|e| e == "app"));
            let temporary = |a: &Path| {
                a.starts_with("/Volumes") || a.components().any(|c| c.as_os_str() == "AppTranslocation")
            };
            if let Some(a) = app
                && !temporary(a)
                && a.parent().is_some_and(writable)
            {
                return Self::MacApp(a.to_path_buf());
            }
        } else if let Some(a) = appimage
            && a.is_file()
            && a.parent().is_some_and(writable)
        {
            return Self::AppImage(a.to_path_buf());
        }
        Self::Other
    }

    /// Nome no manifesto do arquivo que atualiza esta instalação.
    fn kind(&self) -> Option<&'static str> {
        match self {
            Self::Windows(_) => Some("windows-instalador"),
            Self::MacApp(_) => Some("macos-app"),
            Self::AppImage(_) => Some("linux-appimage"),
            Self::Other => None,
        }
    }

    /// Troca esta instalação por `file`. No Windows só começa: o instalador
    /// espera o programa fechar. `reopen` = abrir de novo no fim.
    fn apply(&self, file: &Path, reopen: bool) -> Result<(), String> {
        let fail = |e: std::io::Error| e.to_string();
        match self {
            Self::Windows(dir) => {
                let mut c = Command::new(file);
                c.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER"])
                    // Outra janela aberta nunca é fechada à força.
                    .args(["/NOCLOSEAPPLICATIONS", "/NOCANCEL"])
                    .arg(format!("/DIR={}", dir.display()));
                if let Some(d) = file.parent() {
                    c.arg(format!("/LOG={}", d.join("instalador.log").display())).current_dir(d);
                }
                if reopen {
                    c.arg("/reabrir=1");
                }
                c.spawn().map(|_| ()).map_err(fail)
            }
            Self::MacApp(app) => {
                let parent = app.parent().ok_or("sem pasta")?;
                let temp = parent.join(format!(".duck-sheets-{}", std::process::id()));
                let swap = || -> Result<(), String> {
                    std::fs::create_dir_all(&temp).map_err(fail)?;
                    run(Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(file).arg(&temp))?;
                    let new = std::fs::read_dir(&temp)
                        .map_err(fail)?
                        .flatten()
                        .map(|e| e.path())
                        .find(|p| p.extension().is_some_and(|e| e == "app"))
                        .ok_or("o pacote não tem um .app")?;
                    run(Command::new("/usr/bin/codesign").args(["--verify", "--strict"]).arg(&new))?;
                    let old = temp.join("anterior");
                    std::fs::rename(app, &old).map_err(fail)?;
                    if let Err(e) = std::fs::rename(&new, app) {
                        let _ = std::fs::rename(&old, app);
                        return Err(e.to_string());
                    }
                    Ok(())
                };
                let r = swap();
                let _ = std::fs::remove_dir_all(&temp);
                r?;
                if reopen {
                    run(Command::new("/usr/bin/open").arg("-n").arg(app))?;
                }
                Ok(())
            }
            Self::AppImage(at) => {
                let name = at.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let temp = at.with_file_name(format!(".{name}.novo"));
                let swap = || -> std::io::Result<()> {
                    std::fs::copy(file, &temp)?;
                    #[cfg(unix)]
                    std::fs::set_permissions(&temp, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
                    File::open(&temp)?.sync_all()?;
                    std::fs::rename(&temp, at)
                };
                if let Err(e) = swap() {
                    let _ = std::fs::remove_file(&temp);
                    return Err(e.to_string());
                }
                if reopen {
                    Command::new(at).spawn().map_err(fail)?;
                }
                Ok(())
            }
            Self::Other => Err("esta instalação não se atualiza sozinha".into()),
        }
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".duck-sheets-teste-{}", std::process::id()));
    let ok = File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn run(c: &mut Command) -> Result<(), String> {
    let out = c.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if e.is_empty() { format!("{}", out.status) } else { e })
}

// ---------------------------------------------------------------------------
// Baixar e conferir
// ---------------------------------------------------------------------------

/// De onde baixar e onde guardar.
#[derive(Clone)]
struct Place {
    dir: PathBuf,
    /// Endereço de ensaio (`DUCK_ATUALIZACAO`): manifesto e arquivos juntos. A
    /// assinatura vale do mesmo jeito.
    base: Option<String>,
    key: &'static str,
}

impl Place {
    fn new() -> Option<Self> {
        let base = std::env::var("DUCK_ATUALIZACAO").ok();
        Some(Self { dir: crate::recovery::home()?.join("atualizacao"), base, key: key()? })
    }

    fn channel() -> &'static str {
        if VERSION.contains('-') { "beta" } else { "estavel" }
    }

    fn manifest_url(&self) -> String {
        format!("{}/{}.txt", self.base.as_deref().unwrap_or(MANIFESTS), Self::channel())
    }

    fn asset_url(&self, m: &Manifest, a: &Asset) -> String {
        match &self.base {
            Some(b) => format!("{b}/{}", a.name),
            None => format!("{REPO}/releases/download/v{}/{}", m.version, a.name),
        }
    }

    fn fetch(&self, url: &str, out: &Path, max: u64) -> Result<(), String> {
        let curl = if cfg!(windows) {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
            PathBuf::from(root).join(r"System32\curl.exe")
        } else if cfg!(target_os = "macos") {
            "/usr/bin/curl".into()
        } else {
            "curl".into()
        };
        let proto = if self.base.is_some() { "=https,http,file" } else { "=https" };
        let mut c = Command::new(curl);
        // Sem o `.curlrc` de quem usa.
        c.args(["-q", "-fsSL", "--proto", proto, "--proto-redir", proto, "--max-redirs", "5"])
            .args(["--connect-timeout", "15", "--max-time", "1800", "--retry", "2"])
            .args(["-A", "duck-sheets", "--max-filesize", &max.to_string()]);
        #[cfg(windows)]
        {
            // Sem abrir um terminal na tela.
            std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0800_0000);
            c.arg("--ssl-revoke-best-effort");
        }
        run(c.arg("-o").arg(out).arg("--").arg(url))
    }

    fn read(&self, file: &Path, sig: &Path) -> Result<Manifest, String> {
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        let s = std::fs::read_to_string(sig).map_err(|e| e.to_string())?;
        verify(text.as_bytes(), &s, self.key)?;
        Manifest::parse(&text).ok_or_else(|| "manifesto da atualização inválido".into())
    }

    /// Onde o manifesto e a assinatura dele ficam guardados.
    fn stored(&self) -> (PathBuf, PathBuf) {
        let c = Self::channel();
        (self.dir.join(format!("{c}.txt")), self.dir.join(format!("{c}.txt.minisig")))
    }

    /// O manifesto guardado (a assinatura é conferida sempre).
    fn cached(&self) -> Result<Manifest, String> {
        let (file, sig) = self.stored();
        self.read(&file, &sig)
    }

    /// O manifesto assinado: o guardado, se é recente, ou o da rede.
    fn manifest(&self, force: bool) -> Result<Manifest, String> {
        let (file, sig) = self.stored();
        let recent = std::fs::metadata(&file)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|d| d < FRESH));
        if !force
            && recent
            && let Ok(m) = self.cached()
        {
            return Ok(m);
        }
        crate::recovery::private_dir(&self.dir).map_err(|e| e.to_string())?;
        let pid = std::process::id();
        let (f, s) = (self.dir.join(format!("manifesto-{pid}")), self.dir.join(format!("assinatura-{pid}")));
        let url = self.manifest_url();
        let get = || -> Result<Manifest, String> {
            self.fetch(&url, &f, MANIFEST_MAX)?;
            self.fetch(&format!("{url}.minisig"), &s, MANIFEST_MAX)?;
            let m = self.read(&f, &s)?;
            std::fs::rename(&s, &sig).map_err(|e| e.to_string())?;
            std::fs::rename(&f, &file).map_err(|e| e.to_string())?;
            Ok(m)
        };
        let r = get();
        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_file(&s);
        r
    }

    /// O instalador, baixado e conferido.
    fn download(&self, m: &Manifest, a: &Asset) -> Result<PathBuf, String> {
        let file = self.dir.join(&a.name);
        if intact(&file, a) {
            return Ok(file);
        }
        crate::recovery::private_dir(&self.dir).map_err(|e| e.to_string())?;
        let part = self.dir.join(format!("{}.{}", a.name, std::process::id()));
        let get = || -> Result<(), String> {
            self.fetch(&self.asset_url(m, a), &part, a.size)?;
            if !intact(&part, a) {
                return Err("o arquivo baixado não confere com o manifesto".into());
            }
            std::fs::rename(&part, &file).map_err(|e| e.to_string())
        };
        let r = get();
        let _ = std::fs::remove_file(&part);
        r.map(|()| file)
    }

    /// Quantas vezes já se tentou aplicar a versão.
    fn tries(&self, version: &str) -> u32 {
        let t = std::fs::read_to_string(self.dir.join("tentativas")).unwrap_or_default();
        t.trim().split_once(' ').filter(|x| x.0 == version).and_then(|x| x.1.parse().ok()).unwrap_or(0)
    }

    fn note_try(&self, version: &str) {
        let n = self.tries(version) + 1;
        let _ = std::fs::write(self.dir.join("tentativas"), format!("{version} {n}"));
    }

    /// Ao abrir: apaga o que já foi usado. Devolve `true` se esta é a primeira
    /// vez nesta versão, vindo de uma anterior.
    fn started(&self) -> bool {
        let last = self.dir.join("versao");
        let before = std::fs::read_to_string(&last).unwrap_or_default();
        let before = before.trim();
        if before == VERSION {
            return false;
        }
        for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("duck-sheets-") || name == "tentativas" {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let _ = crate::recovery::private_dir(&self.dir);
        let _ = std::fs::write(&last, VERSION);
        compare(VERSION, before) == Some(Ordering::Greater)
    }
}

// ---------------------------------------------------------------------------
// No app
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nada de novo, até onde se sabe.
    None,
    Checking,
    UpToDate,
    Downloading(String),
    /// Baixada e conferida: entra ao fechar.
    Ready(String, PathBuf),
    /// Há versão nova, mas esta instalação não se troca sozinha.
    Available(String),
    Failed(String),
}

#[derive(Default)]
pub struct Updater {
    /// O canal das threads que procuram.
    channel: Option<(Sender<Status>, Receiver<Status>)>,
    status: Option<Status>,
    /// As Configurações deixam procurar sozinho.
    enabled: Arc<AtomicBool>,
    /// Foi a pessoa que pediu (as mensagens aparecem mesmo sem novidade).
    manual: bool,
    /// Uma trava por janela aberta: só a última a fechar atualiza.
    instance: Option<File>,
    /// "Reiniciar e atualizar": abre de novo depois.
    reopen: bool,
}

impl Updater {
    pub fn status(&self) -> &Status {
        self.status.as_ref().unwrap_or(&Status::None)
    }

    fn sender(&mut self) -> Sender<Status> {
        self.channel.get_or_insert_with(channel).0.clone()
    }

    pub fn enable(&self, on: bool) {
        self.enabled.store(on, Atomic::Relaxed);
    }
}

/// Uma olhada completa; as etapas vão para `tx`.
fn look(place: &Place, install: &Install, force: bool, tx: &dyn Fn(Status)) -> Status {
    let m = match place.manifest(force) {
        Ok(m) => m,
        Err(e) => return Status::Failed(e),
    };
    if !m.newer() {
        return Status::UpToDate;
    }
    let asset = install.kind().and_then(|k| m.asset(k));
    let Some(asset) = asset.filter(|_| place.tries(&m.version) < TRIES) else {
        return Status::Available(m.version);
    };
    tx(Status::Downloading(m.version.clone()));
    match place.download(&m, asset) {
        Ok(f) => Status::Ready(m.version, f),
        Err(e) => Status::Failed(e),
    }
}

impl SheetsApp {
    /// Com a janela aberta: começa a olhar sozinho.
    pub fn update_start(&mut self) {
        if self.unattended() || cfg!(test) {
            return;
        }
        let (Some(place), Some(waker)) = (Place::new(), self.waker.clone()) else { return };
        let updated = place.started();
        if updated {
            self.message = format!("Duck Sheets atualizado para a versão {VERSION}");
        }
        self.updater.instance = hold(&place.dir);
        self.updater.enable(self.prefs.update);
        let on = self.updater.enabled.clone();
        let tx = self.updater.sender();
        let _ = std::thread::Builder::new().name("duck-atualizar".into()).spawn(move || {
            std::thread::sleep(FIRST);
            loop {
                if on.load(Atomic::Relaxed) && !search(&place, false, &tx, &waker) {
                    return;
                }
                std::thread::sleep(EVERY);
            }
        });
    }

    /// Menu Arquivo: procurar, reiniciar para atualizar ou abrir a página.
    pub fn update_act(&mut self) -> bool {
        match self.updater.status().clone() {
            Status::Ready(..) => {
                if !self.confirm_discard() {
                    return false;
                }
                self.updater.reopen = true;
                return true;
            }
            Status::Available(v) => {
                duck_platform::open_url(&format!("{REPO}/releases/tag/v{v}"));
            }
            Status::Checking | Status::Downloading(_) => {}
            _ => {
                let (Some(place), Some(waker)) = (Place::new(), self.waker.clone()) else {
                    self.message = "Esta compilação não procura atualizações".into();
                    return false;
                };
                self.updater.manual = true;
                self.updater.status = Some(Status::Checking);
                self.message = "Procurando atualizações…".into();
                let tx = self.updater.sender();
                let _ = std::thread::Builder::new().name("duck-atualizar".into()).spawn(move || {
                    search(&place, true, &tx, &waker);
                });
            }
        }
        false
    }

    /// O texto do item no menu Arquivo.
    pub fn update_label(&self) -> String {
        match self.updater.status() {
            Status::Ready(v, _) => format!("Reiniciar e atualizar para {v}"),
            Status::Available(v) => format!("Baixar a versão {v}…"),
            Status::Checking => "Procurando atualizações…".into(),
            Status::Downloading(v) => format!("Baixando a versão {v}…"),
            _ => "Procurar atualizações".into(),
        }
    }

    /// Novidades da thread. `true` = redesenhar.
    pub fn update_poll(&mut self) -> bool {
        let Some((_, rx)) = &self.updater.channel else { return false };
        let Some(s) = rx.try_iter().last() else { return false };
        let manual = self.updater.manual;
        match &s {
            Status::Ready(v, _) => {
                self.message = format!("A versão {v} está pronta: entra quando você fechar o Duck Sheets");
            }
            Status::Available(v) => {
                self.message = format!("Há uma versão nova do Duck Sheets: {v} (menu Arquivo)");
            }
            Status::UpToDate if manual => {
                self.message = format!("O Duck Sheets está na versão mais nova ({VERSION})");
            }
            Status::Failed(e) if manual => self.message = format!("Não foi possível atualizar: {e}"),
            Status::Downloading(v) if manual => self.message = format!("Baixando a versão {v}…"),
            _ => {}
        }
        if !matches!(s, Status::Checking | Status::Downloading(_)) {
            self.updater.manual = false;
        }
        // Uma versão já pronta não deixa de estar por uma falha de rede.
        let keep = matches!(self.updater.status(), Status::Ready(..)) && matches!(s, Status::Failed(_));
        if !keep {
            self.updater.status = Some(s);
        }
        true
    }

    /// Depois de a janela fechar: aplica a versão baixada, se esta era a última
    /// janela.
    pub fn update_on_exit(&mut self) {
        let Status::Ready(version, file) = self.updater.status().clone() else { return };
        if self.unattended() || !(self.prefs.update || self.updater.reopen) {
            return;
        }
        let Some(place) = Place::new() else { return };
        self.updater.instance = None;
        let Some(_alone) = alone(&place.dir) else { return };
        // Confere de novo: pode ter passado muito tempo desde o download.
        let install = Install::detect();
        let good = place.cached().ok().is_some_and(|m| {
            m.version == version && install.kind().and_then(|k| m.asset(k)).is_some_and(|a| intact(&file, a))
        });
        if !good {
            return;
        }
        place.note_try(&version);
        if let Err(e) = install.apply(&file, self.updater.reopen) {
            let _ = std::fs::write(place.dir.join("erro.txt"), format!("{version}: {e}\n"));
        }
    }
}

/// `false` = ninguém mais escuta.
fn search(place: &Place, force: bool, tx: &Sender<Status>, waker: &duck_platform::Waker) -> bool {
    let send = |s: Status| {
        let ok = tx.send(s).is_ok();
        waker.wake();
        ok
    };
    let last = look(place, &Install::detect(), force, &|s| {
        send(s);
    });
    send(last)
}

/// Marca que há uma janela aberta.
fn hold(dir: &Path) -> Option<File> {
    let f = open_lock(dir)?;
    f.try_lock_shared().ok()?;
    Some(f)
}

/// Ninguém mais tem uma janela aberta (e, enquanto isto viver, ninguém marca).
fn alone(dir: &Path) -> Option<File> {
    let f = open_lock(dir)?;
    f.try_lock().ok()?;
    Some(f)
}

fn open_lock(dir: &Path) -> Option<File> {
    crate::recovery::private_dir(dir).ok()?;
    File::options().create(true).truncate(false).read(true).write(true).open(dir.join("janelas.lock")).ok()
}

/// `--atualizar`: procura, baixa e aplica agora. Devolve o código de saída.
pub fn command() -> i32 {
    let Some(place) = Place::new() else {
        eprintln!("esta compilação não tem a chave das atualizações");
        return 2;
    };
    let install = Install::detect();
    match look(&place, &install, true, &|s| {
        if let Status::Downloading(v) = s {
            println!("baixando a versão {v}…");
        }
    }) {
        Status::UpToDate => {
            println!("Duck Sheets {VERSION}: já é a versão mais nova");
            0
        }
        Status::Ready(v, file) => {
            if alone(&place.dir).is_none() {
                eprintln!("feche as janelas do Duck Sheets e tente de novo");
                return 1;
            }
            match install.apply(&file, false) {
                Ok(()) => {
                    println!("instalando a versão {v}");
                    0
                }
                Err(e) => {
                    eprintln!("não foi possível instalar a versão {v}: {e}");
                    1
                }
            }
        }
        Status::Available(v) => {
            println!("há uma versão nova, {v}, para instalar à mão: {REPO}/releases/tag/v{v}");
            0
        }
        Status::Failed(e) => {
            eprintln!("{e}");
            1
        }
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        let order = [
            "0.9.9",
            "1.0.0-alpha",
            "1.0.0-beta.1",
            "1.0.0-beta.2",
            "1.0.0-beta.10",
            "1.0.0-beta.10.1",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1-beta.1",
            "1.0.1",
            "1.10.0",
            "2.0.0",
        ];
        for (i, a) in order.iter().enumerate() {
            for (j, b) in order.iter().enumerate() {
                assert_eq!(compare(a, b), Some(i.cmp(&j)), "{a} {b}");
            }
        }
        for bad in ["", "1", "1.0", "1.0.x", "v1.0.0", "1.0.0.0"] {
            assert_eq!(compare(bad, "1.0.0"), None, "{bad}");
        }
        assert!(compare(VERSION, "0.0.1").is_some());
    }

    fn sample(sha: &str) -> String {
        format!(
            "duck-sheets 1\r\nversao=9.9.9-beta.1\nfutura=linha nova\narquivo=windows-instalador|a-1.exe|5|{sha}\narquivo=linux-appimage|b.AppImage|7|{sha}\n"
        )
    }

    #[test]
    fn manifest() {
        let sha = "AB".repeat(32);
        let m = Manifest::parse(&sample(&sha)).unwrap();
        assert!(m.newer());
        assert_eq!(m.version, "9.9.9-beta.1");
        let a = m.asset("windows-instalador").unwrap();
        assert_eq!((a.name.as_str(), a.size, a.sha256.as_str()), ("a-1.exe", 5, "ab".repeat(32).as_str()));
        assert!(m.asset("macos-app").is_none());
        let good = sample(&sha);
        for (from, to) in [
            ("duck-sheets 1", "duck-sheets 2"),
            ("9.9.9-beta.1", "9.9"),
            ("9.9.9-beta.1", "9.9.9-beta/../.."),
            ("a-1.exe", "../a-1.exe"),
            ("a-1.exe", "..\\a.exe"),
            ("a-1.exe", ".oculto"),
            ("a-1.exe", ""),
            ("|5|", "|0|"),
            ("|5|", "|99999999999|"),
            ("|5|", "|x|"),
            ("ABAB", "ZZZZ"),
            ("|b.AppImage", ""),
        ] {
            assert!(Manifest::parse(&good.replacen(from, to, 1)).is_none(), "{from} → {to}");
        }
    }

    // Par de ensaio (`minisign -G -W`); só para estes testes.
    const KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIG: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";

    #[test]
    fn signature() {
        assert_eq!(verify(b"test", SIG, KEY), Ok(()));
        assert!(verify(b"teste", SIG, KEY).is_err());
        assert!(verify(b"test", "lixo", KEY).is_err());
        let other = KEY.replace("GFO3", "GFO4");
        assert!(verify(b"test", SIG, &other).is_err());
        assert_eq!(
            key_line(
                "untrusted comment: minisign public key\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n"
            ),
            Some(KEY)
        );
        assert_eq!(key_line("untrusted comment: ainda sem chave\n"), None);
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("duck-atualizar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Só o arquivo inteiro e igual ao do manifesto serve.
    #[test]
    fn whole_files_only() {
        let d = temp("inteiro");
        let f = d.join("x");
        std::fs::write(&f, b"abc").unwrap();
        let a = |size: u64, sha: &str| Asset { name: "x".into(), size, sha256: sha.into() };
        let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(intact(&f, &a(3, sha)));
        assert!(!intact(&f, &a(4, sha)));
        assert!(!intact(&f, &a(3, &sha.replace("ba78", "ba79"))));
        assert!(!intact(&d.join("y"), &a(3, sha)));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Sem assinatura que confira nada é aceito, nem o que está guardado.
    #[test]
    fn unsigned_manifest_is_refused() {
        let d = temp("assinatura");
        let from = d.join("origem");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("beta.txt"), sample(&"ab".repeat(32))).unwrap();
        std::fs::write(from.join("beta.txt.minisig"), SIG).unwrap();
        std::fs::copy(from.join("beta.txt"), from.join("estavel.txt")).unwrap();
        std::fs::copy(from.join("beta.txt.minisig"), from.join("estavel.txt.minisig")).unwrap();
        let url =
            format!("file:///{}", from.display().to_string().replace('\\', "/").trim_start_matches('/'));
        let p = Place { dir: d.join("guardado"), base: Some(url), key: KEY };
        let e = p.manifest(true).unwrap_err();
        assert!(e.contains("assinatura") || e.contains("curl"), "{e}");
        std::fs::create_dir_all(&p.dir).unwrap();
        let name = format!("{}.txt", Place::channel());
        std::fs::copy(from.join("beta.txt"), p.dir.join(&name)).unwrap();
        std::fs::copy(from.join("beta.txt.minisig"), p.dir.join(format!("{name}.minisig"))).unwrap();
        assert!(p.manifest(false).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Só a última janela atualiza.
    #[test]
    fn last_window_only() {
        let d = temp("janelas");
        let (a, b) = (hold(&d).unwrap(), hold(&d).unwrap());
        assert!(alone(&d).is_none());
        drop(a);
        assert!(alone(&d).is_none());
        drop(b);
        let x = alone(&d).unwrap();
        assert!(hold(&d).is_none());
        drop(x);
        assert!(hold(&d).is_some());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn tries_and_first_run() {
        let d = temp("vezes");
        let p = Place { dir: d.clone(), base: None, key: KEY };
        assert_eq!(p.tries("2.0.0"), 0);
        p.note_try("2.0.0");
        p.note_try("2.0.0");
        assert_eq!((p.tries("2.0.0"), p.tries("2.0.1")), (2, 0));
        // Instalação nova: não é atualização. Vindo de uma anterior, é.
        assert!(!p.started());
        assert!(!p.started());
        std::fs::write(d.join("versao"), "0.0.1").unwrap();
        std::fs::write(d.join("duck-sheets-0.0.2-x.exe"), "velho").unwrap();
        assert!(p.started());
        assert!(!d.join("duck-sheets-0.0.2-x.exe").exists());
        assert_eq!(p.tries("2.0.0"), 0);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn kinds_of_install() {
        let d = temp("tipos");
        let exe = d.join("duck-sheets.exe");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(Install::of(&exe, None), Install::Other);
        if cfg!(windows) {
            std::fs::write(d.join("unins000.exe"), "").unwrap();
            assert_eq!(Install::of(&exe, None), Install::Windows(d.clone()));
        } else if cfg!(target_os = "macos") {
            let app = d.join("Duck Sheets.app");
            let exe = app.join("Contents/MacOS/duck-sheets");
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            assert_eq!(Install::of(&exe, None), Install::MacApp(app));
        } else {
            let file = d.join("Duck.AppImage");
            std::fs::write(&file, "velho").unwrap();
            let i = Install::of(Path::new("/tmp/.mount_x/usr/bin/duck-sheets"), Some(&file));
            assert_eq!(i, Install::AppImage(file.clone()));
            std::fs::write(d.join("novo"), "novo").unwrap();
            i.apply(&d.join("novo"), false).unwrap();
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "novo");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}
