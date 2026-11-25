//! Conferência de uma fórmula digitada, antes de ela ir para a célula.
//!
//! O compilador ([`crate::parse`]) é tolerante: o que ele não entende vira
//! `#NOME?`. Gravado assim, o Excel acusa o arquivo de danificado e tira a
//! fórmula. Aqui a gramática é a do arquivo (nomes em inglês, separador `,`),
//! mais larga que a do compilador: o que o Excel lê e o motor ainda não
//! calcula (interseção, união, 3D) passa.

use std::borrow::Cow;

use crate::funcs;
use crate::parse::{Node, Resolve, TableInfo, compile, parse_cell};

/// Limites do Excel.
const MAX_CHARS: usize = 8192;
const MAX_TEXT: usize = 255;
const MAX_NEST: usize = 64;
/// Parênteses e chamadas abertas (a conferência é recursiva).
const MAX_DEPTH: usize = 256;

type R<T> = Result<T, String>;

const EXTERNAL: &str = "Referências a outro arquivo ainda não podem ser digitadas.";
const BRACKET: &str = "Falta fechar um colchete (]).";
const PAREN: &str = "Falta fechar um parêntese.";
const DEEP: &str = "A fórmula tem níveis demais.";

/// Onde a fórmula está e o que existe no arquivo (`None` = só a escrita).
struct Cx<'a> {
    res: Option<&'a dyn Resolve>,
    sheet: u32,
    row: u32,
    col: u32,
}

/// Deixa `f` (texto do arquivo, sem `=`) como o arquivo guarda e confere se o
/// Excel a lê. `Err` = o que está errado, em português.
pub fn prepare(f: &str, sheet: u32, row: u32, col: u32, res: &dyn Resolve) -> R<String> {
    let cx = Cx { res: Some(res), sheet, row, col };
    if f.chars().count() > MAX_CHARS {
        return Err(format!("A fórmula passa de {MAX_CHARS} caracteres."));
    }
    let full = qualify(f, &cx)?;
    Checker::new(&full, &cx).run()?;
    Ok(full.into_owned())
}

/// Só a escrita, sem olhar o que existe no arquivo.
pub fn syntax(f: &str) -> R<()> {
    let cx = Cx { res: None, sheet: 0, row: 0, col: 0 };
    Checker::new(f, &cx).run()
}

// ---------------------------------------------------------------------------
// Referências estruturadas como o arquivo guarda
// ---------------------------------------------------------------------------

/// As palavras do Excel em português, e as do arquivo.
const ITEMS: [(&str, &str); 11] = [
    ("#tudo", "#All"),
    ("#todos", "#All"),
    ("#dados", "#Data"),
    ("#cabeçalhos", "#Headers"),
    ("#totais", "#Totals"),
    ("#esta linha", "#This Row"),
    ("#all", "#All"),
    ("#data", "#Data"),
    ("#headers", "#Headers"),
    ("#totals", "#Totals"),
    ("#this row", "#This Row"),
];

/// `[@Valor]` → `Tabela[[#This Row],[Valor]]`, `[Valor]` → `Tabela[Valor]`,
/// `Tabela[#Tudo]` → `Tabela[#All]`: no arquivo, o nome da tabela está sempre
/// lá e as palavras são em inglês.
fn qualify<'f>(f: &'f str, cx: &Cx) -> R<Cow<'f, str>> {
    if !f.contains('[') {
        return Ok(Cow::Borrowed(f));
    }
    let b = f.as_bytes();
    let mut out = String::with_capacity(f.len() + 16);
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                i += 1;
            }
            b'[' => {
                let end = close(b, i).ok_or(BRACKET)?;
                let inner = &f[i + 1..end];
                let named = f[..i].chars().next_back().is_some_and(is_word);
                let book = !named && !inner.is_empty() && inner.bytes().all(|c| c.is_ascii_digit());
                if !named && !book && b.get(end + 1).is_some_and(|&c| is_word_byte(c)) {
                    return Err(EXTERNAL.into());
                }
                if !book {
                    out.push_str(&f[copied..i]);
                    if !named {
                        let t = cx.res.and_then(|r| r.table_at(cx.sheet, cx.row, cx.col));
                        let t = t.ok_or_else(|| {
                            format!(
                                "\"[{}]\" só vale dentro de uma tabela. Fora dela, escreva o nome da tabela antes: Tabela[Coluna].",
                                short(inner)
                            )
                        })?;
                        out.push_str(t.name);
                    }
                    out.push_str(&spec(inner));
                    copied = end + 1;
                }
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    out.push_str(&f[copied..]);
    Ok(Cow::Owned(out))
}

/// Uma especificação, com os colchetes.
fn spec(inner: &str) -> String {
    let t = inner.trim();
    if let Some(rest) = t.strip_prefix('@') {
        let rest = rest.trim();
        return match rest {
            "" => "[#This Row]".into(),
            r if r.starts_with('[') => format!("[[#This Row],{r}]"),
            r => format!("[[#This Row],[{r}]]"),
        };
    }
    let word = |w: &str| {
        let low = w.trim().to_lowercase();
        ITEMS.iter().find(|(k, _)| *k == low).map(|(_, en)| *en)
    };
    if let Some(en) = word(t) {
        return format!("[{en}]");
    }
    if !t.starts_with('[') {
        return format!("[{inner}]");
    }
    // Itens `[...]`: só as palavras e o separador mudam.
    let b = inner.as_bytes();
    let mut out = String::from("[");
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        match b[i] {
            b'[' => {
                let Some(end) = close(b, i) else { break };
                if let Some(en) = word(&inner[i + 1..end]) {
                    out.push_str(&inner[copied..i]);
                    out.push_str(&format!("[{en}]"));
                    copied = end + 1;
                }
                i = end + 1;
            }
            b';' => {
                out.push_str(&inner[copied..i]);
                out.push(',');
                i += 1;
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&inner[copied..]);
    out.push(']');
    out
}

/// O `]` que fecha o `[` em `i` (`'` escapa o caractere seguinte).
fn close(b: &[u8], i: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = i;
    while j < b.len() {
        match b[j] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            b'\'' => j += 1,
            _ => {}
        }
        j += 1;
    }
    None
}

fn is_word(c: char) -> bool {
    !c.is_ascii() || is_word_byte(c as u8)
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'$' | b'\\' | b'?') || c >= 0x80
}

// ---------------------------------------------------------------------------
// A gramática
// ---------------------------------------------------------------------------

struct Checker<'a> {
    text: &'a str,
    s: &'a [u8],
    i: usize,
    cx: &'a Cx<'a>,
    depth: usize,
    nest: usize,
}

impl<'a> Checker<'a> {
    fn new(text: &'a str, cx: &'a Cx<'a>) -> Self {
        Self { text, s: text.as_bytes(), i: 0, cx, depth: 0, nest: 0 }
    }

    fn run(&mut self) -> R<()> {
        self.expr(false)?;
        match self.peek() {
            None => Ok(()),
            Some(b')') => Err("Há um parêntese fechado a mais.".into()),
            Some(b',') => Err("Há um separador (;) fora de uma função.".into()),
            Some(_) => Err(self.stray()),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// Pula espaços; `true` = havia algum.
    fn ws(&mut self) -> bool {
        let st = self.i;
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.i += 1;
        }
        self.i > st
    }

    fn digits(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
    }

    fn word(&mut self) -> &'a str {
        let st = self.i;
        while self.peek().is_some_and(is_word_byte) {
            self.i += 1;
        }
        &self.text[st..self.i]
    }

    /// O que está em `i` não cabe ali.
    fn stray(&self) -> String {
        match self.text[self.i..].chars().next() {
            Some('#') => "O operador # (área despejada) ainda não é aceito.".into(),
            Some(c) => format!("\"{c}\" não cabe nessa parte da fórmula."),
            None => PAREN.into(),
        }
    }

    fn deeper(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { Err(DEEP.into()) } else { Ok(()) }
    }

    /// Operandos e operadores, até o que não é da expressão. `union` = dentro
    /// de parênteses, onde `,` une referências.
    fn expr(&mut self, union: bool) -> R<()> {
        self.deeper()?;
        loop {
            loop {
                self.ws();
                match self.peek() {
                    Some(b'+' | b'-') => self.i += 1,
                    Some(b'@') => return Err("O operador @ ainda não é aceito fora de uma tabela.".into()),
                    _ => break,
                }
            }
            self.operand()?;
            let mut spaced = self.ws();
            while self.peek() == Some(b'%') {
                self.i += 1;
                spaced = self.ws();
            }
            let Some(c) = self.peek() else { break };
            let next = self.s.get(self.i + 1);
            match c {
                b'<' if matches!(next, Some(b'=' | b'>')) => self.i += 2,
                b'>' if next == Some(&b'=') => self.i += 2,
                b'+' | b'-' | b'*' | b'/' | b'^' | b'&' | b':' | b'=' | b'<' | b'>' => self.i += 1,
                b',' if union => self.i += 1,
                // Espaço entre duas referências: interseção.
                _ if spaced && (is_word_byte(c) || matches!(c, b'\'' | b'(')) => {}
                _ => break,
            }
        }
        self.depth -= 1;
        Ok(())
    }

    fn operand(&mut self) -> R<()> {
        let Some(c) = self.peek() else {
            return Err("A fórmula está incompleta: falta um valor no fim.".into());
        };
        match c {
            b'(' => {
                self.i += 1;
                self.expr(true)?;
                if self.peek() != Some(b')') {
                    return Err(self.stray());
                }
                self.i += 1;
                self.calls()
            }
            b'"' => self.string(),
            b'{' => self.array(),
            b'#' => self.error(),
            b'\'' => {
                let name = self.quoted()?;
                if self.peek() != Some(b'!') {
                    return Err(
                        "Depois do nome da aba entre apóstrofos vem um ponto de exclamação (!).".into()
                    );
                }
                self.i += 1;
                self.sheets(&name)?;
                self.after_sheet()
            }
            b'[' => self.book(),
            b'.' if !self.s.get(self.i + 1).is_some_and(u8::is_ascii_digit) => Err(self.stray()),
            b'0'..=b'9' | b'.' => self.number(),
            _ if is_word_byte(c) => self.ident(),
            b')' => Err("Falta um valor antes do parêntese.".into()),
            b',' => Err("Falta um valor antes do separador (;).".into()),
            b'*' | b'/' | b'^' | b'&' | b'=' | b'<' | b'>' | b'%' | b':' => {
                Err(format!("Falta um valor antes de \"{}\".", c as char))
            }
            _ => Err(self.stray()),
        }
    }

    /// `[1]Aba!A1`, `[1]!Nome`: outro arquivo.
    fn book(&mut self) -> R<()> {
        let end = close(self.s, self.i).ok_or(BRACKET)?;
        let inner = &self.text[self.i + 1..end];
        if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
            return Err(EXTERNAL.into());
        }
        self.i = end + 1;
        self.word();
        if self.peek() == Some(b':') {
            self.i += 1;
            self.word();
        }
        if self.peek() != Some(b'!') {
            return Err(EXTERNAL.into());
        }
        self.i += 1;
        self.after_sheet()
    }

    /// Texto entre aspas (`""` = uma aspa).
    fn string(&mut self) -> R<()> {
        let mut len = 0;
        let mut it = self.text[self.i + 1..].char_indices().peekable();
        while let Some((k, c)) = it.next() {
            if c != '"' || it.next_if(|x| x.1 == '"').is_some() {
                len += 1;
                continue;
            }
            if len > MAX_TEXT {
                return Err(format!(
                    "Um texto da fórmula passa de {MAX_TEXT} caracteres. Divida em partes com &."
                ));
            }
            self.i += k + 2;
            return Ok(());
        }
        Err("Falta fechar as aspas (\").".into())
    }

    /// Nome de aba entre apóstrofos.
    fn quoted(&mut self) -> R<String> {
        let mut out = String::new();
        let mut it = self.text[self.i + 1..].char_indices().peekable();
        while let Some((k, c)) = it.next() {
            if c != '\'' || it.next_if(|x| x.1 == '\'').is_some() {
                out.push(c);
                continue;
            }
            self.i += k + 2;
            return Ok(out);
        }
        Err("Falta fechar o apóstrofo (') do nome da aba.".into())
    }

    fn number(&mut self) -> R<()> {
        let st = self.i;
        self.digits();
        if self.peek() == Some(b'.') {
            self.i += 1;
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let sign = matches!(self.s.get(self.i + 1), Some(b'+' | b'-'));
            let k = self.i + 1 + usize::from(sign);
            if self.s.get(k).is_some_and(u8::is_ascii_digit) {
                self.i = k;
                self.digits();
            }
        }
        // `2º` e `1A` só valem como nome de aba.
        let glued = !self.word().is_empty();
        if self.peek() == Some(b'!') {
            let name = self.text[st..self.i].to_string();
            self.i += 1;
            self.sheets(&name)?;
            return self.after_sheet();
        }
        if glued {
            return Err(format!("\"{}\" não é um número, um nome nem uma célula.", &self.text[st..self.i]));
        }
        Ok(())
    }

    fn error(&mut self) -> R<()> {
        const ERRS: [&str; 15] = [
            "#NULL!",
            "#DIV/0!",
            "#VALUE!",
            "#REF!",
            "#NAME?",
            "#NUM!",
            "#N/A",
            "#GETTING_DATA",
            "#SPILL!",
            "#CALC!",
            "#FIELD!",
            "#BLOCKED!",
            "#CONNECT!",
            "#UNKNOWN!",
            "#BUSY!",
        ];
        let rest = &self.s[self.i..];
        match ERRS
            .iter()
            .find(|e| rest.len() >= e.len() && rest[..e.len()].eq_ignore_ascii_case(e.as_bytes()))
        {
            Some(e) => {
                self.i += e.len();
                Ok(())
            }
            None => Err(self.stray()),
        }
    }

    /// `{1,2;3,4}`.
    fn array(&mut self) -> R<()> {
        const WHAT: &str = "Entre chaves só cabem números, textos, VERDADEIRO e FALSO, com \\ entre as colunas e ; entre as linhas.";
        self.i += 1;
        let (mut cols, mut first) = (0usize, None);
        loop {
            self.ws();
            match self.peek() {
                Some(b'"') => self.string()?,
                Some(b'#') => self.error()?,
                Some(b'+' | b'-' | b'0'..=b'9' | b'.') => {
                    if matches!(self.peek(), Some(b'+' | b'-')) {
                        self.i += 1;
                        self.ws();
                    }
                    if !matches!(self.peek(), Some(b'0'..=b'9' | b'.')) {
                        return Err(WHAT.into());
                    }
                    self.number()?;
                }
                _ => {
                    let w = self.word();
                    if !w.eq_ignore_ascii_case("TRUE") && !w.eq_ignore_ascii_case("FALSE") {
                        return Err(WHAT.into());
                    }
                }
            }
            cols += 1;
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(c @ (b';' | b'}')) => {
                    self.i += 1;
                    if *first.get_or_insert(cols) != cols {
                        return Err("As linhas da matriz entre chaves têm tamanhos diferentes.".into());
                    }
                    if c == b'}' {
                        return Ok(());
                    }
                    cols = 0;
                }
                None => return Err("Falta fechar uma chave (}).".into()),
                Some(_) => return Err(WHAT.into()),
            }
        }
    }

    /// As abas de `Aba!`, `Aba1:Aba3!` ou `[1]Aba!` (já sem apóstrofos).
    fn sheets(&self, name: &str) -> R<()> {
        if let Some(rest) = name.strip_prefix('[') {
            let n = rest.split(']').next().unwrap_or("");
            let known = !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
            return if known { Ok(()) } else { Err(EXTERNAL.into()) };
        }
        let Some(res) = self.cx.res else { return Ok(()) };
        if name.contains(['[', '\\', '/']) {
            return Err(EXTERNAL.into());
        }
        match name.split(':').find(|n| res.sheet(n).is_none()) {
            Some(n) => Err(format!("A aba \"{}\" não existe.", short(n))),
            None => Ok(()),
        }
    }

    /// Depois do `!`: célula, linhas, nome, função, tabela ou `#REF!`.
    fn after_sheet(&mut self) -> R<()> {
        match self.peek() {
            Some(b'#') => self.error(),
            Some(c) if is_word_byte(c) => {
                self.word();
                match self.peek() {
                    Some(b'(') => self.call(""),
                    Some(b'[') => self.brackets(),
                    _ => Ok(()),
                }
            }
            _ => Err("Falta a célula depois do nome da aba.".into()),
        }
    }

    fn brackets(&mut self) -> R<()> {
        self.i = close(self.s, self.i).ok_or(BRACKET)? + 1;
        Ok(())
    }

    fn ident(&mut self) -> R<()> {
        let st = self.i;
        let w = self.word();
        match self.peek() {
            Some(b'(') => {
                self.call(w)?;
                self.calls()
            }
            Some(b'!') => {
                self.i += 1;
                self.sheets(w)?;
                self.after_sheet()
            }
            Some(b':') if self.sheet_range(w) => {
                self.i += 1;
                self.word();
                let name = self.text[st..self.i].to_string();
                self.i += 1;
                self.sheets(&name)?;
                self.after_sheet()
            }
            Some(b'[') => {
                self.brackets()?;
                self.table(w, &self.text[st..self.i])
            }
            // Espaço entre o nome da função e o parêntese.
            Some(b' ') if funcs::lookup(w).is_some() && self.text[self.i..].trim_start().starts_with('(') => {
                self.ws();
                self.call(w)?;
                self.calls()
            }
            _ if !w.contains(|c: char| c.is_alphanumeric() || c == '_' || c == '\\') => {
                Err(format!("\"{w}\" não é um nome nem uma célula."))
            }
            _ => Ok(()),
        }
    }

    /// O `:` em `i` é o de `Aba1:Aba3!` (e não o de `A1:Plan2!B3`).
    fn sheet_range(&self, first: &str) -> bool {
        if parse_cell(first).is_some() {
            return false;
        }
        let st = self.i + 1;
        let n = self.s[st..].iter().take_while(|&&c| is_word_byte(c)).count();
        n > 0 && self.s.get(st + n) == Some(&b'!')
    }

    /// A tabela existe e tem a coluna? (`part` = a referência inteira.)
    fn table(&self, name: &str, part: &str) -> R<()> {
        let Some(res) = self.cx.res else { return Ok(()) };
        let Some(t) = res.table(name) else {
            return Err(format!("A tabela \"{}\" não existe.", short(name)));
        };
        let f = compile(part, self.cx.sheet, self.cx.row, self.cx.col, res);
        if matches!(f.node(f.root), Node::Err(duck_core::CellError::Name)) {
            return Err(no_column(&t));
        }
        Ok(())
    }

    /// `(...)` colado num valor que já é uma função (`LAMBDA(x;x+1)(2)`).
    fn calls(&mut self) -> R<()> {
        while self.peek() == Some(b'(') {
            self.call("")?;
        }
        Ok(())
    }

    /// Argumentos, a partir do `(`. `name` vazio = não é uma função conhecida.
    fn call(&mut self, name: &str) -> R<()> {
        self.i += 1;
        self.nest += 1;
        self.deeper()?;
        if self.nest > MAX_NEST {
            return Err(format!("A fórmula tem mais de {MAX_NEST} níveis de funções."));
        }
        self.ws();
        let mut n = 0usize;
        if self.peek() == Some(b')') {
            self.i += 1;
        } else {
            loop {
                n += 1;
                self.ws();
                // Argumento vazio (`SE(A1;;2)`) também vale.
                if !matches!(self.peek(), Some(b',' | b')')) {
                    if self.peek().is_none() {
                        return Err(PAREN.into());
                    }
                    self.expr(false)?;
                }
                match self.peek() {
                    Some(b',') => self.i += 1,
                    Some(b')') => {
                        self.i += 1;
                        break;
                    }
                    _ => return Err(self.stray()),
                }
            }
        }
        self.nest -= 1;
        self.depth -= 1;
        let Some(f) = funcs::lookup(name) else { return Ok(()) };
        let d = funcs::def(f);
        if d.opaque {
            return Ok(());
        }
        if n < d.min as usize {
            return Err(format!("Faltam argumentos na função {}({}).", d.pt, d.args));
        }
        // 255 (e perto disso) = sem limite, para o motor.
        if n > d.max as usize && d.max < 100 {
            return Err(format!("Há argumentos demais na função {}({}).", d.pt, d.args));
        }
        Ok(())
    }
}

fn no_column(t: &TableInfo) -> String {
    format!("A tabela \"{}\" não tem essa coluna. As que tem: {}.", t.name, short(&t.columns.join(", ")))
}

fn short(s: &str) -> String {
    if s.chars().count() <= 120 {
        return s.to_string();
    }
    let cut: String = s.chars().take(119).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::CellRange;

    struct Book;
    impl Resolve for Book {
        fn sheet(&self, name: &str) -> Option<u32> {
            ["Plan1", "Minha Aba", "D'água", "2024"].iter().position(|n| *n == name).map(|i| i as u32)
        }
        fn ext_sheet(&self, _: u16, _: &str) -> Option<u16> {
            Some(0)
        }
        fn name(&self, _: &str, _: u32) -> Option<u32> {
            None
        }
        fn table(&self, name: &str) -> Option<TableInfo<'_>> {
            static COLS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
            let columns = COLS.get_or_init(|| vec!["Nome".into(), "Valor R$".into()]);
            (name == "Vendas").then(|| TableInfo {
                name: "Vendas",
                sheet: 0,
                range: CellRange { first_row: 0, first_col: 2, last_row: 10, last_col: 3 },
                header_rows: 1,
                totals_rows: 0,
                columns,
            })
        }
        fn table_at(&self, _: u32, row: u32, _: u32) -> Option<TableInfo<'_>> {
            if row < 100 { self.table("Vendas") } else { None }
        }
    }

    /// Digitada em português, numa célula dentro da tabela.
    fn typed(pt: &str) -> R<String> {
        prepare(&crate::locale::from_pt(pt), 0, 4, 1, &Book)
    }

    #[test]
    fn good() {
        for f in [
            "A1",
            "1+1",
            "-A1%",
            "--(A1>2)",
            "+A1",
            "2^-1",
            "1,5E+3",
            ",5+1",
            "\"a\"\"b\"&A1",
            "A1<>B1",
            "A1<=B1",
            "SOMA(A1:B2;C:C;1:5;$2:$3)",
            "SOMA((A1;B1;C1:C5))",
            "SOMA(A1:A9 A5:B5)",
            "SE(A1;;2)",
            "SE(A1>0;\"sim\";)",
            "PI()",
            "AGORA( )",
            "SOMA (A1:A3)",
            " SOMA( A1 ; 2 ) ",
            "Plan1!A1+'Minha Aba'!B2:C3",
            "'D''água'!A1",
            "Plan1!A:A",
            "Plan1!1:5",
            "2024!A1",
            "Plan1!#REF!",
            "#N/D",
            "#REF!+1",
            "{1\\2;3\\4}",
            "SOMA({1\\-2,5;\"a\"\\VERDADEIRO})",
            "Vendas[Nome]",
            "Vendas[[#Dados];[Nome]:[Valor R$]]",
            "[@Nome]",
            "LET(x;2;y;x*3;x+y)",
            "LAMBDA(x;[y];x+y)(1;2)",
            "BYROW(A1:B3;SOMA)",
            "NomeQueNaoExiste+1",
            "FUNCAO.QUE.NAO.EXISTE(1;2)",
            "_nome.com_pontos*2",
            "A1:ÍNDICE(B:B;3)",
            "DESLOC(A1;1;1):B5",
            "A1\n+ 2",
            "Plan1!A1:Plan1!B3",
        ] {
            assert!(typed(f).is_ok(), "{f}: {:?}", typed(f));
        }
        // Como o arquivo guarda.
        for f in
            ["[1]Plan1!A1", "'[2]Outra aba'!$A:$F", "[1]!Nome", "_xlfn.ANCHORARRAY(A1)", "Plan1:Plan3!A1"]
        {
            assert_eq!(syntax(f), Ok(()), "{f}");
        }
    }

    #[test]
    fn bad() {
        for (f, what) in [
            ("SOMA(", "parêntese"),
            ("SOMA(A1", "parêntese"),
            ("SOMA(A1;", "parêntese"),
            ("(1+2", "parêntese"),
            ("SOMA(A1))", "a mais"),
            ("1+*2", "Falta um valor antes de \"*\""),
            ("1+", "incompleta"),
            ("*2", "Falta um valor"),
            ("A1;B1", "separador"),
            ("()", "Falta um valor"),
            ("\"abc", "aspas"),
            ("'Minha Aba!A1", "apóstrofo"),
            ("'Minha Aba'A1", "exclamação"),
            ("Plan1!", "Falta a célula"),
            ("Plan1!+1", "Falta a célula"),
            ("Outra!A1", "\"Outra\" não existe"),
            ("'Outra aba'!A1", "\"Outra aba\" não existe"),
            ("[Livro.xlsx]Plan1!A1", "outro arquivo"),
            ("'C:\\pasta\\[Livro.xlsx]Plan1'!A1", "outro arquivo"),
            ("Outra[Nome]", "\"Outra\" não existe"),
            ("Vendas[Preço]", "não tem essa coluna"),
            ("Vendas[Nome", "colchete"),
            ("{1\\2;3}", "tamanhos diferentes"),
            ("{A1\\2}", "chaves"),
            ("{1\\2", "chave"),
            ("1A+2", "não é um número"),
            ("A1#", "#"),
            ("@A1", "@"),
            ("A1!!B2", "não existe"),
            ("SE()", "Faltam argumentos na função SE("),
            ("PI(1)", "argumentos demais"),
            ("ABS(1;2)", "argumentos demais"),
            ("A1 = = 2", "Falta um valor"),
            ("$", "não é um nome"),
            ("", "incompleta"),
            ("1 2 \"a\"", "não cabe"),
            ("A1~2", "não cabe"),
        ] {
            match typed(f) {
                Err(e) => assert!(e.contains(what), "{f}: {e}"),
                Ok(x) => panic!("{f} passou: {x}"),
            }
        }
    }

    #[test]
    fn limits() {
        assert!(typed(&format!("\"{}\"", "a".repeat(255))).is_ok());
        assert!(typed(&format!("\"{}\"", "a".repeat(256))).unwrap_err().contains("255"));
        assert!(typed(&"1+".repeat(5000)).unwrap_err().contains("8192"));
        assert!(typed(&"(".repeat(5000)).unwrap_err().contains("níveis"));
        assert!(typed(&format!("{}1{}", "ABS(".repeat(64), ")".repeat(64))).is_ok());
        assert!(typed(&format!("{}1{}", "ABS(".repeat(65), ")".repeat(65))).unwrap_err().contains("64"));
    }

    /// O arquivo guarda o nome da tabela e as palavras em inglês.
    #[test]
    fn table_refs_as_stored() {
        for (pt, file) in [
            ("[@Nome]", "Vendas[[#This Row],[Nome]]"),
            ("[@[Valor R$]]*2", "Vendas[[#This Row],[Valor R$]]*2"),
            ("Vendas[@Nome]", "Vendas[[#This Row],[Nome]]"),
            ("SOMA([@])", "SUM(Vendas[#This Row])"),
            ("SOMA([Nome])", "SUM(Vendas[Nome])"),
            ("Vendas[#Tudo]", "Vendas[#All]"),
            ("Vendas[[#Cabeçalhos];[Nome]]", "Vendas[[#Headers],[Nome]]"),
            ("Vendas[[#Esta Linha];[Nome]:[Valor R$]]", "Vendas[[#This Row],[Nome]:[Valor R$]]"),
            ("Vendas[[#This Row],[Nome]]", "Vendas[[#This Row],[Nome]]"),
            ("\"[@Nome]\"&Vendas[Nome]", "\"[@Nome]\"&Vendas[Nome]"),
        ] {
            assert_eq!(typed(pt).as_deref(), Ok(file), "{pt}");
        }
        let out = prepare("[@Nome]", 0, 200, 1, &Book);
        assert!(out.unwrap_err().contains("dentro de uma tabela"));
    }
}
