//! Instala o loader de mods (MelonLoader ou BepInEx) em jogos Unity rodando pelo Proton.

use crate::games::Loader;
use crate::steam::{self, SteamGame};
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Jogo Unity de Windows (roda pelo Proton). `il2cpp` = compilado em C++ (GameAssembly.dll);
/// senão é Mono. Isso decide qual BepInEx serve.
#[derive(Debug, Clone)]
pub struct UnityGame {
    pub steam: SteamGame,
    pub il2cpp: bool,
}

pub fn unity_kind(dir: &Path) -> Option<bool> {
    let has_exe = fs::read_dir(dir).ok()?.flatten().any(|e| {
        e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"))
    });
    if !has_exe {
        return None;
    }
    if dir.join("GameAssembly.dll").is_file() {
        return Some(true);
    }
    if dir.join("UnityPlayer.dll").is_file() {
        return Some(false);
    }
    // Unity antigo (sem UnityPlayer.dll): <Jogo>_Data/Managed/UnityEngine.dll.
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        let is_data = p.is_dir() && e.file_name().to_string_lossy().ends_with("_Data");
        if is_data && p.join("il2cpp_data").is_dir() {
            Some(true)
        } else if is_data && p.join("Managed/UnityEngine.dll").is_file() {
            Some(false)
        } else {
            None
        }
    })
}

/// Jogos Unity instalados que ainda não têm loader.
pub fn unity_games_without_loader() -> Vec<UnityGame> {
    steam::installed_games()
        .into_iter()
        .filter(|g| crate::games::detect_loader(&g.dir).is_none())
        .filter_map(|g| unity_kind(&g.dir).map(|il2cpp| UnityGame { steam: g, il2cpp }))
        .collect()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("linex-mods/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(std::time::Duration::from_secs(15))
        .build()
}

/// Arquivo da última versão de um repositório do GitHub cujo nome satisfaz `pick`.
fn github_asset(repo: &str, tag: Option<&str>, pick: impl Fn(&str) -> bool) -> Result<(String, String)> {
    let url = match tag {
        Some(t) => format!("https://api.github.com/repos/{repo}/releases/tags/{t}"),
        None => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };
    let v: serde_json::Value = agent().get(&url).call().context("falha ao consultar o GitHub")?.into_json()?;
    v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|a| {
            let name = a["name"].as_str()?;
            let url = a["browser_download_url"].as_str()?;
            pick(name).then(|| (name.to_string(), url.to_string()))
        })
        .ok_or_else(|| anyhow!("não achei o arquivo certo na versão mais nova de {repo}"))
}

/// Build "bleeding edge" mais nova do BepInEx 6 (a página lista da mais nova para a
/// mais velha). Os jogos IL2CPP recentes precisam dela; a pre.2 do GitHub é antiga.
fn bepinex_be(il2cpp: bool) -> Result<(String, String)> {
    let kind = if il2cpp { "IL2CPP" } else { "Mono" };
    let html = agent()
        .get("https://builds.bepinex.dev/projects/bepinex_be")
        .call()
        .context("falha ao consultar builds.bepinex.dev")?
        .into_string()?;
    let needle = format!("BepInEx-Unity.{kind}-win-x64-");
    let href = html
        .split("href=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .find(|h| h.contains(&needle) && h.ends_with(".zip"))
        .ok_or_else(|| anyhow!("não achei a build do BepInEx 6 {kind}"))?;
    let name = href.rsplit('/').next().unwrap_or("bepinex.zip").replace("%2B", "+");
    Ok((name, format!("https://builds.bepinex.dev{href}")))
}

fn pick_download(loader: Loader, il2cpp: bool) -> Result<(String, String)> {
    match loader {
        Loader::MelonLoader => github_asset("LavaGang/MelonLoader", None, |n| n == "MelonLoader.x64.zip"),
        Loader::BepInEx if !il2cpp => {
            github_asset("BepInEx/BepInEx", None, |n| n.starts_with("BepInEx_win_x64_") && n.ends_with(".zip"))
        }
        Loader::BepInEx => bepinex_be(true).or_else(|_| {
            github_asset("BepInEx/BepInEx", Some("v6.0.0-pre.2"), |n| {
                n == "BepInEx-Unity.IL2CPP-win-x64-6.0.0-pre.2.zip"
            })
        }),
        Loader::Smapi => bail!("o SMAPI tem instalador próprio (smapi.io)"),
    }
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| steam::home().join(".cache"))
        .join("linex-mods/loaders")
}

/// Baixa (ou reaproveita do cache) e extrai o loader na pasta do jogo. Devolve o nome
/// do arquivo instalado (com a versão).
pub fn install(dir: &Path, loader: Loader, il2cpp: bool, mut progress: impl FnMut(u64, u64)) -> Result<String> {
    let (name, url) = pick_download(loader, il2cpp)?;
    let dest_dir = cache_dir();
    fs::create_dir_all(&dest_dir)?;
    let file = dest_dir.join(&name);
    if !file.is_file() {
        let resp = agent().get(&url).call().with_context(|| format!("falha ao baixar {name}"))?;
        let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let part = dest_dir.join(format!("{name}.part"));
        let mut out = fs::File::create(&part)?;
        let mut reader = resp.into_reader();
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
        fs::rename(&part, &file)?;
    }
    crate::install::extract(&file, dir).with_context(|| format!("extraindo {name} em {}", dir.display()))?;
    // O BepInEx só cria plugins/ ao abrir o jogo; criar já deixa o app listar e instalar.
    if loader == Loader::BepInEx {
        fs::create_dir_all(dir.join("BepInEx/plugins"))?;
    } else {
        fs::create_dir_all(dir.join("Mods"))?;
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconhece_unity() {
        let base = std::env::temp_dir().join(format!("linex-unity-{}", std::process::id()));
        let (mono, il2, old, nat) = (base.join("mono"), base.join("il2"), base.join("old"), base.join("nat"));
        for d in [&mono, &il2, &old.join("Jogo_Data/Managed"), &nat] {
            fs::create_dir_all(d).unwrap();
        }
        for (d, f) in [(&mono, "UnityPlayer.dll"), (&il2, "GameAssembly.dll"), (&old.join("Jogo_Data/Managed"), "UnityEngine.dll"), (&nat, "UnityPlayer.so")] {
            fs::write(d.join(f), "").unwrap();
        }
        for d in [&mono, &il2, &old] {
            fs::write(d.join("Jogo.exe"), "").unwrap();
        }
        assert_eq!(unity_kind(&mono), Some(false));
        assert_eq!(unity_kind(&il2), Some(true));
        assert_eq!(unity_kind(&old), Some(false));
        assert_eq!(unity_kind(&nat), None, "jogo nativo do Linux fica de fora");
        fs::remove_dir_all(base).unwrap();
    }
}
