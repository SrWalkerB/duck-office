//! O Modelo de Dados em colunas: cada coluna é um dicionário de valores e um
//! código por linha, montada só quando alguma conta precisa dela.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::parse::{self, Node};

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Blank,
    Num(f64),
    Str(Arc<str>),
    Bool(bool),
}

impl Val {
    pub fn str(s: &str) -> Val {
        Val::Str(s.into())
    }

    /// Chave de igualdade: o texto não diferencia maiúsculas.
    pub fn key(&self) -> Key {
        match self {
            Val::Blank => Key::Blank,
            Val::Num(n) => Key::Num((if *n == 0.0 { 0.0 } else { *n }).to_bits()),
            Val::Str(s) => Key::Str(s.to_lowercase()),
            Val::Bool(b) => Key::Bool(*b),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Blank,
    Num(u64),
    Str(String),
    Bool(bool),
}

/// Conjunto de códigos de uma coluna.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Bits(Vec<u64>);

impl Bits {
    pub fn new(n: usize) -> Self {
        Bits(vec![0; n.div_ceil(64)])
    }

    pub fn set(&mut self, i: u32) {
        self.0[(i / 64) as usize] |= 1 << (i % 64);
    }

    #[inline]
    pub fn get(&self, i: u32) -> bool {
        self.0.get((i / 64) as usize).is_some_and(|w| w >> (i % 64) & 1 == 1)
    }

    pub fn and(&mut self, o: &Bits) {
        for (a, b) in self.0.iter_mut().zip(&o.0) {
            *a &= b;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &bits)| {
            let mut b = bits;
            std::iter::from_fn(move || {
                if b == 0 {
                    return None;
                }
                let k = b.trailing_zeros();
                b &= b - 1;
                Some(w as u32 * 64 + k)
            })
        })
    }
}

/// Linhas de cada código (para varrer só um grupo).
pub struct Index {
    pub start: Vec<u32>,
    pub rows: Vec<u32>,
}

pub struct Column {
    pub codes: Vec<u32>,
    pub dict: Vec<Val>,
    /// Posição de cada código na ordem dos valores.
    pub order: Vec<u32>,
    pub map: HashMap<Key, u32>,
    index: OnceLock<Index>,
}

impl Column {
    pub fn new(vals: Vec<Val>) -> Self {
        let mut map: HashMap<Key, u32> = HashMap::new();
        let mut dict = Vec::new();
        let mut codes = Vec::with_capacity(vals.len());
        // Valores repetidos em seguida são comuns: evita a chave.
        let mut last: Option<(Val, u32)> = None;
        for v in vals {
            if let Some((l, c)) = &last
                && *l == v
            {
                codes.push(*c);
                continue;
            }
            let c = *map.entry(v.key()).or_insert_with(|| {
                dict.push(v.clone());
                dict.len() as u32 - 1
            });
            codes.push(c);
            last = Some((v, c));
        }
        let mut sorted: Vec<u32> = (0..dict.len() as u32).collect();
        sorted.sort_by(|&a, &b| compare(&dict[a as usize], &dict[b as usize]));
        let mut order = vec![0; dict.len()];
        for (p, c) in sorted.into_iter().enumerate() {
            order[c as usize] = p as u32;
        }
        Column { codes, dict, order, map, index: OnceLock::new() }
    }

    pub fn index(&self) -> &Index {
        self.index.get_or_init(|| {
            let mut start = vec![0u32; self.dict.len() + 1];
            for &c in &self.codes {
                start[c as usize + 1] += 1;
            }
            for i in 1..start.len() {
                start[i] += start[i - 1];
            }
            let mut at = start.clone();
            let mut rows = vec![0u32; self.codes.len()];
            for (r, &c) in self.codes.iter().enumerate() {
                rows[at[c as usize] as usize] = r as u32;
                at[c as usize] += 1;
            }
            Index { start, rows }
        })
    }

    /// Conjunto dos códigos cujos valores passam em `f`.
    pub fn select(&self, f: impl Fn(&Val) -> bool) -> Bits {
        let mut b = Bits::new(self.dict.len());
        for (i, v) in self.dict.iter().enumerate() {
            if f(v) {
                b.set(i as u32);
            }
        }
        b
    }
}

/// Ordem dos membros: vazio, números, textos (sem maiúsculas nem acentos, e
/// depois com eles), lógicos.
pub fn compare(a: &Val, b: &Val) -> std::cmp::Ordering {
    let rank = |v: &Val| match v {
        Val::Blank => 0,
        Val::Num(_) => 1,
        Val::Str(_) => 2,
        Val::Bool(_) => 3,
    };
    match (a, b) {
        (Val::Num(x), Val::Num(y)) => x.total_cmp(y),
        (Val::Str(x), Val::Str(y)) => collate(x, y),
        (Val::Bool(x), Val::Bool(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// Comparação de textos como a do Windows em português: primeiro as letras
/// base (pontuação antes de números antes de letras), depois os acentos.
pub fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    let weight = |s: &str| -> Vec<(u8, char)> {
        s.chars()
            .flat_map(char::to_lowercase)
            .map(|c| {
                let c = base(c);
                let class = if c.is_whitespace() {
                    0
                } else if c.is_alphabetic() {
                    3
                } else if c.is_numeric() {
                    2
                } else {
                    1
                };
                (class, c)
            })
            .collect()
    };
    weight(a).cmp(&weight(b)).then_with(|| a.to_lowercase().cmp(&b.to_lowercase())).then_with(|| b.cmp(a))
}

fn base(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        c => c,
    }
}

/// Tipo da coluna no modelo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Any,
    Text,
    Number,
    Date,
    Bool,
}

pub struct ColumnDef {
    pub name: String,
    pub kind: Kind,
    /// Coluna calculada.
    pub expr: Option<Result<Node, String>>,
    /// Coluna que dá a ordem dos membros.
    pub sort_by: Option<usize>,
}

pub struct Table {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    data: Vec<OnceLock<Arc<Column>>>,
}

/// Relação: muitos (`from`) para um (`to`).
#[derive(Clone, Copy, Debug)]
pub struct Rel {
    pub from: (usize, usize),
    pub to: (usize, usize),
}

/// Código da coluna "muitos" → linha da tabela "um".
pub struct Join {
    pub row_of: Vec<Option<u32>>,
}

pub struct Measure {
    pub table: String,
    pub name: String,
    pub body: Result<Node, String>,
}

/// De onde vêm os dados das colunas (as tabelas das abas).
pub trait Source: Sync + Send {
    fn rows(&self, table: usize) -> usize;
    fn column(&self, table: usize, col: usize) -> Vec<Val>;
}

/// Definição de uma tabela: nome e colunas (nome, tipo, fórmula).
pub type TableDef = (String, Vec<(String, Kind, Option<String>)>);

pub struct Model {
    pub tables: Vec<Table>,
    pub rels: Vec<Rel>,
    joins: Vec<OnceLock<Join>>,
    pub measures: HashMap<String, Measure>,
    /// Ligada só durante as contas (`attach`/`detach`).
    source: RwLock<Option<Box<dyn Source>>>,
    rows: Vec<usize>,
    /// Data de hoje (serial).
    pub today: f64,
    pub now: f64,
}

impl Model {
    pub fn new(
        defs: Vec<TableDef>,
        rels: &[(String, String, String, String)],
        measures: Vec<(String, String, String)>,
        source: Box<dyn Source>,
        now: f64,
    ) -> Self {
        let tables: Vec<Table> = defs
            .into_iter()
            .map(|(name, cols)| Table {
                data: cols.iter().map(|_| OnceLock::new()).collect(),
                columns: cols
                    .into_iter()
                    .map(|(name, kind, expr)| ColumnDef {
                        name,
                        kind,
                        expr: expr.map(|e| parse::parse(&e)),
                        sort_by: None,
                    })
                    .collect(),
                name,
            })
            .collect();
        let find = |t: &str, c: &str| -> Option<(usize, usize)> {
            let ti = tables.iter().position(|x| x.name.to_lowercase() == t.to_lowercase())?;
            let ci = tables[ti].columns.iter().position(|x| x.name.to_lowercase() == c.to_lowercase())?;
            Some((ti, ci))
        };
        let rels: Vec<Rel> = rels
            .iter()
            .filter_map(|(ft, fc, tt, tc)| Some(Rel { from: find(ft, fc)?, to: find(tt, tc)? }))
            .collect();
        Model {
            joins: rels.iter().map(|_| OnceLock::new()).collect(),
            rels,
            measures: measures
                .into_iter()
                .map(|(table, name, dax)| {
                    (name.to_lowercase(), Measure { body: parse::parse(&dax), table, name })
                })
                .collect(),
            rows: (0..tables.len()).map(|t| source.rows(t)).collect(),
            tables,
            source: RwLock::new(Some(source)),
            today: now.floor(),
            now,
        }
    }

    /// Define a coluna que dá a ordem de outra.
    pub fn sort_by(&mut self, table: &str, col: &str, by: &str) {
        if let (Some((t, c)), Some((_, b))) = (self.find(table, col), self.find(table, by)) {
            self.tables[t].columns[c].sort_by = Some(b);
        }
    }

    /// Posição de cada código na ordem de exibição da coluna.
    pub fn display_order(&self, t: usize, c: usize) -> Result<Vec<u32>, String> {
        let col = self.column(t, c)?;
        let Some(by) = self.tables[t].columns[c].sort_by else { return Ok(col.order.clone()) };
        let key = self.column(t, by)?;
        let ix = col.index();
        // O valor da outra coluna na primeira linha de cada código.
        let mut codes: Vec<u32> = (0..col.dict.len() as u32).collect();
        codes.sort_by_key(|&k| {
            let row = ix.rows.get(ix.start[k as usize] as usize).copied();
            (row.map_or(u32::MAX, |r| key.order[key.codes[r as usize] as usize]), col.order[k as usize])
        });
        let mut out = vec![0; codes.len()];
        for (p, k) in codes.into_iter().enumerate() {
            out[k as usize] = p as u32;
        }
        Ok(out)
    }

    pub fn table(&self, name: &str) -> Option<usize> {
        let n = name.to_lowercase();
        self.tables.iter().position(|t| t.name.to_lowercase() == n)
    }

    pub fn find(&self, table: &str, col: &str) -> Option<(usize, usize)> {
        let t = self.table(table)?;
        let c = col.to_lowercase();
        Some((t, self.tables[t].columns.iter().position(|x| x.name.to_lowercase() == c)?))
    }

    pub fn rows(&self, t: usize) -> usize {
        self.rows[t]
    }

    /// Liga a origem dos dados (a mesma da montagem do modelo).
    pub fn attach(&self, source: Box<dyn Source>) {
        *self.source.write().unwrap() = Some(source);
    }

    /// Solta a origem: as colunas já lidas ficam.
    pub fn detach(&self) {
        *self.source.write().unwrap() = None;
    }

    fn read(&self, t: usize, c: usize) -> Result<Vec<Val>, String> {
        let s = self.source.read().unwrap();
        let mut v = s.as_ref().ok_or("modelo sem os dados")?.column(t, c);
        v.resize(self.rows[t], Val::Blank);
        Ok(v)
    }

    /// Coluna pronta (monta na primeira vez).
    pub fn column(&self, t: usize, c: usize) -> Result<Arc<Column>, String> {
        let slot = &self.tables[t].data[c];
        if let Some(col) = slot.get() {
            return Ok(col.clone());
        }
        let def = &self.tables[t].columns[c];
        let vals = match &def.expr {
            None => convert(self.read(t, c)?, def.kind),
            Some(Err(e)) => return Err(format!("coluna calculada: {e}")),
            Some(Ok(n)) => crate::eval::calc_column(self, t, n)?,
        };
        Ok(slot.get_or_init(|| Arc::new(Column::new(vals))).clone())
    }

    pub fn join(&self, r: usize) -> Result<&Join, String> {
        if let Some(j) = self.joins[r].get() {
            return Ok(j);
        }
        let rel = self.rels[r];
        let from = self.column(rel.from.0, rel.from.1)?;
        let to = self.column(rel.to.0, rel.to.1)?;
        let mut row_of_key: HashMap<u32, u32> = HashMap::with_capacity(to.codes.len());
        for (row, &c) in to.codes.iter().enumerate() {
            row_of_key.entry(c).or_insert(row as u32);
        }
        let row_of =
            from.dict.iter().map(|v| to.map.get(&v.key()).and_then(|c| row_of_key.get(c)).copied()).collect();
        Ok(self.joins[r].get_or_init(|| Join { row_of }))
    }
}

/// Valores de uma coluna ligada no tipo dela no modelo.
fn convert(vals: Vec<Val>, kind: Kind) -> Vec<Val> {
    match kind {
        Kind::Any | Kind::Bool => vals,
        Kind::Text => vals
            .into_iter()
            .map(|v| match v {
                // O modelo tira os espaços do fim dos textos.
                Val::Str(s) if s.ends_with(' ') => Val::str(s.trim_end_matches(' ')),
                Val::Num(n) => Val::Str(num_text(n).into()),
                Val::Bool(b) => Val::str(if b { "TRUE" } else { "FALSE" }),
                v => v,
            })
            .collect(),
        Kind::Number | Kind::Date => vals
            .into_iter()
            .map(|v| match v {
                Val::Num(_) => v,
                Val::Bool(b) => Val::Num(b as u8 as f64),
                _ => Val::Blank,
            })
            .collect(),
    }
}

/// Número como texto (vírgula decimal, como o modelo em português).
pub fn num_text(n: f64) -> String {
    let mut s = String::new();
    duck_core::numfmt::format_general(n, 15, &mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_and_order() {
        let c = Column::new(vec![Val::str("b"), Val::str("A"), Val::str("B"), Val::Blank, Val::Num(3.0)]);
        assert_eq!(c.codes, [0, 1, 0, 2, 3]);
        // vazio, 3, A, b.
        assert_eq!(c.order, [3, 2, 0, 1]);
        let ix = c.index();
        assert_eq!(&ix.rows[ix.start[0] as usize..ix.start[1] as usize], [0, 2]);
        let b = c.select(|v| matches!(v, Val::Str(_)));
        assert_eq!(b.iter().collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn collation() {
        let mut v = vec!["zebra", "Ábaco", "abaco", "10", "-x", "Abd", "ação"];
        v.sort_by(|a, b| collate(a, b));
        assert_eq!(v, ["-x", "10", "abaco", "Ábaco", "Abd", "ação", "zebra"]);
    }

    #[test]
    fn kinds() {
        let v = convert(vec![Val::Num(1.5), Val::str("x"), Val::Bool(true)], Kind::Text);
        assert_eq!(v, [Val::str("1,5"), Val::str("x"), Val::str("TRUE")]);
        let v = convert(vec![Val::Num(1.5), Val::str("")], Kind::Date);
        assert_eq!(v, [Val::Num(1.5), Val::Blank]);
    }
}
