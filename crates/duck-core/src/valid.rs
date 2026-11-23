//! Validação de dados (`<dataValidations>` e `<x14:dataValidations>`): o que
//! cada intervalo aceita, a lista suspensa e as mensagens.
//!
//! As fórmulas ficam como no arquivo (A1, sem o `=`) e valem para a base da
//! regra ([`DataValidation::base`]); nas outras células, as referências
//! relativas andam junto. Quem avalia é o `duck-calc` (`valid`).

use crate::CellRange;
use crate::shift::{Axis, FormulaOps, Shift};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DvKind {
    /// Qualquer valor (só a mensagem de entrada).
    #[default]
    Any,
    Whole,
    Decimal,
    List,
    Date,
    Time,
    TextLength,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DvOp {
    #[default]
    Between,
    NotBetween,
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterEqual,
    LessEqual,
}

impl DvOp {
    /// Usa a segunda fórmula.
    pub fn two(self) -> bool {
        matches!(self, DvOp::Between | DvOp::NotBetween)
    }
}

/// O que acontece com um valor inválido.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DvAlert {
    /// Recusa.
    #[default]
    Stop,
    /// Pergunta se mantém.
    Warning,
    /// Só avisa.
    Information,
}

/// Uma regra de validação.
#[derive(Clone, Debug, PartialEq)]
pub struct DataValidation {
    pub ranges: Vec<CellRange>,
    pub kind: DvKind,
    pub op: DvOp,
    pub f1: String,
    pub f2: String,
    /// "Ignorar em branco".
    pub allow_blank: bool,
    /// Mostra a seta da lista (no arquivo, `showDropDown` diz o contrário).
    pub dropdown: bool,
    pub show_input: bool,
    pub show_error: bool,
    pub alert: DvAlert,
    pub error_title: String,
    pub error: String,
    pub prompt_title: String,
    pub prompt: String,
    /// Vem da extensão do Excel 2010 (`x14`), e é gravada lá.
    pub ext: bool,
    /// Itens como o Excel 2010 os grava quando têm vírgula
    /// (`<x12ac:list>`, texto CSV); `f1` fica com a versão antiga.
    pub list12: Option<String>,
    /// Atributos que o duck-sheets não usa (`imeMode`, `xr:uid`), como no
    /// arquivo (` k="v"`).
    pub extra: String,
}

impl Default for DataValidation {
    fn default() -> Self {
        Self {
            ranges: Vec::new(),
            kind: DvKind::Any,
            op: DvOp::Between,
            f1: String::new(),
            f2: String::new(),
            allow_blank: true,
            dropdown: true,
            show_input: true,
            show_error: true,
            alert: DvAlert::Stop,
            error_title: String::new(),
            error: String::new(),
            prompt_title: String::new(),
            prompt: String::new(),
            ext: false,
            list12: None,
            extra: String::new(),
        }
    }
}

impl DataValidation {
    /// Canto superior esquerdo do retângulo que cobre os intervalos: a base
    /// das referências relativas.
    pub fn base(&self) -> (u32, u32) {
        (
            self.ranges.iter().map(|r| r.first_row).min().unwrap_or(0),
            self.ranges.iter().map(|r| r.first_col).min().unwrap_or(0),
        )
    }

    pub fn contains(&self, row: u32, col: u32) -> bool {
        self.ranges.iter().any(|r| r.contains(row, col))
    }

    /// Mesma regra (tudo menos onde ela vale).
    pub fn same_rule(&self, o: &DataValidation) -> bool {
        self.base() == o.base() && {
            let mut a = self.clone();
            a.ranges = o.ranges.clone();
            a.extra = o.extra.clone();
            a == *o
        }
    }

    /// A lista tem os itens escritos na própria regra (`"a,b,c"`).
    pub fn literal_items(&self) -> Option<Vec<String>> {
        if self.kind != DvKind::List {
            return None;
        }
        if let Some(x) = &self.list12 {
            return Some(split_csv(x));
        }
        let t = self.f1.trim();
        let body = t.strip_prefix('"')?.strip_suffix('"')?;
        // Uma aspa no meio (que não seja `""`) não é um texto só.
        if body.replace("\"\"", "").contains('"') {
            return None;
        }
        Some(body.replace("\"\"", "\"").split(',').map(|s| s.trim().to_string()).collect())
    }

    /// Guarda os itens na própria regra.
    pub fn set_literal_items(&mut self, items: &[String]) {
        let plain = |s: &String| s.replace('"', "\"\"");
        self.f1 = format!("\"{}\"", items.iter().map(plain).collect::<Vec<_>>().join(","));
        self.list12 = items.iter().any(|s| s.contains(',')).then(|| {
            let quoted = |s: &String| {
                if s.contains([',', '"']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.clone() }
            };
            items.iter().map(quoted).collect::<Vec<_>>().join(",")
        });
    }

    /// Troca os intervalos; se a base mudar, as fórmulas são reescritas para
    /// a nova (`relocate` desloca as referências relativas).
    pub fn set_ranges(&mut self, ranges: Vec<CellRange>, relocate: &dyn Fn(&str, i64, i64) -> String) {
        let old = self.base();
        self.ranges = ranges;
        self.rebase(old, self.base(), relocate);
    }

    fn rebase(&mut self, old: (u32, u32), new: (u32, u32), relocate: &dyn Fn(&str, i64, i64) -> String) {
        if old == new {
            return;
        }
        let (dr, dc) = (new.0 as i64 - old.0 as i64, new.1 as i64 - old.1 as i64);
        for f in [&mut self.f1, &mut self.f2] {
            if !f.is_empty() {
                *f = relocate(f, dr, dc);
            }
        }
    }
}

/// `a,"b,c",d` → itens.
fn split_csv(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' if quoted && it.peek() == Some(&'"') => {
                it.next();
                cur.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur).trim().to_string()),
            c => cur.push(c),
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// `r` sem a parte que cai em `hole` (até quatro pedaços).
pub fn subtract(r: CellRange, hole: CellRange) -> Vec<CellRange> {
    let a = CellRange {
        first_row: r.first_row.max(hole.first_row),
        first_col: r.first_col.max(hole.first_col),
        last_row: r.last_row.min(hole.last_row),
        last_col: r.last_col.min(hole.last_col),
    };
    if a.first_row > a.last_row || a.first_col > a.last_col {
        return vec![r];
    }
    let mut out = Vec::new();
    if r.first_row < a.first_row {
        out.push(CellRange { last_row: a.first_row - 1, ..r });
    }
    if r.first_col < a.first_col {
        out.push(CellRange { first_row: a.first_row, last_row: a.last_row, last_col: a.first_col - 1, ..r });
    }
    if a.last_col < r.last_col {
        out.push(CellRange { first_row: a.first_row, last_row: a.last_row, first_col: a.last_col + 1, ..r });
    }
    if a.last_row < r.last_row {
        out.push(CellRange { first_row: a.last_row + 1, ..r });
    }
    out
}

/// Tira a validação de `hole` (as regras encolhem ou se dividem).
pub fn clear(list: &mut Vec<DataValidation>, hole: CellRange, relocate: &dyn Fn(&str, i64, i64) -> String) {
    list.retain_mut(|v| {
        let ranges: Vec<CellRange> = v.ranges.iter().flat_map(|r| subtract(*r, hole)).collect();
        if ranges.is_empty() {
            return false;
        }
        if ranges != v.ranges {
            v.set_ranges(ranges, relocate);
        }
        true
    });
}

/// Regra que vale na célula. Se várias cobrirem, a última (a mais nova).
pub fn at(list: &[DataValidation], row: u32, col: u32) -> Option<&DataValidation> {
    list.iter().rev().find(|v| v.contains(row, col))
}

/// As regras depois de inserir ou apagar linhas/colunas na própria aba.
pub fn shift(list: &mut Vec<DataValidation>, s: Shift, ops: &dyn FormulaOps) {
    let relocate = |f: &str, dr: i64, dc: i64| ops.shift_a1(f, dr, dc);
    list.retain_mut(|v| {
        let ranges: Vec<CellRange> = v.ranges.iter().filter_map(|r| s.range(*r)).collect();
        if ranges.is_empty() {
            return false;
        }
        // A base foi apagada: as fórmulas passam a ser as da primeira
        // linha/coluna que sobrou, antes de andar.
        if !s.insert {
            let old = v.base();
            let axis_base = if s.axis == Axis::Rows { old.0 } else { old.1 };
            if s.index(axis_base).is_none() {
                let end = s.at + s.n;
                let alive = v
                    .ranges
                    .iter()
                    .filter_map(|r| {
                        let (a, b) = match s.axis {
                            Axis::Rows => (r.first_row, r.last_row),
                            Axis::Cols => (r.first_col, r.last_col),
                        };
                        if a < s.at || a >= end { Some(a) } else { (b >= end).then_some(end) }
                    })
                    .min();
                if let Some(k) = alive {
                    let new = if s.axis == Axis::Rows { (k, old.1) } else { (old.0, k) };
                    v.rebase(old, new, &relocate);
                }
            }
        }
        v.ranges = ranges;
        for f in [&mut v.f1, &mut v.f2] {
            if let Some((new, _)) = ops.adjust(f) {
                *f = new;
            }
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: u32, b: u32, c: u32, d: u32) -> CellRange {
        CellRange { first_row: a, first_col: b, last_row: c, last_col: d }
    }

    #[test]
    fn literal_lists() {
        let mut v =
            DataValidation { kind: DvKind::List, f1: "\"Sim, Não,Talvez\"".into(), ..Default::default() };
        assert_eq!(v.literal_items().unwrap(), ["Sim", "Não", "Talvez"]);
        v.f1 = "$A$1:$A$3".into();
        assert_eq!(v.literal_items(), None);
        v.f1 = "IF(A1=\"x\",\"a\",\"b\")".into();
        assert_eq!(v.literal_items(), None);
        v.set_literal_items(&["1,5".into(), "a \"b\"".into(), "c".into()]);
        assert_eq!(v.literal_items().unwrap(), ["1,5", "a \"b\"", "c"]);
        assert_eq!(v.list12.as_deref(), Some("\"1,5\",\"a \"\"b\"\"\",c"));
        v.set_literal_items(&["a".into(), "b".into()]);
        assert_eq!((v.f1.as_str(), &v.list12), ("\"a,b\"", &None));
    }

    #[test]
    fn holes() {
        assert_eq!(subtract(r(0, 0, 9, 3), r(20, 0, 22, 0)), [r(0, 0, 9, 3)]);
        assert_eq!(subtract(r(0, 0, 9, 3), r(0, 0, 9, 3)), []);
        assert_eq!(
            subtract(r(0, 0, 9, 3), r(2, 1, 4, 1)),
            [r(0, 0, 1, 3), r(2, 0, 4, 0), r(2, 2, 4, 3), r(5, 0, 9, 3)]
        );
        let mut list = vec![
            DataValidation { ranges: vec![r(0, 0, 4, 0)], f1: "B1".into(), ..Default::default() },
            DataValidation { ranges: vec![r(8, 0, 8, 0)], ..Default::default() },
        ];
        // Tirar o começo muda a base: as fórmulas acompanham.
        clear(&mut list, r(0, 0, 1, 0), &|f, dr, dc| format!("{f}+{dr},{dc}"));
        assert_eq!(list[0].ranges, [r(2, 0, 4, 0)]);
        assert_eq!(list[0].f1, "B1+2,0");
        clear(&mut list, r(8, 0, 8, 5), &|f, _, _| f.to_string());
        assert_eq!(list.len(), 1);
        assert!(at(&list, 3, 0).is_some() && at(&list, 1, 0).is_none());
    }
}
