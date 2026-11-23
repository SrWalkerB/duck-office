//! Cada função com valores de referência do Excel (os exemplos da
//! documentação da Microsoft; as distribuições também contra o scipy),
//! inclusive erros e células vazias.

use std::cell::RefCell;

use duck_calc::eval::Full;
use duck_calc::{Book, Engine};
use duck_core::{BookExtras, CellError, CellValue, DefinedName, Sheet, StringTable};

/// Valor esperado.
#[derive(Clone, Debug)]
enum W {
    N(f64),
    S(&'static str),
    B(bool),
    E(CellError),
}

use W::{B, E, N, S};

const NUM: W = E(CellError::Num);
const NA: W = E(CellError::NA);
const DIV0: W = E(CellError::Div0);
const VALUE: W = E(CellError::Value);
const REF: W = E(CellError::Ref);
const CALC: W = E(CellError::Calc);

/// Planilha de teste: linhas separadas por `|`, células por `,`. Números,
/// `VERDADEIRO`/`FALSO`, erros (`#N/A`), fórmulas (`=A1+1`, com `~` no lugar
/// da vírgula), textos e vazias.
struct Plan {
    sheets: RefCell<Vec<Sheet>>,
    names: Vec<DefinedName>,
    path: Option<String>,
}

impl Plan {
    fn new(grid: &str) -> Self {
        let mut s = Sheet::new("Plan1");
        s.finish();
        for (r, line) in grid.split('|').enumerate() {
            for (c, t) in line.split(',').enumerate() {
                put(&mut s, r as u32, c as u32, t.trim());
            }
        }
        Self { sheets: RefCell::new(vec![s]), names: Vec::new(), path: None }
    }

    fn name(mut self, name: &str, formula: &str) -> Self {
        self.names.push(DefinedName {
            name: name.into(),
            formula: formula.into(),
            sheet: None,
            hidden: false,
        });
        self
    }

    /// Calcula a fórmula (do arquivo, em inglês) como se digitada em Z50.
    fn full(&self, formula: &str) -> Option<(u32, u32, Vec<CellValue>)> {
        let mut sheets = self.sheets.borrow_mut();
        sheets[0].set_cell(49, 25, CellValue::Empty, Some(formula), None);
        let shared = StringTable::new();
        let extras = BookExtras { names: self.names.clone(), ..Default::default() };
        let mut book = Book::new(sheets.iter().collect(), &shared, &extras, false);
        book.path = self.path.clone();
        let eng = Engine::new(&book);
        match eng.eval_full(&book, 0, 49, 25, true)? {
            Full::One(v) => Some((1, 1, vec![v])),
            Full::Many { rows, cols, v } => Some((rows, cols, v)),
        }
    }

    #[track_caller]
    fn check(&self, formula: &str, want: W) {
        match self.full(formula) {
            Some((1, 1, v)) => same(formula, &v[0], &want),
            other => panic!("{formula}: {other:?}, esperado {want:?}"),
        }
    }

    /// Matriz esperada, linha por linha.
    #[track_caller]
    fn matrix(&self, formula: &str, want: &[&[W]]) {
        let Some((rows, cols, v)) = self.full(formula) else { panic!("{formula}: não calculada") };
        assert_eq!((rows as usize, cols as usize), (want.len(), want[0].len()), "{formula}: {v:?}");
        for (g, w) in v.iter().zip(want.iter().flat_map(|l| l.iter())) {
            same(formula, g, w);
        }
    }

    #[track_caller]
    fn all(&self, cases: &[(&str, W)]) {
        for (f, w) in cases {
            self.check(f, w.clone());
        }
    }
}

fn put(s: &mut Sheet, r: u32, c: u32, t: &str) {
    if t.is_empty() {
        return;
    }
    if let Some(f) = t.strip_prefix('=') {
        s.set_cell(r, c, CellValue::Number(0.0), Some(&f.replace('~', ",")), None);
        return;
    }
    let v = match t {
        "VERDADEIRO" => CellValue::Bool(true),
        "FALSO" => CellValue::Bool(false),
        _ if t.starts_with('#') => CellValue::Error(CellError::parse(t.as_bytes())),
        _ => match t.parse::<f64>() {
            Ok(x) => CellValue::Number(x),
            Err(_) => CellValue::Text(t.into()),
        },
    };
    s.set_cell(r, c, v, None, None);
}

#[track_caller]
fn same(f: &str, got: &CellValue, want: &W) {
    let ok = match (got, want) {
        (CellValue::Number(a), N(b)) => (a - b).abs() <= 1e-9 * b.abs().max(1e-6),
        (CellValue::Text(a), S(b)) => a == b,
        (CellValue::Bool(a), B(b)) => a == b,
        (CellValue::Error(a), E(b)) => a == b,
        _ => false,
    };
    assert!(ok, "{f}: {got:?}, esperado {want:?}");
}

fn vazia() -> Plan {
    Plan::new("")
}

#[test]
fn parcelas() {
    vazia().all(&[
        ("PMT(0.08/12,10,10000)", N(-1037.0320893591606)),
        ("PMT(0.08/12,10,10000,0,1)", N(-1030.164327177974)),
        ("PMT(0.06/12,18*12,0,50000)", N(-129.0811608679973)),
        ("PMT(0,10,1000)", N(-100.0)),
        ("PMT(0.1,0,1000)", NUM),
        ("PMT(\"x\",10,1000)", VALUE),
        ("PV(0.08/12,12*20,500,,0)", N(-59777.145851187)),
        ("PV(0,10,100)", N(-1000.0)),
        ("FV(0.06/12,10,-200,-500,1)", N(2581.4033740601)),
        ("FV(0.12/12,12,-1000)", N(12682.503013196972)),
        ("FV(0.11/12,35,-2000,,1)", N(82846.24637190059)),
        ("FV(0,5,-10,-100)", N(150.0)),
        ("NPER(0.12/12,-100,-1000,10000,1)", N(59.67386567429457)),
        ("NPER(0.12/12,-100,-1000,10000)", N(60.08212285376166)),
        ("NPER(0.12/12,-100,-1000)", N(-9.57859403981317)),
        ("NPER(0,-100,1000)", N(10.0)),
        ("NPER(0,0,1000)", NUM),
        ("RATE(4*12,-200,8000)", N(0.007701472488246)),
        ("RATE(4*12,-200,8000)*12", N(0.09241766985895)),
        ("ROUND(RATE(10,-100,1000),10)", N(0.0)),
        ("RATE(0,-100,1000)", NUM),
        ("RATE(12,100,1000)", NUM),
        ("IPMT(0.1/12,1,3*12,8000)", N(-66.66666666666667)),
        ("IPMT(0.1,3,3,8000)", N(-292.4471299093658)),
        ("IPMT(0.1/12,1,36,8000,0,1)", N(0.0)),
        ("IPMT(0.1,4,3,8000)", NUM),
        ("IPMT(0.1,0,3,8000)", NUM),
        ("PPMT(0.1/12,1,2*12,2000)", N(-75.62318600836664)),
        ("PPMT(0.08,10,10,200000)", N(-27598.053462421354)),
        ("PPMT(0.1/12,2,36,8000,0,1)+IPMT(0.1/12,2,36,8000,0,1)-PMT(0.1/12,36,8000,0,1)", N(0.0)),
        ("CUMIPMT(0.09/12,30*12,125000,13,24,0)", N(-11135.232130750845)),
        ("CUMIPMT(0.09/12,30*12,125000,1,1,0)", N(-937.5)),
        ("CUMIPMT(0,12,1000,1,2,0)", NUM),
        ("CUMIPMT(0.01,12,1000,3,2,0)", NUM),
        ("CUMIPMT(0.01,12,1000,1,2,2)", NUM),
        ("CUMPRINC(0.09/12,30*12,125000,13,24,0)", N(-934.1071234208765)),
        ("CUMPRINC(0.09/12,30*12,125000,1,1,0)", N(-68.27827118097668)),
        ("ISPMT(0.1/12,1,3*12,8000000)", N(-64814.81481481482)),
        ("ISPMT(0.1,1,0,1)", DIV0),
        ("RRI(96,10000,11000)", N(0.0009933073762913)),
        ("RRI(0,1,2)", NUM),
        ("PDURATION(0.025,2000,2200)", N(3.8598661626226786)),
        ("PDURATION(0,1,2)", NUM),
        ("EFFECT(0.0525,4)", N(0.05354266737075841)),
        ("EFFECT(0,4)", NUM),
        ("NOMINAL(0.053543,4)", N(0.052500319868356)),
        ("NOMINAL(0.05,0.5)", NUM),
    ]);
}

#[test]
fn fluxos_de_caixa() {
    let p = Plan::new("-70000,12000,15000,18000,21000,26000,,texto");
    p.all(&[
        ("IRR(A1:E1)", N(-0.02124484827341)),
        ("IRR(A1:F1)", N(0.08663094803653)),
        ("IRR(A1:C1,-0.1)", N(-0.4435069413347)),
        // Vazias e textos do intervalo não contam.
        ("IRR(A1:H1)", N(0.08663094803653)),
        ("IRR(B1:F1)", NUM),
        ("IRR(G1:G1)", NUM),
        ("NPV(0.1,-10000,3000,4200,6800)", N(1188.4434123352207)),
        ("ROUND(NPV(0.08,8000,9200,10000,12000,14500)-40000,6)", N(1922.061555)),
        ("ROUND(NPV(0.08,B1:H1),6)", N(71390.963786)),
        ("NPV(-1,1,2)", DIV0),
        ("NPV(0.1,A2)", VALUE),
        ("MIRR({-120000,39000,30000,21000,37000,46000},0.1,0.12)", N(0.1260941303659)),
        ("MIRR({-120000,39000,30000,21000},0.1,0.12)", N(-0.048044655249981)),
        ("ROUND(MIRR({-120000,39000,30000,21000,37000,46000},0.1,0.14),6)", N(0.134759)),
        ("MIRR({1,2},0.1,0.1)", DIV0),
        ("FVSCHEDULE(1,{0.09,0.11,0.1})", N(1.33089)),
        ("FVSCHEDULE(1,{0.1,\"x\"})", VALUE),
    ]);
    let p = Plan::new("-10000,39448|2750,39508|4250,39751|3250,39859|2750,39904");
    p.all(&[
        ("XNPV(0.09,A1:A5,B1:B5)", N(2086.6476020315366)),
        ("XIRR(A1:A5,B1:B5,0.1)", N(0.3733625335188315)),
        // Com outra estimativa, a mesma taxa.
        ("ROUND(XIRR(A1:A5,B1:B5,0.9),8)", N(0.37336253)),
        ("ROUND(XIRR(A1:A5,B1:B5,-0.5),8)", N(0.37336253)),
        ("XNPV(0.09,A1:A5,B1:B4)", NUM),
        ("XNPV(0.09,A1:A2,{39448,39000})", NUM),
        ("XIRR(A2:A5,B2:B5)", NUM),
    ]);
}

#[test]
fn depreciacao() {
    vazia().all(&[
        ("SLN(30000,7500,10)", N(2250.0)),
        ("SLN(1,1,0)", DIV0),
        ("SYD(30000,7500,10,1)", N(4090.909090909091)),
        ("SYD(30000,7500,10,10)", N(409.0909090909091)),
        ("SYD(30000,7500,10,11)", NUM),
        ("DB(1000000,100000,6,1,7)", N(186083.33333333334)),
        ("DB(1000000,100000,6,2,7)", N(259639.41666666666)),
        ("DB(1000000,100000,6,3,7)", N(176814.44275)),
        ("DB(1000000,100000,6,6,7)", N(55841.75672793751)),
        ("DB(1000000,100000,6,7,7)", N(15845.098471552268)),
        ("DB(1000000,100000,6,7)", NUM),
        ("DB(-1,100000,6,1)", NUM),
        ("DDB(2400,300,10*365,1)", N(1.3150684931506849)),
        ("DDB(2400,300,10*12,1,2)", N(40.0)),
        ("DDB(2400,300,10,1,2)", N(480.0)),
        ("DDB(2400,300,10,2,1.5)", N(306.0)),
        ("DDB(2400,300,10,10)", N(22.1225472)),
        ("DDB(2400,300,10,11)", NUM),
        ("VDB(2400,300,10*365,0,1)", N(1.3150684931506849)),
        ("VDB(2400,300,10*12,0,1)", N(40.0)),
        ("VDB(2400,300,10,0,1)", N(480.0)),
        ("ROUND(VDB(2400,300,10*12,6,18),2)", N(396.31)),
        ("ROUND(VDB(2400,300,10*12,6,18,1.5),2)", N(311.81)),
        ("VDB(2400,300,10,0,0.875,1.5)", N(315.0)),
        ("VDB(2400,300,10,0,11)", NUM),
        ("DOLLARDE(1.02,16)", N(1.125)),
        ("DOLLARDE(1.1,32)", N(1.3125)),
        ("DOLLARDE(1.1,0)", DIV0),
        ("DOLLARDE(1.1,-1)", NUM),
        ("DOLLARFR(1.125,16)", N(1.02)),
        ("DOLLARFR(1.125,32)", N(1.04)),
    ]);
}

#[test]
fn titulos() {
    vazia().all(&[
        ("COUPNUM(DATE(2007,1,25),DATE(2008,11,15),2,1)", N(4.0)),
        ("COUPDAYBS(DATE(2011,1,25),DATE(2011,11,15),2,1)", N(71.0)),
        ("COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),2,1)", N(181.0)),
        ("COUPDAYSNC(DATE(2011,1,25),DATE(2011,11,15),2,1)", N(110.0)),
        ("COUPNCD(DATE(2011,1,25),DATE(2011,11,15),2,1)-DATE(2011,5,15)", N(0.0)),
        ("COUPPCD(DATE(2011,1,25),DATE(2011,11,15),2,1)-DATE(2010,11,15)", N(0.0)),
        ("COUPNUM(DATE(2007,1,25),DATE(2008,11,15),3,1)", NUM),
        ("COUPNUM(DATE(2009,1,25),DATE(2008,11,15),2,1)", NUM),
        ("ROUND(PRICE(DATE(2008,2,15),DATE(2017,11,15),0.0575,0.065,100,2,0),8)", N(94.63436162)),
        ("ROUND(YIELD(DATE(2008,2,15),DATE(2016,11,15),0.0575,95.04287,100,2,0),6)", N(0.065)),
        ("PRICE(DATE(2008,2,15),DATE(2017,11,15),-1,0.065,100,2,0)", NUM),
        ("ROUND(DURATION(DATE(2018,7,1),DATE(2048,1,1),0.08,0.09,2,1),7)", N(10.9191453)),
        ("ROUND(MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1),5)", N(5.73567)),
        ("ROUND(DISC(DATE(2018,7,1),DATE(2048,1,1),97.975,100),8)", N(0.00068644)),
        ("INTRATE(DATE(2008,2,15),DATE(2008,5,15),1000000,1014420,2)", N(0.05768)),
        ("RECEIVED(DATE(2008,2,15),DATE(2008,5,15),1000000,0.0575,2)", N(1014584.654407102)),
        ("PRICEDISC(DATE(2008,2,16),DATE(2008,3,1),0.0525,100,2)", N(99.79583333333333)),
        ("YIELDDISC(DATE(2008,2,16),DATE(2008,3,1),99.795,100,2)", N(0.05282257198685834)),
        ("ROUND(PRICEMAT(DATE(2008,2,15),DATE(2008,4,13),DATE(2007,11,11),0.061,0.061,0),6)", N(99.984499)),
        ("ROUND(YIELDMAT(DATE(2008,3,15),DATE(2008,11,3),DATE(2007,11,8),0.0625,100.0123,0),6)", N(0.060954)),
        ("ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0.1,1000,3)", N(20.547945205479454)),
        ("ROUND(TBILLEQ(DATE(2008,3,31),DATE(2008,6,1),0.0914),6)", N(0.094151)),
        ("TBILLPRICE(DATE(2008,3,31),DATE(2008,6,1),0.09)", N(98.45)),
        ("ROUND(TBILLYIELD(DATE(2008,3,31),DATE(2008,6,1),98.45),6)", N(0.091417)),
        ("TBILLPRICE(DATE(2008,3,31),DATE(2010,6,1),0.09)", NUM),
    ]);
}

#[test]
fn estatisticas() {
    let p = Plan::new("10,,texto,VERDADEIRO,30,20,10,#N/A|,,");
    p.all(&[
        ("MODE({5.6,4,4,3,2,4})", N(4.0)),
        ("MODE.SNGL({3,3,1,1})", N(3.0)),
        ("MODE(A1:G1)", N(10.0)),
        ("MODE({1,2,3})", NA),
        ("MODE(A2:C2)", NA),
        ("PERCENTILE({1,3,2,4},0.3)", N(1.9)),
        ("PERCENTILE.INC(A1:G1,0.5)", N(15.0)),
        ("PERCENTILE.INC({1,2},1.1)", NUM),
        ("PERCENTILE.INC(A2:C2,0.5)", NUM),
        ("PERCENTILE.INC(A1:H1,0.5)", NA),
        ("PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},0.25)", N(2.5)),
        ("PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},0)", NUM),
        ("PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},0.01)", NUM),
        ("PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},2)", NUM),
        ("QUARTILE({1,2,4,7,8,9,10,12},1)", N(3.5)),
        ("QUARTILE.INC({1,2,4,7,8,9,10,12},0)", N(1.0)),
        ("QUARTILE.INC({1,2},5)", NUM),
        ("QUARTILE.EXC({6,7,15,36,39,40,41,42,43,47,49},1)", N(15.0)),
        ("QUARTILE.EXC({6,7,15,36,39,40,41,42,43,47,49},3)", N(43.0)),
        ("QUARTILE.EXC({6,7,15},4)", NUM),
        ("PERCENTRANK({13,12,11,8,4,3,2,1,1,1},2)", N(0.333)),
        ("PERCENTRANK.INC({13,12,11,8,4,3,2,1,1,1},4)", N(0.555)),
        ("PERCENTRANK.INC({13,12,11,8,4,3,2,1,1,1},8)", N(0.666)),
        ("PERCENTRANK.INC({13,12,11,8,4,3,2,1,1,1},5)", N(0.583)),
        ("PERCENTRANK.INC({1,2},3)", NA),
        ("PERCENTRANK.INC({1,2},1,0)", NUM),
        ("PERCENTRANK.EXC({1,2,3,6,6,6,7,8,9},7)", N(0.7)),
        ("PERCENTRANK.EXC({1,2,3,6,6,6,7,8,9},5.43)", N(0.381)),
        ("PERCENTRANK.EXC({1,2,3,6,6,6,7,8,9},5.43,1)", N(0.3)),
        ("GEOMEAN({4,5,8,7,11,4,3})", N(5.476986969656962)),
        ("GEOMEAN({1,0})", NUM),
        ("GEOMEAN(A2:C2)", NUM),
        ("HARMEAN({4,5,8,7,11,4,3})", N(5.028375962061728)),
        ("HARMEAN({1,-1})", NUM),
        ("TRIMMEAN({4,5,6,7,2,3,4,5,1,2,3},0.2)", N(3.7777777777777777)),
        ("TRIMMEAN({1,2},1)", NUM),
        ("AVEDEV({4,5,6,7,5,4,3})", N(1.0204081632653061)),
        ("DEVSQ({4,5,8,7,11,4,3})", N(48.0)),
        ("DEVSQ(A2:C2)", NUM),
        ("SKEW({3,4,5,2,3,4,5,6,4,7})", N(0.3595430714067974)),
        ("SKEW({1,2})", DIV0),
        ("SKEW.P({3,4,5,2,3,4,5,6,4,7})", N(0.30319333935414394)),
        ("KURT({3,4,5,2,3,4,5,6,4,7})", N(-0.15179963720841405)),
        ("KURT({1,1,1,1})", DIV0),
        ("STANDARDIZE(42,40,1.5)", N(4.0 / 3.0)),
        ("STANDARDIZE(42,40,0)", NUM),
        ("RANK.AVG(94,{89,88,92,101,94,97,95})", N(4.0)),
        ("RANK.AVG(1,{1,1,2},1)", N(1.5)),
        ("RANK.AVG(9,{1,1,2})", NA),
        ("MAXA(A1:D1)", N(10.0)),
        ("MAXA(C1:D1)", N(1.0)),
        ("MINA(A1:D1)", N(0.0)),
        ("VARA({1,2,\"x\"})", N(1.0)),
        ("VARPA({1,3})", N(1.0)),
        ("STDEVA({1,3})", N(2f64.sqrt())),
        ("STDEVPA({1,3})", N(1.0)),
        ("STDEVA(A2:A2)", DIV0),
    ]);
    p.matrix("MODE.MULT({1,2,3,4,3,2,1,2,3,5,6,1})", &[&[N(1.0)], &[N(2.0)], &[N(3.0)]]);
    p.matrix(
        "FREQUENCY({79,85,78,85,50,81,95,88,97},{70,79,89})",
        &[&[N(1.0)], &[N(2.0)], &[N(4.0)], &[N(2.0)]],
    );
    // Faixas fora de ordem; textos e vazias não contam.
    p.matrix("FREQUENCY(A1:G1,{20,10})", &[&[N(1.0)], &[N(2.0)], &[N(1.0)]]);
    p.matrix("FREQUENCY(A2:C2,{1})", &[&[N(0.0)], &[N(0.0)]]);
}

#[test]
fn regressao() {
    let p = Plan::new("2,6|3,5|9,11|1,7|8,5|7,4|5,4|,9|texto,1");
    p.all(&[
        ("CORREL({3,2,4,5,6},{9,7,12,15,17})", N(0.9970544855015815)),
        ("PEARSON({3,2,4,5,6},{9,7,12,15,17})", N(0.9970544855015815)),
        ("CORREL({1,2},{1,2,3})", NA),
        ("CORREL({1,1},{1,2})", DIV0),
        ("COVARIANCE.P({3,2,4,5,6},{9,7,12,15,17})", N(5.2)),
        ("COVAR({3,2,4,5,6},{9,7,12,15,17})", N(5.2)),
        ("COVARIANCE.S({2,4,8},{5,11,12})", N(9.666666666666666)),
        ("COVARIANCE.S({2},{5})", DIV0),
        ("SLOPE(A1:A7,B1:B7)", N(0.3055555555555556)),
        // Linhas com texto ou vazia ficam de fora.
        ("SLOPE(A1:A9,B1:B9)", N(0.3055555555555556)),
        ("SLOPE(A8:A9,B8:B9)", DIV0),
        ("INTERCEPT(A1:A5,B1:B5)", N(0.0483870967741939)),
        ("RSQ(A1:A7,B1:B7)", N(0.05795019157088)),
        ("STEYX(A1:A7,B1:B7)", N(3.305718950210041)),
        ("STEYX(A1:A2,B1:B2)", DIV0),
        ("FORECAST(30,{6,7,9,15,21},{20,28,31,38,40})", N(10.607253086419753)),
        ("FORECAST.LINEAR(30,{6,7,9,15,21},{20,28,31,38,40})", N(10.607253086419753)),
        ("FORECAST(1,{1,2},{3,3})", DIV0),
        ("FORECAST(\"x\",{1,2},{3,4})", VALUE),
        ("INDEX(LINEST({1;9;5;7},{0;4;2;3}),1,1)", N(2.0)),
        ("INDEX(LINEST({1;9;5;7},{0;4;2;3}),1,2)", N(1.0)),
        ("SUM(LINEST({1,2,3,4})*{1,0})", N(1.0)),
        ("LINEST(A1:A9,B1:B9)", VALUE),
        ("LINEST({1;2},{1;2;3})", REF),
        ("SUM(TREND({2;4;6},{1;2;3},{4;5})*{1;0})", N(8.0)),
        ("SUM(TREND({2;4;6})*{0;0;1})", N(6.0)),
        ("TREND({2;4;6},{1;2;3},4)", N(8.0)),
        ("ROUND(GROWTH({33100;47300;69000;102000;150000;220000},{11;12;13;14;15;16},17),3)", N(320196.718)),
        ("GROWTH({-1;2},{1;2})", NUM),
    ]);
    p.matrix(
        "LOGEST({33100;47300;69000;102000;150000;220000},{11;12;13;14;15;16})",
        &[&[N(1.4632756281161756), N(495.3047701587239)]],
    );
    // Várias variáveis, com as estatísticas.
    let p = Plan::new(
        "2310,2,2,20,142000|2333,2,2,12,144000|2356,3,1.5,33,151000|2379,3,2,43,150000|2402,2,3,53,139000|\
         2425,4,2,23,169000|2448,2,1.5,99,126000|2471,2,2,34,142900|2494,3,3,23,163000|2517,4,4,55,169000|\
         2540,2,3,22,149000",
    );
    p.matrix(
        "LINEST(E1:E11,A1:D11,TRUE,TRUE)",
        &[
            &[
                N(-234.2371644712075),
                N(2553.210660391566),
                N(12529.768167086753),
                N(27.64138736602013),
                N(52317.830507291816),
            ],
            &[
                N(13.268011475500272),
                N(530.6691519303672),
                N(400.06683819395073),
                N(5.429374041545011),
                N(12237.361602861636),
            ],
            &[N(0.9967479933845101), N(970.5784629285005), NA, NA, NA],
            &[N(459.75367422540273), N(6.0), NA, NA, NA],
            &[N(1732393319.2292686), N(5652135.316203903), NA, NA, NA],
        ],
    );
    p.check(
        "TREND(E1:E11,A1:D11,{2500,3,2,25})",
        N(52317.830507291816
            + 27.64138736602013 * 2500.0
            + 12529.768167086753 * 3.0
            + 2553.210660391566 * 2.0
            - 234.2371644712075 * 25.0),
    );
}

#[test]
fn agregar() {
    let mut p = Plan::new(
        "#DIV/0!,82|72,65|30,95|#NUM!,63|31,53|96,71|32,55|81,83|33,100|53,91|34,89|=SUBTOTAL(9~A1:A11),=AGGREGATE(9~4~B1:B11)",
    );
    p.sheets.get_mut()[0].set_rows_hidden(&[(1, true)]);
    p.all(&[
        ("AGGREGATE(4,6,A1:A11)", N(96.0)),
        ("AGGREGATE(14,6,A1:A11,3)", N(72.0)),
        ("AGGREGATE(15,6,A1:A11)", VALUE),
        ("AGGREGATE(12,6,A1:A11,B1:B11)", N(68.0)),
        ("AGGREGATE(9,4,A1:A11)", DIV0),
        ("AGGREGATE(9,4,B1:B11)", N(847.0)),
        // Sem a linha oculta.
        ("AGGREGATE(9,5,B1:B11)", N(782.0)),
        ("AGGREGATE(9,7,A1:A11)", N(390.0)),
        // Outros SUBTOTAL e AGREGAR não contam (opções 0 a 3) e contam (4 a 7).
        ("AGGREGATE(2,0,B1:B12)", N(11.0)),
        ("AGGREGATE(2,4,B1:B12)", N(12.0)),
        ("AGGREGATE(1,6,A1:A11)", N(462.0 / 9.0)),
        ("AGGREGATE(3,6,A1:A11)", N(9.0)),
        ("AGGREGATE(5,6,A1:A11)", N(30.0)),
        ("AGGREGATE(6,6,A2:A3)", N(2160.0)),
        ("AGGREGATE(16,6,A1:A11,0.5)", N(34.0)),
        ("AGGREGATE(17,6,A1:A11,2)", N(34.0)),
        ("AGGREGATE(18,6,A1:A11,0.5)", N(34.0)),
        ("AGGREGATE(19,6,A1:A11,4)", NUM),
        ("AGGREGATE(13,6,A1:A11)", NA),
        ("AGGREGATE(1,6,C1:C5)", DIV0),
        ("AGGREGATE(20,6,A1:A11)", VALUE),
        ("AGGREGATE(9,8,A1:A11)", VALUE),
        // Conta com condição: o k-ésimo que atende.
        ("AGGREGATE(15,6,ROW(B1:B11)/(B1:B11>90),2)", N(9.0)),
        ("AGGREGATE(14,6,A1:A11/(B1:B11>200),1)", NUM),
    ]);
    p.matrix("AGGREGATE(14,6,A1:A11,{1;2})", &[&[N(96.0)], &[N(81.0)]]);
}

#[test]
fn distribuicoes() {
    vazia().all(&[
        ("NORM.DIST(42,40,1.5,TRUE)", N(0.9087887802741321)),
        ("NORMDIST(42,40,1.5,FALSE)", N(0.10934004978399575)),
        ("NORM.DIST(42,40,0,TRUE)", NUM),
        ("NORM.INV(0.908789,40,1.5)", N(42.00000200956616)),
        ("NORMINV(0.908789,40,1.5)", N(42.00000200956616)),
        ("NORM.INV(1,40,1.5)", NUM),
        ("NORM.S.DIST(1.333333,TRUE)", N(0.9087887256040708)),
        ("NORM.S.DIST(1.333333,FALSE)", N(0.16401014756936722)),
        ("NORMSDIST(0)", N(0.5)),
        ("NORM.S.INV(0.908789)", N(1.3333346730441058)),
        ("NORM.S.INV(0)", NUM),
        ("NORMSINV(0.5)", N(0.0)),
        ("PHI(0.75)", N(0.30113743215480443)),
        ("GAUSS(2)", N(0.4772498680518208)),
        ("LOGNORM.DIST(4,3.5,1.2,TRUE)", N(0.03908355570680047)),
        ("LOGNORMDIST(4,3.5,1.2)", N(0.03908355570680047)),
        ("LOGNORM.DIST(0,3.5,1.2,TRUE)", NUM),
        ("LOGNORM.INV(0.039084,3.5,1.2)", N(4.000025218680635)),
        ("LOGINV(0.039084,3.5,1.2)", N(4.000025218680635)),
        ("EXPON.DIST(0.2,10,TRUE)", N(0.8646647167633873)),
        ("EXPONDIST(0.2,10,FALSE)", N(1.353352832366127)),
        ("EXPON.DIST(-1,10,TRUE)", NUM),
        ("WEIBULL.DIST(105,20,100,TRUE)", N(0.9295813900692769)),
        ("WEIBULL(105,20,100,FALSE)", N(0.035588864024504334)),
        ("WEIBULL(1,0,1,TRUE)", NUM),
        ("GAMMA.DIST(10.00001131,9,2,FALSE)", N(0.03263913041829396)),
        ("GAMMADIST(10.00001131,9,2,TRUE)", N(0.0680940038697872)),
        ("GAMMA.DIST(-1,9,2,TRUE)", NUM),
        ("GAMMA.INV(0.068094,9,2)", N(10.000011191437181)),
        ("GAMMAINV(1,9,2)", NUM),
        ("GAMMA(2.5)", N(1.329340388179137)),
        ("GAMMA(-3.75)", N(0.26786612886141653)),
        ("GAMMA(-2)", NUM),
        ("GAMMALN(4)", N(1.791759469228055)),
        ("GAMMALN.PRECISE(4.5)", N(2.4537365708424423)),
        ("GAMMALN(0)", NUM),
        ("BETA.DIST(2,8,10,TRUE,1,3)", N(0.6854705810546875)),
        ("BETA.DIST(2,8,10,FALSE,1,3)", N(1.4837646484374996)),
        ("BETADIST(2,8,10,1,3)", N(0.6854705810546875)),
        ("BETA.DIST(4,8,10,TRUE,1,3)", NUM),
        ("BETA.INV(0.685470581,8,10,1,3)", N(1.9999999999631428)),
        ("BETAINV(0.685470581,8,10,1,3)", N(1.9999999999631428)),
        ("BETA.INV(0,8,10)", NUM),
        ("CHISQ.DIST(0.5,1,TRUE)", N(0.5204998778130466)),
        ("CHISQ.DIST(2,3,FALSE)", N(0.2075537487102974)),
        ("CHISQ.DIST.RT(18.307,10)", N(0.05000058909139812)),
        ("CHIDIST(-1,10)", NUM),
        ("CHISQ.INV(0.93,1)", N(3.283020286759539)),
        ("CHISQ.INV.RT(0.050001,10)", N(18.30697345696106)),
        ("CHIINV(0.050001,10)", N(18.30697345696106)),
        ("CHISQ.INV(0.5,0)", NUM),
        ("T.DIST(60,1,TRUE)", N(0.9946953263673767)),
        ("T.DIST(8,3,FALSE)", N(0.0007369065209469264)),
        ("T.DIST.RT(1.959999998,60)", N(0.027322464987960437)),
        ("T.DIST.2T(1.959999998,60)", N(0.054644929975920874)),
        ("TDIST(1.959999998,60,2)", N(0.054644929975920874)),
        ("TDIST(1,60,3)", NUM),
        ("T.DIST.2T(-1,60)", NUM),
        ("T.INV(0.75,2)", N(0.8164965809277261)),
        ("T.INV.2T(0.546449,60)", N(0.6065330758257549)),
        ("TINV(0.546449,60)", N(0.6065330758257549)),
        ("T.INV(1,2)", NUM),
        ("F.DIST(15.2068649,6,4,TRUE)", N(1.0 - 0.0099999999524646)),
        ("F.DIST(15.2068649,6,4,FALSE)", N(0.0012237995897796652)),
        ("F.DIST.RT(15.2068649,6,4)", N(0.0099999999524646)),
        ("FDIST(15.2068649,6,4)", N(0.0099999999524646)),
        ("F.INV(0.01,6,4)", N(0.10930991412457852)),
        ("F.INV.RT(0.01,6,4)", N(15.206864861157523)),
        ("FINV(0.01,6,4)", N(15.206864861157523)),
        ("F.DIST(1,0,4,TRUE)", NUM),
        ("BINOM.DIST(6,10,0.5,FALSE)", N(0.205078125)),
        ("BINOMDIST(40,100,0.45,TRUE)", N(0.18305694421446556)),
        ("BINOM.DIST(11,10,0.5,FALSE)", NUM),
        ("BINOM.INV(6,0.5,0.75)", N(4.0)),
        ("CRITBINOM(6,0.5,0.75)", N(4.0)),
        ("BINOM.INV(6,2,0.75)", NUM),
        ("BINOM.DIST.RANGE(60,0.75,48)", N(0.08397496742904746)),
        ("BINOM.DIST.RANGE(60,0.75,45,50)", N(0.5236297934718879)),
        ("BINOM.DIST.RANGE(60,0.75,50,45)", NUM),
        ("NEGBINOM.DIST(10,5,0.25,TRUE)", N(0.3135140584781766)),
        ("NEGBINOMDIST(10,5,0.25)", N(0.05504866037517786)),
        ("NEGBINOM.DIST(-1,5,0.25,TRUE)", NUM),
        ("POISSON.DIST(2,5,TRUE)", N(0.12465201948308108)),
        ("POISSON(2,5,FALSE)", N(0.08422433748856833)),
        ("POISSON.DIST(1000,1010,TRUE)", N(0.3843244154494007)),
        ("POISSON.DIST(-1,5,TRUE)", NUM),
        ("HYPGEOM.DIST(1,4,8,20,TRUE)", N(0.4654282765737874)),
        ("HYPGEOMDIST(1,4,8,20)", N(0.36326109391124867)),
        ("HYPGEOM.DIST(50,200,600,2000,TRUE)", N(0.059535275038662114)),
        ("HYPGEOM.DIST(5,4,8,20,TRUE)", NUM),
        ("CONFIDENCE(0.05,2.5,50)", N(0.6929519121748391)),
        ("CONFIDENCE.NORM(0.05,2.5,50)", N(0.6929519121748391)),
        ("CONFIDENCE.T(0.05,1,50)", N(0.2841968554957299)),
        ("CONFIDENCE.T(0.05,1,1)", DIV0),
        ("CONFIDENCE(0,1,50)", NUM),
        ("FISHER(0.75)", N(0.9729550745276566)),
        ("FISHER(1)", NUM),
        ("FISHERINV(0.972955)", N(0.7499999673941028)),
        ("T.TEST({3,4,5,8,9,1,2,4,5},{6,19,3,2,14,4,5,17,1},2,1)", N(0.19601578492528204)),
        ("TTEST({3,4,5,8,9,1,2,4,5},{6,19,3,2,14,4,5,17,1},1,2)", N(0.19199588676039622 / 2.0)),
        ("T.TEST({3,4,5,8,9,1,2,4,5},{6,19,3,2,14,4,5,17,1},2,3)", N(0.20229392336867796)),
        ("T.TEST({1,2},{1,2,3},2,1)", NA),
        ("T.TEST({1,2},{1,2},3,1)", NUM),
        ("T.TEST({1},{1},2,2)", DIV0),
        ("F.TEST({6,7,9,15,21},{20,28,31,38,40})", N(0.6483178467861745)),
        ("FTEST({1},{2})", DIV0),
        ("Z.TEST({3,6,7,8,6,5,4,2,1,9},4)", N(0.09057419685136386)),
        ("ZTEST({3,6,7,8,6,5,4,2,1,9},4,0)", NUM),
        ("CHISQ.TEST({58,35;11,25;10,23},{45.35,47.65;17.56,18.44;16.09,16.91})", N(0.0003081920170083095)),
        ("CHITEST({1,2},{1,2,3})", NA),
        ("PROB({0,1,2,3},{0.2,0.3,0.1,0.4},2)", N(0.1)),
        ("PROB({0,1,2,3},{0.2,0.3,0.1,0.4},1,3)", N(0.8)),
        ("PROB({0,1},{0.2,0.3},1)", NUM),
    ]);
}

#[test]
fn matematica() {
    let p = Plan::new("24,36,,texto,VERDADEIRO");
    p.all(&[
        ("SIN(RADIANS(30))", N(0.5)),
        ("SIN(PI())", N(0.0)),
        ("SIN(134217728)", NUM),
        ("SIN(\"x\")", VALUE),
        ("SIN(C1)", N(0.0)),
        ("COS(1.047)", N(0.5001710745970701)),
        ("TAN(0.785)", N(0.9992039901050427)),
        ("COT(30)", N(-0.15611995216165922)),
        ("COT(0)", DIV0),
        ("SEC(45)", N(1.9035944074044246)),
        ("CSC(15)", N(1.5377805615408537)),
        ("CSC(0)", DIV0),
        ("ASIN(-0.5)", N(-std::f64::consts::FRAC_PI_6)),
        ("ASIN(2)", NUM),
        ("ACOS(-0.5)", N(2.0943951023931957)),
        ("ACOS(-2)", NUM),
        ("ATAN(1)", N(std::f64::consts::FRAC_PI_4)),
        ("ACOT(2)", N(0.46364760900080615)),
        ("ATAN2(1,1)", N(std::f64::consts::FRAC_PI_4)),
        ("ATAN2(-1,-1)", N(-2.356194490192345)),
        ("ATAN2(0,0)", DIV0),
        ("SINH(1)", N(1.1752011936438014)),
        ("COSH(4)", N(27.308232836016487)),
        ("TANH(0.5)", N(0.46211715726000973)),
        ("COTH(2)", N(1.0373147207275482)),
        ("COTH(0)", DIV0),
        ("SECH(1)", N(0.6480542736638855)),
        ("CSCH(1.5)", N(0.46964244059522464)),
        ("ASINH(-2.5)", N(-1.6472311463710958)),
        ("ACOSH(10)", N(2.993222846126381)),
        ("ACOSH(0.5)", NUM),
        ("ATANH(0.76159416)", N(1.0000000093459483)),
        ("ATANH(1)", NUM),
        ("ACOTH(6)", N(0.16823611831060645)),
        ("ACOTH(1)", NUM),
        ("RADIANS(270)", N(4.71238898038469)),
        ("DEGREES(PI())", N(180.0)),
        ("COMBIN(8,2)", N(28.0)),
        ("COMBIN(100,50)", N(1.0089134454556415e29)),
        ("COMBIN(8.9,2.9)", N(28.0)),
        ("COMBIN(2,3)", NUM),
        ("COMBIN(-1,1)", NUM),
        ("COMBINA(4,3)", N(20.0)),
        ("COMBINA(10,3)", N(220.0)),
        ("COMBINA(0,1)", NUM),
        ("PERMUT(100,3)", N(970200.0)),
        ("PERMUT(3,2)", N(6.0)),
        ("PERMUT(2,3)", NUM),
        ("PERMUTATIONA(3,2)", N(9.0)),
        ("FACTDOUBLE(6)", N(48.0)),
        ("FACTDOUBLE(7)", N(105.0)),
        ("FACTDOUBLE(-2)", NUM),
        ("FACT(171)", NUM),
        ("FACT(1E+300)", NUM),
        ("MULTINOMIAL(2,3,4)", N(1260.0)),
        ("MULTINOMIAL(-1)", NUM),
        ("GCD(24,36)", N(12.0)),
        ("GCD(A1:D1)", N(12.0)),
        ("GCD(5,0)", N(5.0)),
        ("GCD(-1)", NUM),
        ("LCM(24,36)", N(72.0)),
        ("LCM(5,2)", N(10.0)),
        ("LCM(5,0)", N(0.0)),
        ("LCM(-1)", NUM),
        ("SUMSQ(3,4)", N(25.0)),
        ("SUMSQ(A1:E1)", N(24.0 * 24.0 + 36.0 * 36.0)),
        ("SUMSQ(C1:D1)", N(0.0)),
        ("SQRTPI(2)", N(2.5066282746310002)),
        ("SQRTPI(-1)", NUM),
        ("ROUND(SERIESSUM(PI()/4,0,2,{1,-0.5,0.041666667,-0.001388889}),6)", N(0.707103)),
        ("ROMAN(499)", S("CDXCIX")),
        ("ROMAN(499,1)", S("LDVLIV")),
        ("ROMAN(499,2)", S("XDIX")),
        ("ROMAN(499,3)", S("VDIV")),
        ("ROMAN(499,4)", S("ID")),
        ("ROMAN(499,FALSE)", S("ID")),
        ("ROMAN(2026)", S("MMXXVI")),
        ("ROMAN(0)", S("")),
        ("ROMAN(4000)", VALUE),
        ("ROMAN(-1)", VALUE),
        ("ARABIC(\"mcmxii\")", N(1912.0)),
        ("ARABIC(\"-LVII\")", N(-57.0)),
        ("ARABIC(\"\")", N(0.0)),
        ("ARABIC(\"abc\")", VALUE),
        ("BASE(7,2)", S("111")),
        ("BASE(15,2,10)", S("0000001111")),
        ("BASE(100,16)", S("64")),
        ("BASE(-1,2)", NUM),
        ("DECIMAL(\"FF\",16)", N(255.0)),
        ("DECIMAL(111,2)", N(7.0)),
        ("DECIMAL(\"zap\",36)", N(45745.0)),
        ("DECIMAL(\"9\",8)", NUM),
        ("CEILING.PRECISE(-4.1,2)", N(-4.0)),
        ("ISO.CEILING(4.3)", N(5.0)),
        ("FLOOR.PRECISE(-3.2)", N(-4.0)),
        ("SUMX2MY2({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", N(-55.0)),
        ("SUMX2PY2({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", N(521.0)),
        ("SUMXMY2({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", N(79.0)),
        ("SUMXMY2({1,2},{1,2,3})", NA),
        ("MDETERM({1,3,8,5;1,3,6,1;1,1,1,0;7,3,10,2})", N(88.0)),
        ("MDETERM({3,6;1,2})", N(0.0)),
        ("MDETERM({1,2})", VALUE),
        ("MMULT({1,2},{1,2})", VALUE),
        ("MMULT(A1:C1,A1:C1)", VALUE),
        ("MINVERSE({1,2;2,4})", NUM),
        ("MINVERSE({1,\"x\";2,4})", VALUE),
    ]);
    p.matrix("MMULT({1,3;7,2},{2,0;0,2})", &[&[N(2.0), N(6.0)], &[N(14.0), N(4.0)]]);
    p.matrix("MINVERSE({4,-1;2,0})", &[&[N(0.0), N(0.5)], &[N(-1.0), N(2.0)]]);
    p.matrix("MUNIT(2)", &[&[N(1.0), N(0.0)], &[N(0.0), N(1.0)]]);
}

#[test]
fn engenharia() {
    vazia().all(&[
        ("CONVERT(1,\"lbm\",\"kg\")", N(0.45359237)),
        ("CONVERT(68,\"F\",\"C\")", N(20.0)),
        ("CONVERT(0,\"C\",\"K\")", N(273.15)),
        ("CONVERT(2.5,\"ft\",\"sec\")", NA),
        ("CONVERT(1,\"xx\",\"m\")", NA),
        ("CONVERT(CONVERT(100,\"ft\",\"m\"),\"ft\",\"m\")", N(9.290304)),
        ("CONVERT(1,\"km\",\"mi\")", N(0.621371192237334)),
        ("CONVERT(1,\"km2\",\"m2\")", N(1e6)),
        ("CONVERT(1,\"gal\",\"l\")", N(3.785411784)),
        ("CONVERT(1,\"kibyte\",\"byte\")", N(1024.0)),
        ("CONVERT(1,\"HP\",\"W\")", N(745.6998715822702)),
        ("CONVERT(1,\"hr\",\"mn\")", N(60.0)),
        ("CONVERT(1,\"atm\",\"Pa\")", N(101325.0)),
        ("CONVERT(1,\"cal\",\"J\")", N(4.1868)),
        ("CONVERT(100,\"mph\",\"m/s\")", N(44.704)),
        ("BIN2DEC(1100100)", N(100.0)),
        ("BIN2DEC(1111111111)", N(-1.0)),
        ("BIN2DEC(\"102\")", NUM),
        ("BIN2DEC(\"11111111111\")", NUM),
        ("BIN2HEX(11111011,4)", S("00FB")),
        ("BIN2OCT(1001,3)", S("011")),
        ("DEC2BIN(9,4)", S("1001")),
        ("DEC2BIN(-100)", S("1110011100")),
        ("DEC2BIN(512)", NUM),
        ("DEC2BIN(9,2)", NUM),
        ("DEC2HEX(100,4)", S("0064")),
        ("DEC2HEX(-54)", S("FFFFFFFFCA")),
        ("DEC2HEX(64,1)", NUM),
        ("DEC2OCT(58,3)", S("072")),
        ("HEX2BIN(\"F\",8)", S("00001111")),
        ("HEX2DEC(\"A5\")", N(165.0)),
        ("HEX2DEC(\"FFFFFFFF5B\")", N(-165.0)),
        ("HEX2OCT(\"3B4E\")", S("35516")),
        ("OCT2BIN(3,3)", S("011")),
        ("OCT2DEC(54)", N(44.0)),
        ("OCT2HEX(100,4)", S("0040")),
        ("BITAND(13,25)", N(9.0)),
        ("BITOR(23,10)", N(31.0)),
        ("BITXOR(5,3)", N(6.0)),
        ("BITAND(-1,1)", NUM),
        ("BITAND(1.5,1)", NUM),
        ("BITLSHIFT(4,2)", N(16.0)),
        ("BITRSHIFT(13,2)", N(3.0)),
        ("BITLSHIFT(1,60)", NUM),
        ("DELTA(5,5)", N(1.0)),
        ("DELTA(5)", N(0.0)),
        ("GESTEP(5,4)", N(1.0)),
        ("GESTEP(-1)", N(0.0)),
        ("ERF(0.745)", N(0.7079289200957376)),
        ("ERF(1,1.5)", N(0.12340435352559598)),
        ("ERF.PRECISE(1)", N(0.8427007929497149)),
        ("ERFC(1)", N(0.15729920705028513)),
        ("ERFC.PRECISE(1)", N(0.15729920705028513)),
    ]);
}

#[test]
fn datas() {
    let p = Plan::new("38719,38733,,40909");
    p.all(&[
        ("YEARFRAC(DATE(2012,1,1),DATE(2012,7,30))", N(0.5805555555555556)),
        ("YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),1)", N(0.5765027322404371)),
        ("YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),2)", N(211.0 / 360.0)),
        ("YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),3)", N(0.5780821917808219)),
        ("YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),4)", N(209.0 / 360.0)),
        ("YEARFRAC(DATE(2012,7,30),DATE(2012,1,1))", N(0.5805555555555556)),
        ("YEARFRAC(DATE(2010,6,15),DATE(2014,6,15),1)", N(1461.0 / 365.2)),
        ("YEARFRAC(DATE(2011,2,28),DATE(2012,2,29))", N(1.0)),
        ("YEARFRAC(DATE(2012,1,1),DATE(2012,1,1))", N(0.0)),
        ("YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),5)", NUM),
        ("YEARFRAC(\"x\",1)", VALUE),
        ("DAYS360(DATE(2011,1,30),DATE(2011,2,1))", N(1.0)),
        ("DAYS360(DATE(2011,1,1),DATE(2011,12,31))", N(360.0)),
        ("DAYS360(DATE(2011,1,1),DATE(2011,2,1))", N(30.0)),
        ("DAYS360(DATE(2011,1,1),DATE(2011,12,31),TRUE)", N(359.0)),
        ("DAYS360(DATE(2011,12,31),DATE(2011,1,1))", N(-359.0)),
        ("NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31))", N(22.0)),
        ("NETWORKDAYS.INTL(DATE(2006,2,28),DATE(2006,1,31))", N(-21.0)),
        ("NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),7,A1:B1)", N(22.0)),
        ("NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),\"0010001\",A1:B1)", N(20.0)),
        ("NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),\"1111111\")", N(0.0)),
        ("NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),8)", NUM),
        ("NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),\"12\")", VALUE),
        ("NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31),1,C1:C1)", N(22.0)),
        ("WORKDAY.INTL(DATE(2012,1,1),30,0)", NUM),
        ("WORKDAY.INTL(DATE(2012,1,1),90,11)", N(41013.0)),
        ("WORKDAY.INTL(DATE(2012,1,1),30,17)", N(40944.0)),
        ("WORKDAY.INTL(DATE(2012,1,10),-5)-DATE(2012,1,3)", N(0.0)),
        ("WORKDAY.INTL(DATE(2012,1,1),1,\"1111111\")", VALUE),
    ]);
}

#[test]
fn texto() {
    let p = Plan::new("um dois;três,,a1b2c3");
    p.matrix("TEXTSPLIT(\"um,dois,três\",\",\")", &[&[S("um"), S("dois"), S("três")]]);
    p.matrix("TEXTSPLIT(\"um;dois\",,\";\")", &[&[S("um")], &[S("dois")]]);
    p.matrix("TEXTSPLIT(\"a=1;b\",\"=\",\";\")", &[&[S("a"), S("1")], &[S("b"), NA]]);
    p.matrix("TEXTSPLIT(\"a=1;b\",\"=\",\";\",,,\"-\")", &[&[S("a"), S("1")], &[S("b"), S("-")]]);
    p.matrix("TEXTSPLIT(\"a,,b\",\",\")", &[&[S("a"), S(""), S("b")]]);
    p.matrix("TEXTSPLIT(\"a,,b\",\",\",,TRUE)", &[&[S("a"), S("b")]]);
    p.matrix("TEXTSPLIT(\"a.b,c\",{\".\",\",\"})", &[&[S("a"), S("b"), S("c")]]);
    p.matrix("TEXTSPLIT(\"aXbxc\",\"x\",,,1)", &[&[S("a"), S("b"), S("c")]]);
    p.matrix("TEXTSPLIT(\"aXbxc\",\"x\")", &[&[S("aXb"), S("c")]]);
    p.matrix("VALUE(TEXTSPLIT(\"1,2\",\",\"))*2", &[&[N(2.0), N(4.0)]]);
    p.all(&[
        ("TEXTSPLIT(\"abc\",\",\")", S("abc")),
        ("TEXTSPLIT(\"abc\",\"\")", VALUE),
        ("TEXTSPLIT(\",\",\",\",,TRUE)", CALC),
        ("TEXTSPLIT(#N/A,\",\")", NA),
        ("INDEX(TEXTSPLIT(A1,\" \",\";\"),1,2)", S("dois")),
        ("DOLLAR(1234.567,2)", S("R$ 1.234,57")),
        ("DOLLAR(1234.567,-2)", S("R$ 1.200")),
        ("DOLLAR(-1234.567,-2)", S("-R$ 1.200")),
        ("DOLLAR(-0.123,4)", S("-R$ 0,1230")),
        ("DOLLAR(99.888)", S("R$ 99,89")),
        ("DOLLAR(B1)", S("R$ 0,00")),
        ("DOLLAR(\"x\")", VALUE),
    ]);
}

#[test]
fn informacao() {
    let mut p = Plan::new("=SUM(B1:B3),10|texto,20||,");
    let mut other = Sheet::new("Vendas 2026");
    other.finish();
    p.sheets.get_mut().push(other);
    p.all(&[
        ("FORMULATEXT(A1)", S("=SOMA(B1:B3)")),
        ("FORMULATEXT(B1)", NA),
        ("FORMULATEXT(1)", NA),
        ("ISFORMULA(A1)", B(true)),
        ("ISFORMULA(B1)", B(false)),
        ("ISFORMULA(C9)", B(false)),
        ("ISFORMULA(\"x\")", VALUE),
        ("SHEET()", N(1.0)),
        ("SHEET('Vendas 2026'!A1)", N(2.0)),
        ("SHEET(\"vendas 2026\")", N(2.0)),
        ("SHEET(\"não tem\")", NA),
        ("SHEETS()", N(2.0)),
        ("SHEETS(A1:B2)", N(1.0)),
        ("CELL(\"address\",B2)", S("$B$2")),
        ("CELL(\"endereço\",'Vendas 2026'!C4)", S("'Vendas 2026'!$C$4")),
        ("CELL(\"col\",B2)", N(2.0)),
        ("CELL(\"lin\",B2:D9)", N(2.0)),
        ("CELL(\"row\")", N(50.0)),
        ("CELL(\"contents\",A2)", S("texto")),
        ("CELL(\"conteúdo\",C1)", N(0.0)),
        ("CELL(\"type\",C1)", S("b")),
        ("CELL(\"type\",A2)", S("l")),
        ("CELL(\"tipo\",B1)", S("v")),
        ("CELL(\"xyz\",B1)", VALUE),
        ("INDEX(CELL(\"width\",B1),1,1)", N(8.0)),
        ("INFO(\"numfile\")", N(2.0)),
        ("INFO(\"system\")", S(if cfg!(target_os = "macos") { "mac" } else { "pcdos" })),
        ("INFO(\"recalc\")", S("Automático")),
        ("INFO(\"release\")", S("16.0")),
        ("INFO(\"xyz\")", VALUE),
    ]);
    // Formatação e arquivo desconhecido: o valor guardado fica.
    assert!(p.full("CELL(\"color\",B1)").is_none());
    assert!(p.full("CELL(\"filename\",B1)").is_none());
    p.path = Some("C:\\Dados\\vendas.xlsx".into());
    p.check("CELL(\"filename\",B1)", S("C:\\Dados\\[vendas.xlsx]Plan1"));
    p.check("INFO(\"directory\")", S("C:\\Dados\\"));
    p.path = Some(String::new());
    p.check("CELL(\"nome.arquivo\")", S(""));
}

#[test]
fn lambda_e_familia() {
    let p = Plan::new("1,2,3|4,5,6|,texto,#N/A")
        .name("Dobro", "_xlfn.LAMBDA(_xlpm.x,_xlpm.x*2)")
        .name("Fat", "_xlfn.LAMBDA(_xlpm.n,IF(_xlpm.n<2,1,_xlpm.n*Fat(_xlpm.n-1)))")
        .name("Tenta", "_xlfn.LAMBDA(_xlpm.n,IFERROR(1/(_xlpm.n-3),Tenta(_xlpm.n+2)))")
        .name("Fundo", "_xlfn.LAMBDA(_xlpm.n,IF(_xlpm.n<1,0,_xlpm.n+Fundo(_xlpm.n-1)))")
        .name(
            "Aninhada",
            "_xlfn.LAMBDA(_xlpm.n,ABS(ABS(ABS(ABS(ABS(ABS(ABS(ABS(ABS(ABS(ABS(ABS(ABS(ABS(ABS(Aninhada(_xlpm.n+1)))))))))))))))))",
        )
        .name("Sem", "_xlfn.LAMBDA(_xlpm.n,Sem(_xlpm.n+1))");
    p.all(&[
        ("LAMBDA(x,x*2)(21)", N(42.0)),
        ("LAMBDA(x,y,x-y)(5,3)", N(2.0)),
        ("LAMBDA(7)()", N(7.0)),
        ("LAMBDA(x,LAMBDA(y,x*10+y))(1)(2)", N(12.0)),
        ("LAMBDA(x,[y],IF(ISOMITTED(y),x,x+y))(1)", N(1.0)),
        ("LAMBDA(x,[y],IF(ISOMITTED(y),x,x+y))(1,5)", N(6.0)),
        ("LAMBDA(x,_xlop.y,x+_xlpm.y)(1,)", N(1.0)),
        ("LAMBDA(x,x)", CALC),
        ("LAMBDA(x,x)()", VALUE),
        ("LAMBDA(x,x)(1,2)", VALUE),
        ("LAMBDA(x,x/0)(1)", DIV0),
        ("LET(f,LAMBDA(x,x+1),f(f(1)))", N(3.0)),
        ("LET(k,10,f,LAMBDA(x,x*k),f(2))", N(20.0)),
        // A variável de fora continua a mesma dentro da função.
        ("LET(x,1,f,LAMBDA(y,x+y),LET(x,100,f(x)))", N(101.0)),
        ("LET(x,LET(y,2,y*3),x+1)", N(7.0)),
        ("LET(x,5,LET(y,x*2,LET(z,y+x,z)))", N(15.0)),
        ("Dobro(4)", N(8.0)),
        ("Dobro(Dobro(4))", N(16.0)),
        ("Fat(5)", N(120.0)),
        ("Fat(20)", N(2432902008176640000.0)),
        ("Sem(1)", NUM),
        // Recursão que termina, e com muitas funções aninhadas em cada passo.
        ("Fundo(100)", N(5050.0)),
        ("Aninhada(1)", NUM),
        // SEERRO não calcula a alternativa se não precisa.
        ("Tenta(3)", N(0.5)),
        ("SUM(MAP(A1:C2,LAMBDA(v,v*v)))", N(91.0)),
        ("SUM(MAP(A1:C1,A2:C2,LAMBDA(a,b,a*b)))", N(32.0)),
        ("MAP(A1:B1,5)", VALUE),
        ("REDUCE(0,A1:C2,LAMBDA(t,v,t+v))", N(21.0)),
        ("REDUCE(100,A1:C1,LAMBDA(t,v,t-v))", N(94.0)),
        ("REDUCE(,A1:C2,LAMBDA(t,v,t*v))", N(720.0)),
        ("REDUCE(\"\",A1:C1,LAMBDA(t,v,t&v))", S("123")),
        ("REDUCE(0,A1:C3,LAMBDA(t,v,t+v))", VALUE),
        ("SUM(BYROW(A1:C2,_xleta.SUM))", N(21.0)),
        ("MAX(BYROW(A1:C2,LAMBDA(l,MAX(l)-MIN(l))))", N(2.0)),
        ("LET(f,_xleta.SUM,SUM(BYROW(A1:C2,f)))", N(21.0)),
        // Sem o prefixo do arquivo, é um nome que não existe.
        ("BYROW(A1:C2,SUM)", E(CellError::Name)),
        ("ISOMITTED(1)", B(false)),
        ("MAKEARRAY(0,1,LAMBDA(r,c,1))", VALUE),
        ("MAKEARRAY(2,2,3)", VALUE),
    ]);
    // Função que não existe: o motor não calcula.
    assert!(p.full("NaoExiste(1)").is_none());
    p.matrix("MAP(A1:C1,LAMBDA(v,v*10))", &[&[N(10.0), N(20.0), N(30.0)]]);
    p.matrix("MAP(A3:C3,LAMBDA(v,IFERROR(v&\"!\",\"erro\")))", &[&[S("!"), S("texto!"), S("erro")]]);
    p.matrix("MAP(A1:C1,A2:B2,LAMBDA(a,b,a+b))", &[&[N(5.0), N(7.0), NA]]);
    p.matrix("MAP(A1:B1,_xleta.ABS)", &[&[N(1.0), N(2.0)]]);
    p.matrix("SCAN(0,A1:C2,LAMBDA(t,v,t+v))", &[&[N(1.0), N(3.0), N(6.0)], &[N(10.0), N(15.0), N(21.0)]]);
    p.matrix("SCAN(,A1:C1,LAMBDA(t,v,t*v))", &[&[N(1.0), N(2.0), N(6.0)]]);
    p.matrix("BYROW(A1:C2,_xleta.SUM)", &[&[N(6.0)], &[N(15.0)]]);
    p.matrix("BYCOL(A1:C2,LAMBDA(c,MAX(c)))", &[&[N(4.0), N(5.0), N(6.0)]]);
    p.matrix("BYROW(A1:C2,LAMBDA(l,l))", &[&[CALC], &[CALC]]);
    p.matrix(
        "MAKEARRAY(2,3,LAMBDA(r,c,r*10+c))",
        &[&[N(11.0), N(12.0), N(13.0)], &[N(21.0), N(22.0), N(23.0)]],
    );
    p.matrix("BYROW({1,2;3,4}*2,LAMBDA(l,SUM(l*l)))", &[&[N(20.0)], &[N(100.0)]]);
    p.matrix("WRAPROWS({1,2,3,4,5},2)", &[&[N(1.0), N(2.0)], &[N(3.0), N(4.0)], &[N(5.0), NA]]);
    p.matrix("WRAPCOLS({1,2,3,4,5},2,0)", &[&[N(1.0), N(3.0), N(5.0)], &[N(2.0), N(4.0), N(0.0)]]);
    p.check("WRAPROWS({1,2;3,4},2)", VALUE);
    p.check("WRAPROWS({1,2},0)", NUM);
}

#[test]
fn itens_de_matriz() {
    // Funções escalares aplicadas a cada item.
    let p = Plan::new("-1,a,45000|2,b,45040|-3,a,45100|,c,");
    p.all(&[
        ("SUM(ABS(A1:A3))", N(6.0)),
        ("SUMPRODUCT((MONTH(C1:C3)=4)*1)", N(1.0)),
        ("SUM(1/COUNTIF(B1:B3,B1:B3))", N(2.0)),
        ("SUM(SUMIF(B1:B3,{\"a\",\"b\"},A1:A3))", N(-2.0)),
        ("SUM(LARGE(A1:A3,{1,2}))", N(1.0)),
        ("SUM(IFERROR(A1:A3/{1;0;1},100))", N(96.0)),
        ("SUM(LEN(B1:B4))", N(4.0)),
        ("SUM(VLOOKUP({\"a\",\"b\"},CHOOSE({1,2},B1:B3,A1:A3),2,FALSE))", N(1.0)),
        ("SUM(ROW(A1:A3))", N(6.0)),
        ("SUM(COLUMN(A1:C1))", N(6.0)),
        ("SUMPRODUCT(ROW(INDIRECT(\"1:4\")))", N(10.0)),
        ("SUM(SIN(A4:A4))", N(0.0)),
        ("ABS(A1)", N(1.0)),
        ("SUM(IFS(A1:A3>0,10,A1:A3<-2,100,TRUE,1))", N(111.0)),
        ("SUM(SWITCH(B1:B3,\"a\",1,\"b\",10))", N(12.0)),
        ("SUM(IFNA(MATCH(B1:B4,{\"b\",\"c\"},0),0))", N(3.0)),
    ]);
    p.matrix("ROUND({1.234;5.678},1)", &[&[N(1.2)], &[N(5.7)]]);
    p.matrix("ROUND(1.2345,{1,2})", &[&[N(1.2), N(1.23)]]);
    p.matrix("IF({TRUE,FALSE},A1:A2,0)", &[&[N(-1.0), N(0.0)], &[N(2.0), N(0.0)]]);
}

#[test]
fn agrupar() {
    let p = Plan::new("Reg,Prod,Ano,Valor|S,X,1,10|N,X,1,5|S,Y,2,7|N,Y,1,3|s,X,2,20|N,X,2,1");
    p.matrix(
        "GROUPBY(A2:A7,D2:D7,_xleta.SUM)",
        &[&[S("N"), N(9.0)], &[S("S"), N(37.0)], &[S("Total"), N(46.0)]],
    );
    // Com o título: some (um nível só) ou aparece (3).
    p.matrix(
        "GROUPBY(A1:A7,D1:D7,_xleta.SUM,3,0)",
        &[&[S("Reg"), S("Valor")], &[S("N"), N(9.0)], &[S("S"), N(37.0)]],
    );
    p.matrix("GROUPBY(A1:A7,D1:D7,_xleta.SUM,,0)", &[&[S("N"), N(9.0)], &[S("S"), N(37.0)]]);
    // Maiores primeiro, só o ano 1, função LAMBDA.
    p.matrix(
        "GROUPBY(A2:A7,D2:D7,LAMBDA(v,MAX(v)),0,0,-2,C2:C7=1)",
        &[&[S("S"), N(10.0)], &[S("N"), N(5.0)]],
    );
    // Dois campos com subtotais.
    p.matrix(
        "GROUPBY(A2:B7,D2:D7,_xleta.SUM,0,2)",
        &[
            &[S("N"), S("X"), N(6.0)],
            &[S("N"), S("Y"), N(3.0)],
            &[S("N"), S(""), N(9.0)],
            &[S("S"), S("X"), N(30.0)],
            &[S("S"), S("Y"), N(7.0)],
            &[S("S"), S(""), N(37.0)],
            &[S("Total Geral"), S(""), N(46.0)],
        ],
    );
    // Várias funções, com os nomes em cima.
    p.matrix(
        "GROUPBY(A2:A7,D2:D7,HSTACK(_xleta.SUM,_xleta.COUNT),0,0)",
        &[&[S(""), S("SOMA"), S("CONT.NÚM")], &[S("N"), N(9.0), N(3.0)], &[S("S"), N(37.0), N(3.0)]],
    );
    p.matrix(
        "GROUPBY(A2:A7,D2:D7,_xleta.PERCENTOF,0,0)",
        &[&[S("N"), N(9.0 / 46.0)], &[S("S"), N(37.0 / 46.0)]],
    );
    p.check("GROUPBY(A2:A7,D2:D6,_xleta.SUM)", VALUE);
    p.check("GROUPBY(A2:A7,D2:D7,5)", VALUE);
    p.check("GROUPBY(A2:A7,D2:D7,_xleta.SUM,0,3)", VALUE);
    p.check("GROUPBY(A2:A7,D2:D7,_xleta.SUM,,,,D2:D7>99)", CALC);
    p.matrix(
        "PIVOTBY(A2:A7,C2:C7,D2:D7,_xleta.SUM)",
        &[
            &[S(""), N(1.0), N(2.0), S("Total")],
            &[S("N"), N(8.0), N(1.0), N(9.0)],
            &[S("S"), N(10.0), N(27.0), N(37.0)],
            &[S("Total"), N(18.0), N(28.0), N(46.0)],
        ],
    );
    // Combinação que não existe fica em branco.
    p.matrix(
        "PIVOTBY(B2:B7,C2:C7,D2:D7,_xleta.SUM,0,0,,0,,A2:A7=\"S\")",
        &[&[S(""), N(1.0), N(2.0)], &[S("X"), N(10.0), N(20.0)], &[S("Y"), S(""), N(7.0)]],
    );
    p.check("PIVOTBY(A2:A7,C2:C6,D2:D7,_xleta.SUM)", VALUE);
    p.all(&[("PERCENTOF(D2:D3,D2:D7)", N(15.0 / 46.0)), ("PERCENTOF({1},{0})", DIV0)]);
}

#[test]
fn banco_de_dados() {
    let p = Plan::new(
        "Árvore,Altura,Idade,Produção,Lucro,,Árvore,Altura,Altura|\
         Maçã,18,20,14,105,,Maçã,>10,<16|\
         Pera,12,12,10,96,,Pera,,|\
         Cereja,13,14,9,105|\
         Maçã,14,15,10,75|\
         Pera,9,8,8,77,,Árvore,,Crit|\
         Maçã,8,9,6,45,,Maç,,=B2>12|\
         ,,,,,,=\"=Pera\"",
    );
    p.all(&[
        ("DSUM(A1:E7,\"Lucro\",G1:I3)", N(248.0)),
        ("DSUM(A1:E7,5,G1:I3)", N(248.0)),
        ("DSUM(A1:E7,\"Lucro\",G1:G2)", N(225.0)),
        // Texto sem operador vale pelo começo.
        ("DSUM(A1:E7,\"Lucro\",G6:G7)", N(225.0)),
        // Critério calculado para cada registro.
        ("DCOUNT(A1:E7,\"Altura\",I6:I7)", N(3.0)),
        ("DCOUNT(A1:E7,,G1:I3)", N(3.0)),
        ("DCOUNTA(A1:E7,\"Árvore\",G1:I3)", N(3.0)),
        ("DAVERAGE(A1:E7,\"Produção\",G1:H2)", N(12.0)),
        ("DMAX(A1:E7,\"Lucro\",G1:I3)", N(96.0)),
        ("DMIN(A1:E7,\"Lucro\",G1:H2)", N(75.0)),
        ("DPRODUCT(A1:E7,\"Produção\",G1:H2)", N(140.0)),
        ("DSTDEV(A1:E7,\"Produção\",G1:G3)", N(2.9664793948382653)),
        ("DSTDEVP(A1:E7,\"Produção\",G1:G3)", N(2.6532998322843198)),
        ("DVAR(A1:E7,\"Produção\",G1:G3)", N(8.8)),
        ("DVARP(A1:E7,\"Produção\",G1:G3)", N(7.04)),
        ("DGET(A1:E7,\"Produção\",G1:I2)", N(10.0)),
        ("DGET(A1:E7,\"Produção\",G1:G3)", NUM),
        ("DGET(A1:E7,\"Produção\",G7:G8)", VALUE),
        ("DSUM(A1:E7,\"Não tem\",G1:I3)", VALUE),
        ("DSUM(A1:E7,9,G1:I3)", VALUE),
        // Linha de critérios vazia: todos os registros.
        ("DSUM(A1:E7,\"Lucro\",K1:K2)", N(503.0)),
        ("DAVERAGE(A1:A1,1,G1:G2)", DIV0),
    ]);
}

/// As 67 funções do levantamento de 23/11/2026 que deixavam a célula vazia:
/// todas têm implementação, menos IMAGEM (que baixa da internet).
#[test]
fn levantamento() {
    let names = "MODE MODE.SNGL PERCENTILE PERCENTILE.INC QUARTILE PERCENTRANK CORREL FORECAST SLOPE INTERCEPT RSQ \
        FREQUENCY GEOMEAN TRIMMEAN AGGREGATE SUMSQ PMT PV FV NPER RATE NPV IRR XNPV XIRR IPMT PPMT CUMIPMT SLN DB \
        EFFECT NOMINAL MIRR FORMULATEXT SIN COS TAN RADIANS COMBIN PERMUT GCD LCM ROMAN TEXTSPLIT DOLLAR \
        NETWORKDAYS.INTL WORKDAY.INTL YEARFRAC DAYS360 ISFORMULA CELL INFO SHEET MMULT MINVERSE MAP REDUCE CONVERT \
        BIN2DEC DEC2HEX DSUM NORM.DIST NORM.S.INV T.TEST BITAND GROUPBY IMAGE LAMBDA";
    for n in names.split_whitespace() {
        let f = duck_calc::funcs::lookup(n).unwrap_or_else(|| panic!("{n} não está na tabela"));
        assert_eq!(duck_calc::funcs::def(f).opaque, n == "IMAGE", "{n}");
    }
}

/// As funções novas têm a lista de argumentos de acordo com o que aceitam.
#[test]
fn ajuda_das_funcoes() {
    for d in duck_calc::funcs::all().iter().filter(|d| !d.help.is_empty()) {
        let args: Vec<&str> = d.args.split("; ").filter(|a| !a.is_empty()).collect();
        let open = args.last() == Some(&"…");
        let named = &args[..args.len() - open as usize];
        let required = named.iter().take_while(|a| !a.starts_with('[')).count();
        assert_eq!(required, d.min as usize, "{}: obrigatórios em \"{}\"", d.pt, d.args);
        assert!(named[required..].iter().all(|a| a.starts_with('[')), "{}: ordem em \"{}\"", d.pt, d.args);
        if open {
            assert!(d.max > 100, "{}: não aceita mais argumentos", d.pt);
        } else {
            assert_eq!(named.len(), d.max as usize, "{}: total em \"{}\"", d.pt, d.args);
        }
    }
}
