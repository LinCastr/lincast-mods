//! Record of what each mod installed, to uninstall without leftovers.
//! Kept in ~/.local/share/linex-mods/<appid>.json, outside the game folder.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusSource {
    pub domain: String,
    pub mod_id: u64,
    pub file_id: u64,
    #[serde(default)]
    pub version: Option<String>,
}

/// Package installed from Thunderstore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TsSource {
    pub namespace: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    /// Paths relative to the game folder, always at the "enabled" location (e.g. `Mods/X.dll`,
    /// `Mods/Mod Folder`). A folder counts as one whole unit.
    pub files: Vec<String>,
    pub archive: String,
    #[serde(default)]
    pub nexus: Option<NexusSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thunderstore: Option<TsSource>,
    #[serde(default)]
    pub installed_at: u64,
    /// Install warning (e.g. mod made for another loader). Not saved.
    #[serde(skip)]
    pub warning: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Records {
    #[serde(default)]
    pub mods: Vec<Record>,
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::steam::home().join(".local/share"))
        .join("linex-mods")
}

fn path(appid: u32) -> PathBuf {
    data_dir().join(format!("{appid}.json"))
}

pub fn load(appid: u32) -> Records {
    fs::read_to_string(path(appid))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(appid: u32, recs: &Records) -> Result<()> {
    let p = path(appid);
    fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(recs)?)?;
    fs::rename(&tmp, &p).with_context(|| tr!("saving {}", "salvando {}", p.display()))
}

impl Records {
    /// Record that owns this path (the path itself or a folder containing it).
    pub fn owner(&self, rel: &str) -> Option<usize> {
        self.mods.iter().position(|r| {
            r.files.iter().any(|f| f == rel || rel.starts_with(&format!("{f}/")))
        })
    }

    /// Adds a record, dropping old ones that held the same paths (reinstall or update
    /// of the same mod). A library in a shared folder (UserLibs/) doesn't count: two
    /// different mods often ship the same one.
    pub fn put(&mut self, rec: Record) {
        self.mods.retain(|r| {
            r.name != rec.name
                && !r.files.iter().any(|f| !is_shared_dir(f) && rec.files.contains(f))
        });
        self.mods.push(rec);
    }

    /// Does any other record (besides `except`) use this path? Dependencies in
    /// UserLibs are often shared between mods.
    pub fn shared(&self, rel: &str, except: usize) -> bool {
        self.mods
            .iter()
            .enumerate()
            .any(|(i, r)| i != except && r.files.iter().any(|f| f == rel))
    }
}

fn is_shared_dir(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    first.eq_ignore_ascii_case("UserLibs")
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str, files: &[&str]) -> Record {
        Record { name: name.into(), files: files.iter().map(|f| f.to_string()).collect(), archive: String::new(), nexus: None, thunderstore: None, installed_at: 0, warning: None }
    }

    #[test]
    fn shared_library_does_not_drop_the_other_mod() {
        let mut r = Records::default();
        r.put(rec("A", &["Mods/A.dll", "UserLibs/L.dll"]));
        r.put(rec("B", &["Mods/B.dll", "UserLibs/L.dll"]));
        assert_eq!(r.mods.len(), 2);
        assert!(r.shared("UserLibs/L.dll", 0));
        // Reinstalling A replaces only A.
        r.put(rec("A v2", &["Mods/A.dll", "UserLibs/L.dll"]));
        assert_eq!(r.mods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["B", "A v2"]);
        assert_eq!(r.owner("Mods/Pasta/sub"), None);
    }
}
