//! Notas (anotações clássicas, `xl/commentsN.xml` e o desenho VML delas) e
//! conversas (`xl/threadedComments/`, só leitura).
//!
//! Cada nota guarda o XML de origem: ao gravar, as que não mudaram voltam
//! como estavam (texto formatado, tamanho da caixa...), só com a célula nova.

use crate::RichRun;
use crate::shift::Shift;

/// Uma nota numa célula.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub row: u32,
    pub col: u32,
    pub author: String,
    pub text: String,
    /// Trechos formatados do texto (vazio = simples).
    pub runs: Vec<RichRun>,
    /// Sempre à vista (senão, só ao passar o mouse).
    pub visible: bool,
    /// O `<comment>` do arquivo. `None`: nota nova ou com o texto mudado.
    pub xml: Option<Box<str>>,
    /// O `<v:shape>` do arquivo (`None`: um padrão é gravado).
    pub shape: Option<Box<str>>,
    /// Caixa da nota, como no VML: coluna e linha dos cantos, com os
    /// deslocamentos em pixels (esquerda, dx, topo, dy, direita, dx, base, dy).
    pub anchor: [i32; 8],
    /// Conversa do Excel 365 nesta célula: a nota é só a cópia que as versões
    /// antigas mostram.
    pub thread: Option<Box<Thread>>,
}

/// Uma conversa.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thread {
    /// O comentário e as respostas, em ordem.
    pub messages: Vec<Message>,
    /// Marcada como resolvida.
    pub done: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub author: String,
    /// Data e hora como no arquivo (`2026-11-23T10:30:00.00`).
    pub date: String,
    pub text: String,
    /// O `<threadedComment>` do arquivo.
    pub xml: Box<str>,
}

impl Message {
    /// `23/11/2026 10:30`.
    pub fn when(&self) -> String {
        let b = self.date.as_bytes();
        let ok = b.len() >= 16 && b[4] == b'-' && b[7] == b'-' && b[10] == b'T' && b[13] == b':';
        if !ok || !self.date.is_ascii() {
            return self.date.clone();
        }
        let d = &self.date;
        format!("{}/{}/{} {}", &d[8..10], &d[5..7], &d[..4], &d[11..16])
    }
}

impl Note {
    /// Nota nova na célula, com a caixa no lugar de costume do Excel.
    pub fn new(row: u32, col: u32, author: impl Into<String>, text: impl Into<String>) -> Self {
        let mut n = Note { row, col, author: author.into(), ..Default::default() };
        n.anchor = default_anchor(row, col);
        n.set_text(text);
        n
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.runs.clear();
        self.xml = None;
    }

    /// A nota em outra célula (a caixa anda junto).
    pub fn move_to(&mut self, row: u32, col: u32) {
        let (dr, dc) = (row as i32 - self.row as i32, col as i32 - self.col as i32);
        for (i, d) in [(0, dc), (2, dr), (4, dc), (6, dr)] {
            self.anchor[i] = (self.anchor[i] + d).max(0);
        }
        (self.row, self.col) = (row, col);
    }

    /// O texto para mostrar: o Excel costuma começar a nota com `Autor:`, que
    /// já aparece no título.
    pub fn body(&self) -> &str {
        if self.author.is_empty() {
            return &self.text;
        }
        match self.text.strip_prefix(&self.author).and_then(|t| t.strip_prefix(':')) {
            Some(t) => t.trim_start_matches(['\r', '\n', ' ']),
            None => &self.text,
        }
    }
}

/// A caixa padrão: à direita da célula, duas colunas por cinco linhas.
pub fn default_anchor(row: u32, col: u32) -> [i32; 8] {
    let (r, c) = (row as i32, col as i32);
    let top = (r - 1).max(0);
    [c + 1, 15, top, if r == 0 { 2 } else { 10 }, c + 3, 15, top + 4, 4]
}

/// Nota da célula (a lista fica em ordem de linha e coluna).
pub fn at(list: &[Note], row: u32, col: u32) -> Option<&Note> {
    list.binary_search_by_key(&(row, col), |n| (n.row, n.col)).ok().map(|i| &list[i])
}

/// Notas das linhas `[r0, r1]`.
pub fn in_rows(list: &[Note], r0: u32, r1: u32) -> &[Note] {
    let a = list.partition_point(|n| n.row < r0);
    let b = list.partition_point(|n| n.row <= r1);
    &list[a..b.max(a)]
}

pub fn sort(list: &mut [Note]) {
    list.sort_by_key(|n| (n.row, n.col));
}

/// Depois de mover, inserir ou excluir só células: cada nota vai com a dela
/// (as de células excluídas ou cobertas saem).
pub fn move_cells(list: &mut Vec<Note>, op: &crate::cells::CellOp) {
    list.retain_mut(|n| match op.cell(n.row, n.col) {
        Some((r, c)) => {
            if (r, c) != (n.row, n.col) {
                n.move_to(r, c);
            }
            true
        }
        None => false,
    });
    sort(list);
}

/// Depois de inserir ou apagar linhas/colunas.
pub fn shift(list: &mut Vec<Note>, s: Shift) {
    list.retain_mut(|n| {
        let (row, col) = match s.axis {
            crate::Axis::Rows => (s.index(n.row), Some(n.col)),
            crate::Axis::Cols => (Some(n.row), s.index(n.col)),
        };
        match (row, col) {
            (Some(r), Some(c)) => {
                n.move_to(r, c);
                true
            }
            _ => false,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Axis;

    #[test]
    fn notes_follow_their_cells() {
        let mut list = vec![Note::new(4, 1, "Ana", "a"), Note::new(1, 2, "Ana", "Ana:\nolá")];
        sort(&mut list);
        assert_eq!(list[0].body(), "olá");
        assert_eq!(at(&list, 4, 1).map(|n| n.text.as_str()), Some("a"));
        assert_eq!(in_rows(&list, 2, 9).len(), 1);
        let anchor = list[1].anchor;
        shift(&mut list, Shift { axis: Axis::Rows, at: 2, n: 3, insert: true });
        assert_eq!((list[0].row, list[1].row), (1, 7));
        assert_eq!(list[1].anchor[2], anchor[2] + 3);
        assert_eq!(list[1].anchor[6], anchor[6] + 3);
        shift(&mut list, Shift { axis: Axis::Cols, at: 2, n: 1, insert: false });
        assert_eq!(list.len(), 1, "a coluna da outra foi apagada");
        assert_eq!((list[0].row, list[0].col), (7, 1));
    }

    #[test]
    fn dates() {
        let m = Message { date: "2026-11-23T10:30:00.25".into(), ..Default::default() };
        assert_eq!(m.when(), "23/11/2026 10:30");
        assert_eq!(Message { date: "ontem".into(), ..Default::default() }.when(), "ontem");
    }
}
