//! Proteção de planilha e de pasta na tela: o que fazer (e o aviso) quando
//! uma ação não é permitida, e proteger e desproteger. A senha só é conferida
//! com o resumo que o arquivo guarda; sem ela, a proteção fica.

use duck_core::protect::deny;
use duck_core::{BookProtection, CellRange, Command, Password, Sheet, SheetProtection, Styles};

use crate::app::SheetsApp;
use crate::dialog::{Dialog, Field, Kind};

/// Por que a planilha protegida recusa um comando (`None` = permitido).
fn refusal(sheet: &Sheet, styles: &Styles, cmd: &Command) -> Option<&'static str> {
    sheet.protected()?;
    let denied = |bit: u32, why: &'static str| sheet.denies(bit).then_some(why);
    const LOCKED: &str = "A célula está bloqueada, e a planilha está protegida.";
    let locked = |r, c| sheet.cell_locked(styles, r, c);
    match cmd {
        Command::SetCells(v) => v.iter().any(|x| locked(x.row, x.col)).then_some(LOCKED),
        Command::Restore(v) => v.iter().any(|x| locked(x.0, x.1)).then_some(LOCKED),
        Command::SetStyles(_) => {
            denied(deny::FORMAT_CELLS, "A planilha protegida não permite formatar células.")
        }
        Command::ColWidths(_) | Command::ColsHidden(_) => {
            denied(deny::FORMAT_COLUMNS, "A planilha protegida não permite formatar colunas.")
        }
        Command::RowHeights(_) | Command::RowsHidden(_) => {
            denied(deny::FORMAT_ROWS, "A planilha protegida não permite formatar linhas.")
        }
        Command::Merges { .. } => Some("A planilha protegida não permite mesclar células."),
        Command::Outline(_) => Some("A planilha protegida não permite abrir ou fechar grupos."),
        Command::SetFilter { filter, table } => {
            let had = match table {
                None => sheet.layout.autofilter.is_some(),
                Some(t) => sheet.layout.tables.get(*t as usize).is_some_and(|t| t.filter.is_some()),
            };
            // Como no Excel: só os critérios de um filtro que já existe.
            if had != filter.is_some() {
                return Some("A planilha protegida não permite ligar ou desligar o filtro.");
            }
            denied(deny::AUTO_FILTER, "A planilha protegida não permite filtrar.")
        }
        Command::SortRows { r0, c0, c1, perm } => {
            let rows = *r0..*r0 + perm.len() as u32;
            denied(deny::SORT, "A planilha protegida não permite classificar.").or_else(|| {
                rows.into_iter().any(|r| (*c0..=*c1).any(|c| locked(r, c))).then_some(
                    "Para classificar numa planilha protegida, todas as células precisam estar desbloqueadas.",
                )
            })
        }
        Command::Shift(s, _) => {
            use duck_core::Axis;
            let (bit, why) = match (s.insert, s.axis) {
                (true, Axis::Rows) => (deny::INSERT_ROWS, "A planilha protegida não permite inserir linhas."),
                (true, Axis::Cols) => {
                    (deny::INSERT_COLUMNS, "A planilha protegida não permite inserir colunas.")
                }
                (false, Axis::Rows) => {
                    (deny::DELETE_ROWS, "A planilha protegida não permite excluir linhas.")
                }
                (false, Axis::Cols) => {
                    (deny::DELETE_COLUMNS, "A planilha protegida não permite excluir colunas.")
                }
            };
            denied(bit, why).or_else(|| {
                // Excluir só se tudo o que sai estiver desbloqueado.
                if s.insert {
                    return None;
                }
                let (rows, cols) = match s.axis {
                    Axis::Rows => (s.at..(s.at + s.n).min(sheet.rows()), 0..sheet.cols()),
                    Axis::Cols => (0..sheet.rows(), s.at..(s.at + s.n).min(sheet.cols())),
                };
                let hit = rows
                    .into_iter()
                    .any(|r| cols.clone().any(|c| !sheet.raw(r, c).is_blank() && locked(r, c)));
                hit.then_some("Há células bloqueadas no que seria excluído, e a planilha está protegida.")
            })
        }
        Command::Validations(_) => Some("A planilha protegida não permite mudar a validação de dados."),
        Command::Notes(_) => denied(deny::OBJECTS, "A planilha protegida não permite mudar notas."),
        Command::Anchors(_) => {
            denied(deny::OBJECTS, "A planilha protegida não permite mover nem redimensionar objetos.")
        }
        Command::Layout(l) => (l.pivots != sheet.layout.pivots)
            .then(|| denied(deny::PIVOTS, "A planilha protegida não permite mudar tabelas dinâmicas."))
            .flatten(),
        // O que se quer fazer (inserir células) antes do que isso muda.
        Command::Group(list) => {
            let (ops, rest): (Vec<_>, Vec<_>) = list.iter().partition(|c| matches!(c, Command::CellOp(..)));
            ops.into_iter().chain(rest).find_map(|c| refusal(sheet, styles, c))
        }
        Command::TabColor(_) => Some("A planilha protegida não permite mudar a cor da guia."),
        Command::CellOp(duck_core::cells::CellOp::Shift { .. }, _) => {
            Some("A planilha protegida não permite inserir ou excluir células.")
        }
        Command::RichText(v) => v.iter().any(|x| locked(x.0, x.1)).then_some(LOCKED),
        // Mesmo vazias: formatação condicional, validação e links vão junto.
        Command::CellOp(op @ duck_core::cells::CellOp::Move { src, .. }, _) => {
            let hit = [Some(*src), op.target()].into_iter().flatten().any(|a| {
                (a.first_row..=a.last_row).any(|r| (a.first_col..=a.last_col).any(|c| locked(r, c)))
            });
            hit.then_some(LOCKED)
        }
        // A troca de nome de aba nas fórmulas é consequência.
        Command::FormulaTexts(_) => None,
        // A lista de abas é com a proteção da pasta (ver `allowed`).
        Command::Pane(_) | Command::Protect(_) | Command::BookProtect(_) | Command::Book(_) => None,
    }
}

/// O comando insere, apaga ou move linhas, colunas ou células?
fn moves_cells(cmd: &Command) -> bool {
    match cmd {
        Command::Shift(..) | Command::CellOp(..) => true,
        Command::Group(list) => list.iter().any(moves_cells),
        _ => false,
    }
}

/// O comando muda tabelas dinâmicas da aba?
fn changes_pivots(sheet: &Sheet, cmd: &Command) -> bool {
    match cmd {
        Command::Layout(l) => l.pivots != sheet.layout.pivots,
        Command::Group(list) => list.iter().any(|c| changes_pivots(sheet, c)),
        _ => false,
    }
}

/// Como resolver, no fim de todo aviso.
const HOW: &str = "Para mudar, desproteja a planilha em Revisão > Desproteger planilha (pode pedir senha).";

const OPTIONS: [(u32, &str); 15] = [
    (deny::SELECT_LOCKED, "Selecionar células bloqueadas"),
    (deny::SELECT_UNLOCKED, "Selecionar células desbloqueadas"),
    (deny::FORMAT_CELLS, "Formatar células"),
    (deny::FORMAT_COLUMNS, "Formatar colunas"),
    (deny::FORMAT_ROWS, "Formatar linhas"),
    (deny::INSERT_COLUMNS, "Inserir colunas"),
    (deny::INSERT_ROWS, "Inserir linhas"),
    (deny::INSERT_LINKS, "Inserir hiperlinks"),
    (deny::DELETE_COLUMNS, "Excluir colunas"),
    (deny::DELETE_ROWS, "Excluir linhas"),
    (deny::SORT, "Classificar"),
    (deny::AUTO_FILTER, "Usar o filtro"),
    (deny::PIVOTS, "Usar tabelas dinâmicas"),
    (deny::OBJECTS, "Editar objetos e notas"),
    (deny::SCENARIOS, "Editar cenários"),
];

impl SheetsApp {
    /// A proteção permite estes comandos? Se não, avisa.
    ///
    /// Num passo que insere, apaga ou move linhas, colunas ou células, só a
    /// aba que muda conta: os ajustes de fórmulas nas outras são
    /// consequência, não edição. O mesmo vale quando a lista de abas muda (a
    /// proteção que conta é a da pasta).
    pub fn allowed(&mut self, step: &[(usize, &Command)]) -> bool {
        // Com um valor inválido à espera de resposta, nada mais muda.
        if self.answering() {
            return false;
        }
        if step.iter().any(|(_, c)| crate::tabs::changes_tabs(c)) {
            return !self.structure_locked();
        }
        if self.unchecked {
            return true;
        }
        let moved = step.iter().any(|(_, c)| moves_cells(c));
        let Some(doc) = self.doc.as_ref() else { return true };
        let why = step.iter().find_map(|(i, cmd)| {
            if moved && !moves_cells(cmd) {
                return None;
            }
            let sheet = doc.sheets.get(*i)?.data.as_deref()?;
            sheet.protected()?;
            // Uma dinâmica que a proteção deixa mudar reescreve as próprias
            // células, mesmo bloqueadas.
            let pivots = step.iter().any(|(k, c)| k == i && changes_pivots(sheet, c));
            if pivots {
                let why = "A planilha protegida não permite mudar tabelas dinâmicas.";
                return sheet.denies(deny::PIVOTS).then_some(why);
            }
            refusal(sheet, &doc.styles, cmd)
        });
        match why {
            Some(w) => {
                self.refuse(w);
                false
            }
            None => true,
        }
    }

    fn refuse(&mut self, why: &str) {
        self.message = why.to_string();
        self.dialog = Some(Dialog::message("Planilha protegida", &format!("{why}\n\n{HOW}")));
    }

    /// A célula da aba ativa pode ser editada? Se não, avisa.
    pub fn can_edit_cell(&mut self, row: u32, col: u32) -> bool {
        let Some(doc) = self.doc.as_ref() else { return false };
        let Some((sheet, _)) = doc.current() else { return false };
        if sheet.protected().is_some() && sheet.cell_locked(&doc.styles, row, col) {
            let range = sheet
                .layout
                .protected_ranges
                .iter()
                .any(|p| p.password.is_set() && p.ranges.iter().any(|r| r.contains(row, col)));
            self.refuse(if range {
                "A célula está num intervalo protegido por senha própria."
            } else {
                "A célula está bloqueada, e a planilha está protegida."
            });
            return false;
        }
        true
    }

    pub fn can_change_objects(&mut self) -> bool {
        let locked = self.objects_locked();
        if locked {
            self.refuse("A planilha protegida não permite mudar notas.");
        }
        !locked
    }

    /// Os objetos da aba à vista estão protegidos? (Aí o mouse não os pega.)
    pub fn objects_locked(&self) -> bool {
        self.doc.as_ref().and_then(|d| d.current()).is_some_and(|(s, _)| s.denies(deny::OBJECTS))
    }

    /// A célula pode ser selecionada?
    pub fn can_select(&self, row: u32, col: u32) -> bool {
        let Some(doc) = self.doc.as_ref() else { return true };
        let Some((sheet, _)) = doc.current() else { return true };
        let Some(p) = sheet.protected() else { return true };
        if p.deny & (deny::SELECT_LOCKED | deny::SELECT_UNLOCKED) == 0 {
            return true;
        }
        let bit = if sheet.cell_locked(&doc.styles, row, col) {
            deny::SELECT_LOCKED
        } else {
            deny::SELECT_UNLOCKED
        };
        p.deny & bit == 0
    }

    /// A próxima célula selecionável a partir de `from`, andando para
    /// `(dr, dc)`; `None` se não houver.
    pub fn next_selectable(&self, from: (u32, u32), dr: i32, dc: i32) -> Option<(u32, u32)> {
        let (sheet, v) = self.doc.as_ref()?.current()?;
        let (nr, nc) = (v.rows.len() as i64, v.cols.len() as i64);
        // Só vale procurar onde pode haver célula desbloqueada.
        let limit = (sheet.rows().max(sheet.cols()) as i64 + 200).min(100_000);
        let (mut r, mut c) = (from.0 as i64, from.1 as i64);
        for _ in 0..limit {
            r += dr.signum() as i64;
            c += dc.signum() as i64;
            if r < 0 || c < 0 || r >= nr || c >= nc {
                return None;
            }
            let hidden = v.rows.size(r as u32) == 0 || v.cols.size(c as u32) == 0;
            if !hidden && self.can_select(r as u32, c as u32) {
                return Some((r as u32, c as u32));
            }
        }
        None
    }

    /// A estrutura da pasta está protegida (criar, renomear, mover, ocultar e
    /// excluir abas)? Se estiver, avisa.
    pub fn structure_locked(&mut self) -> bool {
        let on = self.doc.as_ref().and_then(|d| d.protection.as_ref()).is_some_and(|p| p.structure);
        if on {
            let why = "A estrutura da pasta de trabalho está protegida.";
            self.message = why.into();
            self.dialog = Some(Dialog::message(
                "Pasta protegida",
                &format!(
                    "{why}\n\nPara mudar as abas, desproteja a pasta em Revisão > Desproteger pasta (pode pedir senha)."
                ),
            ));
        }
        on
    }

    pub fn sheet_protected(&self) -> bool {
        self.doc.as_ref().and_then(|d| d.current()).is_some_and(|(s, _)| s.protected().is_some())
    }

    pub fn book_protected(&self) -> bool {
        self.doc.as_ref().and_then(|d| d.protection.as_ref()).is_some_and(|p| p.structure || p.windows)
    }

    /// Os botões de proteger (`on`) e desproteger a planilha ou a pasta.
    pub fn protect_button(&mut self, on: bool, sheet: bool) {
        let now = if sheet { self.sheet_protected() } else { self.book_protected() };
        let what = if sheet { "A planilha" } else { "A pasta" };
        match (on, now) {
            (true, true) => self.message = format!("{what} já está protegida"),
            (false, false) => self.message = format!("{what} não está protegida"),
            _ if sheet => self.ask_protect_sheet(),
            _ => self.ask_protect_book(),
        }
    }

    /// Proteger a planilha, ou desproteger se já estiver.
    fn ask_protect_sheet(&mut self) {
        self.commit_edit();
        let Some((sheet, _)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        match sheet.protected() {
            Some(p) if p.password.is_set() => {
                self.dialog = Some(Dialog::new(
                    Kind::UnprotectSheet,
                    "Desproteger planilha",
                    vec![Field::secret("Senha")],
                    &["OK", "Cancelar"],
                ));
            }
            Some(_) => {
                self.set_protection(None);
            }
            None => {
                let was = sheet.layout.protection.as_ref().map_or(deny::EXCEL, |p| p.deny);
                let mut fields = vec![
                    Field::secret("Senha (opcional)"),
                    Field::secret("Confirme a senha"),
                    Field::info("Quem abrir a planilha poderá:"),
                ];
                fields.extend(OPTIONS.iter().map(|(bit, label)| Field::check(label, was & bit == 0)));
                self.dialog =
                    Some(Dialog::new(Kind::ProtectSheet, "Proteger planilha", fields, &["OK", "Cancelar"]));
            }
        }
    }

    /// Proteger a pasta, ou desproteger se já estiver.
    fn ask_protect_book(&mut self) {
        self.commit_edit();
        let Some(doc) = self.doc.as_ref() else { return };
        match doc.protection.as_ref().filter(|p| p.structure || p.windows) {
            Some(p) if p.password.is_set() => {
                self.dialog = Some(Dialog::new(
                    Kind::UnprotectBook,
                    "Desproteger pasta",
                    vec![Field::secret("Senha")],
                    &["OK", "Cancelar"],
                ));
            }
            Some(_) => {
                self.set_book_protection(None);
            }
            None => {
                self.dialog = Some(Dialog::new(
                    Kind::ProtectBook,
                    "Proteger a estrutura da pasta",
                    vec![
                        Field::info(
                            "As abas não poderão ser criadas, excluídas, renomeadas, movidas nem ocultadas.",
                        ),
                        Field::secret("Senha (opcional)"),
                        Field::secret("Confirme a senha"),
                    ],
                    &["OK", "Cancelar"],
                ));
            }
        }
    }

    fn set_protection(&mut self, p: Option<SheetProtection>) -> bool {
        let on = p.is_some();
        let ok = self.run_command(Command::Protect(p.map(Box::new)));
        if ok {
            (self.object, self.moving) = (None, None);
            self.message = if on { "Planilha protegida" } else { "Planilha desprotegida" }.into();
        }
        ok
    }

    pub fn set_book_protection(&mut self, p: Option<BookProtection>) {
        let on = p.is_some();
        let Some(active) = self.doc.as_ref().map(|d| d.active) else { return };
        if self.run_step(vec![(active, Command::BookProtect(p.map(Box::new)))]) {
            self.message = if on { "Pasta protegida" } else { "Pasta desprotegida" }.into();
        }
    }

    /// OK numa das caixas. `false` = falta corrigir algo.
    pub fn apply_protection(&mut self, d: &mut Dialog) -> bool {
        match d.kind {
            Kind::UnprotectSheet => {
                let Some(p) =
                    self.doc.as_ref().and_then(|x| x.current()).and_then(|(s, _)| s.protected().cloned())
                else {
                    return true;
                };
                if let Some(e) = wrong(&p.password, d.text_of(0)) {
                    d.error = e.into();
                    return false;
                }
                self.set_protection(None);
            }
            Kind::UnprotectBook => {
                let Some(p) = self.doc.as_ref().and_then(|x| x.protection.clone()) else { return true };
                if let Some(e) = wrong(&p.password, d.text_of(0)) {
                    d.error = e.into();
                    return false;
                }
                self.set_book_protection(None);
            }
            Kind::ProtectSheet => {
                let Some(password) = confirmed(d, 0) else { return false };
                let mut bits = 0;
                for (k, (bit, _)) in OPTIONS.iter().enumerate() {
                    if !d.is_on(3 + k) {
                        bits |= bit;
                    }
                }
                self.set_protection(Some(SheetProtection { enabled: true, deny: bits, password }));
            }
            Kind::ProtectBook => {
                let Some(password) = confirmed(d, 1) else { return false };
                self.set_book_protection(Some(BookProtection {
                    structure: true,
                    password,
                    ..Default::default()
                }));
            }
            _ => {}
        }
        true
    }

    /// Desprotege sem caixa de diálogo (macros e `--edit`). `Err` = o motivo.
    pub fn unprotect_with(&mut self, sheet: Option<usize>, typed: &str) -> Result<(), String> {
        let Some(doc) = self.doc.as_ref() else { return Ok(()) };
        match sheet {
            Some(i) => {
                let Some(p) =
                    doc.sheets.get(i).and_then(|s| s.data.as_deref()).and_then(|s| s.protected().cloned())
                else {
                    return Ok(());
                };
                if let Some(e) = wrong(&p.password, typed) {
                    return Err(e.into());
                }
                self.run_step(vec![(i, Command::Protect(None))]);
            }
            None => {
                let Some(p) = doc.protection.clone() else { return Ok(()) };
                if let Some(e) = wrong(&p.password, typed) {
                    return Err(e.into());
                }
                self.set_book_protection(None);
            }
        }
        Ok(())
    }
}

/// `None` = a senha confere.
fn wrong(p: &Password, typed: &str) -> Option<&'static str> {
    use duck_xlsx::protect::Check;
    match duck_xlsx::protect::check(p, typed) {
        Check::Ok => None,
        Check::Wrong => Some("Senha incorreta (maiúsculas e minúsculas contam)."),
        Check::Unknown => Some("O arquivo guarda a senha num formato que o duck-sheets não sabe conferir."),
    }
}

/// A senha digitada duas vezes nos campos `i` e `i + 1`.
fn confirmed(d: &mut Dialog, i: usize) -> Option<Password> {
    if d.text_of(i) != d.text_of(i + 1) {
        d.error = "As duas senhas não são iguais.".into();
        return None;
    }
    Some(duck_xlsx::protect::new_password(d.text_of(i)))
}

/// Troca a proteção da pasta e devolve o comando que desfaz.
pub fn swap(now: &mut Option<BookProtection>, changed: &mut bool, p: Option<Box<BookProtection>>) -> Command {
    *changed = true;
    Command::BookProtect(std::mem::replace(now, p.map(|b| *b)).map(Box::new))
}

// --- testes sem mexer na janela ----------------------------------------------

impl SheetsApp {
    /// Diz no terminal (sem o texto, que pode ter dados) que a última edição
    /// foi recusada, e fecha o aviso.
    pub fn scripted_refusal(&mut self) {
        let Some(d) = self.dialog.take() else { return };
        let why = match d.kind {
            Kind::Invalid(_) if d.title == "Há um erro na fórmula" => "recusado: erro na fórmula",
            Kind::Invalid(_) => "recusado pela validação",
            Kind::Message => "recusado (aviso)",
            _ => "caixa aberta",
        };
        eprintln!("--edit: {why}");
    }

    /// `validar:B2:B9=lista|Sim;Não`, `nota:B2=texto` (vazio tira),
    /// `nota-visivel:B2`, `lista:B4=2` (escolhe o item), `proteger=senha`,
    /// `desproteger=senha`, `proteger-pasta=senha`, `desproteger-pasta=senha`.
    pub fn scripted_rules(&mut self, e: &str) -> bool {
        let (op, arg) = match e.split_once(':') {
            Some((op, rest)) if matches!(op, "validar" | "nota" | "nota-visivel" | "lista") => (op, rest),
            _ => match e.split_once('=') {
                Some((op, arg))
                    if matches!(op, "proteger" | "desproteger" | "proteger-pasta" | "desproteger-pasta") =>
                {
                    (op, arg)
                }
                _ => return false,
            },
        };
        let password = duck_xlsx::protect::new_password(arg);
        let active = self.doc.as_ref().map(|d| d.active);
        match op {
            "proteger" => {
                self.set_protection(Some(SheetProtection { password, ..Default::default() }));
            }
            "proteger-pasta" => self.set_book_protection(Some(BookProtection {
                structure: true,
                password,
                ..Default::default()
            })),
            "desproteger" | "desproteger-pasta" => {
                let sheet = if op == "desproteger" { active } else { None };
                let r = self.unprotect_with(sheet, arg);
                eprintln!("--edit {op}: {}", if r.is_ok() { "ok" } else { "senha recusada" });
            }
            _ => {
                let (range, value) = arg.split_once('=').unwrap_or((arg, ""));
                let Some(((r0, c0), (r1, c1))) = crate::app::parse_range(range) else {
                    eprintln!("--edit: intervalo inválido: {range}");
                    return true;
                };
                self.go_to(r0, c0);
                match op {
                    "validar" => {
                        let range = CellRange { first_row: r0, first_col: c0, last_row: r1, last_col: c1 };
                        self.scripted_rule(range, value);
                    }
                    "nota" => self.set_note(r0, c0, &value.replace("\\n", "\n"), None),
                    "nota-visivel" => self.toggle_note(),
                    _ => {
                        self.open_list();
                        self.scripted_pick(value.parse().unwrap_or(1));
                    }
                }
            }
        }
        true
    }

    /// `--menu lista|validacao|nota|nota-mouse|proteger|proteger-pasta|desproteger`.
    pub fn scripted_dialog(&mut self, menu: &str) -> bool {
        match menu {
            "lista" => {
                self.open_list();
            }
            "validacao" => self.ask_validation(),
            "nota" => self.ask_note(),
            "nota-mouse" => {
                self.hover_note = self.doc.as_ref().and_then(|d| d.current()).map(|(_, v)| v.cur);
            }
            "notas" => self.show_notes = true,
            "proteger" | "desproteger" => self.ask_protect_sheet(),
            "proteger-pasta" => self.ask_protect_book(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{Axis, CellChange, CellValue, CellXf, Shift};

    fn change(row: u32, col: u32) -> Command {
        Command::SetCells(vec![CellChange {
            row,
            col,
            value: CellValue::Number(1.0),
            formula: None,
            style: None,
        }])
    }

    #[test]
    fn protected_sheets_refuse_what_they_deny() {
        let styles = Styles {
            xfs: vec![CellXf::default(), CellXf { unlocked: true, ..Default::default() }],
            ..Default::default()
        };
        let mut s = Sheet::new("P");
        s.set_cell(0, 0, CellValue::Number(1.0), None, Some(0));
        s.set_cell(1, 0, CellValue::Number(1.0), None, Some(1));
        let no = |s: &Sheet, c: &Command| refusal(s, &styles, c).is_some();
        assert!(!no(&s, &change(0, 0)), "sem proteção tudo vale");
        s.layout.protection = Some(SheetProtection::default());
        assert!(no(&s, &change(0, 0)) && !no(&s, &change(1, 0)));
        assert!(no(&s, &Command::Group(vec![change(1, 0), change(0, 0)])), "basta uma bloqueada");
        assert!(no(&s, &Command::SetStyles(vec![(1, 0, 0)])));
        assert!(no(&s, &Command::Merges { add: vec![], remove: vec![] }));
        assert!(no(&s, &Command::Notes(vec![])) && no(&s, &Command::Validations(vec![])));
        assert!(no(&s, &Command::Anchors(vec![])));
        assert!(!no(&s, &Command::Protect(None)), "desproteger tem a própria conferência");
        let ops = || {
            let shift = Shift { axis: Axis::Rows, at: 1, n: 1, insert: false };
            let ops = duck_calc::ShiftOps { shift, sheet: "P".into(), own: true };
            Command::Shift(shift, duck_core::OpsHandle(std::sync::Arc::new(ops)))
        };
        assert!(no(&s, &ops()));
        s.layout.protection.as_mut().unwrap().deny &=
            !(deny::DELETE_ROWS | deny::FORMAT_CELLS | deny::OBJECTS);
        assert!(!no(&s, &ops()), "a linha 2 só tem células desbloqueadas");
        assert!(!no(&s, &Command::SetStyles(vec![(0, 0, 1)])) && !no(&s, &Command::Notes(vec![])));
        // Células inseridas não; movidas, só entre desbloqueadas (mesmo vazias).
        let at = |r| CellRange { first_row: r, last_row: r, ..Default::default() };
        let shift = Shift { axis: Axis::Rows, at: 0, n: 1, insert: true };
        let insert = duck_core::cells::CellOp::Shift { shift, lo: 0, hi: 0 };
        let mv = duck_core::cells::CellOp::Move { src: at(1), dr: 1, dc: 0 };
        assert!(no(&s, &Command::CellOp(insert, false)) && no(&s, &Command::CellOp(mv, false)));
        s.set_cell(2, 0, CellValue::Empty, None, Some(1));
        assert!(!no(&s, &Command::CellOp(mv, false)));
        assert!(moves_cells(&Command::Group(vec![change(0, 0), Command::CellOp(mv, false)])));
        assert!(no(&s, &Command::TabColor(None)) && !moves_cells(&change(0, 0)));
        // Filtro: só os critérios de um que já existe.
        let filter = Command::SetFilter { table: None, filter: Some(Default::default()) };
        s.layout.protection.as_mut().unwrap().deny &= !deny::AUTO_FILTER;
        assert!(no(&s, &filter));
        s.layout.autofilter = Some(Default::default());
        assert!(!no(&s, &filter));
    }

    #[test]
    fn passwords() {
        let p = duck_xlsx::protect::new_password("pato");
        assert_eq!(wrong(&p, "pato"), None);
        assert!(wrong(&p, "").is_some() && wrong(&p, "Pato").is_some());
        assert_eq!(wrong(&Password::default(), ""), None);
        let odd = Password { algorithm: "MD2".into(), hash: vec![1], ..Default::default() };
        assert!(wrong(&odd, "x").is_some_and(|e| e.contains("não sabe conferir")));
    }
}
