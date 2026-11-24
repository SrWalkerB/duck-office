//! O que muda no arquivo quando uma aba é renomeada, excluída, copiada ou
//! movida: fórmulas das outras abas, nomes definidos, gráficos, links,
//! formatação condicional, segmentações e a fonte das tabelas dinâmicas.
//!
//! Nada é aplicado aqui: cada função devolve comandos (com inverso) por aba e
//! as listas novas do arquivo ([`BookRefs`]); quem chama troca umas pelas
//! outras e guarda o que tinha para desfazer.

use std::collections::{HashMap, HashSet};

use duck_core::drawing::ObjectKind;
use duck_core::{BookExtras, CellRange, CfKind, CfvoKind, Command, DefinedName, Layout, Sheet};

use crate::sheetrefs::{SheetMap, rename_tables};

/// As listas do arquivo que citam abas.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BookRefs {
    pub names: Vec<DefinedName>,
    /// Fonte (aba, intervalo) de cada cache de tabela dinâmica.
    pub sources: Vec<Option<(String, CellRange)>>,
}

impl BookRefs {
    pub fn of(e: &BookExtras) -> Self {
        Self { names: e.names.clone(), sources: e.pivot_caches.iter().map(|c| c.source.clone()).collect() }
    }

    /// Troca as listas de `e` por estas; devolve as de antes.
    pub fn swap(self, e: &mut BookExtras) -> Self {
        let old = Self::of(e);
        e.names = self.names;
        for (c, s) in e.pivot_caches.iter_mut().zip(self.sources) {
            c.source = s;
        }
        old
    }

    /// Nomes e fontes com os nomes de aba trocados.
    fn retext(&mut self, map: &SheetMap) {
        for n in &mut self.names {
            if let Some(f) = map.apply(&n.formula) {
                n.formula = f;
            }
        }
        for (s, _) in self.sources.iter_mut().flatten() {
            if let Some(Some(n)) = map.name(s) {
                *s = n.to_string();
            }
        }
    }

    /// Índice novo da aba de cada nome local (`None` = a aba saiu: o nome sai).
    pub fn reindex(&mut self, f: impl Fn(u32) -> Option<u32>) {
        self.names.retain_mut(|n| match n.sheet {
            None => true,
            Some(i) => match f(i) {
                Some(j) => {
                    n.sheet = Some(j);
                    true
                }
                None => false,
            },
        });
    }
}

/// Posição nova da aba `i` quando a aba `from` vai para a posição `to`.
pub fn moved(i: u32, from: u32, to: u32) -> u32 {
    if i == from {
        to
    } else if from < i && i <= to {
        i - 1
    } else if to <= i && i < from {
        i + 1
    } else {
        i
    }
}

/// O layout com os nomes de aba trocados, se algo muda.
pub fn retext_layout(l: &Layout, map: &SheetMap) -> Option<Layout> {
    let mut new = l.clone();
    let fix = |s: &mut String| {
        if let Some(n) = map.apply(s) {
            *s = n;
        }
    };
    for v in &mut new.validations {
        fix(&mut v.f1);
        fix(&mut v.f2);
    }
    for r in &mut new.cond {
        match &mut r.kind {
            CfKind::CellIs { f1, f2, .. } => {
                fix(f1);
                f2.iter_mut().for_each(fix);
            }
            CfKind::Expression(f) => fix(f),
            CfKind::ColorScale(v) => {
                v.iter_mut().filter(|c| c.0.kind == CfvoKind::Formula).for_each(|c| fix(&mut c.0.val))
            }
            CfKind::DataBar { min, max, .. } => {
                for c in [min, max] {
                    if c.kind == CfvoKind::Formula {
                        fix(&mut c.val);
                    }
                }
            }
            CfKind::IconSet { cfvos, .. } => {
                cfvos.iter_mut().filter(|c| c.kind == CfvoKind::Formula).for_each(|c| fix(&mut c.val))
            }
            _ => {}
        }
    }
    let loc = |s: &mut String| {
        if let Some(n) = map.apply_location(s) {
            *s = n;
        }
    };
    for k in &mut new.links {
        k.location.iter_mut().for_each(loc);
    }
    for o in &mut new.objects {
        if o.link.as_deref().is_some_and(|l| l.starts_with('#')) {
            o.link.iter_mut().for_each(loc);
        }
        match &mut o.kind {
            ObjectKind::Chart(c) => c.refs_mut().for_each(fix),
            ObjectKind::Control(c) => c.link.iter_mut().for_each(fix),
            ObjectKind::Picture { camera, .. } => camera.iter_mut().for_each(fix),
            _ => {}
        }
    }
    (new != *l).then_some(new)
}

/// Fórmulas e layout de uma aba com os nomes trocados.
fn retext_sheet(s: &Sheet, map: &SheetMap) -> Vec<Command> {
    let mut cmds = Vec::new();
    let texts = s.formula_text_changes(&|t| map.apply(t));
    if !texts.is_empty() {
        cmds.push(Command::FormulaTexts(texts));
    }
    if let Some(l) = retext_layout(&s.layout, map) {
        cmds.push(Command::Layout(Box::new(l)));
    }
    cmds
}

fn pack(i: usize, mut cmds: Vec<Command>) -> Option<(usize, Command)> {
    match cmds.len() {
        0 => None,
        1 => Some((i, cmds.remove(0))),
        _ => Some((i, Command::Group(cmds))),
    }
}

/// Comandos por aba (índices de antes) e listas novas do arquivo.
pub type Change = (Vec<(usize, Command)>, BookRefs);

/// Renomear a aba `old` para `new` (o nome da própria aba fica por conta de
/// quem chama).
pub fn rename(sheets: &[&Sheet], extras: &BookExtras, old: &str, new: &str) -> Change {
    let map = SheetMap::rename(old, new);
    let cmds = sheets.iter().enumerate().filter_map(|(i, s)| pack(i, retext_sheet(s, &map))).collect();
    let mut refs = BookRefs::of(extras);
    refs.retext(&map);
    (cmds, refs)
}

/// Excluir a aba `i`. Os comandos são das outras abas; os índices dos nomes
/// locais já são os de depois.
pub fn delete(sheets: &[&Sheet], extras: &BookExtras, i: usize) -> Result<Change, String> {
    let gone = sheets[i];
    let order: Vec<&str> = sheets.iter().map(|s| s.name.as_str()).collect();
    if sheets.iter().enumerate().all(|(k, s)| k == i || s.hidden) {
        return Err("O arquivo precisa ter pelo menos uma aba visível".into());
    }
    // Uma segmentação de tabela não existe sem a tabela.
    let ids: HashSet<String> = gone.layout.tables.iter().map(|t| t.id.to_string()).collect();
    for (k, s) in sheets.iter().enumerate() {
        let linked = |o: &duck_core::drawing::Object| matches!(&o.kind, ObjectKind::Slicer(x) if x.table.as_ref().is_some_and(|t| ids.contains(&t.0)));
        if k != i && s.layout.objects.iter().any(linked) {
            return Err(format!(
                "A aba \"{}\" tem uma segmentação de dados de uma tabela desta aba; exclua a segmentação antes",
                s.name
            ));
        }
    }
    let map = SheetMap::delete(&gone.name, &order);
    // Tabelas da aba somem: as referências a elas viram #REF!.
    let tables: HashSet<String> = gone.layout.tables.iter().map(|t| t.name.to_lowercase()).collect();
    let mut cmds = Vec::new();
    for (k, s) in sheets.iter().enumerate() {
        if k == i {
            continue;
        }
        let mut list = Vec::new();
        let texts = s.formula_text_changes(&|t| {
            let a = map.apply(t);
            match drop_tables(a.as_deref().unwrap_or(t), &tables) {
                Some(b) => Some(b),
                None => a,
            }
        });
        if !texts.is_empty() {
            list.push(Command::FormulaTexts(texts));
        }
        let mut layout = retext_layout(&s.layout, &map).unwrap_or_else(|| s.layout.clone());
        // Segmentações e linhas do tempo soltam as dinâmicas da aba.
        for o in &mut layout.objects {
            match &mut o.kind {
                ObjectKind::Slicer(x) => {
                    if let Some(p) = &mut x.pivot {
                        p.tables.retain(|t| t.0 != gone.tab_id);
                    }
                }
                ObjectKind::Timeline(x) => x.tables.retain(|t| t.0 != gone.tab_id),
                _ => {}
            }
        }
        if layout != s.layout {
            list.push(Command::Layout(Box::new(layout)));
        }
        cmds.extend(pack(k, list));
    }
    let mut refs = BookRefs::of(extras);
    refs.retext(&map);
    let at = i as u32;
    refs.reindex(|k| match k.cmp(&at) {
        std::cmp::Ordering::Less => Some(k),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(k - 1),
    });
    Ok((cmds, refs))
}

/// Troca as referências a tabelas que deixam de existir por `#REF!`.
fn drop_tables(formula: &str, tables: &HashSet<String>) -> Option<String> {
    if tables.is_empty() {
        return None;
    }
    // Um nome provisório que não aparece em fórmulas, trocado com os colchetes.
    const MARK: &str = "\u{F0FF}";
    let map: HashMap<String, String> = tables.iter().map(|t| (t.clone(), MARK.to_string())).collect();
    let marked = rename_tables(formula, &map)?;
    let mut out = String::with_capacity(marked.len());
    let mut rest = marked.as_str();
    while let Some(k) = rest.find(MARK) {
        out.push_str(&rest[..k]);
        out.push_str("#REF!");
        rest = &rest[k + MARK.len()..];
        if rest.starts_with('[') {
            rest = &rest[crate::skip_brackets(rest.as_bytes(), 0)..];
        }
    }
    out.push_str(rest);
    Some(out)
}

/// Incluir uma aba na posição `at` (só os índices dos nomes locais mudam).
pub fn insert(extras: &BookExtras, at: usize) -> BookRefs {
    let mut refs = BookRefs::of(extras);
    refs.reindex(|k| Some(if k >= at as u32 { k + 1 } else { k }));
    refs
}

/// Linhas ou colunas inseridas ou apagadas na aba `sheet`: os nomes definidos
/// que apontam para ela acompanham. (As fontes das dinâmicas andam no layout.)
pub fn shift_names(extras: &BookExtras, s: duck_core::Shift, sheet: &str) -> BookRefs {
    let mut refs = BookRefs::of(extras);
    for n in &mut refs.names {
        if let Some((f, _)) = crate::adjust_refs(&n.formula, s, false, sheet) {
            n.formula = f;
        }
    }
    refs
}

/// Mover a aba `from` para a posição `to`.
pub fn reorder(extras: &BookExtras, from: usize, to: usize) -> BookRefs {
    let mut refs = BookRefs::of(extras);
    refs.reindex(|k| Some(moved(k, from as u32, to as u32)));
    refs
}

/// Cópia de uma aba.
pub struct Copy {
    pub sheet: Sheet,
    /// Segmentações e linhas do tempo da cópia: (nome na original, nome novo).
    pub renamed: Vec<(String, String)>,
    pub refs: BookRefs,
}

/// Marca, no caminho das partes da cópia, que elas ainda não existem no
/// arquivo: `xl/charts/chart1.xml#copia7`.
pub const COPY_MARK: &str = "#copia";

/// A parte de que uma parte de cópia veio (e se é de cópia).
pub fn source_part(part: &str) -> (&str, bool) {
    match part.split_once(COPY_MARK) {
        Some((p, _)) => (p, true),
        None => (part, false),
    }
}

/// Copia a aba `i` para a posição `at`, com o nome `name` e o `sheetId` `id`.
///
/// Como no Excel: referências da aba a ela mesma passam a apontar para a
/// cópia; tabelas ganham nome e id novos; nomes locais (e os do arquivo que
/// apontam só para ela) viram nomes locais da cópia.
pub fn duplicate(sheets: &[&Sheet], extras: &BookExtras, i: usize, at: usize, name: &str, id: u32) -> Copy {
    let src = sheets[i];
    let mut s = src.clone();
    s.name = name.to_string();
    s.hidden = false;
    let old_id = s.tab_id;
    s.tab_id = id;
    let map = SheetMap::rename(&src.name, name);

    // Tabelas: nome e id únicos no arquivo.
    let mut used: HashSet<String> =
        sheets.iter().flat_map(|s| &s.layout.tables).map(|t| t.name.to_lowercase()).collect();
    used.extend(extras.names.iter().map(|n| n.name.to_lowercase()));
    let mut next = sheets.iter().flat_map(|s| &s.layout.tables).map(|t| t.id).max().unwrap_or(0);
    let mut tables: HashMap<String, String> = HashMap::new();
    let mut table_ids: HashMap<String, String> = HashMap::new();
    for t in &mut s.layout.tables {
        next += 1;
        let base = t.name.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
        let new = (1..)
            .map(|k| format!("{base}{k}"))
            .find(|n| !used.contains(&n.to_lowercase()))
            .unwrap_or_default();
        used.insert(new.to_lowercase());
        tables.insert(t.name.to_lowercase(), new.clone());
        table_ids.insert(t.id.to_string(), next.to_string());
        t.name = new;
        t.id = next;
    }
    for t in &mut s.layout.tables {
        for x in &mut t.column_xml {
            *x = retext_column(x, &map, &tables);
        }
    }
    let texts = s.formula_text_changes(&|t| {
        let a = map.apply(t);
        match rename_tables(a.as_deref().unwrap_or(t), &tables) {
            Some(b) => Some(b),
            None => a,
        }
    });
    if !texts.is_empty() {
        s.set_formula_texts(texts);
    }
    if let Some(l) = retext_layout(&s.layout, &map) {
        s.layout = l;
    }

    // Segmentações e linhas do tempo: nomes únicos, ligadas ao que veio junto.
    let mut taken: HashSet<String> = sheets.iter().flat_map(|s| object_names(&s.layout)).collect();
    let mut renamed = Vec::new();
    let has_pivot = |tables: &[(u32, String)]| tables.iter().any(|t| t.0 == old_id);
    let relink = |tables: &mut Vec<(u32, String)>| {
        tables.retain(|t| t.0 == old_id);
        tables.iter_mut().for_each(|t| t.0 = id);
    };
    let mark = |part: &str| format!("{}{COPY_MARK}{id}", source_part(part).0);
    for o in &mut s.layout.objects {
        let n = match &mut o.kind {
            ObjectKind::Slicer(x) => {
                if let Some(p) = &mut x.pivot
                    && has_pivot(&p.tables)
                {
                    relink(&mut p.tables);
                    p.part = mark(&p.part);
                }
                if let Some(t) = &mut x.table
                    && let Some(n) = table_ids.get(&t.0)
                {
                    t.0 = n.clone();
                }
                &mut x.name
            }
            ObjectKind::Timeline(x) => {
                if has_pivot(&x.tables) {
                    relink(&mut x.tables);
                    x.part = mark(&x.part);
                }
                &mut x.name
            }
            ObjectKind::Chart(c) => {
                c.part = mark(&c.part);
                continue;
            }
            ObjectKind::Control(c) => {
                c.part = mark(&c.part);
                continue;
            }
            _ => continue,
        };
        let base = n.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end().to_string();
        let new = (1..).map(|k| format!("{base} {k}")).find(|x| !taken.contains(&x.to_lowercase()));
        let new = new.unwrap_or_default();
        taken.insert(new.to_lowercase());
        renamed.push((std::mem::replace(n, new.clone()), new));
    }
    for t in &mut s.layout.tables {
        if !t.part.is_empty() {
            t.part = mark(&t.part);
        }
    }
    for p in &mut s.layout.pivots {
        p.part = mark(&p.part);
    }
    for a in &mut s.layout.drawing_anchors {
        a.part = mark(&a.part);
    }
    // Os dados das dinâmicas continuam na aba original.
    s.layout.pivot_sources.clear();

    let mut refs = insert(extras, at);
    let lname = src.name.to_lowercase();
    let mut extra: Vec<DefinedName> = Vec::new();
    for n in &extras.names {
        // Nomes internos do Excel (filtro avançado, segmentações) ficam de fora.
        let local = n.sheet == Some(i as u32);
        let points = n.sheet.is_none() && n.formula != "#N/A" && points_only_to(&n.formula, &lname);
        if !local && !points {
            continue;
        }
        let copy = DefinedName {
            sheet: Some(at as u32),
            formula: map.apply(&n.formula).unwrap_or_else(|| n.formula.clone()),
            ..n.clone()
        };
        if !extra.iter().any(|e| e.name.eq_ignore_ascii_case(&copy.name)) {
            extra.push(copy);
        }
    }
    refs.names.extend(extra);
    Copy { sheet: s, renamed, refs }
}

/// A fórmula do nome cita a aba `lname` (minúsculas) e nenhuma outra.
fn points_only_to(formula: &str, lname: &str) -> bool {
    let mut probe = SheetMap::default();
    probe.add(lname, None);
    let Some(rest) = probe.apply(formula) else { return false };
    // Sem a aba, não sobra nenhum outro qualificador.
    !rest.replace("#REF!", "").contains('!')
}

/// Nomes das segmentações e linhas do tempo, em minúsculas.
fn object_names(l: &Layout) -> Vec<String> {
    l.objects
        .iter()
        .filter_map(|o| match &o.kind {
            ObjectKind::Slicer(x) => Some(x.name.to_lowercase()),
            ObjectKind::Timeline(x) => Some(x.name.to_lowercase()),
            _ => None,
        })
        .collect()
}

/// Fórmulas dentro de um `<tableColumn>` (coluna calculada e total) com os
/// nomes de aba e de tabela trocados.
fn retext_column(xml: &str, map: &SheetMap, tables: &HashMap<String, String>) -> String {
    let mut out = xml.to_string();
    for tag in ["calculatedColumnFormula", "totalsRowFormula"] {
        let Some(open) = out.find(&format!("<{tag}")) else { continue };
        let Some(start) = out[open..].find('>').map(|k| open + k + 1) else { continue };
        let Some(end) = out[start..].find(&format!("</{tag}>")).map(|k| start + k) else { continue };
        let text = out[start..end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&");
        let a = map.apply(&text).unwrap_or(text);
        let b = rename_tables(&a, tables).unwrap_or(a);
        let esc = b.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        out.replace_range(start..end, &esc);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::{CellValue, StringTable, Table};

    fn sheet(name: &str, id: u32) -> Sheet {
        let mut s = Sheet::new(name);
        s.tab_id = id;
        s
    }

    fn name(n: &str, sheet: Option<u32>, f: &str) -> DefinedName {
        DefinedName { name: n.into(), sheet, formula: f.into(), hidden: false }
    }

    #[test]
    fn positions() {
        let order = |from, to| (0..5).map(|i| moved(i, from, to)).collect::<Vec<_>>();
        assert_eq!(order(1, 3), vec![0, 3, 1, 2, 4]);
        assert_eq!(order(3, 1), vec![0, 2, 3, 1, 4]);
        assert_eq!(order(2, 2), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn rename_and_delete() {
        let shared = StringTable::new();
        let mut a = sheet("Dados", 1);
        a.set_cell(0, 0, CellValue::Number(5.0), None, None);
        let mut b = sheet("Conta", 2);
        b.set_cell(0, 0, CellValue::Number(10.0), Some("Dados!A1*2"), None);
        b.set_cell(1, 0, CellValue::Number(1.0), Some("A1+1"), None);
        b.layout.links.push(duck_core::Hyperlink {
            range: CellRange::default(),
            target: None,
            location: Some("Dados!A1".into()),
            tooltip: None,
        });
        let extras = BookExtras {
            names: vec![name("Total", None, "Dados!$A$1"), name("Local", Some(1), "Conta!$A$1")],
            ..Default::default()
        };
        let (cmds, refs) = rename(&[&a, &b], &extras, "Dados", "Meus dados");
        assert_eq!(cmds.len(), 1);
        assert_eq!(refs.names[0].formula, "'Meus dados'!$A$1");
        for (_, c) in cmds {
            let undo = c.apply(&mut b);
            assert_eq!(b.formula(0, 0).unwrap().text, "'Meus dados'!A1*2");
            assert_eq!(b.layout.links[0].location.as_deref(), Some("'Meus dados'!A1"));
            undo.apply(&mut b);
        }
        assert_eq!(b.formula(0, 0).unwrap().text, "Dados!A1*2");

        let (cmds, refs) = delete(&[&a, &b], &extras, 0).unwrap();
        assert_eq!(refs.names.len(), 2);
        assert_eq!((refs.names[0].formula.as_str(), refs.names[1].sheet), ("#REF!", Some(0)));
        for (_, c) in cmds {
            c.apply(&mut b);
        }
        assert_eq!(b.formula(0, 0).unwrap().text, "#REF!*2");
        assert_eq!(b.formula(1, 0).unwrap().text, "A1+1");
        let _ = shared;
        // A única visível não sai.
        a.hidden = true;
        assert!(delete(&[&a, &b], &extras, 1).is_err());
    }

    #[test]
    fn names_follow_rows() {
        let mut extras = BookExtras {
            names: vec![name("Total", None, "Dados!$A$5:$A$9"), name("Outro", None, "Conta!$A$5")],
            ..Default::default()
        };
        let s = duck_core::Shift { axis: duck_core::Axis::Rows, at: 2, n: 3, insert: true };
        let undo = shift_names(&extras, s, "Dados").swap(&mut extras);
        assert_eq!(extras.names[0].formula, "Dados!$A$8:$A$12");
        assert_eq!(extras.names[1].formula, "Conta!$A$5");
        undo.swap(&mut extras);
        assert_eq!(extras.names[0].formula, "Dados!$A$5:$A$9");
    }

    #[test]
    fn copy() {
        let mut a = sheet("Dados", 4);
        a.layout.tables.push(Table {
            name: "Vendas1".into(),
            id: 3,
            part: "xl/tables/table1.xml".into(),
            columns: vec!["Valor".into()],
            column_xml: vec![
                "<tableColumn id=\"1\" name=\"Valor\"><calculatedColumnFormula>Vendas1[[#This Row],[Q]]&gt;1</calculatedColumnFormula></tableColumn>".into(),
            ],
            ..Default::default()
        });
        a.set_cell(0, 0, CellValue::Number(1.0), Some("SUM(Vendas1[Valor])+Dados!B1+Outra!C1"), None);
        let b = sheet("Outra", 5);
        let extras = BookExtras {
            names: vec![
                name("Area", Some(0), "Dados!$A$1:$B$2"),
                name("Geral", None, "Dados!$A$1"),
                name("Mista", None, "Dados!$A$1+Outra!$A$1"),
            ],
            ..Default::default()
        };
        let c = duplicate(&[&a, &b], &extras, 0, 1, "Dados (2)", 9);
        assert_eq!(c.sheet.formula(0, 0).unwrap().text, "SUM(Vendas2[Valor])+'Dados (2)'!B1+Outra!C1");
        let t = &c.sheet.layout.tables[0];
        assert_eq!((t.name.as_str(), t.id), ("Vendas2", 4));
        assert!(t.column_xml[0].contains("Vendas2[[#This Row],[Q]]&gt;1"), "{}", t.column_xml[0]);
        assert_eq!(source_part(&t.part), ("xl/tables/table1.xml", true));
        let local: Vec<_> = c.refs.names.iter().filter(|n| n.sheet == Some(1)).collect();
        assert_eq!(local.len(), 2);
        assert_eq!(local[0].formula, "'Dados (2)'!$A$1:$B$2");
        // A original continua como estava.
        assert_eq!(a.formula(0, 0).unwrap().text, "SUM(Vendas1[Valor])+Dados!B1+Outra!C1");

        let mut d = sheet("X", 6);
        d.set_cell(0, 0, CellValue::Number(1.0), Some("SUM(Vendas1[Valor])+1"), None);
        let (cmds, _) = delete(&[&a, &d], &extras, 0).unwrap();
        for (_, c) in cmds {
            c.apply(&mut d);
        }
        assert_eq!(d.formula(0, 0).unwrap().text, "SUM(#REF!)+1");
    }
}
