//! Quais jogos dá para modar e com qual loader.

use crate::steam::{self, SteamGame};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loader {
    MelonLoader,
    BepInEx,
    Smapi,
}

impl Loader {
    pub fn label(self) -> &'static str {
        match self {
            Loader::MelonLoader => "MelonLoader",
            Loader::BepInEx => "BepInEx",
            Loader::Smapi => "SMAPI",
        }
    }

    /// Pasta (relativa ao jogo) onde ficam os mods ativos.
    pub fn mods_dir(self) -> &'static str {
        match self {
            Loader::MelonLoader => "Mods",
            Loader::BepInEx => "BepInEx/plugins",
            Loader::Smapi => "Mods",
        }
    }

    /// DLL que o Proton precisa carregar da pasta do jogo (`WINEDLLOVERRIDES`).
    pub fn proxy_dll(self) -> Option<&'static str> {
        match self {
            Loader::MelonLoader => Some("version"),
            Loader::BepInEx => Some("winhttp"),
            Loader::Smapi => None,
        }
    }
}

/// Jogos conhecidos: loader esperado e o "domínio" do jogo no Nexus.
struct Known {
    appid: u32,
    loader: Loader,
    nexus: &'static str,
}

const KNOWN: &[Known] = &[
    Known { appid: 413150, loader: Loader::Smapi, nexus: "stardewvalley" },
    Known { appid: 1657630, loader: Loader::MelonLoader, nexus: "slimerancher2" },
    Known { appid: 960090, loader: Loader::MelonLoader, nexus: "bloonstd6" },
];

#[derive(Debug, Clone)]
pub struct Game {
    pub steam: SteamGame,
    /// Loader instalado de fato na pasta do jogo.
    pub installed_loader: Option<Loader>,
    /// Loader que o jogo usa (instalado ou, se conhecido, o esperado).
    pub loader: Loader,
    pub nexus: Option<&'static str>,
}

/// "Slime Rancher 2" → "slimerancher2": é como o Nexus forma o domínio da maioria
/// dos jogos (stardewvalley, bloonstd6, megabonk...).
fn as_nexus_domain(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

impl Game {
    /// Este jogo é o do domínio do Nexus? Usa a tabela conhecida e, para o resto,
    /// o nome do Steam no formato do domínio.
    pub fn matches_nexus(&self, domain: &str) -> bool {
        let d = domain.to_lowercase();
        self.nexus.map(|n| n == d).unwrap_or(false) || as_nexus_domain(&self.steam.name) == d
    }

    /// Game "domain" on the Nexus (stardewvalley, slimerancher2...).
    pub fn nexus_domain(&self) -> String {
        self.nexus.map(String::from).unwrap_or_else(|| as_nexus_domain(&self.steam.name))
    }

    pub fn dir(&self) -> &Path {
        &self.steam.dir
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.dir().join(self.loader.mods_dir())
    }

    /// Onde ficam os mods desligados (fora da vista do loader, nunca apagados).
    pub fn disabled_dir(&self) -> PathBuf {
        self.dir().join(".linex-disabled").join(self.loader.mods_dir())
    }
}

pub fn detect_loader(dir: &Path) -> Option<Loader> {
    if dir.join("StardewModdingAPI.dll").is_file() || dir.join("StardewModdingAPI").is_file() {
        Some(Loader::Smapi)
    } else if dir.join("MelonLoader").is_dir() {
        Some(Loader::MelonLoader)
    } else if dir.join("BepInEx").is_dir() {
        Some(Loader::BepInEx)
    } else {
        None
    }
}

fn from_steam(s: SteamGame) -> Option<Game> {
    let known = KNOWN.iter().find(|k| k.appid == s.appid);
    let installed = detect_loader(&s.dir);
    let loader = installed.or(known.map(|k| k.loader))?;
    Some(Game {
        installed_loader: installed,
        loader,
        nexus: known.map(|k| k.nexus),
        steam: s,
    })
}

/// Jogos instalados que têm loader ou que o app sabe modar.
pub fn moddable_games() -> Vec<Game> {
    steam::installed_games().into_iter().filter_map(from_steam).collect()
}

pub fn find(query: &str) -> Option<Game> {
    let games = moddable_games();
    if let Ok(id) = query.parse::<u32>() {
        return games.into_iter().find(|g| g.steam.appid == id);
    }
    let q = query.to_lowercase();
    games.into_iter().find(|g| g.steam.name.to_lowercase().contains(&q))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominio_pelo_nome() {
        assert_eq!(as_nexus_domain("Slime Rancher 2"), "slimerancher2");
        assert_eq!(as_nexus_domain("Megabonk"), "megabonk");
        assert_eq!(as_nexus_domain("Bloons TD 6"), "bloonstd6");
    }
}
