//! Nexus Mods: nxm:// links, API key and download.

use crate::records::NexusSource;
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

const API: &str = "https://api.nexusmods.com/v1";
const KEYRING_SERVICE: &str = "linex-mods";
const KEYRING_USER: &str = "nexus-api-key";
pub const API_KEYS_PAGE: &str = "https://www.nexusmods.com/settings/api-keys";

/// `nxm://<domain>/mods/<mod>/files/<file>?key=..&expires=..`. A free account can only
/// download with the one-time key in the link ("Mod Manager Download").
#[derive(Debug, Clone)]
pub struct NxmLink {
    pub domain: String,
    pub mod_id: u64,
    pub file_id: u64,
    pub key: Option<String>,
    pub expires: Option<String>,
}

impl NxmLink {
    pub fn parse(url: &str) -> Result<Self> {
        let rest = url
            .strip_prefix("nxm://")
            .ok_or_else(|| anyhow!("{}", tr!("not an nxm:// link", "não é um link nxm://")))?;
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        let [domain, "mods", mod_id, "files", file_id] = parts.as_slice() else {
            if parts.get(1) == Some(&"collections") {
                bail!("{}", tr!("Nexus collections are not supported yet", "coleções do Nexus ainda não são suportadas"));
            }
            bail!("{}", tr!("unknown nxm:// link format", "link nxm:// em formato desconhecido"));
        };
        let mut key = None;
        let mut expires = None;
        for kv in query.split('&') {
            match kv.split_once('=') {
                Some(("key", v)) => key = Some(v.to_string()),
                Some(("expires", v)) => expires = Some(v.to_string()),
                _ => {}
            }
        }
        Ok(NxmLink {
            domain: domain.to_lowercase(),
            mod_id: mod_id.parse().context(tr!("invalid mod ID", "ID do mod inválido"))?,
            file_id: file_id.parse().context(tr!("invalid file ID", "ID do arquivo inválido"))?,
            key,
            expires,
        })
    }

    /// Version without the one-time key, for the log.
    pub fn redacted(&self) -> String {
        format!("nxm://{}/mods/{}/files/{}", self.domain, self.mod_id, self.file_id)
    }
}

// ---- API key ----------------------------------------------------------------

fn key_file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::steam::home().join(".config"))
        .join("linex-mods/nexus-key")
}

fn entry() -> Option<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()
}

/// Saved key: first the system keyring, then the fallback file.
pub fn load_key() -> Option<String> {
    if let Some(k) = entry().and_then(|e| e.get_password().ok()) {
        return Some(k);
    }
    fs::read_to_string(key_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Saves to the keyring (Secret Service: KWallet, GNOME Keyring...). Without a working
/// keyring, falls back to a file only the user can read. Returns where it went.
pub fn save_key(key: &str) -> Result<String> {
    let key = key.trim();
    if let Some(e) = entry()
        && e.set_password(key).is_ok() && e.get_password().ok().as_deref() == Some(key) {
        let _ = fs::remove_file(key_file());
        return Ok(tr!("in the system keyring", "no chaveiro do sistema"));
    }
    use std::os::unix::fs::OpenOptionsExt;
    let p = key_file();
    fs::create_dir_all(p.parent().unwrap())?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&p)?;
    f.write_all(key.as_bytes())?;
    Ok(tr!("in {} (system keyring unavailable)", "em {} (chaveiro do sistema indisponível)", p.display()))
}

pub fn clear_key() {
    if let Some(e) = entry() {
        let _ = e.delete_credential();
    }
    let _ = fs::remove_file(key_file());
}

// ---- API --------------------------------------------------------------------

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("linex-mods/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(std::time::Duration::from_secs(15))
        .build()
}

fn get_json(path_and_query: &str, key: &str) -> Result<serde_json::Value> {
    let url = format!("{API}{path_and_query}");
    match agent()
        .get(&url)
        .set("apikey", key)
        .set("Application-Name", "linex-mods")
        .set("Application-Version", env!("CARGO_PKG_VERSION"))
        .call()
    {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(401, _)) => bail!("{}", tr!("the Nexus rejected the API key", "chave da API recusada pelo Nexus")),
        Err(ureq::Error::Status(403, _)) => {
            bail!("{}", tr!("the Nexus refused the download (the website link only lasts a few minutes; click it again)", "o Nexus negou o download (o link do site vale poucos minutos; clique de novo)"))
        }
        Err(ureq::Error::Status(404, _)) => bail!("{}", tr!("mod or file not found on the Nexus", "mod ou arquivo não encontrado no Nexus")),
        Err(ureq::Error::Status(429, _)) => bail!("{}", tr!("Nexus request limit reached; try again later", "limite de pedidos do Nexus atingido; tente mais tarde")),
        Err(e) => Err(anyhow!(e).context(tr!("couldn't reach the Nexus", "falha ao falar com o Nexus"))),
    }
}

/// Validates the key: returns the user name and whether it is premium.
pub fn validate(key: &str) -> Result<(String, bool)> {
    let v = get_json("/users/validate.json", key)?;
    let name = v["name"].as_str().unwrap_or("?").to_string();
    let premium = v["is_premium"].as_bool().unwrap_or(false);
    Ok((name, premium))
}

/// Current version and main picture of a mod on the Nexus.
pub fn mod_info(domain: &str, mod_id: u64, key: &str) -> Result<crate::meta::NexusInfo> {
    let v = get_json(&format!("/games/{domain}/mods/{mod_id}.json"), key)?;
    let s = |k: &str| v[k].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Ok(crate::meta::NexusInfo { version: s("version"), picture: s("picture_url"), fetched: crate::records::now() })
}

pub fn mod_page(domain: &str, mod_id: u64) -> String {
    format!("https://www.nexusmods.com/{domain}/mods/{mod_id}?tab=files")
}

/// Is `remote` newer than `local`? Compares number by number ("1.10" > "1.9");
/// when that isn't possible, any difference counts.
pub fn is_newer(remote: &str, local: &str) -> bool {
    let norm = |s: &str| s.trim().trim_start_matches(['v', 'V']).to_string();
    let (r, l) = (norm(remote), norm(local));
    if r == l {
        return false;
    }
    let nums = |s: &str| -> Option<Vec<u64>> {
        let mut v: Vec<u64> = s.split(['.', '-', '_']).map(|p| p.parse().ok()).collect::<Option<_>>()?;
        while v.len() > 1 && v.last() == Some(&0) {
            v.pop();
        }
        Some(v)
    };
    match (nums(&r), nums(&l)) {
        (Some(a), Some(b)) => a > b,
        _ => true,
    }
}

pub struct Downloaded {
    pub path: PathBuf,
    pub mod_name: String,
    pub source: NexusSource,
}

fn downloads_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::steam::home().join(".cache"))
        .join("linex-mods/downloads")
}

/// Downloads the link's file. `progress(done, total)`; total 0 = unknown.
pub fn download(link: &NxmLink, key: &str, mut progress: impl FnMut(u64, u64)) -> Result<Downloaded> {
    let (d, m, f) = (&link.domain, link.mod_id, link.file_id);
    let mod_info = get_json(&format!("/games/{d}/mods/{m}.json"), key)?;
    let file_info = get_json(&format!("/games/{d}/mods/{m}/files/{f}.json"), key)?;
    let mut q = String::new();
    if let (Some(k), Some(e)) = (&link.key, &link.expires) {
        q = format!("?key={k}&expires={e}");
    }
    let links = get_json(&format!("/games/{d}/mods/{m}/files/{f}/download_link.json{q}"), key)?;
    let uri = links
        .as_array()
        .and_then(|a| a.first())
        .and_then(|l| l["URI"].as_str())
        .ok_or_else(|| anyhow!("{}", tr!("the Nexus returned no download link", "o Nexus não devolveu link de download")))?
        .to_string();

    // Only the file name, never a path from outside.
    let file_name = file_info["file_name"]
        .as_str()
        .and_then(|n| std::path::Path::new(n).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{m}-{f}.zip"));
    let dir = downloads_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(&file_name);
    let part = dir.join(format!("{file_name}.part"));

    let resp = agent().get(&uri).call().context(tr!("couldn't download the file", "falha ao baixar o arquivo"))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut out = fs::File::create(&part)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
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

    let mod_name = mod_info["name"].as_str().unwrap_or(&file_name).to_string();
    Ok(Downloaded {
        path,
        mod_name,
        source: NexusSource {
            domain: d.clone(),
            mod_id: m,
            file_id: f,
            version: file_info["version"].as_str().or(mod_info["version"].as_str()).map(String::from),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("1.10", "1.9"));
        assert!(is_newer("v2.0.1", "2.0"));
        assert!(!is_newer("1.0.0", "1.0"));
        assert!(!is_newer("1.2", "1.3"));
        assert!(is_newer("1.2b", "1.2a"));
    }

    #[test]
    fn parses_nxm_link() {
        let l = NxmLink::parse("nxm://SlimeRancher2/mods/191/files/1234?key=abc&expires=999&user_id=5").unwrap();
        assert_eq!(l.domain, "slimerancher2");
        assert_eq!((l.mod_id, l.file_id), (191, 1234));
        assert_eq!(l.key.as_deref(), Some("abc"));
        assert_eq!(l.expires.as_deref(), Some("999"));
        assert!(!l.redacted().contains("abc"));
        assert!(NxmLink::parse("nxm://x/collections/abc").is_err());
        assert!(NxmLink::parse("https://x").is_err());
    }
}
