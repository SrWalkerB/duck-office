//! Configurações do app (Arquivo → Configurações, Ctrl+,): o que o usuário
//! escolheu, o arquivo em que isso fica e a caixa que muda.
//!
//! O arquivo é o `preferencias.txt` da pasta de configuração, uma escolha por
//! linha (`chave=valor`). Um valor que não se entende vale o padrão, e as
//! linhas de outras chaves ficam como estão.

use std::path::{Path, PathBuf};

use crate::app::SheetsApp;
use crate::dialog::{Dialog, Field, Kind};
use crate::theme::Mode;

/// Para onde a célula ativa vai depois do Enter (com Shift, ao contrário).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Enter {
    #[default]
    Down,
    Right,
    Up,
    Left,
    Stay,
}

impl Enter {
    const ALL: [Enter; 5] = [Enter::Down, Enter::Right, Enter::Up, Enter::Left, Enter::Stay];

    fn name(self) -> &'static str {
        match self {
            Enter::Down => "baixo",
            Enter::Right => "direita",
            Enter::Up => "cima",
            Enter::Left => "esquerda",
            Enter::Stay => "ficar",
        }
    }

    /// O passo (linhas, colunas).
    pub fn step(self, shift: bool) -> (i32, i32) {
        let k = if shift { -1 } else { 1 };
        match self {
            Enter::Down => (k, 0),
            Enter::Right => (0, k),
            Enter::Up => (-k, 0),
            Enter::Left => (0, -k),
            Enter::Stay => (0, 0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub mode: Mode,
    /// No tema escuro, as células também escurecem.
    pub dark_sheet: bool,
    pub enter: Enter,
    /// Lista de funções e nomes ao digitar uma fórmula.
    pub suggest: bool,
    /// Lembrar os arquivos abertos no menu Arquivo.
    pub recent: bool,
    /// Macros do arquivo: perguntar (`true`) ou nunca executar.
    pub macros: bool,
    /// Baixar versões novas e instalar ao fechar.
    pub update: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            mode: Mode::System,
            dark_sheet: true,
            enter: Enter::Down,
            suggest: true,
            recent: true,
            macros: true,
            update: true,
        }
    }
}

fn yes(on: bool) -> &'static str {
    if on { "sim" } else { "não" }
}

impl Prefs {
    fn pairs(&self) -> [(&'static str, &'static str); 7] {
        [
            ("tema", self.mode.name()),
            ("planilha-escura", yes(self.dark_sheet)),
            ("enter", self.enter.name()),
            ("sugestoes", yes(self.suggest)),
            ("recentes", yes(self.recent)),
            ("macros", if self.macros { "perguntar" } else { "nunca" }),
            ("atualizar", yes(self.update)),
        ]
    }

    fn set(&mut self, key: &str, value: &str) {
        let v = value.trim().to_lowercase();
        let on = match v.as_str() {
            "sim" | "perguntar" => Some(true),
            "não" | "nao" | "nunca" => Some(false),
            _ => None,
        };
        let put = |x: &mut bool| *x = on.unwrap_or(*x);
        match key {
            "tema" => self.mode = Mode::parse(&v).unwrap_or(self.mode),
            "planilha-escura" => put(&mut self.dark_sheet),
            "enter" => {
                self.enter = Enter::ALL.into_iter().find(|e| e.name() == v).unwrap_or(self.enter);
            }
            "sugestoes" => put(&mut self.suggest),
            "recentes" => put(&mut self.recent),
            "macros" => put(&mut self.macros),
            "atualizar" => put(&mut self.update),
            _ => {}
        }
    }

    pub fn load(file: &Path) -> Self {
        let mut p = Self::default();
        for line in std::fs::read_to_string(file).unwrap_or_default().lines() {
            if let Some((k, v)) = line.split_once('=') {
                p.set(k.trim(), v);
            }
        }
        p
    }

    /// Grava o que mudou desde `before`; o resto do arquivo fica.
    pub fn save(&self, file: &Path, before: &Prefs) -> std::io::Result<()> {
        let mut changed = self.pairs().to_vec();
        let old = before.pairs();
        changed.retain(|p| !old.contains(p));
        write(file, &changed)
    }
}

/// Troca (ou põe no fim) as linhas dessas chaves.
pub fn write(file: &Path, pairs: &[(&str, &str)]) -> std::io::Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let ours = |l: &str| l.split_once('=').is_some_and(|(k, _)| pairs.iter().any(|p| p.0 == k.trim()));
    let mut text: String =
        old.lines().filter(|l| !ours(l) && !l.trim().is_empty()).map(|l| format!("{l}\n")).collect();
    for (k, v) in pairs {
        text.push_str(&format!("{k}={v}\n"));
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, text)
}

/// Arquivo das preferências (ao lado da lista de recentes). Os testes não
/// mexem no de quem os roda.
pub fn file() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    duck_platform::config_dir("duck-office").map(|d| d.join("preferencias.txt"))
}

// ---------------------------------------------------------------------------
// A caixa
// ---------------------------------------------------------------------------

// Posição de cada campo na caixa.
const THEME: usize = 1;
const SHEET: usize = 2;
const ENTER: usize = 4;
const SUGGEST: usize = 5;
const RECENT: usize = 7;
const MACROS: usize = 8;
const UPDATE: usize = 10;

const MODES: [Mode; 3] = [Mode::Light, Mode::Dark, Mode::System];

/// Botão "Restaurar padrões".
pub const RESET: usize = 1;

fn fields(p: &Prefs) -> Vec<Field> {
    let at = |m: Mode| MODES.iter().position(|x| *x == m).unwrap_or(0);
    let enter = Enter::ALL.iter().position(|x| *x == p.enter).unwrap_or(0);
    let mut v = vec![
        Field::heading("Aparência"),
        Field::choice("Tema", &["Claro", "Escuro", "Igual ao do sistema"], at(p.mode)),
        Field::check("No tema escuro, escurecer também as células", p.dark_sheet),
        Field::heading("Edição"),
        Field::choice(
            "Depois do Enter",
            &["Ir para baixo", "Ir para a direita", "Ir para cima", "Ir para a esquerda", "Ficar na célula"],
            enter,
        ),
        Field::check("Sugerir funções e nomes ao digitar fórmulas", p.suggest),
        Field::heading("Arquivos"),
        Field::check("Lembrar os arquivos recentes (desligar apaga a lista)", p.recent),
        Field::choice(
            "Macros do arquivo",
            &["Perguntar antes de executar", "Nunca executar"],
            usize::from(!p.macros),
        ),
        Field::heading("Atualizações"),
        Field::check("Baixar versões novas e instalar ao fechar", p.update),
    ];
    v[SHEET].show = p.mode != Mode::Light;
    v
}

/// O que está marcado na caixa.
fn read(d: &Dialog) -> Prefs {
    Prefs {
        mode: MODES[d.choice_of(THEME).min(MODES.len() - 1)],
        dark_sheet: d.is_on(SHEET),
        enter: Enter::ALL[d.choice_of(ENTER).min(Enter::ALL.len() - 1)],
        suggest: d.is_on(SUGGEST),
        recent: d.is_on(RECENT),
        macros: d.choice_of(MACROS) == 0,
        update: d.is_on(UPDATE),
    }
}

impl SheetsApp {
    pub fn ask_settings(&mut self) {
        self.commit_edit();
        // O valor em edição foi recusado: o aviso fica.
        if self.dialog.is_some() {
            return;
        }
        let kind = Kind::Settings(Box::new(self.prefs));
        let buttons = ["OK", "Restaurar padrões", "Cancelar"];
        self.dialog = Some(Dialog::new(kind, "Configurações", fields(&self.prefs), &buttons));
    }

    /// Algo mudou na caixa: já aparece (Cancelar volta atrás).
    pub fn settings_changed(&mut self, d: &mut Dialog) {
        let p = read(d);
        d.fields[SHEET].show = p.mode != Mode::Light;
        self.use_prefs(p);
    }

    pub fn settings_reset(&mut self, d: &mut Dialog) {
        d.fields = fields(&Prefs::default());
        self.settings_changed(d);
    }

    /// OK: lembra para as próximas vezes.
    pub fn settings_ok(&mut self, d: &Dialog, before: &Prefs) {
        self.use_prefs(read(d));
        if !self.prefs.recent && before.recent {
            self.forget_recent();
        }
        if let Some(Err(e)) = file().map(|f| self.prefs.save(&f, before)) {
            self.message = format!("Vale só até fechar: não foi possível gravar as configurações ({e})");
        }
    }

    /// Passa a usar estas preferências.
    pub fn use_prefs(&mut self, p: Prefs) {
        let mode = self.prefs.mode;
        self.prefs = p;
        self.updater.enable(p.update);
        if p.mode != mode
            && let Some(w) = &self.window
        {
            w.set_color_scheme(p.mode.frame());
            if p.mode == Mode::System {
                self.system = w.color_scheme().unwrap_or(self.system);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::Outcome;
    use crate::testing::{app, get};
    use duck_platform::{Clipboard, Key, Modifiers};

    #[test]
    fn remembered_between_runs() {
        let dir = std::env::temp_dir().join(format!("duck-prefs-{}", std::process::id()));
        let file = dir.join("preferencias.txt");
        // Sem arquivo: o padrão.
        let d = Prefs::default();
        assert_eq!(Prefs::load(&file), d);
        let all = Prefs {
            mode: Mode::Dark,
            dark_sheet: false,
            enter: Enter::Right,
            suggest: false,
            recent: false,
            macros: false,
            update: false,
        };
        all.save(&file, &d).unwrap();
        assert_eq!(Prefs::load(&file), all);
        for e in Enter::ALL {
            let p = Prefs { enter: e, ..all };
            p.save(&file, &Prefs { enter: Enter::ALL[usize::from(e == Enter::Down)], ..all }).unwrap();
            assert_eq!(Prefs::load(&file), p);
        }
        // Só o que mudou é gravado, outras linhas ficam, e valor estranho
        // vale o padrão.
        std::fs::write(&file, "outra=1\ntema=azul\nenter=?\nmacros=talvez\n").unwrap();
        assert_eq!(Prefs::load(&file), d);
        Prefs { mode: Mode::Light, ..d }.save(&file, &d).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text, "outra=1\nenter=?\nmacros=talvez\ntema=claro\n");
        assert_eq!(Prefs::load(&file).mode, Mode::Light);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn open(a: &mut SheetsApp) {
        a.ask_settings();
        assert_eq!(a.dialog.as_ref().unwrap().title, "Configurações");
    }

    fn pick(a: &mut SheetsApp, field: usize, option: usize) {
        match &mut a.dialog.as_mut().unwrap().fields[field].control {
            crate::dialog::Control::Choice { sel, .. } => *sel = option,
            crate::dialog::Control::Check(on) => *on = option == 1,
            _ => panic!("campo {field}"),
        }
        a.dialog_event(Outcome::Changed);
    }

    #[test]
    fn preview_cancel_reset_and_ok() {
        let mut a = app(&[("A", &[])]);
        a.use_prefs(Prefs { mode: Mode::Dark, ..a.prefs });
        // A caixa mostra o que está valendo.
        open(&mut a);
        assert_eq!(read(a.dialog.as_ref().unwrap()), a.prefs);
        // Trocar o tema já aparece; Cancelar e Esc voltam atrás.
        for o in [Outcome::Cancel, Outcome::Button(2)] {
            pick(&mut a, THEME, 0);
            assert!(!a.theme().dark);
            assert!(!a.dialog.as_ref().unwrap().fields[SHEET].show);
            a.dialog_event(o);
            assert!(a.dialog.is_none() && a.theme().dark);
            open(&mut a);
        }
        // Só a planilha clara: a interface continua escura.
        pick(&mut a, SHEET, 0);
        assert!(a.theme().dark && !a.sheet_theme().dark);
        a.dialog_event(Outcome::Button(0));
        assert!(a.dialog.is_none() && !a.prefs.dark_sheet);
        // Restaurar: a caixa continua aberta, e ainda dá para cancelar.
        open(&mut a);
        a.dialog_event(Outcome::Button(RESET));
        assert_eq!(a.prefs, Prefs::default());
        a.dialog_event(Outcome::Cancel);
        assert_eq!((a.prefs.mode, a.prefs.dark_sheet), (Mode::Dark, false));
    }

    #[test]
    fn recent_files_are_forgotten_only_on_ok() {
        let mut a = app(&[("A", &[])]);
        a.recent = vec!["x.xlsx".into()];
        open(&mut a);
        pick(&mut a, RECENT, 0);
        a.dialog_event(Outcome::Cancel);
        assert_eq!(a.recent.len(), 1);
        open(&mut a);
        pick(&mut a, RECENT, 0);
        a.dialog_event(Outcome::Button(0));
        a.remember_recent(Path::new("y.xlsx"));
        assert!(a.recent.is_empty());
    }

    #[test]
    fn enter_and_suggestions() {
        let mut a = app(&[("A", &[])]);
        let mut clip = Clipboard::default();
        let mut typed = |a: &mut SheetsApp, enter: Enter, shift: bool| {
            a.prefs.enter = enter;
            a.go_to(4, 4);
            a.start_edit(Some("7"), false);
            let mods = Modifiers { shift, ..Default::default() };
            a.editor_key(&mut clip, &Key::Enter, mods, None);
            assert_eq!(get(a, 0, "E5"), "7");
            a.doc.as_ref().unwrap().current().unwrap().1.cur
        };
        assert_eq!(typed(&mut a, Enter::Down, false), (5, 4));
        assert_eq!(typed(&mut a, Enter::Down, true), (3, 4));
        assert_eq!(typed(&mut a, Enter::Right, false), (4, 5));
        assert_eq!(typed(&mut a, Enter::Right, true), (4, 3));
        assert_eq!(typed(&mut a, Enter::Up, false), (3, 4));
        assert_eq!(typed(&mut a, Enter::Left, false), (4, 3));
        assert_eq!(typed(&mut a, Enter::Stay, false), (4, 4));

        for on in [true, false] {
            a.prefs.suggest = on;
            a.start_edit(Some("=som"), false);
            a.refresh_complete(true);
            assert_eq!(a.editor.as_ref().unwrap().complete.list.is_some(), on);
            // A assinatura continua nos dois casos.
            let ed = a.editor.as_mut().unwrap();
            ed.input.set_text("=SOMA(");
            a.refresh_complete(true);
            assert!(a.editor.as_ref().unwrap().complete.call.is_some());
            a.cancel_edit();
        }
    }
}
