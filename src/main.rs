// First, so `tr!` is visible in every module below.
#[macro_use]
mod i18n;
mod config;
mod desktop;
mod games;
mod gui;
mod install;
mod ipc;
mod loaders;
mod log;
mod meta;
mod mods;
mod nexus;
mod records;
mod steam;
mod theme;
mod vdf;

use anyhow::{Context, Result, bail};
use std::path::Path;

const USAGE: &str = "usage:
  linex-mods                      open the window
  linex-mods games                list games with a mod loader
  linex-mods mods <game>          list a game's mods
  linex-mods install <game> <file.zip|.7z|.rar>...
  linex-mods enable <game> <mod>
  linex-mods disable <game> <mod>
  linex-mods remove <game> <mod>  (sends it to the trash)
  linex-mods unity                list Unity games without a loader
  linex-mods loader <game> melonloader|bepinex

<game> is the Steam AppID or part of the name.";

fn game(q: &str) -> Result<games::Game> {
    games::find(q).with_context(|| format!("game not found (or it has no mod loader): {q}"))
}

fn run(args: &[String]) -> Result<()> {
    // The Portuguese command names from the first versions still work.
    let cmd = match args[0].as_str() {
        "jogos" => "games",
        "instalar" => "install",
        "ligar" => "enable",
        "desligar" => "disable",
        "remover" => "remove",
        other => other,
    };
    let rest: Vec<&str> = args[1..].iter().map(String::as_str).collect();
    match (cmd, rest.as_slice()) {
        ("games", []) => {
            for g in games::moddable_games() {
                let state = match g.installed_loader {
                    Some(l) => l.label().to_string(),
                    None => format!("{} not installed", g.loader.label()),
                };
                let proton = if g.steam.uses_proton() { " · Proton" } else { "" };
                println!("{:>8}  {}  [{state}{proton}]", g.steam.appid, g.steam.name);
            }
        }
        ("mods", [q]) => {
            let g = game(q)?;
            for m in mods::list(&g) {
                let on = if m.enabled { "on " } else { "off" };
                let ver = m.version.map(|v| format!(" {v}")).unwrap_or_default();
                let by = m.author.map(|a| format!(" — {a}")).unwrap_or_default();
                println!("{on}  {}{ver}{by}   ({})", m.name, m.id);
            }
        }
        ("install", [q, files @ ..]) if !files.is_empty() => {
            let g = game(q)?;
            for f in files {
                let rec = install::install_archive(&g, Path::new(f), None, None)
                    .with_context(|| format!("installing {f}"))?;
                println!("{}:", rec.name);
                for n in rec.files {
                    println!("  → {n}");
                }
                if let Some(w) = rec.warning {
                    println!("  warning: {w}");
                }
            }
        }
        ("enable", [q, id]) => mods::set_enabled(&game(q)?, id, true)?,
        ("disable", [q, id]) => mods::set_enabled(&game(q)?, id, false)?,
        ("remove", [q, id]) => println!("{} was moved to the trash", mods::uninstall(&game(q)?, id)?),
        ("unity", []) => {
            for u in loaders::unity_games_without_loader() {
                println!("{:>8}  {}  [{}]", u.steam.appid, u.steam.name, if u.il2cpp { "IL2CPP" } else { "Mono" });
            }
        }
        ("loader", [q, which]) => {
            let q = q.to_lowercase();
            let g = steam::installed_games()
                .into_iter()
                .find(|g| g.appid.to_string() == q || g.name.to_lowercase().contains(&q))
                .with_context(|| format!("game not found: {q}"))?;
            let il2cpp = loaders::unity_kind(&g.dir).context("not a Windows Unity game")?;
            let loader = match which.to_lowercase().as_str() {
                "melonloader" => games::Loader::MelonLoader,
                "bepinex" => games::Loader::BepInEx,
                _ => bail!("loader: melonloader or bepinex"),
            };
            let file = loaders::install(&g.dir, loader, il2cpp, |_, _| {})?;
            println!("{file} installed in {}", g.dir.display());
        }
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0].starts_with("nxm://") {
        gui::prefer_x11();
    } else {
        // Error texts from the core follow the language too.
        i18n::apply_without_ui(&config::load().lang);
    }
    let result = match args.as_slice() {
        [] => gui::run(None),
        // "Mod Manager Download" click: hand the link to the open window, or open one.
        [url] if url.starts_with("nxm://") => {
            let shown = nexus::NxmLink::parse(url).map(|l| l.redacted()).unwrap_or_else(|_| "invalid nxm://".into());
            log::line(&format!("opened by the browser with {shown}"));
            if ipc::send(url) { Ok(()) } else { gui::run(Some(url.clone())) }
        }
        _ => run(&args),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
