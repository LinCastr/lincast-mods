//! Registro do que cada mod instalou, para desinstalar sem deixar sobra.
//! Fica em ~/.local/share/linex-mods/<appid>.json, fora da pasta do jogo.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    /// Caminhos relativos à pasta do jogo, sempre no lugar "ligado" (ex.: `Mods/X.dll`,
    /// `Mods/Pasta Do Mod`). Uma pasta conta como uma unidade inteira.
    pub files: Vec<String>,
    pub archive: String,
    #[serde(default)]
    pub nexus: Option<NexusSource>,
    #[serde(default)]
    pub installed_at: u64,
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
    fs::rename(&tmp, &p).with_context(|| format!("salvando {}", p.display()))
}

impl Records {
    /// Registro dono deste caminho (o próprio caminho ou uma pasta que o contém).
    pub fn owner(&self, rel: &str) -> Option<usize> {
        self.mods.iter().position(|r| {
            r.files.iter().any(|f| f == rel || rel.starts_with(&format!("{f}/")))
        })
    }

    /// Adiciona um registro, tirando os antigos que ocupavam os mesmos caminhos
    /// (reinstalação ou atualização do mesmo mod). Biblioteca em pasta compartilhada
    /// (UserLibs/) não conta: dois mods diferentes costumam trazer a mesma.
    pub fn put(&mut self, rec: Record) {
        self.mods.retain(|r| {
            r.name != rec.name
                && !r.files.iter().any(|f| !is_shared_dir(f) && rec.files.contains(f))
        });
        self.mods.push(rec);
    }

    /// Algum outro registro (além de `except`) usa este caminho? Dependências em
    /// UserLibs costumam ser compartilhadas entre mods.
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
        Record { name: name.into(), files: files.iter().map(|f| f.to_string()).collect(), archive: String::new(), nexus: None, installed_at: 0 }
    }

    #[test]
    fn biblioteca_compartilhada_nao_apaga_o_outro_mod() {
        let mut r = Records::default();
        r.put(rec("A", &["Mods/A.dll", "UserLibs/L.dll"]));
        r.put(rec("B", &["Mods/B.dll", "UserLibs/L.dll"]));
        assert_eq!(r.mods.len(), 2);
        assert!(r.shared("UserLibs/L.dll", 0));
        // Reinstalar A substitui só A.
        r.put(rec("A v2", &["Mods/A.dll", "UserLibs/L.dll"]));
        assert_eq!(r.mods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["B", "A v2"]);
        assert_eq!(r.owner("Mods/Pasta/sub"), None);
    }
}
