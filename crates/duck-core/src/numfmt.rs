//! Formatos de número do Excel (ECMA-376, 18.8.31), com saída em pt-BR:
//! vírgula decimal, ponto de milhar, nomes de mês e de dia em português.
//!
//! O código do formato no arquivo é sempre "invariante" (`.` decimal, `,`
//! milhar); só a saída segue o idioma. Um formato é lido uma vez
//! ([`NumFmt::parse`]) e depois aplicado a qualquer número de células.

use std::fmt::Write as _;

/// Código de um formato interno (ids 0–49), já adaptado para pt-BR.
pub fn builtin(id: u16) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "\"R$ \"#,##0;\\-\"R$ \"#,##0",
        6 => "\"R$ \"#,##0;[Red]\\-\"R$ \"#,##0",
        7 => "\"R$ \"#,##0.00;\\-\"R$ \"#,##0.00",
        8 => "\"R$ \"#,##0.00;[Red]\\-\"R$ \"#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        // Data curta do idioma (no Excel em pt-BR: dd/mm/aaaa).
        14 => "dd/mm/yyyy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "dd/mm/yyyy hh:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        41 => "_-* #,##0_-;-* #,##0_-;_-* \"-\"_-;_-@_-",
        42 => "_-\"R$ \"* #,##0_-;-\"R$ \"* #,##0_-;_-\"R$ \"* \"-\"_-;_-@_-",
        43 => "_-* #,##0.00_-;-* #,##0.00_-;_-* \"-\"??_-;_-@_-",
        44 => "_-\"R$ \"* #,##0.00_-;-\"R$ \"* #,##0.00_-;_-\"R$ \"* \"-\"??_-;_-@_-",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        // Formatos de data do Leste Asiático: mostrados como data curta.
        27..=36 | 50..=58 => "dd/mm/yyyy",
        _ => return None,
    })
}

/// Paleta legada de 64 cores (`indexed="n"`), ARGB.
pub const INDEXED: [u32; 66] = [
    0xFF000000, 0xFFFFFFFF, 0xFFFF0000, 0xFF00FF00, 0xFF0000FF, 0xFFFFFF00, 0xFFFF00FF, 0xFF00FFFF,
    0xFF000000, 0xFFFFFFFF, 0xFFFF0000, 0xFF00FF00, 0xFF0000FF, 0xFFFFFF00, 0xFFFF00FF, 0xFF00FFFF,
    0xFF800000, 0xFF008000, 0xFF000080, 0xFF808000, 0xFF800080, 0xFF008080, 0xFFC0C0C0, 0xFF808080,
    0xFF9999FF, 0xFF993366, 0xFFFFFFCC, 0xFFCCFFFF, 0xFF660066, 0xFFFF8080, 0xFF0066CC, 0xFFCCCCFF,
    0xFF000080, 0xFFFF00FF, 0xFFFFFF00, 0xFF00FFFF, 0xFF800080, 0xFF800000, 0xFF008080, 0xFF0000FF,
    0xFF00CCFF, 0xFFCCFFFF, 0xFFCCFFCC, 0xFFFFFF99, 0xFF99CCFF, 0xFFFF99CC, 0xFFCC99FF, 0xFFFFCC99,
    0xFF3366FF, 0xFF33CCCC, 0xFF99CC00, 0xFFFFCC00, 0xFFFF9900, 0xFFFF6600, 0xFF666699, 0xFF969696,
    0xFF003366, 0xFF339966, 0xFF003300, 0xFF333300, 0xFF993300, 0xFF993366, 0xFF333399, 0xFF333333,
    // 64: cor do texto do sistema; 65: fundo do sistema.
    0xFF000000, 0xFFFFFFFF,
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Cmp {
    fn test(self, a: f64, b: f64) -> bool {
        match self {
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
            Cmp::Eq => a == b,
            Cmp::Ne => a != b,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Lit(String),
    /// `0`: dígito ou zero.
    Zero,
    /// `#`: dígito, ou nada.
    Hash,
    /// `?`: dígito, ou espaço.
    Q,
    Point,
    /// Vírgula no código: separador de milhar ou divisão por mil (decidido depois).
    Comma,
    Percent,
    Exp {
        plus: bool,
    },
    Slash,
    Text,
    General,
    Year(u8),
    /// Mês ou minuto, decidido depois pela vizinhança.
    MonthOrMinute(u8),
    Month(u8),
    Minute(u8),
    Day(u8),
    Hour(u8),
    Second(u8),
    /// `[h]`, `[mm]`, `[ss]`: tempo decorrido.
    Elapsed(u8, u8),
    /// Frações de segundo (`.0`, `.00`, `.000`).
    SubSec(u8),
    /// `true` = AM/PM, `false` = A/P.
    AmPm(bool),
}

impl Tok {
    fn is_digit(&self) -> bool {
        matches!(self, Tok::Zero | Tok::Hash | Tok::Q)
    }

    fn is_date(&self) -> bool {
        matches!(
            self,
            Tok::Year(_)
                | Tok::MonthOrMinute(_)
                | Tok::Month(_)
                | Tok::Minute(_)
                | Tok::Day(_)
                | Tok::Hour(_)
                | Tok::Second(_)
                | Tok::Elapsed(..)
                | Tok::SubSec(_)
                | Tok::AmPm(_)
        )
    }
}

#[derive(Clone, Debug, Default)]
struct Section {
    toks: Vec<Tok>,
    color: Option<u32>,
    cond: Option<(Cmp, f64)>,
    date: bool,
    /// Usa nomes de mês/dia em inglês (`[$-409]`).
    english: bool,
    text: bool,
    // Números:
    int_digits: usize,
    frac_digits: usize,
    exp_digits: usize,
    has_exp: bool,
    thousands: bool,
    /// Divisões por mil (vírgulas no fim).
    scale: i32,
    percent: i32,
    /// Fração (`# ?/?`): dígitos do numerador e do denominador, ou denominador fixo.
    fraction: Option<Fraction>,
    has_ampm: bool,
    sub_sec: u8,
    general: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Fraction {
    /// Há uma parte inteira antes da fração.
    whole: bool,
    num_digits: usize,
    den_digits: usize,
    fixed_den: Option<u32>,
}

/// Um formato de número já interpretado.
#[derive(Clone, Debug)]
pub struct NumFmt {
    sections: Vec<Section>,
    general: bool,
}

/// O que a formatação produziu além do texto.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Formatted {
    /// Cor do formato (`[Red]`, `[Color10]`...), ARGB.
    pub color: Option<u32>,
    /// O valor não pode ser mostrado no formato (data negativa): mostre `###`.
    pub overflow: bool,
    /// Posição (byte em `out`) do preenchimento `*`: o texto antes vai para a
    /// esquerda da célula e o depois para a direita (formatos contábeis).
    pub fill_at: Option<usize>,
}

/// Marca interna do preenchimento `*` dentro do texto formatado.
const FILL: &str = "\u{1}";

/// Tira as marcas de preenchimento de `out[start..]`, guardando a primeira.
fn take_fill(out: &mut String, start: usize, res: &mut Formatted) {
    if let Some(p) = out[start..].find(FILL) {
        res.fill_at = Some(start + p);
        out.retain(|c| c != '\u{1}');
    }
}

impl Default for NumFmt {
    fn default() -> Self {
        Self::general()
    }
}

impl NumFmt {
    pub fn general() -> Self {
        Self { sections: Vec::new(), general: true }
    }

    /// Interpreta um código de formato. Nunca falha: o que não é entendido
    /// vira texto literal.
    pub fn parse(code: &str) -> Self {
        let trimmed = code.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("general") {
            return Self::general();
        }
        let sections: Vec<Section> = split_sections(code).into_iter().map(parse_section).collect();
        let general = sections.len() == 1 && sections[0].general && sections[0].toks.len() == 1;
        Self { sections, general }
    }

    /// O formato é "Geral".
    pub fn is_general(&self) -> bool {
        self.general
    }

    /// O formato mostra datas ou horas.
    pub fn is_date(&self) -> bool {
        self.sections.first().is_some_and(|s| s.date)
    }

    /// O formato mostra percentual.
    pub fn is_percent(&self) -> bool {
        self.sections.first().is_some_and(|s| s.percent > 0)
    }

    /// Formata um número em `out` (anexa).
    pub fn format_number(&self, v: f64, date1904: bool, out: &mut String) -> Formatted {
        let start = out.len();
        let mut res = self.format_number_inner(v, date1904, out);
        take_fill(out, start, &mut res);
        res
    }

    fn format_number_inner(&self, v: f64, date1904: bool, out: &mut String) -> Formatted {
        if self.general || !v.is_finite() {
            format_general(v, 11, out);
            return Formatted::default();
        }
        let (sec, v, minus) = self.pick(v);
        let res = Formatted { color: sec.color, ..Default::default() };
        if sec.general && !sec.toks.iter().any(Tok::is_digit) {
            // "Geral" com texto em volta: `"Total: "General`.
            for t in &sec.toks {
                match t {
                    Tok::Lit(s) => out.push_str(s),
                    Tok::General => {
                        if minus {
                            out.push('-');
                        }
                        format_general(v, 11, out);
                    }
                    _ => {}
                }
            }
            return res;
        }
        if sec.date {
            if v < 0.0 || (minus && v != 0.0) {
                return Formatted { color: sec.color, overflow: true, ..Default::default() };
            }
            format_date(sec, v, date1904, out);
            return res;
        }
        let has_number = sec.toks.iter().any(Tok::is_digit);
        if !has_number {
            if sec.text {
                // "@" aplicado a número: o número aparece como no formato Geral.
                let mut g = String::new();
                if minus {
                    g.push('-');
                }
                format_general(v, 11, &mut g);
                for t in &sec.toks {
                    match t {
                        Tok::Lit(s) => out.push_str(s),
                        Tok::Text => out.push_str(&g),
                        _ => {}
                    }
                }
                return res;
            }
            // Só texto literal (ex.: `"-"` para o zero): o sinal some também.
            for t in &sec.toks {
                if let Tok::Lit(s) = t {
                    out.push_str(s);
                }
            }
            return res;
        }
        let start = out.len();
        format_digits(sec, v, out);
        if minus && out[start..].bytes().any(|b| b.is_ascii_digit() && b != b'0') {
            out.insert(start, '-');
        }
        res
    }

    /// Formata um texto. `None` = mostre o texto como está.
    pub fn format_text(&self, s: &str, out: &mut String) -> Option<Formatted> {
        if self.general {
            return None;
        }
        let sec = match self.sections.len() {
            4.. => &self.sections[3],
            _ => self.sections.iter().find(|s| s.text)?,
        };
        let start = out.len();
        for t in &sec.toks {
            match t {
                Tok::Lit(l) => out.push_str(l),
                Tok::Text => out.push_str(s),
                _ => {}
            }
        }
        let mut res = Formatted { color: sec.color, ..Default::default() };
        take_fill(out, start, &mut res);
        Some(res)
    }

    /// Escolhe a seção para `v`. Retorna o valor a formatar e se leva sinal de menos.
    fn pick(&self, v: f64) -> (&Section, f64, bool) {
        let n = self.sections.len();
        let s = &self.sections;
        if s[0].cond.is_some() || s.get(1).is_some_and(|x| x.cond.is_some()) {
            let matches = |sec: &Section| sec.cond.is_some_and(|(c, b)| c.test(v, b));
            if matches(&s[0]) {
                return (&s[0], v.abs(), v < 0.0);
            }
            if n > 1 && matches(&s[1]) {
                return (&s[1], v.abs(), v < 0.0);
            }
            if n > 2 && !s[2].text {
                return (&s[2], v.abs(), v < 0.0);
            }
            if n > 1 && s[1].cond.is_none() {
                return (&s[1], v.abs(), false);
            }
            return (&s[0], v.abs(), v < 0.0);
        }
        // Seções só de texto (`@`) não valem para números.
        let numeric = s.iter().take(3).filter(|x| !(x.text && !x.toks.iter().any(Tok::is_digit))).count();
        let numeric = numeric.max(1);
        if v < 0.0 && numeric >= 2 {
            (&s[1], -v, false)
        } else if v == 0.0 && numeric >= 3 {
            (&s[2], 0.0, false)
        } else {
            (&s[0], v.abs(), v < 0.0)
        }
    }
}

// ---------------------------------------------------------------------------
// Leitura do código
// ---------------------------------------------------------------------------

/// Corta em `;` fora de aspas, colchetes e escapes.
fn split_sections(code: &str) -> Vec<&str> {
    let b = code.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
            }
            b'[' => {
                while i < b.len() && b[i] != b']' {
                    i += 1;
                }
            }
            b'\\' | b'_' | b'*' => i += 1,
            b';' => {
                out.push(&code[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&code[start.min(code.len())..]);
    out.truncate(4);
    out
}

fn named_color(name: &str) -> Option<u32> {
    Some(match name.to_ascii_lowercase().as_str() {
        "black" | "preto" => 0xFF000000,
        "blue" | "azul" => 0xFF0000FF,
        "cyan" | "ciano" => 0xFF00FFFF,
        "green" | "verde" => 0xFF00FF00,
        "magenta" => 0xFFFF00FF,
        "red" | "vermelho" => 0xFFFF0000,
        "white" | "branco" => 0xFFFFFFFF,
        "yellow" | "amarelo" => 0xFFFFFF00,
        _ => return None,
    })
}

fn push_lit(toks: &mut Vec<Tok>, s: &str) {
    if let Some(Tok::Lit(prev)) = toks.last_mut() {
        prev.push_str(s);
    } else {
        toks.push(Tok::Lit(s.to_string()));
    }
}

fn parse_section(code: &str) -> Section {
    let mut sec = Section::default();
    let chars: Vec<char> = code.chars().collect();
    let mut toks: Vec<Tok> = Vec::new();
    let mut i = 0;
    let lower = |i: usize| chars.get(i).map(|c| c.to_ascii_lowercase());
    let run = |i: usize, c: char| {
        let mut n = 0;
        while chars.get(i + n).is_some_and(|x| x.eq_ignore_ascii_case(&c)) {
            n += 1;
        }
        n
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    s.push(chars[i]);
                    i += 1;
                }
                push_lit(&mut toks, &s);
                i += 1;
            }
            '\\' => {
                if let Some(&n) = chars.get(i + 1) {
                    push_lit(&mut toks, n.encode_utf8(&mut [0; 4]));
                }
                i += 2;
            }
            '_' => {
                // Espaço da largura do próximo caractere.
                push_lit(&mut toks, " ");
                i += 2;
            }
            '*' => {
                // Repetir o caractere até encher a célula: marca a posição.
                push_lit(&mut toks, FILL);
                i += 2;
            }
            '[' => {
                let end = chars[i..].iter().position(|&x| x == ']').map_or(chars.len(), |p| i + p);
                let inner: String = chars[i + 1..end].iter().collect();
                bracket(&mut sec, &mut toks, &inner);
                i = end + 1;
            }
            '0' => {
                toks.push(Tok::Zero);
                i += 1;
            }
            '#' => {
                toks.push(Tok::Hash);
                i += 1;
            }
            '?' => {
                toks.push(Tok::Q);
                i += 1;
            }
            '.' => {
                toks.push(Tok::Point);
                i += 1;
            }
            ',' => {
                toks.push(Tok::Comma);
                i += 1;
            }
            '%' => {
                toks.push(Tok::Percent);
                i += 1;
            }
            '/' => {
                toks.push(Tok::Slash);
                i += 1;
            }
            '@' => {
                toks.push(Tok::Text);
                i += 1;
            }
            'E' | 'e' if matches!(chars.get(i + 1), Some('+') | Some('-')) => {
                toks.push(Tok::Exp { plus: chars[i + 1] == '+' });
                i += 2;
            }
            _ if c.is_ascii_alphabetic() => {
                let rest: String = chars[i..].iter().take(7).collect::<String>().to_ascii_lowercase();
                if rest.starts_with("general") {
                    toks.push(Tok::General);
                    sec.general = true;
                    i += 7;
                } else if rest.starts_with("am/pm") {
                    toks.push(Tok::AmPm(true));
                    i += 5;
                } else if rest.starts_with("a/p") {
                    toks.push(Tok::AmPm(false));
                    i += 3;
                } else {
                    let n = run(i, c);
                    let k = n.min(255) as u8;
                    match lower(i).unwrap_or(' ') {
                        'y' | 'e' => toks.push(Tok::Year(k)),
                        'm' => toks.push(Tok::MonthOrMinute(k)),
                        'd' => toks.push(Tok::Day(k)),
                        'h' => toks.push(Tok::Hour(k)),
                        's' => toks.push(Tok::Second(k)),
                        // `b` (ano budista), `g` (era): raros, viram texto.
                        _ => {
                            let s: String = chars[i..i + n].iter().collect();
                            push_lit(&mut toks, &s);
                        }
                    }
                    i += n;
                }
            }
            _ => {
                push_lit(&mut toks, c.encode_utf8(&mut [0; 4]));
                i += 1;
            }
        }
    }

    sec.date = toks.iter().any(Tok::is_date);
    if sec.date {
        finish_date(&mut sec, toks);
    } else {
        finish_number(&mut sec, toks);
    }
    sec.text = sec.toks.contains(&Tok::Text);
    sec
}

fn bracket(sec: &mut Section, toks: &mut Vec<Tok>, inner: &str) {
    let lower = inner.to_ascii_lowercase();
    if let Some(c) = named_color(inner) {
        sec.color = Some(c);
    } else if let Some(n) = lower.strip_prefix("color").or_else(|| lower.strip_prefix("cor")) {
        if let Ok(n) = n.trim().parse::<usize>()
            && (1..=56).contains(&n)
        {
            sec.color = Some(INDEXED[7 + n]);
        }
    } else if let Some(rest) = inner.strip_prefix('$') {
        // `[$R$-416]`: símbolo de moeda + idioma. `[$-409]`: só idioma.
        let (sym, locale) = rest.split_once('-').unwrap_or((rest, ""));
        push_lit(toks, sym);
        let locale = locale.trim_start_matches(|c: char| !c.is_ascii_hexdigit());
        if let Ok(id) = u32::from_str_radix(locale, 16) {
            // Inglês (EUA, Reino Unido...): nomes de mês e dia em inglês.
            sec.english = id & 0x3ff == 0x09;
        }
    } else if inner.starts_with(['<', '>', '=']) {
        let (op, rest) = if let Some(r) = inner.strip_prefix("<=") {
            (Cmp::Le, r)
        } else if let Some(r) = inner.strip_prefix(">=") {
            (Cmp::Ge, r)
        } else if let Some(r) = inner.strip_prefix("<>") {
            (Cmp::Ne, r)
        } else if let Some(r) = inner.strip_prefix('<') {
            (Cmp::Lt, r)
        } else if let Some(r) = inner.strip_prefix('>') {
            (Cmp::Gt, r)
        } else {
            (Cmp::Eq, &inner[1..])
        };
        if let Ok(b) = rest.trim().parse::<f64>() {
            sec.cond = Some((op, b));
        }
    } else if !lower.is_empty() && lower.chars().all(|c| c == 'h') {
        toks.push(Tok::Elapsed(b'h', lower.len() as u8));
    } else if !lower.is_empty() && lower.chars().all(|c| c == 'm') {
        toks.push(Tok::Elapsed(b'm', lower.len() as u8));
    } else if !lower.is_empty() && lower.chars().all(|c| c == 's') {
        toks.push(Tok::Elapsed(b's', lower.len() as u8));
    }
    // Outros (`[DBNum1]`, `[natnum]`...): ignorados.
}

fn finish_date(sec: &mut Section, toks: Vec<Tok>) {
    // Dígitos e pontuação numérica numa data: `.0` depois dos segundos são
    // frações de segundo; o resto é texto.
    let mut out: Vec<Tok> = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Point if toks.get(i + 1) == Some(&Tok::Zero) => {
                let mut n = 0u8;
                while toks.get(i + 1 + n as usize) == Some(&Tok::Zero) {
                    n += 1;
                }
                out.push(Tok::SubSec(n.min(3)));
                sec.sub_sec = sec.sub_sec.max(n.min(3));
                i += 1 + n as usize;
                continue;
            }
            Tok::Zero => push_lit(&mut out, "0"),
            Tok::Hash => push_lit(&mut out, "#"),
            Tok::Q => push_lit(&mut out, "?"),
            Tok::Point => push_lit(&mut out, "."),
            Tok::Comma => push_lit(&mut out, ","),
            Tok::Slash => push_lit(&mut out, "/"),
            Tok::Percent => push_lit(&mut out, "%"),
            t => out.push(t.clone()),
        }
        i += 1;
    }
    // `m` é minuto logo depois de uma hora ou logo antes de segundos.
    let date_idx: Vec<usize> = (0..out.len()).filter(|&i| out[i].is_date()).collect();
    for (k, &i) in date_idx.iter().enumerate() {
        if let Tok::MonthOrMinute(n) = out[i] {
            let prev = k.checked_sub(1).map(|p| &out[date_idx[p]]);
            let next = date_idx.get(k + 1).map(|&j| &out[j]);
            let minute = n <= 2
                && (matches!(prev, Some(Tok::Hour(_)) | Some(Tok::Elapsed(b'h', _)))
                    || matches!(next, Some(Tok::Second(_)) | Some(Tok::Elapsed(b's', _))));
            out[i] = if minute { Tok::Minute(n) } else { Tok::Month(n) };
        }
    }
    sec.has_ampm = out.iter().any(|t| matches!(t, Tok::AmPm(_)));
    sec.toks = out;
}

fn finish_number(sec: &mut Section, mut toks: Vec<Tok>) {
    // Vírgulas: entre dígitos = milhar; logo depois do último dígito = ÷1000.
    let mut i = 0;
    while i < toks.len() {
        if toks[i] == Tok::Comma {
            let prev_digit = i > 0 && (toks[i - 1].is_digit() || toks[i - 1] == Tok::Comma);
            let next_digit = toks.get(i + 1).is_some_and(Tok::is_digit);
            if prev_digit && next_digit {
                sec.thousands = true;
                toks.remove(i);
                continue;
            }
            if prev_digit {
                // Conta todas as vírgulas seguidas no fim.
                let mut n = 0;
                while toks.get(i + n) == Some(&Tok::Comma) {
                    n += 1;
                }
                if !toks[i + n..].iter().any(Tok::is_digit) || toks.get(i + n) == Some(&Tok::Point) {
                    sec.scale += n as i32;
                    toks.drain(i..i + n);
                    continue;
                }
            }
            toks[i] = Tok::Lit(",".into());
        }
        i += 1;
    }
    sec.percent = toks.iter().filter(|t| **t == Tok::Percent).count() as i32;
    for t in toks.iter_mut() {
        if *t == Tok::Percent {
            *t = Tok::Lit("%".into());
        }
    }

    if let Some(slash) = toks.iter().position(|t| *t == Tok::Slash)
        && toks[..slash].iter().any(Tok::is_digit)
    {
        // Fração: o último grupo de dígitos antes da barra é o numerador.
        let mut j = slash;
        while j > 0 && toks[j - 1].is_digit() {
            j -= 1;
        }
        let num_digits = slash - j;
        let whole = toks[..j].iter().any(Tok::is_digit);
        let after = &toks[slash + 1..];
        let den_digits = after.iter().take_while(|t| t.is_digit()).count();
        let fixed_den = if den_digits == 0 {
            match after.first() {
                Some(Tok::Lit(s)) => {
                    let d: String = s.chars().take_while(char::is_ascii_digit).collect();
                    d.parse().ok()
                }
                _ => None,
            }
        } else {
            None
        };
        sec.fraction = Some(Fraction { whole, num_digits, den_digits, fixed_den });
        sec.int_digits = toks[..j].iter().filter(|t| t.is_digit()).count();
        sec.toks = toks;
        return;
    }

    let mut part = 0; // 0 = inteira, 1 = decimal, 2 = expoente
    for t in &toks {
        match t {
            Tok::Point if part == 0 => part = 1,
            Tok::Point => {}
            Tok::Exp { .. } => {
                part = 2;
                sec.has_exp = true;
            }
            t if t.is_digit() => match part {
                0 => sec.int_digits += 1,
                1 => sec.frac_digits += 1,
                _ => sec.exp_digits += 1,
            },
            _ => {}
        }
    }
    // Pontos repetidos viram texto (só o primeiro é o decimal).
    let mut seen = false;
    for t in toks.iter_mut() {
        if *t == Tok::Point {
            if seen {
                *t = Tok::Lit(".".into());
            }
            seen = true;
        }
    }
    sec.toks = toks;
}

// ---------------------------------------------------------------------------
// Números
// ---------------------------------------------------------------------------

/// Arredonda `x ≥ 0` para `frac` casas, em decimal e "meio para cima" como o
/// Excel (com 15 dígitos significativos). Retorna (parte inteira sem zeros à
/// esquerda — vazia se zero —, parte decimal com exatamente `frac` dígitos).
fn round_decimal(x: f64, frac: usize) -> (String, String) {
    if x == 0.0 || !x.is_finite() {
        return (String::new(), "0".repeat(frac));
    }
    let sci = format!("{:.14e}", x);
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let digits: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    // Valor = 0.d1d2… × 10^(exp+1); manter `keep` dígitos = até `frac` casas.
    let keep = exp + 1 + frac as i64;
    let mut n: Vec<u8> = if keep <= 0 {
        if keep == 0 && digits[0] >= 5 { vec![1] } else { vec![0] }
    } else {
        let keep = keep as usize;
        let mut v: Vec<u8> = (0..keep).map(|i| digits.get(i).copied().unwrap_or(0)).collect();
        if digits.get(keep).is_some_and(|&d| d >= 5) {
            let mut i = v.len();
            loop {
                if i == 0 {
                    v.insert(0, 1);
                    break;
                }
                i -= 1;
                if v[i] == 9 {
                    v[i] = 0;
                } else {
                    v[i] += 1;
                    break;
                }
            }
        }
        v
    };
    if n.len() < frac {
        let mut pad = vec![0u8; frac - n.len()];
        pad.extend_from_slice(&n);
        n = pad;
    }
    let split = n.len() - frac;
    let int: String = n[..split].iter().skip_while(|&&d| d == 0).map(|d| (b'0' + d) as char).collect();
    let fr: String = n[split..].iter().map(|d| (b'0' + d) as char).collect();
    (int, fr)
}

/// Formato "Geral" com no máximo `max_chars` caracteres (sem contar o sinal
/// quando possível), como o Excel faz para caber na coluna.
pub fn format_general(v: f64, max_chars: usize, out: &mut String) {
    if !v.is_finite() {
        out.push_str("#NUM!");
        return;
    }
    if v == 0.0 {
        out.push('0');
        return;
    }
    let neg = v < 0.0;
    let a = v.abs();
    let budget = max_chars.max(2) - usize::from(neg);
    if neg {
        out.push('-');
    }
    let mag = a.log10().floor() as i64;
    let int_len = if mag >= 0 { mag as usize + 1 } else { 1 };
    let sci = a >= 1e11 || int_len > budget || (a < 1e-9) || (mag < 0 && (-mag) as usize + 1 >= budget);
    if sci {
        // 1,23457E+11
        let mut e = mag;
        let mant_frac = budget.saturating_sub(6).clamp(0, 5);
        let mut m = a / 10f64.powi(e as i32);
        let (mut int, mut fr) = round_decimal(m, mant_frac);
        if int.len() > 1 {
            e += 1;
            m = a / 10f64.powi(e as i32);
            (int, fr) = round_decimal(m, mant_frac);
        }
        let fr = fr.trim_end_matches('0');
        out.push_str(if int.is_empty() { "0" } else { &int });
        if !fr.is_empty() {
            out.push(',');
            out.push_str(fr);
        }
        let _ = write!(out, "E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
        return;
    }
    let frac = budget.saturating_sub(int_len + 1);
    let (int, fr) = round_decimal(a, frac);
    let fr = fr.trim_end_matches('0');
    out.push_str(if int.is_empty() { "0" } else { &int });
    if !fr.is_empty() {
        out.push(',');
        out.push_str(fr);
    }
}

/// Escreve os dígitos da parte inteira nos marcadores (`0`, `#`, `?`), com
/// separador de milhar. `int` sem zeros à esquerda (vazio = zero).
struct IntWriter<'a> {
    digits: &'a [u8],
    placeholders: usize,
    seen: usize,
    thousands: bool,
}

impl IntWriter<'_> {
    fn put(&mut self, t: &Tok, out: &mut String) {
        let pos = self.placeholders - 1 - self.seen; // posição a partir das unidades
        let n = self.digits.len();
        if self.seen == 0 && n > self.placeholders {
            // Dígitos que sobram vão antes do primeiro marcador.
            for p in (self.placeholders..n).rev() {
                out.push(self.digits[n - 1 - p] as char);
                if self.thousands && p % 3 == 0 {
                    out.push('.');
                }
            }
        }
        self.seen += 1;
        let ch = if pos < n {
            Some(self.digits[n - 1 - pos] as char)
        } else {
            match t {
                Tok::Zero => Some('0'),
                Tok::Q => Some(' '),
                _ => None,
            }
        };
        if let Some(ch) = ch {
            out.push(ch);
            if self.thousands && pos > 0 && pos.is_multiple_of(3) && ch != ' ' {
                out.push('.');
            }
        }
    }
}

fn format_digits(sec: &Section, v: f64, out: &mut String) {
    let mut x = v * 100f64.powi(sec.percent) / 1000f64.powi(sec.scale);
    if let Some(fr) = sec.fraction {
        format_fraction(sec, fr, x, out);
        return;
    }
    let mut exp = 0i64;
    if sec.has_exp {
        let n_int = sec.int_digits.max(1) as i64;
        if x != 0.0 {
            let mag = x.log10().floor() as i64;
            let eng = sec.int_digits > 1
                && sec.toks.iter().take_while(|t| **t != Tok::Point).any(|t| *t == Tok::Hash);
            exp = if eng { mag.div_euclid(n_int) * n_int } else { mag - (n_int - 1) };
            x /= 10f64.powi(exp as i32);
            // Arredondar pode virar 10.0: ajusta o expoente.
            let (int, _) = round_decimal(x, sec.frac_digits);
            if int.len() as i64 > n_int && !eng {
                exp += 1;
                x /= 10.0;
            }
        }
    }
    let (int, frac) = round_decimal(x, sec.frac_digits);
    let int = int.into_bytes();
    // Decimais com `#` no fim: zeros à direita somem.
    let frac_toks: Vec<&Tok> = {
        let mut after = false;
        sec.toks
            .iter()
            .filter(|t| {
                if **t == Tok::Point {
                    after = true;
                }
                if matches!(t, Tok::Exp { .. }) {
                    after = false;
                }
                after && t.is_digit()
            })
            .collect()
    };
    let frac = frac.into_bytes();
    let mut frac_keep = frac.len();
    while frac_keep > 0 && frac[frac_keep - 1] == b'0' && *frac_toks[frac_keep - 1] != Tok::Zero {
        frac_keep -= 1;
    }
    let mut w = IntWriter { digits: &int, placeholders: sec.int_digits, seen: 0, thousands: sec.thousands };
    let mut part = 0;
    let mut fi = 0;
    let mut ei = 0;
    let exp_str = exp.abs().to_string();
    for t in &sec.toks {
        match t {
            Tok::Lit(s) => out.push_str(s),
            Tok::Point => {
                part = 1;
                out.push(',');
            }
            Tok::Exp { plus } => {
                if part == 0 && sec.int_digits == 0 && !int.is_empty() {
                    out.push_str(std::str::from_utf8(&int).unwrap_or(""));
                }
                part = 2;
                out.push('E');
                if exp < 0 {
                    out.push('-');
                } else if *plus {
                    out.push('+');
                }
            }
            t if t.is_digit() => match part {
                0 => w.put(t, out),
                1 => {
                    if fi < frac_keep {
                        out.push(frac[fi] as char);
                    } else if *t == Tok::Q {
                        out.push(' ');
                    }
                    fi += 1;
                }
                _ => {
                    // Expoente inteiro no primeiro marcador, completado com zeros.
                    if ei == 0 {
                        for _ in exp_str.len()..sec.exp_digits {
                            out.push('0');
                        }
                        out.push_str(&exp_str);
                    }
                    ei += 1;
                }
            },
            _ => {}
        }
    }
}

fn format_fraction(sec: &Section, fr: Fraction, x: f64, out: &mut String) {
    let (mut whole, rest) = if fr.whole { (x.trunc(), x.fract()) } else { (0.0, x) };
    let (mut num, den) = match fr.fixed_den {
        Some(d) if d > 0 => ((rest * d as f64).round() as u64, d as u64),
        _ => {
            let max_den = 10u64.pow(fr.den_digits.clamp(1, 4) as u32) - 1;
            let mut best = (0u64, 1u64, f64::MAX);
            for d in 1..=max_den {
                let n = (rest * d as f64).round();
                let err = (rest - n / d as f64).abs();
                if err < best.2 - 1e-12 {
                    best = (n as u64, d, err);
                }
                if err == 0.0 {
                    break;
                }
            }
            (best.0, best.1)
        }
    };
    if fr.whole && num >= den {
        whole += (num / den) as f64;
        num %= den;
    }
    let whole_digits = if whole == 0.0 { Vec::new() } else { format!("{}", whole as u64).into_bytes() };
    let show_frac = num != 0 || !fr.whole;
    let mut w =
        IntWriter { digits: &whole_digits, placeholders: sec.int_digits, seen: 0, thousands: sec.thousands };
    let num_s = num.to_string();
    let den_s = den.to_string();
    let mut stage = 0; // 0 = inteiro, 1 = numerador, 2 = denominador
    let slash = sec.toks.iter().position(|t| *t == Tok::Slash).unwrap_or(0);
    let num_start = {
        let mut j = slash;
        while j > 0 && sec.toks[j - 1].is_digit() {
            j -= 1;
        }
        j
    };
    for (i, t) in sec.toks.iter().enumerate() {
        if i == num_start {
            stage = 1;
            if show_frac {
                let width = fr.num_digits;
                for _ in num_s.len()..width {
                    out.push(' ');
                }
                out.push_str(&num_s);
            } else {
                out.push_str(&" ".repeat(fr.num_digits));
            }
        }
        match t {
            Tok::Slash => {
                stage = 2;
                out.push(if show_frac { '/' } else { ' ' });
                if fr.fixed_den.is_none() {
                    if show_frac {
                        out.push_str(&den_s);
                        for _ in den_s.len()..fr.den_digits {
                            out.push(' ');
                        }
                    } else {
                        out.push_str(&" ".repeat(fr.den_digits));
                    }
                }
            }
            Tok::Lit(s) if stage == 2 && fr.fixed_den.is_some() => {
                if show_frac {
                    out.push_str(s);
                } else {
                    out.push_str(&" ".repeat(s.chars().count()));
                }
            }
            Tok::Lit(s) => out.push_str(s),
            t if t.is_digit() && stage == 0 => {
                if whole == 0.0 && fr.whole && !show_frac && w.seen + 1 == sec.int_digits {
                    out.push('0');
                    w.seen += 1;
                } else {
                    w.put(t, out);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Datas
// ---------------------------------------------------------------------------

const MONTHS_PT: [&str; 12] = [
    "janeiro",
    "fevereiro",
    "março",
    "abril",
    "maio",
    "junho",
    "julho",
    "agosto",
    "setembro",
    "outubro",
    "novembro",
    "dezembro",
];
const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS_PT: [&str; 7] =
    ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"];
const DAYS_EN: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// Data e hora de um número serial do Excel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// 0 = domingo
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Milissegundos.
    pub milli: u32,
}

/// Converte um serial (dias desde a época do arquivo) em data e hora.
/// `sub` = casas de fração de segundo mostradas (o resto é arredondado).
pub fn serial_to_datetime(v: f64, date1904: bool, sub: u8) -> Option<DateTime> {
    if !(0.0..2_958_466.0).contains(&v) {
        return None;
    }
    let unit = 10u64.pow(3 - sub.min(3) as u32);
    let total_ms = (v * 86_400_000.0).round() as u64;
    let total_ms = (total_ms + unit / 2) / unit * unit;
    let mut days = (total_ms / 86_400_000) as i64;
    let ms = total_ms % 86_400_000;
    let (year, month, day, weekday);
    if !date1904 && days == 60 {
        // 29/02/1900: não existe, mas o Excel (e o Lotus 1-2-3) contam ele.
        (year, month, day, weekday) = (1900, 2, 29, 3);
    } else {
        if !date1904 && days == 0 {
            // 00/01/1900
            return Some(DateTime {
                year: 1900,
                month: 1,
                day: 0,
                weekday: 6,
                hour: (ms / 3_600_000) as u32,
                minute: (ms / 60_000 % 60) as u32,
                second: (ms / 1000 % 60) as u32,
                milli: (ms % 1000) as u32,
            });
        }
        if !date1904 && days < 60 {
            days += 1; // antes do dia fictício
        }
        // Dias desde 1970-01-01.
        let epoch = if date1904 { -24_107 } else { -25_569 };
        let z = days + epoch;
        let (y, m, d) = civil_from_days(z);
        (year, month, day) = (y, m, d);
        weekday = (z + 4).rem_euclid(7) as u32;
    }
    Some(DateTime {
        year,
        month,
        day,
        weekday,
        hour: (ms / 3_600_000) as u32,
        minute: (ms / 60_000 % 60) as u32,
        second: (ms / 1000 % 60) as u32,
        milli: (ms % 1000) as u32,
    })
}

/// Algoritmo de Howard Hinnant: dias desde 1970-01-01 → (ano, mês, dia).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

fn pad(out: &mut String, v: u64, width: u8) {
    let _ = write!(out, "{:0width$}", v, width = width as usize);
}

fn format_date(sec: &Section, v: f64, date1904: bool, out: &mut String) {
    let Some(dt) = serial_to_datetime(v, date1904, sec.sub_sec) else {
        out.push_str("########");
        return;
    };
    let (months, days) = if sec.english { (MONTHS_EN, DAYS_EN) } else { (MONTHS_PT, DAYS_PT) };
    let total_secs = {
        let unit = 10f64.powi(sec.sub_sec as i32);
        ((v * 86_400.0 * unit).round() / unit).floor() as u64
    };
    for t in &sec.toks {
        match *t {
            Tok::Lit(ref s) => out.push_str(s),
            Tok::Year(n) => {
                if n <= 2 {
                    pad(out, (dt.year.rem_euclid(100)) as u64, 2);
                } else {
                    pad(out, dt.year as u64, 4);
                }
            }
            Tok::Month(n) => {
                let name = months[(dt.month.clamp(1, 12) - 1) as usize];
                match n {
                    1 => pad(out, dt.month as u64, 1),
                    2 => pad(out, dt.month as u64, 2),
                    3 => out.extend(name.chars().take(3)),
                    5 => out.extend(name.chars().take(1)),
                    _ => out.push_str(name),
                }
            }
            Tok::Day(n) => match n {
                1 => pad(out, dt.day as u64, 1),
                2 => pad(out, dt.day as u64, 2),
                3 => out.extend(days[dt.weekday as usize].chars().take(3)),
                _ => out.push_str(days[dt.weekday as usize]),
            },
            Tok::Hour(n) => {
                let h = if sec.has_ampm {
                    match dt.hour % 12 {
                        0 => 12,
                        h => h,
                    }
                } else {
                    dt.hour
                };
                pad(out, h as u64, n.min(2));
            }
            Tok::Minute(n) | Tok::MonthOrMinute(n) => pad(out, dt.minute as u64, n.min(2)),
            Tok::Second(n) => pad(out, dt.second as u64, n.min(2)),
            Tok::Elapsed(unit, n) => {
                let val = match unit {
                    b'h' => total_secs / 3600,
                    b'm' => total_secs / 60,
                    _ => total_secs,
                };
                pad(out, val, n);
            }
            Tok::SubSec(n) => {
                out.push(',');
                let digits = format!("{:03}", dt.milli);
                out.push_str(&digits[..n as usize]);
            }
            Tok::AmPm(full) => {
                let pm = dt.hour >= 12;
                out.push_str(match (full, pm) {
                    (true, false) => "AM",
                    (true, true) => "PM",
                    (false, false) => "A",
                    (false, true) => "P",
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(code: &str, v: f64) -> String {
        let mut s = String::new();
        NumFmt::parse(code).format_number(v, false, &mut s);
        s
    }

    fn color(code: &str, v: f64) -> Option<u32> {
        let mut s = String::new();
        NumFmt::parse(code).format_number(v, false, &mut s).color
    }

    #[test]
    fn general() {
        assert_eq!(f("General", 0.0), "0");
        assert_eq!(f("General", 1.5), "1,5");
        assert_eq!(f("General", -42.0), "-42");
        assert_eq!(f("General", 1.0 / 3.0), "0,333333333");
        assert_eq!(f("General", 123456.789123), "123456,7891");
        assert_eq!(f("General", 1e12), "1E+12");
        assert_eq!(f("General", 123456789012.0), "1,23457E+11");
        assert_eq!(f("General", 0.1 + 0.2), "0,3");
        assert_eq!(f("General", 1e-10), "1E-10");
        let mut s = String::new();
        format_general(1.0 / 3.0, 5, &mut s);
        assert_eq!(s, "0,333");
    }

    #[test]
    fn integers_and_decimals() {
        assert_eq!(f("0", 2.5), "3");
        assert_eq!(f("0", -2.5), "-3");
        assert_eq!(f("0.00", 1.005), "1,01");
        assert_eq!(f("0.00", 0.0), "0,00");
        assert_eq!(f("#,##0", 1234567.0), "1.234.567");
        assert_eq!(f("#,##0", 0.0), "0");
        assert_eq!(f("#,##0", 999.6), "1.000");
        assert_eq!(f("#,##0.00", 1234.5), "1.234,50");
        assert_eq!(f("#,##0.00", -1234.5), "-1.234,50");
        assert_eq!(f("#.##", 0.5), ",5");
        assert_eq!(f("#.##", 3.0), "3,");
        assert_eq!(f("0.0#", 2.0), "2,0");
        assert_eq!(f("0.0#", 2.25), "2,25");
        assert_eq!(f("000", 7.0), "007");
        assert_eq!(f("00000", 1234.0), "01234");
        assert_eq!(f("#,##0", 100.0), "100");
        assert_eq!(f("#,##0", 1000.0), "1.000");
    }

    #[test]
    fn scaling_and_percent() {
        assert_eq!(f("0%", 0.256), "26%");
        assert_eq!(f("0.00%", 0.12345), "12,35%");
        assert_eq!(f("#,##0,", 1234567.0), "1.235");
        assert_eq!(f("0.0,,\" M\"", 12_345_678.0), "12,3 M");
    }

    #[test]
    fn sections_and_colors() {
        assert_eq!(f("0;(0)", -5.0), "(5)");
        assert_eq!(f("0;(0);\"zero\"", 0.0), "zero");
        assert_eq!(f("#,##0.00;[Red]-#,##0.00", -3.0), "-3,00");
        assert_eq!(color("#,##0.00;[Red]-#,##0.00", -3.0), Some(0xFFFF0000));
        assert_eq!(color("#,##0.00;[Red]-#,##0.00", 3.0), None);
        assert_eq!(f("[Blue]0", -1.0), "-1");
        assert_eq!(color("[Color10]0", 1.0), Some(INDEXED[17]));
        assert_eq!(f("[>=100]\"alto\";[<0]\"neg\";0", 150.0), "alto");
        assert_eq!(f("[>=100]\"alto\";[<0]\"neg\";0", -1.0), "neg");
        assert_eq!(f("[>=100]\"alto\";[<0]\"neg\";0", 50.0), "50");
        assert_eq!(f(";;;", 5.0), "");
    }

    #[test]
    fn currency_and_accounting() {
        assert_eq!(f("[$R$-416] #,##0.00", 1234.5), "R$ 1.234,50");
        assert_eq!(f("\"R$\" #,##0.00", -2.0), "-R$ 2,00");
        assert_eq!(f(builtin(8).unwrap(), -10.0), "-R$ 10,00");
        assert_eq!(color(builtin(8).unwrap(), -10.0), Some(0xFFFF0000));
        assert_eq!(f(builtin(44).unwrap(), 1234.5), " R$ 1.234,50 ");
        assert_eq!(f(builtin(44).unwrap(), 0.0), " R$ -   ");
        assert_eq!(f(builtin(43).unwrap(), -7.0), "-7,00 ");
        let mut s = String::new();
        let r = NumFmt::parse(builtin(44).unwrap()).format_number(5.0, false, &mut s);
        assert_eq!((s.as_str(), r.fill_at), (" R$ 5,00 ", Some(4)));
    }

    #[test]
    fn scientific() {
        assert_eq!(f("0.00E+00", 12345.0), "1,23E+04");
        assert_eq!(f("0.00E+00", 0.000123), "1,23E-04");
        assert_eq!(f("0.00E+00", 9.999e5), "1,00E+06");
        assert_eq!(f("##0.0E+0", 12345.0), "12,3E+3");
    }

    #[test]
    fn fractions() {
        assert_eq!(f("# ?/?", 1.5), "1 1/2");
        assert_eq!(f("# ??/??", 0.75), "  3/4 ");
        assert_eq!(f("# ?/8", 2.375), "2 3/8");
        assert_eq!(f("# ?/?", 3.0), "3    ");
    }

    #[test]
    fn dates() {
        // 45000 = 15/03/2023 (quarta-feira)
        assert_eq!(f("dd/mm/yyyy", 45000.0), "15/03/2023");
        assert_eq!(f("d/m/yy", 45000.0), "15/3/23");
        assert_eq!(f("dddd, d \"de\" mmmm \"de\" yyyy", 45000.0), "quarta-feira, 15 de março de 2023");
        assert_eq!(f("ddd mmm", 45000.0), "qua mar");
        assert_eq!(f("[$-409]mmmm", 45000.0), "March");
        assert_eq!(f("mmmmm", 45000.0), "m");
        assert_eq!(f("dd/mm/yyyy hh:mm", 45000.75), "15/03/2023 18:00");
        assert_eq!(f("h:mm AM/PM", 0.75), "6:00 PM");
        assert_eq!(f("h:mm:ss", 0.5 + 1.0 / 86400.0 * 0.6), "12:00:01");
        assert_eq!(f("hh:mm:ss.00", 0.5 + 0.25 / 86400.0), "12:00:00,25");
        assert_eq!(f("[h]:mm:ss", 1.5), "36:00:00");
        assert_eq!(f("mm:ss", 1.0 / 1440.0 * 2.5), "02:30");
        assert_eq!(f("dd/mm/yyyy", 1.0), "01/01/1900");
        assert_eq!(f("dd/mm/yyyy", 59.0), "28/02/1900");
        assert_eq!(f("dd/mm/yyyy", 60.0), "29/02/1900");
        assert_eq!(f("dd/mm/yyyy", 61.0), "01/03/1900");
        assert_eq!(f(builtin(14).unwrap(), 46344.0), "18/11/2026");
        let mut s = String::new();
        let r = NumFmt::parse("dd/mm/yyyy").format_number(-1.0, false, &mut s);
        assert!(r.overflow);
        let mut s = String::new();
        NumFmt::parse("dd/mm/yyyy").format_number(0.0, true, &mut s);
        assert_eq!(s, "01/01/1904");
        assert!(NumFmt::parse("dd/mm/yyyy").is_date());
        assert!(!NumFmt::parse("#,##0").is_date());
        assert!(!NumFmt::parse("[Red]0").is_date());
    }

    #[test]
    fn text_sections() {
        let fmt = NumFmt::parse("0;-0;0;\"Texto: \"@");
        let mut s = String::new();
        fmt.format_text("abc", &mut s);
        assert_eq!(s, "Texto: abc");
        let mut s = String::new();
        assert!(NumFmt::parse("0.00").format_text("abc", &mut s).is_none());
        assert_eq!(f("@", 12.5), "12,5");
    }

    #[test]
    fn literals_and_escapes() {
        assert_eq!(f("0\" kg\"", 5.0), "5 kg");
        assert_eq!(f("\\(0\\)", 5.0), "(5)");
        assert_eq!(f("0_)", 5.0), "5 ");
        assert_eq!(f("00\\-000", 12345.0), "12-345");
        assert_eq!(f("\"Total: \"General", 2.5), "Total: 2,5");
    }
}
