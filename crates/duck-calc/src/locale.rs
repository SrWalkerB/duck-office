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

/// Nomes que a fórmula define.
#[derive(Default)]
struct Vars {
    /// Variáveis de LET e parâmetros de LAMBDA (minúsculas, sem prefixo).
    names: std::collections::HashSet<String>,
    /// Parâmetros opcionais digitados (`[b]`): começo e fim do trecho.
    optional: std::collections::HashMap<usize, usize>,
}

/// Uma chamada aberta: nome (maiúsculas, inglês) e o argumento atual.
struct Call {
    name: &'static str,
    arg: usize,
    /// Até aqui só vieram nomes (parâmetros de LAMBDA).
    only_names: bool,
}

/// Nome de arquivo da função escrita em qualquer das línguas.
fn canonical(word: &str) -> &'static str {
    funcs::lookup_pt(word).or_else(|| funcs::lookup(word)).map_or("", |f| funcs::def(f).name)
}

/// Pula espaços a partir de `j`.
fn skip_ws(s: &[char], mut j: usize) -> usize {
    while j < s.len() && s[j] == ' ' {
        j += 1;
    }
    j
}

/// Variáveis de LET (nos argumentos pares, um nome seguido do separador) e
/// parâmetros de LAMBDA (os nomes antes do cálculo).
fn find_vars(s: &[char], sep: char) -> Vars {
    let mut out = Vars::default();
    let mut stack: Vec<Call> = Vec::new();
    let mut i = 0;
    let mut scratch = String::new();
    while i < s.len() {
        let c = s[i];
        match c {
            '"' | '\'' => {
                scratch.clear();
                i = quoted(s, i, c, &mut scratch);
            }
            '[' => {
                scratch.clear();
                let st = i;
                i = bracketed(s, i, &mut scratch);
                let name = scratch.trim_matches(['[', ']', ' ']);
                let param = stack.last().is_some_and(|t| t.name == "LAMBDA" && t.only_names)
                    && s.get(skip_ws(s, i)) == Some(&sep)
                    && !name.is_empty()
                    && name.chars().all(|c| is_ident(c) && c != '.');
                if param {
                    out.names.insert(name.to_lowercase());
                    out.optional.insert(st, i);
                }
            }
            '(' => {
                stack.push(Call { name: "", arg: 0, only_names: false });
                i += 1;
            }
            ')' => {
                stack.pop();
                i += 1;
            }
            _ if c == sep => {
                if let Some(top) = stack.last_mut() {
                    top.arg += 1;
                }
                i += 1;
            }
            _ if is_ident(c) && !c.is_ascii_digit() => {
                let st = i;
                while i < s.len() && is_ident(s[i]) {
                    i += 1;
                }
                let word: String = s[st..i].iter().collect();
                let j = skip_ws(s, i);
                if s.get(j) == Some(&'(') {
                    if let Some(top) = stack.last_mut() {
                        top.only_names = false;
                    }
                    stack.push(Call { name: canonical(&word), arg: 0, only_names: true });
                    i = j + 1;
                } else if let Some(top) = stack.last_mut() {
                    let named = s.get(j) == Some(&sep);
                    let var = match top.name {
                        "LET" => named && top.arg % 2 == 0,
                        "LAMBDA" => named && top.only_names,
                        _ => false,
                    };
                    if var {
                        out.names.insert(local(&word));
                    } else {
                        top.only_names = false;
                    }
                }
            }
            ' ' => i += 1,
            _ => {
                if let Some(top) = stack.last_mut() {
                    top.only_names = false;
                }
                i += 1;
            }
        }
    }
    out
}

/// Variável sem o prefixo do arquivo, em minúsculas.
fn local(w: &str) -> String {
    bare(w).to_lowercase()
}

fn bare(w: &str) -> &str {
    let mut w = w;
    for p in ["_xlpm.", "_XLPM.", "_xlop.", "_XLOP."] {
        w = w.strip_prefix(p).unwrap_or(w);
    }
    w
}

/// Nome de função no lugar de uma LAMBDA (`BYROW(A1:B3;SOMA)`,
/// `AGRUPARPOR(A:A;B:B;EMPILHARH(SOMA;MÉDIA))`).
fn as_value(stack: &[Call], word: &str, last_arg: bool) -> bool {
    let takes = |c: &Call, last: bool| match c.name {
        "BYROW" | "BYCOL" | "MAP" | "REDUCE" | "SCAN" | "MAKEARRAY" => c.arg >= 1 && last,
        "GROUPBY" => c.arg == 2,
        "PIVOTBY" => c.arg == 3,
        // LET(f;SOMA;...): nomes curtos (N, T, E, OU) são mais provavelmente
        // nomes definidos.
        "LET" => c.arg % 2 == 1 && word.chars().count() > 2,
        _ => false,
    };
    match stack {
        [.., up, c] if matches!(c.name, "HSTACK" | "VSTACK") => takes(up, false),
        [.., c] => takes(c, last_arg),
        [] => false,
    }
}

fn translate(f: &str, to_pt: bool) -> String {
    let s: Vec<char> = f.chars().collect();
    let vars = find_vars(&s, if to_pt { ',' } else { ';' });
    let mut out = String::with_capacity(f.len() + 8);
    let mut i = 0;
    let mut stack: Vec<Call> = Vec::new();
    let mut braces = 0u32;
    while i < s.len() {
        let c = s[i];
        // Dentro de `{...}` as colunas se separam com `,` no arquivo e `\`
        // em português; as linhas, com `;` nos dois.
        if braces > 0 {
            match (c, to_pt) {
                (',', true) => {
                    out.push('\\');
                    i += 1;
                    continue;
                }
                ('\\', false) => {
                    out.push(',');
                    i += 1;
                    continue;
                }
                (';', _) => {
                    out.push(';');
                    i += 1;
                    continue;
                }
                _ => {}
            }
        }
        match c {
            '{' | '}' => {
                braces = if c == '{' { braces + 1 } else { braces.saturating_sub(1) };
                out.push(c);
                i += 1;
            }
            '(' => {
                stack.push(Call { name: "", arg: 0, only_names: false });
                out.push(c);
                i += 1;
            }
            ')' => {
                stack.pop();
                out.push(c);
                i += 1;
            }
            '[' if vars.optional.contains_key(&i) => {
                // Parâmetro opcional de LAMBDA.
                let end = vars.optional[&i];
                let name: String = s[i + 1..end - 1].iter().collect();
                out.push_str("_xlop.");
                out.push_str(name.trim());
                i = end;
            }
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
                if let Some(top) = stack.last_mut() {
                    top.arg += 1;
                }
                out.push(';');
                i += 1;
            }
            ';' if !to_pt => {
                if let Some(top) = stack.last_mut() {
                    top.arg += 1;
                }
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
                if i < s.len()
                    && is_ident(s[i])
                    && !matches!(s[i], '.' | '$')
                    && (braces == 0 || s[i] != '\\')
                {
                    // Dígito colado em letras: é um nome; copia o resto.
                    while i < s.len() && is_ident(s[i]) {
                        out.push(s[i]);
                        i += 1;
                    }
                }
            }
            _ if is_ident(c) => {
                let st = i;
                while i < s.len() && is_ident(s[i]) && (braces == 0 || s[i] != '\\') {
                    i += 1;
                }
                let word: String = s[st..i].iter().collect();
                let j = skip_ws(&s, i);
                let var = !vars.names.is_empty()
                    && vars.names.contains(&local(&word))
                    && s.get(i) != Some(&'!')
                    && (s.get(j) != Some(&'(') || canonical(&word).is_empty());
                if var {
                    // Variável de LET ou LAMBDA: o arquivo grava `_xlpm.nome`.
                    let optional = word.len() > 6
                        && word.is_char_boundary(6)
                        && word[..6].eq_ignore_ascii_case("_xlop.");
                    match (to_pt, optional) {
                        (true, true) => {
                            out.push('[');
                            out.push_str(bare(&word));
                            out.push(']');
                        }
                        (true, false) => out.push_str(bare(&word)),
                        (false, true) => out.push_str(&word),
                        (false, false) => {
                            out.push_str("_xlpm.");
                            out.push_str(bare(&word));
                        }
                    }
                } else if s.get(j) == Some(&'(') {
                    out.push_str(&func_name(&word, to_pt));
                    out.extend(&s[i..=j]);
                    i = j + 1;
                    stack.push(Call { name: canonical(&word), arg: 0, only_names: false });
                } else if to_pt && word.to_ascii_lowercase().starts_with("_xleta.") {
                    out.push_str(&func_name(&word[7..], true));
                } else if !to_pt
                    && as_value(&stack, &word, s.get(j) == Some(&')'))
                    && !canonical(&word).is_empty()
                    && crate::parse::parse_cell(&word).is_none()
                {
                    out.push_str("_xleta.");
                    out.push_str(canonical(&word));
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

/// Primeira função chamada na fórmula (do arquivo, sem `=`) que não existe.
pub fn unknown_function(f: &str) -> Option<String> {
    let s: Vec<char> = f.chars().collect();
    let vars = find_vars(&s, ',');
    let mut scratch = String::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        match c {
            '"' | '\'' => i = quoted(&s, i, c, &mut scratch),
            '[' => i = bracketed(&s, i, &mut scratch),
            _ if is_ident(c) && !c.is_ascii_digit() => {
                let st = i;
                while i < s.len() && is_ident(s[i]) {
                    i += 1;
                }
                let word: String = s[st..i].iter().collect();
                if s.get(skip_ws(&s, i)) == Some(&'(')
                    && funcs::lookup(&word).is_none()
                    && !vars.names.contains(&local(&word))
                {
                    let w = word.trim_start_matches("_xlfn.").trim_start_matches("_xlws.");
                    return Some(w.to_string());
                }
            }
            _ => i += 1,
        }
    }
    None
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
                if matches!(name, "FILTER" | "SORT") {
                    format!("_xlfn._xlws.{name}")
                } else if NEW_FUNCS.contains(&name) {
                    format!("_xlfn.{name}")
                } else {
                    name.to_string()
                }
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
    "RANDARRAY",
    "TAKE",
    "DROP",
    "CHOOSECOLS",
    "CHOOSEROWS",
    "VSTACK",
    "HSTACK",
    "TOCOL",
    "TOROW",
    "EXPAND",
    // Estatísticas (2010).
    "MODE.SNGL",
    "MODE.MULT",
    "PERCENTILE.INC",
    "PERCENTILE.EXC",
    "QUARTILE.INC",
    "QUARTILE.EXC",
    "PERCENTRANK.INC",
    "PERCENTRANK.EXC",
    "RANK.AVG",
    "COVARIANCE.P",
    "COVARIANCE.S",
    "NORM.DIST",
    "NORM.INV",
    "NORM.S.DIST",
    "NORM.S.INV",
    "LOGNORM.DIST",
    "LOGNORM.INV",
    "EXPON.DIST",
    "WEIBULL.DIST",
    "GAMMA.DIST",
    "GAMMA.INV",
    "GAMMALN.PRECISE",
    "BETA.DIST",
    "BETA.INV",
    "CHISQ.DIST",
    "CHISQ.DIST.RT",
    "CHISQ.INV",
    "CHISQ.INV.RT",
    "CHISQ.TEST",
    "T.DIST",
    "T.DIST.RT",
    "T.DIST.2T",
    "T.INV",
    "T.INV.2T",
    "T.TEST",
    "F.DIST",
    "F.DIST.RT",
    "F.INV",
    "F.INV.RT",
    "F.TEST",
    "Z.TEST",
    "BINOM.DIST",
    "BINOM.INV",
    "NEGBINOM.DIST",
    "POISSON.DIST",
    "HYPGEOM.DIST",
    "CONFIDENCE.NORM",
    "CONFIDENCE.T",
    "ERF.PRECISE",
    "ERFC.PRECISE",
    "CEILING.PRECISE",
    "FLOOR.PRECISE",
    // 2013 e 2016.
    "ACOT",
    "ACOTH",
    "ARABIC",
    "BASE",
    "BINOM.DIST.RANGE",
    "BITAND",
    "BITLSHIFT",
    "BITOR",
    "BITRSHIFT",
    "BITXOR",
    "COMBINA",
    "COT",
    "COTH",
    "CSC",
    "CSCH",
    "DECIMAL",
    "FORECAST.LINEAR",
    "FORMULATEXT",
    "GAMMA",
    "GAUSS",
    "ISFORMULA",
    "MUNIT",
    "PDURATION",
    "PERMUTATIONA",
    "PHI",
    "RRI",
    "SEC",
    "SECH",
    "SHEET",
    "SHEETS",
    "SKEW.P",
    "WEBSERVICE",
    // Microsoft 365.
    "BYCOL",
    "BYROW",
    "GROUPBY",
    "IMAGE",
    "ISOMITTED",
    "MAKEARRAY",
    "MAP",
    "PERCENTOF",
    "PIVOTBY",
    "REDUCE",
    "SCAN",
    "TEXTSPLIT",
    "WRAPCOLS",
    "WRAPROWS",
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
        // Matrizes dinâmicas e LET.
        assert_eq!(from_pt("FILTRO(A1:B9;B1:B9>2)"), "_xlfn._xlws.FILTER(A1:B9,B1:B9>2)");
        assert_eq!(
            from_pt("LET(x;2;total;x*3;total+x)"),
            "_xlfn.LET(_xlpm.x,2,_xlpm.total,_xlpm.x*3,_xlpm.total+_xlpm.x)"
        );
        assert_eq!(to_pt("_xlfn.LET(_xlpm.x,1.5,_xlpm.x*2)"), "LET(x;1,5;x*2)");
        assert_eq!(from_pt("SOMA(A1:A3)*1,5"), "SUM(A1:A3)*1.5");
        assert_eq!(from_pt("SE(A1>0;\"sim\";\"não\")"), "IF(A1>0,\"sim\",\"não\")");
        assert_eq!(to_pt("1E+5*2"), "1E+5*2");
        assert_eq!(from_pt("soma(a1:b2)"), "SUM(A1:B2)");
    }

    #[test]
    fn lambdas_and_function_values() {
        // Parâmetros de LAMBDA (opcionais entre colchetes) e chamadas.
        let file = "_xlfn.LAMBDA(_xlpm.x,_xlop.y,_xlpm.x+_xlpm.y)(1,2)";
        assert_eq!(from_pt("LAMBDA(x;[y];x+y)(1;2)"), file);
        assert_eq!(to_pt(file), "LAMBDA(x;[y];x+y)(1;2)");
        let file = "_xlfn.LET(_xlpm.f,_xlfn.LAMBDA(_xlpm.n,_xlpm.n*2),_xlpm.f(A1))";
        assert_eq!(from_pt("LET(f;LAMBDA(n;n*2);f(A1))"), file);
        assert_eq!(to_pt(file), "LET(f;LAMBDA(n;n*2);f(A1))");
        // Função pelo nome.
        assert_eq!(from_pt("BYROW(A1:B3;SOMA)"), "_xlfn.BYROW(A1:B3,_xleta.SUM)");
        assert_eq!(to_pt("_xlfn.BYROW(A1:B3,_xleta.SUM)"), "BYROW(A1:B3;SOMA)");
        assert_eq!(
            from_pt("AGRUPARPOR(A1:A9;B1:B9;EMPILHARH(SOMA;MÉDIA))"),
            "_xlfn.GROUPBY(A1:A9,B1:B9,_xlfn.HSTACK(_xleta.SUM,_xleta.AVERAGE))"
        );
        assert_eq!(
            from_pt("LET(f;SOMA;BYROW(A1:B3;f))"),
            "_xlfn.LET(_xlpm.f,_xleta.SUM,_xlfn.BYROW(A1:B3,_xlpm.f))"
        );
        // Um nome igual ao de uma função, fora do lugar da função, fica como está.
        assert_eq!(from_pt("BYROW(N;SOMA)"), "_xlfn.BYROW(N,_xleta.SUM)");
        assert_eq!(from_pt("SOMA(N)"), "SUM(N)");
        // Referência de tabela dentro de LAMBDA não é parâmetro opcional.
        assert_eq!(from_pt("LAMBDA(x;T[Valor]+x)(1)"), "_xlfn.LAMBDA(_xlpm.x,T[Valor]+_xlpm.x)(1)");
    }

    #[test]
    fn unknown_functions() {
        assert_eq!(unknown_function("SUM(MINHA.FUNC(A1),\"OUTRA(1)\")").as_deref(), Some("MINHA.FUNC"));
        assert_eq!(unknown_function("SUM(A1,'Aba (1)'!B2)+T[Valor (R$)]"), None);
        assert_eq!(unknown_function("_xlfn.LET(_xlpm.f,_xlfn.LAMBDA(_xlpm.x,_xlpm.x),_xlpm.f(1))"), None);
        assert_eq!(unknown_function("_xlfn.NOVA(1)").as_deref(), Some("NOVA"));
    }

    #[test]
    fn array_constants() {
        assert_eq!(to_pt("SUM({1,2;3,4.5})"), "SOMA({1\\2;3\\4,5})");
        assert_eq!(from_pt("SOMA({1\\2;3\\4,5})"), "SUM({1,2;3,4.5})");
        assert_eq!(from_pt("ESCOLHER({1\\2};A1;\"a\\b\")"), "CHOOSE({1,2},A1,\"a\\b\")");
        assert_eq!(from_pt("SOMA({VERDADEIRO\\FALSO})"), "SUM({TRUE,FALSE})");
    }

    #[test]
    fn portuguese_names() {
        assert_eq!(from_pt("PREVISÃO(1;A1:A3;B1:B3)"), "FORECAST(1,A1:A3,B1:B3)");
        assert_eq!(from_pt("TEXTODIVIDIR(A1;\",\")"), "_xlfn.TEXTSPLIT(A1,\",\")");
        assert_eq!(from_pt("DIVIDIRTEXTO(A1;\",\")"), "_xlfn.TEXTSPLIT(A1,\",\")");
        assert_eq!(to_pt("_xlfn.TEXTSPLIT(A1,\",\")"), "DIVIDIRTEXTO(A1;\",\")");
        assert_eq!(from_pt("DIATRABALHOTOTAL.INTL(A1;B1;7)"), "NETWORKDAYS.INTL(A1,B1,7)");
        assert_eq!(from_pt("percentil.exc(a1:a9;0,9)"), "_xlfn.PERCENTILE.EXC(A1:A9,0.9)");
        // Cada nome em português volta para a mesma função, sem repetidos.
        let mut seen = std::collections::HashMap::new();
        for d in funcs::all() {
            assert_eq!(canonical(d.pt), d.name, "{}", d.pt);
            assert!(seen.insert(d.pt, d.name).is_none(), "{} repetido", d.pt);
            assert!(funcs::lookup(d.name).is_some_and(|f| funcs::def(f).pt == d.pt), "{}", d.name);
        }
    }
}
