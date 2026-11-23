//! Analisador das fórmulas DAX das medidas e colunas calculadas.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    StrictEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    In,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Num(f64),
    Str(String),
    /// `Tabela[Coluna]`.
    Col(String, String),
    /// `[Nome]`: medida ou coluna da linha atual.
    Bracket(String),
    /// Variável ou tabela.
    Name(String),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Func(String, Vec<Node>),
    /// `{a, b}`.
    List(Vec<Node>),
    /// `VAR x = … RETURN …`.
    Vars(Vec<(String, Node)>, Box<Node>),
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Bracket(String),
    /// Nome solto ou `'entre aspas'`.
    Id(String, bool),
    Op(&'static str),
}

type Res<T> = Result<T, String>;

fn lex(s: &str) -> Res<Vec<Tok>> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let two = |a: char, b: char| ch == a && c.get(i + 1) == Some(&b);
        if ch.is_whitespace() {
            i += 1;
        } else if two('-', '-') || two('/', '/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if two('/', '*') {
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if ch == '"' || ch == '\'' || ch == '[' {
            let close = if ch == '[' { ']' } else { ch };
            let mut t = String::new();
            i += 1;
            loop {
                match c.get(i) {
                    None => return Err("texto sem fim".into()),
                    Some(&x) if x == close => {
                        if c.get(i + 1) == Some(&close) {
                            t.push(close);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    Some(&x) => t.push(x),
                }
                i += 1;
            }
            out.push(match ch {
                '"' => Tok::Str(t),
                '[' => Tok::Bracket(t),
                _ => Tok::Id(t, true),
            });
        } else if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let st = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            if matches!(c.get(i), Some('e' | 'E')) {
                let mut k = i + 1;
                if matches!(c.get(k), Some('+' | '-')) {
                    k += 1;
                }
                if c.get(k).is_some_and(|d| d.is_ascii_digit()) {
                    i = k;
                    while i < c.len() && c[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let t: String = c[st..i].iter().collect();
            out.push(Tok::Num(t.parse().map_err(|_| format!("número {t}"))?));
        } else if ch.is_alphabetic() || ch == '_' {
            let st = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.') {
                i += 1;
            }
            out.push(Tok::Id(c[st..i].iter().collect(), false));
        } else {
            let op = [
                "<>", "<=", ">=", "==", "&&", "||", "+", "-", "*", "/", "^", "&", "=", "<", ">", "(", ")",
                ",", ";", "{", "}", "!",
            ]
            .iter()
            .find(|o| c[i..].starts_with(&o.chars().collect::<Vec<_>>()))
            .ok_or_else(|| format!("caractere {ch}"))?;
            out.push(Tok::Op(op));
            i += op.len();
        }
    }
    Ok(out)
}

struct P {
    t: Vec<Tok>,
    i: usize,
}

/// Analisa uma fórmula.
pub fn parse(s: &str) -> Res<Node> {
    let mut p = P { t: lex(s.trim().trim_start_matches('='))?, i: 0 };
    let n = p.expr()?;
    if p.i < p.t.len() {
        return Err(format!("sobrou texto depois da fórmula: {:?}", p.t[p.i]));
    }
    Ok(n)
}

impl P {
    fn op(&mut self, o: &str) -> bool {
        let y = matches!(self.t.get(self.i), Some(Tok::Op(x)) if *x == o);
        self.i += y as usize;
        y
    }

    fn kw(&mut self, k: &str) -> bool {
        let y = matches!(self.t.get(self.i), Some(Tok::Id(x, false)) if x.eq_ignore_ascii_case(k));
        self.i += y as usize;
        y
    }

    fn is_kw(&self, k: &str) -> bool {
        matches!(self.t.get(self.i), Some(Tok::Id(x, false)) if x.eq_ignore_ascii_case(k))
    }

    fn want(&mut self, o: &str) -> Res<()> {
        if self.op(o) { Ok(()) } else { Err(format!("esperado '{o}', veio {:?}", self.t.get(self.i))) }
    }

    fn expr(&mut self) -> Res<Node> {
        if self.is_kw("var") {
            let mut vars = Vec::new();
            while self.kw("var") {
                let name = match self.t.get(self.i) {
                    Some(Tok::Id(n, false)) => n.clone(),
                    t => return Err(format!("nome de variável: {t:?}")),
                };
                self.i += 1;
                self.want("=")?;
                vars.push((name.to_lowercase(), self.or()?));
            }
            if !self.kw("return") {
                return Err("faltou RETURN".into());
            }
            return Ok(Node::Vars(vars, Box::new(self.expr()?)));
        }
        self.or()
    }

    fn or(&mut self) -> Res<Node> {
        let mut a = self.and()?;
        while self.op("||") {
            a = Node::Bin(Op::Or, Box::new(a), Box::new(self.and()?));
        }
        Ok(a)
    }

    fn and(&mut self) -> Res<Node> {
        let mut a = self.not()?;
        while self.op("&&") {
            a = Node::Bin(Op::And, Box::new(a), Box::new(self.not()?));
        }
        Ok(a)
    }

    fn not(&mut self) -> Res<Node> {
        if self.is_kw("not") && !matches!(self.t.get(self.i + 1), Some(Tok::Op("("))) || self.op("!") {
            self.kw("not");
            return Ok(Node::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Res<Node> {
        let mut a = self.concat()?;
        loop {
            let op = match self.t.get(self.i) {
                Some(Tok::Op("=")) => Op::Eq,
                Some(Tok::Op("==")) => Op::StrictEq,
                Some(Tok::Op("<>")) => Op::Ne,
                Some(Tok::Op("<")) => Op::Lt,
                Some(Tok::Op("<=")) => Op::Le,
                Some(Tok::Op(">")) => Op::Gt,
                Some(Tok::Op(">=")) => Op::Ge,
                Some(Tok::Id(x, false)) if x.eq_ignore_ascii_case("in") => Op::In,
                _ => return Ok(a),
            };
            self.i += 1;
            a = Node::Bin(op, Box::new(a), Box::new(self.concat()?));
        }
    }

    fn concat(&mut self) -> Res<Node> {
        let mut a = self.add()?;
        while self.op("&") {
            a = Node::Bin(Op::Concat, Box::new(a), Box::new(self.add()?));
        }
        Ok(a)
    }

    fn add(&mut self) -> Res<Node> {
        let mut a = self.mul()?;
        loop {
            let op = if self.op("+") {
                Op::Add
            } else if self.op("-") {
                Op::Sub
            } else {
                return Ok(a);
            };
            a = Node::Bin(op, Box::new(a), Box::new(self.mul()?));
        }
    }

    fn mul(&mut self) -> Res<Node> {
        let mut a = self.unary()?;
        loop {
            let op = if self.op("*") {
                Op::Mul
            } else if self.op("/") {
                Op::Div
            } else {
                return Ok(a);
            };
            a = Node::Bin(op, Box::new(a), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> Res<Node> {
        if self.op("-") {
            return Ok(Node::Neg(Box::new(self.unary()?)));
        }
        if self.op("+") {
            return self.unary();
        }
        let a = self.atom()?;
        if self.op("^") {
            return Ok(Node::Bin(Op::Pow, Box::new(a), Box::new(self.unary()?)));
        }
        Ok(a)
    }

    fn atom(&mut self) -> Res<Node> {
        let t = self.t.get(self.i).cloned().ok_or("fórmula incompleta")?;
        self.i += 1;
        match t {
            Tok::Num(n) => Ok(Node::Num(n)),
            Tok::Str(s) => Ok(Node::Str(s)),
            Tok::Bracket(b) => Ok(Node::Bracket(b)),
            Tok::Op("(") => {
                let e = self.expr()?;
                self.want(")")?;
                Ok(e)
            }
            Tok::Op("{") => {
                let mut list = Vec::new();
                while !self.op("}") {
                    list.push(self.expr()?);
                    if !self.op(",") && !self.op(";") {
                        self.want("}")?;
                        break;
                    }
                }
                Ok(Node::List(list))
            }
            Tok::Id(n, quoted) => {
                if let Some(Tok::Bracket(c)) = self.t.get(self.i) {
                    let c = c.clone();
                    self.i += 1;
                    return Ok(Node::Col(n, c));
                }
                if !quoted && self.op("(") {
                    let mut args = Vec::new();
                    if !self.op(")") {
                        loop {
                            args.push(self.expr()?);
                            if self.op(",") || self.op(";") {
                                continue;
                            }
                            self.want(")")?;
                            break;
                        }
                    }
                    return Ok(Node::Func(n.to_uppercase(), args));
                }
                Ok(Node::Name(n))
            }
            t => Err(format!("inesperado: {t:?}")),
        }
    }
}

/// Medidas de um script do modelo: (tabela, nome, fórmula).
/// `CREATE MEASURE 'T'[Nome]=fórmula;` ou `CREATE MEASURE [x].'T'[Nome]=…`.
pub fn measures(script: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = script;
    while let Some(i) = rest.find("CREATE MEASURE") {
        rest = &rest[i + "CREATE MEASURE".len()..];
        let end = statement_end(rest);
        let st = &rest[..end];
        rest = &rest[end..];
        // Cabeçalho: até o `=` fora de colchetes e aspas.
        let Some(eq) = outside(st, '=') else { continue };
        let (head, body) = (st[..eq].trim(), st[eq + 1..].trim());
        // A última parte entre colchetes é o nome; antes dela, a tabela.
        let Some(open) = last_bracket(head) else { continue };
        let name = head[open + 1..head.len() - 1].replace("]]", "]");
        let table = head[..open].trim().trim_end_matches('.');
        let table = match table.rfind("].") {
            Some(k) => &table[k + 2..],
            None => table,
        };
        let table = table.trim().trim_matches('\'').replace("''", "'");
        out.push((table, name, body.to_string()));
    }
    out
}

/// Posição do `;` que fecha a instrução (fora de textos, nomes e comentários).
fn statement_end(s: &str) -> usize {
    outside(s, ';').unwrap_or(s.len())
}

fn outside(s: &str, want: char) -> Option<usize> {
    let b: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0;
    while i < b.len() {
        let (pos, c) = b[i];
        if c == want {
            return Some(pos);
        }
        match c {
            '"' | '\'' | '[' => {
                let close = if c == '[' { ']' } else { c };
                i += 1;
                while i < b.len() {
                    if b[i].1 == close {
                        if b.get(i + 1).is_some_and(|x| x.1 == close) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
            }
            '-' | '/' if b.get(i + 1).is_some_and(|x| x.1 == c) => {
                while i < b.len() && b[i].1 != '\n' {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Começo do último `[...]` de um cabeçalho que termina em `]`.
fn last_bracket(head: &str) -> Option<usize> {
    if !head.ends_with(']') {
        return None;
    }
    let mut start = None;
    let mut depth = false;
    let mut it = head.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match (c, depth) {
            ('[', false) => {
                depth = true;
                start = Some(i);
            }
            (']', true) => {
                if it.peek().is_some_and(|x| x.1 == ']') {
                    it.next();
                } else {
                    depth = false;
                }
            }
            _ => {}
        }
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas() {
        let n = parse("VAR h = TODAY() - MAX(Aux[Dias]) -- comentário\nRETURN CALCULATE(DIVIDE(SUM('Minha Tab'[Qtd]), 5), 'Minha Tab'[Data] >= h, ALL(T[x]))").unwrap();
        let Node::Vars(v, r) = n else { panic!() };
        assert_eq!(v[0].0, "h");
        let Node::Func(f, a) = *r else { panic!() };
        assert_eq!((f.as_str(), a.len()), ("CALCULATE", 3));
        assert!(
            matches!(&a[1], Node::Bin(Op::Ge, l, _) if **l == Node::Col("Minha Tab".into(), "Data".into()))
        );
        assert_eq!(
            parse("[a]-[b]").unwrap(),
            Node::Bin(Op::Sub, Box::new(Node::Bracket("a".into())), Box::new(Node::Bracket("b".into())))
        );
        assert!(parse("SUM(").is_err());
        assert!(matches!(parse("NOT ISBLANK([x]) && 1 <> 2").unwrap(), Node::Bin(Op::And, ..)));
    }

    #[test]
    fn script() {
        let s = "CALCULATE;\nCREATE MEASURE 'Tab um'[Total; x]=SUM('Tab um'[v]); -- nada\nCREATE MEASURE [Sandbox].Outra[Contagem]=COUNTA('Outra'[c]);\nCREATE MEASURE T[Com ]] chave]=VAR a = \"x;y\" RETURN a;\n";
        let m = measures(s);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], ("Tab um".into(), "Total; x".into(), "SUM('Tab um'[v])".into()));
        assert_eq!((m[1].0.as_str(), m[1].1.as_str()), ("Outra", "Contagem"));
        assert_eq!((m[2].1.as_str(), m[2].2.as_str()), ("Com ] chave", "VAR a = \"x;y\" RETURN a"));
    }
}
