//! Descobre a instalação do Steam, as bibliotecas e os jogos instalados.

use crate::vdf;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SteamGame {
    pub appid: u32,
    pub name: String,
    pub dir: PathBuf,
    pub library: PathBuf,
}

impl SteamGame {
    /// Jogo de Windows rodando pelo Proton. O prefixo em compatdata não basta: ele fica
    /// para trás quando um jogo nativo já foi aberto pelo Proton alguma vez (era o caso
    /// do Stardew). Então: tem .exe na pasta e nenhum executável Linux (ELF).
    pub fn uses_proton(&self) -> bool {
        let Ok(rd) = fs::read_dir(&self.dir) else { return false };
        let mut exe = false;
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
                exe = true;
            } else if is_elf_executable(&p) {
                return false;
            }
        }
        exe
    }
}

fn is_elf_executable(p: &Path) -> bool {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = fs::metadata(p) else { return false };
    if meta.permissions().mode() & 0o111 == 0 {
        return false;
    }
    let mut magic = [0u8; 4];
    fs::File::open(p).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == *b"\x7fELF"
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Pastas do Steam existentes: nativo, link antigo e Flatpak.
pub fn steam_roots() -> Vec<PathBuf> {
    let h = home();
    let candidates = [
        h.join(".local/share/Steam"),
        h.join(".steam/steam"),
        h.join(".steam/root"),
        h.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|p| p.join("steamapps").is_dir())
        .filter(|p| seen.insert(fs::canonicalize(p).unwrap_or_else(|_| p.clone())))
        .collect()
}

pub fn libraries() -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in steam_roots() {
        let mut libs = vec![root.clone()];
        if let Ok(txt) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
            let v = vdf::parse(&txt);
            if let Some(lf) = v.get("libraryfolders") {
                for (_, lib) in lf.entries() {
                    if let Some(p) = lib.str("path") {
                        libs.push(PathBuf::from(p));
                    }
                }
            }
        }
        for lib in libs {
            let key = fs::canonicalize(&lib).unwrap_or_else(|_| lib.clone());
            if lib.join("steamapps").is_dir() && seen.insert(key) {
                out.push(lib);
            }
        }
    }
    out
}

fn read_manifest(library: &Path, acf: &Path) -> Option<SteamGame> {
    let v = vdf::parse(&fs::read_to_string(acf).ok()?);
    let st = v.get("AppState")?;
    let appid = st.str("appid")?.parse().ok()?;
    let dir = library.join("steamapps/common").join(st.str("installdir")?);
    if !dir.is_dir() {
        return None;
    }
    Some(SteamGame {
        appid,
        name: st.str("name").unwrap_or("?").to_string(),
        dir,
        library: library.to_path_buf(),
    })
}

/// Arte do jogo no cache do Steam (`library_hero_blur.jpg`, `logo.png`...). Versões
/// novas do Steam guardam em subpastas com hash, então olha um nível abaixo também.
pub fn art(appid: u32, file: &str) -> Option<PathBuf> {
    for root in steam_roots() {
        let dir = root.join("appcache/librarycache").join(appid.to_string());
        let direct = dir.join(file);
        if direct.is_file() {
            return Some(direct);
        }
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path().join(file);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Opções de inicialização do jogo (de qualquer usuário logado neste Steam).
pub fn launch_options(appid: u32) -> Vec<String> {
    let mut out = Vec::new();
    for root in steam_roots() {
        let Ok(rd) = fs::read_dir(root.join("userdata")) else { continue };
        for user in rd.flatten() {
            let Ok(txt) = fs::read_to_string(user.path().join("config/localconfig.vdf")) else { continue };
            let v = vdf::parse(&txt);
            let opt = v
                .get("UserLocalConfigStore")
                .and_then(|s| s.get("Software"))
                .and_then(|s| s.get("Valve"))
                .and_then(|s| s.get("Steam"))
                .and_then(|s| s.get("apps"))
                .and_then(|s| s.get(&appid.to_string()))
                .and_then(|s| s.str("LaunchOptions"));
            if let Some(o) = opt {
                out.push(o.to_string());
            }
        }
    }
    out
}

pub fn installed_games() -> Vec<SteamGame> {
    let mut games = Vec::new();
    let mut seen = HashSet::new();
    for lib in libraries() {
        let Ok(rd) = fs::read_dir(lib.join("steamapps")) else { continue };
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("appmanifest_") && name.ends_with(".acf") {
                if let Some(g) = read_manifest(&lib, &e.path()) {
                    if seen.insert(g.appid) {
                        games.push(g);
                    }
                }
            }
        }
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games
}
