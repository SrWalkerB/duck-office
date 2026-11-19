//! Partes pequenas do pacote: relacionamentos, workbook, strings e estilos.

use duck_core::{
    Alignment, Border, BorderSide, BorderStyle, CellXf, Color, Fill, Font, HAlign, StringTable, Styles,
    Theme, VAlign,
};
use rayon::prelude::*;

use crate::Error;
use crate::package::Package;
use crate::xml::{attr, attr_str, find_tag, for_each, inner, parse_u32, rich_text_into};

// ---------------------------------------------------------------------------
// Relacionamentos e workbook
// ---------------------------------------------------------------------------

/// Uma aba listada no `workbook.xml`.
#[derive(Clone, Debug)]
pub struct SheetMeta {
    pub name: String,
    pub hidden: bool,
    /// Caminho da parte no zip (ex.: `xl/worksheets/sheet1.xml`).
    pub path: String,
}

pub struct WorkbookMeta {
    pub sheets: Vec<SheetMeta>,
    /// Aba aberta quando o arquivo foi salvo (índice em `sheets`).
    pub active: usize,
    pub date1904: bool,
    pub shared_strings: Option<String>,
    pub styles: Option<String>,
    pub theme: Option<String>,
}

struct Rel {
    id: String,
    kind: String,
    target: String,
}

/// Lê `dir/_rels/file.rels` e resolve os alvos relativos a `dir`.
fn read_rels(pkg: &Package, part: &str) -> Result<Vec<Rel>, Error> {
    let (dir, file) = match part.rfind('/') {
        Some(i) => (&part[..i], &part[i + 1..]),
        None => ("", part),
    };
    let rels_path =
        if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
    let Ok(buf) = pkg.read(&rels_path) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for_each(&buf, b"Relationship", |a, _| {
        let (Some(id), Some(kind), Some(target)) =
            (attr_str(a, b"Id"), attr_str(a, b"Type"), attr_str(a, b"Target"))
        else {
            return;
        };
        if attr(a, b"TargetMode") == Some(b"External") {
            return;
        }
        out.push(Rel { id, kind, target: resolve(dir, &target) });
    });
    Ok(out)
}

/// Junta `dir` + `target`, tratando caminhos absolutos e `..`.
fn resolve(dir: &str, target: &str) -> String {
    let joined = if let Some(abs) = target.strip_prefix('/') {
        abs.to_string()
    } else if dir.is_empty() {
        target.to_string()
    } else {
        format!("{dir}/{target}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for p in joined.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

pub fn read_workbook(pkg: &Package) -> Result<WorkbookMeta, Error> {
    let root = read_rels(pkg, "")?;
    let wb_path = root
        .iter()
        .find(|r| r.kind.ends_with("/officeDocument"))
        .map(|r| r.target.clone())
        .unwrap_or_else(|| "xl/workbook.xml".into());
    let buf = pkg.read(&wb_path)?;
    let rels = read_rels(pkg, &wb_path)?;
    let by_id = |id: &str| rels.iter().find(|r| r.id == id);

    let date1904 = find_tag(&buf, 0, b"workbookPr")
        .and_then(|t| attr(t.attrs, b"date1904"))
        .is_some_and(|v| v == b"1" || v == b"true");

    // `activeTab` conta todas as abas, inclusive as de gráfico.
    let active_tab = find_tag(&buf, 0, b"workbookView")
        .and_then(|t| attr(t.attrs, b"activeTab"))
        .and_then(parse_u32)
        .unwrap_or(0) as usize;
    let mut sheets = Vec::new();
    let mut active = None;
    let mut tab = 0usize;
    for_each(&buf, b"sheet", |a, _| {
        let name = attr_str(a, b"name").unwrap_or_default();
        let hidden = matches!(attr(a, b"state"), Some(b"hidden") | Some(b"veryHidden"));
        let this_tab = tab;
        tab += 1;
        let Some(rid) = attr_str(a, b"r:id") else { return };
        // Só planilhas comuns (abas de gráfico e diálogo não têm células).
        if let Some(rel) = by_id(&rid)
            && rel.kind.ends_with("/worksheet")
        {
            if this_tab == active_tab && !hidden {
                active = Some(sheets.len());
            }
            sheets.push(SheetMeta { name, hidden, path: rel.target.clone() });
        }
    });
    // Aba ativa escondida ou de gráfico: a primeira visível.
    let active = active.or_else(|| sheets.iter().position(|s| !s.hidden)).unwrap_or(0);
    let find = |suffix: &str| rels.iter().find(|r| r.kind.ends_with(suffix)).map(|r| r.target.clone());
    Ok(WorkbookMeta {
        sheets,
        active,
        date1904,
        shared_strings: find("/sharedStrings"),
        styles: find("/styles"),
        theme: find("/theme"),
    })
}

// ---------------------------------------------------------------------------
// Strings compartilhadas
// ---------------------------------------------------------------------------

/// Lê `sharedStrings.xml`. O XML é cortado em fronteiras de `</si>` e cada
/// pedaço vira uma tabela numa thread; as tabelas são juntadas em ordem.
pub fn read_shared_strings(pkg: &Package, path: &str) -> Result<StringTable, Error> {
    let buf = pkg.read(path)?;
    const PIECE: usize = 4 * 1024 * 1024;
    let mut cuts = vec![0usize];
    while buf.len() - cuts[cuts.len() - 1] > PIECE {
        let from = cuts[cuts.len() - 1] + PIECE;
        match memchr::memmem::find(&buf[from..], b"</si>") {
            Some(i) => cuts.push(from + i + 5),
            None => break,
        }
    }
    cuts.push(buf.len());
    let tables: Vec<StringTable> = cuts
        .par_windows(2)
        .map(|w| {
            let piece = &buf[w[0]..w[1]];
            let mut t = StringTable::with_capacity(piece.len() / 32, piece.len() / 2);
            let mut s = String::new();
            for_each(piece, b"si", |_, content| {
                s.clear();
                rich_text_into(content, &mut s);
                t.push(&s);
            });
            t
        })
        .collect();
    let mut out = StringTable::new();
    for t in &tables {
        out.append(t);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Estilos
// ---------------------------------------------------------------------------

fn section<'a>(buf: &'a [u8], name: &[u8]) -> &'a [u8] {
    find_tag(buf, 0, name).map(|t| inner(buf, &t, name)).unwrap_or(&[])
}

pub(crate) fn color(attrs: &[u8]) -> Color {
    if let Some(rgb) = attr(attrs, b"rgb").and_then(|v| std::str::from_utf8(v).ok())
        && let Ok(v) = u32::from_str_radix(rgb, 16)
    {
        return Color::Rgb(if rgb.len() <= 6 { 0xff00_0000 | v } else { v });
    }
    if let Some(theme) = attr(attrs, b"theme").and_then(parse_u32) {
        let tint = attr(attrs, b"tint")
            .and_then(|v| std::str::from_utf8(v).ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        return Color::Theme { index: theme as u8, tint };
    }
    if let Some(i) = attr(attrs, b"indexed").and_then(parse_u32) {
        return Color::Indexed(i as u8);
    }
    Color::Auto
}

fn flag(content: &[u8], name: &[u8]) -> bool {
    match find_tag(content, 0, name) {
        Some(t) => !matches!(attr(t.attrs, b"val"), Some(b"0") | Some(b"false") | Some(b"none")),
        None => false,
    }
}

pub fn read_styles(pkg: &Package, path: &str) -> Result<Styles, Error> {
    let buf = pkg.read(path)?;
    let mut st = Styles::default();

    for_each(section(&buf, b"numFmts"), b"numFmt", |a, _| {
        if let (Some(id), Some(code)) = (attr(a, b"numFmtId").and_then(parse_u32), attr_str(a, b"formatCode"))
        {
            st.num_fmts.insert(id as u16, code);
        }
    });

    for_each(section(&buf, b"fonts"), b"font", |_, c| {
        let mut f = Font {
            bold: flag(c, b"b"),
            italic: flag(c, b"i"),
            underline: flag(c, b"u"),
            ..Default::default()
        };
        if let Some(t) = find_tag(c, 0, b"sz") {
            f.size = attr(t.attrs, b"val")
                .and_then(|v| std::str::from_utf8(v).ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(11.0);
        }
        if let Some(t) = find_tag(c, 0, b"name") {
            f.name = attr_str(t.attrs, b"val").unwrap_or_default();
        }
        if let Some(t) = find_tag(c, 0, b"color") {
            f.color = color(t.attrs);
        }
        st.fonts.push(f);
    });

    for_each(section(&buf, b"fills"), b"fill", |_, c| {
        let mut fill = Fill::default();
        if let Some(p) = find_tag(c, 0, b"patternFill") {
            fill.pattern = attr_str(p.attrs, b"patternType").filter(|t| t != "none");
            let pc = inner(c, &p, b"patternFill");
            if let Some(t) = find_tag(pc, 0, b"fgColor") {
                fill.fg = color(t.attrs);
            }
            if let Some(t) = find_tag(pc, 0, b"bgColor") {
                fill.bg = color(t.attrs);
            }
        }
        st.fills.push(fill);
    });

    for_each(section(&buf, b"borders"), b"border", |_, c| {
        let side = |names: &[&[u8]]| {
            for name in names {
                if let Some(t) = find_tag(c, 0, name) {
                    let style = attr(t.attrs, b"style").map(BorderStyle::parse).unwrap_or_default();
                    let color =
                        find_tag(inner(c, &t, name), 0, b"color").map(|t| color(t.attrs)).unwrap_or_default();
                    return BorderSide { style, color };
                }
            }
            BorderSide::default()
        };
        st.borders.push(Border {
            left: side(&[b"left", b"start"]),
            right: side(&[b"right", b"end"]),
            top: side(&[b"top"]),
            bottom: side(&[b"bottom"]),
        });
    });

    // Paleta indexada própria (arquivos antigos).
    let colors = section(&buf, b"colors");
    if !colors.is_empty() {
        let mut pal = Vec::new();
        for_each(section(colors, b"indexedColors"), b"rgbColor", |a, _| {
            if let Some(v) = attr(a, b"rgb").and_then(|v| std::str::from_utf8(v).ok())
                && let Ok(v) = u32::from_str_radix(v, 16)
            {
                pal.push(v | 0xFF00_0000);
            }
        });
        if !pal.is_empty() {
            st.indexed = Some(pal);
        }
    }

    for_each(section(&buf, b"cellXfs"), b"xf", |a, c| {
        let num = |k: &[u8]| attr(a, k).and_then(parse_u32).unwrap_or(0) as u16;
        let mut xf = CellXf {
            num_fmt: num(b"numFmtId"),
            font: num(b"fontId"),
            fill: num(b"fillId"),
            border: num(b"borderId"),
            alignment: Alignment::default(),
        };
        if let Some(t) = find_tag(c, 0, b"alignment") {
            let al = &mut xf.alignment;
            al.horizontal = match attr(t.attrs, b"horizontal") {
                Some(b"left") => HAlign::Left,
                Some(b"center") => HAlign::Center,
                Some(b"right") => HAlign::Right,
                Some(b"fill") => HAlign::Fill,
                Some(b"justify") => HAlign::Justify,
                Some(b"centerContinuous") => HAlign::CenterContinuous,
                Some(b"distributed") => HAlign::Distributed,
                _ => HAlign::General,
            };
            al.vertical = match attr(t.attrs, b"vertical") {
                Some(b"top") => VAlign::Top,
                Some(b"center") => VAlign::Center,
                Some(b"justify") => VAlign::Justify,
                Some(b"distributed") => VAlign::Distributed,
                _ => VAlign::Bottom,
            };
            al.wrap = matches!(attr(t.attrs, b"wrapText"), Some(b"1") | Some(b"true"));
            al.indent = attr(t.attrs, b"indent").and_then(parse_u32).unwrap_or(0).min(255) as u8;
        }
        st.xfs.push(xf);
    });
    Ok(st)
}

// ---------------------------------------------------------------------------
// Tema
// ---------------------------------------------------------------------------

/// Lê as cores e as fontes de `theme1.xml`. O que faltar fica do tema padrão.
pub fn read_theme(pkg: &Package, path: &str) -> Result<Theme, Error> {
    let buf = pkg.read(path)?;
    Ok(parse_theme(&buf))
}

pub(crate) fn parse_theme(buf: &[u8]) -> Theme {
    let mut theme = Theme::default();
    let scheme = section(buf, b"a:clrScheme");
    const NAMES: [&[u8]; 12] = [
        b"a:dk1",
        b"a:lt1",
        b"a:dk2",
        b"a:lt2",
        b"a:accent1",
        b"a:accent2",
        b"a:accent3",
        b"a:accent4",
        b"a:accent5",
        b"a:accent6",
        b"a:hlink",
        b"a:folHlink",
    ];
    let hex = |v: Option<&[u8]>| {
        v.and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| u32::from_str_radix(v, 16).ok())
    };
    for (i, name) in NAMES.iter().enumerate() {
        let Some(t) = find_tag(scheme, 0, name) else { continue };
        let c = inner(scheme, &t, name);
        let v = if let Some(x) = find_tag(c, 0, b"a:srgbClr") {
            hex(attr(x.attrs, b"val"))
        } else if let Some(x) = find_tag(c, 0, b"a:sysClr") {
            hex(attr(x.attrs, b"lastClr")).or(match attr(x.attrs, b"val") {
                Some(b"window") => Some(0xFFFFFF),
                Some(b"windowText") => Some(0x000000),
                _ => None,
            })
        } else {
            None
        };
        if let Some(v) = v {
            theme.colors[i] = v & 0xFF_FFFF;
        }
    }
    let font = |name: &[u8]| {
        let s = section(buf, name);
        find_tag(s, 0, b"a:latin").and_then(|t| attr_str(t.attrs, b"typeface")).filter(|f| !f.is_empty())
    };
    if let Some(f) = font(b"a:minorFont") {
        theme.minor_font = f;
    }
    if let Some(f) = font(b"a:majorFont") {
        theme.major_font = f;
    }
    theme
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_paths() {
        assert_eq!(resolve("xl", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("xl", "/xl/worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("xl/worksheets", "../drawings/d1.xml"), "xl/drawings/d1.xml");
        assert_eq!(resolve("", "xl/workbook.xml"), "xl/workbook.xml");
    }

    #[test]
    fn theme_colors_and_fonts() {
        let xml = br#"<a:theme><a:themeElements><a:clrScheme name="X">
<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="0E2841"/></a:dk2><a:lt2><a:srgbClr val="E8E8E8"/></a:lt2>
<a:accent1><a:srgbClr val="156082"/></a:accent1></a:clrScheme>
<a:fontScheme name="Office"><a:majorFont><a:latin typeface="Aptos Display"/></a:majorFont>
<a:minorFont><a:latin typeface="Aptos Narrow"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;
        let t = parse_theme(xml);
        assert_eq!(t.colors[2], 0x0E2841);
        assert_eq!(t.colors[4], 0x156082);
        assert_eq!(t.colors[5], Theme::default().colors[5]);
        assert_eq!(t.minor_font, "Aptos Narrow");
        assert_eq!(t.major_font, "Aptos Display");
    }
}
