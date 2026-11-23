//! Analisador do VBA: o bastante para macros de planilha (declarações, laços,
//! `With`, tratamento de erro, objetos, matrizes). O que não é conhecido vira
//! um erro de compilação da rotina, não do módulo inteiro.

use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Id(String),
    Str(String),
    Num(f64),
    Op(&'static str),
    /// Fim da linha.
    End,
    /// `:` entre instruções da mesma linha.
    Colon,
    /// Rótulo no começo da linha (`nome:`).
    Label(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bin {
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Is,
    Like,
    And,
    Or,
    Xor,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    Nothing,
    Empty,
    Null,
    /// Nome solto: variável, função ou objeto global.
    Name(String),
    /// `.membro` dentro de `With`.
    With(String),
    Member(Box<Expr>, String),
    /// Chamada ou índice: `x(a, b)`; argumentos podem ter nome ou faltar.
    Call(Box<Expr>, Vec<Arg>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(Bin, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OnError {
    ResumeNext,
    Goto(String),
    Off,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dim {
    pub name: String,
    pub ty: String,
    /// `Some` = matriz; limites (menor, maior) por dimensão, ou vazio se
    /// dinâmica.
    pub bounds: Option<Vec<(Option<Expr>, Expr)>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Dim(Vec<Dim>),
    ReDim {
        preserve: bool,
        list: Vec<Dim>,
    },
    Const(Vec<(String, Expr)>),
    Let(Expr, Expr),
    Set(Expr, Expr),
    Call(Expr),
    If(Vec<(Expr, Vec<Stmt>)>, Vec<Stmt>),
    For {
        var: Expr,
        from: Expr,
        to: Expr,
        step: Option<Expr>,
        body: Vec<Stmt>,
    },
    ForEach {
        var: Expr,
        list: Expr,
        body: Vec<Stmt>,
    },
    /// `pre`: condição antes; `until`: inverte; `None` = laço sem condição.
    Do {
        cond: Option<(Expr, bool)>,
        pre: bool,
        body: Vec<Stmt>,
    },
    With(Expr, Vec<Stmt>),
    Select(Expr, Vec<(Vec<Case>, Vec<Stmt>)>),
    OnError(OnError),
    Resume(Option<String>),
    Goto(String),
    Label(String),
    ExitSub,
    ExitFor,
    ExitDo,
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Case {
    Value(Expr),
    Range(Expr, Expr),
    Is(Bin, Expr),
    Else,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub optional: Option<Expr>,
    pub by_val: bool,
}

#[derive(Debug)]
pub struct Proc {
    pub name: String,
    pub module: String,
    pub is_function: bool,
    pub params: Vec<Param>,
    /// Corpo, ou a mensagem do que não foi entendido.
    pub body: Result<Rc<Vec<Stmt>>, String>,
}

/// Módulo analisado: rotinas e declarações do topo.
#[derive(Debug, Default)]
pub struct Unit {
    pub procs: Vec<Proc>,
    pub globals: Vec<Stmt>,
}

pub fn lex(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    for line in logical_lines(src) {
        lex_line(&line, &mut out);
        out.push(Tok::End);
    }
    out
}

/// Junta linhas continuadas por ` _`.
fn logical_lines(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for l in src.lines() {
        let t = l.trim_end();
        if let Some(head) = t.strip_suffix(" _").or_else(|| t.strip_suffix("\t_")) {
            cur.push_str(head);
            cur.push(' ');
            continue;
        }
        cur.push_str(t);
        out.push(std::mem::take(&mut cur));
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn lex_line(line: &str, out: &mut Vec<Tok>) {
    let c: Vec<char> = line.chars().collect();
    let mut i = 0;
    let first = out.len();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '\'' {
            break;
        } else if ch == '"' {
            let mut s = String::new();
            i += 1;
            while i < c.len() {
                if c[i] == '"' {
                    if c.get(i + 1) == Some(&'"') {
                        s.push('"');
                        i += 2;
                        continue;
                    }
                    break;
                }
                s.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if ch == '#' {
            // Data literal: fica como texto, convertida na avaliação.
            let end = (i + 1..c.len()).find(|&k| c[k] == '#').unwrap_or(c.len());
            out.push(Tok::Str(c[i + 1..end].iter().collect()));
            i = end + 1;
        } else if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            if matches!(c.get(i), Some('e' | 'E'))
                && c.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == '-' || *d == '+')
            {
                i += 2;
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let t: String = c[start..i].iter().collect();
            // Sufixos de tipo: `5&`, `3#`, `2!`.
            if matches!(c.get(i), Some('&' | '#' | '!' | '%' | '@')) {
                i += 1;
            }
            out.push(Tok::Num(t.parse().unwrap_or(0.0)));
        } else if ch == '&' && matches!(c.get(i + 1), Some('H' | 'h')) {
            let start = i + 2;
            i = start;
            while i < c.len() && c[i].is_ascii_hexdigit() {
                i += 1;
            }
            let t: String = c[start..i].iter().collect();
            if c.get(i) == Some(&'&') {
                i += 1;
            }
            out.push(Tok::Num(i64::from_str_radix(&t, 16).unwrap_or(0) as f64));
        } else if ch.is_alphabetic() || ch == '_' || ch == '[' {
            if ch == '[' {
                let end = (i + 1..c.len()).find(|&k| c[k] == ']').unwrap_or(c.len());
                out.push(Tok::Id(c[i + 1..end].iter().collect()));
                i = end + 1;
                continue;
            }
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            let id: String = c[start..i].iter().collect();
            if id.eq_ignore_ascii_case("rem") {
                break;
            }
            // Sufixo de tipo do nome (`nome$`).
            if matches!(c.get(i), Some('$' | '%' | '#')) {
                i += 1;
            }
            // `rotulo:` no começo da linha.
            if out.len() == first
                && c.get(i) == Some(&':')
                && c.get(i + 1) != Some(&'=')
                && !KEYWORDS.contains(&id.to_ascii_lowercase().as_str())
            {
                out.push(Tok::Label(id));
                out.push(Tok::End);
                i += 1;
                continue;
            }
            out.push(Tok::Id(id));
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            let op: Option<&'static str> = match two.as_str() {
                "<>" => Some("<>"),
                "<=" => Some("<="),
                ">=" => Some(">="),
                ":=" => Some(":="),
                _ => None,
            };
            if let Some(op) = op {
                out.push(Tok::Op(op));
                i += 2;
                continue;
            }
            let op: &'static str = match ch {
                '+' => "+",
                '-' => "-",
                '*' => "*",
                '/' => "/",
                '\\' => "\\",
                '^' => "^",
                '&' => "&",
                '=' => "=",
                '<' => "<",
                '>' => ">",
                '(' => "(",
                ')' => ")",
                ',' => ",",
                '.' => ".",
                '!' => "!",
                ';' => ";",
                ':' => {
                    out.push(Tok::Colon);
                    i += 1;
                    continue;
                }
                _ => "?",
            };
            out.push(Tok::Op(op));
            i += 1;
        }
    }
}

const KEYWORDS: [&str; 4] = ["else", "do", "loop", "wend"];

type Res<T> = Result<T, String>;

struct Parser {
    t: Vec<Tok>,
    i: usize,
    /// Blocos abertos.
    depth: u32,
}

/// Analisa um módulo.
pub fn parse_module(name: &str, src: &str) -> Unit {
    let mut p = Parser { t: lex(src), i: 0, depth: 0 };
    let mut unit = Unit::default();
    while p.i < p.t.len() {
        p.skip_ends();
        if p.i >= p.t.len() {
            break;
        }
        let start = p.i;
        // Modificadores antes de Sub/Function/Const/Dim.
        let mut public = false;
        while p.is_kw("public") || p.is_kw("private") || p.is_kw("friend") || p.is_kw("static") {
            public = true;
            p.i += 1;
        }
        if p.is_kw("sub") || p.is_kw("function") || p.is_kw("property") {
            let is_function = !p.is_kw("sub");
            let kind = if p.is_kw("sub") {
                "sub"
            } else if p.is_kw("function") {
                "function"
            } else {
                "property"
            };
            p.i += 1;
            if kind == "property" {
                p.i += 1; // Get/Let/Set
            }
            let pname = p.ident().unwrap_or_default();
            let params = p.params();
            p.skip_line();
            let body_start = p.i;
            let parsed = match params {
                Ok(params) => {
                    let body = p.block(&[("end", kind)]);
                    match body {
                        Ok(b) => {
                            p.i += 2;
                            (params, Ok(Rc::new(b)))
                        }
                        Err(e) => {
                            p.i = body_start;
                            p.skip_to_end(kind);
                            (params, Err(e))
                        }
                    }
                }
                Err(e) => {
                    p.skip_to_end(kind);
                    (Vec::new(), Err(e))
                }
            };
            unit.procs.push(Proc {
                name: pname,
                module: name.to_string(),
                is_function,
                params: parsed.0,
                body: parsed.1,
            });
            continue;
        }
        if p.is_kw("dim") || p.is_kw("const") || (public && matches!(p.peek(), Some(Tok::Id(_)))) {
            if public && !p.is_kw("const") && !p.is_kw("dim") {
                // `Private x As Long` no topo: declaração.
                if let Ok(d) = p.dims() {
                    unit.globals.push(Stmt::Dim(d));
                    continue;
                }
            } else if let Ok(s) = p.stmt() {
                unit.globals.push(s);
                continue;
            }
            p.i = start;
        }
        p.skip_line();
    }
    unit
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn is_kw(&self, k: &str) -> bool {
        self.kw_at(self.i, k)
    }

    fn kw_at(&self, i: usize, k: &str) -> bool {
        matches!(self.t.get(i), Some(Tok::Id(s)) if s.eq_ignore_ascii_case(k))
    }

    fn is_op(&self, o: &str) -> bool {
        matches!(self.peek(), Some(Tok::Op(x)) if *x == o)
    }

    fn eat_kw(&mut self, k: &str) -> bool {
        let y = self.is_kw(k);
        self.i += y as usize;
        y
    }

    fn eat_op(&mut self, o: &str) -> bool {
        let y = self.is_op(o);
        self.i += y as usize;
        y
    }

    fn want_op(&mut self, o: &str) -> Res<()> {
        if self.eat_op(o) { Ok(()) } else { Err(format!("esperado '{o}', veio {:?}", self.peek())) }
    }

    fn want_kw(&mut self, k: &str) -> Res<()> {
        if self.eat_kw(k) { Ok(()) } else { Err(format!("esperado {k}, veio {:?}", self.peek())) }
    }

    fn ident(&mut self) -> Res<String> {
        match self.peek() {
            Some(Tok::Id(s)) => {
                let s = s.clone();
                self.i += 1;
                Ok(s)
            }
            t => Err(format!("esperado um nome, veio {t:?}")),
        }
    }

    fn at_end(&self) -> bool {
        matches!(self.peek(), None | Some(Tok::End | Tok::Colon))
    }

    fn skip_ends(&mut self) {
        while matches!(self.peek(), Some(Tok::End | Tok::Colon)) {
            self.i += 1;
        }
    }

    fn skip_line(&mut self) {
        while !matches!(self.peek(), None | Some(Tok::End)) {
            self.i += 1;
        }
        self.i += 1;
    }

    fn skip_to_end(&mut self, kind: &str) {
        while self.i < self.t.len() {
            if self.is_kw("end") && self.kw_at(self.i + 1, kind) {
                self.i += 2;
                return;
            }
            self.i += 1;
        }
    }

    fn params(&mut self) -> Res<Vec<Param>> {
        let mut out = Vec::new();
        if !self.eat_op("(") {
            return Ok(out);
        }
        while !self.eat_op(")") {
            let mut optional = false;
            let mut by_val = false;
            loop {
                if self.eat_kw("optional") {
                    optional = true;
                } else if self.eat_kw("byval") {
                    by_val = true;
                } else if !self.eat_kw("byref") && !self.eat_kw("paramarray") {
                    break;
                }
            }
            let name = self.ident()?;
            if self.eat_op("(") {
                self.want_op(")")?;
            }
            let ty = if self.eat_kw("as") { self.type_name()? } else { "variant".into() };
            let mut default = optional.then_some(Expr::Empty);
            if self.eat_op("=") {
                default = Some(self.expr()?);
            }
            out.push(Param { name, ty, optional: default, by_val });
            if !self.eat_op(",") && !self.is_op(")") {
                return Err("lista de parâmetros".into());
            }
        }
        Ok(out)
    }

    fn type_name(&mut self) -> Res<String> {
        self.eat_kw("new");
        let mut s = self.ident()?;
        while self.eat_op(".") {
            s = self.ident()?;
        }
        Ok(s.to_ascii_lowercase())
    }

    fn dims(&mut self) -> Res<Vec<Dim>> {
        let mut out = Vec::new();
        loop {
            let name = self.ident()?;
            let mut bounds = None;
            if self.eat_op("(") {
                let mut list = Vec::new();
                while !self.eat_op(")") {
                    let a = self.expr()?;
                    let b = if self.eat_kw("to") { (Some(a), self.expr()?) } else { (None, a) };
                    list.push(b);
                    if !self.eat_op(",") && !self.is_op(")") {
                        return Err("limites da matriz".into());
                    }
                }
                bounds = Some(list);
            }
            let ty = if self.eat_kw("as") { self.type_name()? } else { "variant".into() };
            out.push(Dim { name, ty, bounds });
            if !self.eat_op(",") {
                return Ok(out);
            }
        }
    }

    /// Instruções até um dos fechos (`("end", "if")`, `("next", "")`...), sem
    /// consumi-lo.
    fn block(&mut self, stop: &[(&str, &str)]) -> Res<Vec<Stmt>> {
        let mut out = Vec::new();
        self.depth += 1;
        if self.depth > 64 {
            return Err("blocos demais".into());
        }
        loop {
            self.skip_ends();
            if self.i >= self.t.len() {
                return Err(format!("faltou fechar o bloco {stop:?}"));
            }
            if stop.iter().any(|(a, b)| self.is_kw(a) && (b.is_empty() || self.kw_at(self.i + 1, b))) {
                self.depth -= 1;
                return Ok(out);
            }
            let s = self.stmt()?;
            out.push(s);
        }
    }

    fn stmt(&mut self) -> Res<Stmt> {
        if let Some(Tok::Label(l)) = self.peek() {
            let l = l.clone();
            self.i += 1;
            return Ok(Stmt::Label(l.to_ascii_lowercase()));
        }
        let kw = match self.peek() {
            Some(Tok::Id(s)) => s.to_ascii_lowercase(),
            _ => String::new(),
        };
        match kw.as_str() {
            "dim" | "static" => {
                self.i += 1;
                Ok(Stmt::Dim(self.dims()?))
            }
            "redim" => {
                self.i += 1;
                let preserve = self.eat_kw("preserve");
                Ok(Stmt::ReDim { preserve, list: self.dims()? })
            }
            "const" => {
                self.i += 1;
                let mut list = Vec::new();
                loop {
                    let name = self.ident()?;
                    if self.eat_kw("as") {
                        self.type_name()?;
                    }
                    self.want_op("=")?;
                    list.push((name, self.expr()?));
                    if !self.eat_op(",") {
                        return Ok(Stmt::Const(list));
                    }
                }
            }
            "set" => {
                self.i += 1;
                let target = self.postfix()?;
                self.want_op("=")?;
                self.eat_kw("new");
                Ok(Stmt::Set(target, self.expr()?))
            }
            "let" => {
                self.i += 1;
                self.assign()
            }
            "call" => {
                self.i += 1;
                Ok(Stmt::Call(self.postfix()?))
            }
            "if" => self.if_stmt(),
            "for" => self.for_stmt(),
            "do" => self.do_stmt(),
            "while" => {
                self.i += 1;
                let c = self.expr()?;
                let body = self.block(&[("wend", "")])?;
                self.i += 1;
                Ok(Stmt::Do { cond: Some((c, false)), pre: true, body })
            }
            "with" => {
                self.i += 1;
                let e = self.expr()?;
                let body = self.block(&[("end", "with")])?;
                self.i += 2;
                Ok(Stmt::With(e, body))
            }
            "select" => self.select(),
            "on" if self.kw_at(self.i + 1, "error") => {
                self.i += 2;
                if self.eat_kw("resume") {
                    self.want_kw("next")?;
                    return Ok(Stmt::OnError(OnError::ResumeNext));
                }
                self.want_kw("goto")?;
                if let Some(Tok::Num(_)) = self.peek() {
                    self.i += 1;
                    return Ok(Stmt::OnError(OnError::Off));
                }
                if self.eat_op("-") {
                    self.i += 1;
                    return Ok(Stmt::OnError(OnError::Off));
                }
                Ok(Stmt::OnError(OnError::Goto(self.ident()?.to_ascii_lowercase())))
            }
            "resume" => {
                self.i += 1;
                if self.at_end() {
                    return Err("Resume sem destino".into());
                }
                if self.eat_kw("next") {
                    return Ok(Stmt::Resume(None));
                }
                Ok(Stmt::Resume(Some(self.ident()?.to_ascii_lowercase())))
            }
            "goto" => {
                self.i += 1;
                Ok(Stmt::Goto(self.ident()?.to_ascii_lowercase()))
            }
            "exit" => {
                self.i += 1;
                let what = self.ident()?.to_ascii_lowercase();
                match what.as_str() {
                    "sub" | "function" | "property" => Ok(Stmt::ExitSub),
                    "for" => Ok(Stmt::ExitFor),
                    "do" => Ok(Stmt::ExitDo),
                    w => Err(format!("Exit {w}")),
                }
            }
            "end" if matches!(self.t.get(self.i + 1), None | Some(Tok::End | Tok::Colon)) => {
                self.i += 1;
                Ok(Stmt::End)
            }
            _ => self.assign(),
        }
    }

    /// `alvo = valor` ou a chamada `nome arg, arg`.
    fn assign(&mut self) -> Res<Stmt> {
        let target = self.postfix()?;
        if self.eat_op("=") {
            return Ok(Stmt::Let(target, self.expr()?));
        }
        if self.at_end() || self.is_kw("else") {
            return Ok(Stmt::Call(target));
        }
        // Chamada sem parênteses.
        let args = self.args(None)?;
        Ok(Stmt::Call(Expr::Call(Box::new(target), args)))
    }

    fn if_stmt(&mut self) -> Res<Stmt> {
        self.i += 1;
        let cond = self.expr()?;
        self.want_kw("then")?;
        if !matches!(self.peek(), None | Some(Tok::End)) {
            // Numa linha só.
            let mut yes = Vec::new();
            let mut no = Vec::new();
            let mut other = false;
            loop {
                while matches!(self.peek(), Some(Tok::Colon)) {
                    self.i += 1;
                }
                if matches!(self.peek(), None | Some(Tok::End)) {
                    break;
                }
                if self.eat_kw("else") {
                    other = true;
                    continue;
                }
                let s = self.stmt()?;
                if other { no.push(s) } else { yes.push(s) }
            }
            return Ok(Stmt::If(vec![(cond, yes)], no));
        }
        let stop = [("elseif", ""), ("else", ""), ("end", "if")];
        let mut arms = vec![(cond, self.block(&stop)?)];
        let mut no = Vec::new();
        loop {
            if self.eat_kw("elseif") {
                let c = self.expr()?;
                self.want_kw("then")?;
                arms.push((c, self.block(&stop)?));
            } else if self.eat_kw("else") {
                no = self.block(&[("end", "if")])?;
            } else {
                self.i += 2;
                return Ok(Stmt::If(arms, no));
            }
        }
    }

    fn for_stmt(&mut self) -> Res<Stmt> {
        self.i += 1;
        if self.eat_kw("each") {
            let var = self.postfix()?;
            self.want_kw("in")?;
            let list = self.expr()?;
            let body = self.block(&[("next", "")])?;
            self.next_tail();
            return Ok(Stmt::ForEach { var, list, body });
        }
        let var = self.postfix()?;
        self.want_op("=")?;
        let from = self.expr()?;
        self.want_kw("to")?;
        let to = self.expr()?;
        let step = if self.eat_kw("step") { Some(self.expr()?) } else { None };
        let body = self.block(&[("next", "")])?;
        self.next_tail();
        Ok(Stmt::For { var, from, to, step, body })
    }

    fn next_tail(&mut self) {
        self.i += 1;
        if let Some(Tok::Id(_)) = self.peek() {
            self.i += 1;
        }
    }

    fn do_stmt(&mut self) -> Res<Stmt> {
        self.i += 1;
        let mut cond = None;
        let mut pre = false;
        if self.is_kw("while") || self.is_kw("until") {
            let until = self.is_kw("until");
            self.i += 1;
            cond = Some((self.expr()?, until));
            pre = true;
        }
        let body = self.block(&[("loop", "")])?;
        self.i += 1;
        if self.is_kw("while") || self.is_kw("until") {
            let until = self.is_kw("until");
            self.i += 1;
            cond = Some((self.expr()?, until));
        }
        Ok(Stmt::Do { cond, pre, body })
    }

    fn select(&mut self) -> Res<Stmt> {
        self.i += 1;
        self.want_kw("case")?;
        let e = self.expr()?;
        self.skip_ends();
        let mut arms = Vec::new();
        while !(self.is_kw("end") && self.kw_at(self.i + 1, "select")) {
            self.want_kw("case")?;
            let mut cases = Vec::new();
            loop {
                if self.eat_kw("else") {
                    cases.push(Case::Else);
                } else if self.eat_kw("is") {
                    let op = self.cmp_op().ok_or("Case Is")?;
                    cases.push(Case::Is(op, self.additive()?));
                } else {
                    let a = self.expr()?;
                    if self.eat_kw("to") {
                        cases.push(Case::Range(a, self.expr()?));
                    } else {
                        cases.push(Case::Value(a));
                    }
                }
                if !self.eat_op(",") {
                    break;
                }
            }
            let body = self.block(&[("case", ""), ("end", "select")])?;
            arms.push((cases, body));
        }
        self.i += 2;
        Ok(Stmt::Select(e, arms))
    }

    // --- expressões (da menor para a maior precedência) --------------------

    pub fn expr(&mut self) -> Res<Expr> {
        let mut a = self.and()?;
        loop {
            let op = if self.eat_kw("or") {
                Bin::Or
            } else if self.eat_kw("xor") {
                Bin::Xor
            } else {
                return Ok(a);
            };
            let b = self.and()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }

    fn and(&mut self) -> Res<Expr> {
        let mut a = self.not()?;
        while self.eat_kw("and") {
            let b = self.not()?;
            a = Expr::Bin(Bin::And, Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn not(&mut self) -> Res<Expr> {
        if self.eat_kw("not") {
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.comparison()
    }

    fn cmp_op(&mut self) -> Option<Bin> {
        let op = match self.peek()? {
            Tok::Op("=") => Bin::Eq,
            Tok::Op("<>") => Bin::Ne,
            Tok::Op("<") => Bin::Lt,
            Tok::Op("<=") => Bin::Le,
            Tok::Op(">") => Bin::Gt,
            Tok::Op(">=") => Bin::Ge,
            Tok::Id(s) if s.eq_ignore_ascii_case("is") => Bin::Is,
            Tok::Id(s) if s.eq_ignore_ascii_case("like") => Bin::Like,
            _ => return None,
        };
        self.i += 1;
        Some(op)
    }

    fn comparison(&mut self) -> Res<Expr> {
        let mut a = self.concat()?;
        while let Some(op) = self.cmp_op() {
            let b = self.concat()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn concat(&mut self) -> Res<Expr> {
        let mut a = self.additive()?;
        while self.eat_op("&") {
            let b = self.additive()?;
            a = Expr::Bin(Bin::Concat, Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn additive(&mut self) -> Res<Expr> {
        let mut a = self.modulo()?;
        loop {
            let op = if self.eat_op("+") {
                Bin::Add
            } else if self.eat_op("-") {
                Bin::Sub
            } else {
                return Ok(a);
            };
            let b = self.modulo()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }

    fn modulo(&mut self) -> Res<Expr> {
        let mut a = self.int_div()?;
        while self.eat_kw("mod") {
            let b = self.int_div()?;
            a = Expr::Bin(Bin::Mod, Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn int_div(&mut self) -> Res<Expr> {
        let mut a = self.mul()?;
        while self.eat_op("\\") {
            let b = self.mul()?;
            a = Expr::Bin(Bin::IntDiv, Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn mul(&mut self) -> Res<Expr> {
        let mut a = self.unary()?;
        loop {
            let op = if self.eat_op("*") {
                Bin::Mul
            } else if self.eat_op("/") {
                Bin::Div
            } else {
                return Ok(a);
            };
            let b = self.unary()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }

    fn unary(&mut self) -> Res<Expr> {
        if self.eat_op("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.eat_op("+") {
            return self.unary();
        }
        let a = self.postfix()?;
        if self.eat_op("^") {
            let b = self.unary()?;
            return Ok(Expr::Bin(Bin::Pow, Box::new(a), Box::new(b)));
        }
        Ok(a)
    }

    fn postfix(&mut self) -> Res<Expr> {
        let mut e = match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.i += 1;
                Expr::Num(n)
            }
            Some(Tok::Str(s)) => {
                self.i += 1;
                Expr::Str(s)
            }
            Some(Tok::Op("(")) => {
                self.i += 1;
                let e = self.expr()?;
                self.want_op(")")?;
                e
            }
            Some(Tok::Op(".")) => {
                self.i += 1;
                Expr::With(self.ident()?)
            }
            Some(Tok::Id(s)) => {
                self.i += 1;
                match s.to_ascii_lowercase().as_str() {
                    "true" => Expr::Bool(true),
                    "false" => Expr::Bool(false),
                    "nothing" => Expr::Nothing,
                    "empty" => Expr::Empty,
                    "null" => Expr::Null,
                    "new" => {
                        let n = self.ident()?;
                        Expr::Name(format!("New {n}"))
                    }
                    _ => Expr::Name(s),
                }
            }
            t => return Err(format!("expressão inesperada: {t:?}")),
        };
        loop {
            if self.eat_op(".") || self.eat_op("!") {
                e = Expr::Member(Box::new(e), self.ident()?);
            } else if self.is_op("(") {
                self.i += 1;
                let args = self.args(Some(")"))?;
                e = Expr::Call(Box::new(e), args);
            } else {
                return Ok(e);
            }
        }
    }

    /// Argumentos até `close` (consumido) ou o fim da instrução.
    fn args(&mut self, close: Option<&str>) -> Res<Vec<Arg>> {
        let mut out = Vec::new();
        if let Some(c) = close
            && self.eat_op(c)
        {
            return Ok(out);
        }
        loop {
            if self.is_op(",") || close.is_some_and(|c| self.is_op(c)) {
                out.push(Arg { name: None, value: None });
            } else {
                self.eat_kw("byval");
                let named = matches!(self.peek(), Some(Tok::Id(_)))
                    && matches!(self.t.get(self.i + 1), Some(Tok::Op(":=")));
                let name = if named {
                    let n = self.ident()?;
                    self.i += 1;
                    Some(n.to_ascii_lowercase())
                } else {
                    None
                };
                out.push(Arg { name, value: Some(self.expr()?) });
            }
            if self.eat_op(",") {
                continue;
            }
            match close {
                Some(c) => {
                    self.want_op(c)?;
                    return Ok(out);
                }
                None if self.at_end() || self.is_kw("else") => return Ok(out),
                None => return Err(format!("argumentos: {:?}", self.peek())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_shapes() {
        let u = parse_module(
            "M",
            "Option Explicit\nPrivate total As Long\nPublic Sub A()\n  Dim x As Integer, y() As Variant\n  If x < 1 Or x > UBound(y) + 1 Then Exit Sub\n  For Each c In pt.RowFields\n    c.Orientation = xlHidden\n  Next c\nfim:\n  MsgBox \"a \"\"b\"\"\" & _\n    vbNewLine, vbCritical\nEnd Sub\nFunction F(ByVal s As String, Optional n As Long = 2) As String\n  F = UCase(s): F = Replace(F, \".\", \"\")\nEnd Function\nSub Ruim()\n  Isto ? não\nEnd Sub\n",
        );
        assert_eq!(u.globals.len(), 1);
        let names: Vec<&str> = u.procs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A", "F", "Ruim"]);
        let body = u.procs[0].body.as_ref().unwrap();
        assert!(matches!(body[0], Stmt::Dim(ref d) if d.len() == 2 && d[1].bounds == Some(vec![])));
        assert!(matches!(&body[1], Stmt::If(a, _) if a[0].1 == [Stmt::ExitSub]));
        assert!(matches!(&body[2], Stmt::ForEach { body, .. } if body.len() == 1));
        assert_eq!(body[3], Stmt::Label("fim".into()));
        let Stmt::Call(Expr::Call(f, args)) = &body[4] else { panic!("{:?}", body[4]) };
        assert_eq!((&**f, args.len()), (&Expr::Name("MsgBox".into()), 2));
        assert_eq!(u.procs[1].params[1].optional, Some(Expr::Num(2.0)));
        assert_eq!(u.procs[1].body.as_ref().unwrap().len(), 2);
        assert!(u.procs[2].body.is_err());
    }

    #[test]
    fn precedence_and_named_args() {
        let u = parse_module(
            "M",
            "Sub A()\n r.Sort Key1:=.Cells(1, 2), Order1:=xlDescending\n x = 1 + 2 * 3 & \"a\" = \"7a\" And Not b Is Nothing\nEnd Sub",
        );
        let b = u.procs[0].body.as_ref().unwrap();
        let Stmt::Call(Expr::Call(_, args)) = &b[0] else { panic!() };
        assert_eq!(args[0].name.as_deref(), Some("key1"));
        let Stmt::Let(_, Expr::Bin(Bin::And, l, r)) = &b[1] else { panic!("{:?}", b[1]) };
        assert!(matches!(**l, Expr::Bin(Bin::Eq, ..)));
        assert!(matches!(**r, Expr::Not(_)));
    }
}
