//! O modelo de objetos do Excel que as macros veem (`ThisWorkbook`, `Range`,
//! `PivotTable`...), sobre um [`Backend`] que é a planilha aberta.

use duck_core::{CellRange, CellValue};

use crate::interp::{Args, Arr, Host, Res, Val, VbErr, to_num, to_str, truth};

const MAX_ROWS: u32 = 1_048_576;
const MAX_COLS: u32 = 16_384;

/// Eixo de um campo de tabela dinâmica (`XlPivotFieldOrientation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orient {
    Hidden,
    Row,
    Col,
    Page,
    Data,
}

/// Um campo de tabela dinâmica: índice e nome.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: usize,
    pub name: String,
    /// Legenda (dinâmicas do Modelo de Dados).
    pub caption: String,
}

/// Tabela do Excel (`ListObject`).
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    pub range: CellRange,
    pub header_rows: u32,
    pub totals_rows: u32,
    pub columns: Vec<String>,
}

type R<T> = Result<T, String>;

/// A planilha aberta.
pub trait Backend {
    fn workbook_name(&self) -> String;
    fn sheets(&self) -> Vec<String>;
    fn active_sheet(&self) -> usize;
    fn activate(&mut self, sheet: usize);
    fn value(&mut self, sheet: usize, row: u32, col: u32) -> CellValue;
    /// Escreve valores em ordem de linha (texto com `=` vira fórmula).
    fn set_values(&mut self, sheet: usize, range: CellRange, vals: Vec<CellValue>) -> R<()>;
    fn used_range(&self, sheet: usize) -> Option<CellRange>;
    fn select(&mut self, sheet: usize, range: CellRange);
    fn sort(&mut self, sheet: usize, range: CellRange, key_col: u32, descending: bool) -> R<()>;
    /// Troca fórmulas pelos valores no intervalo.
    fn freeze_values(&mut self, sheet: usize, range: CellRange) -> R<()>;
    fn set_hidden(&mut self, sheet: usize, rows: bool, first: u32, last: u32, hidden: bool) -> R<()>;
    fn delete_sheet(&mut self, sheet: usize) -> R<()>;
    fn tables(&self, sheet: usize) -> Vec<Table>;
    /// Acrescenta uma linha no fim da tabela; devolve a linha da aba.
    fn add_table_row(&mut self, sheet: usize, table: &str) -> R<u32>;
    /// Nomes das dinâmicas da aba.
    fn pivots(&self, sheet: usize) -> Vec<String>;
    fn pivot_fields(&mut self, sheet: usize, pivot: usize) -> Vec<Field>;
    /// Campos no eixo, na ordem.
    fn pivot_axis(&mut self, sheet: usize, pivot: usize, axis: Orient) -> Vec<usize>;
    /// Dinâmica do Modelo de Dados (tem `CubeFields`).
    fn pivot_is_cube(&self, sheet: usize, pivot: usize) -> bool;
    fn set_orientation(&mut self, sheet: usize, pivot: usize, field: usize, o: Orient) -> R<()>;
    fn set_position(&mut self, sheet: usize, pivot: usize, field: usize, pos: usize) -> R<()>;
    fn set_show_detail(&mut self, sheet: usize, pivot: usize, field: usize, show: bool) -> R<()>;
    fn refresh_pivot(&mut self, sheet: usize, pivot: usize) -> R<()>;
    /// Áreas da dinâmica: (tudo, valores, rótulos de linha).
    fn pivot_ranges(&mut self, sheet: usize, pivot: usize) -> R<(CellRange, Option<CellRange>, CellRange)>;
    /// Aplica as mudanças de dinâmicas pendentes.
    fn flush(&mut self) -> R<()>;
    fn calculate(&mut self);
    /// Caixa de mensagem; devolve o botão (`vbYes` = 6, `vbNo` = 7, `vbOK` = 1).
    fn msgbox(&mut self, text: &str, buttons: i32, title: &str) -> i32;
    fn status(&mut self, text: Option<&str>);
}

#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Application,
    WorksheetFunction,
    CommandBars,
    ActiveWindow,
    Workbook,
    Sheets,
    Sheet(usize),
    Range(usize, CellRange),
    /// `Range.Rows` / `Range.Columns` / `Range.Cells`.
    RangeRows(usize, CellRange),
    RangeCols(usize, CellRange),
    Cells(usize, CellRange),
    /// Fonte, preenchimento...: aceita e ignora.
    Format,
    Pivots(usize),
    Pivot(usize, usize),
    Fields(usize, usize, Option<Orient>),
    Field(usize, usize, usize),
    CubeFields(usize, usize),
    Tables(usize),
    Table(usize, String),
    TableCols(usize, String),
    TableCol(usize, String, usize),
    TableRows(usize, String),
    TableRow(usize, String, u32),
}

type V = Val<Obj>;
type A = Args<Obj>;

pub struct Excel<'b, B: Backend> {
    pub b: &'b mut B,
    /// O que `Copy` guardou.
    copied: Option<(usize, CellRange)>,
}

fn fail(text: impl Into<String>) -> VbErr {
    VbErr::new(1004, text)
}

fn out_of_range() -> VbErr {
    VbErr::new(9, "Subscrito fora do intervalo")
}

fn cell_val(v: CellValue) -> V {
    match v {
        CellValue::Empty => Val::Empty,
        CellValue::Number(n) => Val::Num(n),
        CellValue::Text(s) => Val::Str(s),
        CellValue::Bool(b) => Val::Bool(b),
        CellValue::Error(e) => Val::Str(format!("Error {}", e.as_str())),
    }
}

fn val_cell(v: &V) -> Res<CellValue> {
    Ok(match v {
        Val::Empty | Val::Null | Val::Nothing => CellValue::Empty,
        Val::Num(n) | Val::Date(n) => CellValue::Number(*n),
        Val::Bool(b) => CellValue::Bool(*b),
        Val::Str(s) if s.is_empty() => CellValue::Empty,
        Val::Str(s) => CellValue::Text(s.clone()),
        _ => return Err(VbErr::new(13, "Tipos incompatíveis")),
    })
}

fn size(r: CellRange) -> (u32, u32) {
    (r.last_row - r.first_row + 1, r.last_col - r.first_col + 1)
}

fn cell(row: u32, col: u32) -> CellRange {
    CellRange { first_row: row, first_col: col, last_row: row, last_col: col }
}

const WHOLE: CellRange =
    CellRange { first_row: 0, first_col: 0, last_row: MAX_ROWS - 1, last_col: MAX_COLS - 1 };

/// Letras de coluna → índice.
fn col_index(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 3 || !s.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let n = s.bytes().fold(0u32, |a, b| a * 26 + (b.to_ascii_uppercase() - b'A') as u32 + 1);
    (n <= MAX_COLS).then(|| n - 1)
}

/// `A1`, `$A$1:B2`, `A:C`, `2:5`.
pub fn parse_ref(s: &str) -> Option<CellRange> {
    let part = |p: &str| -> Option<(Option<u32>, Option<u32>)> {
        let p = p.trim().replace('$', "");
        let k = p.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
        let (c, r) = p.split_at(k);
        let col = if c.is_empty() { None } else { Some(col_index(c)?) };
        let row = if r.is_empty() { None } else { Some(r.parse::<u32>().ok()?.checked_sub(1)?) };
        (col.is_some() || row.is_some()).then_some((row, col))
    };
    let (a, b) = s.split_once(':').unwrap_or((s, s));
    let ((r0, c0), (r1, c1)) = (part(a)?, part(b)?);
    if !s.contains(':') && (r0.is_none() || c0.is_none()) {
        return None;
    }
    let (r0, r1) = (r0.unwrap_or(0), r1.unwrap_or(MAX_ROWS - 1));
    let (c0, c1) = (c0.unwrap_or(0), c1.unwrap_or(MAX_COLS - 1));
    Some(CellRange {
        first_row: r0.min(r1),
        first_col: c0.min(c1),
        last_row: r0.max(r1).min(MAX_ROWS - 1),
        last_col: c0.max(c1).min(MAX_COLS - 1),
    })
}

fn orient(n: f64) -> Res<Orient> {
    Ok(match n as i32 {
        0 => Orient::Hidden,
        1 => Orient::Row,
        2 => Orient::Col,
        3 => Orient::Page,
        4 => Orient::Data,
        _ => return Err(fail("Orientação inválida")),
    })
}

fn constant(name: &str) -> Option<f64> {
    Some(match name {
        "xlhidden" => 0.0,
        "xlrowfield" => 1.0,
        "xlcolumnfield" => 2.0,
        "xlpagefield" => 3.0,
        "xldatafield" => 4.0,
        "xlascending" => 1.0,
        "xldescending" => 2.0,
        "xlyes" => 1.0,
        "xlno" => 2.0,
        "xlguess" => 0.0,
        "xltoptobottom" | "xlsortcolumns" => 1.0,
        "xllefttoright" | "xlsortrows" => 2.0,
        "xlsortnormal" => 0.0,
        "xlup" => -4162.0,
        "xldown" => -4121.0,
        "xltoleft" => -4159.0,
        "xltoright" => -4161.0,
        "xlpastevalues" => -4163.0,
        "xlpasteall" => -4104.0,
        "xlpasteformats" => -4122.0,
        "xlpastecolumnwidths" => 8.0,
        "xlcalculationmanual" => -4135.0,
        "xlcalculationautomatic" => -4105.0,
        "xlsheetvisible" => -1.0,
        "xlsheethidden" => 0.0,
        "xlsheetveryhidden" => 2.0,
        "vbokonly" => 0.0,
        "vbokcancel" => 1.0,
        "vbyesnocancel" => 3.0,
        "vbyesno" => 4.0,
        "vbcritical" => 16.0,
        "vbquestion" => 32.0,
        "vbexclamation" => 48.0,
        "vbinformation" => 64.0,
        "vbdefaultbutton2" => 256.0,
        "vbok" => 1.0,
        "vbcancel" => 2.0,
        "vbyes" => 6.0,
        "vbno" => 7.0,
        "vbtextcompare" => 1.0,
        "vbbinarycompare" => 0.0,
        _ => return None,
    })
}

impl<'b, B: Backend> Excel<'b, B> {
    pub fn new(b: &'b mut B) -> Self {
        Self { b, copied: None }
    }

    fn sheet_by(&self, v: &V) -> Res<usize> {
        let list = self.b.sheets();
        match v {
            Val::Obj(Obj::Sheet(i)) => Ok(*i),
            Val::Str(s) => {
                list.iter().position(|n| n.to_lowercase() == s.to_lowercase()).ok_or_else(out_of_range)
            }
            v => {
                let i = to_num(v)? as usize;
                if (1..=list.len()).contains(&i) { Ok(i - 1) } else { Err(out_of_range()) }
            }
        }
    }

    fn num(&self, a: &A, i: usize, name: &str) -> Res<Option<f64>> {
        a.get(i, name).map(to_num).transpose()
    }

    /// `Range("A1:B2")`, `Range(cel1, cel2)`.
    fn range_arg(&mut self, sheet: usize, a: &A) -> Res<V> {
        let first = a.get(0, "cell1").ok_or_else(|| fail("Range sem referência"))?;
        let one = |it: &mut Self, v: &V| -> Res<(usize, CellRange)> {
            match v {
                Val::Obj(Obj::Range(s, r) | Obj::Cells(s, r)) => Ok((*s, *r)),
                v => {
                    let t = to_str(v)?;
                    let (s, t) = match t.rfind('!') {
                        Some(i) => {
                            let n = t[..i].trim_matches('\'').to_string();
                            (it.sheet_by(&Val::Str(n))?, t[i + 1..].to_string())
                        }
                        None => (sheet, t),
                    };
                    parse_ref(&t).map(|r| (s, r)).ok_or_else(|| fail(format!("Referência inválida: {t}")))
                }
            }
        };
        let (s, a0) = one(self, first)?;
        let Some(second) = a.get(1, "cell2") else { return Ok(Val::Obj(Obj::Range(s, a0))) };
        let (_, b0) = one(self, second)?;
        Ok(Val::Obj(Obj::Range(
            s,
            CellRange {
                first_row: a0.first_row.min(b0.first_row),
                first_col: a0.first_col.min(b0.first_col),
                last_row: a0.last_row.max(b0.last_row),
                last_col: a0.last_col.max(b0.last_col),
            },
        )))
    }

    /// `x.Cells(linha, coluna)` dentro de um intervalo.
    fn cells_arg(&mut self, s: usize, r: CellRange, a: &A) -> Res<V> {
        let Some(row) = self.num(a, 0, "rowindex")? else { return Ok(Val::Obj(Obj::Cells(s, r))) };
        let (rows, cols) = size(r);
        let (row, col) = match a.get(1, "columnindex") {
            Some(Val::Str(c)) => (row, col_index(c).ok_or_else(|| fail("Coluna inválida"))? as f64 + 1.0),
            Some(v) => (row, to_num(v)?),
            // Um índice só: em ordem de linha.
            None => {
                let k = row - 1.0;
                ((k / cols as f64).floor() + 1.0, k % cols as f64 + 1.0)
            }
        };
        let _ = rows;
        let (rr, cc) = (r.first_row as f64 + row - 1.0, r.first_col as f64 + col - 1.0);
        if rr < 0.0 || cc < 0.0 || rr >= MAX_ROWS as f64 || cc >= MAX_COLS as f64 {
            return Err(fail("Célula fora da planilha"));
        }
        Ok(Val::Obj(Obj::Range(s, cell(rr as u32, cc as u32))))
    }

    fn value(&mut self, s: usize, r: CellRange) -> Res<V> {
        self.b.flush().map_err(fail)?;
        let (rows, cols) = size(r);
        if (rows, cols) == (1, 1) {
            return Ok(cell_val(self.b.value(s, r.first_row, r.first_col)));
        }
        // Colunas inteiras: só a parte usada.
        let last = self.b.used_range(s).map_or(0, |u| u.last_row);
        let rows = if rows == MAX_ROWS { (last + 1).saturating_sub(r.first_row).max(1) } else { rows };
        if rows as u64 * cols as u64 > 20_000_000 {
            return Err(VbErr::new(7, "Memória insuficiente"));
        }
        let mut arr = Arr::new(vec![(1, rows as i64), (1, cols as i64)]);
        for i in 0..rows {
            for j in 0..cols {
                arr.data[(i * cols + j) as usize] =
                    cell_val(self.b.value(s, r.first_row + i, r.first_col + j));
            }
        }
        Ok(Val::Arr(std::rc::Rc::new(std::cell::RefCell::new(arr))))
    }

    fn set_value(&mut self, s: usize, r: CellRange, v: &V) -> Res<()> {
        let (rows, cols) = size(r);
        if rows as u64 * cols as u64 > 5_000_000 {
            return Err(VbErr::new(7, "Intervalo grande demais"));
        }
        let vals = match v {
            Val::Arr(a) => {
                let a = a.borrow();
                // Uma dimensão preenche a linha; duas, linha × coluna.
                let (ar, ac) = match a.bounds[..] {
                    [(x, y)] => (1, (y - x + 1) as usize),
                    [(x, y), (z, w)] => ((y - x + 1) as usize, (w - z + 1) as usize),
                    _ => return Err(VbErr::new(13, "Tipos incompatíveis")),
                };
                let mut out = Vec::with_capacity((rows * cols) as usize);
                for i in 0..rows as usize {
                    for j in 0..cols as usize {
                        let i = if ar == 1 { 0 } else { i };
                        out.push(match a.data.get(i * ac + j) {
                            Some(v) if i < ar && j < ac => val_cell(v)?,
                            _ => CellValue::Empty,
                        });
                    }
                }
                out
            }
            Val::Obj(Obj::Range(s2, r2)) => {
                let v = self.value(*s2, *r2)?;
                return self.set_value(s, r, &v);
            }
            v => vec![val_cell(v)?; (rows * cols) as usize],
        };
        self.b.set_values(s, r, vals).map_err(fail)
    }

    fn range_get(&mut self, s: usize, r: CellRange, name: &str, a: &A) -> Res<V> {
        let (rows, cols) = size(r);
        let obj = |o: Obj| Ok(Val::Obj(o));
        match name {
            "value" | "value2" => self.value(s, r),
            "text" => {
                let v = self.value(s, cell(r.first_row, r.first_col))?;
                Ok(Val::Str(to_str(&v)?))
            }
            "cells" => self.cells_arg(s, r, a),
            "range" => {
                let Val::Obj(Obj::Range(_, q)) = self.range_arg(s, a)? else { return Err(out_of_range()) };
                obj(Obj::Range(
                    s,
                    CellRange {
                        first_row: r.first_row + q.first_row,
                        first_col: r.first_col + q.first_col,
                        last_row: r.first_row + q.last_row,
                        last_col: r.first_col + q.last_col,
                    },
                ))
            }
            "rows" | "columns" => {
                let by_row = name == "rows";
                match a.get(0, "index") {
                    None if by_row => obj(Obj::RangeRows(s, r)),
                    None => obj(Obj::RangeCols(s, r)),
                    Some(v) => {
                        let k = match v {
                            Val::Str(c) if !by_row => col_index(c).ok_or_else(out_of_range)?,
                            v => to_num(v)? as u32 - 1,
                        };
                        obj(Obj::Range(
                            s,
                            if by_row {
                                CellRange { first_row: r.first_row + k, last_row: r.first_row + k, ..r }
                            } else {
                                CellRange { first_col: r.first_col + k, last_col: r.first_col + k, ..r }
                            },
                        ))
                    }
                }
            }
            "count" | "countlarge" => Ok(Val::Num(rows as f64 * cols as f64)),
            "row" => Ok(Val::Num(r.first_row as f64 + 1.0)),
            "column" => Ok(Val::Num(r.first_col as f64 + 1.0)),
            "address" => Ok(Val::Str(address(r))),
            "parent" | "worksheet" => obj(Obj::Sheet(s)),
            "entirerow" => obj(Obj::Range(s, CellRange { first_col: 0, last_col: MAX_COLS - 1, ..r })),
            "entirecolumn" => obj(Obj::Range(s, CellRange { first_row: 0, last_row: MAX_ROWS - 1, ..r })),
            "currentregion" => obj(Obj::Range(s, self.b.used_range(s).unwrap_or(r))),
            "offset" => {
                let dr = self.num(a, 0, "rowoffset")?.unwrap_or(0.0);
                let dc = self.num(a, 1, "columnoffset")?.unwrap_or(0.0);
                let (r0, c0) = (r.first_row as f64 + dr, r.first_col as f64 + dc);
                if r0 < 0.0 || c0 < 0.0 {
                    return Err(fail("Deslocamento fora da planilha"));
                }
                let (r0, c0) = (r0 as u32, c0 as u32);
                obj(Obj::Range(
                    s,
                    CellRange {
                        first_row: r0,
                        first_col: c0,
                        last_row: (r0 + rows - 1).min(MAX_ROWS - 1),
                        last_col: (c0 + cols - 1).min(MAX_COLS - 1),
                    },
                ))
            }
            "resize" => {
                let nr = self.num(a, 0, "rowsize")?.unwrap_or(rows as f64).max(1.0) as u32;
                let nc = self.num(a, 1, "columnsize")?.unwrap_or(cols as f64).max(1.0) as u32;
                obj(Obj::Range(
                    s,
                    CellRange {
                        last_row: (r.first_row + nr - 1).min(MAX_ROWS - 1),
                        last_col: (r.first_col + nc - 1).min(MAX_COLS - 1),
                        ..r
                    },
                ))
            }
            "end" => {
                let dir = self.num(a, 0, "direction")?.unwrap_or(-4121.0) as i32;
                let (row, col) = self.end(s, r.first_row, r.first_col, dir)?;
                obj(Obj::Range(s, cell(row, col)))
            }
            "clearcontents" | "clear" => {
                self.clear(s, r)?;
                Ok(Val::Empty)
            }
            "copy" => {
                self.copied = Some((s, r));
                match a.get(0, "destination") {
                    Some(Val::Obj(Obj::Range(s2, d))) => {
                        let v = self.value(s, r)?;
                        let d = CellRange {
                            last_row: d.first_row + rows - 1,
                            last_col: d.first_col + cols - 1,
                            ..*d
                        };
                        self.set_value(*s2, d, &v)?;
                        Ok(Val::Empty)
                    }
                    _ => Ok(Val::Empty),
                }
            }
            "pastespecial" => {
                let kind = self.num(a, 0, "paste")?.unwrap_or(-4104.0) as i32;
                let Some((s0, r0)) = self.copied else { return Err(fail("Nada para colar")) };
                if kind != -4163 {
                    return Ok(Val::Empty);
                }
                if (s0, r0.first_row, r0.first_col) == (s, r.first_row, r.first_col) {
                    self.b.freeze_values(s, r0).map_err(fail)?;
                } else {
                    let v = self.value(s0, r0)?;
                    let (h, w) = size(r0);
                    let d = CellRange { last_row: r.first_row + h - 1, last_col: r.first_col + w - 1, ..r };
                    self.set_value(s, d, &v)?;
                }
                Ok(Val::Empty)
            }
            "select" | "activate" => {
                self.b.select(s, r);
                Ok(Val::Empty)
            }
            "sort" => {
                let key = match a.get(0, "key1") {
                    Some(Val::Obj(Obj::Range(_, k))) => k.first_col,
                    Some(v) => parse_ref(&to_str(v)?).ok_or_else(|| fail("Chave inválida"))?.first_col,
                    None => r.first_col,
                };
                let desc = self.num(a, 1, "order1")?.unwrap_or(1.0) == 2.0;
                let header = self.num(a, 7, "header")?.unwrap_or(2.0) == 1.0;
                let r = if header && rows > 1 { CellRange { first_row: r.first_row + 1, ..r } } else { r };
                self.b.sort(s, r, key, desc).map_err(fail)?;
                Ok(Val::Empty)
            }
            "hidden" => Ok(Val::Bool(false)),
            "font" | "interior" | "borders" | "validation" | "formatconditions" => obj(Obj::Format),
            "specialcells" | "find" | "autofilter" | "autofill" | "delete" | "insert" => {
                Err(VbErr::unsupported(&format!("Range.{name}")))
            }
            "autofit" | "calculate" | "rowheight" | "columnwidth" => Ok(Val::Empty),
            _ => Err(VbErr::unsupported(&format!("Range.{name}"))),
        }
    }

    fn clear(&mut self, s: usize, r: CellRange) -> Res<()> {
        // Só a parte usada (colunas inteiras).
        let Some(u) = self.b.used_range(s) else { return Ok(()) };
        let r = CellRange { last_row: r.last_row.min(u.last_row), last_col: r.last_col.min(u.last_col), ..r };
        if r.last_row < r.first_row || r.last_col < r.first_col {
            return Ok(());
        }
        let (rows, cols) = size(r);
        self.b.set_values(s, r, vec![CellValue::Empty; (rows * cols) as usize]).map_err(fail)
    }

    /// `Range.End(direção)`.
    fn end(&mut self, s: usize, row: u32, col: u32, dir: i32) -> Res<(u32, u32)> {
        self.b.flush().map_err(fail)?;
        let u = self.b.used_range(s).unwrap_or(cell(0, 0));
        let (dr, dc): (i64, i64) = match dir {
            -4162 => (-1, 0),
            -4121 => (1, 0),
            -4159 => (0, -1),
            _ => (0, 1),
        };
        // Começando depois da área usada, pula direto para ela.
        let (mut r, mut c) = (row as i64, col as i64);
        if dr < 0 && r > u.last_row as i64 {
            r = u.last_row as i64 + 1;
        }
        if dc < 0 && c > u.last_col as i64 {
            c = u.last_col as i64 + 1;
        }
        let empty =
            |it: &mut Self, r: i64, c: i64| matches!(it.b.value(s, r as u32, c as u32), CellValue::Empty);
        let inside =
            |r: i64, c: i64| r >= 0 && c >= 0 && r <= u.last_row as i64 + 1 && c <= u.last_col as i64 + 1;
        let start_empty = empty(self, r, c);
        let mut moved = false;
        loop {
            let (nr, nc) = (r + dr, c + dc);
            if !inside(nr, nc) {
                break;
            }
            let e = empty(self, nr, nc);
            if !start_empty && e && moved {
                break;
            }
            if !start_empty && e && !moved {
                // Vizinha vazia: vai até o próximo valor.
                let (mut qr, mut qc) = (nr, nc);
                while inside(qr, qc) && empty(self, qr, qc) {
                    qr += dr;
                    qc += dc;
                }
                (r, c) = if inside(qr, qc) { (qr, qc) } else { (qr - dr, qc - dc) };
                return Ok((r.max(0) as u32, c.max(0) as u32));
            }
            (r, c) = (nr, nc);
            moved = true;
            if start_empty && !e {
                break;
            }
        }
        Ok((r.max(0) as u32, c.max(0) as u32))
    }

    fn pivot_field(&mut self, s: usize, p: usize, v: &V) -> Res<usize> {
        let list = self.b.pivot_fields(s, p);
        match v {
            Val::Str(n) => {
                let k = n.to_lowercase();
                list.iter()
                    .find(|f| f.name.to_lowercase() == k)
                    .or_else(|| list.iter().find(|f| f.caption.to_lowercase() == k))
                    .map(|f| f.id)
                    .ok_or_else(|| fail(format!("Campo não encontrado: {n}")))
            }
            v => list.get((to_num(v)? as usize).wrapping_sub(1)).map(|f| f.id).ok_or_else(out_of_range),
        }
    }

    fn table(&self, s: usize, name: &str) -> Res<Table> {
        self.b
            .tables(s)
            .into_iter()
            .find(|t| t.name.to_lowercase() == name.to_lowercase())
            .ok_or_else(out_of_range)
    }

    /// Linhas de dados de uma tabela (`None` = vazia).
    fn body(t: &Table) -> Option<CellRange> {
        let r = CellRange {
            first_row: t.range.first_row + t.header_rows,
            last_row: t.range.last_row.checked_sub(t.totals_rows)?,
            ..t.range
        };
        (r.last_row >= r.first_row).then_some(r)
    }

    fn opt_range(s: usize, r: Option<CellRange>) -> V {
        r.map_or(Val::Nothing, |r| Val::Obj(Obj::Range(s, r)))
    }
}

fn address(r: CellRange) -> String {
    let one = |row: u32, col: u32| {
        let mut n = col + 1;
        let mut s = String::new();
        while n > 0 {
            s.insert(0, (b'A' + ((n - 1) % 26) as u8) as char);
            n = (n - 1) / 26;
        }
        format!("${s}${}", row + 1)
    };
    let a = one(r.first_row, r.first_col);
    if (r.first_row, r.first_col) == (r.last_row, r.last_col) {
        a
    } else {
        format!("{a}:{}", one(r.last_row, r.last_col))
    }
}

impl<B: Backend> Host for Excel<'_, B> {
    type Obj = Obj;

    fn global(&mut self, name: &str, a: &A) -> Option<Res<V>> {
        if let Some(n) = constant(name) {
            return Some(Ok(Val::Num(n)));
        }
        let active = self.b.active_sheet();
        let o = |o: Obj| Some(Ok(Val::Obj(o)));
        match name {
            "vbnewline" | "vbcrlf" | "vblf" => Some(Ok(Val::Str("\n".into()))),
            "vbtab" => Some(Ok(Val::Str("\t".into()))),
            "vbnullstring" => Some(Ok(Val::Str(String::new()))),
            "application" => o(Obj::Application),
            "thisworkbook" | "activeworkbook" => o(Obj::Workbook),
            "activesheet" => o(Obj::Sheet(active)),
            "activewindow" => o(Obj::ActiveWindow),
            "sheets" | "worksheets" => Some(match a.pos.first() {
                None => Ok(Val::Obj(Obj::Sheets)),
                Some(v) => self.sheet_by(v).map(|i| Val::Obj(Obj::Sheet(i))),
            }),
            "range" => Some(self.range_arg(active, a)),
            "cells" => Some(self.cells_arg(active, WHOLE, a)),
            "rows" | "columns" => Some(self.range_get(active, WHOLE, name, a)),
            "worksheetfunction" => o(Obj::WorksheetFunction),
            "calculate" => {
                self.b.calculate();
                Some(Ok(Val::Empty))
            }
            "msgbox" => Some((|| {
                self.b.flush().map_err(fail)?;
                let text = a.get(0, "prompt").map(to_str).transpose()?.unwrap_or_default();
                let buttons = self.num(a, 1, "buttons")?.unwrap_or(0.0) as i32;
                let title = a.get(2, "title").map(to_str).transpose()?.unwrap_or_default();
                Ok(Val::Num(self.b.msgbox(&text, buttons, &title) as f64))
            })()),
            "inputbox" => Some(Err(VbErr::unsupported("InputBox"))),
            _ => None,
        }
    }

    fn get(&mut self, o: &Obj, name: &str, a: &A) -> Res<V> {
        let obj = |o: Obj| Ok(Val::Obj(o));
        match o {
            Obj::Application => match name {
                "worksheetfunction" => obj(Obj::WorksheetFunction),
                "commandbars" => obj(Obj::CommandBars),
                "activesheet" => obj(Obj::Sheet(self.b.active_sheet())),
                "thisworkbook" | "activeworkbook" => obj(Obj::Workbook),
                "activewindow" => obj(Obj::ActiveWindow),
                "calculate" | "calculatefull" => {
                    self.b.calculate();
                    Ok(Val::Empty)
                }
                "screenupdating" | "enableevents" | "displayalerts" => Ok(Val::Bool(true)),
                "calculation" => Ok(Val::Num(-4105.0)),
                "statusbar" | "cutcopymode" => Ok(Val::Bool(false)),
                "goto" => Ok(Val::Empty),
                "wait" => Ok(Val::Bool(true)),
                _ => match self.global(name, a) {
                    Some(v) => v,
                    None => Err(VbErr::unsupported(&format!("Application.{name}"))),
                },
            },
            Obj::WorksheetFunction => {
                let s = |i: usize| to_str(a.pos.get(i).unwrap_or(&Val::Empty));
                match name {
                    // O `Trim` da planilha também junta espaços de dentro.
                    "trim" => {
                        Ok(Val::Str(s(0)?.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")))
                    }
                    "sum" | "max" | "min" | "count" | "average" => {
                        let mut v = Vec::new();
                        for x in &a.pos {
                            let x = match x {
                                Val::Obj(Obj::Range(s, r)) => self.value(*s, *r)?,
                                x => x.clone(),
                            };
                            match x {
                                Val::Arr(a) => v.extend(a.borrow().data.iter().filter_map(|x| match x {
                                    Val::Num(n) | Val::Date(n) => Some(*n),
                                    _ => None,
                                })),
                                Val::Num(n) | Val::Date(n) => v.push(n),
                                _ => {}
                            }
                        }
                        let sum: f64 = v.iter().sum();
                        Ok(Val::Num(match name {
                            "sum" => sum,
                            "count" => v.len() as f64,
                            "average" if v.is_empty() => return Err(VbErr::new(11, "Divisão por zero")),
                            "average" => sum / v.len() as f64,
                            "max" => v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                            _ => v.iter().copied().fold(f64::INFINITY, f64::min),
                        }))
                    }
                    _ => Err(VbErr::unsupported(&format!("WorksheetFunction.{name}"))),
                }
            }
            Obj::CommandBars => Err(VbErr::unsupported(&format!("CommandBars.{name}"))),
            Obj::ActiveWindow | Obj::Format => Ok(Val::Empty),
            Obj::Workbook => match name {
                "sheets" | "worksheets" => match a.pos.first() {
                    None => obj(Obj::Sheets),
                    Some(v) => obj(Obj::Sheet(self.sheet_by(v)?)),
                },
                "name" => Ok(Val::Str(self.b.workbook_name())),
                "activesheet" => obj(Obj::Sheet(self.b.active_sheet())),
                "refreshall" => {
                    for s in 0..self.b.sheets().len() {
                        for p in 0..self.b.pivots(s).len() {
                            // As ligadas a fontes externas não atualizam aqui.
                            let _ = self.b.refresh_pivot(s, p);
                        }
                    }
                    Ok(Val::Empty)
                }
                _ => Err(VbErr::unsupported(&format!("Workbook.{name}"))),
            },
            Obj::Sheets => match name {
                "count" => Ok(Val::Num(self.b.sheets().len() as f64)),
                "item" => {
                    let v = a.pos.first().cloned().unwrap_or(Val::Empty);
                    obj(Obj::Sheet(self.sheet_by(&v)?))
                }
                _ => Err(VbErr::unsupported(&format!("Sheets.{name}"))),
            },
            Obj::Sheet(s) => {
                let s = *s;
                match name {
                    "name" => Ok(Val::Str(self.b.sheets().get(s).cloned().ok_or_else(out_of_range)?)),
                    "index" => Ok(Val::Num(s as f64 + 1.0)),
                    "range" => self.range_arg(s, a),
                    "cells" => self.cells_arg(s, WHOLE, a),
                    "rows" | "columns" => self.range_get(s, WHOLE, name, a),
                    "usedrange" => {
                        self.b.flush().map_err(fail)?;
                        Ok(Self::opt_range(s, Some(self.b.used_range(s).unwrap_or(cell(0, 0)))))
                    }
                    "pivottables" => match a.pos.first() {
                        None => obj(Obj::Pivots(s)),
                        Some(_) => self.default(&Obj::Pivots(s), a),
                    },
                    "listobjects" => match a.pos.first() {
                        None => obj(Obj::Tables(s)),
                        Some(_) => self.default(&Obj::Tables(s), a),
                    },
                    "activate" | "select" => {
                        self.b.activate(s);
                        Ok(Val::Empty)
                    }
                    "delete" => {
                        self.b.flush().map_err(fail)?;
                        self.b.delete_sheet(s).map_err(fail)?;
                        Ok(Val::Empty)
                    }
                    "calculate" => {
                        self.b.calculate();
                        Ok(Val::Empty)
                    }
                    "visible" => Ok(Val::Num(-1.0)),
                    "protect" | "unprotect" => Ok(Val::Empty),
                    _ => Err(VbErr::unsupported(&format!("Worksheet.{name}"))),
                }
            }
            Obj::Range(s, r) => self.range_get(*s, *r, name, a),
            Obj::Cells(s, r) => self.range_get(*s, *r, name, a),
            Obj::RangeRows(s, r) | Obj::RangeCols(s, r) => {
                let by_row = matches!(o, Obj::RangeRows(..));
                match name {
                    "count" => {
                        let (rows, cols) = size(*r);
                        Ok(Val::Num(if by_row { rows } else { cols } as f64))
                    }
                    _ => self.range_get(*s, *r, name, a),
                }
            }
            Obj::Pivots(s) => match name {
                "count" => Ok(Val::Num(self.b.pivots(*s).len() as f64)),
                _ => self.default(o, a),
            },
            Obj::Pivot(s, p) => {
                let (s, p) = (*s, *p);
                let axis = |x: Orient| Ok(Val::Obj(Obj::Fields(s, p, Some(x))));
                match name {
                    "name" => Ok(Val::Str(self.b.pivots(s).get(p).cloned().unwrap_or_default())),
                    "rowfields" if a.pos.is_empty() => axis(Orient::Row),
                    "columnfields" if a.pos.is_empty() => axis(Orient::Col),
                    "pagefields" if a.pos.is_empty() => axis(Orient::Page),
                    "pivotfields" => match a.pos.first() {
                        None => obj(Obj::Fields(s, p, None)),
                        Some(v) => {
                            let f = self.pivot_field(s, p, v)?;
                            obj(Obj::Field(s, p, f))
                        }
                    },
                    "cubefields" => {
                        if !self.b.pivot_is_cube(s, p) {
                            return Err(fail("A tabela dinâmica não é do Modelo de Dados"));
                        }
                        match a.pos.first() {
                            None => obj(Obj::CubeFields(s, p)),
                            Some(v) => {
                                let f = self.pivot_field(s, p, v)?;
                                obj(Obj::Field(s, p, f))
                            }
                        }
                    }
                    "databodyrange" | "rowrange" | "tablerange1" | "tablerange2" => {
                        self.b.flush().map_err(fail)?;
                        let (all, body, rows) = self.b.pivot_ranges(s, p).map_err(fail)?;
                        Ok(match name {
                            "databodyrange" => Self::opt_range(s, body),
                            "rowrange" => Self::opt_range(s, Some(rows)),
                            _ => Self::opt_range(s, Some(all)),
                        })
                    }
                    "manualupdate" => Ok(Val::Bool(false)),
                    "refreshtable" | "update" => {
                        self.b.refresh_pivot(s, p).map_err(fail)?;
                        Ok(Val::Bool(true))
                    }
                    "pivotcache" => obj(Obj::Pivot(s, p)),
                    "refresh" => {
                        self.b.refresh_pivot(s, p).map_err(fail)?;
                        Ok(Val::Empty)
                    }
                    _ => Err(VbErr::unsupported(&format!("PivotTable.{name}"))),
                }
            }
            Obj::Fields(s, p, axis) => match name {
                "count" => {
                    let n = match axis {
                        Some(x) => self.b.pivot_axis(*s, *p, *x).len(),
                        None => self.b.pivot_fields(*s, *p).len(),
                    };
                    Ok(Val::Num(n as f64))
                }
                _ => self.default(o, a),
            },
            Obj::CubeFields(..) => self.default(o, a),
            Obj::Field(s, p, f) => {
                let list = self.b.pivot_fields(*s, *p);
                let fd = list.iter().find(|x| x.id == *f).cloned().ok_or_else(out_of_range)?;
                match name {
                    "name" | "value" => Ok(Val::Str(fd.name)),
                    "caption" | "sourcename" => Ok(Val::Str(fd.caption)),
                    "orientation" => {
                        for (x, n) in [(Orient::Row, 1.0), (Orient::Col, 2.0), (Orient::Page, 3.0)] {
                            if self.b.pivot_axis(*s, *p, x).contains(f) {
                                return Ok(Val::Num(n));
                            }
                        }
                        Ok(Val::Num(0.0))
                    }
                    "position" => {
                        for x in [Orient::Row, Orient::Col, Orient::Page] {
                            if let Some(i) = self.b.pivot_axis(*s, *p, x).iter().position(|q| q == f) {
                                return Ok(Val::Num(i as f64 + 1.0));
                            }
                        }
                        Err(fail("O campo não está na tabela"))
                    }
                    // Um campo de cubo tem um campo de dinâmica por nível.
                    "pivotfields" => Ok(crate::interp::array(vec![Val::Obj(o.clone())])),
                    "showdetail" => Ok(Val::Bool(true)),
                    _ => Err(VbErr::unsupported(&format!("PivotField.{name}"))),
                }
            }
            Obj::Tables(s) => match name {
                "count" => Ok(Val::Num(self.b.tables(*s).len() as f64)),
                _ => self.default(o, a),
            },
            Obj::Table(s, t) => {
                let tb = self.table(*s, t)?;
                match name {
                    "name" => Ok(Val::Str(tb.name)),
                    "range" => obj(Obj::Range(*s, tb.range)),
                    "databodyrange" => Ok(Self::opt_range(*s, Self::body(&tb))),
                    "headerrowrange" => {
                        obj(Obj::Range(*s, CellRange { last_row: tb.range.first_row, ..tb.range }))
                    }
                    "listcolumns" => match a.pos.first() {
                        None => obj(Obj::TableCols(*s, t.clone())),
                        Some(_) => self.default(&Obj::TableCols(*s, t.clone()), a),
                    },
                    "listrows" => match a.pos.first() {
                        None => obj(Obj::TableRows(*s, t.clone())),
                        Some(v) => obj(Obj::TableRow(*s, t.clone(), to_num(v)? as u32 - 1)),
                    },
                    "parent" => obj(Obj::Sheet(*s)),
                    _ => Err(VbErr::unsupported(&format!("ListObject.{name}"))),
                }
            }
            Obj::TableCols(s, t) => match name {
                "count" => Ok(Val::Num(self.table(*s, t)?.columns.len() as f64)),
                _ => self.default(o, a),
            },
            Obj::TableCol(s, t, c) => {
                let tb = self.table(*s, t)?;
                let col = tb.range.first_col + *c as u32;
                match name {
                    "index" => Ok(Val::Num(*c as f64 + 1.0)),
                    "name" => Ok(Val::Str(tb.columns.get(*c).cloned().unwrap_or_default())),
                    "databodyrange" => Ok(Self::opt_range(
                        *s,
                        Self::body(&tb).map(|r| CellRange { first_col: col, last_col: col, ..r }),
                    )),
                    "range" => obj(Obj::Range(*s, CellRange { first_col: col, last_col: col, ..tb.range })),
                    _ => Err(VbErr::unsupported(&format!("ListColumn.{name}"))),
                }
            }
            Obj::TableRows(s, t) => match name {
                "count" => {
                    let tb = self.table(*s, t)?;
                    Ok(Val::Num(Self::body(&tb).map_or(0.0, |r| size(r).0 as f64)))
                }
                "add" => {
                    let row = self.b.add_table_row(*s, t).map_err(fail)?;
                    let tb = self.table(*s, t)?;
                    obj(Obj::TableRow(*s, t.clone(), row - tb.range.first_row - tb.header_rows))
                }
                _ => self.default(o, a),
            },
            Obj::TableRow(s, t, k) => {
                let tb = self.table(*s, t)?;
                let row = tb.range.first_row + tb.header_rows + k;
                let r = CellRange { first_row: row, last_row: row, ..tb.range };
                match name {
                    "range" => match a.pos.first() {
                        None => obj(Obj::Range(*s, r)),
                        Some(_) => self.cells_arg(*s, r, a),
                    },
                    "index" => Ok(Val::Num(*k as f64 + 1.0)),
                    _ => Err(VbErr::unsupported(&format!("ListRow.{name}"))),
                }
            }
        }
    }

    fn set(&mut self, o: &Obj, name: &str, a: &A, v: V) -> Res<()> {
        match o {
            Obj::Application => {
                if name == "statusbar" {
                    match &v {
                        Val::Bool(_) => self.b.status(None),
                        v => self.b.status(Some(&to_str(v)?)),
                    }
                }
                // Atualização de tela, eventos e modo de cálculo: sem efeito aqui.
                Ok(())
            }
            Obj::ActiveWindow | Obj::Format | Obj::CommandBars => Ok(()),
            Obj::Range(s, r) | Obj::Cells(s, r) | Obj::RangeRows(s, r) | Obj::RangeCols(s, r) => match name {
                "value" | "value2" | "formula" | "formulalocal" => self.set_value(*s, *r, &v),
                "hidden" => {
                    let rows = matches!(o, Obj::RangeRows(..)) || size(*r).1 == MAX_COLS;
                    let (a, b) = if rows { (r.first_row, r.last_row) } else { (r.first_col, r.last_col) };
                    self.b.set_hidden(*s, rows, a, b, truth(&v)?).map_err(fail)
                }
                "columnwidth" | "rowheight" | "numberformat" | "horizontalalignment" | "wraptext" => Ok(()),
                _ => Err(VbErr::unsupported(&format!("Range.{name}"))),
            },
            Obj::Pivot(..) if matches!(name, "manualupdate") => {
                if !truth(&v)? {
                    self.b.flush().map_err(fail)?;
                }
                Ok(())
            }
            Obj::Field(s, p, f) => match name {
                "orientation" => {
                    let o = orient(to_num(&v)?)?;
                    self.b.set_orientation(*s, *p, *f, o).map_err(fail)
                }
                "position" => {
                    let k = to_num(&v)?.max(1.0) as usize - 1;
                    self.b.set_position(*s, *p, *f, k).map_err(fail)
                }
                "showdetail" => self.b.set_show_detail(*s, *p, *f, truth(&v)?).map_err(fail),
                _ => Err(VbErr::unsupported(&format!("PivotField.{name}"))),
            },
            Obj::Sheet(_) if matches!(name, "visible" | "name") => {
                Err(VbErr::unsupported(&format!("Worksheet.{name}")))
            }
            Obj::TableRow(..) | Obj::Sheet(_) => {
                let t = self.get(o, name, a)?;
                match t {
                    Val::Obj(t) => self.set(&t, "value", &Args::none(), v),
                    _ => Err(VbErr::unsupported(name)),
                }
            }
            _ => Err(VbErr::unsupported(name)),
        }
    }

    fn default(&mut self, o: &Obj, a: &A) -> Res<V> {
        let first = a.pos.first().cloned();
        match (o, first) {
            (Obj::Range(s, r) | Obj::Cells(s, r), None) => self.value(*s, *r),
            (Obj::Range(s, r) | Obj::Cells(s, r), Some(_)) => self.cells_arg(*s, *r, a),
            (Obj::RangeRows(s, r), Some(_)) => self.range_get(*s, *r, "rows", a),
            (Obj::RangeCols(s, r), Some(_)) => self.range_get(*s, *r, "columns", a),
            (Obj::RangeRows(s, r) | Obj::RangeCols(s, r), None) => self.value(*s, *r),
            (Obj::Sheets, Some(v)) => Ok(Val::Obj(Obj::Sheet(self.sheet_by(&v)?))),
            (Obj::Pivots(s), Some(v)) => {
                let list = self.b.pivots(*s);
                let i = match &v {
                    Val::Str(n) => list.iter().position(|x| x.to_lowercase() == n.to_lowercase()),
                    v => Some((to_num(v)? as usize).wrapping_sub(1)).filter(|i| *i < list.len()),
                };
                let i = i.ok_or_else(|| fail("Tabela dinâmica não encontrada"))?;
                Ok(Val::Obj(Obj::Pivot(*s, i)))
            }
            (Obj::Fields(s, p, axis), Some(v)) => {
                let f = match (axis, &v) {
                    (Some(x), Val::Num(n)) => {
                        let l = self.b.pivot_axis(*s, *p, *x);
                        l.get((*n as usize).wrapping_sub(1)).copied().ok_or_else(out_of_range)?
                    }
                    _ => self.pivot_field(*s, *p, &v)?,
                };
                Ok(Val::Obj(Obj::Field(*s, *p, f)))
            }
            (Obj::CubeFields(s, p), Some(v)) => {
                let f = self.pivot_field(*s, *p, &v)?;
                Ok(Val::Obj(Obj::Field(*s, *p, f)))
            }
            (Obj::Tables(s), Some(v)) => {
                let list = self.b.tables(*s);
                let t = match &v {
                    Val::Str(n) => list.iter().find(|t| t.name.to_lowercase() == n.to_lowercase()),
                    v => list.get((to_num(v)? as usize).wrapping_sub(1)),
                };
                Ok(Val::Obj(Obj::Table(*s, t.ok_or_else(out_of_range)?.name.clone())))
            }
            (Obj::TableCols(s, t), Some(v)) => {
                let tb = self.table(*s, t)?;
                let c = match &v {
                    Val::Str(n) => tb.columns.iter().position(|c| c.to_lowercase() == n.to_lowercase()),
                    v => Some((to_num(v)? as usize).wrapping_sub(1)).filter(|i| *i < tb.columns.len()),
                };
                Ok(Val::Obj(Obj::TableCol(*s, t.clone(), c.ok_or_else(out_of_range)?)))
            }
            (Obj::TableRows(s, t), Some(v)) => {
                Ok(Val::Obj(Obj::TableRow(*s, t.clone(), to_num(&v)? as u32 - 1)))
            }
            (Obj::Field(..) | Obj::Sheet(_) | Obj::Pivot(..) | Obj::Workbook, None) => {
                self.get(o, "name", &Args::none())
            }
            _ => Err(VbErr::new(438, "O objeto não tem valor")),
        }
    }

    fn items(&mut self, o: &Obj) -> Res<Vec<V>> {
        Ok(match o {
            Obj::Sheets => (0..self.b.sheets().len()).map(|i| Val::Obj(Obj::Sheet(i))).collect(),
            Obj::Pivots(s) => (0..self.b.pivots(*s).len()).map(|i| Val::Obj(Obj::Pivot(*s, i))).collect(),
            Obj::Fields(s, p, axis) => {
                let ids = match axis {
                    Some(x) => self.b.pivot_axis(*s, *p, *x),
                    None => self.b.pivot_fields(*s, *p).iter().map(|f| f.id).collect(),
                };
                ids.into_iter().map(|f| Val::Obj(Obj::Field(*s, *p, f))).collect()
            }
            Obj::CubeFields(s, p) => {
                self.b.pivot_fields(*s, *p).iter().map(|f| Val::Obj(Obj::Field(*s, *p, f.id))).collect()
            }
            Obj::Tables(s) => {
                self.b.tables(*s).into_iter().map(|t| Val::Obj(Obj::Table(*s, t.name))).collect()
            }
            Obj::TableCols(s, t) => {
                let n = self.table(*s, t)?.columns.len();
                (0..n).map(|c| Val::Obj(Obj::TableCol(*s, t.clone(), c))).collect()
            }
            Obj::TableRows(s, t) => {
                let n = Self::body(&self.table(*s, t)?).map_or(0, |r| size(r).0);
                (0..n).map(|k| Val::Obj(Obj::TableRow(*s, t.clone(), k))).collect()
            }
            Obj::Range(s, r) | Obj::Cells(s, r) => {
                self.b.flush().map_err(fail)?;
                let last = self.b.used_range(*s).map_or(0, |u| u.last_row);
                let mut out = Vec::new();
                for row in r.first_row..=r.last_row.min(last.max(r.first_row)) {
                    for c in r.first_col..=r.last_col {
                        out.push(Val::Obj(Obj::Range(*s, cell(row, c))));
                    }
                }
                out
            }
            Obj::RangeRows(s, r) => (r.first_row..=r.last_row)
                .map(|k| Val::Obj(Obj::Range(*s, CellRange { first_row: k, last_row: k, ..*r })))
                .collect(),
            Obj::RangeCols(s, r) => (r.first_col..=r.last_col)
                .map(|k| Val::Obj(Obj::Range(*s, CellRange { first_col: k, last_col: k, ..*r })))
                .collect(),
            _ => return Err(VbErr::new(438, "O objeto não é uma coleção")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interp::Interp;
    use crate::parse::parse_module;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Book {
        cells: HashMap<(usize, u32, u32), CellValue>,
        rows: Vec<usize>,
        msgs: Vec<String>,
        flushed: u32,
        table_rows: u32,
        sorted: Option<(CellRange, u32, bool)>,
    }

    impl Backend for Book {
        fn workbook_name(&self) -> String {
            "Teste.xlsm".into()
        }
        fn sheets(&self) -> Vec<String> {
            vec!["Dados".into(), "Painel".into()]
        }
        fn active_sheet(&self) -> usize {
            0
        }
        fn activate(&mut self, _: usize) {}
        fn value(&mut self, s: usize, r: u32, c: u32) -> CellValue {
            self.cells.get(&(s, r, c)).cloned().unwrap_or(CellValue::Empty)
        }
        fn set_values(&mut self, s: usize, r: CellRange, vals: Vec<CellValue>) -> R<()> {
            let mut it = vals.into_iter();
            for row in r.first_row..=r.last_row {
                for c in r.first_col..=r.last_col {
                    match it.next().unwrap() {
                        CellValue::Empty => {
                            self.cells.remove(&(s, row, c));
                        }
                        v => {
                            self.cells.insert((s, row, c), v);
                        }
                    }
                }
            }
            Ok(())
        }
        fn used_range(&self, s: usize) -> Option<CellRange> {
            let k: Vec<_> = self.cells.keys().filter(|k| k.0 == s).collect();
            Some(CellRange {
                first_row: 0,
                first_col: 0,
                last_row: k.iter().map(|k| k.1).max()?,
                last_col: k.iter().map(|k| k.2).max()?,
            })
        }
        fn select(&mut self, _: usize, _: CellRange) {}
        fn sort(&mut self, _: usize, r: CellRange, k: u32, d: bool) -> R<()> {
            self.sorted = Some((r, k, d));
            Ok(())
        }
        fn freeze_values(&mut self, _: usize, _: CellRange) -> R<()> {
            Ok(())
        }
        fn set_hidden(&mut self, _: usize, _: bool, _: u32, _: u32, _: bool) -> R<()> {
            Ok(())
        }
        fn delete_sheet(&mut self, _: usize) -> R<()> {
            Err("não".into())
        }
        fn tables(&self, s: usize) -> Vec<Table> {
            if s != 0 {
                return Vec::new();
            }
            vec![Table {
                name: "Tabela".into(),
                range: CellRange { first_row: 9, first_col: 0, last_row: 9 + self.table_rows, last_col: 1 },
                header_rows: 1,
                totals_rows: 0,
                columns: vec!["Nome".into(), "Qtd".into()],
            }]
        }
        fn add_table_row(&mut self, _: usize, _: &str) -> R<u32> {
            self.table_rows += 1;
            Ok(9 + self.table_rows)
        }
        fn pivots(&self, s: usize) -> Vec<String> {
            if s == 1 { vec!["Din1".into()] } else { Vec::new() }
        }
        fn pivot_fields(&mut self, _: usize, _: usize) -> Vec<Field> {
            ["Loja", "Estado", "Mês"]
                .iter()
                .enumerate()
                .map(|(id, n)| Field { id, name: n.to_string(), caption: n.to_string() })
                .collect()
        }
        fn pivot_axis(&mut self, _: usize, _: usize, x: Orient) -> Vec<usize> {
            if x == Orient::Row { self.rows.clone() } else { Vec::new() }
        }
        fn pivot_is_cube(&self, _: usize, _: usize) -> bool {
            false
        }
        fn set_orientation(&mut self, _: usize, _: usize, f: usize, o: Orient) -> R<()> {
            self.rows.retain(|x| *x != f);
            if o == Orient::Row {
                self.rows.push(f);
            }
            Ok(())
        }
        fn set_position(&mut self, _: usize, _: usize, f: usize, pos: usize) -> R<()> {
            self.rows.retain(|x| *x != f);
            self.rows.insert(pos, f);
            Ok(())
        }
        fn set_show_detail(&mut self, _: usize, _: usize, _: usize, _: bool) -> R<()> {
            Ok(())
        }
        fn refresh_pivot(&mut self, _: usize, _: usize) -> R<()> {
            Ok(())
        }
        fn pivot_ranges(&mut self, _: usize, _: usize) -> R<(CellRange, Option<CellRange>, CellRange)> {
            let r = |a, b, c, d| CellRange { first_row: a, first_col: b, last_row: c, last_col: d };
            Ok((r(2, 0, 6, 2), Some(r(3, 1, 6, 2)), r(2, 0, 6, 0)))
        }
        fn flush(&mut self) -> R<()> {
            self.flushed += 1;
            Ok(())
        }
        fn calculate(&mut self) {}
        fn msgbox(&mut self, t: &str, _: i32, _: &str) -> i32 {
            self.msgs.push(t.into());
            6
        }
        fn status(&mut self, _: Option<&str>) {}
    }

    fn run(b: &mut Book, src: &str) -> Result<(), VbErr> {
        let mut x = Excel::new(b);
        Interp::new(&mut x, vec![parse_module("M", src)]).run("main")
    }

    /// Trocar o campo de linha conforme a célula ligada aos botões de opção.
    #[test]
    fn swap_pivot_row_field() {
        let src = "Sub Main()\nConst ABA As String = \"Painel\"\nDim pt As PivotTable, pf As PivotField, ws As Worksheet\nDim n As Integer, campos As Variant\ncampos = Array(\"Loja\", \"Estado\", \"Mês\")\nApplication.ScreenUpdating = False\nSet ws = ThisWorkbook.Sheets(ABA)\nn = ws.Range(\"$B$1\").Value\nIf n < 1 Or n > UBound(campos) + 1 Then\n MsgBox \"fora: \" & n, vbExclamation\n Exit Sub\nEnd If\nOn Error Resume Next\nSet pt = ws.PivotTables(\"Din1\")\nOn Error GoTo 0\nIf pt Is Nothing Then Exit Sub\nFor Each pf In pt.RowFields\n pf.Orientation = xlHidden\nNext pf\nOn Error Resume Next\nSet pf = pt.PivotFields(campos(n - 1))\nOn Error GoTo 0\nIf pf Is Nothing Then MsgBox \"sem campo\": Exit Sub\npf.Orientation = xlRowField\npf.Position = 1\npt.ManualUpdate = False\nDim r As Range\nSet r = pt.DataBodyRange\nIf Not r Is Nothing Then\n With r\n  If .Columns.Count > 0 And .Cells.Count > 1 Then .Sort Key1:=.Cells(1, .Columns.Count), Order1:=xlDescending, Header:=xlNo\n End With\nEnd If\nEnd Sub";
        let mut b = Book { rows: vec![0], ..Default::default() };
        b.cells.insert((1, 0, 1), CellValue::Number(3.0));
        run(&mut b, src).unwrap();
        assert_eq!((b.rows, b.msgs.len()), (vec![2], 0));
        assert_eq!(
            b.sorted,
            Some((CellRange { first_row: 3, first_col: 1, last_row: 6, last_col: 2 }, 2, true))
        );
        assert!(b.flushed > 0);
        let mut b = Book::default();
        b.cells.insert((1, 0, 1), CellValue::Number(9.0));
        run(&mut b, src).unwrap();
        assert_eq!(b.msgs, ["fora: 9"]);
    }

    /// Ler uma tabela para um dicionário e acrescentar linhas.
    #[test]
    fn tables_ranges_and_dictionary() {
        let mut b = Book { table_rows: 2, ..Default::default() };
        for (i, n) in ["a", "b"].iter().enumerate() {
            b.cells.insert((0, 10 + i as u32, 0), CellValue::Text(n.to_string()));
        }
        run(&mut b, "Sub Main()\nDim lo As ListObject, d As Object, v, i As Long, lr As ListRow\nSet lo = ThisWorkbook.Sheets(\"Dados\").ListObjects(\"Tabela\")\nSet d = CreateObject(\"Scripting.Dictionary\")\nv = lo.ListColumns(\"Nome\").DataBodyRange.Value\nFor i = 1 To UBound(v, 1)\n d.Add Trim(CStr(v(i, 1))), Empty\nNext i\nIf Not d.Exists(\"c\") Then\n Set lr = lo.ListRows.Add\n With lr\n  .Range(lo.ListColumns(\"Nome\").Index).Value = \"c\"\n  .Range(2).Value = 7\n End With\nEnd If\nWith Sheets(\"Painel\")\n .Range(\"A1\").Resize(d.Count, 1).Value = \"x\"\n .Range(\"C1\").Value = .Cells(.Rows.Count, \"A\").End(xlUp).Row\n .Range(\"A2:A99\").ClearContents\nEnd With\nEnd Sub").unwrap();
        assert_eq!(b.cells[&(0, 12, 0)], CellValue::Text("c".into()));
        assert_eq!(b.cells[&(0, 12, 1)], CellValue::Number(7.0));
        assert_eq!(b.cells[&(1, 0, 2)], CellValue::Number(2.0));
        assert!(!b.cells.contains_key(&(1, 1, 0)) && b.cells.contains_key(&(1, 0, 0)));
    }

    #[test]
    fn references() {
        assert_eq!(parse_ref("$B$2"), Some(cell(1, 1)));
        assert_eq!(parse_ref("A:B").unwrap().last_row, MAX_ROWS - 1);
        assert_eq!(parse_ref("3:3").unwrap().last_col, MAX_COLS - 1);
        assert_eq!(parse_ref("Nome"), None);
        assert_eq!(address(parse_ref("AA3:AB4").unwrap()), "$AA$3:$AB$4");
    }
}
