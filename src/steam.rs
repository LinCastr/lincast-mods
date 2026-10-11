//! Finds the Steam installation, its libraries and the installed games.

use crate::vdf;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SteamGame {
    pub appid: u32,
    pub name: String,
    pub dir: PathBuf,
}

impl SteamGame {
    /// Windows game running through Proton. A prefix in compatdata is not enough: it stays
    /// behind when a native game was once opened through Proton (Stardew was such a
    /// case). So: there is an .exe in the folder and no Linux (ELF) executable.
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

/// Existing Steam folders: native, the old symlink and Flatpak.
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
    })
}

/// Game art in Steam's cache (`library_hero_blur.jpg`, `logo.png`...). Newer Steam
/// versions keep it in hashed subfolders, so look one level down too.
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

/// Small game icon (32×32). In Steam's new cache it is the .jpg with a hash name
/// (40 hex characters) inside the game's folder; in the old one, `<appid>_icon.jpg`.
pub fn icon(appid: u32) -> Option<PathBuf> {
    for root in steam_roots() {
        let cache = root.join("appcache/librarycache");
        let dir = cache.join(appid.to_string());
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if let Some(stem) = n.strip_suffix(".jpg")
                    && stem.len() == 40 && stem.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(e.path());
                }
            }
        }
        let old = cache.join(format!("{appid}_icon.jpg"));
        if old.is_file() {
            return Some(old);
        }
    }
    None
}

/// The game's launch options (from any user logged in to this Steam).
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
            if name.starts_with("appmanifest_") && name.ends_with(".acf")
                && let Some(g) = read_manifest(&lib, &e.path())
                && seen.insert(g.appid) {
                games.push(g);
            }
        }
    }
    games.sort_by_key(|a| a.name.to_lowercase());
    games
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn proton_only_when_there_is_no_linux_executable() {
        let base = std::env::temp_dir().join(format!("linex-proton-{}", std::process::id()));
        let game = |name: &str| SteamGame { appid: 1, name: name.into(), dir: base.join(name) };
        let (win, native, stale) = (game("win"), game("native"), game("stale"));
        for g in [&win, &native, &stale] {
            fs::create_dir_all(&g.dir).unwrap();
            fs::write(g.dir.join("Game.exe"), "MZ").unwrap();
        }
        // A real ELF binary next to the .exe: native game (Stardew ships both).
        fs::write(native.dir.join("Game"), b"\x7fELF....").unwrap();
        fs::set_permissions(native.dir.join("Game"), fs::Permissions::from_mode(0o755)).unwrap();
        // ELF magic without the executable bit (a .so, say) does not count.
        fs::write(stale.dir.join("lib.so"), b"\x7fELF....").unwrap();
        assert!(win.uses_proton());
        assert!(!native.uses_proton());
        assert!(stale.uses_proton());
        assert!(!game("missing").uses_proton());
        fs::remove_dir_all(base).unwrap();
    }
}
