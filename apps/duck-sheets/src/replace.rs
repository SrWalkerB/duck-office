//! Localizar e substituir (Ctrl+H): na seleção, na aba ou na pasta inteira,
//! nas fórmulas e textos como aparecem na barra, ou só nos valores digitados.

use duck_core::input::{Input, parse_input};
use duck_core::replace::{Options, Pattern};
use duck_core::{CellChange, CellRange, CellValue, CfCtx, Command, RichRun, Sheet, StringTable, Value};
use rayon::prelude::*;

use crate::app::{SheetsApp, fmt_int};
use crate::edit::{Step, should_apply};
use crate::forms::{Field, Form, FormId};
use crate::look::{Looks, edit_text};

/// Máximo de células trocadas de uma vez.
const MAX_REPLACE: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Sheet,
    Book,
    Selection,
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub find: String,
    pub with: String,
    pub scope: Scope,
    /// Olha também as fórmulas (senão, só os valores digitados).
    pub formulas: bool,
    pub opts: Options,
}

/// Uma célula achada: posição e o texto como na barra de fórmulas.
type Hit = (u32, u32, String);

/// O texto da célula como aparece na barra de fórmulas.
fn bar_text(sheet: &Sheet, strings: &StringTable, looks: &Looks, date1904: bool, r: u32, c: u32) -> String {
    if let Some(f) = duck_calc::cell_formula(sheet, r, c) {
        return format!("={}", duck_calc::locale::to_pt(&f));
    }
    let cell = sheet.cell(r, c, strings);
    let mut s = String::new();
    if !matches!(cell.value, Value::Empty) {
        edit_text(looks, looks.get(cell.style), cell.value, date1904, &mut s);
    }
    s
}

/// As células de `sheet` em que o padrão aparece, em ordem de linha.
fn search(
    sheet: &Sheet,
    strings: &StringTable,
    looks: &Looks,
    date1904: bool,
    q: &Query,
    p: &Pattern,
    areas: Option<&[CellRange]>,
) -> Vec<Hit> {
    const SPAN: u32 = 65_536;
    let rows = sheet.rows();
    let tasks: Vec<(u32, u32)> =
        (0..sheet.cols()).flat_map(|c| (0..rows.div_ceil(SPAN)).map(move |k| (c, k * SPAN))).collect();
    let mut hits: Vec<Hit> = tasks
        .par_iter()
        .map(|&(c, start)| {
            let mut out = Vec::new();
            let end = (start + SPAN).min(rows);
            let test = |r: u32, out: &mut Vec<Hit>| {
                if areas.is_some_and(|a| !a.iter().any(|x| x.contains(r, c))) {
                    return;
                }
                let t = bar_text(sheet, strings, looks, date1904, r, c);
                if !t.is_empty() && p.is_match(&t) {
                    out.push((r, c, t));
                }
            };
            let mut last = None;
            sheet.for_each_value(c, start..end, strings, |r, cell| {
                if q.formulas || !cell.formula {
                    test(r, &mut out);
                }
                last = Some(r);
                true
            });
            // Fórmulas com resultado vazio não passam acima.
            if q.formulas {
                sheet.for_each_formula(c, |r, _| {
                    if (start..end).contains(&r) && !sheet.has_value(r, c) {
                        test(r, &mut out);
                    }
                });
            }
            out
        })
        .flatten()
        .collect();
    hits.par_sort_unstable_by_key(|h| (h.0, h.1));
    hits
}

impl SheetsApp {
    /// Ctrl+H.
    pub fn open_replace(&mut self) {
        self.commit_edit();
        let Some(v) = self.doc.as_ref().and_then(|d| d.current()).map(|x| x.1) else { return };
        // Com várias células escolhidas, o normal é trocar só nelas.
        let areas = v.areas();
        let one = matches!(areas.as_slice(), [(r0, c0, r1, c1)] if (r0, c0) == (r1, c1));
        let scope = if one { 0 } else { 2 };
        self.replace_areas = areas.into_iter().map(crate::fill::range_of).collect();
        let last = self.replace_last.clone();
        let find =
            self.find.take().map(|f| f.input.text).filter(|t| !t.is_empty()).unwrap_or(last.find.clone());
        let mut f = Form::new(
            FormId::Replace,
            "Localizar e substituir",
            vec![
                Field::text("find", "Localizar", &find),
                Field::text("with", "Substituir por", &last.with),
                Field::choice("scope", "Em", &["Aba ativa", "Pasta inteira", "Seleção"], scope),
                Field::choice(
                    "look",
                    "Procurar em",
                    &["Fórmulas e valores", "Só valores"],
                    usize::from(!last.formulas && !last.find.is_empty()),
                ),
                Field::check("exact", "Diferenciar maiúsculas e acentos", last.opts.exact),
                Field::check("whole", "A célula inteira", last.opts.whole),
            ],
            &["Localizar próxima", "Substituir", "Substituir tudo", "Fechar"],
        );
        f.light = true;
        f.width = 500.0;
        self.form = Some(f);
    }

    fn replace_query(f: &Form) -> Query {
        Query {
            find: f.text("find").to_string(),
            with: f.text("with").to_string(),
            scope: [Scope::Sheet, Scope::Book, Scope::Selection][f.sel("scope").min(2)],
            formulas: f.sel("look") == 0,
            opts: Options { exact: f.on("exact"), whole: f.on("whole") },
        }
    }

    /// Botão da caixa; devolve o que dizer nela.
    pub fn replace_form(&mut self, f: &Form, button: usize) -> String {
        let q = Self::replace_query(f);
        self.replace_last = q.clone();
        match button {
            0 => self.find_one(&q),
            1 => self.replace_one(&q),
            _ => self.replace_all(&q),
        }
    }

    /// As células achadas, por aba.
    fn replace_hits(&mut self, q: &Query) -> Vec<(usize, Vec<Hit>)> {
        let Some(p) = Pattern::new(&q.find, q.opts) else { return Vec::new() };
        let areas = self.replace_areas.clone();
        let Some(doc) = &self.doc else { return Vec::new() };
        let active = doc.active;
        let which: Vec<usize> = match q.scope {
            Scope::Book => (0..doc.sheets.len()).filter(|&i| !doc.sheets[i].info.hidden).collect(),
            _ => vec![active],
        };
        let mut out = Vec::new();
        for i in which {
            let Some(sheet) = doc.sheets[i].data.as_ref().filter(|_| !doc.sheets[i].preview) else {
                continue;
            };
            let only = (q.scope == Scope::Selection).then_some(areas.as_slice());
            let mut hits = search(sheet, &doc.strings, &doc.looks, doc.date1904, q, &p, only);
            // O resultado espalhado de uma matriz não é editável.
            hits.retain(|(r, c, _)| {
                !doc.calc
                    .spills
                    .iter()
                    .any(|((s, ar, ac), rg)| *s == i as u32 && rg.contains(*r, *c) && (*ar, *ac) != (*r, *c))
            });
            if !hits.is_empty() {
                out.push((i, hits));
            }
        }
        out
    }

    /// Vai para a próxima célula achada (depois da ativa).
    fn find_one(&mut self, q: &Query) -> String {
        if q.find.is_empty() {
            return "Digite o que localizar".into();
        }
        let hits = self.replace_hits(q);
        let total: usize = hits.iter().map(|h| h.1.len()).sum();
        if total == 0 {
            return "Nada encontrado".into();
        }
        let Some(doc) = &self.doc else { return String::new() };
        let here = (doc.active, doc.current().map_or((0, 0), |(_, v)| v.cur));
        let all: Vec<(usize, u32, u32)> =
            hits.iter().flat_map(|(i, h)| h.iter().map(move |x| (*i, x.0, x.1))).collect();
        let k = all.iter().position(|&(i, r, c)| (i, (r, c)) > here).unwrap_or(0);
        let (i, r, c) = all[k];
        self.switch_sheet(i);
        self.go_to(r, c);
        format!("{} de {} células", fmt_int(k as u64 + 1), fmt_int(total as u64))
    }

    /// Troca na célula ativa (se ela bate) e vai para a próxima.
    fn replace_one(&mut self, q: &Query) -> String {
        let hits = self.replace_hits(q);
        let Some(doc) = &self.doc else { return String::new() };
        let here = (doc.active, doc.current().map_or((0, 0), |(_, v)| v.cur));
        let hit = hits
            .iter()
            .find(|h| h.0 == here.0)
            .and_then(|h| h.1.iter().find(|x| (x.0, x.1) == here.1))
            .cloned();
        if let Some(hit) = hit {
            let one = vec![(here.0, vec![hit])];
            if let Err(e) = self.apply_replace(q, one) {
                return e;
            }
        }
        self.find_one(q)
    }

    pub fn replace_all(&mut self, q: &Query) -> String {
        if q.find.is_empty() {
            return "Digite o que localizar".into();
        }
        let hits = self.replace_hits(q);
        if hits.is_empty() {
            return "Nada encontrado".into();
        }
        match self.apply_replace(q, hits) {
            Ok((cells, times)) => {
                let m =
                    format!("{} substituições em {} células", fmt_int(times as u64), fmt_int(cells as u64));
                self.message = m.clone();
                m
            }
            Err(e) => e,
        }
    }

    /// Faz as trocas (um passo só). Devolve células e ocorrências.
    fn apply_replace(&mut self, q: &Query, hits: Vec<(usize, Vec<Hit>)>) -> Result<(usize, usize), String> {
        let total: usize = hits.iter().map(|h| h.1.len()).sum();
        if total > MAX_REPLACE {
            return Err(format!("São {} células: demais para trocar de uma vez", fmt_int(total as u64)));
        }
        let Some(p) = Pattern::new(&q.find, q.opts) else { return Err("Digite o que localizar".into()) };
        let today = self.today;
        let Some(doc) = self.doc.as_mut() else { return Err(String::new()) };
        let mut styles = doc.styles.clone();
        let before = styles.xfs.len();
        let mut step: Step = Vec::new();
        let (mut cells, mut times) = (0, 0);
        for (i, list) in hits {
            let Some(sheet) = doc.sheets[i].data.as_ref() else { continue };
            let cfx = CfCtx { sheet, shared: &doc.strings, today };
            let mut changes = Vec::new();
            let mut rich: Vec<(u32, u32, Vec<RichRun>)> = Vec::new();
            for (r, c, text) in list {
                let Some((new, n)) = p.replace(&text, &q.with) else { continue };
                if new == text {
                    continue;
                }
                let style = sheet.raw(r, c).style;
                let fmt_id = styles.xfs.get(style as usize).map_or(0, |x| x.num_fmt);
                let was_formula = text.starts_with('=') && sheet.formula(r, c).is_some();
                let input = if fmt_id == 49 {
                    // Célula de texto: continua texto.
                    Input::Value(CellValue::Text(new.clone()), None)
                } else if !was_formula && new.starts_with('=') && !text.starts_with('=') {
                    Input::Value(CellValue::Text(new.clone()), None)
                } else {
                    parse_input(&new, doc.date1904, duck_core::input::today())
                };
                let mut st = None;
                let (value, formula) = match input {
                    Input::Formula(f) => {
                        let f = duck_calc::locale::from_pt(&f);
                        (cfx.eval_formula(&f, r, c).unwrap_or(CellValue::Empty), Some(f))
                    }
                    Input::Value(v, fmt) => {
                        if let Some(f) = fmt
                            && should_apply(&styles, fmt_id, f)
                        {
                            st = Some(
                                std::sync::Arc::make_mut(&mut styles).derive_xf(style, |x| x.num_fmt = f),
                            );
                        }
                        if matches!(v, CellValue::Text(_))
                            && let Some(runs) = sheet.rich_at(r, c, &doc.strings)
                            && let Some(new) = duck_core::rich::rebase(&text, runs, &new)
                        {
                            rich.push((r, c, new));
                        }
                        (v, None)
                    }
                };
                cells += 1;
                times += n;
                changes.push(CellChange { row: r, col: c, value, formula, style: st });
            }
            let mut cmds = Vec::new();
            if !changes.is_empty() {
                cmds.push(Command::SetCells(changes));
            }
            if !rich.is_empty() {
                cmds.push(Command::RichText(rich));
            }
            if !cmds.is_empty() {
                step.push((i, Command::Group(cmds)));
            }
        }
        if step.is_empty() {
            return Err("Nada mudou".into());
        }
        if styles.xfs.len() != before {
            doc.looks = std::sync::Arc::new(Looks::new(&styles));
            doc.styles = styles;
            self.font_key = (0, 0);
        }
        if self.run_step(step) { Ok((cells, times)) } else { Err(self.message.clone()) }
    }

    /// `--edit substituir=ACHAR>TROCAR` (também `substituir-pasta=`,
    /// `substituir-valores=`, `substituir-exato=`, `substituir-celula=`).
    pub fn scripted_replace(&mut self, e: &str) -> bool {
        let Some((op, arg)) = e.split_once('=') else { return false };
        let mut q = Query { formulas: true, ..Default::default() };
        match op {
            "substituir" => {}
            "substituir-pasta" => q.scope = Scope::Book,
            "substituir-valores" => q.formulas = false,
            "substituir-exato" => q.opts.exact = true,
            "substituir-celula" => q.opts.whole = true,
            _ => return false,
        }
        let Some((find, with)) = arg.split_once('>') else {
            eprintln!("--edit: use substituir=achar>trocar");
            return true;
        };
        q.find = find.into();
        q.with = with.into();
        let m = self.replace_all(&q);
        eprintln!("--edit {op}: {m}");
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, get, select};

    fn q(find: &str, with: &str) -> Query {
        Query { find: find.into(), with: with.into(), formulas: true, ..Default::default() }
    }

    fn sample() -> SheetsApp {
        app(&[
            ("P", &[("A1", "Ação da loja"), ("A2", "10"), ("A3", "=SOMA(A2:A2)*10"), ("B1", "acao")]),
            ("Q", &[("A1", "ação"), ("B2", "x")]),
        ])
    }

    #[test]
    fn sheet_and_book() {
        let mut a = sample();
        assert_eq!(a.replace_all(&q("acao", "Meta")), "2 substituições em 2 células");
        assert_eq!(
            (get(&a, 0, "A1"), get(&a, 0, "B1"), get(&a, 1, "A1")),
            ("Meta da loja".into(), "Meta".into(), "ação".into())
        );
        a.undo();
        assert_eq!(get(&a, 0, "A1"), "Ação da loja");
        let mut b = q("acao", "Meta");
        b.scope = Scope::Book;
        assert_eq!(a.replace_all(&b), "3 substituições em 3 células");
        assert_eq!(get(&a, 1, "A1"), "Meta");
        a.undo();
        assert_eq!((get(&a, 0, "B1"), get(&a, 1, "A1")), ("acao".into(), "ação".into()));
        let mut e = q("acao", "Meta");
        e.opts.exact = true;
        assert_eq!(a.replace_all(&e), "1 substituições em 1 células");
        assert_eq!(a.replace_all(&q("inexistente", "x")), "Nada encontrado");
    }

    #[test]
    fn formulas_and_values() {
        let mut a = sample();
        // Nas fórmulas em português; números como aparecem na barra.
        assert_eq!(a.replace_all(&q("SOMA", "MÉDIA")), "1 substituições em 1 células");
        assert_eq!(get(&a, 0, "A3"), "=MÉDIA(A2:A2)*10");
        a.undo();
        let mut v = q("10", "20");
        v.formulas = false;
        assert_eq!(a.replace_all(&v), "1 substituições em 1 células");
        assert_eq!((get(&a, 0, "A2"), get(&a, 0, "A3")), ("20".into(), "=SOMA(A2:A2)*10".into()));
        a.undo();
        assert_eq!(a.replace_all(&q("10", "20")), "2 substituições em 2 células");
        assert_eq!(get(&a, 0, "A3"), "=SOMA(A2:A2)*20");
    }

    #[test]
    fn selection_next_and_one() {
        let mut a = sample();
        select(&mut a, "A1:A3");
        a.open_replace();
        let mut f = a.form.take().unwrap();
        assert_eq!(f.sel("scope"), 2);
        f.set_text("find", "acao");
        f.set_text("with", "Z");
        assert_eq!(a.replace_form(&f, 2), "1 substituições em 1 células");
        assert_eq!(get(&a, 0, "B1"), "acao");
        a.undo();
        f.set_sel("scope", 1);
        a.go_to(5, 5);
        // Dá a volta: depois da aba ativa, a outra; depois, a primeira.
        assert_eq!(a.replace_form(&f, 0), "3 de 3 células");
        assert_eq!(a.doc.as_ref().unwrap().active, 1);
        assert_eq!(a.replace_form(&f, 1), "1 de 2 células");
        assert_eq!(get(&a, 1, "A1"), "Z");
        assert_eq!(a.doc.as_ref().unwrap().active, 0);
    }

    #[test]
    fn rich_text_stays() {
        use duck_core::{RichRun, RunFont};
        let mut a = sample();
        let runs = vec![
            RichRun { start: 0, font: None },
            RichRun { start: 8, font: Some(RunFont { bold: true, ..Default::default() }) },
        ];
        a.run_command(Command::RichText(vec![(0, 0, runs)]));
        a.replace_all(&q("loja", "rua"));
        let doc = a.doc.as_ref().unwrap();
        let s = doc.sheets[0].data.as_ref().unwrap();
        let r = s.rich_at(0, 0, &doc.strings).unwrap();
        assert!(r.iter().any(|x| x.font.is_some()), "{r:?}");
    }
}
