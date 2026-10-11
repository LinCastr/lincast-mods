//! Importing an r2modman profile into the game folder. A profile is a copy of the game's
//! BepInEx folder (BepInEx itself included) plus `mods.yml` with what each package is;
//! r2modman disables a mod by renaming its files to `*.old`.

use crate::games::Loader;
use crate::records::{self, Record, TsSource};
use crate::steam::{self, SteamGame};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Profile {
    pub game: SteamGame,
    pub name: String,
    pub dir: PathBuf,
    pub mods: usize,
}

/// One package of `mods.yml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub full_name: String,
    pub version: String,
    pub enabled: bool,
}

fn roots() -> Vec<PathBuf> {
    let h = steam::home();
    [h.join(".config/r2modmanPlus-local"), h.join(".var/app/com.github.ebkr.r2modman/config/r2modmanPlus-local")]
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}

/// r2modman's folder name for each Steam AppID (from Thunderstore's game list).
fn folder_for(schema: &serde_json::Value, appid: u32) -> Option<String> {
    let id = appid.to_string();
    schema["games"].as_object()?.values().find_map(|g| {
        let r = &g["r2modman"][0];
        let on_steam = r["distributions"].as_array()?.iter().any(|d| d["platform"] == "steam" && d["identifier"].as_str() == Some(id.as_str()));
        on_steam.then(|| r["internalFolderName"].as_str().map(String::from)).flatten()
    })
}

/// Profiles of installed Steam games that have at least one mod.
pub fn profiles() -> Vec<Profile> {
    let roots = roots();
    if roots.is_empty() {
        return Vec::new();
    }
    let Some(schema) = crate::thunderstore::schema() else { return Vec::new() };
    let mut out = Vec::new();
    for game in steam::installed_games() {
        let Some(folder) = folder_for(&schema, game.appid) else { continue };
        for root in &roots {
            let Ok(rd) = fs::read_dir(root.join(&folder).join("profiles")) else { continue };
            for e in rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
                let dir = e.path();
                let mods = fs::read_to_string(dir.join("mods.yml")).map(|t| parse_mods_yml(&t).len()).unwrap_or(0);
                if mods > 0 {
                    out.push(Profile { game: game.clone(), name: e.file_name().to_string_lossy().to_string(), dir, mods });
                }
            }
        }
    }
    out
}

/// The fields we need from `mods.yml`, read by line (top-level list of maps).
pub fn parse_mods_yml(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let (mut major, mut minor) = (String::new(), String::new());
    let mut in_version = false;
    for line in text.lines() {
        if line.starts_with("- ") {
            out.push(Entry { full_name: String::new(), version: String::new(), enabled: true });
            in_version = false;
        }
        let Some(cur) = out.last_mut() else { continue };
        let t = line.trim_start_matches("- ").trim();
        let indent = line.len() - line.trim_start().len();
        if indent <= 2 {
            in_version = t == "versionNumber:";
        }
        let value = |k: &str| t.strip_prefix(k).map(|v| v.trim().trim_matches(['\'', '"']).to_string());
        if indent <= 2 {
            if let Some(v) = value("name:") {
                cur.full_name = v;
            } else if let Some(v) = value("enabled:") {
                cur.enabled = v != "false";
            }
        } else if in_version {
            if let Some(v) = value("major:") {
                major = v;
            } else if let Some(v) = value("minor:") {
                minor = v;
            } else if let Some(v) = value("patch:") {
                cur.version = format!("{major}.{minor}.{v}");
            }
        }
    }
    out.retain(|e| !e.full_name.is_empty());
    out
}

fn copy_tree(src: &Path, dst: &Path, overwrite: bool) -> Result<()> {
    let t = fs::symlink_metadata(src)?.file_type();
    if t.is_dir() {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(src)?.flatten() {
            copy_tree(&e.path(), &dst.join(e.file_name()), overwrite)?;
        }
    } else if t.is_file() && (overwrite || !dst.exists()) {
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        fs::copy(src, dst).with_context(|| tr!("copying to {}", "copiando para {}", dst.display()))?;
    }
    Ok(())
}

/// Files renamed `x.old` by r2modman get their name back.
fn restore_old(dir: &Path) -> Result<()> {
    for e in fs::read_dir(dir)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            restore_old(&p)?;
        } else if let Some(name) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".old")) {
            fs::rename(&p, p.with_file_name(name))?;
        }
    }
    Ok(())
}

const SUBDIRS: [&str; 4] = ["plugins", "patchers", "core", "monomod"];

pub struct Report {
    pub mods: usize,
    pub loader_installed: bool,
}

/// Copies the profile into the game folder and records each package. BepInEx comes too
/// when the game doesn't have it yet. The user's configs from r2modman are kept, but
/// never over configs the game folder already has.
pub fn import(p: &Profile) -> Result<Report> {
    let game_dir = &p.game.dir;
    let src = p.dir.join("BepInEx");
    if !src.is_dir() {
        bail!("{}", tr!("the profile has no BepInEx folder", "o perfil não tem pasta BepInEx"));
    }
    let loader_installed = crate::games::detect_loader(game_dir).is_none();
    if loader_installed {
        for f in ["winhttp.dll", "doorstop_config.ini", ".doorstop_version"] {
            if p.dir.join(f).is_file() {
                copy_tree(&p.dir.join(f), &game_dir.join(f), false)?;
            }
        }
    } else if crate::games::detect_loader(game_dir) != Some(Loader::BepInEx) {
        bail!("{}", tr!("this game uses another loader, not BepInEx", "este jogo usa outro loader, não o BepInEx"));
    }
    let dst = game_dir.join("BepInEx");
    for sub in SUBDIRS {
        let Ok(rd) = fs::read_dir(src.join(sub)) else { continue };
        for e in rd.flatten() {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            // Loose files in core/ are BepInEx itself: only when installing it.
            if sub == "core" && !is_dir && !loader_installed {
                continue;
            }
            copy_tree(&e.path(), &dst.join(sub).join(e.file_name()), is_dir || sub != "core")?;
        }
    }
    copy_tree(&src.join("config"), &dst.join("config"), false).ok();

    let entries = fs::read_to_string(p.dir.join("mods.yml")).map(|t| parse_mods_yml(&t)).unwrap_or_default();
    let mut recs = records::load(p.game.appid);
    let off = game_dir.join(".linex-disabled");
    let mut count = 0;
    for e in &entries {
        let Ok(r) = crate::thunderstore::PackageRef::parse(&e.full_name) else { continue };
        if crate::thunderstore::is_loader_pack(&r) {
            continue;
        }
        let units: Vec<String> = SUBDIRS
            .iter()
            .map(|sub| format!("BepInEx/{sub}/{}", e.full_name))
            .filter(|u| game_dir.join(u).is_dir())
            .collect();
        if units.is_empty() {
            continue;
        }
        for u in &units {
            restore_old(&game_dir.join(u))?;
            if !e.enabled {
                let to = off.join(u);
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                if !to.exists() {
                    fs::rename(game_dir.join(u), &to)?;
                }
            }
        }
        recs.put(Record {
            name: r.name.replace('_', " "),
            files: units,
            archive: String::new(),
            nexus: None,
            thunderstore: Some(TsSource { namespace: r.namespace.clone(), name: r.name.clone(), version: e.version.clone() }),
            installed_at: records::now(),
            warning: None,
        });
        count += 1;
    }
    records::save(p.game.appid, &recs)?;
    Ok(Report { mods: count, loader_installed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_mods_yml() {
        let yml = "- manifestVersion: 1\n  name: Cysharp-UniTask\n  authorName: Cysharp\n  dependencies:\n    - BepInEx-BepInExPack_PEAK-5.4.2403\n  versionNumber:\n    major: 2\n    minor: 5\n    patch: 0\n  enabled: true\n- manifestVersion: 1\n  name: IntRabbit-Last_Chance\n  versionNumber:\n    major: 1\n    minor: 0\n    patch: 3\n  enabled: false\n";
        assert_eq!(
            parse_mods_yml(yml),
            [
                Entry { full_name: "Cysharp-UniTask".into(), version: "2.5.0".into(), enabled: true },
                Entry { full_name: "IntRabbit-Last_Chance".into(), version: "1.0.3".into(), enabled: false },
            ]
        );
    }
}
