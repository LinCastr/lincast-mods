//! Missing dependencies. SMAPI mods declare them in manifest.json (`Dependencies`,
//! `ContentPackFor`) by UniqueID, and smapi.io says which Nexus mod each ID is. Other
//! Nexus mods list theirs on the Nexus ("requirements"). Thunderstore packages are
//! handled at install time (see thunderstore.rs).

use crate::games::{Game, Loader};
use crate::mods;
use anyhow::{Context, Result, anyhow};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Name and Nexus ID of a SMAPI UniqueID.
type IdInfo = (String, Option<u64>);

/// A dependency that is not installed.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Missing {
    /// SMAPI UniqueID (empty for Nexus requirements).
    pub unique_id: String,
    pub name: String,
    pub nexus_id: Option<u64>,
}

/// Every manifest.json under a SMAPI mod folder (packs have one per component).
fn manifests(dir: &Path, depth: u32, out: &mut Vec<serde_json::Value>) {
    let m = dir.join("manifest.json");
    if let Ok(text) = fs::read_to_string(&m)
        && let Some(v) = mods::lenient_json(&text)
    {
        out.push(v);
    }
    if depth == 0 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            manifests(&e.path(), depth - 1, out);
        }
    }
}

fn field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    v.as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
}

/// UniqueIDs a manifest needs: required `Dependencies` and `ContentPackFor`.
fn needs(man: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(id) = field(man, "ContentPackFor").and_then(|c| field(c, "UniqueID")).and_then(|v| v.as_str()) {
        out.push(id.trim().to_string());
    }
    for d in field(man, "Dependencies").and_then(|d| d.as_array()).into_iter().flatten() {
        let required = field(d, "IsRequired").and_then(|v| v.as_bool()).unwrap_or(true);
        if required && let Some(id) = field(d, "UniqueID").and_then(|v| v.as_str()) {
            out.push(id.trim().to_string());
        }
    }
    out
}

fn smapi_mod_dirs(game: &Game) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for root in [game.mods_dir(), game.disabled_dir()] {
        let Ok(rd) = fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                out.push((e.file_name().to_string_lossy().to_string(), e.path()));
            }
        }
    }
    out
}

/// SMAPI: mod folder → UniqueIDs it needs that no installed mod provides. A disabled
/// dependency counts as installed (turning it on is the fix, not downloading it).
pub fn smapi_missing_ids(game: &Game) -> HashMap<String, Vec<String>> {
    let dirs = smapi_mod_dirs(game);
    let mut have: HashSet<String> = HashSet::new();
    let mut wants: Vec<(String, Vec<String>)> = Vec::new();
    for (id, dir) in &dirs {
        let mut mans = Vec::new();
        manifests(dir, 3, &mut mans);
        let mut need = Vec::new();
        for m in &mans {
            if let Some(uid) = field(m, "UniqueID").and_then(|v| v.as_str()) {
                have.insert(uid.trim().to_lowercase());
            }
            need.extend(needs(m));
        }
        wants.push((id.clone(), need));
    }
    let mut out = HashMap::new();
    for (id, need) in wants {
        let mut missing: Vec<String> = need.into_iter().filter(|u| !have.contains(&u.to_lowercase())).collect();
        missing.sort();
        missing.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        if !missing.is_empty() {
            out.insert(id, missing);
        }
    }
    out
}

fn cache_file() -> PathBuf {
    crate::meta::icons_dir().with_file_name("smapi-ids.json")
}

/// Name and Nexus ID of SMAPI UniqueIDs, from smapi.io (cached on disk; IDs don't move).
pub fn resolve_smapi(ids: &[String]) -> HashMap<String, IdInfo> {
    let mut cache: HashMap<String, IdInfo> =
        fs::read_to_string(cache_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let unknown: Vec<&String> = ids.iter().filter(|i| !cache.contains_key(&i.to_lowercase())).collect();
    if !unknown.is_empty() && let Ok(found) = smapi_api(&unknown) {
        for (id, v) in found {
            cache.insert(id.to_lowercase(), v);
        }
        if let Some(dir) = cache_file().parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(cache_file(), serde_json::to_string(&cache).unwrap_or_default());
    }
    ids.iter().filter_map(|i| cache.get(&i.to_lowercase()).map(|v| (i.clone(), v.clone()))).collect()
}

fn smapi_api(ids: &[&String]) -> Result<Vec<(String, IdInfo)>> {
    let body = serde_json::json!({
        "mods": ids.iter().map(|i| serde_json::json!({"id": i})).collect::<Vec<_>>(),
        "includeExtendedMetadata": true,
        "apiVersion": "4.0.0",
        "platform": "Windows",
    });
    let v: serde_json::Value = ureq::AgentBuilder::new()
        .user_agent(concat!("linex-mods/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(std::time::Duration::from_secs(15))
        .build()
        .post("https://smapi.io/api/v4.6.0/mods")
        .send_json(body)
        .map_err(|e| anyhow!(e.to_string()))
        .context("smapi.io")?
        .into_json()?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let meta = &m["metadata"];
            let name = meta["name"].as_str().map(String::from).unwrap_or_else(|| pretty_id(&id));
            Some((id, (name, meta["nexusID"].as_u64())))
        })
        .collect())
}

/// "Pathoschild.ContentPatcher" → "ContentPatcher", until smapi.io answers.
pub fn pretty_id(id: &str) -> String {
    id.rsplit('.').next().unwrap_or(id).to_string()
}

/// SMAPI: missing dependencies of every mod, with names and Nexus IDs when smapi.io knows
/// them (network: call off the UI thread).
pub fn smapi_missing(game: &Game) -> HashMap<String, Vec<Missing>> {
    if game.loader != Loader::Smapi {
        return HashMap::new();
    }
    let ids = smapi_missing_ids(game);
    let all: Vec<String> = ids.values().flatten().cloned().collect();
    let known = resolve_smapi(&all);
    ids.into_iter()
        .map(|(m, list)| {
            let list = list
                .into_iter()
                .map(|u| {
                    let (name, nexus_id) = known.get(&u).cloned().unwrap_or_else(|| (pretty_id(&u), None));
                    Missing { unique_id: u, name, nexus_id }
                })
                .collect();
            (m, list)
        })
        .collect()
}

/// Nexus requirements of a mod that are not installed (by Nexus ID; SMAPI itself and
/// external links left out).
pub fn nexus_missing(game: &Game, mod_id: u64) -> Result<Vec<Missing>> {
    let domain = game.nexus_domain();
    let body = serde_json::json!({
        "query": "query($ids: [CompositeDomainWithIdInput!]!) { legacyModsByDomain(ids: $ids) { nodes { modRequirements { nexusRequirements { nodes { modId modName externalRequirement } } } } } }",
        "variables": {"ids": [{"gameDomain": domain, "modId": mod_id}]},
    });
    let v: serde_json::Value = ureq::AgentBuilder::new()
        .user_agent(concat!("linex-mods/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(std::time::Duration::from_secs(15))
        .build()
        .post("https://api.nexusmods.com/v2/graphql")
        .send_json(body)
        .map_err(|e| anyhow!(e.to_string()))
        .context("Nexus")?
        .into_json()?;
    let installed: HashSet<u64> = mods::list(game).into_iter().filter_map(|m| m.nexus_key.map(|(_, id)| id)).collect();
    let nodes = &v["data"]["legacyModsByDomain"]["nodes"][0]["modRequirements"]["nexusRequirements"]["nodes"];
    Ok(nodes
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| !n["externalRequirement"].as_bool().unwrap_or(false))
        .filter_map(|n| {
            let id: u64 = n["modId"].as_str().and_then(|s| s.parse().ok()).or(n["modId"].as_u64())?;
            let name = n["modName"].as_str().unwrap_or("").to_string();
            // The loader itself is installed differently (SMAPI has its own installer).
            let is_loader = name.starts_with("SMAPI") || name.eq_ignore_ascii_case("BepInEx") || name.eq_ignore_ascii_case("MelonLoader");
            (!installed.contains(&id) && !is_loader).then_some(Missing { unique_id: String::new(), name, nexus_id: Some(id) })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_manifest_needs() {
        let man = mods::lenient_json(
            r#"{ "UniqueID": "a.b", "ContentPackFor": { "UniqueID": "Pathoschild.ContentPatcher" },
                 "Dependencies": [ { "UniqueID": "spacechase0.SpaceCore" }, { "UniqueID": "x.optional", "IsRequired": false } ] }"#,
        )
        .unwrap();
        assert_eq!(needs(&man), ["Pathoschild.ContentPatcher", "spacechase0.SpaceCore"]);
        assert_eq!(pretty_id("Pathoschild.ContentPatcher"), "ContentPatcher");
    }
}
