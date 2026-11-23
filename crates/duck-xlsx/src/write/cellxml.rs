//! XML de uma célula a partir do modelo, e os escapes do OOXML.

use duck_core::{FormulaKind, Sheet, StringTable, Value, kind};

/// Nome da coluna (0 → `A`).
pub fn col_name(mut c: u32, out: &mut Vec<u8>) {
    let mut tmp = [0u8; 4];
    let mut n = 0;
    loop {
        tmp[n] = b'A' + (c % 26) as u8;
        n += 1;
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    out.extend(tmp[..n].iter().rev());
}

/// Referência A1 (linha e coluna a partir de 0).
pub fn cell_ref(row: u32, col: u32, out: &mut Vec<u8>) {
    col_name(col, out);
    push_u32(out, row + 1);
}

pub fn push_u32(out: &mut Vec<u8>, v: u32) {
    let mut b = itoa_buf(v);
    out.extend_from_slice(b.as_bytes_mut());
}

struct Digits {
    buf: [u8; 10],
    start: usize,
}

impl Digits {
    fn as_bytes_mut(&mut self) -> &[u8] {
        &self.buf[self.start..]
    }
}

fn itoa_buf(mut v: u32) -> Digits {
    let mut buf = [0u8; 10];
    let mut i = 10;
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    Digits { buf, start: i }
}

/// Número como o `<v>` espera: a menor forma que volta ao mesmo `f64`.
pub fn push_number(out: &mut Vec<u8>, x: f64) {
    use std::io::Write;
    let a = x.abs();
    if x == 0.0 {
        out.extend_from_slice(if x.is_sign_negative() { b"-0" } else { b"0" });
    } else if (1e-5..1e15).contains(&a) {
        let _ = write!(out, "{x}");
    } else {
        let _ = write!(out, "{x:e}");
    }
}

/// Escapa texto para dentro de um elemento: `& < >`, quebras de linha
/// `\r` e caracteres de controle como `_xHHHH_` (e `_x` literal como
/// `_x005F_x`, para não ser lido como escape).
pub fn escape_text(s: &str, out: &mut Vec<u8>) {
    let b = s.as_bytes();
    let mut from = 0;
    for (i, &c) in b.iter().enumerate() {
        let rep: Option<&[u8]> = match c {
            b'&' => Some(b"&amp;"),
            b'<' => Some(b"&lt;"),
            b'>' => Some(b"&gt;"),
            b'\t' | b'\n' => None,
            0..=0x1F => {
                out.extend_from_slice(&b[from..i]);
                let _ = std::io::Write::write_fmt(out, format_args!("_x{c:04X}_"));
                from = i + 1;
                continue;
            }
            b'_' if is_ooxml_escape(&b[i..]) => Some(b"_x005F_"),
            _ => None,
        };
        if let Some(r) = rep {
            out.extend_from_slice(&b[from..i]);
            out.extend_from_slice(r);
            from = i + 1;
        }
    }
    out.extend_from_slice(&b[from..]);
}

fn is_ooxml_escape(b: &[u8]) -> bool {
    b.len() >= 7 && b[1] == b'x' && b[6] == b'_' && b[2..6].iter().all(u8::is_ascii_hexdigit)
}

/// Escapa para dentro de um atributo entre aspas duplas.
pub fn escape_attr(s: &str, out: &mut Vec<u8>) {
    for &c in s.as_bytes() {
        match c {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            b'\n' => out.extend_from_slice(b"&#10;"),
            b'\r' => out.extend_from_slice(b"&#13;"),
            b'\t' => out.extend_from_slice(b"&#9;"),
            c => out.push(c),
        }
    }
}

/// Escreve a célula (linha, coluna) a partir do modelo. `extra` são
/// atributos do `<c>` original a manter (ex.: ` cm="1"`), já formatados.
/// Uma célula vazia e sem estilo não escreve nada.
pub fn write_cell(out: &mut Vec<u8>, sheet: &Sheet, shared: &StringTable, row: u32, col: u32, extra: &[u8]) {
    write_cell_with(out, sheet, shared, row, col, extra, None);
}

/// Como [`write_cell`]; `orig_f` é o `<f…>` original da célula. Numa fórmula
/// compartilhada cujo grupo continua inteiro, ele é mantido (a mestre guarda o
/// intervalo do grupo; escrever uma fórmula comum desfaria o grupo e deixaria
/// os outros membros sem fórmula).
pub fn write_cell_with(
    out: &mut Vec<u8>,
    sheet: &Sheet,
    shared: &StringTable,
    row: u32,
    col: u32,
    extra: &[u8],
    orig_f: Option<&[u8]>,
) {
    let raw = sheet.raw(row, col);
    if raw.is_blank() {
        return;
    }
    let cell = sheet.cell(row, col, shared);
    let has_formula = raw.formula != 0 || raw.kind & kind::FORMULA != 0;
    let formula = if has_formula { sheet.formula(row, col) } else { None };
    out.extend_from_slice(b"<c r=\"");
    cell_ref(row, col, out);
    out.push(b'"');
    if raw.style != 0 {
        out.extend_from_slice(b" s=\"");
        push_u32(out, raw.style as u32);
        out.push(b'"');
    }
    let t: &[u8] = match (cell.value, raw.kind & kind::TYPE_MASK) {
        (Value::Text(_), kind::SHARED_STR) if formula.is_none() => b"s",
        (Value::Text(_), _) if formula.is_some() => b"str",
        (Value::Text(_), _) => b"inlineStr",
        (Value::Bool(_), _) => b"b",
        (Value::Error(_), _) => b"e",
        _ => b"",
    };
    if !t.is_empty() {
        out.extend_from_slice(b" t=\"");
        out.extend_from_slice(t);
        out.push(b'"');
    }
    out.extend_from_slice(extra);
    let body_start = out.len();
    out.push(b'>');
    let keep = orig_f.filter(|_| {
        matches!(formula.map(|f| f.kind), Some(FormulaKind::Shared { .. })) && sheet.shared_intact(row, col)
    });
    if let Some(orig) = keep {
        out.extend_from_slice(orig);
    } else if let Some(f) = formula {
        match f.kind {
            FormulaKind::Array(range) if (row, col) == (f.anchor_row, f.anchor_col) => {
                out.extend_from_slice(b"<f t=\"array\" ref=\"");
                cell_ref(range.first_row, range.first_col, out);
                if (range.first_row, range.first_col) != (range.last_row, range.last_col) {
                    out.push(b':');
                    cell_ref(range.last_row, range.last_col, out);
                }
                out.extend_from_slice(b"\">");
            }
            _ => out.extend_from_slice(b"<f>"),
        }
        if let Some(text) = duck_calc::cell_formula(sheet, row, col) {
            escape_text(&text, out);
        }
        out.extend_from_slice(b"</f>");
    }
    match cell.value {
        Value::Empty => {}
        Value::Number(x) => {
            out.extend_from_slice(b"<v>");
            push_number(out, x);
            out.extend_from_slice(b"</v>");
        }
        Value::Text(s) if t == b"s" => {
            out.extend_from_slice(b"<v>");
            push_u32(out, raw.payload as u32);
            out.extend_from_slice(b"</v>");
            let _ = s;
        }
        Value::Text(s) if t == b"str" => {
            out.extend_from_slice(b"<v>");
            escape_text(s, out);
            out.extend_from_slice(b"</v>");
        }
        Value::Text(s) => {
            out.extend_from_slice(b"<is><t xml:space=\"preserve\">");
            escape_text(s, out);
            out.extend_from_slice(b"</t></is>");
        }
        Value::Bool(b) => out.extend_from_slice(if b { b"<v>1</v>" } else { b"<v>0</v>" }),
        Value::Error(e) => {
            out.extend_from_slice(b"<v>");
            out.extend_from_slice(e.as_str().as_bytes());
            out.extend_from_slice(b"</v>");
        }
    }
    if out.len() == body_start + 1 {
        // Sem filhos: `<c .../>`.
        out.truncate(body_start);
        out.extend_from_slice(b"/>");
    } else {
        out.extend_from_slice(b"</c>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_numbers_and_escapes() {
        let mut v = Vec::new();
        for c in [0, 25, 26, 701, 702, 16383] {
            col_name(c, &mut v);
            v.push(b' ');
        }
        assert_eq!(v, b"A Z AA ZZ AAA XFD ");
        let n = |x: f64| {
            let mut v = Vec::new();
            push_number(&mut v, x);
            String::from_utf8(v).unwrap()
        };
        for x in [0.1, 1.0, -3.5, 1e300, 1.234e-12, 123456789012345.0, 45000.375, -0.0] {
            assert_eq!(n(x).parse::<f64>().unwrap().to_bits(), x.to_bits(), "{x} → {}", n(x));
        }
        assert_eq!(n(1.0), "1");
        let mut e = Vec::new();
        escape_text("a<b & c\r\n_x0041_\u{1}", &mut e);
        assert_eq!(e, b"a&lt;b &amp; c_x000D_\n_x005F_x0041__x0001_");
        let mut s = String::new();
        crate::xml::unescape_into(&e, &mut s);
        assert_eq!(s, "a<b & c\r\n_x0041_\u{1}");
    }
}
