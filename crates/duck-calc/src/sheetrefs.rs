//! Nomes de aba dentro das fórmulas: o que muda quando uma aba é renomeada,
//! excluída ou copiada. Funciona no texto A1 e nos modelos relativos (o nome
//! da aba fica igual nos dois).
//!
//! - `Velha!A1`, `'Minha aba'!A1:B2`, `Jan:Mar!A1` (3D) e `Aba!Nome` mudam.
//! - Referências a outro arquivo (`[1]Aba!A1`) ficam como estão.
//! - Uma referência a uma aba excluída vira `#REF!` inteira; numa 3D, a ponta
//!   excluída passa para a vizinha que fica dentro do intervalo.

use std::collections::HashMap;

use super::{
    M_CELL, M_COL, M_END, M_ROW, is_ident_char, is_ident_start, parse_cell, skip_brackets, skip_quoted,
};

/// Abas que mudaram: nome antigo → nome novo (`None` = excluída).
#[derive(Clone, Debug, Default)]
pub struct SheetMap {
    map: HashMap<String, Option<String>>,
    /// Ordem das abas antes da mudança (pontas de referências 3D).
    order: Vec<String>,
}

impl SheetMap {
    pub fn rename(old: &str, new: &str) -> Self {
        let mut m = Self::default();
        m.add(old, Some(new));
        m
    }

    pub fn delete(name: &str, order: &[&str]) -> Self {
        let mut m = Self { order: order.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        m.add(name, None);
        m
    }

    pub fn with_order(order: &[&str]) -> Self {
        Self { order: order.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    /// Acrescenta uma troca (`None` = excluída). Nome igual não conta.
    pub fn add(&mut self, old: &str, new: Option<&str>) {
        if new != Some(old) {
            self.map.insert(old.to_lowercase(), new.map(str::to_string));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// O que aconteceu com a aba `name`: `None` = nada; `Some(None)` = excluída.
    pub fn name(&self, name: &str) -> Option<Option<&str>> {
        self.map.get(&name.to_lowercase()).map(|o| o.as_deref())
    }

    /// Algum dos nomes mudados aparece no texto (triagem rápida)?
    pub fn mentions(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        self.map.keys().any(|k| lower.contains(k.as_str()) || lower.contains(&k.replace('\'', "''")))
    }

    /// A fórmula com os nomes trocados, se mudar.
    pub fn apply(&self, formula: &str) -> Option<String> {
        if self.map.is_empty() || !formula.contains('!') {
            return None;
        }
        let b = formula.as_bytes();
        let mut out = String::new();
        let mut from = 0;
        let mut i = 0;
        // O que veio logo antes é de outro arquivo (`[1]`).
        let mut external = false;
        while i < b.len() {
            let c = b[i];
            if c == b'"' {
                i = skip_quoted(b, i, b'"');
                external = false;
                continue;
            }
            if c == b'[' {
                i = skip_brackets(b, i);
                external = true;
                continue;
            }
            if c == b'#' {
                i = skip_error(b, i);
                external = false;
                continue;
            }
            if let Some(end) = marker_end(formula, i) {
                i = end;
                external = false;
                continue;
            }
            if c == b'\'' {
                let end = skip_quoted(b, i, b'\'');
                if b.get(end) == Some(&b'!') {
                    let name = formula[i + 1..end - 1].replace("''", "'");
                    if !external && !name.contains([']', '[']) {
                        let names: Vec<&str> = name.splitn(2, ':').collect();
                        i = self.emit(formula, &names, (i, end + 1), &mut from, &mut out);
                        external = false;
                        continue;
                    }
                }
                i = end;
                external = false;
                continue;
            }
            if is_ident_start(c) {
                let end = ident_end(formula, i);
                let after_bang = i > 0 && b[i - 1] == b'!';
                if b.get(end) == Some(&b'!') {
                    if !external {
                        i = self.emit(formula, &[&formula[i..end]], (i, end + 1), &mut from, &mut out);
                        continue;
                    }
                } else if b.get(end) == Some(&b':')
                    && !external
                    && !after_bang
                    && parse_cell(&b[i..end]).is_none()
                {
                    let e2 = ident_end(formula, end + 1);
                    if e2 > end + 1 && b.get(e2) == Some(&b'!') {
                        let names = [&formula[i..end], &formula[end + 1..e2]];
                        i = self.emit(formula, &names, (i, e2 + 1), &mut from, &mut out);
                        continue;
                    }
                }
                i = end;
                external = false;
                continue;
            }
            // Um caractere qualquer (operadores, espaços, dígitos soltos).
            external = false;
            i += 1;
        }
        if from == 0 && out.is_empty() {
            return None;
        }
        out.push_str(&formula[from..]);
        (out != formula).then_some(out)
    }

    /// Escreve o qualificador `names` (o trecho `span`, com o `!`) trocado.
    /// Devolve a posição de onde continuar.
    fn emit(
        &self,
        formula: &str,
        names: &[&str],
        span: (usize, usize),
        from: &mut usize,
        out: &mut String,
    ) -> usize {
        let hits: Vec<Option<Option<&str>>> = names.iter().map(|n| self.name(n)).collect();
        if hits.iter().all(Option::is_none) {
            return span.1;
        }
        let new: Option<Vec<String>> = if names.len() == 1 {
            hits[0].flatten().map(|n| vec![n.to_string()])
        } else {
            self.span3d(names)
        };
        out.push_str(&formula[*from..span.0]);
        match new {
            Some(v) => {
                push_qualifier(&v, out);
                *from = span.1;
                span.1
            }
            None => {
                out.push_str("#REF!");
                *from = operand_end(formula, span.1);
                *from
            }
        }
    }

    /// As pontas de uma referência 3D depois da mudança (`None` = nenhuma aba
    /// do intervalo sobrou).
    fn span3d(&self, names: &[&str]) -> Option<Vec<String>> {
        let pos = |n: &str| self.order.iter().position(|o| o.to_lowercase() == n.to_lowercase());
        let alive = |n: &str| match self.name(n) {
            None => Some(n.to_string()),
            Some(x) => x.map(str::to_string),
        };
        if names.iter().all(|n| alive(n).is_some()) {
            let mut v: Vec<String> = names.iter().filter_map(|n| alive(n)).collect();
            v.dedup();
            return Some(v);
        }
        let (a, b) = (pos(names[0])?, pos(names[1])?);
        let (lo, hi) = (a.min(b), a.max(b));
        let first = self.order[lo..=hi].iter().find_map(|n| alive(n))?;
        let last = self.order[lo..=hi].iter().rev().find_map(|n| alive(n))?;
        let mut v = if a <= b { vec![first, last] } else { vec![last, first] };
        v.dedup();
        Some(v)
    }

    /// Lugar de um link (`Aba!A1`, `'Minha aba'!A1`; alguns programas gravam
    /// nomes com espaço sem aspas). `#` na frente fica.
    pub fn apply_location(&self, loc: &str) -> Option<String> {
        let (hash, rest) = match loc.strip_prefix('#') {
            Some(r) => ("#", r),
            None => ("", loc),
        };
        let i = rest.rfind('!')?;
        let name = &rest[..i];
        let bare = name.strip_prefix('\'').and_then(|n| n.strip_suffix('\'')).map(|n| n.replace("''", "'"));
        let hit = self.name(bare.as_deref().unwrap_or(name))?;
        let mut out = hash.to_string();
        match hit {
            Some(n) => {
                push_qualifier(&[n.to_string()], &mut out);
                out.push_str(&rest[i + 1..]);
            }
            None => out.push_str("#REF!"),
        }
        Some(out)
    }
}

/// `Aba!` ou `'Minha aba'!` (duas = 3D, `Jan:Mar!`).
fn push_qualifier(names: &[String], out: &mut String) {
    if names.iter().any(|n| needs_quotes(n)) {
        out.push('\'');
        for (k, n) in names.iter().enumerate() {
            if k > 0 {
                out.push(':');
            }
            out.push_str(&n.replace('\'', "''"));
        }
        out.push('\'');
    } else {
        out.push_str(&names.join(":"));
    }
    out.push('!');
}

/// O nome da aba como aparece numa fórmula: com aspas se precisar.
pub fn quoted(name: &str) -> String {
    let mut s = String::new();
    push_qualifier(&[name.to_string()], &mut s);
    s.pop();
    s
}

/// O nome precisa de aspas numa fórmula?
fn needs_quotes(n: &str) -> bool {
    let first = n.chars().next();
    if first.is_none_or(|c| !(c.is_alphabetic() || c == '_')) {
        return true;
    }
    if n.chars().any(|c| !(c.is_alphanumeric() || c == '_')) {
        return true;
    }
    if parse_cell(n.as_bytes()).is_some() {
        return true;
    }
    // `R`, `C`, `R1C1`, `L1C1`: parecem referências no outro estilo.
    let up = n.to_ascii_uppercase();
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    for row in ['R', 'L'] {
        if let Some(rest) = up.strip_prefix(row) {
            match rest.split_once('C') {
                Some((a, b)) if digits(a) && digits(b) => return true,
                None if digits(rest) => return true,
                _ => {}
            }
        }
    }
    up.strip_prefix('C').is_some_and(digits) || up == "TRUE" || up == "FALSE"
}

/// Depois de um erro (`#REF!`, `#N/A`).
fn skip_error(b: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'/' | b'_')) {
        i += 1;
    }
    if i < b.len() && matches!(b[i], b'!' | b'?') {
        i += 1;
    }
    i
}

/// Fim de uma referência de modelo relativo que começa em `i`.
fn marker_end(s: &str, i: usize) -> Option<usize> {
    if !s.is_char_boundary(i) || !s[i..].starts_with([M_CELL, M_COL, M_ROW]) {
        return None;
    }
    s[i..].find(M_END).map(|k| i + k + M_END.len_utf8())
}

/// Fim do identificador que começa em `i` (para nos marcadores de modelo).
fn ident_end(s: &str, i: usize) -> usize {
    let b = s.as_bytes();
    let mut j = i;
    while j < b.len() && is_ident_char(b[j]) {
        if s.is_char_boundary(j) && s[j..].starts_with([M_CELL, M_COL, M_ROW, M_END]) {
            break;
        }
        j += 1;
    }
    j
}

/// Fim do operando depois de `Aba!`: célula, intervalo, nome, modelo.
fn operand_end(s: &str, i: usize) -> usize {
    let b = s.as_bytes();
    let one = |mut i: usize| -> usize {
        // A outra ponta pode repetir a aba (`Aba!A1:Aba!B2`).
        if b.get(i) == Some(&b'\'') {
            let e = skip_quoted(b, i, b'\'');
            if b.get(e) == Some(&b'!') {
                i = e + 1;
            }
        }
        if let Some(e) = marker_end(s, i) {
            return e;
        }
        if b.get(i) == Some(&b'#') {
            return skip_error(b, i);
        }
        let e = ident_end(s, i);
        if b.get(e) == Some(&b'!') {
            return marker_end(s, e + 1).unwrap_or_else(|| ident_end(s, e + 1));
        }
        e
    };
    let mut e = one(i);
    if b.get(e) == Some(&b':') {
        let e2 = one(e + 1);
        if e2 > e + 1 {
            e = e2;
        }
    }
    if b.get(e) == Some(&b'[') {
        e = skip_brackets(b, e);
    }
    // Operador de matriz dinâmica (`Aba!A1#`).
    if b.get(e) == Some(&b'#') && !b.get(e + 1).is_some_and(u8::is_ascii_alphabetic) {
        e += 1;
    }
    e
}

/// Troca nomes de tabela (nome antigo em minúsculas → novo) numa fórmula:
/// `Tabela1[Coluna]`, `ROWS(Tabela1)`.
pub fn rename_tables(formula: &str, map: &HashMap<String, String>) -> Option<String> {
    if map.is_empty() {
        return None;
    }
    let b = formula.as_bytes();
    let mut out = String::new();
    let mut from = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => i = skip_quoted(b, i, b'"'),
            b'\'' => i = skip_quoted(b, i, b'\''),
            b'[' => i = skip_brackets(b, i),
            b'#' => i = skip_error(b, i),
            c if is_ident_start(c) => {
                if let Some(e) = marker_end(formula, i) {
                    i = e;
                    continue;
                }
                let e = ident_end(formula, i);
                if e == i {
                    i += 1;
                    continue;
                }
                let qualified = i > 0 && b[i - 1] == b'!';
                if !qualified
                    && !matches!(b.get(e), Some(b'!' | b'('))
                    && let Some(n) = map.get(&formula[i..e].to_lowercase())
                {
                    out.push_str(&formula[from..i]);
                    out.push_str(n);
                    from = e;
                }
                i = e;
            }
            _ => i += 1,
        }
    }
    if from == 0 {
        return None;
    }
    out.push_str(&formula[from..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename() {
        let m = SheetMap::rename("Velha", "Nova aba");
        let t = |f: &str| m.apply(f);
        assert_eq!(t("Velha!A1+velha!$B$2").as_deref(), Some("'Nova aba'!A1+'Nova aba'!$B$2"));
        assert_eq!(t("SUM('Velha'!A1:B2)").as_deref(), Some("SUM('Nova aba'!A1:B2)"));
        assert_eq!(t("Velha!A1:Velha!B3").as_deref(), Some("'Nova aba'!A1:'Nova aba'!B3"));
        assert_eq!(t("SUM(Jan:Velha!A1)").as_deref(), Some("SUM('Jan:Nova aba'!A1)"));
        assert_eq!(t("Velha!Nome*2").as_deref(), Some("'Nova aba'!Nome*2"));
        assert_eq!(t("\"Velha!A1\"&A1"), None);
        assert_eq!(t("[1]Velha!A1+'[2]Velha'!A1"), None);
        assert_eq!(t("Velha2!A1+MinhaVelha!A1"), None);
        assert_eq!(t("Tabela1[Velha!]"), None);
        let m = SheetMap::rename("Minha aba", "Dados");
        assert_eq!(m.apply("'Minha aba'!A1*2").as_deref(), Some("Dados!A1*2"));
        let m = SheetMap::rename("D'água", "Água");
        assert_eq!(m.apply("'D''água'!A1").as_deref(), Some("Água!A1"));
        let m = SheetMap::rename("Ação", "AB12");
        assert_eq!(m.apply("IF(AÇÃO!A1>0,1,0)").as_deref(), Some("IF('AB12'!A1>0,1,0)"));
    }

    #[test]
    fn rename_in_templates() {
        let t = super::super::to_template("Velha!A1*B2+SUM(Velha!$A$1:A3)", 1, 1).unwrap();
        let m = SheetMap::rename("Velha", "Nova");
        let new = m.apply(&t).unwrap();
        assert_eq!(super::super::from_template(&new, 1, 1), "Nova!A1*B2+SUM(Nova!$A$1:A3)");
        let m = SheetMap::delete("Velha", &[]);
        let new = m.apply(&t).unwrap();
        assert_eq!(super::super::from_template(&new, 1, 1), "#REF!*B2+SUM(#REF!)");
    }

    #[test]
    fn delete() {
        let m = SheetMap::delete("Velha", &["Jan", "Velha", "Mar", "Abr"]);
        let t = |f: &str| m.apply(f);
        assert_eq!(t("Velha!A1+1").as_deref(), Some("#REF!+1"));
        assert_eq!(t("SUM(Velha!A1:B2,A1)").as_deref(), Some("SUM(#REF!,A1)"));
        assert_eq!(t("'Velha'!$A:$C").as_deref(), Some("#REF!"));
        assert_eq!(t("Velha!A1:Velha!B2*3").as_deref(), Some("#REF!*3"));
        assert_eq!(t("Velha!A1#").as_deref(), Some("#REF!"));
        assert_eq!(t("Velha!#REF!").as_deref(), Some("#REF!"));
        assert_eq!(t("SUM(Jan:Velha!A1)").as_deref(), Some("SUM(Jan!A1)"));
        assert_eq!(t("SUM(Velha:Abr!A1)").as_deref(), Some("SUM(Mar:Abr!A1)"));
        assert_eq!(t("SUM(Jan:Abr!A1)"), None);
        assert_eq!(t("#REF!+Jan!A1"), None);
    }

    #[test]
    fn links_and_quotes() {
        let m = SheetMap::rename("Minha aba", "X");
        assert_eq!(m.apply_location("#'Minha aba'!A1").as_deref(), Some("#X!A1"));
        assert_eq!(m.apply_location("Minha aba!A1").as_deref(), Some("X!A1"));
        assert_eq!(m.apply_location("Outra!A1"), None);
        assert_eq!(quoted("Dados"), "Dados");
        assert_eq!(quoted("R1C1"), "'R1C1'");
        assert_eq!(quoted("D'água"), "'D''água'");
        assert_eq!(quoted("2026"), "'2026'");
        assert_eq!(quoted("Ação"), "Ação");
    }

    #[test]
    fn tables() {
        let map = [("tabela1".to_string(), "Tabela13".to_string())].into_iter().collect();
        assert_eq!(
            rename_tables("SUM(Tabela1[Valor])/ROWS(TABELA1)", &map).as_deref(),
            Some("SUM(Tabela13[Valor])/ROWS(Tabela13)")
        );
        assert_eq!(rename_tables("Tabela10[x]+Tabela1!A1+\"Tabela1\"", &map), None);
    }
}
