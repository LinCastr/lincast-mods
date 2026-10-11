mod games;
mod gui;
mod install;
mod mods;
mod steam;
mod vdf;

use anyhow::{Context, Result, bail};
use std::path::Path;

const USAGE: &str = "uso:
  linex-mods jogos
  linex-mods mods <jogo>
  linex-mods instalar <jogo> <arquivo.zip|.7z|.rar>...
  linex-mods ligar <jogo> <mod>
  linex-mods desligar <jogo> <mod>

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
                let names = install::install_archive(&g, Path::new(f)).with_context(|| format!("instalando {f}"))?;
                println!("{f}:");
                for n in names {
                    println!("  → {n}");
                }
            }
        }
        ["ligar", q, id] => mods::set_enabled(&game(q)?, id, true)?,
        ["desligar", q, id] => mods::set_enabled(&game(q)?, id, false)?,
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.is_empty() { gui::run() } else { run(&args) };
    if let Err(e) = result {
        eprintln!("erro: {e:#}");
        std::process::exit(1);
    }
}
