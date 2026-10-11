//! Lista, liga e desliga os mods de um jogo.

use crate::games::{Game, Loader};
use crate::records;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Mod {
    /// Nome do arquivo ou pasta dentro da pasta de mods; identifica o mod.
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub enabled: bool,
    /// Instalado pelo botão do Nexus (tem a origem no registro).
    pub from_nexus: bool,
    pub nexus: Option<records::NexusSource>,
    /// Mod on the Nexus (game domain, mod id): from the install record or, for SMAPI,
    /// from the manifest's UpdateKeys ("Nexus:1234").
    pub nexus_key: Option<(String, u64)>,
    smapi_nexus_id: Option<u64>,
    /// `icon.png` shipped inside the mod folder (Thunderstore packages have one).
    pub icon_src: Option<std::path::PathBuf>,
}

/// "Nexus:1234" or "Nexus:1234@sub" among the UpdateKeys of a SMAPI manifest.
fn smapi_nexus_id(man: &serde_json::Value) -> Option<u64> {
    let keys = man.as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case("UpdateKeys"))?.1.as_array()?;
    keys.iter().filter_map(|k| k.as_str()).find_map(|k| {
        let (site, rest) = k.split_once(':')?;
        if !site.trim().eq_ignore_ascii_case("nexus") {
            return None;
        }
        let digits: String = rest.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    })
}

/// "GrowableMoondewNectarMod" → "Growable Moondew Nectar Mod", "SR2_INF" → "SR2 INF".
/// Só para DLL sem registro (instalada à mão): o nome real do mod não está em lugar nenhum.
fn humanize(stem: &str) -> String {
    // Names that must not be split ("BepInEx" would become "Bep In Ex").
    const KEEP: [&str; 3] = ["BepInEx", "MelonLoader", "IL2CPP"];
    let mut stem = stem.to_string();
    for (i, k) in KEEP.iter().enumerate() {
        stem = stem.replace(k, &format!(" \u{1}{i}\u{1} "));
    }
    let chars: Vec<char> = stem.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            out.push(' ');
            continue;
        }
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || (prev.is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(c);
    }
    let mut out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for (i, k) in KEEP.iter().enumerate() {
        out = out.replace(&format!("\u{1}{i}\u{1}"), k);
    }
    out
}

/// The mod's manifest.json (SMAPI or Thunderstore), if it has one.
pub fn manifest(path: &Path) -> Option<serde_json::Value> {
    if path.is_dir() { find_manifest(path, 2) } else { None }
}

/// Where the mod is now (enabled or disabled location).
pub fn mod_path(game: &Game, id: &str) -> Option<std::path::PathBuf> {
    [game.mods_dir().join(id), game.disabled_dir().join(id)].into_iter().find(|p| p.exists())
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

pub fn json_field(v: &serde_json::Value, key: &str) -> Option<String> {
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
        from_nexus: false,
        nexus: None,
        nexus_key: None,
        smapi_nexus_id: None,
        icon_src: path.join("icon.png").is_file().then(|| path.join("icon.png")),
    };
    match loader {
        Loader::MelonLoader => {
            if !path.is_file() || !m.id.to_lowercase().ends_with(".dll") {
                return None;
            }
            m.name = humanize(&m.id[..m.id.len() - 4]);
        }
        Loader::BepInEx => {
            if path.is_file() {
                if !m.id.to_lowercase().ends_with(".dll") {
                    return None;
                }
                m.name = humanize(&m.id[..m.id.len() - 4]);
            } else if let Some(man) = fs::read_to_string(path.join("manifest.json")).ok().and_then(|t| lenient_json(&t)) {
                // Thunderstore package (r2modman layout): folder "Author-Name" with a
                // manifest.json carrying the name and version_number.
                if let Some(n) = json_field(&man, "name") {
                    let n = n.trim_end_matches("_BepInEx").trim_end_matches("BepInEx");
                    m.name = humanize(if n.is_empty() { &m.id } else { n });
                }
                m.version = json_field(&man, "version_number");
                if let Some((author, _)) = m.id.split_once('-') {
                    m.author = Some(author.to_string());
                }
            } else {
                m.name = humanize(&m.id);
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
                m.smapi_nexus_id = smapi_nexus_id(&man);
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
    // Mods do SMAPI já têm nome no manifest.json.
    let recs = records::load(game.steam.appid);
    for m in &mut mods {
        if let Some(i) = recs.owner(&format!("{}/{}", game.loader.mods_dir(), m.id)) {
            let r = &recs.mods[i];
            m.from_nexus = r.nexus.is_some();
            m.nexus = r.nexus.clone();
            m.nexus_key = r.nexus.as_ref().map(|n| (n.domain.clone(), n.mod_id));
            if game.loader != Loader::Smapi {
                m.name = r.name.clone();
            }
            if m.version.is_none() {
                m.version = r.nexus.as_ref().and_then(|n| n.version.clone());
            }
        }
    }
    let domain = game.nexus_domain();
    for m in &mut mods {
        if m.nexus_key.is_none() {
            m.nexus_key = m.smapi_nexus_id.map(|id| (domain.clone(), id));
        }
    }
    mods.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    mods
}

pub fn set_enabled(game: &Game, id: &str, enable: bool) -> Result<()> {
    if id.contains('/') || id == ".." || id.is_empty() {
        bail!("{}", tr!("invalid mod name: {id}", "nome de mod inválido: {id}"));
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
        bail!("{}", tr!("mod not found: {id}", "mod não encontrado: {id}"));
    }
    if dst.exists() {
        bail!("{}", tr!("there is already a mod named {id} in {}", "já existe um mod com o nome {id} em {}", to.display()));
    }
    fs::create_dir_all(&to).with_context(|| tr!("creating {}", "criando {}", to.display()))?;
    fs::rename(&src, &dst).with_context(|| tr!("moving {} to {}", "movendo {} para {}", src.display(), dst.display()))
}

/// Manda o mod para a Lixeira (recuperável), com tudo o que ele instalou e que
/// nenhum outro mod usa. Mod sem registro (instalado à mão): só a própria unidade.
pub fn uninstall(game: &Game, id: &str) -> Result<String> {
    if id.contains('/') || id == ".." || id.is_empty() {
        bail!("{}", tr!("invalid mod name: {id}", "nome de mod inválido: {id}"));
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
                trash::delete(&p).with_context(|| tr!("sending {} to the trash", "mandando {} para a Lixeira", p.display()))?;
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
    fn nomes_de_dll_legiveis() {
        assert_eq!(humanize("GrowableMoondewNectarMod"), "Growable Moondew Nectar Mod");
        assert_eq!(humanize("SR2_INF"), "SR2 INF");
        assert_eq!(humanize("RichPresenceMod"), "Rich Presence Mod");
        assert_eq!(humanize("UIInfoSuite"), "UI Info Suite");
        assert_eq!(humanize("BepInExConfigManager"), "BepInEx Config Manager");
    }

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
