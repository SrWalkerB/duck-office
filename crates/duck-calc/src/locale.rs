//! Fórmulas em português: o arquivo guarda `SUM(A1,1.5)`; na tela e ao
//! digitar é `SOMA(A1;1,5)`.
//!
//! A tradução troca nomes de função, separador de argumentos, vírgula
//! decimal, VERDADEIRO/FALSO e os erros (`#N/D`), sem mexer em textos entre
//! aspas, nomes de aba entre apóstrofos e referências de tabela.

use crate::funcs;

const ERRORS: [(&str, &str); 7] = [
    ("#N/A", "#N/D"),
    ("#VALUE!", "#VALOR!"),
    ("#NAME?", "#NOME?"),
    ("#NUM!", "#NÚM!"),
    ("#NULL!", "#NULO!"),
    ("#REF!", "#REF!"),
    ("#DIV/0!", "#DIV/0!"),
];

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\\' | '?')
}

/// Copia um trecho entre `q` (com `qq` como escape) de `s[i..]`.
fn quoted(s: &[char], i: usize, q: char, out: &mut String) -> usize {
    out.push(q);
    let mut j = i + 1;
    while j < s.len() {
        out.push(s[j]);
        if s[j] == q {
            if s.get(j + 1) == Some(&q) {
                out.push(q);
                j += 2;
                continue;
            }
            return j + 1;
        }
        j += 1;
    }
    j
}

/// Copia `[...]` (aninhado; `'` escapa) de `s[i..]`.
fn bracketed(s: &[char], i: usize, out: &mut String) -> usize {
    let mut depth = 0;
    let mut j = i;
    while j < s.len() {
        let c = s[j];
        out.push(c);
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            '\'' => {
                if let Some(&n) = s.get(j + 1) {
                    out.push(n);
                    j += 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    j
}

/// Fórmula do arquivo (sem `=`) → como o Excel em português mostra.
pub fn to_pt(f: &str) -> String {
    translate(f, true)
}

/// Fórmula digitada em português (sem `=`) → como o arquivo guarda.
/// Nomes de função em inglês também são aceitos.
pub fn from_pt(f: &str) -> String {
    translate(f, false)
}

fn translate(f: &str, to_pt: bool) -> String {
    let s: Vec<char> = f.chars().collect();
    let mut out = String::with_capacity(f.len() + 8);
    let mut i = 0;
    // Dentro de `{...}` o separador de colunas também troca.
    while i < s.len() {
        let c = s[i];
        match c {
            '"' => i = quoted(&s, i, '"', &mut out),
            '\'' => i = quoted(&s, i, '\'', &mut out),
            '[' => i = bracketed(&s, i, &mut out),
            '#' => {
                let rest: String = s[i..].iter().take(8).collect();
                let rest_up = rest.to_uppercase();
                let mut done = false;
                for (en, pt) in ERRORS {
                    let (from, to) = if to_pt { (en, pt) } else { (pt, en) };
                    if rest_up.starts_with(from) {
                        out.push_str(to);
                        i += from.chars().count();
                        done = true;
                        break;
                    }
                }
                if !done {
                    out.push(c);
                    i += 1;
                }
            }
            ',' if to_pt => {
                out.push(';');
                i += 1;
            }
            ';' if !to_pt => {
                out.push(',');
                i += 1;
            }
            _ if c.is_ascii_digit()
                || (c == '.' && to_pt && s.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                // Número: troca o separador decimal.
                let st = i;
                while i < s.len() && (s[i].is_ascii_digit() || s[i] == '.' || s[i] == ',') {
                    let ch = s[i];
                    let dec_in = if to_pt { '.' } else { ',' };
                    if ch == dec_in && s.get(i + 1).is_some_and(char::is_ascii_digit) {
                        out.push(if to_pt { ',' } else { '.' });
                    } else if ch == '.' || ch == ',' {
                        break;
                    } else {
                        out.push(ch);
                    }
                    i += 1;
                }
                // Expoente.
                if i < s.len() && (s[i] == 'E' || s[i] == 'e') && i > st {
                    let mut j = i + 1;
                    if j < s.len() && (s[j] == '+' || s[j] == '-') {
                        j += 1;
                    }
                    if j < s.len() && s[j].is_ascii_digit() {
                        while j < s.len() && s[j].is_ascii_digit() {
                            j += 1;
                        }
                        out.extend(&s[i..j]);
                        i = j;
                    }
                }
                // Parte de uma referência (`1:1`) ou identificador (`A1` já
                // foi pego como identificador abaixo): nada mais a fazer.
                if i < s.len() && is_ident(s[i]) && s[i] != '.' && s[i] != '$' {
                    // Dígito colado em letras: é um nome; copia o resto.
                    while i < s.len() && is_ident(s[i]) {
                        out.push(s[i]);
                        i += 1;
                    }
                }
            }
            _ if is_ident(c) => {
                let st = i;
                while i < s.len() && is_ident(s[i]) {
                    i += 1;
                }
                let word: String = s[st..i].iter().collect();
                let mut j = i;
                while j < s.len() && s[j] == ' ' {
                    j += 1;
                }
                if s.get(j) == Some(&'(') {
                    out.push_str(&func_name(&word, to_pt));
                } else if s.get(i) != Some(&'!') && s.get(i) != Some(&'[') {
                    let up = word.to_uppercase();
                    let b = if to_pt {
                        match up.as_str() {
                            "TRUE" => Some("VERDADEIRO"),
                            "FALSE" => Some("FALSO"),
                            _ => None,
                        }
                    } else {
                        match up.as_str() {
                            "VERDADEIRO" => Some("TRUE"),
                            "FALSO" => Some("FALSE"),
                            _ => None,
                        }
                    };
                    match b {
                        Some(b) => out.push_str(b),
                        // Referência digitada em minúsculas (`a1`) vai em maiúsculas.
                        None if !to_pt && crate::parse::parse_cell(&word).is_some() => {
                            out.push_str(&word.to_uppercase())
                        }
                        None => out.push_str(&word),
                    }
                } else {
                    out.push_str(&word);
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn func_name(word: &str, to_pt: bool) -> String {
    // Prefixos de funções novas (`_xlfn.`) somem na tela e voltam ao gravar.
    if to_pt {
        match funcs::lookup(word) {
            Some(f) => funcs::def(f).pt.to_string(),
            None => word.to_string(),
        }
    } else {
        match funcs::lookup_pt(word).or_else(|| funcs::lookup(word)) {
            Some(f) => {
                let name = funcs::def(f).name;
                if NEW_FUNCS.contains(&name) { format!("_xlfn.{name}") } else { name.to_string() }
            }
            None => word.to_uppercase(),
        }
    }
}

/// Funções que o Excel grava com `_xlfn.` (novas depois do 2007).
const NEW_FUNCS: &[&str] = &[
    "IFS",
    "IFNA",
    "XOR",
    "SWITCH",
    "MINIFS",
    "MAXIFS",
    "CONCAT",
    "TEXTJOIN",
    "XLOOKUP",
    "XMATCH",
    "ISOWEEKNUM",
    "DAYS",
    "NUMBERVALUE",
    "STDEV.S",
    "STDEV.P",
    "VAR.S",
    "VAR.P",
    "RANK.EQ",
    "CEILING.MATH",
    "FLOOR.MATH",
    "UNICHAR",
    "UNICODE",
    "TEXTBEFORE",
    "TEXTAFTER",
    "FILTER",
    "UNIQUE",
    "SORT",
    "SORTBY",
    "SEQUENCE",
    "LET",
    "LAMBDA",
    "AGGREGATE",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let en =
            r#"IFERROR(VLOOKUP(A2,'Base X'!$A:$F,3,FALSE),"a,b")+SUM(1.5,T[[#This Row],[Valor, total]])"#;
        let pt = to_pt(en);
        assert_eq!(
            pt,
            r#"SEERRO(PROCV(A2;'Base X'!$A:$F;3;FALSO);"a,b")+SOMA(1,5;T[[#This Row],[Valor, total]])"#
        );
        assert_eq!(from_pt(&pt), en);
        assert_eq!(to_pt("_xlfn.XLOOKUP(1,A:A,B:B,#N/A)"), "PROCX(1;A:A;B:B;#N/D)");
        assert_eq!(from_pt("procx(1;A:A;B:B)"), "_xlfn.XLOOKUP(1,A:A,B:B)");
        assert_eq!(from_pt("SOMA(A1:A3)*1,5"), "SUM(A1:A3)*1.5");
        assert_eq!(from_pt("SE(A1>0;\"sim\";\"não\")"), "IF(A1>0,\"sim\",\"não\")");
        assert_eq!(to_pt("1E+5*2"), "1E+5*2");
        assert_eq!(from_pt("soma(a1:b2)"), "SUM(A1:B2)");
    }
}
