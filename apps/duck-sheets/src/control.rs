//! Objetos que respondem ao clique: controles de formulário (botões de
//! opção, caixas de seleção, botões), links e macros de formas e imagens.

use duck_core::drawing::{Control, ControlKind, ObjectKind};
use duck_core::shape::TextAnchor;
use duck_core::{CellChange, CellValue, Command, Hyperlink, Layout};
use duck_ui::{Painter, Rect, Rgba, rgb};

use crate::app::{Geom, SheetsApp};
use crate::chart::{ObjCtx, object_rect};

const DARK: Rgba = rgb(0x333333);
const WHITE: Rgba = rgb(0xFFFFFF);

pub fn draw(p: &mut Painter, cx: &ObjCtx, c: &Control, r: Rect, clip: &Rect) {
    let g = cx.g;
    let mut text = c.text.clone();
    text.anchor = TextAnchor::Middle;
    let cy = ((r.y0 + r.y1) / 2.0).round();
    match c.kind {
        ControlKind::Radio | ControlKind::Check => {
            let d = 12.0 * g;
            let x0 = (r.x0 + 2.0 * g).round();
            if c.kind == ControlKind::Radio {
                let circle = |cx: f32, rad: f32| -> Vec<[f32; 2]> {
                    (0..24)
                        .map(|i| {
                            let a = i as f32 * std::f32::consts::TAU / 24.0;
                            [cx + rad * a.cos(), cy + rad * a.sin()]
                        })
                        .collect()
                };
                let cxx = x0 + d / 2.0;
                let ring = circle(cxx, d / 2.0);
                p.shape(&ring, clip, WHITE);
                p.outline(&ring, clip, g.max(1.0), DARK);
                if c.checked {
                    p.shape(&circle(cxx, d / 4.0), clip, DARK);
                }
            } else {
                let b = Rect::new(x0, cy - d / 2.0, x0 + d, cy + d / 2.0);
                p.fill(b, clip, WHITE);
                p.stroke(b, clip, g.max(1.0), DARK);
                if c.checked {
                    let at = |x: f32, y: f32| [b.x0 + x * d, b.y0 + y * d];
                    let w = (1.6 * g).max(1.0);
                    p.line(at(0.2, 0.52), at(0.42, 0.74), w, clip, DARK);
                    p.line(at(0.42, 0.74), at(0.8, 0.26), w, clip, DARK);
                }
            }
            text.insets = [0.0; 4];
            let tr = Rect::new(x0 + d + 4.0 * g, r.y0, r.x1, r.y1);
            crate::shape::draw_text(p, cx, &text, tr, &tr.intersect(clip));
        }
        ControlKind::Button => {
            p.fill(r, clip, rgb(0xE1E1E1));
            p.stroke(r, clip, g.max(1.0), rgb(0xADADAD));
            for para in &mut text.paras {
                para.align = duck_core::shape::TextAlign::Center;
            }
            crate::shape::draw_text(p, cx, &text, r, clip);
        }
        ControlKind::Label => crate::shape::draw_text(p, cx, &text, r, clip),
        ControlKind::GroupBox => {
            p.stroke(r, clip, g.max(1.0), rgb(0xBFBFBF));
            text.anchor = TextAnchor::Top;
            crate::shape::draw_text(p, cx, &text, r, clip);
        }
        ControlKind::Other => p.stroke(r, clip, g.max(1.0), rgb(0xBFBFBF)),
    }
}

/// Marca um botão de opção ou inverte uma caixa de seleção no layout.
/// Devolve a célula ligada e o valor que ela recebe.
pub fn press(l: &mut Layout, obj: usize) -> Option<(Option<String>, CellValue)> {
    let ObjectKind::Control(c) = &l.objects.get(obj)?.kind else { return None };
    let (kind, group, index, link) = (c.kind, c.group, c.index, c.link.clone());
    match kind {
        ControlKind::Radio => {
            // A ligação fica no primeiro do grupo.
            let mut cell = None;
            for o in &mut l.objects {
                if let ObjectKind::Control(c) = &mut o.kind
                    && c.kind == ControlKind::Radio
                    && c.group == group
                {
                    c.checked = c.index == index;
                    cell = cell.or_else(|| c.link.clone());
                }
            }
            Some((cell, CellValue::Number(index as f64)))
        }
        ControlKind::Check => {
            let ObjectKind::Control(c) = &mut l.objects[obj].kind else { return None };
            c.checked = !c.checked;
            Some((link, CellValue::Bool(c.checked)))
        }
        _ => None,
    }
}

/// `Aba!$B$2` ou `$B$2` → (aba, linha, coluna).
fn cell_ref(s: &str, names: &[String], host: usize) -> Option<(usize, u32, u32)> {
    let (sheet, cell) = match s.rfind('!') {
        Some(i) => {
            let n = &s[..i];
            let n = n
                .strip_prefix('\'')
                .and_then(|n| n.strip_suffix('\''))
                .map_or(n.to_string(), |n| n.replace("''", "'"));
            (names.iter().position(|x| x.eq_ignore_ascii_case(&n))?, &s[i + 1..])
        }
        None => (host, s),
    };
    let (a, _) = crate::app::parse_range(&cell.replace('$', ""))?;
    Some((sheet, a.0, a.1))
}

/// `%20` e afins de um endereço.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%')
            .then(|| b.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match hex {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

impl SheetsApp {
    /// Objeto clicável sob o mouse e o que ele faz (para a barra de status;
    /// começa com `\u{1}`).
    pub fn object_action(&self, g: &Geom, x: f32, y: f32) -> Option<(usize, String)> {
        let (sheet, v) = self.doc.as_ref()?.current()?;
        let q = g.quads.iter().find(|q| !q.rect.is_empty() && q.rect.contains(x, y))?;
        // De cima para baixo; segmentações por cima ficam com o clique.
        for (i, o) in sheet.layout.objects.iter().enumerate().rev() {
            if o.hidden {
                continue;
            }
            let [ox, oy, w, h] = object_rect(o, v, None);
            let r = Rect::new(
                q.rect.x0 + ((ox - q.ox) * g.g as f64) as f32,
                q.rect.y0 + ((oy - q.oy) * g.g as f64) as f32,
                q.rect.x0 + ((ox + w - q.ox) * g.g as f64) as f32,
                q.rect.y0 + ((oy + h - q.oy) * g.g as f64) as f32,
            );
            if !r.contains(x, y) {
                continue;
            }
            match &o.kind {
                ObjectKind::Slicer(_) | ObjectKind::Timeline(_) => return None,
                ObjectKind::Control(c)
                    if matches!(c.kind, ControlKind::Radio | ControlKind::Check | ControlKind::Button) =>
                {
                    return Some((i, "\u{1}Clique para escolher".into()));
                }
                _ => {}
            }
            if let Some(l) = &o.link {
                return Some((i, format!("\u{1}Clique para ir: {}", percent_decode(l))));
            }
            if o.macro_name.is_some() {
                return Some((i, "\u{1}Clique para executar a macro".into()));
            }
        }
        None
    }

    /// Clique num objeto. `true` = o clique era dele.
    pub fn object_click(&mut self, g: &Geom, x: f32, y: f32) -> bool {
        let Some((i, _)) = self.object_action(g, x, y) else { return false };
        self.commit_edit();
        self.object_run(i);
        true
    }

    /// Faz o que o objeto `i` da aba ativa faz ao ser clicado.
    pub fn object_run(&mut self, i: usize) {
        let Some(doc) = &self.doc else { return };
        let active = doc.active;
        let Some((sheet, _)) = doc.current() else { return };
        let Some(o) = sheet.layout.objects.get(i) else { return };
        let (link, mac) = (o.link.clone(), o.macro_name.clone());
        if matches!(o.kind, ObjectKind::Control(_)) {
            let mut layout = sheet.layout.clone();
            if let Some((cell, value)) = press(&mut layout, i) {
                let names: Vec<String> = doc.sheets.iter().map(|s| s.info.name.clone()).collect();
                let mut step = vec![(active, Command::Layout(Box::new(layout)))];
                if let Some((s, row, col)) = cell.and_then(|c| cell_ref(&c, &names, active)) {
                    step.push((
                        s,
                        Command::SetCells(vec![CellChange { row, col, value, formula: None, style: None }]),
                    ));
                }
                self.run_step(step);
            }
        }
        if let Some(l) = link {
            let l = percent_decode(&l);
            let h = match l.strip_prefix('#') {
                Some(loc) => Hyperlink {
                    range: Default::default(),
                    target: None,
                    location: Some(loc.to_string()),
                    tooltip: None,
                },
                None => {
                    Hyperlink { range: Default::default(), target: Some(l), location: None, tooltip: None }
                }
            };
            self.follow_link(&h);
            return;
        }
        if let Some(m) = mac {
            self.run_macro(&m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duck_core::drawing::{Anchor, Marker, Object};

    fn radio(group: u32, index: u32, link: Option<&str>) -> Object {
        let m = Marker::default();
        Object::new(
            Anchor::Two { from: m, to: m },
            ObjectKind::Control(Box::new(Control {
                kind: ControlKind::Radio,
                checked: index == 1,
                link: link.map(String::from),
                group,
                index,
                ..Default::default()
            })),
        )
    }

    #[test]
    fn radio_groups() {
        let mut l = Layout {
            objects: vec![
                radio(1, 1, Some("$B$2")),
                radio(1, 2, None),
                radio(2, 1, Some("Outra!$A$1")),
                radio(1, 3, None),
            ],
            ..Default::default()
        };
        assert_eq!(press(&mut l, 3), Some((Some("$B$2".into()), CellValue::Number(3.0))));
        let on: Vec<bool> =
            l.objects.iter().map(|o| matches!(&o.kind, ObjectKind::Control(c) if c.checked)).collect();
        assert_eq!(on, [false, false, true, true]);
    }

    #[test]
    fn refs_and_urls() {
        let names = vec!["Dados".to_string(), "Minha Aba".to_string()];
        assert_eq!(cell_ref("$B$2", &names, 0), Some((0, 1, 1)));
        assert_eq!(cell_ref("'Minha Aba'!$C$4", &names, 0), Some((1, 3, 2)));
        assert_eq!(cell_ref("Outra!A1", &names, 0), None);
        assert_eq!(percent_decode("#'Minha%20Aba'!A1%"), "#'Minha Aba'!A1%");
    }
}
