//! No Windows, põe no `.exe` o ícone e as informações de versão.
//!
//! O formato `.res` é simples e o `link.exe` o aceita direto: escrever aqui
//! dispensa o `rc.exe` e mais uma dependência.

use std::path::{Path, PathBuf};
use std::{env, fs};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
/// Português do Brasil.
const LANG: u16 = 0x0416;
const UTF16: u16 = 0x04B0;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let var = |k: &str| env::var(k).unwrap_or_default();
    if var("CARGO_CFG_TARGET_OS") != "windows" || var("CARGO_CFG_TARGET_ENV") != "msvc" {
        return;
    }
    let ico =
        fs::read(Path::new(&var("CARGO_MANIFEST_DIR")).join("assets/icon.ico")).expect("assets/icon.ico");
    let mut res = Vec::new();
    // Um recurso vazio abre todo `.res`.
    push(&mut res, 0, 0, &[]);
    icon(&mut res, &ico);
    push(&mut res, RT_VERSION, 1, &version(&var("CARGO_PKG_VERSION")));
    let out = PathBuf::from(var("OUT_DIR")).join("duck-sheets.res");
    fs::write(&out, res).expect("gravar o .res");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

fn push(res: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    let lang = if kind == 0 { 0 } else { LANG };
    res.extend((data.len() as u32).to_le_bytes());
    res.extend(32u32.to_le_bytes());
    for n in [0xFFFF, kind, 0xFFFF, id] {
        res.extend(n.to_le_bytes());
    }
    res.extend(0u32.to_le_bytes());
    for n in [if kind == 0 { 0 } else { 0x1010 }, lang] {
        res.extend(n.to_le_bytes());
    }
    res.extend([0; 8]);
    res.extend(data);
    align(res);
}

fn align(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

/// Cada imagem do `.ico` vira um recurso, e o grupo (número 1) as reúne.
fn icon(res: &mut Vec<u8>, ico: &[u8]) {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
    let n = u16_at(4);
    let mut group = ico[..6].to_vec();
    for k in 0..n {
        let e = 6 + 16 * k as usize;
        let (len, at) = (u32_at(e + 8), u32_at(e + 12));
        push(res, RT_ICON, k + 1, &ico[at..at + len]);
        group.extend(&ico[e..e + 12]);
        group.extend((k + 1).to_le_bytes());
    }
    push(res, RT_GROUP_ICON, 1, &group);
}

/// Um nó de `VS_VERSIONINFO`.
fn node(key: &str, text: bool, value: &[u8], children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    let len = if text { value.len() / 2 } else { value.len() };
    v.extend((len as u16).to_le_bytes());
    v.extend(u16::from(text).to_le_bytes());
    v.extend(wide(key));
    align(&mut v);
    v.extend(value);
    for c in children {
        align(&mut v);
        v.extend(c);
    }
    let total = (v.len() as u16).to_le_bytes();
    v[..2].copy_from_slice(&total);
    v
}

fn wide(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn version(v: &str) -> Vec<u8> {
    let core = v.split(['-', '+']).next().unwrap_or_default();
    let mut n = core.split('.').map(|x| x.parse::<u32>().unwrap_or(0)).chain([0; 4]);
    let mut next = || n.next().unwrap_or(0);
    let (hi, lo) = (next() << 16 | next(), next() << 16);
    let fixed: Vec<u8> = [0xFEEF_04BD, 0x1_0000, hi, lo, hi, lo, 0x3F, 0, 0x4_0004, 1, 0, 0, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", "duck-office"),
        ("FileDescription", "Duck Sheets"),
        ("FileVersion", v),
        ("InternalName", "duck-sheets"),
        ("LegalCopyright", "Licença MIT"),
        ("OriginalFilename", "duck-sheets.exe"),
        ("ProductName", "Duck Sheets"),
        ("ProductVersion", v),
    ]
    .into_iter()
    .map(|(k, v)| node(k, true, &wide(v), &[]))
    .collect();
    let table = node(&format!("{LANG:04X}{UTF16:04X}"), true, &[], &strings);
    let lang: Vec<u8> = [LANG, UTF16].into_iter().flat_map(u16::to_le_bytes).collect();
    node(
        "VS_VERSION_INFO",
        false,
        &fixed,
        &[
            node("StringFileInfo", true, &[], &[table]),
            node("VarFileInfo", true, &[], &[node("Translation", false, &lang, &[])]),
        ],
    )
}
