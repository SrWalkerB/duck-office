//! Proteção de planilha (`<sheetProtection>`, `<protectedRanges>`) e de pasta
//! (`<workbookProtection>`).
//!
//! Aqui fica o que é permitido; conferir a senha (os algoritmos do arquivo) é
//! com o `duck-xlsx` (`password`).

use crate::{CellRange, Sheet, Styles};

/// Senha de uma proteção, como está no arquivo. Só o resumo (hash) é
/// guardado; a senha em si nunca.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Password {
    /// Resumo de 16 bits do Excel antigo (`password="CC1A"`).
    pub legacy: Option<u16>,
    /// `algorithmName`, com o resumo, o sal e as voltas.
    pub algorithm: String,
    pub hash: Vec<u8>,
    pub salt: Vec<u8>,
    pub spin: u32,
}

impl Password {
    /// Há uma senha para pedir. `password="0000"` não é senha: o Excel grava
    /// assim quando ela não existe.
    pub fn is_set(&self) -> bool {
        !self.hash.is_empty() || !self.algorithm.is_empty() || self.legacy.is_some_and(|h| h != 0)
    }
}

/// O que uma planilha protegida pode proibir: o bit ligado proíbe.
pub mod deny {
    pub const OBJECTS: u32 = 1 << 0;
    pub const SCENARIOS: u32 = 1 << 1;
    pub const FORMAT_CELLS: u32 = 1 << 2;
    pub const FORMAT_COLUMNS: u32 = 1 << 3;
    pub const FORMAT_ROWS: u32 = 1 << 4;
    pub const INSERT_COLUMNS: u32 = 1 << 5;
    pub const INSERT_ROWS: u32 = 1 << 6;
    pub const INSERT_LINKS: u32 = 1 << 7;
    pub const DELETE_COLUMNS: u32 = 1 << 8;
    pub const DELETE_ROWS: u32 = 1 << 9;
    pub const SELECT_LOCKED: u32 = 1 << 10;
    pub const SORT: u32 = 1 << 11;
    pub const AUTO_FILTER: u32 = 1 << 12;
    pub const PIVOTS: u32 = 1 << 13;
    pub const SELECT_UNLOCKED: u32 = 1 << 14;

    /// O que fica proibido quando o `<sheetProtection>` não diz nada.
    pub const DEFAULT: u32 = FORMAT_CELLS
        | FORMAT_COLUMNS
        | FORMAT_ROWS
        | INSERT_COLUMNS
        | INSERT_ROWS
        | INSERT_LINKS
        | DELETE_COLUMNS
        | DELETE_ROWS
        | SORT
        | AUTO_FILTER
        | PIVOTS;

    /// O que o Excel proíbe ao proteger com as opções de fábrica.
    pub const EXCEL: u32 = DEFAULT | OBJECTS | SCENARIOS;

    /// Nome do atributo de cada bit.
    pub const ATTRS: [(u32, &str); 15] = [
        (OBJECTS, "objects"),
        (SCENARIOS, "scenarios"),
        (FORMAT_CELLS, "formatCells"),
        (FORMAT_COLUMNS, "formatColumns"),
        (FORMAT_ROWS, "formatRows"),
        (INSERT_COLUMNS, "insertColumns"),
        (INSERT_ROWS, "insertRows"),
        (INSERT_LINKS, "insertHyperlinks"),
        (DELETE_COLUMNS, "deleteColumns"),
        (DELETE_ROWS, "deleteRows"),
        (SELECT_LOCKED, "selectLockedCells"),
        (SORT, "sort"),
        (AUTO_FILTER, "autoFilter"),
        (PIVOTS, "pivotTables"),
        (SELECT_UNLOCKED, "selectUnlockedCells"),
    ];
}

/// Proteção de uma planilha.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetProtection {
    /// Protegida agora (`sheet="1"`). Com `false`, o arquivo só guarda as
    /// opções.
    pub enabled: bool,
    /// Bits de [`deny`].
    pub deny: u32,
    pub password: Password,
}

impl Default for SheetProtection {
    fn default() -> Self {
        Self { enabled: true, deny: deny::EXCEL, password: Password::default() }
    }
}

/// Intervalo que pode ser editado mesmo com a planilha protegida
/// (`<protectedRange>`). Com senha, só depois de informada.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProtectedRange {
    pub name: String,
    pub ranges: Vec<CellRange>,
    pub password: Password,
}

/// Proteção da pasta de trabalho.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BookProtection {
    /// Estrutura: criar, excluir, renomear, mover, ocultar e reexibir abas.
    pub structure: bool,
    /// Janelas (o duck-sheets só preserva).
    pub windows: bool,
    pub password: Password,
    /// Atributos que o duck-sheets não usa (proteção das revisões), como no
    /// arquivo (` k="v"`).
    pub extra: String,
}

impl Sheet {
    /// A proteção em vigor, se a planilha estiver protegida.
    pub fn protected(&self) -> Option<&SheetProtection> {
        self.layout.protection.as_ref().filter(|p| p.enabled)
    }

    /// A planilha protegida proíbe `bit` (ver [`deny`]).
    pub fn denies(&self, bit: u32) -> bool {
        self.protected().is_some_and(|p| p.deny & bit != 0)
    }

    /// Estilo da célula como o Excel vê: o dela, ou o da linha ou da coluna
    /// se ela não existe.
    pub fn effective_style(&self, row: u32, col: u32) -> u16 {
        let raw = self.raw(row, col);
        if raw.style != 0 || raw.kind != crate::kind::EMPTY {
            return raw.style;
        }
        if let Some(s) = self.layout.row(row).and_then(|r| r.style) {
            return s;
        }
        self.layout.col(col).map_or(0, |c| c.style)
    }

    /// A célula está bloqueada (só importa com a planilha protegida).
    /// Numa mesclagem, vale a do canto.
    pub fn cell_locked(&self, styles: &Styles, row: u32, col: u32) -> bool {
        let (r, c) = self
            .layout
            .merges
            .iter()
            .find(|m| m.contains(row, col))
            .map_or((row, col), |m| (m.first_row, m.first_col));
        let locked = |r, c| styles.xfs.get(self.effective_style(r, c) as usize).is_none_or(|x| !x.unlocked);
        locked(r, c)
            && !self
                .layout
                .protected_ranges
                .iter()
                .any(|p| !p.password.is_set() && p.ranges.iter().any(|x| x.contains(r, c)))
    }

    /// A fórmula da célula não deve aparecer (planilha protegida).
    pub fn formula_hidden(&self, styles: &Styles, row: u32, col: u32) -> bool {
        self.protected().is_some()
            && styles.xfs.get(self.effective_style(row, col) as usize).is_some_and(|x| x.hide_formula)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellValue, CellXf};

    #[test]
    fn locked_cells() {
        let styles = Styles {
            xfs: vec![CellXf::default(), CellXf { unlocked: true, hide_formula: true, ..Default::default() }],
            ..Default::default()
        };
        let mut s = Sheet::new("P");
        s.set_cell(0, 0, CellValue::Number(1.0), None, Some(0));
        s.set_cell(0, 1, CellValue::Number(1.0), None, Some(1));
        s.layout.cols.push(crate::ColSpec { first: 3, last: 3, width: None, hidden: false, style: 1 });
        s.layout.merges.push(CellRange { first_row: 0, first_col: 1, last_row: 2, last_col: 1 });
        assert!(s.cell_locked(&styles, 0, 0));
        assert!(!s.cell_locked(&styles, 0, 1));
        assert!(!s.cell_locked(&styles, 2, 1), "a mesclagem segue o canto");
        assert!(!s.cell_locked(&styles, 9, 3), "célula vazia segue a coluna");
        assert!(!s.formula_hidden(&styles, 0, 1), "sem proteção nada se esconde");
        assert!(!s.denies(deny::SORT));
        s.layout.protection = Some(SheetProtection { deny: deny::DEFAULT, ..Default::default() });
        assert!(s.denies(deny::SORT) && !s.denies(deny::SELECT_LOCKED));
        assert!(s.formula_hidden(&styles, 0, 1));
        s.layout.protected_ranges.push(ProtectedRange {
            name: "Livre".into(),
            ranges: vec![CellRange::default()],
            password: Password::default(),
        });
        assert!(!s.cell_locked(&styles, 0, 0));
        s.layout.protected_ranges[0].password.legacy = Some(0xCC1A);
        assert!(s.cell_locked(&styles, 0, 0), "o intervalo tem senha");
        s.layout.protection.as_mut().unwrap().enabled = false;
        assert!(!s.denies(deny::SORT));
    }

    #[test]
    fn empty_password() {
        assert!(!Password::default().is_set());
        assert!(!Password { legacy: Some(0), ..Default::default() }.is_set());
        assert!(Password { legacy: Some(1), ..Default::default() }.is_set());
        assert!(Password { algorithm: "SHA-512".into(), ..Default::default() }.is_set());
    }
}
