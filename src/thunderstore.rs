//! Thunderstore packages (the r2modman site): find a package, resolve its dependencies,
//! download and install them. The loader itself (BepInExPack) is left to the loader
//! installer, so packs are skipped as dependencies.

use crate::games::{Game, Loader};
use crate::records::Record;
use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

const API: &str = "https://thunderstore.io/api/experimental/package";

/// A package as the user names it: `Namespace-Name`, `Namespace-Name-1.2.3`, a
/// thunderstore.io page or an r2modman `ror2mm://` link.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageRef {
    pub namespace: String,
    pub name: String,
    pub version: Option<String>,
}

/// One version of a package, from the API.
#[derive(Debug, Clone)]
pub struct Package {
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub download_url: String,
    pub dependencies: Vec<String>,
}

impl Package {
    /// Folder name r2modman uses (`BepInEx/plugins/<Namespace-Name>`), which the mod list
    /// also reads.
    pub fn folder(&self) -> String {
        format!("{}-{}", self.namespace, self.name)
    }

    pub fn display_name(&self) -> String {
        self.name.replace('_', " ")
    }
}

fn plain(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn version_ok(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

impl PackageRef {
    pub fn parse(input: &str) -> Result<Self> {
        let s = input.trim();
        let bad = || anyhow!("{}", tr!("not a Thunderstore package: {s}", "não é um pacote da Thunderstore: {s}"));
        let parts: Vec<&str> = if let Some(rest) = s.strip_prefix("ror2mm://v1/install/") {
            // ror2mm://v1/install/thunderstore.io/<ns>/<name>/<version>/
            let p: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
            match p.as_slice() {
                [_host, ns, name, ver] => vec![ns, name, ver],
                _ => return Err(bad()),
            }
        } else if let Some(rest) = s.strip_prefix("https://thunderstore.io/").or_else(|| s.strip_prefix("thunderstore.io/")) {
            let p: Vec<&str> = rest.split(['?', '#']).next().unwrap_or("").trim_end_matches('/').split('/').collect();
            match p.as_slice() {
                ["c", _, "p", ns, name] | ["package", ns, name] => vec![ns, name],
                ["c", _, "p", ns, name, "v", ver] => vec![ns, name, ver],
                _ => return Err(bad()),
            }
        } else {
            // Namespace-Name[-1.2.3]; names may not contain '-', so split from the left.
            let p: Vec<&str> = s.splitn(3, '-').collect();
            if p.len() < 2 {
                return Err(bad());
            }
            p
        };
        let (ns, name) = (parts[0], parts[1]);
        let version = parts.get(2).map(|v| v.to_string());
        if !plain(ns) || !plain(name) || version.as_deref().is_some_and(|v| !version_ok(v)) {
            return Err(bad());
        }
        Ok(PackageRef { namespace: ns.into(), name: name.into(), version })
    }

    pub fn full_name(&self) -> String {
        format!("{}-{}", self.namespace, self.name)
    }
}

/// The loader packs (BepInExPack, BepInExPack_<Game>, MelonLoader) are installed by the
/// loader installer, not as mods.
pub fn is_loader_pack(r: &PackageRef) -> bool {
    r.name.starts_with("BepInExPack") || r.name.eq_ignore_ascii_case("MelonLoader") || r.full_name() == "BepInEx-BepInEx"
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("linex-mods/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(std::time::Duration::from_secs(15))
        .build()
}

fn get(url: &str) -> Result<serde_json::Value> {
    match agent().get(url).call() {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(404, _)) => bail!("{}", tr!("package not found on Thunderstore", "pacote não encontrado na Thunderstore")),
        Err(e) => Err(anyhow!(e).context(tr!("couldn't reach Thunderstore", "falha ao falar com a Thunderstore"))),
    }
}

fn package_from(v: &serde_json::Value) -> Result<Package> {
    let s = |k: &str| v[k].as_str().map(String::from).ok_or_else(|| anyhow!("Thunderstore: missing {k}"));
    Ok(Package {
        namespace: s("namespace")?,
        name: s("name")?,
        version: s("version_number")?,
        download_url: s("download_url")?,
        dependencies: v["dependencies"].as_array().into_iter().flatten().filter_map(|d| d.as_str().map(String::from)).collect(),
    })
}

/// The requested version, or the latest one.
pub fn fetch(r: &PackageRef) -> Result<Package> {
    let (ns, name) = (&r.namespace, &r.name);
    match &r.version {
        Some(v) => package_from(&get(&format!("{API}/{ns}/{name}/{v}/"))?),
        None => package_from(&get(&format!("{API}/{ns}/{name}/"))?["latest"]),
    }
}

/// The package and every dependency, dependencies first, without loader packs. Each
/// dependency comes in its latest version (the version in the list is a minimum).
pub fn resolve(root: &PackageRef) -> Result<Vec<Package>> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    visit(root, &mut seen, &mut order, 0)?;
    Ok(order)
}

fn visit(r: &PackageRef, seen: &mut HashSet<String>, order: &mut Vec<Package>, depth: u32) -> Result<()> {
    if depth > 20 || !seen.insert(r.full_name().to_lowercase()) || is_loader_pack(r) {
        return Ok(());
    }
    let pkg = fetch(r).with_context(|| r.full_name())?;
    for d in &pkg.dependencies {
        if let Ok(mut dep) = PackageRef::parse(d) {
            dep.version = None;
            visit(&dep, seen, order, depth + 1)?;
        }
    }
    order.push(pkg);
    Ok(())
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::steam::home().join(".cache"))
        .join("linex-mods/thunderstore")
}

/// Downloads the package zip (cached: a version never changes).
pub fn download(p: &Package, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    if !p.download_url.starts_with("https://thunderstore.io/") {
        bail!("Thunderstore: unexpected download address");
    }
    let dir = cache_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}-{}.zip", p.folder(), p.version));
    if path.is_file() {
        return Ok(path);
    }
    let resp = agent().get(&p.download_url).call().context(tr!("couldn't download the file", "falha ao baixar o arquivo"))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let part = path.with_extension("zip.part");
    let mut out = fs::File::create(&part)?;
    let (mut reader, mut buf, mut done) = (resp.into_reader(), vec![0u8; 64 * 1024], 0u64);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    out.flush()?;
    fs::rename(&part, &path)?;
    Ok(path)
}

/// Thunderstore's game list (the one r2modman uses), cached for a day: maps Steam AppIDs
/// to communities ("peak", "riskofrain2"...).
fn schema() -> Option<serde_json::Value> {
    let p = cache_dir().join("schema.json");
    let fresh = fs::metadata(&p)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() < 24 * 3600));
    if !fresh && let Ok(v) = get("https://thunderstore.io/api/experimental/schema/dev/latest/") {
        let _ = fs::create_dir_all(cache_dir());
        let _ = fs::write(&p, v.to_string());
        return Some(v);
    }
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

/// Thunderstore community of a Steam game, if it has one.
pub fn community_for(appid: u32) -> Option<String> {
    community_in(&schema()?, appid)
}

fn community_in(schema: &serde_json::Value, appid: u32) -> Option<String> {
    let id = appid.to_string();
    schema["games"].as_object()?.iter().find_map(|(key, g)| {
        let on_steam = g["distributions"]
            .as_array()?
            .iter()
            .any(|d| d["platform"] == "steam" && d["identifier"].as_str() == Some(id.as_str()));
        if !on_steam {
            return None;
        }
        // The community is in the package index URL: .../c/<community>/api/...
        let from_index = g["r2modman"][0]["packageIndex"]
            .as_str()
            .and_then(|u| u.split("/c/").nth(1))
            .and_then(|r| r.split('/').next())
            .map(String::from);
        Some(from_index.unwrap_or_else(|| key.clone()))
    })
}

/// Does the package belong to this community? The site only serves the package page
/// under its own community.
pub fn in_community(community: &str, r: &PackageRef) -> bool {
    if !plain_slug(community) {
        return false;
    }
    let url = format!("https://thunderstore.io/c/{community}/p/{}/{}/", r.namespace, r.name);
    agent().head(&url).call().is_ok()
}

fn plain_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Which of the games a package from a link (which doesn't say the game) is for: the
/// first game, in the given order, whose community has the package.
pub fn game_for<'a>(r: &PackageRef, games: &'a [Game]) -> Option<&'a Game> {
    games
        .iter()
        .filter(|g| g.loader != Loader::Smapi)
        .find(|g| community_for(g.steam.appid).is_some_and(|c| in_community(&c, r)))
}

/// Is this package (any version) already in the game? Then a dependency is not reinstalled.
pub fn installed(game: &Game, p: &Package) -> bool {
    let unit = format!("BepInEx/plugins/{}", p.folder());
    game.dir().join(&unit).exists() || game.dir().join(".linex-disabled").join(&unit).exists()
}

/// Installs the package with its dependencies. Returns the records, root package last.
pub fn install(game: &Game, r: &PackageRef, mut on_step: impl FnMut(&str)) -> Result<Vec<Record>> {
    if game.loader == Loader::Smapi {
        bail!("{}", tr!("Thunderstore packages are for BepInEx and MelonLoader games", "pacotes da Thunderstore são para jogos com BepInEx ou MelonLoader"));
    }
    let list = resolve(r)?;
    let mut done = Vec::new();
    for p in &list {
        let is_root = p.namespace.eq_ignore_ascii_case(&r.namespace) && p.name.eq_ignore_ascii_case(&r.name);
        if !is_root && installed(game, p) {
            continue;
        }
        on_step(&format!("{} {}", p.display_name(), p.version));
        let zip = download(p, |_, _| {})?;
        let rec = crate::install::install_thunderstore(game, &zip, &p.folder(), &p.display_name())
            .with_context(|| tr!("installing {}", "instalando {}", p.folder()))?;
        done.push(rec);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_names_and_links() {
        let want = |v: Option<&str>| PackageRef { namespace: "Owner".into(), name: "Cool_Mod".into(), version: v.map(String::from) };
        assert_eq!(PackageRef::parse("Owner-Cool_Mod").unwrap(), want(None));
        assert_eq!(PackageRef::parse("Owner-Cool_Mod-1.2.3").unwrap(), want(Some("1.2.3")));
        assert_eq!(PackageRef::parse("https://thunderstore.io/c/peak/p/Owner/Cool_Mod/").unwrap(), want(None));
        assert_eq!(PackageRef::parse("https://thunderstore.io/c/peak/p/Owner/Cool_Mod/v/1.2.3/").unwrap(), want(Some("1.2.3")));
        assert_eq!(PackageRef::parse("https://thunderstore.io/package/Owner/Cool_Mod/").unwrap(), want(None));
        assert_eq!(PackageRef::parse("ror2mm://v1/install/thunderstore.io/Owner/Cool_Mod/1.2.3/").unwrap(), want(Some("1.2.3")));
        for bad in ["Owner", "../x-y", "Owner-Cool_Mod-1.x", "Owner-Co/ol", "https://evil.example/c/p/p/A/B/", "Owner-Mod-1.2-3"] {
            assert!(PackageRef::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn finds_the_community_of_a_steam_game() {
        let schema = serde_json::json!({"games": {
            "peak": {"distributions": [{"platform": "steam", "identifier": "3527290"}],
                     "r2modman": [{"packageIndex": "https://thunderstore.io/c/peak/api/v1/package-listing-index/"}]},
            "other": {"distributions": [{"platform": "epic-games-store", "identifier": "3527290"}]},
            "ror2": {"distributions": [{"platform": "steam", "identifier": "632360"}]}
        }});
        assert_eq!(community_in(&schema, 3527290).as_deref(), Some("peak"));
        assert_eq!(community_in(&schema, 632360).as_deref(), Some("ror2"));
        assert_eq!(community_in(&schema, 1), None);
    }

    #[test]
    fn loader_packs_are_not_mods() {
        for p in ["BepInEx-BepInExPack-5.4.2100", "BepInEx-BepInExPack_PEAK", "LavaGang-MelonLoader-0.6.1"] {
            assert!(is_loader_pack(&PackageRef::parse(p).unwrap()), "{p}");
        }
        assert!(!is_loader_pack(&PackageRef::parse("Owner-Cool_Mod").unwrap()));
    }
}
