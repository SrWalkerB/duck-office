//! Conferência do pacote salvo: o que o Excel exige para abrir sem reparar.
//!
//! - toda relação interna aponta para uma parte que existe;
//! - toda parte tem tipo (`Override` ou `Default`), e todo `Override` existe;
//! - nomes e ids de aba únicos, `r:id` de cada aba válido, aba ativa visível;
//! - `localSheetId` dentro da lista, nomes definidos sem repetição;
//! - ids e nomes de tabela, segmentação e cache únicos;
//! - caches de segmentação só com abas e tabelas que existem.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use duck_xlsx::Package;
use duck_xlsx::xml::{attr_str, find_tag, for_each};

fn rels_of(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((d, f)) => format!("{d}/_rels/{f}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

fn resolve(dir: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(t) => t.to_string(),
        None => format!("{dir}/{target}"),
    };
    let mut out: Vec<&str> = Vec::new();
    for p in joined.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    out.join("/")
}

/// Relações internas de uma parte: (id, tipo, parte).
fn rels(pkg: &Package, part: &str) -> Vec<(String, String, String)> {
    let Ok(x) = pkg.read(&rels_of(part)) else { return Vec::new() };
    let dir = part.rsplit_once('/').map_or("", |p| p.0);
    let mut out = Vec::new();
    for_each(&x, b"Relationship", |a, _| {
        let external = attr_str(a, b"TargetMode").is_some_and(|m| m == "External");
        if let (Some(id), Some(k), Some(t)) =
            (attr_str(a, b"Id"), attr_str(a, b"Type"), attr_str(a, b"Target"))
            && !external
        {
            out.push((id, k, resolve(dir, &t)));
        }
    });
    out
}

/// Problemas achados no pacote (vazio = tudo certo).
pub fn check(path: &Path) -> Vec<String> {
    let mut bad = Vec::new();
    let pkg = match Package::open(path) {
        Ok(p) => p,
        Err(e) => return vec![e.to_string()],
    };
    let names: HashSet<String> = pkg.entries().map(|e| e.name.clone()).collect();

    // Relações.
    for n in names.iter().filter(|n| n.ends_with(".rels")) {
        let part = n.replacen("_rels/", "", 1);
        let part = part.strip_suffix(".rels").unwrap_or(&part).to_string();
        if !part.is_empty() && !names.contains(&part) {
            bad.push(format!("relações de uma parte que não existe: {n}"));
        }
        let mut ids = HashSet::new();
        for (id, _, t) in rels(&pkg, &part) {
            if !names.contains(&t) && !t.contains('#') {
                bad.push(format!("{n}: {id} aponta para {t}, que não existe"));
            }
            if !ids.insert(id.clone()) {
                bad.push(format!("{n}: id repetido {id}"));
            }
        }
    }

    // Tipos.
    let Ok(ct) = pkg.read("[Content_Types].xml") else {
        bad.push("sem [Content_Types].xml".into());
        return bad;
    };
    let mut defaults = HashSet::new();
    for_each(&ct, b"Default", |a, _| defaults.extend(attr_str(a, b"Extension").map(|e| e.to_lowercase())));
    let mut over = HashSet::new();
    let mut twice = Vec::new();
    for_each(&ct, b"Override", |a, _| {
        if let Some(p) = attr_str(a, b"PartName") {
            let p = p.trim_start_matches('/').to_string();
            if !over.insert(p.clone()) {
                twice.push(p);
            }
        }
    });
    bad.extend(twice.into_iter().map(|p| format!("tipo repetido: {p}")));
    for p in &over {
        if !names.contains(p) {
            bad.push(format!("tipo de uma parte que não existe: {p}"));
        }
    }
    for n in &names {
        let ext = n.rsplit_once('.').map_or("", |x| x.1).to_lowercase();
        if n != "[Content_Types].xml" && !n.ends_with('/') && !over.contains(n) && !defaults.contains(&ext) {
            bad.push(format!("parte sem tipo: {n}"));
        }
    }

    // Workbook.
    let wb_path = rels(&pkg, "")
        .into_iter()
        .find(|r| r.1.ends_with("/officeDocument"))
        .map_or("xl/workbook.xml".to_string(), |r| r.2);
    let Ok(wb) = pkg.read(&wb_path) else {
        bad.push("sem workbook".into());
        return bad;
    };
    let wb_rels = rels(&pkg, &wb_path);
    let by_id: HashMap<&str, &(String, String, String)> = wb_rels.iter().map(|r| (r.0.as_str(), r)).collect();
    let mut tabs: Vec<(String, String, bool)> = Vec::new(); // nome, parte, visível
    let mut ids: HashMap<u32, usize> = HashMap::new();
    for_each(&wb, b"sheet", |a, _| {
        let name = attr_str(a, b"name").unwrap_or_default();
        let rid = attr_str(a, b"r:id").unwrap_or_default();
        let id = attr_str(a, b"sheetId").and_then(|s| s.parse().ok()).unwrap_or(0);
        if tabs.iter().any(|t| t.0.to_lowercase() == name.to_lowercase()) {
            bad.push("nome de aba repetido".into());
        }
        if name.is_empty() || name.encode_utf16().count() > 31 {
            bad.push("nome de aba inválido".into());
        }
        if ids.insert(id, tabs.len()).is_some() {
            bad.push(format!("sheetId repetido: {id}"));
        }
        let part = by_id.get(rid.as_str()).map(|r| r.2.clone()).unwrap_or_default();
        if part.is_empty() {
            bad.push(format!("aba {}: r:id {rid} sem relação", tabs.len() + 1));
        }
        let vis = !attr_str(a, b"state").is_some_and(|s| s != "visible");
        tabs.push((name, part, vis));
    });
    if !tabs.iter().any(|t| t.2) {
        bad.push("nenhuma aba visível".into());
    }
    let active = find_tag(&wb, 0, b"workbookView")
        .and_then(|t| attr_str(t.attrs, b"activeTab"))
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    if !tabs.get(active).is_some_and(|t| t.2) {
        bad.push(format!("aba ativa {active} não existe ou está escondida"));
    }
    let mut selected = 0;
    for (k, t) in tabs.iter().enumerate() {
        let Some(e) = pkg.entry(&t.1) else { continue };
        let mut buf = Vec::new();
        let _ = pkg.inflater(e).read_into(&mut buf, 64 * 1024);
        if let Some(v) = find_tag(&buf, 0, b"sheetView")
            && attr_str(v.attrs, b"tabSelected").is_some_and(|s| s == "1")
        {
            selected += 1;
            if !t.2 {
                bad.push(format!("aba {} escondida e selecionada", k + 1));
            }
        }
    }
    if selected > 1 {
        bad.push(format!("{selected} abas selecionadas juntas"));
    }
    let mut dn = HashSet::new();
    for_each(&wb, b"definedName", |a, _| {
        let n = attr_str(a, b"name").unwrap_or_default().to_lowercase();
        let l = attr_str(a, b"localSheetId").and_then(|s| s.parse::<usize>().ok());
        if l.is_some_and(|l| l >= tabs.len()) {
            bad.push("nome definido de uma aba que não existe".into());
        }
        if !dn.insert((n, l)) {
            bad.push("nome definido repetido".into());
        }
    });
    if let Some(t) = find_tag(&wb, 0, b"definedNames")
        && find_tag(&wb, t.end, b"definedName").is_none()
    {
        bad.push("lista de nomes definidos vazia".into());
    }
    // Toda referência do workbook a uma relação existe.
    let text = String::from_utf8_lossy(&wb);
    for part in text.split("r:id=\"").skip(1) {
        let id = part.split('"').next().unwrap_or("");
        if !by_id.contains_key(id) {
            bad.push(format!("workbook cita a relação {id}, que não existe"));
        }
    }

    // Tabelas, segmentações e caches.
    let (mut table_ids, mut table_names) = (HashSet::new(), HashSet::new());
    let mut slicers = HashSet::new();
    let mut used_caches = HashSet::new();
    for n in names.iter().filter(|n| n.starts_with("xl/tables/") && n.ends_with(".xml")) {
        let Ok(x) = pkg.read(n) else { continue };
        let Some(t) = find_tag(&x, 0, b"table") else { continue };
        if !table_ids.insert(attr_str(t.attrs, b"id").unwrap_or_default()) {
            bad.push(format!("id de tabela repetido em {n}"));
        }
        if !table_names.insert(attr_str(t.attrs, b"displayName").unwrap_or_default().to_lowercase()) {
            bad.push(format!("nome de tabela repetido em {n}"));
        }
    }
    for n in names
        .iter()
        .filter(|n| n.ends_with(".xml") && (n.starts_with("xl/slicers/") || n.starts_with("xl/timelines/")))
    {
        let Ok(x) = pkg.read(n) else { continue };
        for tag in [&b"slicer"[..], b"timeline"] {
            for_each(&x, tag, |a, _| {
                if !slicers.insert(attr_str(a, b"name").unwrap_or_default().to_lowercase()) {
                    bad.push(format!("nome de segmentação repetido em {n}"));
                }
                used_caches.extend(attr_str(a, b"cache"));
            });
        }
    }
    let mut cache_names = HashSet::new();
    for (_, kind, part) in &wb_rels {
        let tag: &[u8] = if kind.ends_with("/slicerCache") {
            b"slicerCacheDefinition"
        } else if kind.ends_with("/timelineCache") {
            b"timelineCacheDefinition"
        } else {
            continue;
        };
        let Ok(x) = pkg.read(part) else { continue };
        let name = find_tag(&x, 0, tag).and_then(|t| attr_str(t.attrs, b"name")).unwrap_or_default();
        if !cache_names.insert(name.clone()) {
            bad.push(format!("nome de cache repetido em {part}"));
        }
        for_each(&x, b"pivotTable", |a, _| {
            let id = attr_str(a, b"tabId").and_then(|s| s.parse::<u32>().ok());
            if id.is_some_and(|id| !ids.contains_key(&id)) {
                bad.push(format!("{part}: dinâmica numa aba que não existe"));
            }
        });
        for_each(&x, b"x15:tableSlicerCache", |a, _| {
            if attr_str(a, b"tableId").is_some_and(|t| !table_ids.contains(&t)) {
                bad.push(format!("{part}: tabela que não existe"));
            }
        });
    }
    for c in &used_caches {
        if !cache_names.contains(c) {
            bad.push("segmentação com cache que não existe".into());
        }
    }
    bad
}
