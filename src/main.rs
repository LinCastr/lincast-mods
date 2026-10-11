mod config;
mod desktop;
mod games;
mod gui;
mod install;
mod ipc;
mod loaders;
mod meta;
mod log;
mod mods;
mod nexus;
mod records;
mod steam;
mod theme;
mod vdf;

use anyhow::{Context, Result, bail};
use std::path::Path;

const USAGE: &str = "uso:
  linex-mods jogos
  linex-mods mods <jogo>
  linex-mods instalar <jogo> <arquivo.zip|.7z|.rar>...
  linex-mods ligar <jogo> <mod>
  linex-mods desligar <jogo> <mod>
  linex-mods remover <jogo> <mod>
  linex-mods unity
  linex-mods loader <jogo> melonloader|bepinex

<jogo> é o AppID do Steam ou parte do nome.";

fn game(q: &str) -> Result<games::Game> {
    games::find(q).with_context(|| format!("jogo não encontrado (ou sem loader de mods): {q}"))
}

fn run(args: &[String]) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["jogos"] => {
            for g in games::moddable_games() {
                let state = match g.installed_loader {
                    Some(l) => l.label().to_string(),
                    None => format!("{} não instalado", g.loader.label()),
                };
                let proton = if g.steam.uses_proton() { " · Proton" } else { "" };
                println!("{:>8}  {}  [{state}{proton}]", g.steam.appid, g.steam.name);
            }
        }
        ["mods", q] => {
            let g = game(q)?;
            for m in mods::list(&g) {
                let on = if m.enabled { "ligado   " } else { "desligado" };
                let ver = m.version.map(|v| format!(" {v}")).unwrap_or_default();
                let by = m.author.map(|a| format!(" — {a}")).unwrap_or_default();
                println!("{on}  {}{ver}{by}   ({})", m.name, m.id);
            }
        }
        ["instalar", q, files @ ..] if !files.is_empty() => {
            let g = game(q)?;
            for f in files {
                let rec = install::install_archive(&g, Path::new(f), None, None).with_context(|| format!("instalando {f}"))?;
                println!("{}:", rec.name);
                for n in rec.files {
                    println!("  → {n}");
                }
                if let Some(w) = rec.warning {
                    println!("  aviso: {w}");
                }
            }
        }
        ["ligar", q, id] => mods::set_enabled(&game(q)?, id, true)?,
        ["desligar", q, id] => mods::set_enabled(&game(q)?, id, false)?,
        ["remover", q, id] => println!("{} foi para a Lixeira", mods::uninstall(&game(q)?, id)?),
        ["unity"] => {
            for u in loaders::unity_games_without_loader() {
                println!("{:>8}  {}  [{}]", u.steam.appid, u.steam.name, if u.il2cpp { "IL2CPP" } else { "Mono" });
            }
        }
        ["loader", q, which] => {
            let q = q.to_lowercase();
            let g = steam::installed_games()
                .into_iter()
                .find(|g| g.appid.to_string() == q || g.name.to_lowercase().contains(&q))
                .with_context(|| format!("jogo não encontrado: {q}"))?;
            let il2cpp = loaders::unity_kind(&g.dir).context("não é um jogo Unity de Windows")?;
            let loader = match which.to_lowercase().as_str() {
                "melonloader" => games::Loader::MelonLoader,
                "bepinex" => games::Loader::BepInEx,
                _ => bail!("loader: melonloader ou bepinex"),
            };
            let file = loaders::install(&g.dir, loader, il2cpp, |_, _| {})?;
            println!("{file} instalado em {}", g.dir.display());
        }
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0].starts_with("nxm://") {
        gui::prefer_x11();
    }
    let result = match args.as_slice() {
        [] => gui::run(None),
        // Clique em "Mod Manager Download": repassa para a janela aberta, ou abre uma.
        [url] if url.starts_with("nxm://") => {
            let shown = nexus::NxmLink::parse(url).map(|l| l.redacted()).unwrap_or_else(|_| "nxm:// inválido".into());
            log::line(&format!("aberto pelo navegador com {shown}"));
            if ipc::send(url) { Ok(()) } else { gui::run(Some(url.clone())) }
        }
        _ => run(&args),
    };
    if let Err(e) = result {
        eprintln!("erro: {e:#}");
        std::process::exit(1);
    }
}
