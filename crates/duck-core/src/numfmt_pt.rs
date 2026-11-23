//! Códigos de formato como o Excel em português mostra e aceita
//! (`#.##0,00`, `dd/mm/aaaa`, `[Vermelho]`, `Geral`) ↔ o código do arquivo
//! (`#,##0.00`, `dd/mm/yyyy`, `[Red]`, `General`).

const COLORS: [(&str, &str); 8] = [
    ("Black", "Preto"),
    ("Blue", "Azul"),
    ("Cyan", "Ciano"),
    ("Green", "Verde"),
    ("Magenta", "Magenta"),
    ("Red", "Vermelho"),
    ("White", "Branco"),
    ("Yellow", "Amarelo"),
];

/// Uma seção mostra data? (letras de data fora de aspas e colchetes.)
fn is_date(section: &str, pt: bool) -> bool {
    let mut date = false;
    let _ = scan(section, &mut |p, _| {
        if let Piece::Char(c) = p {
            let c = c.to_ascii_lowercase();
            date |= matches!(c, 'd' | 'm' | 'h') || c == if pt { 'a' } else { 'y' };
        }
    });
    date
}

enum Piece<'a> {
    /// Fora de aspas, colchetes e escapes.
    Char(char),
    /// Literal (aspas, `\x`, `_x`, `*x`): fica como está.
    Raw(&'a str),
    /// `[...]`, sem os colchetes.
    Bracket(&'a str),
}

/// Percorre o código; devolve falso se aspas ou colchetes ficaram abertos.
fn scan<'a>(code: &'a str, f: &mut dyn FnMut(Piece<'a>, usize)) -> bool {
    let mut it = code.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => {
                let Some(e) = code[i + 1..].find('"').map(|k| i + 1 + k) else { return false };
                while it.peek().is_some_and(|x| x.0 <= e) {
                    it.next();
                }
                f(Piece::Raw(&code[i..=e]), i);
            }
            '[' => {
                let Some(e) = code[i + 1..].find(']').map(|k| i + 1 + k) else { return false };
                while it.peek().is_some_and(|x| x.0 <= e) {
                    it.next();
                }
                f(Piece::Bracket(&code[i + 1..e]), i);
            }
            '\\' | '_' | '*' => {
                let end = match it.next() {
                    Some((k, n)) => k + n.len_utf8(),
                    None => return false,
                };
                f(Piece::Raw(&code[i..end]), i);
            }
            c => f(Piece::Char(c), i),
        }
    }
    true
}

fn sections(code: &str) -> Vec<&str> {
    let mut cuts = vec![0];
    let mut f = |p: Piece, i: usize| {
        if let Piece::Char(';') = p {
            cuts.push(i);
        }
    };
    scan(code, &mut f);
    let n = cuts.len();
    (0..n)
        .map(|k| {
            let a = if k == 0 { 0 } else { cuts[k] + 1 };
            let b = cuts.get(k + 1).copied().unwrap_or(code.len());
            &code[a..b]
        })
        .collect()
}

fn bracket(b: &str, to_pt: bool) -> String {
    for (en, pt) in COLORS {
        let (from, to) = if to_pt { (en, pt) } else { (pt, en) };
        if b.eq_ignore_ascii_case(from) || (!to_pt && b.eq_ignore_ascii_case(en)) {
            return to.to_string();
        }
    }
    let lower = b.to_lowercase();
    if to_pt {
        if let Some(n) = lower.strip_prefix("color") {
            return format!("Cor{}", n.trim());
        }
    } else if let Some(n) = lower.strip_prefix("cor").or_else(|| lower.strip_prefix("color")) {
        return format!("Color{}", n.trim());
    }
    // Condições: `[>=1,5]`.
    if b.starts_with(['<', '>', '=']) {
        return if to_pt { b.replace('.', ",") } else { b.replace(',', ".") };
    }
    b.to_string()
}

fn convert(code: &str, to_pt: bool) -> Option<String> {
    let mut out = String::with_capacity(code.len());
    let parts = sections(code);
    for (k, s) in parts.iter().enumerate() {
        if k > 0 {
            out.push(';');
        }
        let (from, to) = if to_pt { ("general", "Geral") } else { ("geral", "General") };
        let date = is_date(s, !to_pt) && find_word(s, from).is_none() && find_word(s, "general").is_none();
        let ok = scan(s, &mut |p, _| match p {
            Piece::Raw(r) => out.push_str(r),
            Piece::Bracket(b) => {
                out.push('[');
                out.push_str(&bracket(b, to_pt));
                out.push(']');
            }
            Piece::Char(c) if date => out.push(match (c, to_pt) {
                ('y', true) => 'a',
                ('Y', true) => 'A',
                ('a' | 'A', false) => 'y',
                // Fração de segundo.
                ('.', true) => '.',
                (',', false) => '.',
                (c, _) => c,
            }),
            Piece::Char(c) => out.push(match c {
                '.' => ',',
                ',' => '.',
                c => c,
            }),
        });
        if !ok {
            return None;
        }
        // `Geral` / `General` inteiros (o laço acima pode ter mexido neles).
        let start = out.len() - out.rsplit(';').next().unwrap_or("").len();
        let sec = out[start..].to_string();
        if let Some(i) = find_word(&sec, from) {
            out.truncate(start);
            out.push_str(&sec[..i]);
            out.push_str(to);
            out.push_str(&sec[i + from.len()..]);
        }
    }
    // `AM/PM` tem letras de data: o laço troca o A por y; desfaz.
    if !to_pt {
        for (bad, good) in [("yM/PM", "AM/PM"), ("ym/pm", "AM/PM"), ("y/P", "A/P"), ("y/p", "A/P")] {
            out = out.replace(bad, good);
        }
    }
    Some(out)
}

/// Posição de uma palavra (sem diferenciar maiúsculas) fora de aspas.
fn find_word(s: &str, word: &str) -> Option<usize> {
    let lower = s.to_lowercase();
    if lower.len() != s.len() {
        return None;
    }
    let i = lower.find(word)?;
    let quotes = s[..i].matches('"').count();
    quotes.is_multiple_of(2).then_some(i)
}

/// O código do arquivo como o Excel em português mostra.
pub fn to_pt(code: &str) -> String {
    convert(code, true).unwrap_or_else(|| code.to_string())
}

/// O que o usuário digitou → código do arquivo. Erro se aspas ou colchetes
/// ficaram abertos, ou se não há o que mostrar.
pub fn from_pt(code: &str) -> Result<String, String> {
    let code = code.trim();
    if code.is_empty() {
        return Err("Digite um código de formato".into());
    }
    if sections(code).len() > 4 {
        return Err("Um formato tem no máximo quatro partes separadas por ;".into());
    }
    convert(code, false).ok_or_else(|| "Código de formato incompleto (aspas, colchetes ou \\ no fim)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for (en, pt) in [
            ("General", "Geral"),
            ("#,##0.00", "#.##0,00"),
            ("0.00%", "0,00%"),
            ("dd/mm/yyyy", "dd/mm/aaaa"),
            ("d-mmm-yy", "d-mmm-aa"),
            ("dd/mm/yyyy hh:mm", "dd/mm/aaaa hh:mm"),
            ("[h]:mm:ss", "[h]:mm:ss"),
            ("mm:ss.0", "mm:ss.0"),
            ("\"R$ \"#,##0.00;[Red]\\-\"R$ \"#,##0.00", "\"R$ \"#.##0,00;[Vermelho]\\-\"R$ \"#.##0,00"),
            ("[>=1.5]0.0;[Color10]General", "[>=1,5]0,0;[Cor10]Geral"),
            (
                "_-* #,##0.00_-;-* #,##0.00_-;_-* \"-\"??_-;_-@_-",
                "_-* #.##0,00_-;-* #.##0,00_-;_-* \"-\"??_-;_-@_-",
            ),
            ("\"Total: \"General", "\"Total: \"Geral"),
            ("0.00E+00", "0,00E+00"),
            ("h:mm AM/PM", "h:mm AM/PM"),
            ("@", "@"),
        ] {
            assert_eq!(to_pt(en), pt);
            assert_eq!(from_pt(pt).as_deref(), Ok(en), "{pt}");
        }
    }

    #[test]
    fn errors() {
        assert!(from_pt("").is_err());
        assert!(from_pt("0\"aberto").is_err());
        assert!(from_pt("[Vermelho 0").is_err());
        assert!(from_pt("0;0;0;@;0").is_err());
        // O código em inglês também serve.
        assert_eq!(from_pt("[Red]0").as_deref(), Ok("[Red]0"));
    }
}
