//! Notas e conversas na tela: a marca no canto da célula, o texto ao passar
//! o mouse, as notas sempre à vista e a edição. As conversas do Excel 365 só
//! aparecem (não dá para responder nesta versão).

use duck_core::{Command, Note};
use duck_text::TextStyle;
use duck_ui::{Align, Painter, Rect, Rgba};

use crate::app::{Geom, Quad, SheetsApp, View};
use crate::dialog::{Dialog, Field, Kind, wrap};
use crate::theme::Theme;

/// Nome de quem escreve as notas novas.
fn author() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Duck Sheets".into())
}

/// As marcas das notas nas células de um painel.
pub fn draw_marks(p: &mut Painter, notes: &[Note], v: &View, q: &Quad, g: f32, th: &Theme) {
    if notes.is_empty() || q.rect.is_empty() {
        return;
    }
    for n in duck_core::notes::in_rows(notes, q.row_start, q.row_end) {
        if !(q.col_start..q.col_end).contains(&n.col) || v.rows.size(n.row) == 0 || v.cols.size(n.col) == 0 {
            continue;
        }
        let (r1, c1) = v.merges.at(n.row, n.col).map_or((n.row, n.col), |m| (m.last_row, m.last_col));
        let x1 = crate::draw::pos(&v.cols, q.ox, c1 + 1, q.rect.x0, g);
        let y0 = crate::draw::pos(&v.rows, q.oy, n.row, q.rect.y0, g);
        let h = crate::draw::pos(&v.rows, q.oy, r1 + 1, q.rect.y0, g) - y0;
        let k = (7.0 * g).min(h * 0.5).max(3.0);
        let color = if n.thread.is_some() { th.thread } else { th.note };
        p.triangle([x1 - k - 1.0, y0], [x1 - 1.0, y0], [x1 - 1.0, y0 + k], &q.rect, color);
    }
}

/// Linhas de uma nota: (texto, negrito, cor discreta).
fn lines(n: &Note) -> Vec<(String, bool, bool)> {
    let mut out = Vec::new();
    match &n.thread {
        Some(t) => {
            for m in &t.messages {
                let who = if m.author.is_empty() { "Alguém" } else { &m.author };
                out.push((who.to_string(), true, false));
                out.push((m.when(), false, true));
                out.push((m.text.clone(), false, false));
            }
            let end = if t.done { "Conversa resolvida" } else { "Conversa do Excel (só leitura)" };
            out.push((end.into(), false, true));
        }
        None => {
            // `tc={…}` é o autor das cópias de conversa, não um nome.
            if !n.author.is_empty() && !n.author.starts_with("tc=") {
                out.push((n.author.clone(), true, false));
            }
            out.push((n.body().to_string(), false, false));
        }
    }
    out
}

/// Uma linha já quebrada, com o estilo e a cor.
type Run = (String, TextStyle, Rgba);

fn layout(
    n: &Note,
    ui: TextStyle,
    s: f32,
    max: f32,
    text: &mut duck_text::TextSystem,
    th: &Theme,
) -> Vec<Run> {
    let small = ui.with_px((12.0 * s).round());
    let mut bold = small;
    bold.bold = true;
    let mut out = Vec::new();
    for (t, b, muted) in lines(n) {
        let style = if b { bold } else { small };
        let color = if muted { th.note_muted } else { th.note_text };
        let t = t.replace("\r\n", "\n").replace('\r', "\n");
        for (a, z) in wrap(&t, style, max, text) {
            out.push((t[a..z].trim_end().to_string(), style, color));
        }
    }
    out
}

fn draw_box(p: &mut Painter, r: Rect, clip: &Rect, runs: &[Run], s: f32, th: &Theme) {
    let px = s.round().max(1.0);
    p.fill(Rect::new(r.x0 + 2.0 * s, r.y0 + 2.0 * s, r.x1 + 2.0 * s, r.y1 + 2.0 * s), clip, th.shadow);
    p.fill(r.inset(-px), clip, th.note_line);
    p.fill(r, clip, th.note_bg);
    let line = 17.0 * s;
    let mut y = r.y0 + 5.0 * s;
    for (t, style, color) in runs {
        if y + line > r.y1 + 1.0 {
            break;
        }
        let row = Rect::new(r.x0, y, r.x1, y + line).intersect(clip);
        if !row.is_empty() && row.h() > line * 0.6 {
            p.label(t, *style, row, Align::Left, 7.0 * s, *color);
        }
        y += line;
    }
}

/// A caixa da nota sob o mouse, ao lado da célula.
pub fn draw_hover(p: &mut Painter, g: &Geom, ui: TextStyle, cell: Rect, n: &Note, th: &Theme) {
    let s = g.s;
    let runs = layout(n, ui, s, 260.0 * s, p.text, th);
    let mut w = 70.0 * s;
    for (t, style, _) in &runs {
        w = f32::max(w, p.text.measure_styled(t, *style));
    }
    let line = 17.0 * s;
    let w = (w + 14.0 * s).round();
    let h = (runs.len() as f32 * line + 10.0 * s).min(g.grid.h() - 8.0 * s).round();
    // À direita da célula; se não couber, à esquerda.
    let right = cell.x1 + 12.0 * s;
    let x0 = if right + w > g.grid.x1 { (cell.x0 - w - 12.0 * s).max(g.grid.x0) } else { right };
    let y0 = (cell.y0 - 8.0 * s).clamp(g.grid.y0, (g.grid.y1 - h - 4.0 * s).max(g.grid.y0));
    let r = Rect::new(x0.round(), y0.round(), x0.round() + w, y0.round() + h);
    let all = Rect::everything();
    p.line(
        [cell.x1 - 1.0, cell.y0 + 1.0],
        [if x0 > cell.x0 { r.x0 } else { r.x1 }, r.y0 + 8.0 * s],
        s,
        &all,
        th.note_line,
    );
    draw_box(p, r, &all, &runs, s, th);
}

/// Notas sempre à vista: na caixa que o arquivo guarda.
pub fn draw_visible(
    p: &mut Painter,
    g: &Geom,
    ui: TextStyle,
    notes: &[Note],
    v: &View,
    all: bool,
    th: &Theme,
) {
    for n in notes.iter().filter(|n| n.visible || all) {
        let Some(q) = g.quads.iter().find(|q| {
            !q.rect.is_empty()
                && (q.row_start..q.row_end).contains(&n.row)
                && (q.col_start..q.col_end).contains(&n.col)
        }) else {
            continue;
        };
        let a = n.anchor.map(|x| x.max(0) as u32);
        let (nr, nc) = (v.rows.len() - 1, v.cols.len() - 1);
        let x =
            |c: u32, dx: u32| crate::draw::pos(&v.cols, q.ox, c.min(nc), q.rect.x0, g.g) + dx as f32 * g.g;
        let y =
            |r: u32, dy: u32| crate::draw::pos(&v.rows, q.oy, r.min(nr), q.rect.y0, g.g) + dy as f32 * g.g;
        let r = Rect::new(
            x(a[0], a[1]).round(),
            y(a[2], a[3]).round(),
            x(a[4], a[5]).round(),
            y(a[6], a[7]).round(),
        );
        let runs = layout(n, ui, g.g, (r.w() - 14.0 * g.g).max(30.0), p.text, th);
        let cell = crate::draw::cell_screen_rect(g, v, n.row, n.col);
        if let Some(c) = cell {
            p.line([c.x1 - 1.0, c.y0 + 1.0], [r.x0, r.y0], g.s, &g.grid, th.note_line);
        }
        draw_box(p, r, &g.grid, &runs, g.g, th);
    }
}

impl SheetsApp {
    /// Nota da célula ativa (do canto, se mesclada).
    fn note_cell(&self) -> Option<(u32, u32, Option<&Note>)> {
        let (sheet, v) = self.doc.as_ref()?.current()?;
        let (r, c) = v.merges.at(v.cur.0, v.cur.1).map_or(v.cur, |m| (m.first_row, m.first_col));
        Some((r, c, duck_core::notes::at(&sheet.layout.notes, r, c)))
    }

    pub fn has_note(&self) -> bool {
        self.note_cell().is_some_and(|x| x.2.is_some())
    }

    /// A nota sob o mouse: a célula dela.
    pub fn note_under(&self, g: &Geom, x: f32, y: f32) -> Option<(u32, u32)> {
        let (sheet, v) = self.doc.as_ref()?.current()?;
        if sheet.layout.notes.is_empty() || !g.grid.contains(x, y) {
            return None;
        }
        let (r, c) = g.hit(v, x, y)?;
        let (r, c) = v.merges.at(r, c).map_or((r, c), |m| (m.first_row, m.first_col));
        let n = duck_core::notes::at(&sheet.layout.notes, r, c)?;
        (!n.visible && !self.show_notes).then_some((r, c))
    }

    /// Cria ou edita a nota da célula ativa.
    pub fn ask_note(&mut self) {
        self.commit_edit();
        let Some((row, col, note)) = self.note_cell() else { return };
        if note.is_some_and(|n| n.thread.is_some()) {
            self.dialog = Some(Dialog::message(
                "Conversa",
                "Esta célula tem uma conversa do Excel 365. Nesta versão, as conversas só podem ser lidas.",
            ));
            return;
        }
        let (text, visible) = note.map_or((String::new(), false), |n| (n.text.clone(), n.visible));
        let title = if note.is_some() { "Editar nota" } else { "Nova nota" };
        if !self.can_change_objects() {
            return;
        }
        self.dialog = Some(Dialog::new(
            Kind::Note { row, col },
            title,
            vec![Field::area("", &text), Field::check("Deixar sempre à vista", visible)],
            &["OK", "Cancelar"],
        ));
    }

    /// Grava a nota (texto vazio = tira).
    pub fn set_note(&mut self, row: u32, col: u32, text: &str, visible: Option<bool>) {
        let Some((sheet, _)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let mut list = sheet.layout.notes.clone();
        let at = list.iter().position(|n| (n.row, n.col) == (row, col));
        if at.is_some_and(|i| list[i].thread.is_some()) {
            self.message = "As conversas só podem ser lidas nesta versão".into();
            return;
        }
        let text = text.trim_end();
        match at {
            Some(i) if text.is_empty() => {
                list.remove(i);
            }
            Some(i) => {
                if list[i].text != text {
                    list[i].set_text(text);
                }
                list[i].visible = visible.unwrap_or(list[i].visible);
            }
            None if text.is_empty() => return,
            None => {
                let mut n = Note::new(row, col, author(), text);
                n.visible = visible.unwrap_or(false);
                list.push(n);
                duck_core::notes::sort(&mut list);
            }
        }
        if list != sheet.layout.notes {
            self.run_command(Command::Notes(list));
        }
    }

    /// Tira as notas da seleção (as conversas ficam).
    pub fn delete_notes(&mut self) {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let (r0, c0, r1, c1) = v.sel();
        let mut list = sheet.layout.notes.clone();
        let inside = |n: &Note| (r0..=r1).contains(&n.row) && (c0..=c1).contains(&n.col);
        let threads = list.iter().filter(|n| inside(n) && n.thread.is_some()).count();
        list.retain(|n| !inside(n) || n.thread.is_some());
        let n = sheet.layout.notes.len() - list.len();
        if n > 0 && self.run_command(Command::Notes(list)) {
            self.message = if n == 1 { "Nota excluída".into() } else { format!("{n} notas excluídas") };
        } else if threads > 0 {
            self.message = "As conversas só podem ser lidas nesta versão".into();
        } else if n == 0 {
            self.message = "Não há notas na seleção".into();
        }
    }

    /// Deixa a nota da célula ativa à vista, ou a esconde.
    pub fn toggle_note(&mut self) {
        let Some((row, col, Some(n))) = self.note_cell() else {
            self.message = "A célula ativa não tem nota".into();
            return;
        };
        let (text, show) = (n.text.clone(), !n.visible);
        self.set_note(row, col, &text, Some(show));
    }

    /// Vai à próxima nota da aba (ou à anterior), dando a volta.
    pub fn next_note(&mut self, back: bool) {
        let Some((sheet, v)) = self.doc.as_ref().and_then(|d| d.current()) else { return };
        let list = &sheet.layout.notes;
        if list.is_empty() {
            self.message = "A aba não tem notas".into();
            return;
        }
        let i = list.partition_point(|n| (n.row, n.col) < v.cur);
        let here = list.get(i).is_some_and(|n| (n.row, n.col) == v.cur);
        let k = match (back, here) {
            (true, _) => (i + list.len() - 1) % list.len(),
            (false, true) => (i + 1) % list.len(),
            (false, false) => i % list.len(),
        };
        let (r, c) = (list[k].row, list[k].col);
        self.go_to(r, c);
        self.hover_note = Some((r, c));
    }
}
