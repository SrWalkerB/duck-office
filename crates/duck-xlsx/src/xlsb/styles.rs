//! `styles.bin` → `styles.xml`. O resto da leitura (e o gravador, ao salvar
//! como `.xlsx`) usa o XML, como num `.xlsx` qualquer.

use super::out::{attr, flag};
use super::rec::{Cur, Records, color_attrs, id};

const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

const PATTERNS: [&str; 19] = [
    "none",
    "solid",
    "mediumGray",
    "darkGray",
    "lightGray",
    "darkHorizontal",
    "darkVertical",
    "darkDown",
    "darkUp",
    "darkGrid",
    "darkTrellis",
    "lightHorizontal",
    "lightVertical",
    "lightDown",
    "lightUp",
    "lightGrid",
    "lightTrellis",
    "gray125",
    "gray0625",
];

const BORDERS: [&str; 14] = [
    "none",
    "thin",
    "medium",
    "dashed",
    "dotted",
    "thick",
    "double",
    "hair",
    "mediumDashed",
    "dashDot",
    "mediumDashDot",
    "dashDotDot",
    "mediumDashDotDot",
    "slantDashDot",
];

const HALIGN: [&str; 8] =
    ["general", "left", "center", "right", "fill", "justify", "centerContinuous", "distributed"];
const VALIGN: [&str; 5] = ["top", "center", "bottom", "justify", "distributed"];

fn underline(code: u16) -> Option<&'static str> {
    Some(match code {
        1 => "<u/>",
        2 => "<u val=\"double\"/>",
        0x21 => "<u val=\"singleAccounting\"/>",
        0x22 => "<u val=\"doubleAccounting\"/>",
        _ => return None,
    })
}

fn script(code: u16) -> Option<&'static str> {
    Some(match code {
        1 => "<vertAlign val=\"superscript\"/>",
        2 => "<vertAlign val=\"subscript\"/>",
        _ => return None,
    })
}

fn color_tag(tag: &str, c: &[u8], out: &mut String) {
    if let Some(a) = color_attrs(c) {
        out.push_str(&format!("<{tag}{a}/>"));
    }
}

/// Conteúdo de um `<font>` (ou `<rPr>`, com `name` = `rFont`).
fn font(b: &[u8], name: &str, out: &mut String) -> Option<()> {
    let mut c = Cur::new(b);
    let (height, bits, weight, sss) = (c.u16()?, c.u16()?, c.u16()?, c.u16()?);
    let (uls, family, charset, _) = (c.u8()?, c.u8()?, c.u8()?, c.u8()?);
    let color = c.take(8)?;
    let scheme = c.u8()?;
    let face = c.string()?;
    for (on, tag) in [
        (weight >= 600, "<b/>"),
        (bits & 0x02 != 0, "<i/>"),
        (bits & 0x08 != 0, "<strike/>"),
        (bits & 0x40 != 0, "<condense/>"),
        (bits & 0x80 != 0, "<extend/>"),
        (bits & 0x10 != 0, "<outline/>"),
        (bits & 0x20 != 0, "<shadow/>"),
    ] {
        if on {
            out.push_str(tag);
        }
    }
    out.push_str(underline(uls as u16).unwrap_or_default());
    out.push_str(script(sss).unwrap_or_default());
    out.push_str(&format!("<sz val=\"{}\"/>", height as f64 / 20.0));
    color_tag("color", color, out);
    out.push_str(&format!("<{name}{}/>", attr("val", &face)));
    if family != 0 {
        out.push_str(&format!("<family val=\"{family}\"/>"));
    }
    if charset != 0 {
        out.push_str(&format!("<charset val=\"{charset}\"/>"));
    }
    match scheme {
        1 => out.push_str("<scheme val=\"major\"/>"),
        2 => out.push_str("<scheme val=\"minor\"/>"),
        _ => {}
    }
    Some(())
}

fn fill(b: &[u8], out: &mut String) -> Option<()> {
    const GRADIENT: usize = 0x28;
    let mut c = Cur::new(b);
    let pattern = c.u32()? as usize;
    let (fg, bg) = (c.take(8)?, c.take(8)?);
    out.push_str("<fill>");
    if pattern == GRADIENT {
        let kind = c.u32()?;
        let (degree, l, r, t, btm) = (c.f64()?, c.f64()?, c.f64()?, c.f64()?, c.f64()?);
        out.push_str(&match kind {
            1 => format!(
                "<gradientFill type=\"path\" left=\"{l}\" right=\"{r}\" top=\"{t}\" bottom=\"{btm}\">"
            ),
            _ => format!("<gradientFill degree=\"{degree}\">"),
        });
        for _ in 0..c.u32()? {
            let color = c.take(8)?;
            out.push_str(&format!("<stop position=\"{}\">", c.f64()?));
            color_tag("color", color, out);
            out.push_str("</stop>");
        }
        out.push_str("</gradientFill></fill>");
        return Some(());
    }
    let name = PATTERNS.get(pattern).copied().unwrap_or("none");
    out.push_str(&format!("<patternFill patternType=\"{name}\""));
    if name == "none" {
        out.push_str("/></fill>");
        return Some(());
    }
    out.push('>');
    color_tag("fgColor", fg, out);
    color_tag("bgColor", bg, out);
    out.push_str("</patternFill></fill>");
    Some(())
}

fn side(tag: &str, style: u16, color: &[u8], out: &mut String) {
    match BORDERS.get(style as usize).filter(|_| style != 0) {
        Some(name) => {
            out.push_str(&format!("<{tag} style=\"{name}\">"));
            color_tag("color", color, out);
            out.push_str(&format!("</{tag}>"));
        }
        None => out.push_str(&format!("<{tag}/>")),
    }
}

fn border(b: &[u8], out: &mut String) -> Option<()> {
    let mut c = Cur::new(b);
    let diag = c.u8()?;
    let mut line = || Some((c.u16()? & 0xFF, c.take(8)?));
    let (top, bottom, left, right, diagonal) = (line()?, line()?, line()?, line()?, line()?);
    out.push_str("<border");
    out.push_str(&flag("diagonalUp", diag & 2 != 0));
    out.push_str(&flag("diagonalDown", diag & 1 != 0));
    out.push('>');
    for (tag, s) in
        [("left", left), ("right", right), ("top", top), ("bottom", bottom), ("diagonal", diagonal)]
    {
        side(tag, s.0, s.1, out);
    }
    out.push_str("</border>");
    Some(())
}

fn xf(b: &[u8], cell: bool, out: &mut String) -> Option<()> {
    let mut c = Cur::new(b);
    let (parent, fmt, font, fill, border) = (c.u16()?, c.u16()?, c.u16()?, c.u16()?, c.u16()?);
    let (rotation, indent) = (c.u8()?, c.u8()?);
    let (bits, apply) = (c.u16()?, c.u16()?);
    out.push_str(&format!(
        "<xf numFmtId=\"{fmt}\" fontId=\"{font}\" fillId=\"{fill}\" borderId=\"{border}\""
    ));
    if cell {
        out.push_str(&format!(" xfId=\"{parent}\""));
    }
    out.push_str(&flag("quotePrefix", bits & 0x8000 != 0));
    out.push_str(&flag("pivotButton", bits & 0x4000 != 0));
    // Nas células, o bit ligado diz que o atributo é próprio; nos estilos, é
    // o contrário (ligado = ignorado).
    const APPLY: [&str; 6] =
        ["applyNumberFormat", "applyFont", "applyAlignment", "applyBorder", "applyFill", "applyProtection"];
    for (k, name) in APPLY.iter().enumerate() {
        let on = apply >> k & 1 != 0;
        if cell {
            out.push_str(&flag(name, on));
        } else if on {
            out.push_str(&format!(" {name}=\"0\""));
        }
    }
    let mut inner = String::new();
    let (h, v) = ((bits & 7) as usize, (bits >> 3 & 7) as usize);
    let (wrap, justify, shrink) = (bits & 0x40 != 0, bits & 0x80 != 0, bits & 0x100 != 0);
    let order = bits >> 10 & 3;
    if h != 0 || v != 2 || wrap || justify || shrink || rotation != 0 || indent != 0 || order != 0 {
        inner.push_str("<alignment");
        if h != 0 {
            inner.push_str(&attr("horizontal", HALIGN[h]));
        }
        if v != 2 {
            inner.push_str(&attr("vertical", VALIGN.get(v).copied().unwrap_or("bottom")));
        }
        for (name, n) in
            [("textRotation", rotation as u16), ("indent", indent as u16), ("readingOrder", order)]
        {
            if n != 0 {
                inner.push_str(&format!(" {name}=\"{n}\""));
            }
        }
        inner.push_str(&flag("wrapText", wrap));
        inner.push_str(&flag("justifyLastLine", justify));
        inner.push_str(&flag("shrinkToFit", shrink));
        inner.push_str("/>");
    }
    // Travada (o padrão) não precisa dizer.
    let (locked, hidden) = (bits & 0x1000 != 0, bits & 0x2000 != 0);
    if !locked || hidden {
        inner.push_str("<protection");
        if !locked {
            inner.push_str(" locked=\"0\"");
        }
        inner.push_str(&flag("hidden", hidden));
        inner.push_str("/>");
    }
    if inner.is_empty() {
        out.push_str("/>");
    } else {
        out.push('>');
        out.push_str(&inner);
        out.push_str("</xf>");
    }
    Some(())
}

/// `BrtDXF`: lista de propriedades (`XFProp`: tipo, tamanho, dados).
fn dxf(b: &[u8], out: &mut String) -> Option<()> {
    let mut c = Cur::new(b);
    c.skip(4)?;
    let n = c.u16()?;
    // Na ordem do esquema: negrito, itálico, riscado, sublinhado, sobrescrito, cor, tamanho.
    let mut font = [const { String::new() }; 7];
    let mut num_fmt = String::new();
    let (mut pattern, mut fg, mut bg) = (None, String::new(), String::new());
    let mut sides = [const { String::new() }; 4];
    let mut fmt_id = None;
    for _ in 0..n {
        let (kind, len) = (c.u16()?, c.u16()?);
        let data = c.take((len as usize).checked_sub(4)?)?;
        let mut d = Cur::new(data);
        match kind {
            0 => pattern = PATTERNS.get(d.u8()? as usize),
            1 => color_tag("fgColor", data, &mut fg),
            2 => color_tag("bgColor", data, &mut bg),
            5 => color_tag("color", data, &mut font[5]),
            // Cima, baixo, esquerda, direita → na ordem do XML.
            6..=9 => {
                let k = [2, 3, 0, 1][kind as usize - 6];
                let color = d.take(8)?;
                let tag = ["left", "right", "top", "bottom"][k];
                sides[k].clear();
                side(tag, d.u16()?, color, &mut sides[k]);
            }
            0x19 => font[0] = if d.u16()? >= 600 { "<b/>" } else { "<b val=\"0\"/>" }.into(),
            0x1A => font[3] = underline(d.u16()?).unwrap_or("<u val=\"none\"/>").into(),
            0x1B => font[4] = script(d.u16()?).unwrap_or_default().into(),
            0x1C => font[1] = if d.u8()? != 0 { "<i/>" } else { "<i val=\"0\"/>" }.into(),
            0x1D => font[2] = if d.u8()? != 0 { "<strike/>" } else { "<strike val=\"0\"/>" }.into(),
            0x24 => font[6] = format!("<sz val=\"{}\"/>", d.u32()? as f64 / 20.0),
            0x26 => {
                let n = d.u16()? as usize;
                num_fmt = super::rec::utf16(d.take(n * 2)?);
            }
            0x29 => fmt_id = Some(d.u16()?),
            _ => {}
        }
    }
    out.push_str("<dxf>");
    let font = font.concat();
    if !font.is_empty() {
        out.push_str(&format!("<font>{font}</font>"));
    }
    if let Some(n) = fmt_id {
        out.push_str(&format!("<numFmt numFmtId=\"{n}\"{}/>", attr("formatCode", &num_fmt)));
    }
    if pattern.is_some() || !fg.is_empty() || !bg.is_empty() {
        out.push_str("<fill><patternFill");
        if let Some(p) = pattern {
            out.push_str(&attr("patternType", p));
        }
        out.push_str(&format!(">{fg}{bg}</patternFill></fill>"));
    }
    let sides = sides.concat();
    if !sides.is_empty() {
        out.push_str(&format!("<border>{sides}</border>"));
    }
    out.push_str("</dxf>");
    Some(())
}

/// Uma lista de itens (`<fonts count="3">…`).
#[derive(Default)]
struct List {
    xml: String,
    count: usize,
}

impl List {
    /// Um item; se o registro estiver truncado, o item vazio guarda o lugar
    /// (os índices das células não podem andar).
    fn add(&mut self, empty: &str, f: impl FnOnce(&mut String) -> Option<()>) {
        let at = self.xml.len();
        if f(&mut self.xml).is_none() {
            self.xml.truncate(at);
            self.xml.push_str(empty);
        }
        self.count += 1;
    }

    fn write(&self, tag: &str, out: &mut String) {
        if self.count > 0 {
            out.push_str(&format!("<{tag} count=\"{}\">{}</{tag}>", self.count, self.xml));
        }
    }
}

/// O `styles.xml` equivalente.
pub fn to_xml(bin: &[u8]) -> Vec<u8> {
    let (mut fmts, mut fonts, mut fills, mut borders) =
        (List::default(), List::default(), List::default(), List::default());
    let (mut style_xfs, mut xfs, mut styles, mut dxfs) =
        (List::default(), List::default(), List::default(), List::default());
    let mut palette = String::new();
    // Seções em que `BrtXF` e `BrtDXF` contam (há outros dentro de extensões).
    let (mut in_cells, mut in_styles, mut in_dxfs) = (false, false, false);
    for (kind, b) in Records::new(bin) {
        match kind {
            id::BEGIN_CELL_XFS | id::END_CELL_XFS => in_cells = kind == id::BEGIN_CELL_XFS,
            id::BEGIN_STYLE_XFS | id::END_STYLE_XFS => in_styles = kind == id::BEGIN_STYLE_XFS,
            id::BEGIN_DXFS | id::END_DXFS => in_dxfs = kind == id::BEGIN_DXFS,
            id::FMT => fmts.add("", |o| {
                let mut c = Cur::new(b);
                let (n, code) = (c.u16()?, c.string()?);
                o.push_str(&format!("<numFmt numFmtId=\"{n}\"{}/>", attr("formatCode", &code)));
                Some(())
            }),
            id::FONT => fonts.add("<font/>", |o| {
                o.push_str("<font>");
                font(b, "name", o)?;
                o.push_str("</font>");
                Some(())
            }),
            id::FILL => fills.add("<fill/>", |o| fill(b, o)),
            id::BORDER => borders.add("<border/>", |o| border(b, o)),
            id::XF if in_cells => xfs.add("<xf/>", |o| xf(b, true, o)),
            id::XF if in_styles => style_xfs.add("<xf/>", |o| xf(b, false, o)),
            id::STYLE => styles.add("", |o| cell_style(b, o)),
            id::DXF if in_dxfs => dxfs.add("<dxf/>", |o| dxf(b, o)),
            id::INDEXED_COLOR => {
                if let Some([r, g, bl, _]) = b.first_chunk::<4>() {
                    palette.push_str(&format!("<rgbColor rgb=\"00{r:02X}{g:02X}{bl:02X}\"/>"));
                }
            }
            _ => {}
        }
    }
    let mut out =
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<styleSheet xmlns=\"{NS}\">");
    for (tag, list) in [
        ("numFmts", &fmts),
        ("fonts", &fonts),
        ("fills", &fills),
        ("borders", &borders),
        ("cellStyleXfs", &style_xfs),
        ("cellXfs", &xfs),
        ("cellStyles", &styles),
        ("dxfs", &dxfs),
    ] {
        list.write(tag, &mut out);
    }
    if !palette.is_empty() {
        out.push_str(&format!("<colors><indexedColors>{palette}</indexedColors></colors>"));
    }
    out.push_str("</styleSheet>");
    out.into_bytes()
}

fn cell_style(b: &[u8], out: &mut String) -> Option<()> {
    let mut c = Cur::new(b);
    let (xf, bits, builtin, level, name) = (c.u32()?, c.u16()?, c.u8()?, c.u8()?, c.string()?);
    out.push_str(&format!("<cellStyle{} xfId=\"{xf}\"", attr("name", &name)));
    if bits & 1 != 0 {
        out.push_str(&format!(" builtinId=\"{builtin}\""));
        // Níveis de linha e coluna (estilos 1 e 2).
        if matches!(builtin, 1 | 2) {
            out.push_str(&format!(" iLevel=\"{level}\""));
        }
    }
    out.push_str(&flag("hidden", bits & 2 != 0));
    out.push_str(&flag("customBuiltin", bits & 4 != 0));
    out.push_str("/>");
    Some(())
}

/// O `<rPr>` de cada fonte do arquivo, para os trechos de texto rico.
pub fn run_fonts(bin: &[u8]) -> Vec<String> {
    Records::new(bin)
        .filter(|r| r.0 == id::FONT)
        .map(|(_, b)| {
            let mut s = String::from("<rPr>");
            let _ = font(b, "rFont", &mut s);
            s.push_str("</rPr>");
            s
        })
        .collect()
}
