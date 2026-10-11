//! Nexus Mods: links nxm://, chave da API e download.

use crate::records::NexusSource;
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

const API: &str = "https://api.nexusmods.com/v1";
const KEYRING_SERVICE: &str = "linex-mods";
const KEYRING_USER: &str = "nexus-api-key";
pub const API_KEYS_PAGE: &str = "https://www.nexusmods.com/settings/api-keys";

/// `nxm://<domínio>/mods/<mod>/files/<arquivo>?key=..&expires=..`. Conta gratuita só
/// baixa com a chave de uso único que vem no link ("Mod Manager Download").
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
            .ok_or_else(|| anyhow!("não é um link nxm://"))?;
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        let [domain, "mods", mod_id, "files", file_id] = parts.as_slice() else {
            if parts.get(1) == Some(&"collections") {
                bail!("coleções do Nexus ainda não são suportadas");
            }
            bail!("link nxm:// em formato desconhecido");
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
            mod_id: mod_id.parse().context("ID do mod inválido")?,
            file_id: file_id.parse().context("ID do arquivo inválido")?,
            key,
            expires,
        })
    }

    /// Versão sem a chave de uso único, para log.
    pub fn redacted(&self) -> String {
        format!("nxm://{}/mods/{}/files/{}", self.domain, self.mod_id, self.file_id)
    }
}

// ---- chave da API -----------------------------------------------------------

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

/// Chave salva: primeiro o chaveiro do sistema, depois o arquivo de reserva.
pub fn load_key() -> Option<String> {
    if let Some(k) = entry().and_then(|e| e.get_password().ok()) {
        return Some(k);
    }
    fs::read_to_string(key_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Salva no chaveiro (Secret Service: KWallet, GNOME Keyring...). Sem chaveiro
/// funcionando, cai num arquivo legível só pelo usuário. Devolve onde ficou.
pub fn save_key(key: &str) -> Result<String> {
    let key = key.trim();
    if let Some(e) = entry() {
        if e.set_password(key).is_ok() && e.get_password().ok().as_deref() == Some(key) {
            let _ = fs::remove_file(key_file());
            return Ok("no chaveiro do sistema".into());
        }
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
    Ok(format!("em {} (chaveiro do sistema indisponível)", p.display()))
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
        Err(ureq::Error::Status(401, _)) => bail!("chave da API recusada pelo Nexus"),
        Err(ureq::Error::Status(403, _)) => {
            bail!("o Nexus negou o download (o link do site vale poucos minutos; clique de novo)")
        }
        Err(ureq::Error::Status(404, _)) => bail!("mod ou arquivo não encontrado no Nexus"),
        Err(ureq::Error::Status(429, _)) => bail!("limite de pedidos do Nexus atingido; tente mais tarde"),
        Err(e) => Err(anyhow!(e).context("falha ao falar com o Nexus")),
    }
}

/// Valida a chave: devolve o nome do usuário e se é premium.
pub fn validate(key: &str) -> Result<(String, bool)> {
    let v = get_json("/users/validate.json", key)?;
    let name = v["name"].as_str().unwrap_or("?").to_string();
    let premium = v["is_premium"].as_bool().unwrap_or(false);
    Ok((name, premium))
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

/// Baixa o arquivo do link. `progress(baixado, total)`; total 0 = desconhecido.
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
        .ok_or_else(|| anyhow!("o Nexus não devolveu link de download"))?
        .to_string();

    // Só o nome do arquivo, nunca um caminho vindo de fora.
    let file_name = file_info["file_name"]
        .as_str()
        .and_then(|n| std::path::Path::new(n).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{m}-{f}.zip"));
    let dir = downloads_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(&file_name);
    let part = dir.join(format!("{file_name}.part"));

    let resp = agent().get(&uri).call().context("falha ao baixar o arquivo")?;
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
    fn le_link_nxm() {
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
