//! Funções de planilha.
//!
//! Cada função recebe os nós dos argumentos (sem avaliar): SE, SEERRO,
//! ESCOLHER e outras só avaliam o que precisam, como no Excel. Os nomes são
//! os do arquivo (inglês); o nome em português serve para mostrar e digitar.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use duck_core::numfmt::serial_to_datetime;
use duck_core::{CellError, NumFmt};

use crate::eval::{Eval, Res, to_bool, to_text};
use crate::index::{self, Crit, Wild, has_wildcards, lookup_eq, round15};
use crate::parse::{self, Node};
use crate::value::{Arr, Rng, V, compare, parse_number};

pub type Func = u16;
pub type Imp = for<'e> fn(&mut Eval<'e>, &[u32]) -> Res<V<'e>>;

pub struct FnDef {
    /// Nome no arquivo (inglês, sem `_xlfn.`).
    pub name: &'static str,
    /// Nome no Excel em português.
    pub pt: &'static str,
    pub min: u8,
    pub max: u8,
    pub volatile: bool,
    /// Referências que só se sabem ao calcular (INDIRETO, DESLOC).
    pub dynamic: bool,
    /// O motor não calcula: o valor guardado fica.
    pub opaque: bool,
    pub imp: Imp,
}

const fn f(name: &'static str, pt: &'static str, min: u8, max: u8, imp: Imp) -> FnDef {
    FnDef { name, pt, min, max, volatile: false, dynamic: false, opaque: false, imp }
}

const fn vol(name: &'static str, pt: &'static str, min: u8, max: u8, imp: Imp) -> FnDef {
    FnDef { name, pt, min, max, volatile: true, dynamic: false, opaque: false, imp }
}

const fn dynf(name: &'static str, pt: &'static str, min: u8, max: u8, imp: Imp) -> FnDef {
    FnDef { name, pt, min, max, volatile: true, dynamic: true, opaque: false, imp }
}

const fn opq(name: &'static str, pt: &'static str) -> FnDef {
    FnDef { name, pt, min: 0, max: 255, volatile: false, dynamic: false, opaque: true, imp: opaque }
}

static FUNCS: &[FnDef] = &[
    // Lógicas
    f("IF", "SE", 1, 3, if_),
    f("IFS", "SES", 2, 254, ifs),
    f("IFERROR", "SEERRO", 2, 2, iferror),
    f("IFNA", "SENÃODISP", 2, 2, ifna),
    f("AND", "E", 1, 255, and),
    f("OR", "OU", 1, 255, or),
    f("XOR", "OUEXCL", 1, 255, xor),
    f("NOT", "NÃO", 1, 1, not),
    f("TRUE", "VERDADEIRO", 0, 0, true_),
    f("FALSE", "FALSO", 0, 0, false_),
    f("SWITCH", "PARÂMETRO", 3, 254, switch),
    f("CHOOSE", "ESCOLHER", 2, 255, choose),
    // Informação
    f("ISBLANK", "ÉCÉL.VAZIA", 1, 1, isblank),
    f("ISERROR", "ÉERROS", 1, 1, iserror),
    f("ISERR", "ÉERRO", 1, 1, iserr),
    f("ISNA", "É.NÃO.DISP", 1, 1, isna),
    f("ISNUMBER", "ÉNÚM", 1, 1, isnumber),
    f("ISTEXT", "ÉTEXTO", 1, 1, istext),
    f("ISNONTEXT", "É.NÃO.TEXTO", 1, 1, isnontext),
    f("ISLOGICAL", "ÉLÓGICO", 1, 1, islogical),
    f("ISEVEN", "ÉPAR", 1, 1, iseven),
    f("ISODD", "ÉIMPAR", 1, 1, isodd),
    f("ISREF", "ÉREF", 1, 1, isref),
    f("NA", "NÃO.DISP", 0, 0, na),
    f("ERROR.TYPE", "TIPO.ERRO", 1, 1, error_type),
    f("TYPE", "TIPO", 1, 1, type_),
    f("N", "N", 1, 1, n_),
    f("T", "T", 1, 1, t_),
    f("ROW", "LIN", 0, 1, row),
    f("COLUMN", "COL", 0, 1, column),
    f("ROWS", "LINS", 1, 1, rows),
    f("COLUMNS", "COLS", 1, 1, columns),
    // Matemáticas e estatísticas
    f("SUM", "SOMA", 1, 255, sum),
    f("SUMPRODUCT", "SOMARPRODUTO", 1, 255, sumproduct),
    f("SUMIF", "SOMASE", 2, 3, sumif),
    f("SUMIFS", "SOMASES", 3, 255, sumifs),
    f("COUNT", "CONT.NÚM", 1, 255, count),
    f("COUNTA", "CONT.VALORES", 1, 255, counta),
    f("COUNTBLANK", "CONTAR.VAZIO", 1, 1, countblank),
    f("COUNTIF", "CONT.SE", 2, 2, countif),
    f("COUNTIFS", "CONT.SES", 2, 255, countifs),
    f("AVERAGE", "MÉDIA", 1, 255, average),
    f("AVERAGEA", "MÉDIAA", 1, 255, averagea),
    f("AVERAGEIF", "MÉDIASE", 2, 3, averageif),
    f("AVERAGEIFS", "MÉDIASES", 3, 255, averageifs),
    f("MIN", "MÍNIMO", 1, 255, min),
    f("MAX", "MÁXIMO", 1, 255, max),
    f("MINIFS", "MÍNIMOSES", 3, 255, minifs),
    f("MAXIFS", "MÁXIMOSES", 3, 255, maxifs),
    f("MEDIAN", "MED", 1, 255, median),
    f("LARGE", "MAIOR", 2, 2, large),
    f("SMALL", "MENOR", 2, 2, small),
    f("RANK", "ORDEM", 2, 3, rank),
    f("RANK.EQ", "ORDEM.EQ", 2, 3, rank),
    f("STDEV", "DESVPAD", 1, 255, stdev_s),
    f("STDEV.S", "DESVPAD.A", 1, 255, stdev_s),
    f("STDEV.P", "DESVPAD.P", 1, 255, stdev_p),
    f("STDEVP", "DESVPADP", 1, 255, stdev_p),
    f("VAR", "VAR", 1, 255, var_s),
    f("VAR.S", "VAR.A", 1, 255, var_s),
    f("VAR.P", "VAR.P", 1, 255, var_p),
    f("VARP", "VARP", 1, 255, var_p),
    f("PRODUCT", "MULT", 1, 255, product),
    f("SUBTOTAL", "SUBTOTAL", 2, 255, subtotal),
    f("ABS", "ABS", 1, 1, abs),
    f("SIGN", "SINAL", 1, 1, sign),
    f("ROUND", "ARRED", 2, 2, round),
    f("ROUNDUP", "ARREDONDAR.PARA.CIMA", 2, 2, roundup),
    f("ROUNDDOWN", "ARREDONDAR.PARA.BAIXO", 2, 2, rounddown),
    f("INT", "INT", 1, 1, int),
    f("TRUNC", "TRUNCAR", 1, 2, trunc),
    f("MOD", "MOD", 2, 2, mod_),
    f("QUOTIENT", "QUOCIENTE", 2, 2, quotient),
    f("POWER", "POTÊNCIA", 2, 2, power),
    f("SQRT", "RAIZ", 1, 1, sqrt),
    f("EXP", "EXP", 1, 1, exp),
    f("LN", "LN", 1, 1, ln),
    f("LOG", "LOG", 1, 2, log),
    f("LOG10", "LOG10", 1, 1, log10),
    f("PI", "PI", 0, 0, pi),
    f("CEILING", "TETO", 1, 2, ceiling),
    f("CEILING.MATH", "TETO.MAT", 1, 3, ceiling_math),
    f("FLOOR", "ARREDMULTB", 1, 2, floor),
    f("FLOOR.MATH", "ARREDMULTB.MAT", 1, 3, floor_math),
    f("MROUND", "MARRED", 2, 2, mround),
    f("EVEN", "PAR", 1, 1, even),
    f("ODD", "ÍMPAR", 1, 1, odd),
    f("FACT", "FATORIAL", 1, 1, fact),
    vol("RAND", "ALEATÓRIO", 0, 0, rand),
    vol("RANDBETWEEN", "ALEATÓRIOENTRE", 2, 2, randbetween),
    // Texto
    f("CONCATENATE", "CONCATENAR", 1, 255, concatenate),
    f("CONCAT", "CONCAT", 1, 255, concat),
    f("TEXTJOIN", "UNIRTEXTO", 3, 255, textjoin),
    f("LEFT", "ESQUERDA", 1, 2, left),
    f("RIGHT", "DIREITA", 1, 2, right),
    f("MID", "EXT.TEXTO", 3, 3, mid),
    f("LEN", "NÚM.CARACT", 1, 1, len),
    f("UPPER", "MAIÚSCULA", 1, 1, upper),
    f("LOWER", "MINÚSCULA", 1, 1, lower),
    f("PROPER", "PRI.MAIÚSCULA", 1, 1, proper),
    f("TRIM", "ARRUMAR", 1, 1, trim),
    f("CLEAN", "TIRAR", 1, 1, clean),
    f("SUBSTITUTE", "SUBSTITUIR", 3, 4, substitute),
    f("REPLACE", "MUDAR", 4, 4, replace),
    f("REPT", "REPT", 2, 2, rept),
    f("FIND", "PROCURAR", 2, 3, find),
    f("SEARCH", "LOCALIZAR", 2, 3, search),
    f("TEXT", "TEXTO", 2, 2, text),
    f("VALUE", "VALOR", 1, 1, value),
    f("NUMBERVALUE", "VALORNUMÉRICO", 1, 3, numbervalue),
    f("EXACT", "EXATO", 2, 2, exact),
    f("CHAR", "CARACT", 1, 1, char_),
    f("CODE", "CÓDIGO", 1, 1, code),
    f("UNICHAR", "CARACTUNICODE", 1, 1, unichar),
    f("UNICODE", "UNICODE", 1, 1, unicode),
    f("FIXED", "DEF.NÚM.DEC", 1, 3, fixed),
    f("TEXTBEFORE", "TEXTOANTES", 2, 6, textbefore),
    f("TEXTAFTER", "TEXTODEPOIS", 2, 6, textafter),
    // Datas
    f("DATE", "DATA", 3, 3, date),
    f("TIME", "TEMPO", 3, 3, time),
    f("YEAR", "ANO", 1, 1, year),
    f("MONTH", "MÊS", 1, 1, month),
    f("DAY", "DIA", 1, 1, day),
    f("HOUR", "HORA", 1, 1, hour),
    f("MINUTE", "MINUTO", 1, 1, minute),
    f("SECOND", "SEGUNDO", 1, 1, second),
    vol("TODAY", "HOJE", 0, 0, today),
    vol("NOW", "AGORA", 0, 0, now),
    f("WEEKDAY", "DIA.DA.SEMANA", 1, 2, weekday),
    f("WEEKNUM", "NÚMSEMANA", 1, 2, weeknum),
    f("ISOWEEKNUM", "NÚMSEMANAISO", 1, 1, isoweeknum),
    f("EOMONTH", "FIMMÊS", 2, 2, eomonth),
    f("EDATE", "DATAM", 2, 2, edate),
    f("DATEDIF", "DATADIF", 3, 3, datedif),
    f("DAYS", "DIAS", 2, 2, days),
    f("DATEVALUE", "DATA.VALOR", 1, 1, datevalue),
    f("TIMEVALUE", "VALOR.TEMPO", 1, 1, datevalue_time),
    f("NETWORKDAYS", "DIATRABALHOTOTAL", 2, 3, networkdays),
    f("WORKDAY", "DIATRABALHO", 2, 3, workday),
    // Busca e referência
    f("VLOOKUP", "PROCV", 3, 4, vlookup),
    f("HLOOKUP", "PROCH", 3, 4, hlookup),
    f("LOOKUP", "PROC", 2, 3, lookup_fn),
    f("MATCH", "CORRESP", 2, 3, match_),
    f("XMATCH", "CORRESPX", 2, 4, xmatch),
    f("XLOOKUP", "PROCX", 3, 6, xlookup),
    f("INDEX", "ÍNDICE", 1, 4, index),
    dynf("OFFSET", "DESLOC", 3, 5, offset),
    dynf("INDIRECT", "INDIRETO", 1, 2, indirect),
    f("ADDRESS", "ENDEREÇO", 2, 5, address),
    f("HYPERLINK", "HIPERLINK", 1, 2, hyperlink),
    // Ainda não calculadas: o valor guardado fica.
    opq("GETPIVOTDATA", "INFODADOSTABELADINÂMICA"),
    opq("FILTER", "FILTRO"),
    opq("UNIQUE", "ÚNICO"),
    opq("SORT", "CLASSIFICAR"),
    opq("SORTBY", "CLASSIFICARPOR"),
    opq("SEQUENCE", "SEQUÊNCIA"),
    opq("LET", "LET"),
    opq("LAMBDA", "LAMBDA"),
    opq("CELL", "CÉL"),
    opq("INFO", "INFORMAÇÃO"),
    opq("AGGREGATE", "AGREGAR"),
    opq("RTD", "RTD"),
    opq("CUBEVALUE", "VALORCUBO"),
    opq("CUBEMEMBER", "MEMBROCUBO"),
    opq("WEBSERVICE", "SERVIÇOWEB"),
    opq("YEARFRAC", "FRAÇÃOANO"),
    opq("PMT", "PGTO"),
    opq("NPV", "VPL"),
    opq("IRR", "TIR"),
];

fn table() -> &'static HashMap<String, Func> {
    static T: OnceLock<HashMap<String, Func>> = OnceLock::new();
    T.get_or_init(|| FUNCS.iter().enumerate().map(|(i, d)| (d.name.to_string(), i as Func)).collect())
}

fn table_pt() -> &'static HashMap<String, Func> {
    static T: OnceLock<HashMap<String, Func>> = OnceLock::new();
    T.get_or_init(|| FUNCS.iter().enumerate().map(|(i, d)| (d.pt.to_uppercase(), i as Func)).collect())
}

/// Função pelo nome do arquivo (`_xlfn.` e `_xlws.` ignorados).
pub fn lookup(name: &str) -> Option<Func> {
    let mut n = name;
    loop {
        let before = n;
        for p in ["_xlfn.", "_xlws.", "_XLFN.", "_XLWS."] {
            n = n.strip_prefix(p).unwrap_or(n);
        }
        if n == before {
            break;
        }
    }
    if let Some(&f) = table().get(n) {
        return Some(f);
    }
    table().get(&n.to_ascii_uppercase()).copied()
}

/// Função pelo nome em português.
pub fn lookup_pt(name: &str) -> Option<Func> {
    table_pt().get(&name.to_uppercase()).copied()
}

pub fn def(f: Func) -> &'static FnDef {
    &FUNCS[f as usize]
}

pub fn all() -> &'static [FnDef] {
    FUNCS
}

fn opaque<'e>(e: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    e.opaque = true;
    Err(CellError::Name)
}

// ---------------------------------------------------------------------------
// Ajudantes
// ---------------------------------------------------------------------------

fn arg_or<'e>(e: &mut Eval<'e>, a: &[u32], i: usize) -> Option<V<'e>> {
    let &n = a.get(i)?;
    if e.is_missing(n) { None } else { Some(e.scalar(n)) }
}

/// Números dos argumentos como SOMA conta: em referências e matrizes só os
/// números; argumentos diretos convertidos (lógicos, texto numérico).
fn nums<'e>(e: &mut Eval<'e>, a: &[u32], f: &mut dyn FnMut(f64)) -> Res<()> {
    for &n in a {
        match e.eval(n) {
            V::Ref(r) => {
                let mut err = None;
                e.book.each(&r, |_, _, v| match v {
                    V::Num(x) => {
                        f(x);
                        true
                    }
                    V::Err(x) => {
                        err = Some(x);
                        false
                    }
                    _ => true,
                });
                if let Some(x) = err {
                    return Err(x);
                }
            }
            V::Arr(arr) => {
                for v in &arr.v {
                    match v {
                        V::Num(x) => f(*x),
                        V::Err(x) => return Err(*x),
                        _ => {}
                    }
                }
            }
            V::Empty => {}
            v => f(e.to_num(&v)?),
        }
    }
    Ok(())
}

fn num_list<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<Vec<f64>> {
    let mut v = Vec::new();
    nums(e, a, &mut |x| v.push(x))?;
    Ok(v)
}

/// Todos os valores não vazios (referências, matrizes e diretos).
fn each_value<'e>(e: &mut Eval<'e>, a: &[u32], f: &mut dyn FnMut(&V<'e>)) {
    for &n in a {
        match e.eval(n) {
            V::Ref(r) => e.book.each(&r, |_, _, v| {
                f(&v);
                true
            }),
            V::Arr(arr) => arr.v.iter().for_each(&mut *f),
            v => f(&v),
        }
    }
}

fn tri<'e>(v: V<'e>) -> Res<V<'e>> {
    match v {
        V::Err(e) => Err(e),
        v => Ok(v),
    }
}

fn chars_take(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn chars_skip(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

// ---------------------------------------------------------------------------
// Lógicas
// ---------------------------------------------------------------------------

fn if_<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let c = e.eval(a[0]);
    if e.is_arrayish(&c) {
        // SE elemento a elemento (dentro de SOMARPRODUTO / fórmula matricial).
        let arr = e.to_arr(c);
        let yes = a.get(1).map(|&n| e.eval(n)).unwrap_or(V::Bool(true));
        let no = a.get(2).map(|&n| e.eval(n)).unwrap_or(V::Bool(false));
        let (y, n) = (e.to_arr(yes), e.to_arr(no));
        let v = arr
            .v
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let (r, col) = (i as u32 / arr.cols, i as u32 % arr.cols);
                match to_bool(x) {
                    Ok(true) => y.get(r, col).clone(),
                    Ok(false) => n.get(r, col).clone(),
                    Err(err) => V::Err(err),
                }
            })
            .collect();
        return Ok(V::Arr(Rc::new(Arr { rows: arr.rows, cols: arr.cols, v })));
    }
    let c = e.to_scalar(c);
    let t = to_bool(&c)?;
    let branch = if t { a.get(1) } else { a.get(2) };
    match branch {
        Some(&n) if e.is_missing(n) => Ok(V::Num(0.0)),
        Some(&n) => Ok(e.eval(n)),
        None => Ok(V::Bool(t)),
    }
}

fn ifs<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    for p in a.chunks(2) {
        if p.len() < 2 {
            return Err(CellError::Value);
        }
        if e.boolean(p[0])? {
            return Ok(e.eval(p[1]));
        }
    }
    Err(CellError::NA)
}

fn iferror<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = e.eval(a[0]);
    let v = match v {
        V::Ref(r) if r.is_cell() || !e.array => e.deref(r),
        v => v,
    };
    if v.is_err() { Ok(e.eval(a[1])) } else { Ok(v) }
}

fn ifna<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = e.scalar(a[0]);
    if v == V::Err(CellError::NA) { Ok(e.eval(a[1])) } else { Ok(v) }
}

fn logic_fold<'e>(e: &mut Eval<'e>, a: &[u32], mut f: impl FnMut(bool)) -> Res<()> {
    let mut any = false;
    for &n in a {
        match e.eval(n) {
            V::Ref(r) => {
                let mut err = None;
                e.book.each(&r, |_, _, v| {
                    match v {
                        V::Bool(b) => {
                            any = true;
                            f(b)
                        }
                        V::Num(x) => {
                            any = true;
                            f(x != 0.0)
                        }
                        V::Err(x) => {
                            err = Some(x);
                            return false;
                        }
                        _ => {}
                    }
                    true
                });
                if let Some(x) = err {
                    return Err(x);
                }
            }
            V::Arr(arr) => {
                for v in &arr.v {
                    match v {
                        V::Bool(b) => {
                            any = true;
                            f(*b)
                        }
                        V::Num(x) => {
                            any = true;
                            f(*x != 0.0)
                        }
                        V::Err(x) => return Err(*x),
                        _ => {}
                    }
                }
            }
            V::Empty => {}
            v => {
                any = true;
                f(to_bool(&v)?)
            }
        }
    }
    if any { Ok(()) } else { Err(CellError::Value) }
}

fn and<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut r = true;
    logic_fold(e, a, |b| r &= b)?;
    Ok(V::Bool(r))
}

fn or<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut r = false;
    logic_fold(e, a, |b| r |= b)?;
    Ok(V::Bool(r))
}

fn xor<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut r = false;
    logic_fold(e, a, |b| r ^= b)?;
    Ok(V::Bool(r))
}

fn not<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(!e.boolean(a[0])?))
}

fn true_<'e>(_: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(true))
}

fn false_<'e>(_: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(false))
}

fn switch<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = tri(e.scalar(a[0]))?;
    let rest = &a[1..];
    let pairs = rest.len() / 2;
    for k in 0..pairs {
        let v = e.scalar(rest[2 * k]);
        if compare(&x, &v).is_eq() && std::mem::discriminant(&x) == std::mem::discriminant(&v) {
            return Ok(e.eval(rest[2 * k + 1]));
        }
    }
    if rest.len() % 2 == 1 { Ok(e.eval(rest[rest.len() - 1])) } else { Err(CellError::NA) }
}

fn choose<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let k = e.num(a[0])?.trunc() as i64;
    if k < 1 || k as usize >= a.len() {
        return Err(CellError::Value);
    }
    Ok(e.eval(a[k as usize]))
}

// ---------------------------------------------------------------------------
// Informação
// ---------------------------------------------------------------------------

fn isblank<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(matches!(e.scalar(a[0]), V::Empty)))
}

fn iserror<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(e.scalar(a[0]).is_err()))
}

fn iserr<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(matches!(e.scalar(a[0]), V::Err(x) if x != CellError::NA)))
}

fn isna<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(e.scalar(a[0]) == V::Err(CellError::NA)))
}

fn isnumber<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = e.eval(a[0]);
    if e.is_arrayish(&v) {
        let arr = e.to_arr(v);
        let out = arr.v.iter().map(|x| V::Bool(matches!(x, V::Num(_)))).collect();
        return Ok(V::Arr(Rc::new(Arr { rows: arr.rows, cols: arr.cols, v: out })));
    }
    Ok(V::Bool(matches!(e.to_scalar(v), V::Num(_))))
}

fn istext<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(matches!(e.scalar(a[0]), V::Str(_))))
}

fn isnontext<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(!matches!(e.scalar(a[0]), V::Str(_))))
}

fn islogical<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(matches!(e.scalar(a[0]), V::Bool(_))))
}

fn iseven<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool((e.num(a[0])?.trunc() as i64) % 2 == 0))
}

fn isodd<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool((e.num(a[0])?.trunc() as i64) % 2 != 0))
}

fn isref<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Bool(matches!(e.eval(a[0]), V::Ref(_))))
}

fn na<'e>(_: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    Err(CellError::NA)
}

fn error_type<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match e.scalar(a[0]) {
        V::Err(x) => Ok(V::Num(match x {
            CellError::Null => 1.0,
            CellError::Div0 => 2.0,
            CellError::Value => 3.0,
            CellError::Ref => 4.0,
            CellError::Name => 5.0,
            CellError::Num => 6.0,
            CellError::NA => 7.0,
            CellError::GettingData => 8.0,
            CellError::Spill => 9.0,
            CellError::Calc => 14.0,
            CellError::Other => 15.0,
        })),
        _ => Err(CellError::NA),
    }
}

fn type_<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = e.eval(a[0]);
    Ok(V::Num(match v {
        V::Ref(r) if !r.is_cell() => 64.0,
        V::Arr(_) => 64.0,
        v => match e.to_scalar(v) {
            V::Num(_) | V::Empty => 1.0,
            V::Str(_) => 2.0,
            V::Bool(_) => 4.0,
            V::Err(_) => 16.0,
            _ => 64.0,
        },
    }))
}

fn n_<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match e.scalar(a[0]) {
        V::Num(x) => Ok(V::Num(x)),
        V::Bool(b) => Ok(V::Num(b as u8 as f64)),
        V::Err(x) => Err(x),
        _ => Ok(V::Num(0.0)),
    }
}

fn t_<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match e.scalar(a[0]) {
        V::Str(s) => Ok(V::Str(s)),
        V::Err(x) => Err(x),
        _ => Ok(V::text("")),
    }
}

fn row<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match a.first() {
        None => Ok(V::Num(e.row as f64 + 1.0)),
        Some(&n) => match e.eval(n) {
            V::Ref(r) => Ok(V::Num(r.r0 as f64 + 1.0)),
            V::Err(x) => Err(x),
            _ => Err(CellError::Value),
        },
    }
}

fn column<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match a.first() {
        None => Ok(V::Num(e.col as f64 + 1.0)),
        Some(&n) => match e.eval(n) {
            V::Ref(r) => Ok(V::Num(r.c0 as f64 + 1.0)),
            V::Err(x) => Err(x),
            _ => Err(CellError::Value),
        },
    }
}

fn rows<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match e.eval(a[0]) {
        V::Ref(r) => Ok(V::Num(r.rows() as f64)),
        V::Arr(x) => Ok(V::Num(x.rows as f64)),
        V::Err(x) => Err(x),
        _ => Ok(V::Num(1.0)),
    }
}

fn columns<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match e.eval(a[0]) {
        V::Ref(r) => Ok(V::Num(r.cols() as f64)),
        V::Arr(x) => Ok(V::Num(x.cols as f64)),
        V::Err(x) => Err(x),
        _ => Ok(V::Num(1.0)),
    }
}

// ---------------------------------------------------------------------------
// Matemáticas e estatísticas
// ---------------------------------------------------------------------------

/// Soma com compensação (Kahan-Neumaier): somas longas ficam iguais às do Excel.
#[derive(Default)]
struct Acc {
    s: f64,
    c: f64,
}

impl Acc {
    fn add(&mut self, x: f64) {
        let t = self.s + x;
        if self.s.abs() >= x.abs() {
            self.c += (self.s - t) + x;
        } else {
            self.c += (x - t) + self.s;
        }
        self.s = t;
    }
    fn get(&self) -> f64 {
        self.s + self.c
    }
}

fn sum<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut acc = Acc::default();
    nums(e, a, &mut |x| acc.add(x))?;
    Ok(V::num(acc.get()))
}

fn product<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut p = 1.0;
    let mut any = false;
    nums(e, a, &mut |x| {
        p *= x;
        any = true
    })?;
    Ok(V::num(if any { p } else { 0.0 }))
}

fn sumproduct<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let saved = e.array;
    e.array = true;
    let arrs: Vec<Rc<Arr<'e>>> = a
        .iter()
        .map(|&n| {
            let v = e.eval(n);
            e.to_arr(v)
        })
        .collect();
    e.array = saved;
    let (rows, cols) = (arrs[0].rows, arrs[0].cols);
    if arrs.iter().any(|x| x.rows != rows || x.cols != cols) {
        return Err(CellError::Value);
    }
    let mut acc = Acc::default();
    for i in 0..(rows * cols) as usize {
        let mut p = 1.0;
        for x in &arrs {
            match &x.v[i] {
                V::Num(v) => p *= v,
                V::Bool(b) if arrs.len() == 1 => p *= *b as u8 as f64,
                V::Err(err) => return Err(*err),
                _ => p = 0.0,
            }
        }
        acc.add(p);
    }
    Ok(V::num(acc.get()))
}

fn count<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut n = 0u64;
    for &x in a {
        match e.eval(x) {
            V::Ref(r) => e.book.each(&r, |_, _, v| {
                n += matches!(v, V::Num(_)) as u64;
                true
            }),
            V::Arr(arr) => n += arr.v.iter().filter(|v| matches!(v, V::Num(_))).count() as u64,
            V::Num(_) | V::Bool(_) => n += 1,
            V::Str(s) => n += parse_number(&s, e.book.date1904).is_some() as u64,
            _ => {}
        }
    }
    Ok(V::Num(n as f64))
}

fn counta<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut n = 0u64;
    for &x in a {
        if e.is_missing(x) {
            n += 1;
            continue;
        }
        match e.eval(x) {
            V::Ref(r) => e.book.each(&r, |_, _, _| {
                n += 1;
                true
            }),
            V::Arr(arr) => n += arr.v.iter().filter(|v| !matches!(v, V::Empty)).count() as u64,
            _ => n += 1,
        }
    }
    Ok(V::Num(n as f64))
}

fn countblank<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match e.eval(a[0]) {
        V::Ref(r) => {
            let total = r.rows() as u64 * r.cols() as u64;
            let mut filled = 0u64;
            e.book.each(&r, |_, _, v| {
                if !matches!(&v, V::Str(s) if s.is_empty()) {
                    filled += 1;
                }
                true
            });
            Ok(V::Num((total - filled) as f64))
        }
        V::Err(x) => Err(x),
        _ => Err(CellError::Value),
    }
}

fn average<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut acc = Acc::default();
    let mut n = 0u64;
    nums(e, a, &mut |x| {
        acc.add(x);
        n += 1
    })?;
    if n == 0 { Err(CellError::Div0) } else { Ok(V::num(acc.get() / n as f64)) }
}

fn averagea<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut acc = Acc::default();
    let mut n = 0u64;
    let mut err = None;
    each_value(e, a, &mut |v| match v {
        V::Num(x) => {
            acc.add(*x);
            n += 1
        }
        V::Bool(b) => {
            acc.add(*b as u8 as f64);
            n += 1
        }
        V::Str(_) => n += 1,
        V::Err(x) => err = err.or(Some(*x)),
        _ => {}
    });
    if let Some(x) = err {
        return Err(x);
    }
    if n == 0 { Err(CellError::Div0) } else { Ok(V::num(acc.get() / n as f64)) }
}

fn min<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut m = f64::INFINITY;
    nums(e, a, &mut |x| m = m.min(x))?;
    Ok(V::Num(if m.is_finite() { m } else { 0.0 }))
}

fn max<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut m = f64::NEG_INFINITY;
    nums(e, a, &mut |x| m = m.max(x))?;
    Ok(V::Num(if m.is_finite() { m } else { 0.0 }))
}

fn median<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut v = num_list(e, a)?;
    if v.is_empty() {
        return Err(CellError::Num);
    }
    v.sort_by(|x, y| x.total_cmp(y));
    let n = v.len();
    Ok(V::Num(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }))
}

fn kth<'e>(e: &mut Eval<'e>, a: &[u32], largest: bool) -> Res<V<'e>> {
    let mut v = num_list(e, &a[..1])?;
    let k = e.num(a[1])?.ceil() as usize;
    if k == 0 || k > v.len() {
        return Err(CellError::Num);
    }
    v.sort_by(|x, y| if largest { y.total_cmp(x) } else { x.total_cmp(y) });
    Ok(V::Num(v[k - 1]))
}

fn large<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    kth(e, a, true)
}

fn small<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    kth(e, a, false)
}

fn rank<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let v = num_list(e, &a[1..2])?;
    let asc = e.opt_num(a, 2, 0.0)? != 0.0;
    if !v.contains(&x) {
        return Err(CellError::NA);
    }
    let n = v.iter().filter(|&&y| if asc { y < x } else { y > x }).count();
    Ok(V::Num(n as f64 + 1.0))
}

fn variance(v: &[f64], sample: bool) -> Res<f64> {
    let n = v.len() as f64;
    if v.is_empty() || (sample && v.len() < 2) {
        return Err(CellError::Div0);
    }
    let mean = v.iter().sum::<f64>() / n;
    let ss: f64 = v.iter().map(|x| (x - mean) * (x - mean)).sum();
    Ok(ss / if sample { n - 1.0 } else { n })
}

fn stdev_s<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&num_list(e, a)?, true)?.sqrt()))
}

fn stdev_p<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&num_list(e, a)?, false)?.sqrt()))
}

fn var_s<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&num_list(e, a)?, true)?))
}

fn var_p<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(variance(&num_list(e, a)?, false)?))
}

fn subtotal<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let code = e.num(a[0])?.trunc() as i64;
    let base = if code > 100 { code - 100 } else { code };
    let mut vals = Vec::new();
    let mut nonempty = 0u64;
    for &n in &a[1..] {
        match e.eval(n) {
            V::Ref(r) => {
                let sheet = e.book.sheet(r.sheet);
                let mut err = None;
                e.book.each(&r, |row, _, v| {
                    if sheet.is_some_and(|s| s.row_hidden(row)) {
                        return true;
                    }
                    nonempty += 1;
                    match v {
                        V::Num(x) => vals.push(x),
                        V::Err(x) => {
                            err = Some(x);
                            return false;
                        }
                        _ => {}
                    }
                    true
                });
                if let Some(x) = err {
                    return Err(x);
                }
            }
            V::Err(x) => return Err(x),
            _ => return Err(CellError::Value),
        }
    }
    let s = || {
        let mut acc = Acc::default();
        vals.iter().for_each(|&x| acc.add(x));
        acc.get()
    };
    Ok(V::num(match base {
        1 => {
            if vals.is_empty() {
                return Err(CellError::Div0);
            }
            s() / vals.len() as f64
        }
        2 => vals.len() as f64,
        3 => nonempty as f64,
        4 => vals.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(if vals.is_empty() {
            0.0
        } else {
            f64::MIN
        }),
        5 => {
            if vals.is_empty() {
                0.0
            } else {
                vals.iter().copied().fold(f64::INFINITY, f64::min)
            }
        }
        6 => vals.iter().product(),
        7 => variance(&vals, true)?.sqrt(),
        8 => variance(&vals, false)?.sqrt(),
        9 => s(),
        10 => variance(&vals, true)?,
        11 => variance(&vals, false)?,
        _ => return Err(CellError::Value),
    }))
}

fn abs<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(e.num(a[0])?.abs()))
}

fn sign<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    Ok(V::Num(if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }))
}

fn pow10(d: i64) -> f64 {
    10f64.powi(d.clamp(-308, 308) as i32)
}

/// Arredondamento do Excel: meio para longe do zero, sobre o número com 15
/// algarismos (ARRED(2,675;2) = 2,68).
pub fn round_digits(x: f64, d: i64, mode: i8) -> f64 {
    let p = pow10(d);
    let y = round15(x * p);
    let r = match mode {
        0 => y.round(),
        1 => {
            if y >= 0.0 {
                y.ceil()
            } else {
                y.floor()
            }
        }
        _ => y.trunc(),
    };
    let out = r / p;
    if out == 0.0 { 0.0 } else { out }
}

fn round<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let d = e.num(a[1])?.trunc() as i64;
    Ok(V::num(round_digits(x, d, 0)))
}

fn roundup<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let d = e.num(a[1])?.trunc() as i64;
    Ok(V::num(round_digits(x, d, 1)))
}

fn rounddown<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let d = e.num(a[1])?.trunc() as i64;
    Ok(V::num(round_digits(x, d, 2)))
}

fn int<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(e.num(a[0])?.floor()))
}

fn trunc<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let d = e.opt_num(a, 1, 0.0)?.trunc() as i64;
    Ok(V::num(round_digits(x, d, 2)))
}

fn mod_<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let n = e.num(a[0])?;
    let d = e.num(a[1])?;
    if d == 0.0 {
        return Err(CellError::Div0);
    }
    let r = n - d * (n / d).floor();
    Ok(V::num(if r.abs() < 1e-12 * d.abs() { 0.0 } else { r }))
}

fn quotient<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let n = e.num(a[0])?;
    let d = e.num(a[1])?;
    if d == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(V::num((n / d).trunc()))
}

fn power<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.scalar(a[0]);
    let y = e.scalar(a[1]);
    tri(e.scalar_op(parse::BinOp::Pow, x, y))
}

fn sqrt<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    if x < 0.0 { Err(CellError::Num) } else { Ok(V::Num(x.sqrt())) }
}

fn exp<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::num(e.num(a[0])?.exp()))
}

fn ln<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    if x <= 0.0 { Err(CellError::Num) } else { Ok(V::num(x.ln())) }
}

fn log<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let b = e.opt_num(a, 1, 10.0)?;
    if x <= 0.0 || b <= 0.0 || b == 1.0 { Err(CellError::Num) } else { Ok(V::num(x.ln() / b.ln())) }
}

fn log10<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    if x <= 0.0 { Err(CellError::Num) } else { Ok(V::num(x.log10())) }
}

fn pi<'e>(_: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(std::f64::consts::PI))
}

fn to_multiple(x: f64, s: f64, up: bool) -> f64 {
    let q = round15(x / s);
    let k = if up { q.ceil() } else { q.floor() };
    k * s
}

fn ceiling<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let s = e.opt_num(a, 1, if x < 0.0 { -1.0 } else { 1.0 })?;
    if s == 0.0 {
        return Ok(V::Num(0.0));
    }
    if x > 0.0 && s < 0.0 {
        return Err(CellError::Num);
    }
    Ok(V::num(if x < 0.0 && s < 0.0 { -to_multiple(-x, -s, true) } else { to_multiple(x, s, true) }))
}

fn floor<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let s = e.opt_num(a, 1, if x < 0.0 { -1.0 } else { 1.0 })?;
    if s == 0.0 {
        return if x == 0.0 { Ok(V::Num(0.0)) } else { Err(CellError::Div0) };
    }
    if x > 0.0 && s < 0.0 {
        return Err(CellError::Num);
    }
    Ok(V::num(if x < 0.0 && s < 0.0 { -to_multiple(-x, -s, false) } else { to_multiple(x, s, false) }))
}

fn ceiling_math<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let s = e.opt_num(a, 1, 1.0)?.abs();
    let mode = e.opt_num(a, 2, 0.0)?;
    if s == 0.0 {
        return Ok(V::Num(0.0));
    }
    Ok(V::num(if x < 0.0 && mode != 0.0 { -to_multiple(-x, s, true) } else { to_multiple(x, s, true) }))
}

fn floor_math<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let s = e.opt_num(a, 1, 1.0)?.abs();
    let mode = e.opt_num(a, 2, 0.0)?;
    if s == 0.0 {
        return Ok(V::Num(0.0));
    }
    Ok(V::num(if x < 0.0 && mode != 0.0 { -to_multiple(-x, s, false) } else { to_multiple(x, s, false) }))
}

fn mround<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let m = e.num(a[1])?;
    if m == 0.0 {
        return Ok(V::Num(0.0));
    }
    if x * m < 0.0 {
        return Err(CellError::Num);
    }
    Ok(V::num(round15(x / m).round() * m))
}

fn even<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let r = (x.abs() / 2.0).ceil() * 2.0;
    Ok(V::Num(if x < 0.0 { -r } else { r }))
}

fn odd<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let mut r = x.abs().ceil();
    if r % 2.0 == 0.0 {
        r += 1.0;
    }
    Ok(V::Num(if x < 0.0 { -r } else { r }))
}

fn fact<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?.trunc();
    if x < 0.0 {
        return Err(CellError::Num);
    }
    Ok(V::num((1..=x as u64).fold(1.0, |p, k| p * k as f64)))
}

fn rand_u64() -> u64 {
    thread_local!(static S: std::cell::Cell<u64> = std::cell::Cell::new(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1
    ));
    S.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x
    })
}

fn rand<'e>(_: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    Ok(V::Num((rand_u64() >> 11) as f64 / (1u64 << 53) as f64))
}

fn randbetween<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let lo = e.num(a[0])?.ceil();
    let hi = e.num(a[1])?.floor();
    if lo > hi {
        return Err(CellError::Num);
    }
    let span = (hi - lo + 1.0) as u64;
    Ok(V::Num(lo + (rand_u64() % span.max(1)) as f64))
}

// ---------------------------------------------------------------------------
// Critérios (SE com intervalos)
// ---------------------------------------------------------------------------

/// Intervalo de um argumento de critério.
fn range_arg<'e>(e: &mut Eval<'e>, n: u32) -> Res<Rng> {
    match e.eval(n) {
        // Como no Excel: SOMASE/CONT.SE com outro arquivo fechado dão #VALOR!.
        V::Ref(Rng { sheet: crate::value::Sid::Ext(..), .. }) => Err(CellError::Value),
        V::Ref(r) => Ok(r),
        V::Err(x) => Err(x),
        _ => Err(CellError::Value),
    }
}

/// Posições (deslocamentos) de `ranges[0]` que passam em todos os critérios.
/// Todos os intervalos têm o formato de `ranges[0]`. Usa o índice do
/// primeiro critério de igualdade quando o intervalo é uma coluna.
fn matching<'e>(e: &mut Eval<'e>, ranges: &[Rng], crits: &[Crit], mut f: impl FnMut(u32)) {
    let (rows, cols) = (ranges[0].rows(), ranges[0].cols());
    // Linhas além das usadas são vazias em todas as abas envolvidas.
    let used = ranges.iter().map(|r| e.book.used_rows(r.sheet)).max().unwrap_or(0);
    let rows = rows.min(used.saturating_sub(ranges[0].r0));
    if rows == 0 {
        return;
    }
    let at = |e: &Eval<'e>, k: usize, o: u32| {
        let r = &ranges[k];
        e.book.get(r.sheet, r.r0 + o / cols, r.c0 + o % cols)
    };
    let ok_rest = |e: &Eval<'e>, o: u32, skip: usize| {
        crits.iter().enumerate().all(|(k, c)| k == skip || c.matches(&at(e, k, o)))
    };
    // Índice.
    if cols == 1 {
        for (k, c) in crits.iter().enumerate() {
            let Some(key) = c.eq_key() else { continue };
            let r = ranges[k];
            let vec = Rng { r1: r.r0 + rows - 1, ..r };
            if vec.rows() < index::MIN_INDEXED {
                break;
            }
            let idx = e.eng.index.criteria(e.book, &vec);
            let idx = idx.get().unwrap();
            for &o in idx.candidates(&key) {
                if c.matches(&at(e, k, o)) && ok_rest(e, o, k) {
                    f(o);
                }
            }
            return;
        }
    }
    // Varredura: pelas células com valor do primeiro critério, ou por todas
    // as posições se ele casa com vazias.
    if crits[0].matches_blank() {
        for o in 0..rows * cols {
            if ok_rest(e, o, usize::MAX) {
                f(o);
            }
        }
    } else {
        let r0 = ranges[0];
        let first = Rng { r1: r0.r0 + rows - 1, ..r0 };
        let mut hits = Vec::new();
        e.book.each(&first, |row, col, v| {
            if crits[0].matches(&v) {
                hits.push((row - r0.r0) * cols + (col - r0.c0));
            }
            true
        });
        hits.sort_unstable();
        for o in hits {
            if ok_rest(e, o, 0) {
                f(o);
            }
        }
    }
}

fn crit_args<'e>(e: &mut Eval<'e>, pairs: &[u32]) -> Res<(Vec<Rng>, Vec<Crit>)> {
    let mut ranges = Vec::new();
    let mut crits = Vec::new();
    for p in pairs.chunks(2) {
        if p.len() != 2 {
            return Err(CellError::Value);
        }
        let r = range_arg(e, p[0])?;
        let c = e.scalar(p[1]);
        ranges.push(r);
        crits.push(Crit::parse(&c, e.book.date1904));
    }
    let (h, w) = (ranges[0].rows(), ranges[0].cols());
    if ranges.iter().any(|r| r.rows() != h || r.cols() != w) {
        return Err(CellError::Value);
    }
    Ok((ranges, crits))
}

/// Intervalo de valores com o formato de `like` (SOMASE com intervalo de
/// soma menor usa o canto dele).
fn shaped(r: Rng, like: &Rng) -> Rng {
    Rng { r1: r.r0 + like.rows() - 1, c1: r.c0 + like.cols() - 1, ..r }
}

fn agg_ifs<'e>(
    e: &mut Eval<'e>,
    target: Option<Rng>,
    ranges: &[Rng],
    crits: &[Crit],
    mut f: impl FnMut(&V<'e>),
) {
    let cols = ranges[0].cols();
    let book = e.book;
    let mut offs = Vec::new();
    matching(e, ranges, crits, |o| offs.push(o));
    for o in offs {
        if let Some(t) = &target {
            f(&book.get(t.sheet, t.r0 + o / cols, t.c0 + o % cols));
        } else {
            f(&V::Empty);
        }
    }
}

fn sumif<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let r = range_arg(e, a[0])?;
    let c = e.scalar(a[1]);
    let crit = Crit::parse(&c, e.book.date1904);
    let target = if a.len() > 2 && !e.is_missing(a[2]) { shaped(range_arg(e, a[2])?, &r) } else { r };
    let mut acc = Acc::default();
    let mut err = None;
    agg_ifs(e, Some(target), &[r], &[crit], |v| match v {
        V::Num(x) => acc.add(*x),
        V::Err(x) => err = err.or(Some(*x)),
        _ => {}
    });
    match err {
        Some(x) => Err(x),
        None => Ok(V::num(acc.get())),
    }
}

fn sumifs<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let target = range_arg(e, a[0])?;
    let (ranges, crits) = crit_args(e, &a[1..])?;
    if target.rows() != ranges[0].rows() || target.cols() != ranges[0].cols() {
        return Err(CellError::Value);
    }
    let mut acc = Acc::default();
    let mut err = None;
    agg_ifs(e, Some(target), &ranges, &crits, |v| match v {
        V::Num(x) => acc.add(*x),
        V::Err(x) => err = err.or(Some(*x)),
        _ => {}
    });
    match err {
        Some(x) => Err(x),
        None => Ok(V::num(acc.get())),
    }
}

fn countif<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let r = range_arg(e, a[0])?;
    let c = e.scalar(a[1]);
    let crit = Crit::parse(&c, e.book.date1904);
    let mut n = 0u64;
    agg_ifs(e, None, &[r], &[crit], |_| n += 1);
    Ok(V::Num(n as f64))
}

fn countifs<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let (ranges, crits) = crit_args(e, a)?;
    let mut n = 0u64;
    agg_ifs(e, None, &ranges, &crits, |_| n += 1);
    Ok(V::Num(n as f64))
}

fn averageif<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let r = range_arg(e, a[0])?;
    let c = e.scalar(a[1]);
    let crit = Crit::parse(&c, e.book.date1904);
    let target = if a.len() > 2 && !e.is_missing(a[2]) { shaped(range_arg(e, a[2])?, &r) } else { r };
    let (mut acc, mut n) = (Acc::default(), 0u64);
    agg_ifs(e, Some(target), &[r], &[crit], |v| {
        if let V::Num(x) = v {
            acc.add(*x);
            n += 1;
        }
    });
    if n == 0 { Err(CellError::Div0) } else { Ok(V::num(acc.get() / n as f64)) }
}

fn averageifs<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let target = range_arg(e, a[0])?;
    let (ranges, crits) = crit_args(e, &a[1..])?;
    let (mut acc, mut n) = (Acc::default(), 0u64);
    agg_ifs(e, Some(target), &ranges, &crits, |v| {
        if let V::Num(x) = v {
            acc.add(*x);
            n += 1;
        }
    });
    if n == 0 { Err(CellError::Div0) } else { Ok(V::num(acc.get() / n as f64)) }
}

fn minmaxifs<'e>(e: &mut Eval<'e>, a: &[u32], is_max: bool) -> Res<V<'e>> {
    let target = range_arg(e, a[0])?;
    let (ranges, crits) = crit_args(e, &a[1..])?;
    let mut m: Option<f64> = None;
    agg_ifs(e, Some(target), &ranges, &crits, |v| {
        if let V::Num(x) = v {
            m = Some(match m {
                None => *x,
                Some(y) if is_max => y.max(*x),
                Some(y) => y.min(*x),
            });
        }
    });
    Ok(V::Num(m.unwrap_or(0.0)))
}

fn minifs<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    minmaxifs(e, a, false)
}

fn maxifs<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    minmaxifs(e, a, true)
}

// ---------------------------------------------------------------------------
// Texto
// ---------------------------------------------------------------------------

const MAX_TEXT: usize = 32767;

fn concatenate<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut s = String::new();
    for &n in a {
        s.push_str(&e.text(n)?);
    }
    Ok(V::text(s))
}

fn concat<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut s = String::new();
    let mut err = None;
    each_value(e, a, &mut |v| match to_text(v.clone()) {
        Ok(t) => s.push_str(&t),
        Err(x) => err = err.or(Some(x)),
    });
    match err {
        Some(x) => Err(x),
        None => Ok(V::text(s)),
    }
}

fn textjoin<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let d = e.text(a[0])?.into_owned();
    let skip = e.boolean(a[1])?;
    let mut parts: Vec<String> = Vec::new();
    let mut err = None;
    for &n in &a[2..] {
        let mut push = |v: &V| match to_text(v.clone()) {
            Ok(t) => {
                if !(skip && t.is_empty()) {
                    parts.push(t.into_owned())
                }
            }
            Err(x) => err = err.or(Some(x)),
        };
        match e.eval(n) {
            V::Ref(r) => {
                // Vazias também contam quando não são ignoradas.
                for row in r.r0..=r.r1.min(e.book.used_rows(r.sheet).saturating_sub(1).max(r.r0)) {
                    for col in r.c0..=r.c1 {
                        push(&e.book.get(r.sheet, row, col));
                    }
                }
            }
            V::Arr(arr) => arr.v.iter().for_each(&mut push),
            v => push(&v),
        }
    }
    if let Some(x) = err {
        return Err(x);
    }
    Ok(V::text(parts.join(&d)))
}

fn count_arg<'e>(e: &mut Eval<'e>, a: &[u32], i: usize) -> Res<usize> {
    let n = e.opt_num(a, i, 1.0)?;
    if n < 0.0 { Err(CellError::Value) } else { Ok(n as usize) }
}

fn left<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let n = count_arg(e, a, 1)?;
    Ok(V::Str(match s {
        Cow::Borrowed(b) => Cow::Borrowed(chars_take(b, n)),
        Cow::Owned(o) => Cow::Owned(chars_take(&o, n).to_string()),
    }))
}

fn right<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let n = count_arg(e, a, 1)?;
    let len = s.chars().count();
    let skip = len.saturating_sub(n);
    Ok(V::Str(match s {
        Cow::Borrowed(b) => Cow::Borrowed(chars_skip(b, skip)),
        Cow::Owned(o) => Cow::Owned(chars_skip(&o, skip).to_string()),
    }))
}

fn mid<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let start = e.num(a[1])?;
    let n = e.num(a[2])?;
    if start < 1.0 || n < 0.0 {
        return Err(CellError::Value);
    }
    let rest = chars_skip(&s, start as usize - 1);
    Ok(V::text(chars_take(rest, n as usize).to_string()))
}

fn len<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(e.text(a[0])?.chars().count() as f64))
}

fn upper<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    if !s.chars().any(char::is_lowercase) {
        return Ok(V::Str(s));
    }
    Ok(V::text(s.to_uppercase()))
}

fn lower<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    Ok(V::text(s.to_lowercase()))
}

fn proper<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let mut out = String::with_capacity(s.len());
    let mut prev_alpha = false;
    for c in s.chars() {
        if prev_alpha {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_alpha = c.is_alphabetic();
    }
    Ok(V::text(out))
}

fn trim<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    if !s.starts_with(' ') && !s.ends_with(' ') && !s.contains("  ") {
        return Ok(V::Str(s));
    }
    let out: Vec<&str> = s.split(' ').filter(|p| !p.is_empty()).collect();
    Ok(V::text(out.join(" ")))
}

fn clean<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    Ok(V::text(s.chars().filter(|&c| c as u32 >= 32).collect::<String>()))
}

fn substitute<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let old = e.text(a[1])?;
    let new = e.text(a[2])?;
    if old.is_empty() || !s.contains(old.as_ref()) {
        return Ok(V::Str(s));
    }
    match a.get(3) {
        Some(&n) if !e.is_missing(n) => {
            let k = e.num(n)?.trunc() as i64;
            if k < 1 {
                return Err(CellError::Value);
            }
            match s.match_indices(old.as_ref()).nth(k as usize - 1) {
                Some((i, _)) => {
                    let mut out = String::with_capacity(s.len());
                    out.push_str(&s[..i]);
                    out.push_str(&new);
                    out.push_str(&s[i + old.len()..]);
                    Ok(V::text(out))
                }
                None => Ok(V::Str(s)),
            }
        }
        _ => Ok(V::text(s.replace(old.as_ref(), &new))),
    }
}

fn replace<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let start = e.num(a[1])?;
    let n = e.num(a[2])?;
    let new = e.text(a[3])?;
    if start < 1.0 || n < 0.0 {
        return Err(CellError::Value);
    }
    let head = chars_take(&s, start as usize - 1);
    let tail = chars_skip(&s, start as usize - 1 + n as usize);
    Ok(V::text(format!("{head}{new}{tail}")))
}

fn rept<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let n = e.num(a[1])?;
    if n < 0.0 {
        return Err(CellError::Value);
    }
    let n = n as usize;
    if s.chars().count() * n > MAX_TEXT {
        return Err(CellError::Value);
    }
    Ok(V::text(s.repeat(n)))
}

fn char_pos(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

fn find<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let needle = e.text(a[0])?;
    let hay = e.text(a[1])?;
    let start = e.opt_num(a, 2, 1.0)?;
    let len = hay.chars().count();
    if start < 1.0 || start as usize > len + usize::from(needle.is_empty()) {
        return Err(CellError::Value);
    }
    let from = chars_take(&hay, start as usize - 1).len();
    match hay[from..].find(needle.as_ref()) {
        Some(i) => Ok(V::Num(char_pos(&hay, from + i) as f64 + 1.0)),
        None => Err(CellError::Value),
    }
}

fn search<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let needle = e.text(a[0])?;
    let hay = e.text(a[1])?;
    let start = e.opt_num(a, 2, 1.0)?;
    let len = hay.chars().count();
    if start < 1.0 || start as usize > len + usize::from(needle.is_empty()) {
        return Err(CellError::Value);
    }
    let from = start as usize - 1;
    if has_wildcards(&needle) {
        let w = Wild::new(&format!("{needle}*"));
        return match w.find_in(&hay, from) {
            Some(p) => Ok(V::Num(p as f64 + 1.0)),
            None => Err(CellError::Value),
        };
    }
    let h: Vec<char> = hay.chars().flat_map(char::to_lowercase).collect();
    let n: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if n.is_empty() {
        return Ok(V::Num(start));
    }
    for i in from..h.len() {
        if h[i..].starts_with(&n) {
            return Ok(V::Num(i as f64 + 1.0));
        }
    }
    Err(CellError::Value)
}

/// Código de formato digitado em português (TEXTO(A1;"dd/mm/aaaa"),
/// "#.##0,00") → código invariante do arquivo.
pub fn pt_format_code(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut it = code.chars().peekable();
    let lower = code.to_lowercase();
    let has_date = lower.contains('d') || lower.contains('a') || lower.contains('m') || lower.contains('h');
    while let Some(c) = it.next() {
        match c {
            '"' => {
                out.push(c);
                for d in it.by_ref() {
                    out.push(d);
                    if d == '"' {
                        break;
                    }
                }
            }
            '[' => {
                out.push(c);
                for d in it.by_ref() {
                    out.push(d);
                    if d == ']' {
                        break;
                    }
                }
            }
            '\\' => {
                out.push(c);
                if let Some(d) = it.next() {
                    out.push(d);
                }
            }
            '.' => out.push(','),
            ',' => out.push('.'),
            'a' | 'A' if has_date => {
                // "AM/PM" e "A/P" ficam.
                let rest: String =
                    std::iter::once(c).chain(it.clone().take(4)).collect::<String>().to_uppercase();
                if rest.starts_with("AM/PM") {
                    out.push_str("AM/PM");
                    for _ in 0..4 {
                        it.next();
                    }
                } else if rest.starts_with("A/P") {
                    out.push_str("A/P");
                    it.next();
                    it.next();
                } else {
                    out.push('y');
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn text<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let v = tri(e.scalar(a[0]))?;
    let code = e.text(a[1])?;
    thread_local!(static CACHE: RefCell<HashMap<String, Rc<NumFmt>>> = RefCell::new(HashMap::new()));
    let fmt = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(f) = c.get(code.as_ref()) {
            return f.clone();
        }
        if c.len() > 512 {
            c.clear();
        }
        let f = Rc::new(NumFmt::parse(&pt_format_code(&code)));
        c.insert(code.to_string(), f.clone());
        f
    });
    let mut out = String::new();
    match v {
        V::Str(s) => {
            if let Some(x) = parse_number(&s, e.book.date1904) {
                fmt.format_number(x, e.book.date1904, &mut out);
            } else if fmt.format_text(&s, &mut out).is_none() {
                return Ok(V::Str(s));
            }
        }
        v => {
            let x = e.to_num(&v)?;
            let r = fmt.format_number(x, e.book.date1904, &mut out);
            if r.overflow {
                return Err(CellError::Value);
            }
        }
    }
    Ok(V::text(out))
}

fn value<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match e.scalar(a[0]) {
        V::Num(x) => Ok(V::Num(x)),
        V::Empty => Ok(V::Num(0.0)),
        V::Str(s) => parse_number(&s, e.book.date1904).map(V::Num).ok_or(CellError::Value),
        V::Err(x) => Err(x),
        _ => Err(CellError::Value),
    }
}

fn numbervalue<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let dec = match arg_or(e, a, 1) {
        Some(v) => to_text(v)?.chars().next().unwrap_or(','),
        None => ',',
    };
    let grp = match arg_or(e, a, 2) {
        Some(v) => to_text(v)?.chars().next().unwrap_or('.'),
        None => '.',
    };
    let mut t: String = s.chars().filter(|c| !c.is_whitespace() && *c != grp).collect();
    let mut pct = 0;
    while t.ends_with('%') {
        t.pop();
        pct += 1;
    }
    let t = t.replace(dec, ".");
    if t.is_empty() {
        return Ok(V::Num(0.0));
    }
    let x: f64 = t.parse().map_err(|_| CellError::Value)?;
    Ok(V::num(x / 100f64.powi(pct)))
}

fn exact<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.text(a[0])?;
    let y = e.text(a[1])?;
    Ok(V::Bool(x == y))
}

/// Windows-1252 (o "CARACT" do Excel no Windows).
const CP1252: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152,
    0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161,
    0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
];

fn char_<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let n = e.num(a[0])?.trunc() as i64;
    if !(1..=255).contains(&n) {
        return Err(CellError::Value);
    }
    let u = if (0x80..0xA0).contains(&n) { CP1252[n as usize - 0x80] as u32 } else { n as u32 };
    Ok(V::text(char::from_u32(u).unwrap_or('?').to_string()))
}

fn code<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let c = s.chars().next().ok_or(CellError::Value)? as u32;
    let n = match CP1252.iter().position(|&x| x as u32 == c) {
        Some(i) => 0x80 + i as u32,
        None if c < 256 => c,
        None => b'?' as u32,
    };
    Ok(V::Num(n as f64))
}

fn unichar<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let n = e.num(a[0])?.trunc() as u32;
    char::from_u32(n).filter(|_| n > 0).map(|c| V::text(c.to_string())).ok_or(CellError::Value)
}

fn unicode<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    s.chars().next().map(|c| V::Num(c as u32 as f64)).ok_or(CellError::Value)
}

fn fixed<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let d = e.opt_num(a, 1, 2.0)?.trunc() as i64;
    let no_sep = match arg_or(e, a, 2) {
        Some(v) => to_bool(&v)?,
        None => false,
    };
    let r = round_digits(x, d, 0);
    let code = format!(
        "{}{}",
        if no_sep { "0" } else { "#,##0" },
        if d > 0 { format!(".{}", "0".repeat(d as usize)) } else { String::new() }
    );
    let mut out = String::new();
    NumFmt::parse(&code).format_number(r, false, &mut out);
    Ok(V::text(out))
}

fn text_split_at<'e>(e: &mut Eval<'e>, a: &[u32], before: bool) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let d = e.text(a[1])?;
    let n = e.opt_num(a, 2, 1.0)?.trunc() as i64;
    let ci = e.opt_num(a, 3, 0.0)? != 0.0;
    if n == 0 || d.is_empty() {
        return Err(CellError::Value);
    }
    let (hs, ds) = if ci { (s.to_lowercase(), d.to_lowercase()) } else { (s.to_string(), d.to_string()) };
    let pos: Vec<usize> = hs.match_indices(&ds).map(|(i, _)| i).collect();
    let k = if n > 0 {
        pos.get(n as usize - 1)
    } else {
        pos.len().checked_sub((-n) as usize).and_then(|i| pos.get(i))
    };
    match k {
        Some(&i) => {
            if before {
                Ok(V::text(s[..i].to_string()))
            } else {
                Ok(V::text(s[i + d.len()..].to_string()))
            }
        }
        None => match arg_or(e, a, 5) {
            Some(v) => Ok(v),
            None => Err(CellError::NA),
        },
    }
}

fn textbefore<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    text_split_at(e, a, true)
}

fn textafter<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    text_split_at(e, a, false)
}

// ---------------------------------------------------------------------------
// Datas
// ---------------------------------------------------------------------------

/// Serial (dias) de uma data. Mês e dia fora da faixa andam como no Excel.
pub fn date_serial(y: i64, m: i64, d: i64, date1904: bool) -> Res<f64> {
    let y = y + (m - 1).div_euclid(12);
    let m = (m - 1).rem_euclid(12) + 1;
    if !(0..=9999).contains(&y) {
        return Err(CellError::Num);
    }
    let s = duck_core::input::serial(y as i32, m as u32, 1) + (d - 1) as f64;
    let s = if date1904 { s - 1462.0 } else { s };
    if s < 0.0 { Err(CellError::Num) } else { Ok(s) }
}

fn ymd(e: &Eval, x: f64) -> Res<(i64, i64, i64, u32)> {
    if x < 0.0 {
        return Err(CellError::Num);
    }
    let dt = serial_to_datetime(x.floor(), e.book.date1904, 0).ok_or(CellError::Num)?;
    Ok((dt.year as i64, dt.month as i64, dt.day as i64, dt.weekday))
}

fn date_arg<'e>(e: &mut Eval<'e>, n: u32) -> Res<f64> {
    let v = e.scalar(n);
    match v {
        V::Str(s) => parse_number(&s, e.book.date1904).ok_or(CellError::Value),
        v => e.to_num(&v),
    }
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

fn date<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut y = e.num(a[0])?.trunc() as i64;
    let m = e.num(a[1])?.trunc() as i64;
    let d = e.num(a[2])?.trunc() as i64;
    if (0..1900).contains(&y) {
        y += 1900;
    }
    Ok(V::Num(date_serial(y, m, d, e.book.date1904)?))
}

fn time<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let h = e.num(a[0])?.trunc();
    let m = e.num(a[1])?.trunc();
    let s = e.num(a[2])?.trunc();
    let t = (h * 3600.0 + m * 60.0 + s) / 86400.0;
    if t < 0.0 {
        return Err(CellError::Num);
    }
    Ok(V::Num(t.fract()))
}

fn year<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    Ok(V::Num(ymd(e, x)?.0 as f64))
}

fn month<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    Ok(V::Num(ymd(e, x)?.1 as f64))
}

fn day<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    Ok(V::Num(ymd(e, x)?.2 as f64))
}

fn time_part<'e>(e: &mut Eval<'e>, a: &[u32], k: u8) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    if x < 0.0 {
        return Err(CellError::Num);
    }
    let secs = (x.fract() * 86400.0).round() as i64 % 86400;
    Ok(V::Num(match k {
        0 => secs / 3600,
        1 => secs / 60 % 60,
        _ => secs % 60,
    } as f64))
}

fn hour<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    time_part(e, a, 0)
}

fn minute<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    time_part(e, a, 1)
}

fn second<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    time_part(e, a, 2)
}

fn today<'e>(e: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    let t = e.book.now.floor();
    Ok(V::Num(if e.book.date1904 { t - 1462.0 } else { t }))
}

fn now<'e>(e: &mut Eval<'e>, _: &[u32]) -> Res<V<'e>> {
    let t = e.book.now;
    Ok(V::Num(if e.book.date1904 { t - 1462.0 } else { t }))
}

fn weekday<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    let t = e.opt_num(a, 1, 1.0)?.trunc() as i64;
    let w = ymd(e, x)?.3 as i64; // 0 = domingo
    let r = match t {
        1 | 17 => w + 1,
        2 | 11 => (w + 6) % 7 + 1,
        3 => (w + 6) % 7,
        12..=16 => (w - (t - 10)).rem_euclid(7) + 1,
        _ => return Err(CellError::Num),
    };
    Ok(V::Num(r as f64))
}

fn iso_week(e: &Eval, x: f64) -> Res<i64> {
    let (_, _, _, w) = ymd(e, x)?;
    let mon = (w as i64 + 6) % 7; // 0 = segunda
    let thursday = x.floor() - mon as f64 + 3.0;
    let (ty, _, _, _) = ymd(e, thursday)?;
    let jan1 = date_serial(ty, 1, 1, e.book.date1904)?;
    Ok(((thursday - jan1) / 7.0).floor() as i64 + 1)
}

fn weeknum<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    let t = e.opt_num(a, 1, 1.0)?.trunc() as i64;
    if (0.0..1.0).contains(&x) {
        return Ok(V::Num(0.0));
    }
    if t == 21 {
        return Ok(V::Num(iso_week(e, x)? as f64));
    }
    let start = match t {
        1 | 17 => 0,
        2 | 11 => 1,
        12..=16 => t - 10,
        _ => return Err(CellError::Num),
    };
    let (y, _, _, _) = ymd(e, x)?;
    let jan1 = date_serial(y, 1, 1, e.book.date1904)?;
    let (_, _, _, w1) = ymd(e, jan1)?;
    let offset = (w1 as i64 - start).rem_euclid(7);
    let doy = (x.floor() - jan1) as i64;
    Ok(V::Num(((doy + offset) / 7 + 1) as f64))
}

fn isoweeknum<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    Ok(V::Num(iso_week(e, x)? as f64))
}

fn eomonth<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    let k = e.num(a[1])?.trunc() as i64;
    let (y, m, _, _) = ymd(e, x)?;
    Ok(V::Num(date_serial(y, m + k + 1, 1, e.book.date1904)? - 1.0))
}

fn edate<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = date_arg(e, a[0])?;
    let k = e.num(a[1])?.trunc() as i64;
    let (y, m, d, _) = ymd(e, x)?;
    let t = m - 1 + k;
    let (ny, nm) = (y + t.div_euclid(12), t.rem_euclid(12) + 1);
    let d = d.min(days_in_month(ny, nm));
    Ok(V::Num(date_serial(ny, nm, d, e.book.date1904)?))
}

fn datedif<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = date_arg(e, a[0])?.floor();
    let t = date_arg(e, a[1])?.floor();
    let unit = e.text(a[2])?.to_uppercase();
    if s > t {
        return Err(CellError::Num);
    }
    let (y1, m1, d1, _) = ymd(e, s)?;
    let (y2, m2, d2, _) = ymd(e, t)?;
    let months = (y2 - y1) * 12 + (m2 - m1) - i64::from(d2 < d1);
    Ok(V::Num(match unit.as_str() {
        "Y" => (months / 12) as f64,
        "M" => months as f64,
        "D" => t - s,
        "YM" => (months % 12) as f64,
        "MD" => {
            if d2 >= d1 {
                (d2 - d1) as f64
            } else {
                let (py, pm) = if m2 == 1 { (y2 - 1, 12) } else { (y2, m2 - 1) };
                (days_in_month(py, pm) - d1 + d2).max(0) as f64
            }
        }
        "YD" => {
            let mut start = date_serial(y2, m1, d1, e.book.date1904)?;
            if start > t {
                start = date_serial(y2 - 1, m1, d1, e.book.date1904)?;
            }
            t - start
        }
        _ => return Err(CellError::Num),
    }))
}

fn days<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let end = date_arg(e, a[0])?.floor();
    let start = date_arg(e, a[1])?.floor();
    Ok(V::Num(end - start))
}

fn datevalue<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    parse_number(&s, e.book.date1904).map(|x| V::Num(x.floor())).ok_or(CellError::Value)
}

fn datevalue_time<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    parse_number(&s, e.book.date1904).map(|x| V::Num(x.fract())).ok_or(CellError::Value)
}

fn holidays<'e>(e: &mut Eval<'e>, a: &[u32], i: usize) -> Res<Vec<f64>> {
    match a.get(i) {
        Some(&n) if !e.is_missing(n) => {
            let mut v = num_list(e, &[n])?;
            for x in &mut v {
                *x = x.floor();
            }
            Ok(v)
        }
        _ => Ok(Vec::new()),
    }
}

fn is_workday(e: &Eval, x: f64, hol: &[f64]) -> Res<bool> {
    let w = ymd(e, x)?.3;
    Ok(w != 0 && w != 6 && !hol.contains(&x))
}

fn networkdays<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = date_arg(e, a[0])?.floor();
    let t = date_arg(e, a[1])?.floor();
    let hol = holidays(e, a, 2)?;
    let (lo, hi, sign) = if s <= t { (s, t, 1.0) } else { (t, s, -1.0) };
    let mut n = 0.0;
    let mut d = lo;
    while d <= hi {
        if is_workday(e, d, &hol)? {
            n += 1.0;
        }
        d += 1.0;
    }
    Ok(V::Num(n * sign))
}

fn workday<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let mut d = date_arg(e, a[0])?.floor();
    let mut k = e.num(a[1])?.trunc() as i64;
    let hol = holidays(e, a, 2)?;
    let step = if k < 0 { -1.0 } else { 1.0 };
    while k != 0 {
        d += step;
        if is_workday(e, d, &hol)? {
            k -= step as i64;
        }
    }
    Ok(V::Num(d))
}

// ---------------------------------------------------------------------------
// Busca e referência
// ---------------------------------------------------------------------------

/// Vetor (linha ou coluna) de uma referência ou matriz para buscas.
enum Vector<'e> {
    Ref(Rng),
    Arr(Rc<Arr<'e>>),
}

impl<'e> Vector<'e> {
    fn len(&self, e: &Eval<'e>) -> u32 {
        match self {
            Vector::Ref(r) => {
                let r = index::clamp(e.book, r);
                if r.rows() == 1 { r.cols() } else { r.rows() }
            }
            Vector::Arr(a) => a.v.len() as u32,
        }
    }

    fn at(&self, e: &Eval<'e>, o: u32) -> V<'e> {
        match self {
            Vector::Ref(r) => {
                if r.rows() == 1 && r.cols() > 1 {
                    e.book.get(r.sheet, r.r0, r.c0 + o)
                } else {
                    e.book.get(r.sheet, r.r0 + o, r.c0)
                }
            }
            Vector::Arr(a) => a.v.get(o as usize).cloned().unwrap_or(V::Empty),
        }
    }
}

fn vector_arg<'e>(e: &mut Eval<'e>, n: u32) -> Res<Vector<'e>> {
    match e.eval(n) {
        V::Ref(r) => Ok(Vector::Ref(r)),
        V::Arr(a) => Ok(Vector::Arr(a)),
        V::Err(x) => Err(x),
        v => Ok(Vector::Arr(Rc::new(Arr { rows: 1, cols: 1, v: vec![v] }))),
    }
}

/// Busca exata (com curingas se `wild` e o valor é texto com `*`/`?`).
fn exact_find<'e>(e: &Eval<'e>, vec: &Vector<'e>, key: &V<'e>, wild: bool, last: bool) -> Option<u32> {
    if wild
        && let V::Str(s) = key
        && has_wildcards(s)
    {
        let w = Wild::new(s);
        let n = vec.len(e);
        let test = |o: u32| matches!(vec.at(e, o), V::Str(t) if w.matches(&t));
        return if last { (0..n).rev().find(|&o| test(o)) } else { (0..n).find(|&o| test(o)) };
    }
    match vec {
        Vector::Ref(r) => {
            if last {
                index::find_last(e.book, &e.eng.index, r, key)
            } else {
                index::find_first(e.book, &e.eng.index, r, key)
            }
        }
        Vector::Arr(a) => {
            let it = a.v.iter().enumerate();
            if last {
                it.rev().find(|(_, v)| lookup_eq(v, key)).map(|(i, _)| i as u32)
            } else {
                a.v.iter().position(|v| lookup_eq(v, key)).map(|i| i as u32)
            }
        }
    }
}

fn same_kind(a: &V, b: &V) -> bool {
    matches!((a, b), (V::Num(_), V::Num(_)) | (V::Str(_), V::Str(_)) | (V::Bool(_), V::Bool(_)))
}

/// Busca aproximada em ordem crescente: última posição com valor <= `key`.
fn approx_find<'e>(e: &Eval<'e>, vec: &Vector<'e>, key: &V<'e>) -> Option<u32> {
    let n = vec.len(e);
    let (mut lo, mut hi) = (0u32, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let mut m = mid;
        // Vazias no meio: anda até um valor.
        let mut v = vec.at(e, m);
        while matches!(v, V::Empty) && m + 1 < hi {
            m += 1;
            v = vec.at(e, m);
        }
        if matches!(v, V::Empty) {
            hi = mid;
            continue;
        }
        if !same_kind(&v, key) {
            // Tipos diferentes: o Excel compara pela ordem dos tipos.
            if compare(&v, key).is_gt() {
                hi = mid;
            } else {
                lo = m + 1;
            }
            continue;
        }
        if compare(&v, key).is_le() {
            lo = m + 1;
        } else {
            hi = mid;
        }
    }
    let mut p = lo.checked_sub(1)?;
    loop {
        let v = vec.at(e, p);
        if same_kind(&v, key) && compare(&v, key).is_le() {
            return Some(p);
        }
        p = p.checked_sub(1)?;
    }
}

/// Busca aproximada em ordem decrescente: última posição com valor >= `key`.
fn approx_find_desc<'e>(e: &Eval<'e>, vec: &Vector<'e>, key: &V<'e>) -> Option<u32> {
    let n = vec.len(e);
    let mut best = None;
    for o in 0..n {
        let v = vec.at(e, o);
        if matches!(v, V::Empty) {
            continue;
        }
        if same_kind(&v, key) && compare(&v, key).is_ge() {
            best = Some(o);
        } else {
            break;
        }
    }
    best
}

fn vhlookup<'e>(e: &mut Eval<'e>, a: &[u32], vertical: bool) -> Res<V<'e>> {
    let key = tri(e.scalar(a[0]))?;
    let table = e.eval(a[1]);
    let k = e.num(a[2])?.trunc() as i64;
    let approx = match arg_or(e, a, 3) {
        Some(v) => to_bool(&v)?,
        None => true,
    };
    if k < 1 {
        return Err(CellError::Value);
    }
    let k = k as u32 - 1;
    if matches!(key, V::Empty) {
        return Err(CellError::NA);
    }
    match table {
        V::Ref(r) => {
            if (vertical && k >= r.cols()) || (!vertical && k >= r.rows()) {
                return Err(CellError::Ref);
            }
            let vec = if vertical { Rng { c1: r.c0, ..r } } else { Rng { r1: r.r0, ..r } };
            let vec = Vector::Ref(vec);
            let o = if approx { approx_find(e, &vec, &key) } else { exact_find(e, &vec, &key, true, false) };
            let o = o.ok_or(CellError::NA)?;
            Ok(if vertical {
                e.book.get(r.sheet, r.r0 + o, r.c0 + k)
            } else {
                e.book.get(r.sheet, r.r0 + k, r.c0 + o)
            })
        }
        V::Arr(t) => {
            if (vertical && k >= t.cols) || (!vertical && k >= t.rows) {
                return Err(CellError::Ref);
            }
            let first: Vec<V> = if vertical {
                (0..t.rows).map(|i| t.get(i, 0).clone()).collect()
            } else {
                (0..t.cols).map(|i| t.get(0, i).clone()).collect()
            };
            let len = first.len() as u32;
            let vec = Vector::Arr(Rc::new(Arr { rows: len, cols: 1, v: first }));
            let o = if approx { approx_find(e, &vec, &key) } else { exact_find(e, &vec, &key, true, false) };
            let o = o.ok_or(CellError::NA)?;
            Ok(if vertical { t.get(o, k).clone() } else { t.get(k, o).clone() })
        }
        V::Err(x) => Err(x),
        _ => Err(CellError::NA),
    }
}

fn vlookup<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    vhlookup(e, a, true)
}

fn hlookup<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    vhlookup(e, a, false)
}

fn lookup_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let key = tri(e.scalar(a[0]))?;
    let look = vector_arg(e, a[1])?;
    let o = approx_find(e, &look, &key).ok_or(CellError::NA)?;
    if a.len() > 2 {
        let res = vector_arg(e, a[2])?;
        Ok(res.at(e, o))
    } else {
        Ok(look.at(e, o))
    }
}

fn match_<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let key = tri(e.scalar(a[0]))?;
    let vec = vector_arg(e, a[1])?;
    let t = e.opt_num(a, 2, 1.0)?;
    if matches!(key, V::Empty) {
        return Err(CellError::NA);
    }
    let o = if t == 0.0 {
        exact_find(e, &vec, &key, true, false)
    } else if t > 0.0 {
        approx_find(e, &vec, &key)
    } else {
        approx_find_desc(e, &vec, &key)
    };
    o.map(|o| V::Num(o as f64 + 1.0)).ok_or(CellError::NA)
}

/// Busca de PROCX/CORRESPX: modo 0 exato, -1 exato ou menor, 1 exato ou
/// maior, 2 curinga; pesquisa 1 do começo, -1 do fim.
fn x_find<'e>(e: &Eval<'e>, vec: &Vector<'e>, key: &V<'e>, mode: i64, search: i64) -> Option<u32> {
    let last = search == -1 || search == -2;
    match mode {
        0 => exact_find(e, vec, key, false, last),
        2 => exact_find(e, vec, key, true, last),
        -1 | 1 => {
            if let Some(o) = exact_find(e, vec, key, false, last) {
                return Some(o);
            }
            let n = vec.len(e);
            let mut best: Option<(u32, V)> = None;
            for o in 0..n {
                let v = vec.at(e, o);
                if !same_kind(&v, key) {
                    continue;
                }
                let ok = if mode == -1 { compare(&v, key).is_lt() } else { compare(&v, key).is_gt() };
                if !ok {
                    continue;
                }
                let better = match &best {
                    None => true,
                    Some((_, b)) => {
                        if mode == -1 {
                            compare(&v, b).is_gt()
                        } else {
                            compare(&v, b).is_lt()
                        }
                    }
                };
                if better {
                    best = Some((o, v));
                }
            }
            best.map(|b| b.0)
        }
        _ => None,
    }
}

fn xlookup<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let key = tri(e.scalar(a[0]))?;
    let look = e.eval(a[1]);
    let ret = e.eval(a[2]);
    let mode = e.opt_num(a, 4, 0.0)?.trunc() as i64;
    let search = e.opt_num(a, 5, 1.0)?.trunc() as i64;
    let vec = match &look {
        V::Ref(r) => Vector::Ref(*r),
        V::Arr(x) => Vector::Arr(x.clone()),
        V::Err(x) => return Err(*x),
        _ => return Err(CellError::Value),
    };
    let found = x_find(e, &vec, &key, mode, search);
    let Some(o) = found else {
        return match a.get(3) {
            Some(&n) if !e.is_missing(n) => Ok(e.eval(n)),
            _ => Err(CellError::NA),
        };
    };
    let horizontal = match &look {
        V::Ref(r) => r.rows() == 1 && r.cols() > 1,
        V::Arr(x) => x.rows == 1 && x.cols > 1,
        _ => false,
    };
    match ret {
        V::Ref(r) => {
            if horizontal {
                if o >= r.cols() {
                    return Err(CellError::Value);
                }
                Ok(if r.rows() == 1 {
                    e.book.get(r.sheet, r.r0, r.c0 + o)
                } else {
                    V::Ref(Rng { c0: r.c0 + o, c1: r.c0 + o, ..r })
                })
            } else {
                if o >= r.rows() {
                    return Err(CellError::Value);
                }
                Ok(if r.cols() == 1 {
                    e.book.get(r.sheet, r.r0 + o, r.c0)
                } else {
                    V::Ref(Rng { r0: r.r0 + o, r1: r.r0 + o, ..r })
                })
            }
        }
        V::Arr(x) => Ok(if horizontal { x.get(0, o).clone() } else { x.get(o, 0).clone() }),
        V::Err(x) => Err(x),
        v => Ok(v),
    }
}

fn xmatch<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let key = tri(e.scalar(a[0]))?;
    let vec = vector_arg(e, a[1])?;
    let mode = e.opt_num(a, 2, 0.0)?.trunc() as i64;
    let search = e.opt_num(a, 3, 1.0)?.trunc() as i64;
    x_find(e, &vec, &key, mode, search).map(|o| V::Num(o as f64 + 1.0)).ok_or(CellError::NA)
}

fn index<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let src = e.eval(a[0]);
    let r = e.opt_num(a, 1, 0.0)?.trunc() as i64;
    let c = e.opt_num(a, 2, 0.0)?.trunc() as i64;
    if r < 0 || c < 0 {
        return Err(CellError::Value);
    }
    let two = a.len() > 2 && !e.is_missing(a[2]);
    match src {
        V::Ref(rg) => {
            // Vetor de uma linha com um índice só: é a coluna.
            let (r, c) = if rg.rows() == 1 && !two { (1, r) } else { (r, c) };
            let (r, c) = (r as u32, c as u32);
            if r > rg.rows() || c > rg.cols() {
                return Err(CellError::Ref);
            }
            let (r0, r1) = if r == 0 { (rg.r0, rg.r1) } else { (rg.r0 + r - 1, rg.r0 + r - 1) };
            let (c0, c1) = if c == 0 { (rg.c0, rg.c1) } else { (rg.c0 + c - 1, rg.c0 + c - 1) };
            Ok(V::Ref(Rng { sheet: rg.sheet, r0, r1, c0, c1 }))
        }
        V::Arr(x) => {
            let (r, c) = if x.rows == 1 && !two { (1, r) } else { (r, c) };
            let (r, c) = (r as u32, c as u32);
            if r > x.rows || c > x.cols {
                return Err(CellError::Ref);
            }
            match (r, c) {
                (0, 0) => Ok(V::Arr(x)),
                (0, c) => Ok(V::Arr(Rc::new(Arr {
                    rows: x.rows,
                    cols: 1,
                    v: (0..x.rows).map(|i| x.get(i, c - 1).clone()).collect(),
                }))),
                (r, 0) => Ok(V::Arr(Rc::new(Arr {
                    rows: 1,
                    cols: x.cols,
                    v: (0..x.cols).map(|j| x.get(r - 1, j).clone()).collect(),
                }))),
                (r, c) => Ok(x.get(r - 1, c - 1).clone()),
            }
        }
        V::Err(x) => Err(x),
        v => {
            if r <= 1 && c <= 1 {
                Ok(v)
            } else {
                Err(CellError::Ref)
            }
        }
    }
}

fn offset<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let base = match e.eval(a[0]) {
        V::Ref(r) => r,
        V::Err(x) => return Err(x),
        _ => return Err(CellError::Value),
    };
    let dr = e.num(a[1])?.trunc() as i64;
    let dc = e.num(a[2])?.trunc() as i64;
    let h = e.opt_num(a, 3, base.rows() as f64)?.trunc() as i64;
    let w = e.opt_num(a, 4, base.cols() as f64)?.trunc() as i64;
    if h == 0 || w == 0 {
        return Err(CellError::Ref);
    }
    let r0 = base.r0 as i64 + dr;
    let c0 = base.c0 as i64 + dc;
    let (r0, r1) = if h > 0 { (r0, r0 + h - 1) } else { (r0 + h + 1, r0) };
    let (c0, c1) = if w > 0 { (c0, c0 + w - 1) } else { (c0 + w + 1, c0) };
    if r0 < 0 || c0 < 0 || r1 > parse::MAX_ROW as i64 || c1 > parse::MAX_COL as i64 {
        return Err(CellError::Ref);
    }
    Ok(V::Ref(Rng { sheet: base.sheet, r0: r0 as u32, r1: r1 as u32, c0: c0 as u32, c1: c1 as u32 }))
}

fn indirect<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let s = e.text(a[0])?;
    let a1 = match arg_or(e, a, 1) {
        Some(v) => to_bool(&v)?,
        None => true,
    };
    if !a1 {
        e.opaque = true;
        return Err(CellError::Ref);
    }
    let f = parse::compile(s.trim(), e.sheet, e.row, e.col, e.book);
    match f.node(f.root) {
        Node::Area(ar) => e.area(ar).map(V::Ref).ok_or(CellError::Ref),
        _ => Err(CellError::Ref),
    }
}

fn col_letters(mut c: u32) -> String {
    let mut s = Vec::new();
    c += 1;
    while c > 0 {
        s.push(b'A' + ((c - 1) % 26) as u8);
        c = (c - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

fn address<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let r = e.num(a[0])?.trunc() as i64;
    let c = e.num(a[1])?.trunc() as i64;
    let abs = e.opt_num(a, 2, 1.0)?.trunc() as i64;
    let a1 = match arg_or(e, a, 3) {
        Some(v) => to_bool(&v)?,
        None => true,
    };
    if r < 1 || c < 1 {
        return Err(CellError::Value);
    }
    let (ar, ac) = match abs {
        1 => (true, true),
        2 => (true, false),
        3 => (false, true),
        4 => (false, false),
        _ => return Err(CellError::Value),
    };
    let mut s = String::new();
    if let Some(v) = arg_or(e, a, 4) {
        let sh = to_text(v)?;
        if sh.chars().all(|c| c.is_alphanumeric() || c == '_') {
            s.push_str(&sh);
        } else {
            s.push('\'');
            s.push_str(&sh.replace('\'', "''"));
            s.push('\'');
        }
        s.push('!');
    }
    if a1 {
        if ac {
            s.push('$');
        }
        s.push_str(&col_letters(c as u32 - 1));
        if ar {
            s.push('$');
        }
        s.push_str(&r.to_string());
    } else {
        s.push_str(&if ar { format!("L{r}") } else { format!("L[{r}]") });
        s.push_str(&if ac { format!("C{c}") } else { format!("C[{c}]") });
    }
    Ok(V::text(s))
}

fn hyperlink<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    match a.get(1) {
        Some(&n) if !e.is_missing(n) => Ok(e.scalar(n)),
        _ => Ok(e.scalar(a[0])),
    }
}
