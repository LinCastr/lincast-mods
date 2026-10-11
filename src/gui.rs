//! Janela principal.

use crate::games::{self, Game};
use crate::{install, mods, steam};
use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

slint::include_modules!();

struct State {
    games: Vec<Game>,
    current: Option<usize>,
}

impl State {
    fn game(&self) -> Option<&Game> {
        self.current.and_then(|i| self.games.get(i))
    }
}

/// O Proton só carrega a DLL do loader se WINEDLLOVERRIDES mandar; vale tanto a
/// variável da sessão (o rice hollow-wired define uma) quanto a opção de inicialização.
fn proton_override_missing(g: &Game) -> Option<String> {
    let dll = g.loader.proxy_dll()?;
    if g.installed_loader.is_none() || !g.steam.uses_proton() {
        return None;
    }
    let has = |s: &str| s.contains("WINEDLLOVERRIDES") && s.contains(dll);
    let env = std::env::var("WINEDLLOVERRIDES").unwrap_or_default();
    if env.contains(dll) || steam::launch_options(g.steam.appid).iter().any(|o| has(o)) {
        return None;
    }
    Some(format!("WINEDLLOVERRIDES=\"{dll}=n,b\" %command%"))
}

fn status_of(g: &Game) -> (String, bool) {
    if g.installed_loader.is_none() {
        return (format!("{} não está instalado: os mods não vão carregar.", g.loader.label()), true);
    }
    if let Some(opt) = proton_override_missing(g) {
        return (
            format!("O Proton vai ignorar o {}. Nas opções de inicialização do jogo no Steam, use: {opt}", g.loader.label()),
            true,
        );
    }
    let proton = if g.steam.uses_proton() { " · Proton" } else { "" };
    (format!("{}{proton}", g.loader.label()), false)
}

fn load_image(p: Option<PathBuf>) -> Image {
    p.and_then(|p| Image::load_from_path(&p).ok()).unwrap_or_default()
}

fn refresh_mods(ui: &AppWindow, state: &State) {
    let Some(g) = state.game() else {
        ui.set_mods(ModelRc::default());
        return;
    };
    let all = mods::list(g);
    let on = all.iter().filter(|m| m.enabled).count();
    let q = ui.get_filter().to_lowercase();
    let items: Vec<ModItem> = all
        .into_iter()
        .filter(|m| q.is_empty() || m.name.to_lowercase().contains(&q) || m.id.to_lowercase().contains(&q))
        .map(|m| {
            let mut detail = Vec::new();
            if let Some(a) = m.author {
                detail.push(a);
            }
            if let Some(v) = m.version {
                detail.push(format!("v{v}"));
            }
            ModItem {
                id: m.id.into(),
                name: m.name.into(),
                detail: detail.join(" · ").into(),
                enabled: m.enabled,
            }
        })
        .collect();
    let total = ui_total(&items, on, q.is_empty());
    ui.set_counts(total.into());
    ui.set_mods(ModelRc::new(VecModel::from(items)));
}

fn ui_total(items: &[ModItem], on: usize, unfiltered: bool) -> String {
    if unfiltered {
        let n = items.len();
        match n {
            0 => String::new(),
            1 => format!("1 mod · {on} ligado"),
            _ => format!("{n} mods · {on} ligados"),
        }
    } else {
        format!("{} encontrados", items.len())
    }
}

fn select(ui: &AppWindow, state: &mut State, i: usize) {
    state.current = Some(i);
    let Some(g) = state.game() else { return };
    ui.set_game_name(g.steam.name.clone().into());
    let (status, warn) = status_of(g);
    ui.set_game_status(status.into());
    ui.set_game_warning(warn);
    ui.set_hero(load_image(steam::art(g.steam.appid, "library_hero_blur.jpg")));
    ui.set_logo(load_image(steam::art(g.steam.appid, "logo.png")));
    ui.set_filter(SharedString::new());
    ui.set_message(SharedString::new());
    refresh_mods(ui, state);
}

fn open(target: &str) {
    if let Err(e) = Command::new("xdg-open").arg(target).spawn() {
        eprintln!("xdg-open {target}: {e}");
    }
}

pub fn run() -> anyhow::Result<()> {
    let ui = AppWindow::new()?;
    // Só funciona depois da plataforma existir (o AppWindow::new a cria).
    let _ = slint::set_xdg_app_id("linex-mods");
    let state = Rc::new(RefCell::new(State { games: games::moddable_games(), current: None }));

    let items: Vec<GameItem> = state
        .borrow()
        .games
        .iter()
        .map(|g| GameItem {
            appid: g.steam.appid as i32,
            name: g.steam.name.clone().into(),
            loader: match g.installed_loader {
                Some(l) => l.label().into(),
                None => format!("{} não instalado", g.loader.label()).into(),
            },
        })
        .collect();
    ui.set_games(ModelRc::new(VecModel::from(items)));

    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_select_game(move |i| {
            if let Some(ui) = weak.upgrade() {
                select(&ui, &mut state.borrow_mut(), i as usize);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_filter_changed(move |_| {
            if let Some(ui) = weak.upgrade() {
                refresh_mods(&ui, &state.borrow());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_toggle_mod(move |id, on| {
            let Some(ui) = weak.upgrade() else { return };
            let st = state.borrow();
            let Some(g) = st.game() else { return };
            match mods::set_enabled(g, &id, on) {
                Ok(()) => ui.set_message(SharedString::new()),
                Err(e) => {
                    ui.set_message(format!("Não deu para {} {id}: {e:#}", if on { "ligar" } else { "desligar" }).into());
                    ui.set_message_error(true);
                }
            }
            refresh_mods(&ui, &st);
        });
    }
    {
        let state = state.clone();
        ui.on_play(move || {
            if let Some(g) = state.borrow().game() {
                open(&format!("steam://rungameid/{}", g.steam.appid));
            }
        });
    }
    {
        let state = state.clone();
        ui.on_open_folder(move || {
            if let Some(g) = state.borrow().game() {
                let dir = g.mods_dir();
                let _ = std::fs::create_dir_all(&dir);
                open(&dir.to_string_lossy());
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_add_mod(move || {
            let Some(ui) = weak.upgrade() else { return };
            let Some(game) = state.borrow().game().cloned() else { return };
            ui.set_busy(true);
            let weak = weak.clone();
            // O seletor de arquivos e a extração rodam fora da thread da janela.
            std::thread::spawn(move || {
                let files = rfd::FileDialog::new()
                    .set_title("Escolha os arquivos do mod")
                    .add_filter("Arquivos de mod", &["zip", "7z", "rar"])
                    .pick_files()
                    .unwrap_or_default();
                let mut ok = Vec::new();
                let mut errors = Vec::new();
                for f in &files {
                    let name = install::clean_stem(f);
                    match install::install_archive(&game, f) {
                        Ok(_) => ok.push(name),
                        Err(e) => errors.push(format!("{name}: {e:#}")),
                    }
                }
                let (msg, is_err) = if !errors.is_empty() {
                    (format!("Falhou — {}", errors.join(" | ")), true)
                } else if ok.len() == 1 {
                    (format!("Instalado: {}", ok[0]), false)
                } else if !ok.is_empty() {
                    (format!("{} mods instalados", ok.len()), false)
                } else {
                    (String::new(), false)
                };
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_busy(false);
                    ui.set_message(msg.into());
                    ui.set_message_error(is_err);
                    ui.invoke_filter_changed(ui.get_filter());
                });
            });
        });
    }

    if !state.borrow().games.is_empty() {
        ui.set_current(0);
        select(&ui, &mut state.borrow_mut(), 0);
    }
    ui.run()?;
    Ok(())
}
