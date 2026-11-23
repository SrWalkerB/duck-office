//! Abas: nomes válidos, cor da guia e troca do texto das fórmulas quando uma
//! aba muda de nome ou é excluída.

use std::collections::HashSet;

use crate::{BLOCK_ROWS, Color, FormulaKind, SHARED_BIT, Sheet};

/// Tamanho máximo do nome de uma aba, em caracteres (UTF-16, como no Excel).
pub const MAX_NAME: usize = 31;

/// O nome serve para uma aba? Senão, o motivo (em português).
pub fn check_name(name: &str, others: &[&str]) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("O nome da aba não pode ficar em branco".into());
    }
    if name.encode_utf16().count() > MAX_NAME {
        return Err(format!("O nome da aba pode ter até {MAX_NAME} caracteres"));
    }
    if let Some(c) = name.chars().find(|c| matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')) {
        return Err(format!("O nome da aba não pode ter o caractere {c}"));
    }
    if name.chars().any(char::is_control) {
        return Err("O nome da aba não pode ter quebras de linha".into());
    }
    if name.starts_with('\'') || name.ends_with('\'') {
        return Err("O nome da aba não pode começar nem terminar com apóstrofo".into());
    }
    // Reservado pelo Excel (controle de alterações).
    if same_name(name, "History") || same_name(name, "Histórico") {
        return Err(format!("O nome \"{name}\" é reservado"));
    }
    if others.iter().any(|o| same_name(o, name)) {
        return Err(format!("Já existe uma aba com o nome \"{name}\""));
    }
    Ok(())
}

/// O Excel não diferencia maiúsculas nos nomes das abas.
pub fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Nome da próxima aba nova (`Planilha1`, `Planilha2`...).
pub fn next_name(others: &[&str]) -> String {
    (1..)
        .map(|n| format!("Planilha{n}"))
        .find(|n| !others.iter().any(|o| same_name(o, n)))
        .unwrap_or_default()
}

/// Nome da cópia de uma aba: `Nome (2)`, `Nome (3)`... (cortado para caber).
pub fn copy_name(name: &str, others: &[&str]) -> String {
    // Cópia de uma cópia: continua a numeração.
    let base = match name.rsplit_once(" (") {
        Some((b, n)) if n.strip_suffix(')').is_some_and(|d| d.parse::<u32>().is_ok()) => b,
        _ => name,
    };
    (2..)
        .map(|n| {
            let suffix = format!(" ({n})");
            let room = MAX_NAME - suffix.len();
            let mut used = 0;
            let cut: String = base
                .chars()
                .take_while(|c| {
                    used += c.len_utf16();
                    used <= room
                })
                .collect();
            format!("{}{suffix}", cut.trim_end())
        })
        .find(|n| !others.iter().any(|o| same_name(o, n)))
        .unwrap_or_default()
}

impl crate::Layout {
    /// Troca os caminhos de parte dos objetos, tabelas e dinâmicas (as
    /// imagens ficam de fora: são usadas por várias abas).
    pub fn map_parts(&mut self, f: &mut dyn FnMut(&str) -> Option<String>) {
        use crate::drawing::ObjectKind;
        let mut fix = |s: &mut String| {
            if let Some(n) = f(s) {
                *s = n;
            }
        };
        self.tables.iter_mut().for_each(|t| fix(&mut t.part));
        self.pivots.iter_mut().for_each(|p| fix(&mut p.part));
        self.drawing_anchors.iter_mut().for_each(|a| fix(&mut a.part));
        for o in &mut self.objects {
            match &mut o.kind {
                ObjectKind::Chart(c) => fix(&mut c.part),
                ObjectKind::Control(c) => fix(&mut c.part),
                ObjectKind::Timeline(t) => fix(&mut t.part),
                ObjectKind::Slicer(s) => {
                    if let Some(p) = &mut s.pivot {
                        fix(&mut p.part);
                    }
                }
                ObjectKind::Shape(_) | ObjectKind::Picture { .. } => {}
            }
        }
    }
}

impl Sheet {
    /// As definições de fórmula que `f` muda: (índice, texto novo). O texto
    /// pode ser um modelo relativo (`f` só deve mexer em nomes de aba e de
    /// tabela, que são iguais nos dois).
    pub fn formula_text_changes(&self, f: &dyn Fn(&str) -> Option<String>) -> Vec<(u32, String)> {
        let mut out = Vec::new();
        for (i, d) in self.formula_defs.iter().enumerate() {
            if let Some(t) = self.formula_texts.get(d.text)
                && let Some(new) = f(t)
            {
                out.push((i as u32, new));
            }
        }
        out
    }

    /// Troca o texto das definições; devolve os textos de antes. As células
    /// que usam essas definições são regravadas ao salvar.
    pub fn set_formula_texts(&mut self, list: Vec<(u32, String)>) -> Vec<(u32, String)> {
        let mut old = Vec::with_capacity(list.len());
        let mut ids: HashSet<u32> = HashSet::with_capacity(list.len());
        for (i, text) in list {
            let Some(d) = self.formula_defs.get(i as usize).copied() else { continue };
            old.push((i, self.formula_texts.get(d.text).unwrap_or_default().to_string()));
            let t = self.formula_texts.push(&text);
            self.formula_defs[i as usize].text = t;
            ids.insert(i + 1);
            // Os membros do grupo leem o texto da mestre.
            if let FormulaKind::Shared { si } = d.kind
                && self.shared_groups.get(&(si & !SHARED_BIT)) == Some(&i)
            {
                ids.insert(SHARED_BIT | (si & !SHARED_BIT));
                self.edits.retexted.insert(si);
            }
        }
        self.mark_formula_ids(&ids);
        old
    }

    /// Marca para regravar as células com um destes ids de fórmula. Poucas
    /// são marcadas uma a uma; muitas viram faixas de linhas inteiras.
    fn mark_formula_ids(&mut self, ids: &HashSet<u32>) {
        const MARK_LIMIT: usize = 50_000;
        if ids.is_empty() {
            return;
        }
        let mut cells: Vec<(u32, u32)> = Vec::new();
        let mut rows: Vec<bool> = Vec::new();
        let mut total = 0usize;
        for (c, col) in self.columns.iter().enumerate() {
            for (bi, b) in col.formulas.iter().enumerate() {
                let Some(b) = b else { continue };
                for i in 0..BLOCK_ROWS {
                    if !ids.contains(&b.get(i)) {
                        continue;
                    }
                    let r = bi * BLOCK_ROWS + i;
                    total += 1;
                    if cells.len() < MARK_LIMIT {
                        cells.push((r as u32, c as u32));
                    }
                    if rows.len() <= r {
                        rows.resize(r + 1, false);
                    }
                    rows[r] = true;
                }
            }
        }
        if total <= MARK_LIMIT {
            for (r, c) in cells {
                self.edits.mark(r, c);
            }
            return;
        }
        let mut r = 0;
        while r < rows.len() {
            if !rows[r] {
                r += 1;
                continue;
            }
            let a = r;
            while r < rows.len() && rows[r] {
                r += 1;
            }
            self.edits.rewrite.push((a as u32, r as u32 - 1));
        }
    }

    /// Cor da guia.
    pub fn tab_color(&self) -> Option<Color> {
        self.layout.view.and_then(|v| v.tab_color)
    }

    pub(crate) fn set_tab_color(&mut self, c: Option<Color>) -> Option<Color> {
        let mut v = self.layout.view.unwrap_or_default();
        let old = std::mem::replace(&mut v.tab_color, c);
        self.layout.view = Some(v);
        self.edits.tab_color = true;
        old
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, SheetPart, kind};

    #[test]
    fn names() {
        assert!(check_name("Vendas 2026", &["Custos"]).is_ok());
        for bad in ["", "  ", "a/b", "a:b", "[x]", "'x", "x'", "History", "custos"] {
            assert!(check_name(bad, &["Custos"]).is_err(), "{bad}");
        }
        assert!(check_name(&"a".repeat(31), &[]).is_ok());
        assert!(check_name(&"a".repeat(32), &[]).is_err());
        assert_eq!(next_name(&["Planilha1", "planilha2"]), "Planilha3");
        assert_eq!(copy_name("Vendas", &["Vendas"]), "Vendas (2)");
        assert_eq!(copy_name("Vendas (2)", &["Vendas", "Vendas (2)"]), "Vendas (3)");
        let long = "x".repeat(31);
        let c = copy_name(&long, &[&long]);
        assert_eq!(c.len(), 31);
        assert!(c.ends_with(" (2)"));
    }

    #[test]
    fn formula_texts_undo() {
        let mut p = SheetPart::new();
        p.set(0, 0, kind::NUMBER | kind::FORMULA, 0, 1f64.to_bits());
        p.set_formula(0, 0, "Velha!B1*2", FormulaKind::Shared { si: 3 });
        p.set(1, 0, kind::NUMBER | kind::FORMULA, 0, 1f64.to_bits());
        p.set_shared_formula(1, 0, 3);
        p.set(2, 0, kind::NUMBER | kind::FORMULA, 0, 1f64.to_bits());
        p.set_formula(2, 0, "B1", FormulaKind::Normal);
        let mut s = Sheet::new("t");
        s.merge_part(p);
        s.finish();
        assert!(s.shared_intact(1, 0));
        let list = s.formula_text_changes(&|t| t.contains("Velha").then(|| t.replace("Velha", "Nova")));
        assert_eq!(list.len(), 1);
        let undo = Command::FormulaTexts(list).apply(&mut s);
        assert_eq!(s.formula(1, 0).unwrap().text, "Nova!B1*2");
        assert!(!s.shared_intact(0, 0) && !s.shared_intact(1, 0));
        assert!(s.edits().has(0, 0) && s.edits().has(1, 0) && !s.edits().has(2, 0));
        undo.apply(&mut s);
        assert_eq!(s.formula(0, 0).unwrap().text, "Velha!B1*2");
        let c = Command::TabColor(Some(Color::Rgb(0xFF00_B050))).apply(&mut s);
        assert_eq!(s.tab_color(), Some(Color::Rgb(0xFF00_B050)));
        c.apply(&mut s);
        assert_eq!(s.tab_color(), None);
    }
}
