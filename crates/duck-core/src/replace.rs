//! Localizar e substituir texto (Ctrl+H): o padrão e a troca numa célula.
//!
//! Como no Excel: `*` vale qualquer trecho, `?` um caractere, e `~` antes
//! deles (ou dele mesmo) tira o significado. Sem [`Options::exact`],
//! maiúsculas e acentos não fazem diferença (como na busca).

/// Minúscula sem acento de um caractere (a forma usada para comparar).
pub fn fold_char(c: char) -> char {
    let c = match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        'ñ' | 'Ñ' => 'n',
        c => c,
    };
    c.to_lowercase().next().unwrap_or(c)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Diferencia maiúsculas e acentos.
    pub exact: bool,
    /// A célula inteira precisa bater.
    pub whole: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Char(char),
    Any,
    Star,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    toks: Vec<Tok>,
    opts: Options,
}

impl Pattern {
    /// `None` se o texto procurado é vazio.
    pub fn new(find: &str, opts: Options) -> Option<Self> {
        let mut toks = Vec::new();
        let mut it = find.chars().peekable();
        while let Some(c) = it.next() {
            let t = match c {
                '~' if matches!(it.peek(), Some('*' | '?' | '~')) => Tok::Char(it.next().unwrap_or('~')),
                '*' => Tok::Star,
                '?' => Tok::Any,
                c => Tok::Char(c),
            };
            let t = match t {
                Tok::Char(c) if !opts.exact => Tok::Char(fold_char(c)),
                t => t,
            };
            if !(t == Tok::Star && toks.last() == Some(&Tok::Star)) {
                toks.push(t);
            }
        }
        (!toks.is_empty()).then_some(Self { toks, opts })
    }

    fn same(&self, a: char, b: char) -> bool {
        a == if self.opts.exact { b } else { fold_char(b) }
    }

    /// Fim (em caracteres) da menor ocorrência que começa em `i`.
    fn match_at(&self, text: &[char], i: usize, to_end: bool) -> Option<usize> {
        self.go(text, i, 0, to_end)
    }

    fn go(&self, text: &[char], i: usize, k: usize, to_end: bool) -> Option<usize> {
        let Some(t) = self.toks.get(k) else {
            return (!to_end || i == text.len()).then_some(i);
        };
        match *t {
            Tok::Char(c) => {
                (i < text.len() && self.same(c, text[i])).then(|| self.go(text, i + 1, k + 1, to_end))?
            }
            Tok::Any => (i < text.len()).then(|| self.go(text, i + 1, k + 1, to_end))?,
            Tok::Star => {
                // No fim do padrão, `*` vai até o fim do texto.
                if k + 1 == self.toks.len() {
                    return Some(text.len());
                }
                (i..=text.len()).find_map(|j| self.go(text, j, k + 1, to_end))
            }
        }
    }

    /// Ocorrências (início, fim) em caracteres, sem sobreposição.
    fn matches(&self, text: &[char]) -> Vec<(usize, usize)> {
        if self.opts.whole {
            return match self.match_at(text, 0, true) {
                Some(e) if e == text.len() => vec![(0, e)],
                _ => Vec::new(),
            };
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < text.len() {
            match self.match_at(text, i, false) {
                Some(e) if e > i => {
                    out.push((i, e));
                    i = e;
                }
                _ => i += 1,
            }
        }
        out
    }

    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        !self.matches(&chars).is_empty()
    }

    /// O texto com as ocorrências trocadas e quantas foram.
    pub fn replace(&self, text: &str, with: &str) -> Option<(String, usize)> {
        let chars: Vec<char> = text.chars().collect();
        let m = self.matches(&chars);
        if m.is_empty() {
            return None;
        }
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        for &(a, b) in &m {
            out.extend(&chars[pos..a]);
            out.push_str(with);
            pos = b;
        }
        out.extend(&chars[pos..]);
        Some((out, m.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(find: &str, with: &str, text: &str, opts: Options) -> Option<(String, usize)> {
        Pattern::new(find, opts).unwrap().replace(text, with)
    }

    #[test]
    fn replaces() {
        let d = Options::default();
        assert_eq!(rep("acao", "X", "Ação e reAÇÃO", d), Some(("X e reX".into(), 2)));
        let exact = Options { exact: true, ..d };
        assert_eq!(rep("acao", "X", "Ação acao", exact), Some(("Ação X".into(), 1)));
        let whole = Options { whole: true, ..d };
        assert_eq!(rep("sp", "SP", "sp", whole), Some(("SP".into(), 1)));
        assert_eq!(rep("sp", "SP", "sp 2", whole), None);
        assert_eq!(rep("a", "", "banana", d), Some(("bnn".into(), 3)));
        assert_eq!(rep("x", "y", "abc", d), None);
        assert!(Pattern::new("", d).is_none());
    }

    #[test]
    fn wildcards() {
        let d = Options::default();
        assert_eq!(rep("a?c", "-", "abc aXc ac", d), Some(("- - ac".into(), 2)));
        assert_eq!(rep("(*)", "", "Loja (SP) e (RJ)", d), Some(("Loja  e ".into(), 2)));
        assert_eq!(rep("b*", "!", "aabcd", d), Some(("aa!".into(), 1)));
        assert_eq!(rep("~*", "x", "2*3", d), Some(("2x3".into(), 1)));
        assert_eq!(rep("~?", ".", "ok?", d), Some(("ok.".into(), 1)));
        assert_eq!(rep("a~b", "x", "a~b", d), Some(("x".into(), 1)));
        let whole = Options { whole: true, ..d };
        assert_eq!(rep("*", "novo", "qualquer", whole), Some(("novo".into(), 1)));
        assert_eq!(rep("a*z", "novo", "a até z", whole), Some(("novo".into(), 1)));
        assert_eq!(rep("a*z", "novo", "a até z.", whole), None);
    }
}
