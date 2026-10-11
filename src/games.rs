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

impl Game {
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

pub fn find_by_nexus(domain: &str) -> Option<Game> {
    moddable_games()
        .into_iter()
        .find(|g| g.nexus.is_some_and(|n| n.eq_ignore_ascii_case(domain)))
}
