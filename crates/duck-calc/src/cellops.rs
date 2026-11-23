//! Mover células e inserir ou excluir só células: os comandos que fazem a
//! mudança no arquivo inteiro (células, fórmulas de todas as abas, mesclagens,
//! formatação condicional, links, gráficos e nomes definidos).

use std::collections::{HashMap, HashSet};

use duck_core::cells::{CellOp, intersect};
use duck_core::drawing::ObjectKind;
use duck_core::{
    Axis, BookExtras, CellChange, CellRange, CellValue, Command, FormulaKind, RawCell, Sheet, StringTable,
    Value,
};

use crate::tabs::BookRefs;
use crate::{Piece, axis_refs, cell_formula, push_cell, push_raw, qualifier, walk};

/// Máximo de células que andam de uma vez.
pub const MAX_CELLS: usize = 2_000_000;

/// Ajusta as referências de uma fórmula (A1) a uma operação de células na aba
/// `sheet` (`own` = a fórmula é dessa aba). `None` = nada muda.
pub fn adjust(formula: &str, op: &CellOp, own: bool, sheet: &str) -> Option<String> {
    let mut pieces = Vec::new();
    walk(formula, |p| pieces.push(p));
    if !pieces.iter().any(|p| matches!(p, Piece::Cell(..))) {
        return None;
    }
    let mut out = String::with_capacity(formula.len() + 8);
    let mut i = 0;
    while i < pieces.len() {
        let p = pieces[i];
        let prev = match i.checked_sub(1).map(|k| pieces[k]) {
            Some(Piece::Text(t)) => t,
            _ => "",
        };
        let applies = match qualifier(prev) {
            Some(Some(name)) => name.eq_ignore_ascii_case(sheet),
            Some(None) => own,
            None => false,
        };
        let end = match (pieces.get(i + 1), pieces.get(i + 2)) {
            (Some(Piece::Text(":")), Some(q)) if !matches!(q, Piece::Text(_)) => Some(*q),
            _ => None,
        };
        let step = if end.is_some() { 3 } else { 1 };
        let raw = |out: &mut String| {
            push_raw(out, p);
            if let Some(q) = end {
                out.push(':');
                push_raw(out, q);
            }
        };
        let (a, b) = match (p, end) {
            (Piece::Cell(a, _), None) => (a, a),
            (Piece::Cell(a, _), Some(Piece::Cell(b, _))) => (a, b),
            (Piece::Text(t), _) => {
                out.push_str(t);
                i += 1;
                continue;
            }
            // Linhas e colunas inteiras não mudam.
            _ => {
                raw(&mut out);
                i += step;
                continue;
            }
        };
        if !applies {
            raw(&mut out);
            i += step;
            continue;
        }
        let r = CellRange {
            first_row: a.row.min(b.row) as u32 - 1,
            first_col: a.col.min(b.col) as u32 - 1,
            last_row: a.row.max(b.row) as u32 - 1,
            last_col: a.col.max(b.col) as u32 - 1,
        };
        let new = if end.is_none() {
            op.cell(r.first_row, r.first_col).map(|(row, col)| CellRange {
                first_row: row,
                first_col: col,
                last_row: row,
                last_col: col,
            })
        } else {
            op.reference(r)
        };
        match new {
            None => out.push_str("#REF!"),
            Some(n) if n == r => raw(&mut out),
            Some(n) => {
                let pick = |first: u32, last: u32, low: bool| if low { first } else { last } as i64 + 1;
                let (ar, ac) = (
                    pick(n.first_row, n.last_row, a.row <= b.row),
                    pick(n.first_col, n.last_col, a.col <= b.col),
                );
                push_cell(&mut out, ac, ar, a.abs_col, a.abs_row);
                if end.is_some() {
                    let (br, bc) = (
                        pick(n.first_row, n.last_row, a.row > b.row),
                        pick(n.first_col, n.last_col, a.col > b.col),
                    );
                    out.push(':');
                    push_cell(&mut out, bc, br, b.abs_col, b.abs_row);
                }
            }
        }
        i += step;
    }
    (out != formula).then_some(out)
}

fn value_of(sheet: &Sheet, r: u32, c: u32, strings: &StringTable) -> CellValue {
    match sheet.cell(r, c, strings).value {
        Value::Empty => CellValue::Empty,
        Value::Number(x) => CellValue::Number(x),
        Value::Text(t) => CellValue::Text(t.to_string()),
        Value::Bool(b) => CellValue::Bool(b),
        Value::Error(e) => CellValue::Error(e),
    }
}

/// O que um trecho de células (a mover) pode conter; senão, o motivo.
fn check(sheet: &Sheet, op: &CellOp) -> Result<(), String> {
    // As células que saem do lugar ou são cobertas.
    let moving = match op {
        CellOp::Move { src, .. } => *src,
        CellOp::Shift { .. } => op.area(),
    };
    let covered = op.target();
    let whole = |r: &CellRange| intersect(r, &moving) == Some(*r);
    let cuts = |r: &CellRange| {
        (intersect(r, &moving).is_some() && !whole(r))
            || (!whole(r) && covered.is_some_and(|t| intersect(r, &t).is_some()))
    };
    let l = &sheet.layout;
    let deleted = match op {
        CellOp::Shift { shift, lo, hi } if !shift.insert => Some(match shift.axis {
            Axis::Rows => CellRange {
                first_row: shift.at,
                last_row: shift.at + shift.n - 1,
                first_col: *lo,
                last_col: *hi,
            },
            Axis::Cols => CellRange {
                first_row: *lo,
                last_row: *hi,
                first_col: shift.at,
                last_col: shift.at + shift.n - 1,
            },
        }),
        _ => None,
    };
    if l.pivots.iter().any(|p| {
        intersect(&p.range, &moving).is_some() || covered.is_some_and(|t| intersect(&p.range, &t).is_some())
    }) {
        return Err("Não é possível mudar parte de uma tabela dinâmica".into());
    }
    for m in &l.merges {
        // Mesclagens inteiras na área excluída ou coberta somem.
        let gone = deleted.or(covered).is_some_and(|d| intersect(m, &d) == Some(*m));
        if cuts(m) && !gone {
            return Err("A operação separaria uma célula mesclada".into());
        }
        if let Some(d) = &deleted
            && !gone
            && intersect(m, d).is_some()
        {
            return Err("A operação separaria uma célula mesclada".into());
        }
    }
    for t in &l.tables {
        let touched = intersect(&t.range, &moving).is_some()
            || covered.is_some_and(|c| intersect(&t.range, &c).is_some());
        let ok = matches!(op, CellOp::Move { .. }) && whole(&t.range);
        if touched && !ok {
            return Err(match op {
                CellOp::Move { .. } => "Não é possível mover parte de uma tabela".into(),
                CellOp::Shift { .. } => {
                    "Não é possível deslocar células de uma tabela; insira ou exclua linhas inteiras"
                        .to_string()
                }
            });
        }
    }
    if let Some(f) = &l.autofilter
        && cuts(&f.range)
    {
        return Err("A operação separaria o intervalo do filtro".into());
    }
    for (_, _, r, dynamic) in sheet.array_formulas() {
        if cuts(&r) || (!dynamic && intersect(&r, &moving).is_some()) {
            return Err("Não é possível mudar parte de uma matriz".into());
        }
    }
    Ok(())
}

/// Células com conteúdo ou estilo em `r`, coluna a coluna.
fn cells_in(sheet: &Sheet, r: &CellRange) -> Vec<(u32, u32, RawCell)> {
    let mut out = Vec::new();
    if sheet.rows() == 0 || sheet.cols() == 0 {
        return out;
    }
    for c in r.first_col..=r.last_col.min(sheet.cols() - 1) {
        for row in r.first_row..=r.last_row.min(sheet.rows() - 1) {
            let raw = sheet.raw(row, c);
            if !raw.is_blank() {
                out.push((row, c, raw));
            }
        }
    }
    out
}

/// As mudanças de uma operação de células na aba `i`. Devolve os comandos por
/// aba e as listas novas do arquivo.
pub fn plan(
    sheets: &[&Sheet],
    strings: &StringTable,
    extras: &BookExtras,
    i: usize,
    op: CellOp,
) -> Result<(Vec<(usize, Command)>, BookRefs), String> {
    let sheet = sheets[i];
    check(sheet, &op)?;
    let name = sheet.name.as_str();
    let moving = match &op {
        CellOp::Move { src, .. } => {
            if op.target().is_none() {
                return Err("O bloco não cabe na planilha".into());
            }
            *src
        }
        CellOp::Shift { .. } => op.area(),
    };
    let cells = cells_in(sheet, &moving);
    if cells.len() > MAX_CELLS {
        return Err("Células demais para deslocar de uma vez; insira ou exclua linhas inteiras".into());
    }
    if let CellOp::Shift { shift, .. } = &op
        && shift.insert
        && cells.iter().any(|(r, c, raw)| {
            op.cell(*r, *c).is_none() && (raw.kind & duck_core::kind::TYPE_MASK != 0 || raw.formula != 0)
        })
    {
        return Err("Não cabe: células com conteúdo sairiam da planilha".into());
    }
    // Resultados espalhados de matrizes dinâmicas: a fórmula refaz.
    let spilled: Vec<(u32, u32, CellRange)> =
        sheet.array_formulas().into_iter().filter(|a| a.3).map(|a| (a.0, a.1, a.2)).collect();
    let is_spill =
        |r: u32, c: u32| spilled.iter().any(|(ar, ac, rg)| rg.contains(r, c) && (*ar, *ac) != (r, c));

    // Conteúdo novo de cada célula da aba.
    let mut raws: HashMap<(u32, u32), RawCell> = HashMap::new();
    let mut formulas: HashMap<(u32, u32), CellChange> = HashMap::new();
    if let Some(t) = op.target() {
        // O que é coberto some, mesmo por células vazias.
        for (r, c, _) in cells_in(sheet, &t) {
            raws.insert((r, c), RawCell::default());
        }
    }
    for (r, c, _) in &cells {
        raws.insert((*r, *c), RawCell::default());
    }
    for (r, c, raw) in &cells {
        let Some(to) = op.cell(*r, *c) else { continue };
        if is_spill(*r, *c) {
            let mut only_style = RawCell { style: raw.style, ..Default::default() };
            only_style.formula = 0;
            raws.insert(to, only_style);
            continue;
        }
        match cell_formula(sheet, *r, *c) {
            Some(f) => {
                if matches!(sheet.formula(*r, *c).map(|f| f.kind), Some(FormulaKind::DataTable)) {
                    return Err("Não é possível mudar parte de uma tabela de dados".into());
                }
                let f = adjust(&f, &op, true, name).unwrap_or_else(|| f.into_owned());
                raws.remove(&to);
                formulas.insert(
                    to,
                    CellChange {
                        row: to.0,
                        col: to.1,
                        value: value_of(sheet, *r, *c, strings),
                        formula: Some(f),
                        style: Some(raw.style),
                    },
                );
            }
            None => {
                formulas.remove(&to);
                raws.insert(to, *raw);
            }
        }
    }
    // Células novas: o formato da vizinha de cima (ou da esquerda).
    if let CellOp::Shift { shift, lo, hi } = &op
        && shift.insert
    {
        let limit = match shift.axis {
            Axis::Rows => sheet.cols(),
            Axis::Cols => sheet.rows(),
        };
        for k in *lo..=(*hi).min(limit.max(1) - 1) {
            let style = match (shift.axis, shift.at.checked_sub(1)) {
                (Axis::Rows, Some(p)) => sheet.raw(p, k).style,
                (Axis::Cols, Some(p)) => sheet.raw(k, p).style,
                _ => 0,
            };
            for n in shift.at..shift.at + shift.n {
                let at = match shift.axis {
                    Axis::Rows => (n, k),
                    Axis::Cols => (k, n),
                };
                if style != 0 {
                    raws.insert(at, RawCell { style, ..Default::default() });
                }
            }
        }
    }
    // As outras fórmulas da aba.
    let moved: HashSet<(u32, u32)> = cells.iter().map(|x| (x.0, x.1)).collect();
    let area = op.area();
    let mut tpl: HashMap<u32, bool> = HashMap::new();
    type Refs = Vec<(bool, i64)>;
    let mut refs: HashMap<u32, (Refs, Refs)> = HashMap::new();
    let hits = |refs: &[(bool, i64)], pos: u32, lo: u32, hi: u32| {
        refs.iter().any(|&(abs, v)| {
            let x = if abs { v } else { pos as i64 + 1 + v };
            (lo as i64 + 1..=hi as i64 + 1).contains(&x)
        })
    };
    for (r, c) in sheet.formula_positions() {
        if moved.contains(&(r, c)) || raws.contains_key(&(r, c)) {
            continue;
        }
        let Some((id, f)) = sheet.formula_entry(r, c) else { continue };
        if *tpl.entry(id).or_insert(f.template) {
            // Modelo relativo: só os deslocamentos dizem se toca a área.
            let (rows, cols) =
                refs.entry(id).or_insert_with(|| (axis_refs(f.text, true), axis_refs(f.text, false)));
            if !hits(rows, r, area.first_row, area.last_row) || !hits(cols, c, area.first_col, area.last_col)
            {
                continue;
            }
        }
        let Some(a1) = cell_formula(sheet, r, c) else { continue };
        if let Some(new) = adjust(&a1, &op, true, name) {
            if matches!(f.kind, FormulaKind::Array { dynamic: false, .. }) {
                return Err("Uma fórmula de matriz usa estas células".into());
            }
            formulas.insert(
                (r, c),
                CellChange {
                    row: r,
                    col: c,
                    value: value_of(sheet, r, c, strings),
                    formula: Some(new),
                    style: None,
                },
            );
        }
    }
    let mut cmds: Vec<Command> = Vec::new();
    let mut raws: Vec<(u32, u32, RawCell)> =
        raws.into_iter().filter(|(k, _)| !formulas.contains_key(k)).map(|((r, c), x)| (r, c, x)).collect();
    raws.sort_unstable_by_key(|x| (x.0, x.1));
    let mut list: Vec<CellChange> = formulas.into_values().collect();
    list.sort_unstable_by_key(|x| (x.row, x.col));
    if !raws.is_empty() {
        cmds.push(Command::Restore(raws));
    }
    if !list.is_empty() {
        cmds.push(Command::SetCells(list));
    }
    // Mesclagens.
    let mut add = Vec::new();
    let mut remove = Vec::new();
    for m in &sheet.layout.merges {
        let new = op.split(*m);
        if new != vec![*m] {
            remove.push(*m);
            add.extend(new);
        }
    }
    if !remove.is_empty() {
        cmds.push(Command::Merges { add, remove });
    }
    // Formatação condicional, links, tabelas e gráficos.
    let mut layout = sheet.layout.clone();
    layout.merges = Vec::new();
    for r in &mut layout.cond {
        r.ranges = r.ranges.iter().flat_map(|x| op.split(*x)).collect();
    }
    layout.links = layout
        .links
        .iter()
        .flat_map(|l| op.split(l.range).into_iter().map(|range| duck_core::Hyperlink { range, ..l.clone() }))
        .collect();
    layout.links.sort_by_key(|l| (l.range.first_row, l.range.first_col));
    for t in &mut layout.tables {
        if let Some(n) = op.reference(t.range) {
            t.range = n;
        }
        if let Some(f) = &mut t.filter
            && let Some(n) = op.reference(f.range)
        {
            f.range = n;
        }
    }
    let filter = layout.autofilter.as_ref().and_then(|f| op.reference(f.range));
    for (_, r) in &mut layout.pivot_sources {
        if let Some(n) = op.reference(*r) {
            *r = n;
        }
    }
    adjust_layout(&mut layout, &op, true, name);
    let mut original = sheet.layout.clone();
    original.merges = Vec::new();
    if layout != original {
        // As mesclagens já mudaram pelo comando delas.
        let mut merges = sheet.layout.merges.clone();
        for c in &cmds {
            if let Command::Merges { add, remove } = c {
                merges.retain(|m| !remove.contains(m));
                merges.extend(add);
            }
        }
        merges.sort_by_key(|r| (r.first_row, r.first_col));
        layout.merges = merges;
        cmds.push(Command::Layout(Box::new(layout)));
    }
    if let (Some(f), Some(range)) = (&sheet.layout.autofilter, filter)
        && f.range != range
    {
        let new = duck_core::AutoFilter { range, ..f.clone() };
        cmds.push(Command::SetFilter { table: None, filter: Some(new) });
    }
    cmds.push(Command::CellOp(op, false));
    let mut out = vec![(i, Command::Group(cmds))];

    // As outras abas.
    let lname = name.to_lowercase();
    let quoted = lname.replace('\'', "''");
    for (k, other) in sheets.iter().enumerate() {
        if k == i {
            continue;
        }
        let mut mentions: HashMap<u32, bool> = HashMap::new();
        let mut changes = Vec::new();
        for (r, c) in other.formula_positions() {
            let Some((id, f)) = other.formula_entry(r, c) else { continue };
            let hit = *mentions.entry(id).or_insert_with(|| {
                let l = f.text.to_lowercase();
                l.contains(&lname) || l.contains(&quoted)
            });
            if !hit {
                continue;
            }
            let Some(a1) = cell_formula(other, r, c) else { continue };
            if let Some(new) = adjust(&a1, &op, false, name) {
                changes.push(CellChange {
                    row: r,
                    col: c,
                    value: value_of(other, r, c, strings),
                    formula: Some(new),
                    style: None,
                });
            }
        }
        let mut list = Vec::new();
        if !changes.is_empty() {
            list.push(Command::SetCells(changes));
        }
        let mut layout = other.layout.clone();
        adjust_layout(&mut layout, &op, false, name);
        if layout != other.layout {
            list.push(Command::Layout(Box::new(layout)));
        }
        if !list.is_empty() {
            out.push((k, Command::Group(list)));
        }
    }
    let mut refs = BookRefs::of(extras);
    for n in &mut refs.names {
        if let Some(f) = adjust(&n.formula, &op, false, name) {
            n.formula = f;
        }
    }
    Ok((out, refs))
}

/// Fórmulas e referências do layout (gráficos, formatação condicional, links).
fn adjust_layout(l: &mut duck_core::Layout, op: &CellOp, own: bool, name: &str) {
    let fix = |s: &mut String| {
        if let Some(n) = adjust(s, op, own, name) {
            *s = n;
        }
    };
    for r in &mut l.cond {
        match &mut r.kind {
            duck_core::CfKind::CellIs { f1, f2, .. } => {
                fix(f1);
                f2.iter_mut().for_each(fix);
            }
            duck_core::CfKind::Expression(f) => fix(f),
            _ => {}
        }
    }
    for o in &mut l.objects {
        match &mut o.kind {
            ObjectKind::Chart(c) => c.refs_mut().for_each(fix),
            ObjectKind::Control(c) => c.link.iter_mut().for_each(fix),
            ObjectKind::Picture { camera, .. } => camera.iter_mut().for_each(fix),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::Shift;

    fn rg(a: u32, b: u32, c: u32, d: u32) -> CellRange {
        CellRange { first_row: a, first_col: b, last_row: c, last_col: d }
    }

    #[test]
    fn formulas() {
        // A1:A2 vai para C5:C6.
        let op = CellOp::Move { src: rg(0, 0, 1, 0), dr: 4, dc: 2 };
        let t = |f: &str| adjust(f, &op, true, "Plan");
        assert_eq!(t("A1+$A$2*B1").as_deref(), Some("C5+$C$6*B1"));
        assert_eq!(t("SUM(A1:A2)").as_deref(), Some("SUM(C5:C6)"));
        assert_eq!(t("C5+1").as_deref(), Some("#REF!+1"));
        assert_eq!(t("SUM(A1:B9)"), None);
        assert_eq!(t("SUM(A:A)+Outra!A1"), None);
        assert_eq!(adjust("Plan!A1+A1", &op, false, "Plan").as_deref(), Some("Plan!C5+A1"));
        // Insere células em B2:B3, deslocando para baixo.
        let s = Shift { axis: Axis::Rows, at: 1, n: 2, insert: true };
        let op = CellOp::Shift { shift: s, lo: 1, hi: 1 };
        let t = |f: &str| adjust(f, &op, true, "Plan");
        assert_eq!(t("B1+B2+A2").as_deref(), Some("B1+B4+A2"));
        assert_eq!(t("SUM(B1:B9)").as_deref(), Some("SUM(B1:B11)"));
        assert_eq!(t("SUM(A1:B9)"), None);
        let op = CellOp::Shift { shift: s.inverse(), lo: 1, hi: 1 };
        assert_eq!(adjust("B2+SUM(B1:B9)+B5", &op, true, "Plan").as_deref(), Some("#REF!+SUM(B1:B7)+B3"));
    }

    fn texts(s: &Sheet, sh: &StringTable) -> Vec<String> {
        let mut out = Vec::new();
        for r in 0..8 {
            for c in 0..5 {
                let v = match s.cell(r, c, sh).value {
                    Value::Number(x) => x.to_string(),
                    Value::Text(t) => t.to_string(),
                    _ => continue,
                };
                let f = cell_formula(s, r, c).map(|f| format!("={f}")).unwrap_or_default();
                out.push(format!("{r},{c}:{v}{f}"));
            }
        }
        out
    }

    fn run(op: CellOp) -> (Vec<String>, Vec<String>) {
        let sh = StringTable::new();
        let mut s = Sheet::new("Plan");
        s.set_cell(0, 0, CellValue::Number(1.0), None, Some(3));
        s.set_cell(1, 0, CellValue::Text("x".into()), None, None);
        s.set_cell(2, 0, CellValue::Number(2.0), Some("A1*2"), None);
        s.set_cell(0, 1, CellValue::Number(3.0), Some("SUM(A1:A3)"), None);
        s.layout.merges.push(rg(5, 0, 5, 1));
        s.clear_edits();
        let before = texts(&s, &sh);
        let (cmds, _) = plan(&[&s], &sh, &BookExtras::default(), 0, op).unwrap();
        let undo: Vec<Command> = cmds.into_iter().map(|(_, c)| c.apply(&mut s)).collect();
        let after = texts(&s, &sh);
        assert_eq!(s.edits().cell_ops.len(), 1);
        for c in undo {
            c.apply(&mut s);
        }
        assert!(s.edits().cell_ops.is_empty());
        assert_eq!(texts(&s, &sh), before);
        assert_eq!(s.layout.merges, vec![rg(5, 0, 5, 1)]);
        (before, after)
    }

    #[test]
    fn move_and_undo() {
        let (_, after) = run(CellOp::Move { src: rg(0, 0, 2, 0), dr: 1, dc: 3 });
        assert_eq!(after, ["0,1:3=SUM(D2:D4)", "1,3:1", "2,3:x", "3,3:2=D2*2"]);
    }

    #[test]
    fn shift_and_undo() {
        // Insere uma célula em A2, para baixo.
        let s = Shift { axis: Axis::Rows, at: 1, n: 1, insert: true };
        let e = {
            let sh = StringTable::new();
            let mut sheet = Sheet::new("Plan");
            sheet.layout.merges.push(rg(5, 0, 5, 1));
            plan(&[&sheet], &sh, &BookExtras::default(), 0, CellOp::Shift { shift: s, lo: 0, hi: 0 }).is_err()
        };
        assert!(e, "cortaria a mesclagem");
        let (_, after) = run(CellOp::Shift { shift: s, lo: 0, hi: 1 });
        assert_eq!(after, ["0,0:1", "0,1:3=SUM(A1:A4)", "2,0:x", "3,0:2=A1*2"]);
        let d = Shift { insert: false, ..s };
        let (_, after) = run(CellOp::Shift { shift: d, lo: 0, hi: 1 });
        assert_eq!(after, ["0,0:1", "0,1:3=SUM(A1:A2)", "1,0:2=A1*2"]);
        let r = Shift { axis: Axis::Cols, at: 0, n: 1, insert: true };
        let (_, after) = run(CellOp::Shift { shift: r, lo: 0, hi: 0 });
        assert_eq!(after, ["0,1:1", "0,2:3=SUM(A1:A3)", "1,0:x", "2,0:2=B1*2"]);
    }
}
