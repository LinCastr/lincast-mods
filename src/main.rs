// First, so `tr!` is visible in every module below.
#[macro_use]
mod i18n;
mod browse;
mod config;
mod deps;
mod desktop;
mod games;
mod gui;
mod install;
mod ipc;
mod launchers;
mod loaders;
mod log;
mod meta;
mod mods;
mod nexus;
mod records;
mod steam;
mod theme;
mod thunderstore;
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
  linex-mods search <game> nexus|ts [words]
                                  search mods on the Nexus or Thunderstore
  linex-mods deps <game>          list missing dependencies (SMAPI)
  linex-mods others               list Heroic and Lutris games (detection only for now)
  linex-mods updates <game>       check the game's Nexus mods for new versions
  linex-mods ts install <game> <package>
                                  install a Thunderstore package and its dependencies
                                  (Owner-Name[-1.2.3], a thunderstore.io page or ror2mm:// link)

<game> is the Steam AppID or part of the name.";

fn game(q: &str) -> Result<games::Game> {
    games::find(q).with_context(|| format!("game not found (or it has no mod loader): {q}"))
}

/// Without Steam every list is empty; say why instead of printing nothing.
fn require_steam() -> Result<()> {
    if steam::steam_roots().is_empty() {
        bail!("{}", tr!("Steam not found (looked in ~/.local/share/Steam, ~/.steam and the Flatpak folder)", "Steam não encontrada (procurei em ~/.local/share/Steam, ~/.steam e na pasta do Flatpak)"));
    }
    Ok(())
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
        ("help" | "-h" | "--help", _) => println!("{USAGE}"),
        ("games", []) => {
            require_steam()?;
            let list = games::moddable_games();
            if list.is_empty() {
                println!("{}", tr!("No Steam game with a mod loader. `linex-mods unity` lists Unity games that can get one.", "Nenhum jogo da Steam com loader de mods. `linex-mods unity` lista os jogos Unity que podem receber um."));
            }
            for g in list {
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
            let list = mods::list(&g);
            if list.is_empty() {
                println!("{}", tr!("No mods installed.", "Nenhum mod instalado."));
            }
            for m in list {
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
            require_steam()?;
            let list = loaders::unity_games_without_loader();
            if list.is_empty() {
                println!("{}", tr!("No Unity game without a loader.", "Nenhum jogo Unity sem loader."));
            }
            for u in list {
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
        ("updates", [q]) => updates(&game(q)?)?,
        ("search", [q, site, words @ ..]) => {
            let g = game(q)?;
            let query = words.join(" ");
            let installed = mods::list(&g);
            let list = match *site {
                "nexus" => browse::nexus_search(&g.nexus_domain(), &query, browse::Sort::Popular, false)?,
                "ts" | "thunderstore" => {
                    let c = thunderstore::community_for(g.steam.appid).context(tr!("this game is not on Thunderstore", "este jogo não está na Thunderstore"))?;
                    browse::ts_search(&c, &query, browse::Sort::Popular, false)?
                }
                _ => bail!("search: nexus or ts"),
            };
            for l in list.iter().take(15) {
                let state = match browse::match_installed(l, &installed) {
                    browse::Match::Installed { newer: true } => " [update]".to_string(),
                    browse::Match::Installed { .. } => " [installed]".to_string(),
                    browse::Match::Similar { name, from } => format!(" [similar: {name} ({from:?})]"),
                    browse::Match::None => String::new(),
                };
                println!("{:>9}  {} {} — {}{state}", l.downloads, l.name, l.version, l.author);
            }
        }
        ("deps", [q]) => {
            let g = game(q)?;
            let mut list: Vec<_> = deps::smapi_missing(&g).into_iter().collect();
            list.sort_by(|a, b| a.0.cmp(&b.0));
            if list.is_empty() {
                println!("{}", tr!("No missing dependencies.", "Nenhuma dependência faltando."));
            }
            for (m, missing) in list {
                let names: Vec<String> = missing
                    .iter()
                    .map(|d| match d.nexus_id {
                        Some(id) => format!("{} ({})", d.name, nexus::mod_page(&g.nexus_domain(), id)),
                        None => d.name.clone(),
                    })
                    .collect();
                println!("{m}: {}", names.join(", "));
            }
        }
        ("others", []) => {
            let list = launchers::installed_games();
            if list.is_empty() {
                println!("{}", tr!("No Heroic or Lutris games found.", "Nenhum jogo do Heroic ou do Lutris encontrado."));
            }
            for g in list {
                let kind = match (games::detect_loader(&g.dir), loaders::unity_kind(&g.dir)) {
                    (Some(l), _) => l.label().to_string(),
                    (None, Some(true)) => "Unity IL2CPP".into(),
                    (None, Some(false)) => "Unity Mono".into(),
                    (None, None) => "-".into(),
                };
                println!("{:<7} {}  [{kind}]  {}", g.launcher, g.name, g.dir.display());
            }
        }
        ("ts" | "thunderstore", ["install", q, pkg]) => {
            let g = game(q)?;
            let r = thunderstore::PackageRef::parse(pkg)?;
            let recs = thunderstore::install(&g, &r, |step| println!("{}", tr!("installing {step}…", "instalando {step}…")))?;
            if recs.is_empty() {
                println!("{}", tr!("Nothing to install: only loader packs.", "Nada para instalar: só pacotes do loader."));
            }
            for rec in recs {
                println!("{}:", rec.name);
                for n in rec.files {
                    println!("  → {n}");
                }
                if let Some(w) = rec.warning {
                    println!("  warning: {w}");
                }
            }
        }
        _ => bail!("{}\n\n{USAGE}", tr!("unknown command or wrong arguments", "comando desconhecido ou argumentos errados")),
    }
    Ok(())
}

/// Nexus versions of the game's mods (cached for 6 h, like the window) against the
/// installed ones.
fn updates(g: &games::Game) -> Result<()> {
    let key = nexus::load_key().context(tr!("no Nexus API key: add it in the window's Settings", "sem chave da API do Nexus: coloque em Configurações, na janela"))?;
    let list: Vec<_> = mods::list(g).into_iter().filter(|m| m.nexus_key.is_some()).collect();
    if list.is_empty() {
        println!("{}", tr!("None of these mods has a Nexus link to check.", "Nenhum destes mods tem link do Nexus para conferir."));
        return Ok(());
    }
    let mut info = meta::load_info();
    let mut newer = 0;
    for m in &list {
        let Some((domain, id)) = m.nexus_key.clone() else { continue };
        let k = meta::info_key(&domain, id);
        let i = match info.get(&k).filter(|i| meta::is_fresh(i)).cloned() {
            Some(i) => i,
            None => match nexus::mod_info(&domain, id, &key) {
                Ok(i) => {
                    info.insert(k, i.clone());
                    i
                }
                Err(e) => {
                    eprintln!("{}: {e:#}", m.name);
                    continue;
                }
            },
        };
        if let (Some(remote), Some(local)) = (&i.version, &m.version)
            && nexus::is_newer(remote, local) {
            newer += 1;
            println!("{}  {local} → {remote}   {}", m.name, nexus::mod_page(&domain, id));
        }
    }
    meta::save_info(&info);
    if newer == 0 {
        let total = list.len();
        println!("{}", tr!("All {total} mods linked to the Nexus are up to date.", "Os {total} mods ligados ao Nexus estão atualizados."));
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_link = |a: &str| a.starts_with("nxm://") || a.starts_with("ror2mm://");
    if args.is_empty() || is_link(&args[0]) {
        gui::prefer_x11();
    } else {
        // Error texts from the core follow the language too.
        i18n::apply_without_ui(&config::load().lang);
    }
    let result = match args.as_slice() {
        [] => gui::run(None),
        // "Mod Manager Download" click: hand the link to the open window, or open one.
        [url] if is_link(url) => {
            let shown = match url.starts_with("nxm://") {
                true => nexus::NxmLink::parse(url).map(|l| l.redacted()).unwrap_or_else(|_| "invalid nxm://".into()),
                false => thunderstore::PackageRef::parse(url).map(|r| format!("ror2mm {}", r.full_name())).unwrap_or_else(|_| "invalid ror2mm://".into()),
            };
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
