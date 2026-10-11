//! Games from other launchers: Heroic (Epic, GOG, Amazon, sideloaded) and Lutris. Only
//! detection for now; installing mods still goes through Steam games.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct OtherGame {
    pub launcher: &'static str,
    pub name: String,
    pub dir: PathBuf,
}

fn heroic_roots() -> Vec<PathBuf> {
    let h = crate::steam::home();
    [h.join(".config/heroic"), h.join(".var/app/com.heroicgameslauncher.hgl/config/heroic")]
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}

fn lutris_dirs() -> Vec<PathBuf> {
    let h = crate::steam::home();
    [
        h.join(".local/share/lutris/games"),
        h.join(".config/lutris/games"),
        h.join(".var/app/net.lutris.Lutris/data/lutris/games"),
    ]
    .into_iter()
    .filter(|p| p.is_dir())
    .collect()
}

fn read_json(p: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn s(v: &serde_json::Value, k: &str) -> Option<String> {
    v[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

fn heroic(root: &Path, out: &mut Vec<OtherGame>) {
    let mut push = |name: Option<String>, dir: Option<String>| {
        if let Some(dir) = dir.map(PathBuf::from).filter(|d| d.is_dir()) {
            let name = name.unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            out.push(OtherGame { launcher: "Heroic", name, dir });
        }
    };
    // Epic (legendary): { "<app>": { "title", "install_path", "is_dlc" } }
    if let Some(serde_json::Value::Object(m)) = read_json(&root.join("legendaryConfig/legendary/installed.json")) {
        for g in m.values().filter(|g| !g["is_dlc"].as_bool().unwrap_or(false)) {
            push(s(g, "title"), s(g, "install_path"));
        }
    }
    // GOG: { "installed": [ { "appName", "install_path", "is_dlc" } ] }; Amazon (nile): [ { "id", "path" } ]
    if let Some(v) = read_json(&root.join("gog_store/installed.json")) {
        for g in v["installed"].as_array().into_iter().flatten().filter(|g| !g["is_dlc"].as_bool().unwrap_or(false)) {
            push(None, s(g, "install_path"));
        }
    }
    if let Some(v) = read_json(&root.join("nile_store/installed.json")) {
        for g in v.as_array().into_iter().flatten() {
            push(None, s(g, "path"));
        }
    }
    // Sideloaded: { "games": [ { "title", "folder_name", "is_installed", "install": { "executable" } } ] }
    if let Some(v) = read_json(&root.join("sideload_apps/library.json")) {
        for g in v["games"].as_array().into_iter().flatten().filter(|g| g["is_installed"].as_bool().unwrap_or(true)) {
            let exe_dir = s(&g["install"], "executable").and_then(|e| Path::new(&e).parent().map(|p| p.to_string_lossy().to_string()));
            push(s(g, "title"), exe_dir.or_else(|| s(g, "folder_name")));
        }
    }
}

/// The parts of a Lutris game .yml we need: top-level `name:` and, inside the top-level
/// `game:` block, `exe:`, `working_dir:` and `prefix:`. Simple enough to read by line.
fn lutris_yml(text: &str) -> (Option<String>, Option<PathBuf>) {
    let (mut name, mut exe, mut work, mut prefix) = (None, None, None, None);
    let mut in_game = false;
    for line in text.lines() {
        let indented = line.starts_with(' ');
        let t = line.trim();
        let value = |k: &str| t.strip_prefix(k).map(|v| v.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty());
        if !indented {
            in_game = t == "game:";
            if let Some(v) = value("name:") {
                name = Some(v);
            }
        } else if in_game && line.starts_with("  ") && !line.starts_with("   ") {
            exe = exe.or(value("exe:"));
            work = work.or(value("working_dir:"));
            prefix = prefix.or(value("prefix:"));
        }
    }
    let dir = work.map(PathBuf::from).or_else(|| {
        let exe = PathBuf::from(exe?);
        // A relative exe is inside the Wine prefix.
        let exe = if exe.is_absolute() { exe } else { PathBuf::from(prefix?).join(exe) };
        exe.parent().map(Path::to_path_buf)
    });
    (name, dir)
}

fn lutris(dir: &Path, out: &mut Vec<OtherGame>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "yml") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&p) else { continue };
        let (name, dir) = lutris_yml(&text);
        let Some(dir) = dir.filter(|d| d.is_dir()) else { continue };
        // "slug-1780182510.yml" when the file has no name.
        let slug = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let slug = slug.rsplit_once('-').filter(|(_, n)| n.chars().all(|c| c.is_ascii_digit())).map(|(s, _)| s.to_string()).unwrap_or(slug);
        out.push(OtherGame { launcher: "Lutris", name: name.unwrap_or(slug), dir });
    }
}

/// Installed games from Heroic and Lutris, sorted by name.
pub fn installed_games() -> Vec<OtherGame> {
    let mut out = Vec::new();
    for r in heroic_roots() {
        heroic(&r, &mut out);
    }
    for d in lutris_dirs() {
        lutris(&d, &mut out);
    }
    out.sort_by_key(|g| g.name.to_lowercase());
    out.dedup_by(|a, b| a.dir == b.dir);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lutris_yml() {
        let yml = "game:\n  exe: drive_c/Games/X/X.exe\n  prefix: /p\nname: My Game\nscript:\n  game:\n    exe: other.exe\n";
        let (name, dir) = lutris_yml(yml);
        assert_eq!(name.as_deref(), Some("My Game"));
        assert_eq!(dir, Some(PathBuf::from("/p/drive_c/Games/X")));
        let (name, dir) = lutris_yml("game:\n  exe: /g/Y/Y.exe\n  working_dir: /g/Y\n");
        assert_eq!((name, dir), (None, Some(PathBuf::from("/g/Y"))));
    }
}
