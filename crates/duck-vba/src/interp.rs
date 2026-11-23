//! Interpretador: variáveis, controle de fluxo, tratamento de erro e as
//! funções da linguagem. Os objetos (planilhas, dinâmicas...) ficam com o
//! [`Host`]; a macro não tem outro acesso ao computador.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::{Arg, Bin, Case, Dim, Expr, OnError, Proc, Stmt, Unit};

/// Erro de execução do VBA.
#[derive(Clone, Debug, PartialEq)]
pub struct VbErr {
    pub number: i32,
    pub text: String,
}

impl VbErr {
    pub fn new(number: i32, text: impl Into<String>) -> Self {
        Self { number, text: text.into() }
    }

    pub fn unsupported(what: &str) -> Self {
        Self::new(438, format!("O duck-sheets ainda não tem o recurso de macro: {what}"))
    }
}

pub type Res<T> = Result<T, VbErr>;

fn mismatch() -> VbErr {
    VbErr::new(13, "Tipos incompatíveis")
}

/// Matriz com limites por dimensão (inclusivos), em ordem de linha.
#[derive(Clone, Debug, PartialEq)]
pub struct Arr<O> {
    pub bounds: Vec<(i64, i64)>,
    pub data: Vec<Val<O>>,
}

impl<O: Clone> Arr<O> {
    pub fn new(bounds: Vec<(i64, i64)>) -> Self {
        let n: usize = bounds.iter().map(|(a, b)| (b - a + 1).max(0) as usize).product();
        Self { data: vec![Val::Empty; if bounds.is_empty() { 0 } else { n }], bounds }
    }

    fn index(&self, at: &[i64]) -> Res<usize> {
        if at.len() != self.bounds.len() || self.bounds.is_empty() {
            return Err(VbErr::new(9, "Subscrito fora do intervalo"));
        }
        let mut k = 0usize;
        for (i, (a, b)) in at.iter().zip(&self.bounds) {
            if i < a || i > b {
                return Err(VbErr::new(9, "Subscrito fora do intervalo"));
            }
            k = k * (b - a + 1) as usize + (i - a) as usize;
        }
        Ok(k)
    }
}

/// `Scripting.Dictionary`: chaves na ordem de entrada.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict<O> {
    keys: Vec<Val<O>>,
    vals: Vec<Val<O>>,
    map: HashMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Val<O> {
    Empty,
    Null,
    Nothing,
    Bool(bool),
    Num(f64),
    Date(f64),
    Str(String),
    Arr(Rc<RefCell<Arr<O>>>),
    Dict(Rc<RefCell<Dict<O>>>),
    Obj(O),
    /// O objeto `Err`.
    ErrObj,
    /// Argumento omitido.
    Missing,
}

/// Argumentos de uma chamada a um objeto.
#[derive(Clone, Debug)]
pub struct Args<O> {
    pub pos: Vec<Val<O>>,
    pub named: Vec<(String, Val<O>)>,
}

impl<O: Clone> Args<O> {
    pub fn none() -> Self {
        Self { pos: Vec::new(), named: Vec::new() }
    }

    /// Argumento pela posição ou pelo nome.
    pub fn get(&self, i: usize, name: &str) -> Option<&Val<O>> {
        self.pos
            .get(i)
            .filter(|v| !matches!(v, Val::Missing))
            .or_else(|| self.named.iter().find(|n| n.0 == name).map(|n| &n.1))
    }
}

/// Os objetos da planilha.
pub trait Host {
    type Obj: Clone + std::fmt::Debug + PartialEq;

    /// Nome global (`ThisWorkbook`, `Range`, `MsgBox`, constantes).
    fn global(&mut self, name: &str, args: &Args<Self::Obj>) -> Option<Res<Val<Self::Obj>>>;
    /// Propriedade ou método.
    fn get(&mut self, o: &Self::Obj, name: &str, args: &Args<Self::Obj>) -> Res<Val<Self::Obj>>;
    fn set(&mut self, o: &Self::Obj, name: &str, args: &Args<Self::Obj>, v: Val<Self::Obj>) -> Res<()>;
    /// Membro padrão (`Range` → valor; `Sheets("x")` → item).
    fn default(&mut self, o: &Self::Obj, args: &Args<Self::Obj>) -> Res<Val<Self::Obj>>;
    /// Itens para `For Each`.
    fn items(&mut self, o: &Self::Obj) -> Res<Vec<Val<Self::Obj>>>;
    /// Chamado de tempos em tempos; `true` interrompe a macro.
    fn cancelled(&mut self) -> bool {
        false
    }
}

enum Flow {
    Next,
    ExitSub,
    ExitFor,
    ExitDo,
    Goto(String),
    End,
}

struct Var<O> {
    val: Val<O>,
    ty: String,
}

struct Frame<O> {
    vars: HashMap<String, Var<O>>,
    with: Vec<Val<O>>,
    on_error: Option<OnError>,
    /// Dentro de um tratador: erros novos sobem.
    handling: bool,
    module: String,
}

pub struct Interp<'h, H: Host> {
    pub host: &'h mut H,
    procs: HashMap<String, Rc<Proc>>,
    globals: HashMap<String, Var<H::Obj>>,
    frames: Vec<Frame<H::Obj>>,
    err: VbErr,
    steps: u64,
}

/// Instruções até parar uma macro que não termina.
const MAX_STEPS: u64 = 500_000_000;

type V<H> = Val<<H as Host>::Obj>;

impl<'h, H: Host> Interp<'h, H> {
    pub fn new(host: &'h mut H, units: Vec<Unit>) -> Self {
        let mut it = Self {
            host,
            procs: HashMap::new(),
            globals: HashMap::new(),
            frames: Vec::new(),
            err: VbErr::new(0, ""),
            steps: 0,
        };
        for u in units {
            it.frames.push(it.frame(""));
            for s in &u.globals {
                let _ = it.exec(s);
            }
            let f = it.frames.pop().expect("quadro");
            it.globals.extend(f.vars);
            for p in u.procs {
                it.procs.insert(p.name.to_lowercase(), Rc::new(p));
            }
        }
        it
    }

    fn frame(&self, module: &str) -> Frame<H::Obj> {
        Frame {
            vars: HashMap::new(),
            with: Vec::new(),
            on_error: None,
            handling: false,
            module: module.to_string(),
        }
    }

    /// Executa uma macro pelo nome (sem argumentos).
    pub fn run(&mut self, name: &str) -> Res<()> {
        let key = name.to_lowercase();
        let p = self.procs.get(&key).cloned();
        let p = p.ok_or_else(|| VbErr::new(1004, format!("Macro não encontrada: {name}")))?;
        self.call_proc(&p, Vec::new()).map(|_| ())
    }

    fn call_proc(&mut self, p: &Rc<Proc>, args: Vec<V<H>>) -> Res<(V<H>, Vec<V<H>>)> {
        if self.frames.len() > 200 {
            return Err(VbErr::new(28, "Pilha cheia"));
        }
        let body = match &p.body {
            Ok(b) => b.clone(),
            Err(e) => {
                return Err(VbErr::new(
                    1002,
                    format!("A macro {} usa algo que o duck-sheets não entende: {e}", p.name),
                ));
            }
        };
        let mut f = self.frame(&p.module);
        let mut it = args.into_iter();
        for q in &p.params {
            let v = match it.next().filter(|v| !matches!(v, Val::Missing)) {
                Some(v) => v,
                None => match &q.optional {
                    Some(Expr::Empty) => default_of(&q.ty),
                    Some(e) => self.eval(e)?,
                    None => return Err(VbErr::new(449, "Argumento obrigatório")),
                },
            };
            let v = coerce(v, &q.ty)?;
            f.vars.insert(q.name.to_lowercase(), Var { val: v, ty: q.ty.clone() });
        }
        if p.is_function {
            f.vars.insert(p.name.to_lowercase(), Var { val: Val::Empty, ty: "variant".into() });
        }
        self.frames.push(f);
        let r = self.block(&body);
        let f = self.frames.pop().expect("quadro");
        match r? {
            Flow::End => return Err(VbErr::new(0, "End")),
            Flow::Goto(l) => return Err(VbErr::new(1003, format!("Rótulo não encontrado: {l}"))),
            _ => {}
        }
        let get = |n: &str| f.vars.get(&n.to_lowercase()).map_or(Val::Empty, |v| v.val.clone());
        let ret = if p.is_function { get(&p.name) } else { Val::Empty };
        Ok((ret, p.params.iter().map(|q| get(&q.name)).collect()))
    }

    fn top(&mut self) -> &mut Frame<H::Obj> {
        self.frames.last_mut().expect("quadro")
    }

    // --- instruções ---------------------------------------------------------

    fn block(&mut self, body: &[Stmt]) -> Res<Flow> {
        let mut i = 0;
        while i < body.len() {
            self.steps += 1;
            if self.steps > MAX_STEPS || (self.steps.is_multiple_of(4096) && self.host.cancelled()) {
                return Err(VbErr::new(18, "Macro interrompida"));
            }
            let r = self.exec(&body[i]);
            i += 1;
            let label = match r {
                Ok(Flow::Next) => continue,
                Ok(Flow::Goto(l)) => l,
                Ok(f) => return Ok(f),
                Err(e) => match self.on_error(e)? {
                    None => continue,
                    Some(l) => l,
                },
            };
            match body.iter().position(|s| matches!(s, Stmt::Label(x) if *x == label)) {
                Some(k) => i = k + 1,
                None => return Ok(Flow::Goto(label)),
            }
        }
        Ok(Flow::Next)
    }

    /// O que o tratamento de erro da rotina manda: seguir (`None`), ir para um
    /// rótulo ou deixar o erro subir.
    fn on_error(&mut self, e: VbErr) -> Res<Option<String>> {
        // `End`, pilha cheia e interrupção não são tratáveis.
        if matches!(e.number, 0 | 18 | 28) {
            return Err(e);
        }
        let f = self.top();
        if f.handling {
            return Err(e);
        }
        match f.on_error.clone() {
            Some(OnError::ResumeNext) => {
                self.err = e;
                Ok(None)
            }
            Some(OnError::Goto(l)) => {
                f.handling = true;
                self.err = e;
                Ok(Some(l))
            }
            _ => Err(e),
        }
    }

    fn exec(&mut self, s: &Stmt) -> Res<Flow> {
        match s {
            Stmt::Dim(list) => {
                for d in list {
                    let v = self.dim_value(d)?;
                    self.declare(&d.name, v, &d.ty);
                }
            }
            Stmt::ReDim { preserve, list } => {
                for d in list {
                    let b = self.bounds(d.bounds.as_deref().unwrap_or(&[]))?;
                    let mut new = Arr::new(b);
                    if *preserve && let Some(Val::Arr(old)) = self.lookup(&d.name) {
                        copy_preserved(&old.borrow(), &mut new);
                    }
                    let v = Val::Arr(Rc::new(RefCell::new(new)));
                    self.assign_name(&d.name, v, true)?;
                }
            }
            Stmt::Const(list) => {
                for (n, e) in list {
                    let v = self.eval(e)?;
                    self.declare(n, v, "variant");
                }
            }
            Stmt::Let(t, e) => {
                let v = self.eval_value(e)?;
                // Matrizes são copiadas na atribuição.
                let v = match v {
                    Val::Arr(a) => Val::Arr(Rc::new(RefCell::new(a.borrow().clone()))),
                    v => v,
                };
                self.assign(t, v, false)?;
            }
            Stmt::Set(t, e) => {
                let v = self.eval(e)?;
                if !matches!(v, Val::Obj(_) | Val::Dict(_) | Val::Nothing | Val::ErrObj) {
                    return Err(VbErr::new(424, "Objeto obrigatório"));
                }
                self.assign(t, v, true)?;
            }
            Stmt::Call(e) => {
                self.eval(e)?;
            }
            Stmt::If(arms, no) => {
                for (c, body) in arms {
                    let yes = match self.eval_value(c).and_then(|v| truth(&v)) {
                        Ok(b) => b,
                        // Com `Resume Next`, o VBA entra no bloco.
                        Err(e) => match self.on_error(e)? {
                            None => true,
                            Some(l) => return Ok(Flow::Goto(l)),
                        },
                    };
                    if yes {
                        return self.block(body);
                    }
                }
                return self.block(no);
            }
            Stmt::For { var, from, to, step, body } => {
                let mut x = self.num(from)?;
                let end = self.num(to)?;
                let step = match step {
                    Some(s) => self.num(s)?,
                    None => 1.0,
                };
                loop {
                    self.assign(var, Val::Num(x), false)?;
                    if (step >= 0.0 && x > end) || (step < 0.0 && x < end) {
                        break;
                    }
                    match self.block(body)? {
                        Flow::Next => {}
                        Flow::ExitFor => break,
                        f => return Ok(f),
                    }
                    // O corpo pode mudar a variável.
                    x = self.num(var)? + step;
                }
            }
            Stmt::ForEach { var, list, body } => {
                let items = match self.eval(list)? {
                    Val::Arr(a) => a.borrow().data.clone(),
                    Val::Obj(o) => self.host.items(&o)?,
                    Val::Dict(d) => d.borrow().keys.clone(),
                    _ => return Err(VbErr::new(424, "Objeto obrigatório")),
                };
                for v in items {
                    self.assign(var, v, true)?;
                    match self.block(body)? {
                        Flow::Next => {}
                        Flow::ExitFor => break,
                        f => return Ok(f),
                    }
                }
            }
            Stmt::Do { cond, pre, body } => loop {
                let test = |it: &mut Self| -> Res<bool> {
                    match cond {
                        Some((c, until)) => Ok(truth(&it.eval_value(c)?)? != *until),
                        None => Ok(true),
                    }
                };
                if *pre && !test(self)? {
                    break;
                }
                match self.block(body)? {
                    Flow::Next => {}
                    Flow::ExitDo => break,
                    f => return Ok(f),
                }
                if !*pre && !test(self)? {
                    break;
                }
            },
            Stmt::With(e, body) => {
                let v = self.eval(e)?;
                self.top().with.push(v);
                let r = self.block(body);
                self.top().with.pop();
                return r;
            }
            Stmt::Select(e, arms) => {
                let v = self.eval_value(e)?;
                for (cases, body) in arms {
                    let mut hit = false;
                    for c in cases {
                        hit |= match c {
                            Case::Else => true,
                            Case::Value(x) => {
                                let x = self.eval_value(x)?;
                                truth(&compare(Bin::Eq, &v, &x)?)?
                            }
                            Case::Range(a, b) => {
                                let (a, b) = (self.eval_value(a)?, self.eval_value(b)?);
                                truth(&compare(Bin::Ge, &v, &a)?)? && truth(&compare(Bin::Le, &v, &b)?)?
                            }
                            Case::Is(op, x) => {
                                let x = self.eval_value(x)?;
                                truth(&compare(*op, &v, &x)?)?
                            }
                        };
                    }
                    if hit {
                        return self.block(body);
                    }
                }
            }
            Stmt::OnError(o) => {
                self.err = VbErr::new(0, "");
                self.top().on_error = (*o != OnError::Off).then(|| o.clone());
            }
            Stmt::Resume(l) => {
                self.top().handling = false;
                self.err = VbErr::new(0, "");
                if let Some(l) = l {
                    return Ok(Flow::Goto(l.clone()));
                }
            }
            Stmt::Goto(l) => return Ok(Flow::Goto(l.clone())),
            Stmt::Label(_) => {}
            Stmt::ExitSub => return Ok(Flow::ExitSub),
            Stmt::ExitFor => return Ok(Flow::ExitFor),
            Stmt::ExitDo => return Ok(Flow::ExitDo),
            Stmt::End => return Ok(Flow::End),
        }
        Ok(Flow::Next)
    }

    fn bounds(&mut self, list: &[(Option<Expr>, Expr)]) -> Res<Vec<(i64, i64)>> {
        let mut out = Vec::new();
        let mut total = 1i64;
        for (a, b) in list {
            let lo = match a {
                Some(a) => self.num(a)?.round() as i64,
                None => 0,
            };
            let hi = self.num(b)?.round() as i64;
            total = total.saturating_mul((hi - lo + 1).max(0));
            out.push((lo, hi));
        }
        if total > 50_000_000 {
            return Err(VbErr::new(7, "Memória insuficiente"));
        }
        Ok(out)
    }

    fn dim_value(&mut self, d: &Dim) -> Res<V<H>> {
        Ok(match &d.bounds {
            Some(b) => {
                let b = self.bounds(b)?;
                Val::Arr(Rc::new(RefCell::new(Arr::new(b))))
            }
            None => default_of(&d.ty),
        })
    }

    fn declare(&mut self, name: &str, val: V<H>, ty: &str) {
        self.top().vars.insert(name.to_lowercase(), Var { val, ty: ty.to_string() });
    }

    fn lookup(&self, name: &str) -> Option<V<H>> {
        let k = name.to_lowercase();
        let f = self.frames.last()?;
        f.vars.get(&k).or_else(|| self.globals.get(&k)).map(|v| v.val.clone())
    }

    fn assign_name(&mut self, name: &str, v: V<H>, raw: bool) -> Res<()> {
        let k = name.to_lowercase();
        let f = self.frames.last_mut().expect("quadro");
        let var = match f.vars.get_mut(&k) {
            Some(x) => x,
            None => match self.globals.get_mut(&k) {
                Some(x) => x,
                None => f.vars.entry(k).or_insert(Var { val: Val::Empty, ty: "variant".into() }),
            },
        };
        var.val = if raw { v } else { coerce(v, &var.ty)? };
        Ok(())
    }

    fn assign(&mut self, t: &Expr, v: V<H>, raw: bool) -> Res<()> {
        match t {
            Expr::Name(n) => self.assign_name(n, v, raw),
            Expr::Member(o, name) => {
                let o = self.eval(o)?;
                self.set_member(&o, name, &Args::none(), v)
            }
            Expr::With(name) => {
                let o = self.with_obj()?;
                self.set_member(&o, name, &Args::none(), v)
            }
            Expr::Call(f, args) => {
                // Elemento de matriz, item de dicionário ou propriedade com índice.
                if let Expr::Name(n) = &**f
                    && let Some(base) = self.lookup(n)
                {
                    return self.set_item(&base, args, v);
                }
                match &**f {
                    Expr::Member(o, name) => {
                        let o = self.eval(o)?;
                        let a = self.args(args)?;
                        self.set_member(&o, name, &a, v)
                    }
                    Expr::With(name) => {
                        let o = self.with_obj()?;
                        let a = self.args(args)?;
                        self.set_member(&o, name, &a, v)
                    }
                    _ => {
                        // `Range("A1") = 1`: o membro padrão do objeto.
                        let o = self.eval(t)?;
                        self.set_member(&o, "value", &Args::none(), v)
                    }
                }
            }
            _ => Err(VbErr::new(424, "Não dá para atribuir")),
        }
    }

    fn set_item(&mut self, base: &V<H>, args: &[Arg], v: V<H>) -> Res<()> {
        match base {
            Val::Arr(a) => {
                let mut at = Vec::new();
                for x in args {
                    at.push(self.arg_num(x)?.round() as i64);
                }
                let mut a = a.borrow_mut();
                let k = a.index(&at)?;
                a.data[k] = v;
                Ok(())
            }
            Val::Dict(d) => {
                let a = self.args(args)?;
                let k = a.pos.first().cloned().unwrap_or(Val::Empty);
                dict_set(&mut d.borrow_mut(), k, v);
                Ok(())
            }
            Val::Obj(o) => {
                let a = self.args(args)?;
                let o2 = self.host.default(o, &a)?;
                self.set_member(&o2, "value", &Args::none(), v)
            }
            _ => Err(mismatch()),
        }
    }

    fn set_member(&mut self, o: &V<H>, name: &str, a: &Args<H::Obj>, v: V<H>) -> Res<()> {
        match o {
            Val::Obj(o) => self.host.set(o, &name.to_lowercase(), a, v),
            Val::Dict(d) => match name.to_lowercase().as_str() {
                "item" => {
                    dict_set(&mut d.borrow_mut(), a.pos.first().cloned().unwrap_or(Val::Empty), v);
                    Ok(())
                }
                "comparemode" => Ok(()),
                n => Err(VbErr::unsupported(n)),
            },
            Val::Nothing | Val::Empty => Err(VbErr::new(91, "A variável de objeto não foi definida")),
            _ => Err(VbErr::new(424, "Objeto obrigatório")),
        }
    }

    fn with_obj(&mut self) -> Res<V<H>> {
        self.top().with.last().cloned().ok_or_else(|| VbErr::new(424, "Fora de um With"))
    }

    // --- expressões ---------------------------------------------------------

    fn num(&mut self, e: &Expr) -> Res<f64> {
        to_num(&self.eval_value(e)?)
    }

    fn arg_num(&mut self, a: &Arg) -> Res<f64> {
        match &a.value {
            Some(e) => self.num(e),
            None => Err(mismatch()),
        }
    }

    fn args(&mut self, list: &[Arg]) -> Res<Args<H::Obj>> {
        let mut out = Args::none();
        for a in list {
            let v = match &a.value {
                Some(e) => self.eval(e)?,
                None => Val::Missing,
            };
            match &a.name {
                Some(n) => out.named.push((n.clone(), v)),
                None => out.pos.push(v),
            }
        }
        Ok(out)
    }

    /// Avalia até um valor (objetos dão o membro padrão).
    fn eval_value(&mut self, e: &Expr) -> Res<V<H>> {
        match self.eval(e)? {
            Val::Obj(o) => self.host.default(&o, &Args::none()),
            v => Ok(v),
        }
    }

    fn value_of(&mut self, v: V<H>) -> Res<V<H>> {
        match v {
            Val::Obj(o) => self.host.default(&o, &Args::none()),
            v => Ok(v),
        }
    }

    pub fn eval(&mut self, e: &Expr) -> Res<V<H>> {
        Ok(match e {
            Expr::Num(n) => Val::Num(*n),
            Expr::Str(s) => Val::Str(s.clone()),
            Expr::Bool(b) => Val::Bool(*b),
            Expr::Nothing => Val::Nothing,
            Expr::Empty => Val::Empty,
            Expr::Null => Val::Null,
            Expr::Name(n) => return self.name(n, &[]),
            Expr::With(n) => {
                let o = self.with_obj()?;
                return self.member(&o, n, &Args::none());
            }
            Expr::Member(o, n) => {
                let o = self.eval(o)?;
                return self.member(&o, n, &Args::none());
            }
            Expr::Call(f, args) => return self.call(f, args),
            Expr::Neg(x) => Val::Num(-self.num(x)?),
            Expr::Not(x) => match self.eval_value(x)? {
                Val::Num(n) => Val::Num(!(n as i64) as f64),
                v => Val::Bool(!truth(&v)?),
            },
            Expr::Bin(Bin::Is, a, b) => {
                let (a, b) = (self.eval(a)?, self.eval(b)?);
                let none = |v: &V<H>| matches!(v, Val::Nothing | Val::Empty);
                Val::Bool(match (&a, &b) {
                    (Val::Obj(x), Val::Obj(y)) => x == y,
                    (Val::Dict(x), Val::Dict(y)) => Rc::ptr_eq(x, y),
                    (a, b) => none(a) && none(b),
                })
            }
            Expr::Bin(op, a, b) => {
                let a = self.eval_value(a)?;
                let b = self.eval_value(b)?;
                return binary(*op, &a, &b);
            }
        })
    }

    fn name(&mut self, n: &str, args: &[Arg]) -> Res<V<H>> {
        if args.is_empty()
            && let Some(v) = self.lookup(n)
        {
            return Ok(v);
        }
        let key = n.to_lowercase();
        if let Some(p) = self.procs.get(&key).cloned() {
            return self.user_call(&p, args);
        }
        if key == "err" {
            return Ok(Val::ErrObj);
        }
        let a = self.args(args)?;
        if let Some(v) = self.builtin(&key, &a) {
            return v;
        }
        match self.host.global(&key, &a) {
            Some(v) => v,
            // Nome não declarado e sem parênteses: variável vazia.
            None if args.is_empty() => Ok(Val::Empty),
            None => Err(VbErr::new(35, format!("Função não definida: {n}"))),
        }
    }

    fn user_call(&mut self, p: &Rc<Proc>, args: &[Arg]) -> Res<V<H>> {
        let a = self.args(args)?;
        let (ret, back) = self.call_proc(p, a.pos)?;
        // Parâmetros por referência voltam para as variáveis.
        for ((arg, q), v) in args.iter().zip(&p.params).zip(back) {
            if !q.by_val
                && let Some(Expr::Name(n)) = &arg.value
                && self.lookup(n).is_some()
            {
                self.assign_name(n, v, true)?;
            }
        }
        Ok(ret)
    }

    fn call(&mut self, f: &Expr, args: &[Arg]) -> Res<V<H>> {
        match f {
            Expr::Name(n) => {
                if let Some(base) = self.lookup(n) {
                    // A própria função chamada dentro dela é recursão.
                    let recursive = self.procs.contains_key(&n.to_lowercase())
                        && !matches!(base, Val::Arr(_) | Val::Dict(_) | Val::Obj(_));
                    if !recursive {
                        return self.item(&base, args);
                    }
                }
                self.name(n, args)
            }
            Expr::Member(o, n) => {
                let o = self.eval(o)?;
                let a = self.args(args)?;
                self.member(&o, n, &a)
            }
            Expr::With(n) => {
                let o = self.with_obj()?;
                let a = self.args(args)?;
                self.member(&o, n, &a)
            }
            _ => {
                let base = self.eval(f)?;
                self.item(&base, args)
            }
        }
    }

    fn item(&mut self, base: &V<H>, args: &[Arg]) -> Res<V<H>> {
        match base {
            Val::Arr(a) => {
                let mut at = Vec::new();
                for x in args {
                    at.push(self.arg_num(x)?.round() as i64);
                }
                let a = a.borrow();
                let k = a.index(&at)?;
                Ok(a.data[k].clone())
            }
            Val::Dict(_) => {
                let a = self.args(args)?;
                self.member(base, "item", &a)
            }
            Val::Obj(o) => {
                let a = self.args(args)?;
                self.host.default(o, &a)
            }
            Val::Nothing | Val::Empty => Err(VbErr::new(91, "A variável de objeto não foi definida")),
            _ => Err(mismatch()),
        }
    }

    fn member(&mut self, o: &V<H>, name: &str, a: &Args<H::Obj>) -> Res<V<H>> {
        let key = name.to_lowercase();
        match o {
            Val::Obj(o) => self.host.get(o, &key, a),
            Val::ErrObj => Ok(match key.as_str() {
                "number" => Val::Num(self.err.number as f64),
                "description" => Val::Str(self.err.text.clone()),
                "source" => Val::Str(self.top().module.clone()),
                "clear" => {
                    self.err = VbErr::new(0, "");
                    Val::Empty
                }
                "raise" => {
                    let n = a.get(0, "number").map(to_num).transpose()?.unwrap_or(0.0) as i32;
                    let t = a.get(2, "description").map(to_str).transpose()?.unwrap_or_default();
                    return Err(VbErr::new(n, t));
                }
                n => return Err(VbErr::unsupported(n)),
            }),
            Val::Dict(d) => {
                let k = a.pos.first().cloned().unwrap_or(Val::Empty);
                let k = self.value_of(k)?;
                match key.as_str() {
                    "exists" => Ok(Val::Bool(d.borrow().map.contains_key(&dict_key(&k)))),
                    "count" => Ok(Val::Num(d.borrow().keys.len() as f64)),
                    "item" => {
                        let mut d = d.borrow_mut();
                        match d.map.get(&dict_key(&k)) {
                            Some(&i) => Ok(d.vals[i].clone()),
                            None => {
                                // Ler uma chave que não existe cria o item vazio.
                                dict_set(&mut d, k, Val::Empty);
                                Ok(Val::Empty)
                            }
                        }
                    }
                    "add" => {
                        let mut d = d.borrow_mut();
                        if d.map.contains_key(&dict_key(&k)) {
                            return Err(VbErr::new(457, "Esta chave já está na coleção"));
                        }
                        let v = a.pos.get(1).cloned().unwrap_or(Val::Empty);
                        dict_set(&mut d, k, v);
                        Ok(Val::Empty)
                    }
                    "remove" => {
                        dict_remove(&mut d.borrow_mut(), &k);
                        Ok(Val::Empty)
                    }
                    "removeall" => {
                        *d.borrow_mut() = Dict { keys: vec![], vals: vec![], map: HashMap::new() };
                        Ok(Val::Empty)
                    }
                    "keys" => Ok(array(d.borrow().keys.clone())),
                    "items" => Ok(array(d.borrow().vals.clone())),
                    n => Err(VbErr::unsupported(n)),
                }
            }
            Val::Nothing | Val::Empty => Err(VbErr::new(91, "A variável de objeto não foi definida")),
            _ => Err(VbErr::new(424, "Objeto obrigatório")),
        }
    }

    /// Funções da linguagem. `None` = não é uma.
    fn builtin(&mut self, name: &str, a: &Args<H::Obj>) -> Option<Res<V<H>>> {
        if !BUILTINS.contains(&name) {
            return None;
        }
        Some(self.builtin_run(name, a))
    }

    fn builtin_run(&mut self, name: &str, a: &Args<H::Obj>) -> Res<V<H>> {
        let mut v: Vec<V<H>> = Vec::with_capacity(a.pos.len());
        for x in &a.pos {
            v.push(self.value_of(x.clone())?);
        }
        let n = v.len();
        let at =
            |i: usize| -> Res<&V<H>> { v.get(i).ok_or_else(|| VbErr::new(450, "Número de argumentos")) };
        let s = |i: usize| -> Res<String> { to_str(at(i)?) };
        let f = |i: usize| -> Res<f64> { to_num(at(i)?) };
        Ok(match name {
            "array" => array(v),
            "ubound" | "lbound" => {
                let Val::Arr(x) = at(0)? else { return Err(mismatch()) };
                let d = if n > 1 { f(1)? as usize } else { 1 };
                let b = *x
                    .borrow()
                    .bounds
                    .get(d.wrapping_sub(1))
                    .ok_or_else(|| VbErr::new(9, "Subscrito fora do intervalo"))?;
                Val::Num(if name == "ubound" { b.1 } else { b.0 } as f64)
            }
            "isnumeric" => Val::Bool(match at(0)? {
                Val::Str(x) => parse_num(x).is_some(),
                Val::Num(_) | Val::Bool(_) | Val::Empty => true,
                _ => false,
            }),
            "isempty" => Val::Bool(matches!(at(0)?, Val::Empty)),
            "isnull" => Val::Bool(matches!(at(0)?, Val::Null)),
            "isarray" => Val::Bool(matches!(at(0)?, Val::Arr(_))),
            "iserror" => Val::Bool(matches!(at(0)?, Val::Str(x) if x.starts_with("Error "))),
            "isdate" => Val::Bool(match at(0)? {
                Val::Date(_) => true,
                Val::Str(x) => parse_date(x).is_some(),
                _ => false,
            }),
            "isobject" => Val::Bool(matches!(a.pos.first(), Some(Val::Obj(_) | Val::Dict(_)))),
            "typename" => Val::Str(
                match at(0)? {
                    Val::Empty => "Empty",
                    Val::Null => "Null",
                    Val::Nothing => "Nothing",
                    Val::Bool(_) => "Boolean",
                    Val::Num(_) => "Double",
                    Val::Date(_) => "Date",
                    Val::Str(_) => "String",
                    Val::Arr(_) => "Variant()",
                    Val::Dict(_) => "Dictionary",
                    _ => "Object",
                }
                .into(),
            ),
            "cstr" => Val::Str(s(0)?),
            "cint" | "clng" => Val::Num(round_even(f(0)?)),
            "cdbl" | "csng" | "ccur" | "cdec" => Val::Num(f(0)?),
            "cbool" => Val::Bool(truth(at(0)?)?),
            "cdate" | "datevalue" => match at(0)? {
                Val::Str(x) => Val::Date(parse_date(x).ok_or_else(mismatch)?),
                x => Val::Date(to_num(x)?),
            },
            "cvar" => at(0)?.clone(),
            "val" => {
                let t = s(0)?;
                let t = t.trim_start();
                let end = t
                    .char_indices()
                    .take_while(|(i, c)| c.is_ascii_digit() || *c == '.' || (*i == 0 && "+-".contains(*c)))
                    .last()
                    .map_or(0, |(i, c)| i + c.len_utf8());
                Val::Num(t[..end].parse().unwrap_or(0.0))
            }
            "len" => Val::Num(s(0)?.chars().count() as f64),
            "left" => Val::Str(s(0)?.chars().take(f(1)?.max(0.0) as usize).collect()),
            "right" => {
                let t: Vec<char> = s(0)?.chars().collect();
                let k = (f(1)?.max(0.0) as usize).min(t.len());
                Val::Str(t[t.len() - k..].iter().collect())
            }
            "mid" => {
                let t = s(0)?;
                let start = (f(1)?.max(1.0) as usize) - 1;
                let it = t.chars().skip(start);
                Val::Str(if n > 2 { it.take(f(2)?.max(0.0) as usize).collect() } else { it.collect() })
            }
            "trim" => Val::Str(s(0)?.trim_matches(' ').to_string()),
            "ltrim" => Val::Str(s(0)?.trim_start_matches(' ').to_string()),
            "rtrim" => Val::Str(s(0)?.trim_end_matches(' ').to_string()),
            "ucase" => Val::Str(s(0)?.to_uppercase()),
            "lcase" => Val::Str(s(0)?.to_lowercase()),
            "space" => Val::Str(" ".repeat(f(0)?.clamp(0.0, 1e6) as usize)),
            "string" => {
                Val::Str(s(1)?.chars().take(1).collect::<String>().repeat(f(0)?.clamp(0.0, 1e6) as usize))
            }
            "chr" | "chrw" => Val::Str(char::from_u32(f(0)? as u32).map(String::from).unwrap_or_default()),
            "asc" | "ascw" => Val::Num(s(0)?.chars().next().map_or(0.0, |c| c as u32 as f64)),
            "replace" => {
                let (t, from, to) = (s(0)?, s(1)?, s(2)?);
                Val::Str(if from.is_empty() { t } else { t.replace(&from, &to) })
            }
            "instr" => {
                // InStr([início,] texto, procurado [, comparação])
                let (start, i) = match at(0)? {
                    Val::Num(x) if n > 2 => (*x as usize, 1),
                    _ => (1, 0),
                };
                let (mut t, mut q) = (s(i)?, s(i + 1)?);
                if n > i + 2 && f(i + 2)? == 1.0 {
                    (t, q) = (t.to_lowercase(), q.to_lowercase());
                }
                let skip: String = t.chars().skip(start.max(1) - 1).collect();
                Val::Num(match skip.find(&q) {
                    Some(b) => (skip[..b].chars().count() + start.max(1)) as f64,
                    None => 0.0,
                })
            }
            "instrrev" => {
                let (t, q) = (s(0)?, s(1)?);
                Val::Num(t.rfind(&q).map_or(0.0, |b| t[..b].chars().count() as f64 + 1.0))
            }
            "strcomp" => {
                let (mut x, mut y) = (s(0)?, s(1)?);
                if n > 2 && f(2)? == 1.0 {
                    (x, y) = (x.to_lowercase(), y.to_lowercase());
                }
                Val::Num(x.cmp(&y) as i32 as f64)
            }
            "split" => {
                let sep = if n > 1 { s(1)? } else { " ".into() };
                let t = s(0)?;
                let parts: Vec<V<H>> = if t.is_empty() {
                    Vec::new()
                } else {
                    t.split(sep.as_str()).map(|x| Val::Str(x.to_string())).collect()
                };
                array(parts)
            }
            "join" => {
                let Val::Arr(x) = at(0)? else { return Err(mismatch()) };
                let sep = if n > 1 { s(1)? } else { " ".into() };
                let parts: Res<Vec<String>> = x.borrow().data.iter().map(to_str).collect();
                Val::Str(parts?.join(&sep))
            }
            "format" => Val::Str(if n > 1 { format_val(at(0)?, &s(1)?)? } else { s(0)? }),
            "now" => Val::Date(now()),
            "date" => Val::Date(now().floor()),
            "time" => Val::Date(now().fract()),
            "timer" => Val::Num((now().fract() * 86_400.0 * 100.0).round() / 100.0),
            "year" | "month" | "day" | "weekday" => {
                let d = match at(0)? {
                    Val::Str(x) => parse_date(x).ok_or_else(mismatch)?,
                    x => to_num(x)?,
                };
                if name == "weekday" {
                    Val::Num((d.floor() - 1.0).rem_euclid(7.0) + 1.0)
                } else {
                    let t = duck_core::numfmt::serial_to_datetime(d, false, 0).ok_or_else(mismatch)?;
                    Val::Num(match name {
                        "year" => t.year as f64,
                        "month" => t.month as f64,
                        _ => t.day as f64,
                    })
                }
            }
            "dateserial" => {
                let (y, m, d) = (f(0)? as i64, f(1)? as i64, f(2)? as i64);
                Val::Date(serial(y, m, d).ok_or_else(mismatch)?)
            }
            "abs" => Val::Num(f(0)?.abs()),
            "int" => Val::Num(f(0)?.floor()),
            "fix" => Val::Num(f(0)?.trunc()),
            "sgn" => Val::Num(if f(0)? == 0.0 { 0.0 } else { f(0)?.signum() }),
            "sqr" => Val::Num(f(0)?.sqrt()),
            "round" => {
                let p = 10f64.powi(if n > 1 { f(1)? as i32 } else { 0 });
                Val::Num(round_even(f(0)? * p) / p)
            }
            "rnd" => Val::Num(0.5),
            "rgb" => Val::Num(f(0)? + f(1)? * 256.0 + f(2)? * 65536.0),
            "iif" => {
                if truth(at(0)?)? {
                    a.pos.get(1).cloned().unwrap_or(Val::Empty)
                } else {
                    a.pos.get(2).cloned().unwrap_or(Val::Empty)
                }
            }
            "doevents" | "randomize" | "beep" => Val::Empty,
            "createobject" => {
                let t = s(0)?;
                if t.eq_ignore_ascii_case("scripting.dictionary") {
                    Val::Dict(Rc::new(RefCell::new(Dict { keys: vec![], vals: vec![], map: HashMap::new() })))
                } else {
                    return Err(VbErr::new(
                        429,
                        format!("A macro usa outro programa ({t}), que o duck-sheets não abre"),
                    ));
                }
            }
            "getobject" => {
                let t = if n > 1 { s(1)? } else { s(0)? };
                return Err(VbErr::new(
                    429,
                    format!("A macro usa outro programa ({t}), que o duck-sheets não abre"),
                ));
            }
            _ => return Err(VbErr::unsupported(name)),
        })
    }
}

const BUILTINS: &[&str] = &[
    "array",
    "ubound",
    "lbound",
    "isnumeric",
    "isempty",
    "isnull",
    "isarray",
    "iserror",
    "isdate",
    "isobject",
    "typename",
    "cstr",
    "cint",
    "clng",
    "cdbl",
    "csng",
    "ccur",
    "cdec",
    "cbool",
    "cdate",
    "datevalue",
    "cvar",
    "val",
    "len",
    "left",
    "right",
    "mid",
    "trim",
    "ltrim",
    "rtrim",
    "ucase",
    "lcase",
    "space",
    "string",
    "chr",
    "chrw",
    "asc",
    "ascw",
    "replace",
    "instr",
    "instrrev",
    "strcomp",
    "split",
    "join",
    "format",
    "now",
    "date",
    "time",
    "timer",
    "year",
    "month",
    "day",
    "weekday",
    "dateserial",
    "abs",
    "int",
    "fix",
    "sgn",
    "sqr",
    "round",
    "rnd",
    "rgb",
    "iif",
    "doevents",
    "randomize",
    "beep",
    "createobject",
    "getobject",
];

// ---------------------------------------------------------------------------
// Valores
// ---------------------------------------------------------------------------

/// Matriz de uma dimensão a partir de 0.
pub fn array<O: Clone>(v: Vec<Val<O>>) -> Val<O> {
    let n = v.len() as i64;
    Val::Arr(Rc::new(RefCell::new(Arr { bounds: vec![(0, n - 1)], data: v })))
}

fn copy_preserved<O: Clone>(old: &Arr<O>, new: &mut Arr<O>) {
    if old.bounds.len() != new.bounds.len() || old.bounds.is_empty() {
        return;
    }
    // Só a última dimensão muda no VBA; copia o que existe nas duas.
    let mut at: Vec<i64> = old.bounds.iter().map(|b| b.0).collect();
    for v in &old.data {
        if let Ok(k) = new.index(&at) {
            new.data[k] = v.clone();
        }
        for d in (0..at.len()).rev() {
            at[d] += 1;
            if at[d] <= old.bounds[d].1 {
                break;
            }
            at[d] = old.bounds[d].0;
        }
    }
}

fn dict_key<O>(v: &Val<O>) -> String {
    match v {
        Val::Str(s) => format!("s{s}"),
        Val::Num(n) | Val::Date(n) => format!("n{n}"),
        Val::Bool(b) => format!("n{}", if *b { -1 } else { 0 }),
        _ => "e".into(),
    }
}

fn dict_set<O: Clone>(d: &mut Dict<O>, k: Val<O>, v: Val<O>) {
    let key = dict_key(&k);
    match d.map.get(&key) {
        Some(&i) => d.vals[i] = v,
        None => {
            d.map.insert(key, d.keys.len());
            d.keys.push(k);
            d.vals.push(v);
        }
    }
}

fn dict_remove<O>(d: &mut Dict<O>, k: &Val<O>) {
    if let Some(i) = d.map.remove(&dict_key(k)) {
        d.keys.remove(i);
        d.vals.remove(i);
        d.map.values_mut().filter(|x| **x > i).for_each(|x| *x -= 1);
    }
}

fn default_of<O>(ty: &str) -> Val<O> {
    match ty {
        "integer" | "long" | "longlong" | "longptr" | "byte" | "double" | "single" | "currency" => {
            Val::Num(0.0)
        }
        "string" => Val::Str(String::new()),
        "boolean" => Val::Bool(false),
        "date" => Val::Date(0.0),
        "variant" => Val::Empty,
        _ => Val::Nothing,
    }
}

/// Converte para o tipo declarado da variável.
fn coerce<O: Clone>(v: Val<O>, ty: &str) -> Res<Val<O>> {
    Ok(match ty {
        "integer" | "long" | "longlong" | "longptr" | "byte" => {
            let n = round_even(to_num(&v)?);
            if ty == "integer" && n.abs() > 32_767.0 {
                return Err(VbErr::new(6, "Estouro"));
            }
            Val::Num(n)
        }
        "double" | "single" | "currency" => Val::Num(to_num(&v)?),
        "string" => Val::Str(to_str(&v)?),
        "boolean" => Val::Bool(truth(&v)?),
        "date" => Val::Date(to_num(&v)?),
        _ => v,
    })
}

pub fn round_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 { 2.0 * (x / 2.0).round() } else { r }
}

/// Número escrito em português (`1.234,5`) ou com ponto decimal (`12.5`).
pub fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || "+-.,eE%".contains(c)) {
        return None;
    }
    if t.contains(',') {
        return t.replace('.', "").replace(',', ".").parse().ok();
    }
    // `1.234` em português é mil duzentos e trinta e quatro.
    let groups: Vec<&str> = t.trim_start_matches(['+', '-']).split('.').collect();
    if groups.len() > 1
        && !groups[0].is_empty()
        && groups[0].len() <= 3
        && groups[1..].iter().all(|g| g.len() == 3)
    {
        return t.replace('.', "").parse().ok();
    }
    t.parse().ok()
}

fn parse_date(s: &str) -> Option<f64> {
    let p: Vec<i64> = s.trim().split(['/', '-']).map(|x| x.parse().ok()).collect::<Option<_>>()?;
    match p[..] {
        [d, m, y] if d <= 31 => serial(if y < 100 { y + 2000 } else { y }, m, d),
        [y, m, d] => serial(y, m, d),
        _ => None,
    }
}

fn serial(y: i64, m: i64, d: i64) -> Option<f64> {
    duck_calc::funcs::date_serial(y, m, d, false).ok()
}

pub fn to_num<O>(v: &Val<O>) -> Res<f64> {
    match v {
        Val::Num(n) | Val::Date(n) => Ok(*n),
        Val::Bool(b) => Ok(if *b { -1.0 } else { 0.0 }),
        Val::Empty => Ok(0.0),
        Val::Str(s) => parse_num(s).ok_or_else(mismatch),
        Val::Null => Err(VbErr::new(94, "Uso inválido de Null")),
        _ => Err(mismatch()),
    }
}

/// Texto de um valor, com a vírgula decimal do português.
pub fn to_str<O>(v: &Val<O>) -> Res<String> {
    Ok(match v {
        Val::Str(s) => s.clone(),
        Val::Num(n) => {
            let mut s = String::new();
            duck_core::numfmt::format_general(*n, 15, &mut s);
            s
        }
        Val::Date(d) => {
            let t = duck_core::numfmt::serial_to_datetime(*d, false, 0).ok_or_else(mismatch)?;
            let mut s = format!("{:02}/{:02}/{}", t.day, t.month, t.year);
            if d.fract() != 0.0 {
                s.push_str(&format!(" {:02}:{:02}:{:02}", t.hour, t.minute, t.second));
            }
            s
        }
        Val::Bool(b) => if *b { "Verdadeiro" } else { "Falso" }.into(),
        Val::Empty => String::new(),
        Val::Null => return Err(VbErr::new(94, "Uso inválido de Null")),
        _ => return Err(mismatch()),
    })
}

pub fn truth<O>(v: &Val<O>) -> Res<bool> {
    match v {
        Val::Bool(b) => Ok(*b),
        Val::Str(s) if s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("verdadeiro") => Ok(true),
        Val::Str(s) if s.eq_ignore_ascii_case("false") || s.eq_ignore_ascii_case("falso") => Ok(false),
        Val::Null => Ok(false),
        v => Ok(to_num(v)? != 0.0),
    }
}

fn compare<O>(op: Bin, a: &Val<O>, b: &Val<O>) -> Res<Val<O>> {
    use std::cmp::Ordering;
    if matches!(a, Val::Null) || matches!(b, Val::Null) {
        return Ok(Val::Null);
    }
    let ord = match (a, b) {
        (Val::Str(x), Val::Str(y)) => x.cmp(y),
        (Val::Str(x), Val::Empty) => x.as_str().cmp(""),
        (Val::Empty, Val::Str(y)) => "".cmp(y.as_str()),
        // Texto com número: compara como número se der; senão o texto é maior.
        (Val::Str(x), other) => match parse_num(x) {
            Some(n) => n.partial_cmp(&to_num(other)?).unwrap_or(Ordering::Equal),
            None => {
                to_num(other)?;
                Ordering::Greater
            }
        },
        (other, Val::Str(y)) => match parse_num(y) {
            Some(n) => to_num(other)?.partial_cmp(&n).unwrap_or(Ordering::Equal),
            None => {
                to_num(other)?;
                Ordering::Less
            }
        },
        (x, y) => to_num(x)?.partial_cmp(&to_num(y)?).unwrap_or(Ordering::Equal),
    };
    Ok(Val::Bool(match op {
        Bin::Eq => ord.is_eq(),
        Bin::Ne => ord.is_ne(),
        Bin::Lt => ord.is_lt(),
        Bin::Le => ord.is_le(),
        Bin::Gt => ord.is_gt(),
        _ => ord.is_ge(),
    }))
}

fn like(s: &[char], p: &[char]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some('*') => (0..=s.len()).any(|i| like(&s[i..], &p[1..])),
        Some('?') => !s.is_empty() && like(&s[1..], &p[1..]),
        Some('#') => s.first().is_some_and(|c| c.is_ascii_digit()) && like(&s[1..], &p[1..]),
        Some(c) => s.first() == Some(c) && like(&s[1..], &p[1..]),
    }
}

fn binary<O: Clone>(op: Bin, a: &Val<O>, b: &Val<O>) -> Res<Val<O>> {
    Ok(match op {
        Bin::Concat => {
            let t = |v: &Val<O>| if matches!(v, Val::Null) { Ok(String::new()) } else { to_str(v) };
            Val::Str(t(a)? + &t(b)?)
        }
        Bin::Add => match (a, b) {
            (Val::Str(x), Val::Str(y)) => Val::Str(format!("{x}{y}")),
            (Val::Date(x), y) | (y, Val::Date(x)) => Val::Date(x + to_num(y)?),
            _ => Val::Num(to_num(a)? + to_num(b)?),
        },
        Bin::Sub => match (a, b) {
            (Val::Date(x), Val::Date(y)) => Val::Num(x - y),
            (Val::Date(x), y) => Val::Date(x - to_num(y)?),
            _ => Val::Num(to_num(a)? - to_num(b)?),
        },
        Bin::Mul => Val::Num(to_num(a)? * to_num(b)?),
        Bin::Div | Bin::IntDiv | Bin::Mod => {
            let (x, y) = (to_num(a)?, to_num(b)?);
            let (xi, yi) = (round_even(x), round_even(y));
            if y == 0.0 || (op != Bin::Div && yi == 0.0) {
                return Err(VbErr::new(11, "Divisão por zero"));
            }
            Val::Num(match op {
                Bin::Div => x / y,
                Bin::IntDiv => (xi / yi).trunc(),
                _ => xi % yi,
            })
        }
        Bin::Pow => Val::Num(to_num(a)?.powf(to_num(b)?)),
        Bin::And | Bin::Or | Bin::Xor => match (a, b) {
            // Com números, é bit a bit.
            (Val::Num(x), Val::Num(y)) => {
                let (x, y) = (*x as i64, *y as i64);
                Val::Num(match op {
                    Bin::And => x & y,
                    Bin::Or => x | y,
                    _ => x ^ y,
                } as f64)
            }
            _ => {
                let (x, y) = (truth(a)?, truth(b)?);
                Val::Bool(match op {
                    Bin::And => x && y,
                    Bin::Or => x || y,
                    _ => x != y,
                })
            }
        },
        Bin::Like => {
            let (s, p): (Vec<char>, Vec<char>) = (to_str(a)?.chars().collect(), to_str(b)?.chars().collect());
            Val::Bool(like(&s, &p))
        }
        Bin::Is => Val::Bool(false),
        op => return compare(op, a, b),
    })
}

fn format_val<O>(v: &Val<O>, fmt: &str) -> Res<String> {
    let n = match v {
        Val::Str(s) => match parse_num(s) {
            Some(n) => n,
            None => return Ok(s.clone()),
        },
        v => to_num(v)?,
    };
    // Datas do VBA: `mm` depois de hora são minutos, como nos formatos de célula.
    let code = match fmt.to_lowercase().as_str() {
        "percent" => "0.00%".to_string(),
        "fixed" => "0.00".to_string(),
        "standard" => "#,##0.00".to_string(),
        "short date" => "dd/mm/yyyy".to_string(),
        _ => fmt.to_string(),
    };
    let f = duck_core::NumFmt::parse(&code);
    let mut out = String::new();
    f.format_number(n, false, &mut out);
    Ok(out)
}

/// Data e hora de agora (serial do Excel, hora local).
fn now() -> f64 {
    duck_calc::eval::now_serial()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_module;

    /// Sem objetos: só guarda o que a macro manda para `Saida`.
    struct Log(Vec<String>);

    impl Host for Log {
        type Obj = u8;

        fn global(&mut self, name: &str, a: &Args<u8>) -> Option<Res<Val<u8>>> {
            match name {
                "saida" => {
                    self.0.push(a.pos.iter().map(|v| to_str(v).unwrap()).collect::<Vec<_>>().join("|"));
                    Some(Ok(Val::Empty))
                }
                "falha" => Some(Err(VbErr::new(1004, "falhou"))),
                "coisa" => Some(Ok(Val::Obj(1))),
                "nada" => Some(Ok(Val::Nothing)),
                _ => None,
            }
        }
        fn get(&mut self, _: &u8, name: &str, _: &Args<u8>) -> Res<Val<u8>> {
            Ok(Val::Str(format!(".{name}")))
        }
        fn set(&mut self, _: &u8, name: &str, _: &Args<u8>, v: Val<u8>) -> Res<()> {
            self.0.push(format!("{name}={}", to_str(&v)?));
            Ok(())
        }
        fn default(&mut self, _: &u8, _: &Args<u8>) -> Res<Val<u8>> {
            Ok(Val::Num(42.0))
        }
        fn items(&mut self, _: &u8) -> Res<Vec<Val<u8>>> {
            Ok(vec![Val::Num(1.0), Val::Num(2.0)])
        }
    }

    fn run(src: &str) -> (Res<()>, Vec<String>) {
        let mut log = Log(Vec::new());
        let r = Interp::new(&mut log, vec![parse_module("M", src)]).run("main");
        (r, log.0)
    }

    fn out(src: &str) -> Vec<String> {
        let (r, l) = run(src);
        assert_eq!(r, Ok(()), "{l:?}");
        l
    }

    #[test]
    fn values_and_loops() {
        let o = out(
            "Sub Main()\nDim v As Variant, i As Integer, s As String\nv = Array(\"a\", \"b\", \"c\")\nFor i = LBound(v) To UBound(v)\n s = s & v(i)\nNext i\nSaida s, UBound(v) + 1, 7 Mod 4, 7 \\ 2, 1 / 4\ni = 2.5: Saida i\ni = 3.5: Saida i\nDo While i > 0\n i = i - 2\n If i = 2 Then Exit Do\nLoop\nSaida i\nSelect Case i\nCase 1: Saida \"um\"\nCase 2 To 3: Saida \"dois\"\nCase Else: Saida \"?\"\nEnd Select\nEnd Sub",
        );
        assert_eq!(o, ["abc|3|3|3|0,25", "2", "4", "2", "dois"]);
    }

    #[test]
    fn errors() {
        let o = out(
            "Sub Main()\nOn Error Resume Next\nFalha\nSaida Err.Number, Err.Description\nErr.Clear\nSaida Err.Number\nDim o As Object\nSet o = Nada\nOn Error GoTo 0\nIf o Is Nothing Then Saida \"nada\"\nSet o = Coisa\nIf Not o Is Nothing Then Saida \"coisa\"\nOn Error GoTo ruim\nFalha\nSaida \"não chega\"\nfim:\nSaida \"fim\"\nExit Sub\nruim:\nSaida \"tratou \" & Err.Description\nResume fim\nEnd Sub",
        );
        assert_eq!(o, ["1004|falhou", "0", "nada", "coisa", "tratou falhou", "fim"]);
        // Sem tratamento, sobe; `GoTo` sai de laços.
        assert_eq!(run("Sub Main()\nFalha\nEnd Sub").0, Err(VbErr::new(1004, "falhou")));
        let o =
            out("Sub Main()\nDim i\nFor i = 1 To 9\nIf i = 3 Then GoTo fora\nNext\nfora:\nSaida i\nEnd Sub");
        assert_eq!(o, ["3"]);
    }

    #[test]
    fn functions_dictionary_and_arrays() {
        let o = out(
            "Sub Main()\nDim d As Object, k, m() As Variant, i As Long\nSet d = CreateObject(\"Scripting.Dictionary\")\nd(\"x\") = 1\nIf Not d.Exists(\"y\") Then d.Add \"y\", Dobro(4)\nFor Each k In d.Keys\n Saida k, d(k)\nNext k\nReDim m(1 To 2, 1 To 3)\nm(2, 3) = \"fim\"\nSaida UBound(m, 1), UBound(m, 2), m(2, 3), d.Count\ni = 5\nMuda i\nSaida i, Limpa(\"a.b-c\")\nEnd Sub\nFunction Dobro(n As Long) As Long\nDobro = n * 2\nEnd Function\nSub Muda(x As Long)\nx = x + 1\nEnd Sub\nFunction Limpa(s As String) As String\nDim t As String\nt = UCase(s)\nt = Replace(t, \".\", \"\"): t = Replace(t, \"-\", \" \")\nLimpa = t\nEnd Function",
        );
        assert_eq!(o, ["x|1", "y|8", "2|3|fim|2", "6|AB C"]);
    }

    #[test]
    fn objects_and_with() {
        let o = out(
            "Sub Main()\nWith Coisa\n .Nome = .Outro & \"!\"\nEnd With\nDim x As Long\nx = Coisa\nFor Each i In Coisa\n x = x + i\nNext\nSaida x\nEnd Sub",
        );
        assert_eq!(o, ["nome=.outro!", "45"]);
        assert!(
            run("Sub Main()\nDim p As Object\nSet p = CreateObject(\"PowerPoint.Application\")\nEnd Sub")
                .0
                .unwrap_err()
                .text
                .contains("PowerPoint")
        );
        assert_eq!(run("Sub Outra()\nEnd Sub").0.unwrap_err().number, 1004);
    }

    #[test]
    fn numbers_and_formats() {
        assert_eq!(parse_num("1.234,5"), Some(1234.5));
        assert_eq!(parse_num("12.5"), Some(12.5));
        assert_eq!(parse_num("abc"), None);
        assert_eq!(round_even(0.5), 0.0);
        assert_eq!(round_even(-1.5), -2.0);
        let o = out(
            "Sub Main()\nSaida Format(0.256, \"0.0%\"), InStr(\"abcabc\", \"c\"), InStr(4, \"abcabc\", \"c\"), Mid(\"abcd\", 2, 2), IsNumeric(\"12\"), Trim(\"  a \")\nEnd Sub",
        );
        assert_eq!(o, ["25,6%|3|6|bc|Verdadeiro|a"]);
    }
}
