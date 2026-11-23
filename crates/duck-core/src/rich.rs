//! Texto rico (várias fontes numa célula) depois de editar o texto: os
//! trechos que não mudaram continuam com a fonte deles.

use crate::{RichRun, Sheet, StringTable, kind};

/// Os trechos de `runs` (do texto `old`) aplicados ao texto `new`: o começo e
/// o fim iguais guardam as fontes; o que foi digitado no meio herda a do
/// caractere de antes. `None` = não sobrou fonte própria.
pub fn rebase(old: &str, runs: &[RichRun], new: &str) -> Option<Vec<RichRun>> {
    if runs.is_empty() || new.is_empty() {
        return None;
    }
    let pre: usize =
        old.chars().zip(new.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
    let room = (old.len() - pre).min(new.len() - pre);
    let mut suf = 0;
    for (a, b) in old.chars().rev().zip(new.chars().rev()) {
        if a != b || suf + a.len_utf8() > room {
            break;
        }
        suf += a.len_utf8();
    }
    let font_at = |byte: usize| {
        let k = runs.partition_point(|r| r.start as usize <= byte);
        k.checked_sub(1).and_then(|k| runs[k].font.clone())
    };
    let cut_old = old.len() - suf;
    let cut_new = new.len() - suf;
    let mut out: Vec<RichRun> = Vec::new();
    let mut push = |start: usize, font: Option<crate::RunFont>| {
        if out.last().is_none_or(|r| r.font != font) {
            out.push(RichRun { start: start as u32, font });
        }
    };
    // Começo igual.
    for (k, r) in runs.iter().enumerate() {
        if (r.start as usize) < pre {
            push(if k == 0 { 0 } else { r.start as usize }, r.font.clone());
        }
    }
    // O que foi digitado.
    if cut_new > pre {
        let font = match pre.checked_sub(1) {
            Some(b) => font_at(b),
            None => font_at(0),
        };
        push(pre, font);
    }
    // Fim igual.
    if suf > 0 {
        push(cut_new, font_at(cut_old));
        for r in runs.iter().filter(|r| r.start as usize > cut_old) {
            push(r.start as usize - cut_old + cut_new, r.font.clone());
        }
    }
    out.iter().any(|r| r.font.is_some()).then_some(out)
}

impl Sheet {
    /// Os trechos de texto rico da célula, se ela tiver.
    pub fn rich_at<'a>(&'a self, row: u32, col: u32, shared: &'a StringTable) -> Option<&'a [RichRun]> {
        let raw = self.raw(row, col);
        match raw.kind & kind::TYPE_MASK {
            kind::SHARED_STR => shared.rich(raw.payload as u32),
            kind::LOCAL_STR => self.strings.get((raw.payload >> 32) as usize)?.rich(raw.payload as u32),
            _ => None,
        }
    }

    /// Guarda os trechos de texto rico de uma célula de texto editada (cada
    /// texto digitado tem a sua entrada na tabela de strings das edições).
    pub(crate) fn set_rich_runs(&mut self, row: u32, col: u32, runs: Vec<RichRun>) {
        let raw = self.raw(row, col);
        if raw.kind & kind::TYPE_MASK != kind::LOCAL_STR {
            return;
        }
        let t = (raw.payload >> 32) as usize;
        if self.edit_table != Some(t) {
            return;
        }
        self.strings[t].set_rich(raw.payload as u32, runs);
        self.edits.mark(row, col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellChange, CellValue, Command, RunFont};

    fn bold() -> Option<RunFont> {
        Some(RunFont { bold: true, ..Default::default() })
    }

    fn runs() -> Vec<RichRun> {
        // "ab|CD|ef" com CD em negrito.
        vec![
            RichRun { start: 0, font: None },
            RichRun { start: 2, font: bold() },
            RichRun { start: 4, font: None },
        ]
    }

    fn spans(r: &[RichRun]) -> Vec<(u32, bool)> {
        r.iter().map(|r| (r.start, r.font.is_some())).collect()
    }

    #[test]
    fn edits_keep_fonts() {
        let t = |new: &str| rebase("abCDef", &runs(), new).map(|r| spans(&r));
        // Sem mudança.
        assert_eq!(t("abCDef"), Some(vec![(0, false), (2, true), (4, false)]));
        // Digitar no meio do negrito.
        assert_eq!(t("abCxDef"), Some(vec![(0, false), (2, true), (5, false)]));
        // Digitar antes e no fim.
        assert_eq!(t("zzabCDef"), Some(vec![(0, false), (4, true), (6, false)]));
        assert_eq!(t("abCDef!"), Some(vec![(0, false), (2, true), (4, false)]));
        // Quebra de linha no meio.
        assert_eq!(t("ab\nCDef"), Some(vec![(0, false), (3, true), (5, false)]));
        // Apagar o negrito todo.
        assert_eq!(t("abef"), None);
        assert_eq!(t(""), None);
        // Acentos: posições em bytes.
        let r = vec![RichRun { start: 0, font: bold() }, RichRun { start: 3, font: None }];
        assert_eq!(rebase("aç é", &r, "aç, é").map(|r| spans(&r)), Some(vec![(0, true), (4, false)]));
    }

    #[test]
    fn command_round_trip() {
        let shared = StringTable::new();
        let mut s = Sheet::new("t");
        let cell = CellChange {
            row: 0,
            col: 0,
            value: CellValue::Text("abCDef".into()),
            formula: None,
            style: None,
        };
        let cmd =
            Command::Group(vec![Command::SetCells(vec![cell]), Command::RichText(vec![(0, 0, runs())])]);
        let undo = cmd.apply(&mut s);
        assert_eq!(s.rich_at(0, 0, &shared).map(spans), Some(vec![(0, false), (2, true), (4, false)]));
        let redo = undo.apply(&mut s);
        assert!(s.rich_at(0, 0, &shared).is_none());
        redo.apply(&mut s);
        assert!(s.rich_at(0, 0, &shared).is_some());
        assert_eq!(s.cell(0, 0, &shared).value, crate::Value::Text("abCDef"));
    }
}
