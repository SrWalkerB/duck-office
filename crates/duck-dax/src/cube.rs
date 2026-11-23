//! Consulta de uma tabela dinâmica ao modelo: os membros de cada eixo que
//! têm valor, na ordem de exibição, e os valores das medidas em cada
//! cruzamento (com os totais avaliados, não somados).

use std::collections::HashMap;

use rayon::prelude::*;

use crate::eval::{Ctx, Eval};
use crate::model::{Bits, Model, Val, compare, num_text};

type Res<T> = Result<T, String>;
type Col = (usize, usize);

/// Um campo num eixo.
#[derive(Clone, Debug)]
pub struct Level {
    pub col: Col,
    /// Ordenar por uma medida: (medida em `data`, decrescente, membros do
    /// outro eixo).
    pub sort: Option<Sort>,
    /// Ordem natural ao contrário.
    pub descending: bool,
    /// Ordem própria: chaves na ordem de exibição (os outros vão no fim).
    pub manual: Vec<String>,
    /// Membros recolhidos (chaves): os níveis de dentro não aparecem.
    pub collapsed: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Sort {
    pub data: usize,
    pub descending: bool,
    /// Filtros do outro eixo em que o valor é lido.
    pub at: Vec<(Col, String)>,
}

impl Level {
    pub fn new(col: Col) -> Self {
        Level { col, sort: None, descending: false, manual: Vec::new(), collapsed: Vec::new() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub rows: Vec<Level>,
    pub cols: Vec<Level>,
    /// Medidas.
    pub data: Vec<String>,
    /// Membros escolhidos (segmentações, filtros de página): coluna e chaves.
    pub filters: Vec<(Col, Vec<String>)>,
}

/// Uma linha (ou coluna) do resultado: os membros de cada nível, como
/// posições em `Axis::members`. Menos níveis que o eixo = recolhida.
pub type Path = Vec<u32>;

#[derive(Clone, Debug, Default)]
pub struct Axis {
    /// Membros de cada nível, na ordem natural.
    pub members: Vec<Vec<Val>>,
    pub paths: Vec<Path>,
}

#[derive(Clone, Debug, Default)]
pub struct Answer {
    pub rows: Axis,
    pub cols: Axis,
    /// (linha, coluna, medida) → valor; caminho vazio = total geral.
    pub values: HashMap<(Path, Path, u32), Val>,
}

/// Chave de um membro no nome único (`[T].[c].&[chave]`).
pub fn member_key(v: &Val, date: bool) -> String {
    match v {
        Val::Blank => String::new(),
        Val::Str(s) => s.to_string(),
        Val::Bool(b) => if *b { "True" } else { "False" }.into(),
        Val::Num(n) if date => {
            let t = duck_core::numfmt::serial_to_datetime(*n, false, 0);
            t.map_or_else(
                || n.to_string(),
                |t| {
                    format!(
                        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                        t.year, t.month, t.day, t.hour, t.minute, t.second
                    )
                },
            )
        }
        Val::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
        Val::Num(n) => {
            // Notação do modelo: `1.5E0`.
            let mut s = format!("{n:E}");
            if !s.contains('E') {
                s.push_str("E0");
            }
            s
        }
    }
}

/// Texto de um membro (rótulo).
pub fn member_caption(v: &Val) -> String {
    match v {
        Val::Blank => String::new(),
        Val::Str(s) => s.to_string(),
        Val::Num(n) => num_text(*n),
        Val::Bool(b) => if *b { "True" } else { "False" }.into(),
    }
}

/// `[T].[c].&[chave]` → chave; `None` = todos os membros.
pub fn key_of_unique(unique: &str) -> Option<String> {
    // O `&` que vem depois do nome da coluna (os membros também podem ter `&`).
    let i = unique.find("].&")?;
    let k = &unique[i + 3..];
    if k.is_empty() {
        return Some(String::new());
    }
    Some(k.strip_prefix('[')?.strip_suffix(']')?.replace("]]", "]"))
}

/// Filtro de uma coluna por chaves de membros.
fn bits_of(m: &Model, col: Col, keys: &[String]) -> Res<Bits> {
    let c = m.column(col.0, col.1)?;
    let date = m.tables[col.0].columns[col.1].kind == crate::model::Kind::Date;
    let want: Vec<String> = keys.iter().map(|k| k.to_lowercase()).collect();
    Ok(c.select(|v| {
        let k = member_key(v, date).to_lowercase();
        want.contains(&k)
            // Números e datas escritos de outro jeito.
            || matches!(v, Val::Num(n) if want.iter().any(|w| w.parse::<f64>().is_ok_and(|x| x == *n)))
    }))
}

fn one(m: &Model, col: Col, code: u32) -> Res<Bits> {
    let mut b = Bits::new(m.column(col.0, col.1)?.dict.len());
    b.set(code);
    Ok(b)
}

/// Combinações candidatas de um eixo (códigos por nível).
fn candidates(ev: &Eval, levels: &[Level], ctx: &Ctx) -> Res<Vec<Vec<u32>>> {
    const MAX: usize = 400_000;
    if levels.is_empty() {
        return Ok(vec![Vec::new()]);
    }
    // Níveis da mesma tabela: as combinações que existem nas linhas dela.
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, l) in levels.iter().enumerate() {
        match groups.iter_mut().find(|g| g.0 == l.col.0) {
            Some(g) => g.1.push(i),
            None => groups.push((l.col.0, vec![i])),
        }
    }
    let mut out: Vec<Vec<u32>> = vec![vec![0; levels.len()]];
    for (t, ls) in groups {
        let mut combos: Vec<Vec<u32>> = if let [l] = ls[..] {
            ev.visible_codes(t, levels[l].col.1, ctx)?.iter().map(|c| vec![c]).collect()
        } else {
            let cols = ls.iter().map(|&l| ev.m.column(t, levels[l].col.1)).collect::<Res<Vec<_>>>()?;
            let mut set: Vec<Vec<u32>> = ev
                .visible(t, ctx)?
                .into_iter()
                .map(|r| cols.iter().map(|c| c.codes[r as usize]).collect())
                .collect();
            set.par_sort_unstable();
            set.dedup();
            set
        };
        if out.len().saturating_mul(combos.len()) > MAX {
            return Err("combinações demais nos campos da tabela dinâmica".into());
        }
        combos.sort_unstable();
        out = out
            .iter()
            .flat_map(|base| {
                combos.iter().map(|c| {
                    let mut v = base.clone();
                    for (k, &l) in ls.iter().enumerate() {
                        v[l] = c[k];
                    }
                    v
                })
            })
            .collect();
    }
    Ok(out)
}

fn with(m: &Model, ctx: &Ctx, levels: &[Level], codes: &[u32]) -> Res<Ctx> {
    let mut c = ctx.clone();
    for (l, &code) in levels.iter().zip(codes) {
        c.restrict(l.col, one(m, l.col, code)?);
    }
    Ok(c)
}

/// Tira os níveis recolhidos dos caminhos.
fn collapse(m: &Model, levels: &[Level], list: Vec<Vec<u32>>) -> Res<Vec<Vec<u32>>> {
    if levels.iter().all(|l| l.collapsed.is_empty()) {
        return Ok(list);
    }
    let sets: Vec<Bits> = levels.iter().map(|l| bits_of(m, l.col, &l.collapsed)).collect::<Res<_>>()?;
    let mut out: Vec<Vec<u32>> = list
        .into_iter()
        .map(|mut p| {
            if let Some(k) = p.iter().zip(&sets).position(|(c, s)| s.get(*c)) {
                p.truncate(k + 1);
            }
            p
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

pub fn run(m: &Model, q: &Query) -> Res<Answer> {
    let ev = Eval::new(m);
    let mut ctx = Ctx::default();
    for (col, keys) in &q.filters {
        ctx.restrict(*col, bits_of(m, *col, keys)?);
    }
    let rows = collapse(m, &q.rows, candidates(&ev, &q.rows, &ctx)?)?;
    let cols = collapse(m, &q.cols, candidates(&ev, &q.cols, &ctx)?)?;
    // Monta as colunas que as medidas usam fora do laço paralelo.
    for d in &q.data {
        ev.measure(d, &ctx)?;
    }
    let col_ctx: Vec<Ctx> = cols.iter().map(|c| with(m, &ctx, &q.cols, c)).collect::<Res<_>>()?;
    let n_data = q.data.len();
    // Valores por linha: colunas × medidas.
    let eval_row = |codes: &[u32]| -> Res<Vec<Val>> {
        let mut out = Vec::with_capacity(cols.len() * n_data);
        for c in &col_ctx {
            let mut x = c.clone();
            for (l, &code) in q.rows.iter().zip(codes) {
                x.restrict(l.col, one(m, l.col, code)?);
            }
            for d in &q.data {
                out.push(ev.measure(d, &x)?);
            }
        }
        Ok(out)
    };
    let table: Vec<Vec<Val>> = rows.par_iter().map(|r| eval_row(r)).collect::<Res<_>>()?;
    // Só linhas e colunas com algum valor (sem medidas, só o que existe).
    let keep_row: Vec<bool> =
        table.iter().map(|v| n_data == 0 || v.iter().any(|x| *x != Val::Blank)).collect();
    let keep_col: Vec<bool> = (0..cols.len())
        .map(|j| {
            n_data == 0
                || q.cols.is_empty()
                || table.iter().any(|v| v[j * n_data..(j + 1) * n_data].iter().any(|x| *x != Val::Blank))
        })
        .collect();

    let axis = |levels: &[Level], list: &[Vec<u32>], keep: &[bool]| -> Res<Axis> {
        // Membros de cada nível, na ordem dos valores.
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); levels.len()];
        for p in list.iter().zip(keep).filter(|x| *x.1).map(|x| x.0) {
            for (l, &c) in p.iter().enumerate() {
                members[l].push(c);
            }
        }
        let mut pos: Vec<HashMap<u32, u32>> = Vec::new();
        let mut vals: Vec<Vec<Val>> = Vec::new();
        for (l, v) in levels.iter().zip(&mut members) {
            let c = m.column(l.col.0, l.col.1)?;
            let order = m.display_order(l.col.0, l.col.1)?;
            if l.manual.is_empty() {
                v.sort_unstable_by_key(|&x| order[x as usize]);
            } else {
                let date = m.tables[l.col.0].columns[l.col.1].kind == crate::model::Kind::Date;
                let rank: HashMap<String, usize> =
                    l.manual.iter().enumerate().map(|(i, k)| (k.to_lowercase(), i)).collect();
                let key = |x: u32| {
                    let k = member_key(&c.dict[x as usize], date).to_lowercase();
                    (rank.get(&k).copied().unwrap_or(usize::MAX), order[x as usize])
                };
                v.sort_by_cached_key(|&x| key(x));
            }
            v.dedup();
            pos.push(v.iter().enumerate().map(|(i, &c)| (c, i as u32)).collect());
            vals.push(v.iter().map(|&x| c.dict[x as usize].clone()).collect());
        }
        // Caminhos em posições, na ordem de exibição.
        let mut paths: Vec<(Path, usize)> = list
            .iter()
            .enumerate()
            .filter(|(i, _)| keep[*i])
            .map(|(i, p)| (p.iter().enumerate().map(|(l, c)| pos[l][c]).collect(), i))
            .collect();
        // Valor de ordenação de cada prefixo.
        let mut sort_vals: HashMap<Path, Val> = HashMap::new();
        for (l, lv) in levels.iter().enumerate() {
            let Some(s) = &lv.sort else { continue };
            let Some(measure) = q.data.get(s.data) else { continue };
            let mut base = ctx.clone();
            for (col, key) in &s.at {
                base.restrict(*col, bits_of(m, *col, std::slice::from_ref(key))?);
            }
            let mut prefixes: Vec<(Path, usize)> =
                paths.iter().filter(|p| p.0.len() > l).map(|p| (p.0[..=l].to_vec(), p.1)).collect();
            prefixes.sort();
            prefixes.dedup_by(|a, b| a.0 == b.0);
            let vals: Vec<(Path, Val)> = prefixes
                .into_par_iter()
                .map(|(p, i)| {
                    let x = with(m, &base, &levels[..=l], &list[i][..=l])?;
                    Ok((p, ev.measure(measure, &x)?))
                })
                .collect::<Res<_>>()?;
            sort_vals.extend(vals);
        }
        paths.sort_by(|a, b| {
            for (l, lv) in levels.iter().enumerate().take(a.0.len().min(b.0.len())) {
                if a.0[l] == b.0[l] {
                    continue;
                }
                let natural = a.0[l].cmp(&b.0[l]);
                let natural = if lv.descending { natural.reverse() } else { natural };
                if let Some(s) = &lv.sort {
                    let blank = Val::Blank;
                    let va = sort_vals.get(&a.0[..=l]).unwrap_or(&blank);
                    let vb = sort_vals.get(&b.0[..=l]).unwrap_or(&blank);
                    let o = compare(va, vb);
                    let o = if s.descending { o.reverse() } else { o };
                    return o.then(natural);
                }
                return natural;
            }
            a.0.len().cmp(&b.0.len())
        });
        Ok(Axis { members: vals, paths: paths.into_iter().map(|p| p.0).collect() })
    };
    let row_axis = axis(&q.rows, &rows, &keep_row)?;
    let col_axis = axis(&q.cols, &cols, &keep_col)?;

    // Valores pelas posições.
    let to_path = |levels: &[Level], ax: &Axis, codes: &[u32]| -> Res<Path> {
        levels
            .iter()
            .zip(codes)
            .zip(&ax.members)
            .map(|((l, &c), list)| {
                let col = m.column(l.col.0, l.col.1)?;
                let v = &col.dict[c as usize];
                Ok(list.iter().position(|x| x.key() == v.key()).unwrap_or(0) as u32)
            })
            .collect()
    };
    let col_paths: Vec<Path> = cols
        .iter()
        .zip(&keep_col)
        .map(|(c, k)| if *k { to_path(&q.cols, &col_axis, c) } else { Ok(Vec::new()) })
        .collect::<Res<_>>()?;
    // Posição dos códigos de linha, uma vez por nível.
    let row_pos: Vec<HashMap<u32, u32>> = q
        .rows
        .iter()
        .zip(&row_axis.members)
        .map(|(l, list)| {
            let col = m.column(l.col.0, l.col.1)?;
            Ok(list
                .iter()
                .enumerate()
                .filter_map(|(i, v)| col.map.get(&v.key()).map(|c| (*c, i as u32)))
                .collect())
        })
        .collect::<Res<_>>()?;
    let mut values = HashMap::new();
    for (i, r) in rows.iter().enumerate().filter(|(i, _)| keep_row[*i]) {
        let rp: Path = r.iter().enumerate().map(|(l, c)| row_pos[l][c]).collect();
        for (j, cp) in col_paths.iter().enumerate().filter(|(j, _)| keep_col[*j]) {
            for k in 0..n_data {
                let v = &table[i][j * n_data + k];
                if *v != Val::Blank {
                    values.insert((rp.clone(), cp.clone(), k as u32), v.clone());
                }
            }
        }
    }
    // Totais: de linha (todas as colunas), de coluna (todas as linhas), geral.
    if !q.cols.is_empty() {
        let totals: Vec<(Path, Vec<Val>)> = rows
            .par_iter()
            .enumerate()
            .filter(|(i, _)| keep_row[*i])
            .map(|(_, r)| {
                let x = with(m, &ctx, &q.rows, r)?;
                let rp = r.iter().enumerate().map(|(l, c)| row_pos[l][c]).collect();
                Ok((rp, q.data.iter().map(|d| ev.measure(d, &x)).collect::<Res<_>>()?))
            })
            .collect::<Res<_>>()?;
        for (rp, v) in totals {
            for (k, v) in v.into_iter().enumerate().filter(|x| x.1 != Val::Blank) {
                values.insert((rp.clone(), Vec::new(), k as u32), v);
            }
        }
    }
    if !q.rows.is_empty() {
        for (j, cp) in col_paths.iter().enumerate().filter(|(j, _)| keep_col[*j]) {
            for (k, d) in q.data.iter().enumerate() {
                let v = ev.measure(d, &col_ctx[j])?;
                if v != Val::Blank {
                    values.insert((Vec::new(), cp.clone(), k as u32), v);
                }
            }
        }
    }
    for (k, d) in q.data.iter().enumerate() {
        let v = ev.measure(d, &ctx)?;
        if v != Val::Blank {
            values.insert((Vec::new(), Vec::new(), k as u32), v);
        }
    }
    Ok(Answer { rows: row_axis, cols: col_axis, values })
}

/// Membros de uma coluna e se cada um tem dados com os filtros (segmentações).
pub fn members(m: &Model, col: Col, filters: &[(Col, Vec<String>)]) -> Res<Vec<(Val, bool)>> {
    let ev = Eval::new(m);
    let c = m.column(col.0, col.1)?;
    let mut ctx = Ctx::default();
    for (fc, keys) in filters {
        ctx.restrict(*fc, bits_of(m, *fc, keys)?);
    }
    let seen = ev.visible_codes(col.0, col.1, &ctx)?;
    let mut codes: Vec<u32> = (0..c.dict.len() as u32).collect();
    let order = m.display_order(col.0, col.1)?;
    codes.sort_by_key(|&k| order[k as usize]);
    Ok(codes.into_iter().map(|k| (c.dict[k as usize].clone(), seen.get(k))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Source};

    struct Data(Vec<Vec<Val>>);

    impl Source for Data {
        fn rows(&self, _: usize) -> usize {
            self.0[0].len()
        }
        fn column(&self, _: usize, c: usize) -> Vec<Val> {
            self.0[c].clone()
        }
    }

    fn model() -> Model {
        let s = |v: &[&str]| v.iter().map(|x| Val::str(x)).collect::<Vec<_>>();
        let n = |v: &[f64]| v.iter().map(|x| Val::Num(*x)).collect::<Vec<_>>();
        let c = |n: &str, k: Kind| (n.to_string(), k, None);
        Model::new(
            vec![(
                "V".into(),
                vec![
                    c("uf", Kind::Text),
                    c("mes", Kind::Number),
                    c("valor", Kind::Number),
                    c("cli", Kind::Text),
                ],
            )],
            &[],
            vec![
                ("V".into(), "soma".into(), "SUM(V[valor])".into()),
                ("V".into(), "clientes".into(), "DISTINCTCOUNT(V[cli])".into()),
            ],
            Box::new(Data(vec![
                s(&["SP", "RJ", "SP", "MG", "RJ"]),
                n(&[1.0, 1.0, 2.0, 2.0, 2.0]),
                n(&[10.0, 5.0, 20.0, 1.0, 6.0]),
                s(&["a", "a", "b", "c", "a"]),
            ])),
            46_000.0,
        )
    }

    fn q(m: &Model, rows: &[&str], cols: &[&str], data: &[&str]) -> Query {
        let lv = |l: &[&str]| l.iter().map(|c| Level::new(m.find("V", c).unwrap())).collect();
        Query {
            rows: lv(rows),
            cols: lv(cols),
            data: data.iter().map(|d| d.to_string()).collect(),
            filters: vec![],
        }
    }

    #[test]
    fn cross_table_and_totals() {
        let m = model();
        let a = run(&m, &q(&m, &["uf"], &["mes"], &["soma", "clientes"])).unwrap();
        assert_eq!(a.rows.members[0], [Val::str("MG"), Val::str("RJ"), Val::str("SP")]);
        let v = |r: &[u32], c: &[u32], k: u32| a.values.get(&(r.to_vec(), c.to_vec(), k)).cloned();
        assert_eq!(v(&[2], &[1], 0), Some(Val::Num(20.0)));
        assert_eq!(v(&[0], &[0], 0), None);
        // Contagem distinta: o total não é a soma.
        assert_eq!((v(&[1], &[], 1), v(&[], &[], 1)), (Some(Val::Num(1.0)), Some(Val::Num(3.0))));
        assert_eq!(v(&[], &[1], 0), Some(Val::Num(27.0)));
    }

    #[test]
    fn filters_sort_and_collapse() {
        let m = model();
        let mut x = q(&m, &["uf"], &[], &["soma"]);
        x.rows[0].sort = Some(Sort { data: 0, descending: true, at: vec![] });
        let a = run(&m, &x).unwrap();
        assert_eq!(a.rows.paths, [vec![2], vec![1], vec![0]]);
        x.rows[0].sort = Some(Sort { data: 0, descending: true, at: vec![((0, 1), "1".into())] });
        x.filters = vec![((0, 0), vec!["rj".into(), "SP".into()])];
        let a = run(&m, &x).unwrap();
        assert_eq!(a.rows.members[0], [Val::str("RJ"), Val::str("SP")]);
        assert_eq!(a.rows.paths, [vec![1], vec![0]]);
        // Dois níveis, um membro recolhido.
        let mut x = q(&m, &["uf", "mes"], &[], &["soma"]);
        x.rows[0].collapsed = vec!["SP".into()];
        let a = run(&m, &x).unwrap();
        assert_eq!(a.rows.paths, [vec![0, 1], vec![1, 0], vec![1, 1], vec![2]]);
        assert_eq!(a.values[&(vec![2], vec![], 0)], Val::Num(30.0));
        let mem = members(&m, (0, 0), &[((0, 1), vec!["1".into()])]).unwrap();
        assert_eq!(mem.iter().map(|x| x.1).collect::<Vec<_>>(), [false, true, true]);
    }

    #[test]
    fn keys() {
        assert_eq!(key_of_unique("[T].[c].&[a]]b]"), Some("a]b".into()));
        assert_eq!(key_of_unique("[T].[c].[All]"), None);
        assert_eq!(key_of_unique("[T].[c].&"), Some("".into()));
        assert_eq!(member_key(&Val::Num(46_000.0), true), "2025-12-09T00:00:00");
        assert_eq!(member_key(&Val::Num(1.5), false), "1.5E0");
        assert_eq!(member_key(&Val::Num(12.0), false), "12");
    }
}
