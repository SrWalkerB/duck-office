//! Engenharia: unidades, bases numéricas, bits e função de erro.

use duck_core::CellError;

use super::special::{erf, erfc};
use super::{ALL, num1, nums};
use crate::eval::{Eval, Res};
use crate::funcs::{FnDef, n};
use crate::value::V;

pub const DEFS: &[FnDef] = &[
    n(
        "CONVERT",
        "CONVERTER",
        3,
        3,
        convert,
        "núm; de_unidade; para_unidade",
        "Converte de uma unidade para outra",
    )
    .lift(ALL),
    n("BIN2DEC", "BINADEC", 1, 1, bin2dec, "núm", "Binário para decimal").lift(ALL),
    n("BIN2HEX", "BINAHEX", 1, 2, bin2hex, "núm; [casas]", "Binário para hexadecimal").lift(ALL),
    n("BIN2OCT", "BINAOCT", 1, 2, bin2oct, "núm; [casas]", "Binário para octal").lift(ALL),
    n("DEC2BIN", "DECABIN", 1, 2, dec2bin, "núm; [casas]", "Decimal para binário").lift(ALL),
    n("DEC2HEX", "DECAHEX", 1, 2, dec2hex, "núm; [casas]", "Decimal para hexadecimal").lift(ALL),
    n("DEC2OCT", "DECAOCT", 1, 2, dec2oct, "núm; [casas]", "Decimal para octal").lift(ALL),
    n("HEX2BIN", "HEXABIN", 1, 2, hex2bin, "núm; [casas]", "Hexadecimal para binário").lift(ALL),
    n("HEX2DEC", "HEXADEC", 1, 1, hex2dec, "núm", "Hexadecimal para decimal").lift(ALL),
    n("HEX2OCT", "HEXAOCT", 1, 2, hex2oct, "núm; [casas]", "Hexadecimal para octal").lift(ALL),
    n("OCT2BIN", "OCTABIN", 1, 2, oct2bin, "núm; [casas]", "Octal para binário").lift(ALL),
    n("OCT2DEC", "OCTADEC", 1, 1, oct2dec, "núm", "Octal para decimal").lift(ALL),
    n("OCT2HEX", "OCTAHEX", 1, 2, oct2hex, "núm; [casas]", "Octal para hexadecimal").lift(ALL),
    n("BITAND", "BITAND", 2, 2, bitand, "núm1; núm2", "O E dos bits").lift(ALL),
    n("BITOR", "BITOR", 2, 2, bitor, "núm1; núm2", "O OU dos bits").lift(ALL),
    n("BITXOR", "BITXOR", 2, 2, bitxor, "núm1; núm2", "O OU exclusivo dos bits").lift(ALL),
    n(
        "BITLSHIFT",
        "DESLOCESQBIT",
        2,
        2,
        bitlshift,
        "núm; deslocamento",
        "Os bits deslocados para a esquerda",
    )
    .lift(ALL),
    n("BITRSHIFT", "DESLOCDIRBIT", 2, 2, bitrshift, "núm; deslocamento", "Os bits deslocados para a direita")
        .lift(ALL),
    n("DELTA", "DELTA", 1, 2, delta, "núm1; [núm2]", "1 se os números forem iguais").lift(ALL),
    n("GESTEP", "DEGRAU", 1, 2, gestep, "núm; [passo]", "1 se o número não for menor que o passo").lift(ALL),
    n("ERF", "FUNERRO", 1, 2, erf_fn, "limite_inferior; [limite_superior]", "A função de erro").lift(ALL),
    n("ERF.PRECISE", "FUNERRO.PRECISO", 1, 1, erf_precise, "x", "A função de erro").lift(ALL),
    n("ERFC", "FUNERROCOMPL", 1, 1, erfc_fn, "x", "A função de erro complementar").lift(ALL),
    n("ERFC.PRECISE", "FUNERROCOMPL.PRECISO", 1, 1, erfc_fn, "x", "A função de erro complementar").lift(ALL),
];

const NUM: CellError = CellError::Num;

// ---------------------------------------------------------------------------
// Bases
// ---------------------------------------------------------------------------

/// Número com sinal escrito em `bits` por algarismo (até 10 algarismos,
/// complemento de dois).
fn read_base<'e>(e: &mut Eval<'e>, n: u32, bits: u32) -> Res<i64> {
    let s = e.text(n)?;
    let t = s.trim();
    if t.len() > 10 {
        return Err(NUM);
    }
    let mut r: i64 = 0;
    for c in t.chars() {
        r = (r << bits) | c.to_digit(1 << bits).ok_or(NUM)? as i64;
    }
    if r >> (bits * 10 - 1) & 1 == 1 {
        r -= 1 << (bits * 10);
    }
    Ok(r)
}

fn write_base<'e>(e: &mut Eval<'e>, a: &[u32], x: i64, bits: u32) -> Res<V<'e>> {
    let half = 1i64 << (bits * 10 - 1);
    if x < -half || x >= half {
        return Err(NUM);
    }
    let u = if x < 0 { x + 2 * half } else { x };
    let s = match bits {
        1 => format!("{u:b}"),
        3 => format!("{u:o}"),
        _ => format!("{u:X}"),
    };
    match a.get(1) {
        Some(&n) if !e.is_missing(n) => {
            let w = e.num(n)?.trunc();
            // Negativos têm sempre 10 algarismos.
            if x < 0 {
                return Ok(V::text(s));
            }
            if !(1.0..=10.0).contains(&w) || s.len() > w as usize {
                return Err(NUM);
            }
            Ok(V::text(format!("{s:0>w$}", w = w as usize)))
        }
        _ => Ok(V::text(s)),
    }
}

macro_rules! bases {
    ($($name:ident: $from:expr => $to:expr),+ $(,)?) => {
        $(fn $name<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
            let x = read_base(e, a[0], $from)?;
            write_base(e, a, x, $to)
        })+
    };
}

bases!(bin2hex: 1 => 4, bin2oct: 1 => 3, hex2bin: 4 => 1, hex2oct: 4 => 3, oct2bin: 3 => 1, oct2hex: 3 => 4);

fn bin2dec<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(read_base(e, a[0], 1)? as f64))
}

fn hex2dec<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(read_base(e, a[0], 4)? as f64))
}

fn oct2dec<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    Ok(V::Num(read_base(e, a[0], 3)? as f64))
}

fn from_dec<'e>(e: &mut Eval<'e>, a: &[u32], bits: u32) -> Res<V<'e>> {
    let x = e.num(a[0])?.trunc();
    if x.abs() > 1e15 {
        return Err(NUM);
    }
    write_base(e, a, x as i64, bits)
}

fn dec2bin<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    from_dec(e, a, 1)
}

fn dec2oct<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    from_dec(e, a, 3)
}

fn dec2hex<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    from_dec(e, a, 4)
}

// ---------------------------------------------------------------------------
// Bits
// ---------------------------------------------------------------------------

fn bits(x: f64) -> Res<u64> {
    if x == x.trunc() && (0.0..281_474_976_710_656.0).contains(&x) { Ok(x as u64) } else { Err(NUM) }
}

fn shift(x: f64, by: f64) -> Res<f64> {
    let v = bits(x)?;
    let by = by.trunc();
    if by.abs() > 53.0 {
        return Err(NUM);
    }
    let r = if by >= 0.0 { (v as u128) << by as u32 } else { (v as u128) >> (-by) as u32 };
    if r < 1 << 48 { Ok(r as f64) } else { Err(NUM) }
}

nums!(bitand, [x, y], Ok((bits(x)? & bits(y)?) as f64));
nums!(bitor, [x, y], Ok((bits(x)? | bits(y)?) as f64));
nums!(bitxor, [x, y], Ok((bits(x)? ^ bits(y)?) as f64));
nums!(bitlshift, [x, by], shift(x, by));
nums!(bitrshift, [x, by], shift(x, -by));
nums!(delta, [x, y = 0.0], Ok(if x == y { 1.0 } else { 0.0 }));
nums!(gestep, [x, s = 0.0], Ok(if x >= s { 1.0 } else { 0.0 }));

fn erf_fn<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    match a.get(1) {
        Some(&n) if !e.is_missing(n) => Ok(V::num(erf(e.num(n)?) - erf(x))),
        _ => Ok(V::num(erf(x))),
    }
}

num1!(erf_precise, x, Ok(erf(x)));
num1!(erfc_fn, x, Ok(erfc(x)));

// ---------------------------------------------------------------------------
// Unidades
// ---------------------------------------------------------------------------

/// Grandeza, símbolos, valor na unidade de base e se aceita prefixos.
type Unit = (u8, &'static [&'static str], f64, bool);

const MASS: u8 = 0;
const LEN: u8 = 1;
const TIME: u8 = 2;
const PRESS: u8 = 3;
const FORCE: u8 = 4;
const ENERGY: u8 = 5;
const POWER: u8 = 6;
const MAGNET: u8 = 7;
const VOL: u8 = 8;
const AREA: u8 = 9;
const INFO: u8 = 10;
const SPEED: u8 = 11;

const FT: f64 = 0.3048;
const IN: f64 = 0.0254;
const MI: f64 = 1609.344;
const NMI: f64 = 1852.0;
const LY: f64 = 9.460_730_472_580_8e15;
const PICA: f64 = IN / 72.0;
const LB: f64 = 453.592_37;
const CAL: f64 = 4.1868;
const GAL: f64 = 231.0 * IN * IN * IN * 1000.0;
const UKPT: f64 = 0.568_261_25;

const UNITS: &[Unit] = &[
    // Massa (g)
    (MASS, &["g"], 1.0, true),
    (MASS, &["sg"], 14_593.902_937_206_364, false),
    (MASS, &["lbm"], LB, false),
    (MASS, &["u"], 1.660_538_782e-24, true),
    (MASS, &["ozm"], LB / 16.0, false),
    (MASS, &["grain"], LB / 7000.0, false),
    (MASS, &["cwt", "shweight"], LB * 100.0, false),
    (MASS, &["uk_cwt", "lcwt", "hweight"], LB * 112.0, false),
    (MASS, &["stone"], LB * 14.0, false),
    (MASS, &["ton"], LB * 2000.0, false),
    (MASS, &["uk_ton", "LTON", "brton"], LB * 2240.0, false),
    // Comprimento (m)
    (LEN, &["m"], 1.0, true),
    (LEN, &["mi"], MI, false),
    (LEN, &["Nmi"], NMI, false),
    (LEN, &["in"], IN, false),
    (LEN, &["ft"], FT, false),
    (LEN, &["yd"], FT * 3.0, false),
    (LEN, &["ang"], 1e-10, true),
    (LEN, &["ell"], 1.143, false),
    (LEN, &["ly"], LY, true),
    (LEN, &["parsec", "pc"], 3.085_677_581_281_55e16, true),
    (LEN, &["Picapt", "Pica"], PICA, false),
    (LEN, &["pica"], IN / 6.0, false),
    (LEN, &["survey_mi"], 1_609.347_218_694_437_6, false),
    // Tempo (s)
    (TIME, &["yr"], 31_557_600.0, false),
    (TIME, &["day", "d"], 86_400.0, false),
    (TIME, &["hr"], 3600.0, false),
    (TIME, &["mn", "min"], 60.0, false),
    (TIME, &["sec", "s"], 1.0, true),
    // Pressão (Pa)
    (PRESS, &["Pa", "p"], 1.0, true),
    (PRESS, &["atm", "at"], 101_325.0, true),
    (PRESS, &["mmHg"], 133.322, true),
    (PRESS, &["psi"], 6_894.757_293_168_361, false),
    (PRESS, &["Torr"], 101_325.0 / 760.0, false),
    // Força (N)
    (FORCE, &["N"], 1.0, true),
    (FORCE, &["dyn", "dy"], 1e-5, true),
    (FORCE, &["lbf"], 4.448_221_615_260_5, false),
    (FORCE, &["pond"], 9.806_65e-3, true),
    // Energia (J)
    (ENERGY, &["J"], 1.0, true),
    (ENERGY, &["e"], 1e-7, true),
    (ENERGY, &["c"], 4.184, true),
    (ENERGY, &["cal"], CAL, true),
    (ENERGY, &["eV", "ev"], 1.602_176_487e-19, true),
    (ENERGY, &["HPh", "hh"], 2_684_519.537_696_173, false),
    (ENERGY, &["Wh", "wh"], 3600.0, true),
    (ENERGY, &["flb"], 1.355_817_948_331_400_4, false),
    (ENERGY, &["BTU", "btu"], 1_055.055_852_62, false),
    // Potência (W)
    (POWER, &["HP", "h"], 745.699_871_582_270_2, false),
    (POWER, &["PS"], 735.498_75, false),
    (POWER, &["W", "w"], 1.0, true),
    // Magnetismo (T)
    (MAGNET, &["T"], 1.0, true),
    (MAGNET, &["ga"], 1e-4, true),
    // Volume (l)
    (VOL, &["tsp"], GAL / 768.0, false),
    (VOL, &["tspm"], 0.005, false),
    (VOL, &["tbs"], GAL / 256.0, false),
    (VOL, &["oz"], GAL / 128.0, false),
    (VOL, &["cup"], GAL / 16.0, false),
    (VOL, &["pt", "us_pt"], GAL / 8.0, false),
    (VOL, &["uk_pt"], UKPT, false),
    (VOL, &["qt"], GAL / 4.0, false),
    (VOL, &["uk_qt"], UKPT * 2.0, false),
    (VOL, &["gal"], GAL, false),
    (VOL, &["uk_gal"], UKPT * 8.0, false),
    (VOL, &["l", "L", "lt"], 1.0, true),
    (VOL, &["ang3", "ang^3"], 1e-27, false),
    (VOL, &["barrel"], GAL * 42.0, false),
    (VOL, &["bushel"], 2150.42 * IN * IN * IN * 1000.0, false),
    (VOL, &["ft3", "ft^3"], FT * FT * FT * 1000.0, false),
    (VOL, &["in3", "in^3"], IN * IN * IN * 1000.0, false),
    (VOL, &["ly3", "ly^3"], LY * LY * LY * 1000.0, false),
    (VOL, &["m3", "m^3"], 1000.0, true),
    (VOL, &["mi3", "mi^3"], MI * MI * MI * 1000.0, false),
    (VOL, &["yd3", "yd^3"], FT * FT * FT * 27_000.0, false),
    (VOL, &["Nmi3", "Nmi^3"], NMI * NMI * NMI * 1000.0, false),
    (VOL, &["Picapt3", "Picapt^3", "Pica3", "Pica^3"], PICA * PICA * PICA * 1000.0, false),
    (VOL, &["GRT", "regton"], 100.0 * FT * FT * FT * 1000.0, false),
    (VOL, &["MTON"], 40.0 * FT * FT * FT * 1000.0, false),
    // Área (m²)
    (AREA, &["uk_acre"], 4_046.856_422_4, false),
    (AREA, &["us_acre"], 4_046.872_609_874_252, false),
    (AREA, &["ang2", "ang^2"], 1e-20, false),
    (AREA, &["ar"], 100.0, true),
    (AREA, &["ft2", "ft^2"], FT * FT, false),
    (AREA, &["ha"], 10_000.0, false),
    (AREA, &["in2", "in^2"], IN * IN, false),
    (AREA, &["ly2", "ly^2"], LY * LY, false),
    (AREA, &["m2", "m^2"], 1.0, true),
    (AREA, &["Morgen"], 2500.0, false),
    (AREA, &["mi2", "mi^2"], MI * MI, false),
    (AREA, &["Nmi2", "Nmi^2"], NMI * NMI, false),
    (AREA, &["Picapt2", "Picapt^2", "Pica2", "Pica^2"], PICA * PICA, false),
    (AREA, &["yd2", "yd^2"], FT * FT * 9.0, false),
    // Informação (bit)
    (INFO, &["bit"], 1.0, true),
    (INFO, &["byte"], 8.0, true),
    // Velocidade (m/s)
    (SPEED, &["admkn"], 6080.0 * FT / 3600.0, false),
    (SPEED, &["kn"], NMI / 3600.0, false),
    (SPEED, &["m/h", "m/hr"], 1.0 / 3600.0, true),
    (SPEED, &["m/s", "m/sec"], 1.0, true),
    (SPEED, &["mph"], MI / 3600.0, false),
];

const PREFIXES: &[(&str, f64)] = &[
    ("da", 1e1),
    ("Y", 1e24),
    ("Z", 1e21),
    ("E", 1e18),
    ("P", 1e15),
    ("T", 1e12),
    ("G", 1e9),
    ("M", 1e6),
    ("k", 1e3),
    ("h", 1e2),
    ("e", 1e1),
    ("d", 1e-1),
    ("c", 1e-2),
    ("m", 1e-3),
    ("u", 1e-6),
    ("n", 1e-9),
    ("p", 1e-12),
    ("f", 1e-15),
    ("a", 1e-18),
    ("z", 1e-21),
    ("y", 1e-24),
];

const BINARY: &[(&str, f64)] = &[
    ("Yi", 1_208_925_819_614_629_174_706_176.0),
    ("Zi", 1_180_591_620_717_411_303_424.0),
    ("Ei", 1_152_921_504_606_846_976.0),
    ("Pi", 1_125_899_906_842_624.0),
    ("Ti", 1_099_511_627_776.0),
    ("Gi", 1_073_741_824.0),
    ("Mi", 1_048_576.0),
    ("ki", 1024.0),
];

/// Grandeza e valor de uma unidade (com prefixo).
fn unit(s: &str) -> Option<(u8, f64)> {
    let find = |s: &str| UNITS.iter().find(|u| u.1.contains(&s));
    if let Some(u) = find(s) {
        return Some((u.0, u.2));
    }
    for (p, k) in BINARY {
        if let Some(u) = s.strip_prefix(p).and_then(find)
            && u.0 == INFO
        {
            return Some((u.0, u.2 * k));
        }
    }
    for (p, k) in PREFIXES {
        if let Some(rest) = s.strip_prefix(p)
            && let Some(u) = find(rest)
            && u.3
        {
            // km² é (km)², não mil m².
            let power = match (u.0, rest) {
                (AREA, "m2" | "m^2") => 2,
                (VOL, "m3" | "m^3") => 3,
                _ => 1,
            };
            return Some((u.0, u.2 * k.powi(power)));
        }
    }
    None
}

/// Temperatura em kelvins: (soma, fator), K = (x + soma)·fator.
fn temperature(s: &str) -> Option<(f64, f64)> {
    Some(match s {
        "C" | "cel" => (273.15, 1.0),
        "F" | "fah" => (459.67, 5.0 / 9.0),
        "K" | "kel" => (0.0, 1.0),
        "Rank" => (0.0, 5.0 / 9.0),
        "Reau" => (218.52, 1.25),
        _ => {
            let rest = s.strip_suffix('K').or_else(|| s.strip_suffix("kel"))?;
            let k = PREFIXES.iter().find(|p| p.0 == rest)?.1;
            (0.0, k)
        }
    })
}

fn convert<'e>(e: &mut Eval<'e>, a: &[u32]) -> Res<V<'e>> {
    let x = e.num(a[0])?;
    let from = e.text(a[1])?;
    let to = e.text(a[2])?;
    if let (Some(p), Some(q)) = (temperature(&from), temperature(&to)) {
        if from == to {
            return Ok(V::Num(x));
        }
        return Ok(V::num((x + p.0) * p.1 / q.1 - q.0));
    }
    let p = unit(&from).ok_or(CellError::NA)?;
    let q = unit(&to).ok_or(CellError::NA)?;
    if p.0 != q.0 {
        return Err(CellError::NA);
    }
    if from == to {
        return Ok(V::Num(x));
    }
    Ok(V::num(x * p.1 / q.1))
}
