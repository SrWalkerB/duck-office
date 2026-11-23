//! Fórmula (texto A1 como está no arquivo: nomes de função em inglês,
//! separador `,`) → árvore compilada.
//!
//! A árvore não depende da célula: cada coordenada é absoluta (`$`) ou um
//! deslocamento a partir da célula avaliada. Assim uma fórmula copiada para
//! um milhão de linhas (modelo relativo ou grupo compartilhado) é compilada
//! uma vez só. Nomes de aba, referências estruturadas de tabela
//! (`Tabela1[[#This Row],[Valor]]`) e nomes definidos são resolvidos aqui.

use duck_core::{CellError, CellRange};

use crate::funcs::{self, Func};

/// Última linha e coluna (base 0).
pub const MAX_ROW: u32 = 1_048_575;
pub const MAX_COL: u32 = 16_383;

/// Uma coordenada: absoluta (índice base 0) ou deslocamento a partir da célula.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coord {
    pub abs: bool,
    pub v: i32,
}

impl Coord {
    pub const fn abs(v: u32) -> Self {
        Self { abs: true, v: v as i32 }
    }

    pub const fn rel(v: i32) -> Self {
        Self { abs: false, v }
    }

    /// Valor na célula de origem `origin` (pode sair da planilha).
    #[inline]
    pub fn at(self, origin: u32) -> i64 {
        if self.abs { self.v as i64 } else { origin as i64 + self.v as i64 }
    }
}

/// De que aba é a referência.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetSel {
    /// A aba da célula avaliada.
    This,
    Local(u32),
    /// Aba `sheet` do arquivo externo `book` (`[book+1]`).
    Ext {
        book: u16,
        sheet: u16,
    },
}

/// Retângulo de células (inclusivo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub sheet: SheetSel,
    pub r0: Coord,
    pub c0: Coord,
    pub r1: Coord,
    pub c1: Coord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Cat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Num(f64),
    Str(Box<str>),
    Bool(bool),
    Err(CellError),
    /// Argumento vazio (`SE(A1;;2)`).
    Missing,
    Area(Area),
    Neg(u32),
    Pct(u32),
    Bin(BinOp, u32, u32),
    /// Função: argumentos em `Formula::args[start..start + len]`.
    Call {
        f: Func,
        start: u32,
        len: u32,
    },
    /// Nome definido (índice na lista de nomes do arquivo).
    Name(u32),
    /// Constante matricial `{1,2;3,4}`: elementos (nós) em `args`.
    Array {
        rows: u32,
        cols: u32,
        start: u32,
    },
    /// `ref:ref` em que um dos lados não é uma referência simples
    /// (`A1:ÍNDICE(...)`).
    Range(u32, u32),
}

/// Bits de [`Formula::flags`].
pub mod flag {
    /// Tem função volátil (HOJE, AGORA, ALEATÓRIO...): recalcula sempre.
    pub const VOLATILE: u8 = 1;
    /// Referências que só se sabem ao calcular (INDIRETO, DESLOC).
    pub const DYNAMIC: u8 = 2;
    /// Usa algo que o motor não sabe calcular: o valor guardado fica.
    pub const OPAQUE: u8 = 4;
}

/// Fórmula compilada.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Formula {
    pub nodes: Vec<Node>,
    pub args: Vec<u32>,
    pub root: u32,
    pub flags: u8,
}

impl Formula {
    pub fn node(&self, i: u32) -> &Node {
        &self.nodes[i as usize]
    }

    pub fn call_args(&self, start: u32, len: u32) -> &[u32] {
        &self.args[start as usize..(start + len) as usize]
    }

    /// Fórmula que é só um erro (texto que não deu para entender).
    pub fn error(e: CellError) -> Self {
        Self { nodes: vec![Node::Err(e)], args: Vec::new(), root: 0, flags: 0 }
    }
}

/// Tabela do Excel vista pelo compilador.
pub struct TableInfo<'a> {
    pub sheet: u32,
    pub range: CellRange,
    pub header_rows: u32,
    pub totals_rows: u32,
    pub columns: &'a [String],
}

/// O que o compilador precisa saber do arquivo.
pub trait Resolve {
    /// Índice da aba (sem diferenciar maiúsculas).
    fn sheet(&self, name: &str) -> Option<u32>;
    /// Aba de um arquivo externo (`book` base 0).
    fn ext_sheet(&self, book: u16, name: &str) -> Option<u16>;
    /// Nome definido visível da aba `sheet` (os da aba vencem os do arquivo).
    fn name(&self, name: &str, sheet: u32) -> Option<u32>;
    fn table(&self, name: &str) -> Option<TableInfo<'_>>;
    /// Tabela que contém a célula (para `[@Coluna]` sem o nome da tabela).
    fn table_at(&self, sheet: u32, row: u32, col: u32) -> Option<TableInfo<'_>>;
}

/// Compila `text` (sem o `=`) para a célula (`row`, `col`, base 0) da aba `sheet`.
/// Texto que não dá para entender vira `#NOME?`, como no Excel.
pub fn compile(text: &str, sheet: u32, row: u32, col: u32, res: &dyn Resolve) -> Formula {
    let mut p = Parser { s: text.as_bytes(), text, i: 0, sheet, row, col, res, f: Formula::default() };
    match p.expr().and_then(|root| {
        p.ws();
        if p.i < p.s.len() { Err(()) } else { Ok(root) }
    }) {
        Ok(root) => {
            p.f.root = root;
            p.f
        }
        Err(()) => Formula::error(CellError::Name),
    }
}

type R<T> = Result<T, ()>;

struct Parser<'s> {
    s: &'s [u8],
    text: &'s str,
    i: usize,
    sheet: u32,
    row: u32,
    col: u32,
    res: &'s dyn Resolve,
    f: Formula,
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'$' | b'\\' | b'?') || c >= 0x80
}

impl Parser<'_> {
    fn push(&mut self, n: Node) -> u32 {
        self.f.nodes.push(n);
        (self.f.nodes.len() - 1) as u32
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    // Precedência (da menor para a maior): comparação, &, + -, * /, ^, - unário, %, :.
    fn expr(&mut self) -> R<u32> {
        let mut l = self.concat()?;
        loop {
            self.ws();
            let rest = &self.s[self.i..];
            let (op, n) = if rest.starts_with(b"<=") {
                (BinOp::Le, 2)
            } else if rest.starts_with(b">=") {
                (BinOp::Ge, 2)
            } else if rest.starts_with(b"<>") {
                (BinOp::Ne, 2)
            } else if rest.starts_with(b"=") {
                (BinOp::Eq, 1)
            } else if rest.starts_with(b"<") {
                (BinOp::Lt, 1)
            } else if rest.starts_with(b">") {
                (BinOp::Gt, 1)
            } else {
                return Ok(l);
            };
            self.i += n;
            let r = self.concat()?;
            l = self.push(Node::Bin(op, l, r));
        }
    }

    fn concat(&mut self) -> R<u32> {
        let mut l = self.additive()?;
        while self.eat(b'&') {
            let r = self.additive()?;
            l = self.push(Node::Bin(BinOp::Cat, l, r));
        }
        Ok(l)
    }

    fn additive(&mut self) -> R<u32> {
        let mut l = self.term()?;
        loop {
            let op = if self.eat(b'+') {
                BinOp::Add
            } else if self.eat(b'-') {
                BinOp::Sub
            } else {
                return Ok(l);
            };
            let r = self.term()?;
            l = self.push(Node::Bin(op, l, r));
        }
    }

    fn term(&mut self) -> R<u32> {
        let mut l = self.power()?;
        loop {
            let op = if self.eat(b'*') {
                BinOp::Mul
            } else if self.eat(b'/') {
                BinOp::Div
            } else {
                return Ok(l);
            };
            let r = self.power()?;
            l = self.push(Node::Bin(op, l, r));
        }
    }

    fn power(&mut self) -> R<u32> {
        let mut l = self.unary()?;
        while self.eat(b'^') {
            let r = self.unary()?;
            l = self.push(Node::Bin(BinOp::Pow, l, r));
        }
        Ok(l)
    }

    fn unary(&mut self) -> R<u32> {
        if self.eat(b'-') {
            let x = self.unary()?;
            return Ok(match self.f.nodes[x as usize] {
                Node::Num(v) => {
                    self.f.nodes[x as usize] = Node::Num(-v);
                    x
                }
                _ => self.push(Node::Neg(x)),
            });
        }
        if self.eat(b'+') {
            return self.unary();
        }
        self.postfix()
    }

    fn postfix(&mut self) -> R<u32> {
        let mut x = self.range()?;
        while self.eat(b'%') {
            x = self.push(Node::Pct(x));
        }
        Ok(x)
    }

    /// `:` entre expressões que não são referências simples.
    fn range(&mut self) -> R<u32> {
        let mut l = self.primary()?;
        loop {
            self.ws();
            if self.peek() != Some(b':') {
                return Ok(l);
            }
            self.i += 1;
            let r = self.primary()?;
            l = match (&self.f.nodes[l as usize], &self.f.nodes[r as usize]) {
                (Node::Area(a), Node::Area(b)) if a.sheet == b.sheet || b.sheet == SheetSel::This => {
                    let a = *a;
                    let b = *b;
                    self.push(Node::Area(bounding(a, b)))
                }
                _ => self.push(Node::Range(l, r)),
            };
        }
    }

    fn primary(&mut self) -> R<u32> {
        self.ws();
        let Some(c) = self.peek() else { return Err(()) };
        match c {
            b'(' => {
                self.i += 1;
                let x = self.expr()?;
                if !self.eat(b')') {
                    return Err(());
                }
                Ok(x)
            }
            b'"' => {
                let s = self.string()?;
                Ok(self.push(Node::Str(s.into())))
            }
            b'{' => self.array(),
            b'#' => {
                let e = self.error_lit()?;
                Ok(self.push(Node::Err(e)))
            }
            b'\'' => {
                let name = self.quoted_sheet()?;
                if !self.eat(b'!') {
                    return Err(());
                }
                let sel = self.sheet_sel(&name);
                self.after_sheet(sel)
            }
            b'[' => self.bracket_start(),
            b'0'..=b'9' | b'.' => {
                if let Some(x) = self.row_range(SheetSel::This)? {
                    return Ok(x);
                }
                self.number()
            }
            _ if is_ident_char(c) => self.ident(),
            _ => Err(()),
        }
    }

    fn string(&mut self) -> R<String> {
        // Em `"`.
        let mut out = String::new();
        let mut j = self.i + 1;
        loop {
            let k = j + self.s[j..].iter().position(|&b| b == b'"').ok_or(())?;
            out.push_str(&self.text[j..k]);
            if self.s.get(k + 1) == Some(&b'"') {
                out.push('"');
                j = k + 2;
            } else {
                self.i = k + 1;
                return Ok(out);
            }
        }
    }

    fn quoted_sheet(&mut self) -> R<String> {
        let mut out = String::new();
        let mut j = self.i + 1;
        loop {
            let k = j + self.s[j..].iter().position(|&b| b == b'\'').ok_or(())?;
            out.push_str(&self.text[j..k]);
            if self.s.get(k + 1) == Some(&b'\'') {
                out.push('\'');
                j = k + 2;
            } else {
                self.i = k + 1;
                return Ok(out);
            }
        }
    }

    fn error_lit(&mut self) -> R<CellError> {
        const ERRS: [(&str, CellError); 10] = [
            ("#NULL!", CellError::Null),
            ("#DIV/0!", CellError::Div0),
            ("#VALUE!", CellError::Value),
            ("#REF!", CellError::Ref),
            ("#NAME?", CellError::Name),
            ("#NUM!", CellError::Num),
            ("#N/A", CellError::NA),
            ("#GETTING_DATA", CellError::GettingData),
            ("#SPILL!", CellError::Spill),
            ("#CALC!", CellError::Calc),
        ];
        let rest = &self.text[self.i..];
        for (s, e) in ERRS {
            if rest.len() >= s.len() && rest[..s.len()].eq_ignore_ascii_case(s) {
                self.i += s.len();
                return Ok(e);
            }
        }
        Err(())
    }

    fn number(&mut self) -> R<u32> {
        let st = self.i;
        let s = self.s;
        let mut j = self.i;
        while j < s.len() && (s[j].is_ascii_digit() || s[j] == b'.') {
            j += 1;
        }
        if j < s.len() && matches!(s[j], b'e' | b'E') {
            let mut k = j + 1;
            if k < s.len() && matches!(s[k], b'+' | b'-') {
                k += 1;
            }
            if k < s.len() && s[k].is_ascii_digit() {
                while k < s.len() && s[k].is_ascii_digit() {
                    k += 1;
                }
                j = k;
            }
        }
        let v: f64 = self.text[st..j].parse().map_err(|_| ())?;
        self.i = j;
        Ok(self.push(Node::Num(v)))
    }

    fn array(&mut self) -> R<u32> {
        self.i += 1;
        let mut rows: Vec<Vec<u32>> = vec![Vec::new()];
        loop {
            self.ws();
            let x = self.array_elem()?;
            rows.last_mut().unwrap().push(x);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b';') => {
                    self.i += 1;
                    rows.push(Vec::new());
                }
                Some(b'}') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(()),
            }
        }
        let cols = rows[0].len();
        if rows.iter().any(|r| r.len() != cols) {
            return Err(());
        }
        let start = self.f.args.len() as u32;
        for r in &rows {
            self.f.args.extend_from_slice(r);
        }
        Ok(self.push(Node::Array { rows: rows.len() as u32, cols: cols as u32, start }))
    }

    fn array_elem(&mut self) -> R<u32> {
        match self.peek() {
            Some(b'"') => {
                let s = self.string()?;
                Ok(self.push(Node::Str(s.into())))
            }
            Some(b'#') => {
                let e = self.error_lit()?;
                Ok(self.push(Node::Err(e)))
            }
            Some(b'-') => {
                self.i += 1;
                self.ws();
                let x = self.number()?;
                if let Node::Num(v) = self.f.nodes[x as usize] {
                    self.f.nodes[x as usize] = Node::Num(-v);
                }
                Ok(x)
            }
            Some(b'+') => {
                self.i += 1;
                self.number()
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => self.number(),
            _ => {
                let w = self.word();
                if w.eq_ignore_ascii_case("TRUE") {
                    Ok(self.push(Node::Bool(true)))
                } else if w.eq_ignore_ascii_case("FALSE") {
                    Ok(self.push(Node::Bool(false)))
                } else {
                    Err(())
                }
            }
        }
    }

    /// Palavra de identificador a partir de `i` (avança).
    fn word(&mut self) -> &str {
        let st = self.i;
        while self.i < self.s.len() && is_ident_char(self.s[self.i]) {
            self.i += 1;
        }
        &self.text[st..self.i]
    }

    fn sheet_sel(&self, name: &str) -> SheetSel {
        // `[1]Aba` (arquivo externo) dentro ou fora de aspas.
        if let Some(rest) = name.strip_prefix('[')
            && let Some((n, sheet)) = rest.split_once(']')
            && let Ok(book) = n.parse::<u16>()
        {
            let book = book.saturating_sub(1);
            return match self.res.ext_sheet(book, sheet) {
                Some(s) => SheetSel::Ext { book, sheet: s },
                None => SheetSel::Ext { book, sheet: u16::MAX },
            };
        }
        match self.res.sheet(name) {
            Some(s) => SheetSel::Local(s),
            None => SheetSel::Local(u32::MAX),
        }
    }

    /// `[...` no começo: arquivo externo (`[1]Aba!A1`) ou referência
    /// estruturada sem o nome da tabela (`[@Valor]`).
    fn bracket_start(&mut self) -> R<u32> {
        let close = self.i + self.s[self.i..].iter().position(|&b| b == b']').ok_or(())?;
        let inner = &self.text[self.i + 1..close];
        if !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit()) {
            // Arquivo externo.
            let st = self.i;
            self.i = close + 1;
            let w = self.word().to_string();
            self.ws();
            if self.peek() == Some(b'!') {
                self.i += 1;
                let name = format!("{}{w}", &self.text[st..close + 1]);
                if w.is_empty() {
                    let _ = self.word();
                    if self.peek() == Some(b'[') {
                        // `[1]!Tabela[Coluna]`: tabela de outro arquivo; fechado, o Excel dá #REF!.
                        let (_, end) = bracket_body(self.s, self.i).ok_or(())?;
                        self.i = end;
                    } else {
                        // `[1]!Nome`: nome definido do outro arquivo; sem os valores.
                        self.f.flags |= flag::OPAQUE;
                    }
                    return Ok(self.push(Node::Err(CellError::Ref)));
                }
                let sel = self.sheet_sel(&name);
                return self.after_sheet(sel);
            }
            return Err(());
        }
        let t = self.res.table_at(self.sheet, self.row, self.col).ok_or(())?;
        let (sheet, range, h, tt) = (t.sheet, t.range, t.header_rows, t.totals_rows);
        let cols: Vec<String> = t.columns.to_vec();
        self.structured(sheet, range, h, tt, &cols)
    }

    /// Depois de `Aba!`: célula, intervalo, coluna, linha, nome ou `#REF!`.
    fn after_sheet(&mut self, sel: SheetSel) -> R<u32> {
        self.ws();
        match self.peek() {
            Some(b'#') => {
                let e = self.error_lit()?;
                Ok(self.push(Node::Err(e)))
            }
            Some(c) if c.is_ascii_digit() => self.row_range(sel)?.ok_or(()),
            _ => {
                let st = self.i;
                let w = self.word().to_string();
                if let Some(x) = self.cell_or_cols(&w, sel)? {
                    return Ok(x);
                }
                // Nome definido da aba.
                self.i = st + w.len();
                if let SheetSel::Local(s) = sel
                    && let Some(n) = self.res.name(&w, s)
                {
                    return Ok(self.push(Node::Name(n)));
                }
                if matches!(sel, SheetSel::Ext { .. }) {
                    self.f.flags |= flag::OPAQUE;
                }
                Ok(self.push(Node::Err(CellError::Ref)))
            }
        }
    }

    /// `1:5`, `$3:$3` (linhas inteiras). `None` se não for isso.
    fn row_range(&mut self, sel: SheetSel) -> R<Option<u32>> {
        let st = self.i;
        let read = |p: &mut Self| -> Option<Coord> {
            let abs = p.peek() == Some(b'$');
            if abs {
                p.i += 1;
            }
            let d0 = p.i;
            while p.i < p.s.len() && p.s[p.i].is_ascii_digit() {
                p.i += 1;
            }
            let n: u32 = p.text[d0..p.i].parse().ok()?;
            (1..=MAX_ROW + 1).contains(&n).then(|| p.coord(abs, n - 1, p.row))
        };
        if let Some(a) = read(self)
            && self.peek() == Some(b':')
        {
            self.i += 1;
            if let Some(b) = read(self)
                && !self.peek().is_some_and(is_ident_char)
            {
                let area = Area { sheet: sel, r0: a, c0: Coord::abs(0), r1: b, c1: Coord::abs(MAX_COL) };
                return Ok(Some(self.push(Node::Area(area))));
            }
        }
        self.i = st;
        Ok(None)
    }

    fn coord(&self, abs: bool, v: u32, origin: u32) -> Coord {
        if abs { Coord::abs(v) } else { Coord::rel(v as i32 - origin as i32) }
    }

    /// `A1`, `A1:B2`, `A:C` a partir da palavra `w` já lida.
    fn cell_or_cols(&mut self, w: &str, sel: SheetSel) -> R<Option<u32>> {
        if let Some((c, r, ac, ar)) = parse_cell(w) {
            let r0 = self.coord(ar, r, self.row);
            let c0 = self.coord(ac, c, self.col);
            let mut area = Area { sheet: sel, r0, c0, r1: r0, c1: c0 };
            // `A1:B2` direto (o `:` com outra célula).
            let save = self.i;
            if self.peek() == Some(b':') {
                self.i += 1;
                let w2 = self.word().to_string();
                if let Some((c, r, ac, ar)) = parse_cell(&w2)
                    && self.peek() != Some(b'(')
                    && self.peek() != Some(b'!')
                {
                    area.r1 = self.coord(ar, r, self.row);
                    area.c1 = self.coord(ac, c, self.col);
                    return Ok(Some(self.push(Node::Area(normalize(area)))));
                }
                self.i = save;
            }
            return Ok(Some(self.push(Node::Area(area))));
        }
        if let Some((c, ac)) = parse_col(w)
            && self.peek() == Some(b':')
        {
            let save = self.i;
            self.i += 1;
            let w2 = self.word().to_string();
            if let Some((c2, ac2)) = parse_col(&w2) {
                let area = Area {
                    sheet: sel,
                    r0: Coord::abs(0),
                    c0: self.coord(ac, c, self.col),
                    r1: Coord::abs(MAX_ROW),
                    c1: self.coord(ac2, c2, self.col),
                };
                return Ok(Some(self.push(Node::Area(normalize(area)))));
            }
            self.i = save;
        }
        Ok(None)
    }

    fn ident(&mut self) -> R<u32> {
        let st = self.i;
        let w = self.word().to_string();
        self.ws();
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                return self.call(&w);
            }
            Some(b'!') => {
                self.i += 1;
                let sel = self.sheet_sel(&w);
                return self.after_sheet(sel);
            }
            Some(b':') => {
                // `Aba1:Aba3!A1` (3D): não suportado.
                let save = self.i;
                self.i += 1;
                let w2 = self.word().to_string();
                if !w2.is_empty() && self.peek() == Some(b'!') && parse_cell(&w2).is_none() {
                    self.i += 1;
                    let _ = self.word();
                    self.f.flags |= flag::OPAQUE;
                    return Ok(self.push(Node::Err(CellError::Ref)));
                }
                self.i = save;
            }
            Some(b'[') => {
                if let Some(t) = self.res.table(&w) {
                    let (sheet, range, h, tt) = (t.sheet, t.range, t.header_rows, t.totals_rows);
                    let cols: Vec<String> = t.columns.to_vec();
                    return self.structured(sheet, range, h, tt, &cols);
                }
                return Err(());
            }
            _ => {}
        }
        // Sem espaço entre a palavra e o que vem (o `ws` acima pode ter
        // pulado o operador de interseção; referências não têm espaço).
        self.i = st + w.len();
        if w.eq_ignore_ascii_case("TRUE") {
            return Ok(self.push(Node::Bool(true)));
        }
        if w.eq_ignore_ascii_case("FALSE") {
            return Ok(self.push(Node::Bool(false)));
        }
        if let Some(x) = self.cell_or_cols(&w, SheetSel::This)? {
            return Ok(x);
        }
        if let Some(n) = self.res.name(&w, self.sheet) {
            return Ok(self.push(Node::Name(n)));
        }
        Ok(self.push(Node::Err(CellError::Name)))
    }

    fn call(&mut self, name: &str) -> R<u32> {
        let f = funcs::lookup(name);
        let mut args = Vec::new();
        self.ws();
        if self.peek() == Some(b')') {
            self.i += 1;
        } else {
            loop {
                self.ws();
                let x = if matches!(self.peek(), Some(b',') | Some(b')')) {
                    self.push(Node::Missing)
                } else {
                    self.expr()?
                };
                args.push(x);
                self.ws();
                match self.peek() {
                    Some(b',') => self.i += 1,
                    Some(b')') => {
                        self.i += 1;
                        break;
                    }
                    _ => return Err(()),
                }
            }
        }
        let Some(f) = f else {
            self.f.flags |= flag::OPAQUE;
            return Ok(self.push(Node::Err(CellError::Name)));
        };
        let def = funcs::def(f);
        if def.volatile {
            self.f.flags |= flag::VOLATILE;
        }
        if def.dynamic {
            self.f.flags |= flag::DYNAMIC;
        }
        if def.opaque {
            self.f.flags |= flag::OPAQUE;
        }
        let start = self.f.args.len() as u32;
        self.f.args.extend_from_slice(&args);
        Ok(self.push(Node::Call { f, start, len: args.len() as u32 }))
    }

    /// Referência estruturada: em `[` depois do nome da tabela.
    fn structured(&mut self, sheet: u32, range: CellRange, h: u32, t: u32, cols: &[String]) -> R<u32> {
        let (spec, end) = bracket_body(self.s, self.i).ok_or(())?;
        self.i = end;
        let spec = &self.text[spec.0..spec.1];
        let mut rows_sel: Vec<&str> = Vec::new();
        let mut col_sel: Option<(String, String)> = None;
        let mut this_row = false;
        let col_index = |name: &str| -> Option<u32> {
            let n = unescape_struct(name);
            let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase();
            cols.iter()
                .position(|c| eq(c, &n))
                .or_else(|| cols.iter().position(|c| eq(c.trim(), n.trim())))
                .or_else(|| cols.iter().position(|c| eq(&decode_x(c), &n)))
                .map(|k| k as u32)
        };
        let trimmed = spec.trim();
        if let Some(rest) = trimmed.strip_prefix('@') {
            this_row = true;
            let rest = rest.trim();
            if rest.is_empty() {
                col_sel = None;
            } else if rest.starts_with('[') {
                let items = split_items(rest).ok_or(())?;
                col_sel = Some(col_pair(&items).ok_or(())?);
            } else {
                col_sel = Some((rest.to_string(), rest.to_string()));
            }
        } else if trimmed.starts_with('[') {
            let items = split_items(trimmed).ok_or(())?;
            for it in &items {
                if let Some(k) = it.0.strip_prefix('#') {
                    rows_sel.push(match k.to_ascii_lowercase().as_str() {
                        "this row" => {
                            this_row = true;
                            continue;
                        }
                        "all" => "all",
                        "data" => "data",
                        "headers" => "headers",
                        "totals" => "totals",
                        _ => return Err(()),
                    });
                }
            }
            let colitems: Vec<(String, Option<String>)> =
                items.into_iter().filter(|it| !it.0.starts_with('#')).collect();
            if !colitems.is_empty() {
                col_sel = Some(col_pair(&colitems).ok_or(())?);
            }
        } else if let Some(k) = trimmed.strip_prefix('#') {
            rows_sel.push(match k.to_ascii_lowercase().as_str() {
                "all" => "all",
                "data" => "data",
                "headers" => "headers",
                "totals" => "totals",
                "this row" => {
                    this_row = true;
                    "data"
                }
                _ => return Err(()),
            });
        } else if !trimmed.is_empty() {
            col_sel = Some((trimmed.to_string(), trimmed.to_string()));
        }

        let (c0, c1) = match &col_sel {
            None => (range.first_col, range.last_col),
            Some((a, b)) => {
                let ka = col_index(a).ok_or(())?;
                let kb = col_index(b).ok_or(())?;
                (range.first_col + ka.min(kb), range.first_col + ka.max(kb))
            }
        };
        let data0 = range.first_row + h;
        let data1 = range.last_row.saturating_sub(t);
        let area = |r0: Coord, r1: Coord| Area {
            sheet: SheetSel::Local(sheet),
            r0,
            c0: Coord::abs(c0),
            r1,
            c1: Coord::abs(c1),
        };
        let a = if this_row {
            area(Coord::rel(0), Coord::rel(0))
        } else {
            let mut lo = u32::MAX;
            let mut hi = 0;
            let mut add = |a: u32, b: u32| {
                if a <= b {
                    lo = lo.min(a);
                    hi = hi.max(b);
                }
            };
            if rows_sel.is_empty() {
                rows_sel.push("data");
            }
            for r in &rows_sel {
                match *r {
                    "all" => add(range.first_row, range.last_row),
                    "data" => add(data0, data1),
                    "headers" if h > 0 => add(range.first_row, range.first_row),
                    "totals" if t > 0 => add(range.last_row, range.last_row),
                    _ => {}
                }
            }
            if lo > hi {
                return Ok(self.push(Node::Err(CellError::Ref)));
            }
            area(Coord::abs(lo), Coord::abs(hi))
        };
        Ok(self.push(Node::Area(a)))
    }
}

/// `[...]` a partir de `i` (em `[`): (início, fim) do conteúdo e o índice
/// depois do `]` que fecha. `'` escapa o caractere seguinte.
fn bracket_body(s: &[u8], i: usize) -> Option<((usize, usize), usize)> {
    let mut depth = 0;
    let mut j = i;
    while j < s.len() {
        match s[j] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(((i + 1, j), j + 1));
                }
            }
            b'\'' => j += 1,
            _ => {}
        }
        j += 1;
    }
    None
}

/// Itens de `[a],[b]:[c],[#Data]` → [(a, None), (b, Some(c)), (#Data, None)].
fn split_items(s: &str) -> Option<Vec<(String, Option<String>)>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut pending_range = false;
    while i < b.len() {
        match b[i] {
            b' ' | b',' => i += 1,
            b':' => {
                pending_range = true;
                i += 1;
            }
            b'[' => {
                let ((a, z), end) = bracket_body(b, i)?;
                let item = s[a..z].to_string();
                if pending_range {
                    let last: &mut (String, Option<String>) = out.last_mut()?;
                    last.1 = Some(item);
                    pending_range = false;
                } else {
                    out.push((item, None));
                }
                i = end;
            }
            _ => return None,
        }
    }
    Some(out)
}

fn col_pair(items: &[(String, Option<String>)]) -> Option<(String, String)> {
    let first = items.first()?;
    let last = items.last()?;
    Some((first.0.clone(), last.1.clone().unwrap_or_else(|| last.0.clone())))
}

/// `_x000a_` (como o XML guarda quebras de linha nos nomes) → caractere.
fn decode_x(s: &str) -> String {
    if !s.contains("_x") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("_x") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.len() >= 7
            && tail.as_bytes()[6] == b'_'
            && let Ok(n) = u32::from_str_radix(&tail[2..6], 16)
            && let Some(c) = char::from_u32(n)
        {
            out.push(c);
            rest = &tail[7..];
        } else {
            out.push_str("_x");
            rest = &tail[2..];
        }
    }
    out.push_str(rest);
    out
}

/// Tira os escapes `'` de um nome de coluna.
fn unescape_struct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\'' {
            if let Some(n) = it.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Intervalo que cobre as duas áreas (`A1:B2` montado de dois lados).
fn bounding(a: Area, b: Area) -> Area {
    Area { sheet: a.sheet, r0: a.r0, c0: a.c0, r1: b.r1, c1: b.c1 }
}

/// Coordenadas absolutas em ordem (relativas ficam como vieram).
fn normalize(mut a: Area) -> Area {
    if a.r0.abs && a.r1.abs && a.r0.v > a.r1.v {
        std::mem::swap(&mut a.r0, &mut a.r1);
    }
    if a.c0.abs && a.c1.abs && a.c0.v > a.c1.v {
        std::mem::swap(&mut a.c0, &mut a.c1);
    }
    if !a.r0.abs && !a.r1.abs && a.r0.v > a.r1.v {
        std::mem::swap(&mut a.r0, &mut a.r1);
    }
    if !a.c0.abs && !a.c1.abs && a.c0.v > a.c1.v {
        std::mem::swap(&mut a.c0, &mut a.c1);
    }
    a
}

/// `$AB$12` → (coluna, linha, coluna absoluta, linha absoluta), base 0.
pub fn parse_cell(w: &str) -> Option<(u32, u32, bool, bool)> {
    let b = w.as_bytes();
    let mut i = 0;
    let ac = b.first() == Some(&b'$');
    if ac {
        i += 1;
    }
    let c0 = i;
    let mut col: u32 = 0;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        col = col * 26 + (b[i].to_ascii_uppercase() - b'A') as u32 + 1;
        i += 1;
        if i - c0 > 3 {
            return None;
        }
    }
    if i == c0 || col > MAX_COL + 1 {
        return None;
    }
    let ar = b.get(i) == Some(&b'$');
    if ar {
        i += 1;
    }
    let r0 = i;
    let mut row: u32 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        row = row.checked_mul(10)?.checked_add((b[i] - b'0') as u32)?;
        i += 1;
    }
    if i == r0 || i != b.len() || row == 0 || row > MAX_ROW + 1 {
        return None;
    }
    Some((col - 1, row - 1, ac, ar))
}

/// `$AB` → (coluna base 0, absoluta).
fn parse_col(w: &str) -> Option<(u32, bool)> {
    let abs = w.starts_with('$');
    let t = w.trim_start_matches('$');
    if t.is_empty() || t.len() > 3 || !t.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let col = t.bytes().fold(0u32, |a, c| a * 26 + (c.to_ascii_uppercase() - b'A') as u32 + 1);
    (col <= MAX_COL + 1).then_some((col - 1, abs))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoBook;
    impl Resolve for NoBook {
        fn sheet(&self, name: &str) -> Option<u32> {
            (name == "Dados").then_some(1)
        }
        fn ext_sheet(&self, _: u16, _: &str) -> Option<u16> {
            Some(0)
        }
        fn name(&self, _: &str, _: u32) -> Option<u32> {
            None
        }
        fn table(&self, name: &str) -> Option<TableInfo<'_>> {
            static COLS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
            let cols = COLS.get_or_init(|| vec!["Nome".into(), "Valor".into()]);
            (name == "T").then(|| TableInfo {
                sheet: 0,
                range: CellRange { first_row: 0, first_col: 2, last_row: 10, last_col: 3 },
                header_rows: 1,
                totals_rows: 0,
                columns: cols,
            })
        }
        fn table_at(&self, _: u32, _: u32, _: u32) -> Option<TableInfo<'_>> {
            self.table("T")
        }
    }

    fn c(s: &str) -> Formula {
        compile(s, 0, 4, 1, &NoBook)
    }

    #[test]
    fn refs_and_ops() {
        let f = c("A1+$B$2*2");
        assert!(matches!(f.node(f.root), Node::Bin(BinOp::Add, _, _)));
        let Node::Area(a) = f.nodes[0] else { panic!() };
        assert_eq!((a.r0, a.c0), (Coord::rel(-4), Coord::rel(-1)));
        let f = c("SUM(Dados!A:B)");
        let Node::Area(a) = f.nodes[0] else { panic!() };
        assert_eq!(a.sheet, SheetSel::Local(1));
        assert_eq!(
            (a.r0, a.r1, a.c0, a.c1),
            (Coord::abs(0), Coord::abs(MAX_ROW), Coord::rel(-1), Coord::rel(0))
        );
        assert_eq!(c("-2^2").nodes.len(), 3);
        assert!(matches!(c("IF(A1>0,\"a\"\"b\",)").node(0), Node::Area(_)));
        assert_eq!(c("1+").nodes, Formula::error(CellError::Name).nodes);
        let f = c("'[1]Base X'!$A:$F");
        assert!(matches!(
            f.node(f.root),
            Node::Area(Area { sheet: SheetSel::Ext { book: 0, sheet: 0 }, .. })
        ));
    }

    #[test]
    fn structured_refs() {
        let f = c("T[[#This Row],[Valor]]");
        let Node::Area(a) = f.node(f.root) else { panic!() };
        assert_eq!((a.r0, a.c0, a.c1), (Coord::rel(0), Coord::abs(3), Coord::abs(3)));
        let f = c("SUM(T[Valor])");
        let Node::Area(a) = f.nodes[0] else { panic!() };
        assert_eq!((a.r0, a.r1), (Coord::abs(1), Coord::abs(10)));
        let f = c("[@Nome]&T[#All]");
        let Node::Area(a) = f.nodes[0] else { panic!() };
        assert_eq!((a.r0, a.c0), (Coord::rel(0), Coord::abs(2)));
        let Node::Area(b) = f.nodes[1] else { panic!() };
        assert_eq!((b.r0, b.r1, b.c0, b.c1), (Coord::abs(0), Coord::abs(10), Coord::abs(2), Coord::abs(3)));
    }

    #[test]
    fn arrays_and_rows() {
        let f = c("{1,2;3,-4}");
        assert!(matches!(f.node(f.root), Node::Array { rows: 2, cols: 2, .. }));
        let f = c("SUM(2:3)");
        let Node::Area(a) = f.nodes[0] else { panic!() };
        assert_eq!((a.r0, a.r1), (Coord::rel(-3), Coord::rel(-2)));
        let f = c("_xlfn.XLOOKUP(1,A:A,B:B)");
        assert!(matches!(f.node(f.root), Node::Call { .. }));
        assert_eq!(f.flags & flag::OPAQUE, 0);
    }
}
