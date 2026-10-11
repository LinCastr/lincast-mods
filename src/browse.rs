//! Searching mods on the Nexus (GraphQL API v2, no key needed) and on Thunderstore (the
//! community's package list, cached for an hour), and spotting mods already installed
//! from the other site.

use crate::mods::{Mod, Source};
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Popular,
    Updated,
    Name,
}

/// One search result.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Listing {
    pub source: Option<Source>,
    /// Nexus mod ID, or Thunderstore "Namespace-Name".
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub author: String,
    pub summary: String,
    pub version: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub page: String,
    /// Thunderstore dependencies ("Namespace-Name"), loader packs left out.
    pub deps: Vec<String>,
}

const PAGE: usize = 60;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("linex-mods/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .build()
}

// ---- Nexus -------------------------------------------------------------------

pub fn nexus_search(domain: &str, query: &str, sort: Sort, adult: bool) -> Result<Vec<Listing>> {
    let mut filter = serde_json::json!({
        "gameDomainName": [{"value": domain}],
        "status": [{"value": "published"}],
    });
    if !adult {
        filter["adultContent"] = serde_json::json!([{"value": false}]);
    }
    let q = query.trim();
    if !q.is_empty() {
        filter["nameStemmed"] = serde_json::json!([{"value": q}]);
    }
    let sort = match sort {
        _ if !q.is_empty() && sort == Sort::Popular => serde_json::json!([{"relevance": {"direction": "DESC"}}, {"downloads": {"direction": "DESC"}}]),
        Sort::Popular => serde_json::json!([{"downloads": {"direction": "DESC"}}]),
        Sort::Updated => serde_json::json!([{"updatedAt": {"direction": "DESC"}}]),
        Sort::Name => serde_json::json!([{"name": {"direction": "ASC"}}]),
    };
    let body = serde_json::json!({
        "query": "query($filter: ModsFilter, $sort: [ModsSort!], $count: Int) { mods(filter: $filter, sort: $sort, count: $count) { nodes { modId name summary version author pictureUrl thumbnailUrl downloads } } }",
        "variables": {"filter": filter, "sort": sort, "count": PAGE},
    });
    let v: serde_json::Value = agent()
        .post("https://api.nexusmods.com/v2/graphql")
        .send_json(body)
        .map_err(|e| anyhow!(e.to_string()))
        .context(tr!("couldn't reach the Nexus", "falha ao falar com o Nexus"))?
        .into_json()?;
    if let Some(err) = v["errors"][0]["message"].as_str() {
        return Err(anyhow!("Nexus: {err}"));
    }
    let s = |n: &serde_json::Value, k: &str| n[k].as_str().unwrap_or("").trim().to_string();
    Ok(v["data"]["mods"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["modId"].as_u64()?;
            Some(Listing {
                source: Some(Source::Nexus),
                id: id.to_string(),
                name: s(n, "name"),
                author: s(n, "author"),
                summary: s(n, "summary"),
                version: s(n, "version"),
                downloads: n["downloads"].as_u64().unwrap_or(0),
                icon_url: n["thumbnailUrl"].as_str().or(n["pictureUrl"].as_str()).map(String::from),
                page: crate::nexus::mod_page(domain, id),
                ..Default::default()
            })
        })
        .collect())
}

// ---- Thunderstore ----------------------------------------------------------------

#[derive(Clone)]
struct TsPackage {
    listing: Listing,
    updated: String,
    rating: i64,
    hidden: bool,
}

type TsCache = HashMap<String, (Instant, Vec<TsPackage>)>;
static TS_CACHE: Mutex<Option<TsCache>> = Mutex::new(None);

fn ts_cache_file(community: &str) -> std::path::PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::steam::home().join(".cache"))
        .join(format!("linex-mods/thunderstore/{community}-packages.json"))
}

/// The community's whole package list: memory, then disk (an hour), then the site.
fn ts_packages(community: &str) -> Result<Vec<TsPackage>> {
    const TTL: Duration = Duration::from_secs(3600);
    if let Some((t, list)) = TS_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(community))
        && t.elapsed() < TTL
    {
        return Ok(list.clone());
    }
    let file = ts_cache_file(community);
    let fresh = fs::metadata(&file).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e < TTL));
    let text = match fresh {
        true => fs::read_to_string(&file)?,
        false => {
            let url = format!("https://thunderstore.io/c/{community}/api/v1/package/");
            let text = agent()
                .get(&url)
                .call()
                .map_err(|e| anyhow!(e.to_string()))
                .context(tr!("couldn't reach Thunderstore", "falha ao falar com a Thunderstore"))?
                .into_string()?;
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&file, &text);
            text
        }
    };
    let list = parse_ts(community, &text)?;
    TS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(community.to_string(), (Instant::now(), list.clone()));
    Ok(list)
}

fn parse_ts(community: &str, text: &str) -> Result<Vec<TsPackage>> {
    let v: serde_json::Value = serde_json::from_str(text).context("Thunderstore")?;
    let s = |n: &serde_json::Value, k: &str| n[k].as_str().unwrap_or("").trim().to_string();
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let latest = p["versions"].as_array()?.first()?;
            let (ns, name) = (s(p, "owner"), s(p, "name"));
            let r = crate::thunderstore::PackageRef { namespace: ns.clone(), name: name.clone(), version: None };
            let categories: Vec<String> = p["categories"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_lowercase)).collect();
            // Loader packs, modpacks and r2modman itself are not mods to install here.
            if crate::thunderstore::is_loader_pack(&r) || categories.iter().any(|c| c == "modpacks") || r.full_name() == "ebkr-r2modman" {
                return None;
            }
            let downloads = p["versions"].as_array()?.iter().filter_map(|v| v["downloads"].as_u64()).sum();
            let deps = latest["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| crate::thunderstore::PackageRef::parse(d.as_str()?).ok())
                .filter(|d| !crate::thunderstore::is_loader_pack(d))
                .map(|d| d.full_name())
                .collect();
            Some(TsPackage {
                listing: Listing {
                    source: Some(Source::Thunderstore),
                    id: format!("{ns}-{name}"),
                    page: crate::thunderstore::package_page(community, &ns, &name),
                    name: name.replace('_', " "),
                    author: ns.clone(),
                    namespace: ns,
                    summary: s(latest, "description"),
                    version: s(latest, "version_number"),
                    downloads,
                    icon_url: latest["icon"].as_str().map(String::from),
                    deps,
                },
                updated: s(p, "date_updated"),
                rating: p["rating_score"].as_i64().unwrap_or(0),
                hidden: p["is_deprecated"].as_bool().unwrap_or(false) || p["has_nsfw_content"].as_bool().unwrap_or(false),
            })
        })
        .collect())
}

/// Latest version of every package in the community: "namespace-name" (lowercase) → version.
pub fn ts_latest(community: &str) -> Result<HashMap<String, String>> {
    Ok(ts_packages(community)?
        .into_iter()
        .map(|p| (p.listing.id.to_lowercase(), p.listing.version))
        .collect())
}

pub fn ts_search(community: &str, query: &str, sort: Sort, show_hidden: bool) -> Result<Vec<Listing>> {
    let q = normalize(query);
    let mut list: Vec<TsPackage> = ts_packages(community)?
        .into_iter()
        .filter(|p| show_hidden || !p.hidden)
        .filter(|p| q.is_empty() || normalize(&p.listing.name).contains(&q) || normalize(&p.listing.author).contains(&q))
        .collect();
    match sort {
        Sort::Popular => list.sort_by(|a, b| b.listing.downloads.cmp(&a.listing.downloads).then(b.rating.cmp(&a.rating))),
        Sort::Updated => list.sort_by(|a, b| b.updated.cmp(&a.updated)),
        Sort::Name => list.sort_by_key(|p| p.listing.name.to_lowercase()),
    }
    // With a search, exact name matches first.
    if !q.is_empty() {
        list.sort_by_key(|p| normalize(&p.listing.name) != q);
    }
    Ok(list.into_iter().take(PAGE).map(|p| p.listing).collect())
}

// ---- Installed or similar --------------------------------------------------------

/// Name for comparing across sites: lowercase letters and digits only, without the
/// loader suffixes packages add ("NPCMapLocations_BepInEx").
pub fn normalize(s: &str) -> String {
    let mut n: String = s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    for suffix in ["bepinex5", "bepinex", "melonloader", "smapi"] {
        if n.len() > suffix.len() + 2 && n.ends_with(suffix) {
            n.truncate(n.len() - suffix.len());
        }
    }
    n
}

/// How a result relates to what is installed.
#[derive(Debug, Clone, PartialEq)]
pub enum Match {
    None,
    /// The same mod, from the same site. `newer` = the site has a newer version.
    Installed { newer: bool },
    /// A mod with the same name, installed from the other site (or unknown origin).
    Similar { name: String, from: Source },
}

pub fn match_installed(l: &Listing, installed: &[Mod]) -> Match {
    let newer = |m: &Mod| m.version.as_deref().is_some_and(|v| !l.version.is_empty() && crate::nexus::is_newer(&l.version, v));
    let same = installed.iter().find(|m| match l.source {
        Some(Source::Nexus) => m.nexus_key.as_ref().is_some_and(|(_, id)| id.to_string() == l.id),
        Some(Source::Thunderstore) => m.ts_key.as_ref().is_some_and(|(ns, n)| format!("{ns}-{n}").eq_ignore_ascii_case(&l.id)),
        _ => false,
    });
    if let Some(m) = same {
        return Match::Installed { newer: newer(m) };
    }
    let key = normalize(&l.name);
    if key.len() < 3 {
        return Match::None;
    }
    match installed.iter().find(|m| normalize(&m.name) == key) {
        Some(m) => Match::Similar { name: m.name.clone(), from: m.source },
        None => Match::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(name: &str, source: Source) -> Mod {
        let mut m = crate::mods::Mod::bare(name);
        m.source = source;
        if source == Source::Nexus {
            m.nexus_key = Some(("stardewvalley".into(), 239));
            m.version = Some("3.5.0".into());
        }
        m
    }

    #[test]
    fn spots_the_same_mod_on_the_other_site() {
        assert_eq!(normalize("NPC_Map_Locations_BepInEx"), normalize("NPC Map Locations"));
        let have = [installed("NPC Map Locations", Source::Nexus)];
        let ts = Listing { source: Some(Source::Thunderstore), id: "x-NPC_Map_Locations".into(), name: "NPC Map Locations".into(), ..Default::default() };
        assert_eq!(match_installed(&ts, &have), Match::Similar { name: "NPC Map Locations".into(), from: Source::Nexus });
        let nexus = Listing { source: Some(Source::Nexus), id: "239".into(), name: "NPC Map Locations".into(), version: "3.5.2".into(), ..Default::default() };
        assert_eq!(match_installed(&nexus, &have), Match::Installed { newer: true });
        let other = Listing { source: Some(Source::Nexus), id: "1".into(), name: "Other".into(), ..Default::default() };
        assert_eq!(match_installed(&other, &have), Match::None);
    }

    #[test]
    fn reads_the_thunderstore_list() {
        let text = r#"[
          {"owner":"A","name":"Cool_Mod","categories":["Mods"],"date_updated":"2026-01-02","rating_score":5,"is_deprecated":false,"has_nsfw_content":false,
           "versions":[{"version_number":"1.1.0","description":"Does things","icon":"https://x/i.png","downloads":10,"dependencies":["BepInEx-BepInExPack-5.4.2100","B-Lib-1.0.0"]},{"version_number":"1.0.0","downloads":5}]},
          {"owner":"BepInEx","name":"BepInExPack_PEAK","categories":[],"versions":[{"version_number":"5","downloads":1}]},
          {"owner":"C","name":"Old","categories":[],"is_deprecated":true,"versions":[{"version_number":"1","downloads":99}]}
        ]"#;
        let list = parse_ts("peak", text).unwrap();
        assert_eq!(list.len(), 2, "loader pack skipped");
        let cool = &list[0].listing;
        assert_eq!((cool.id.as_str(), cool.name.as_str(), cool.downloads), ("A-Cool_Mod", "Cool Mod", 15));
        assert_eq!(cool.deps, ["B-Lib"]);
        assert!(list[1].hidden);
    }
}
