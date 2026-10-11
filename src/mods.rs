//! Lista, liga e desliga os mods de um jogo.

use crate::games::{Game, Loader};
use crate::records;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Mod {
    /// Nome do arquivo ou pasta dentro da pasta de mods; identifica o mod.
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub enabled: bool,
    pub path: PathBuf,
}

/// Arquivos que acompanham mods mas não são mods.
fn is_junk(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with('.')
        || n.ends_with(".md")
        || n.ends_with(".txt")
        || n.ends_with(".url")
        || n.ends_with(".pdb")
        || n.ends_with(".xml")
        || n == "license"
}

/// O manifest.json do SMAPI aceita comentários, vírgula sobrando e BOM;
/// o serde_json não, então limpamos antes.
pub fn lenient_json(src: &str) -> Option<serde_json::Value> {
    let src = src.trim_start_matches('\u{feff}');
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            _ => out.push(c),
        }
    }
    // Vírgula antes de } ou ].
    let mut clean = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut in_str = false;
    for (i, &c) in bytes.iter().enumerate() {
        if c == '"' && (i == 0 || bytes[i - 1] != '\\') {
            in_str = !in_str;
        }
        if c == ',' && !in_str {
            let next = bytes[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        clean.push(c);
    }
    serde_json::from_str(&clean).ok()
}

fn json_field(v: &serde_json::Value, key: &str) -> Option<String> {
    v.as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Primeiro manifest.json da pasta (ela mesma ou até dois níveis abaixo).
fn find_manifest(dir: &Path, depth: u32) -> Option<serde_json::Value> {
    let m = dir.join("manifest.json");
    if m.is_file() {
        return lenient_json(&fs::read_to_string(m).ok()?);
    }
    if depth == 0 {
        return None;
    }
    let mut subs: Vec<_> = fs::read_dir(dir).ok()?.flatten().filter(|e| e.path().is_dir()).collect();
    subs.sort_by_key(|e| e.file_name());
    subs.iter().find_map(|e| find_manifest(&e.path(), depth - 1))
}

fn describe(loader: Loader, path: &Path, enabled: bool) -> Option<Mod> {
    let id = path.file_name()?.to_string_lossy().to_string();
    if is_junk(&id) {
        return None;
    }
    let mut m = Mod {
        name: id.clone(),
        id,
        version: None,
        author: None,
        enabled,
        path: path.to_path_buf(),
    };
    match loader {
        Loader::MelonLoader => {
            if !path.is_file() || !m.id.to_lowercase().ends_with(".dll") {
                return None;
            }
            m.name = m.id[..m.id.len() - 4].to_string();
        }
        Loader::BepInEx => {
            if path.is_file() {
                if !m.id.to_lowercase().ends_with(".dll") {
                    return None;
                }
                m.name = m.id[..m.id.len() - 4].to_string();
            }
        }
        Loader::Smapi => {
            if !path.is_dir() {
                return None;
            }
            if let Some(man) = find_manifest(path, 2) {
                // Pacote com vários componentes ("Ridgeside Village/[CP] …"): o nome
                // da pasta descreve melhor que o do primeiro componente.
                if path.join("manifest.json").is_file() {
                    if let Some(n) = json_field(&man, "Name") {
                        m.name = n;
                    }
                }
                m.version = json_field(&man, "Version");
                m.author = json_field(&man, "Author");
            }
        }
    }
    Some(m)
}

fn scan(loader: Loader, dir: &Path, enabled: bool, out: &mut Vec<Mod>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    out.extend(rd.flatten().filter_map(|e| describe(loader, &e.path(), enabled)));
}

pub fn list(game: &Game) -> Vec<Mod> {
    let mut mods = Vec::new();
    scan(game.loader, &game.mods_dir(), true, &mut mods);
    scan(game.loader, &game.disabled_dir(), false, &mut mods);
    // DLLs têm nome de arquivo ("ContinuousSprint"); o registro guarda o nome do mod.
    if game.loader != Loader::Smapi {
        let recs = records::load(game.steam.appid);
        for m in &mut mods {
            if let Some(i) = recs.owner(&format!("{}/{}", game.loader.mods_dir(), m.id)) {
                let r = &recs.mods[i];
                m.name = r.name.clone();
                if m.version.is_none() {
                    m.version = r.nexus.as_ref().and_then(|n| n.version.clone());
                }
            }
        }
    }
    mods.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    mods
}

pub fn set_enabled(game: &Game, id: &str, enable: bool) -> Result<()> {
    if id.contains('/') || id == ".." || id.is_empty() {
        bail!("nome de mod inválido: {id}");
    }
    let (from, to) = if enable {
        (game.disabled_dir(), game.mods_dir())
    } else {
        (game.mods_dir(), game.disabled_dir())
    };
    let src = from.join(id);
    let dst = to.join(id);
    if !src.exists() {
        if dst.exists() {
            return Ok(());
        }
        bail!("mod não encontrado: {id}");
    }
    if dst.exists() {
        bail!("já existe um mod com o nome {id} em {}", to.display());
    }
    fs::create_dir_all(&to).with_context(|| format!("criando {}", to.display()))?;
    fs::rename(&src, &dst).with_context(|| format!("movendo {} para {}", src.display(), dst.display()))
}

/// Manda o mod para a Lixeira (recuperável), com tudo o que ele instalou e que
/// nenhum outro mod usa. Mod sem registro (instalado à mão): só a própria unidade.
pub fn uninstall(game: &Game, id: &str) -> Result<String> {
    if id.contains('/') || id == ".." || id.is_empty() {
        bail!("nome de mod inválido: {id}");
    }
    let appid = game.steam.appid;
    let unit = format!("{}/{id}", game.loader.mods_dir());
    let mut recs = records::load(appid);
    let owner = recs.owner(&unit);
    let (name, targets): (String, Vec<String>) = match owner {
        Some(i) => (
            recs.mods[i].name.clone(),
            recs.mods[i].files.iter().filter(|f| !recs.shared(f, i)).cloned().collect(),
        ),
        None => (id.to_string(), vec![unit.clone()]),
    };
    let dir = game.dir();
    let disabled_root = dir.join(".linex-disabled");
    let mut trashed = Vec::new();
    for t in &targets {
        for p in [dir.join(t), disabled_root.join(t)] {
            if p.exists() {
                trash::delete(&p).with_context(|| format!("mandando {} para a Lixeira", p.display()))?;
                trashed.push(p);
            }
        }
    }
    // Pastas que ficaram vazias (ex.: UserLibs/<mod>/), sem subir até as do loader.
    for p in &trashed {
        let mut cur = p.parent().map(Path::to_path_buf);
        while let Some(d) = cur {
            if d == dir || d == disabled_root || d.parent() == Some(dir) || d.parent() == Some(disabled_root.as_path()) || d == game.mods_dir() {
                break;
            }
            if fs::remove_dir(&d).is_err() {
                break;
            }
            cur = d.parent().map(Path::to_path_buf);
        }
    }
    if let Some(i) = owner {
        recs.mods.remove(i);
        records::save(appid, &recs)?;
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_com_comentario_e_virgula() {
        let v = lenient_json(
            "\u{feff}{\n // comentário\n \"Name\": \"Mod // não é comentário\",\n \"Version\": \"1.0\", /* x */\n}",
        )
        .unwrap();
        assert_eq!(json_field(&v, "name").as_deref(), Some("Mod // não é comentário"));
        assert_eq!(json_field(&v, "Version").as_deref(), Some("1.0"));
    }
}
