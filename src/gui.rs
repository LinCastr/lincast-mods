//! Janela principal.

use crate::games::{self, Game};
use crate::{desktop, install, ipc, log, mods, nexus, steam};
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

fn set_msg(ui: &AppWindow, msg: impl Into<SharedString>, err: bool) {
    ui.set_message(msg.into());
    ui.set_message_error(err);
}

/// Link nxm:// recebido (do navegador ou de outra instância): baixa e instala
/// fora da thread da janela, mostrando o progresso na linha de mensagem.
fn handle_nxm(ui: &AppWindow, state: &mut State, url: &str) {
    let link = match nexus::NxmLink::parse(url) {
        Ok(l) => l,
        Err(e) => {
            log::line(&format!("link recusado: {e:#}"));
            return set_msg(ui, format!("Link do Nexus: {e:#}"), true);
        }
    };
    log::line(&format!("recebido {}", link.redacted()));
    let Some(i) = state.games.iter().position(|g| g.matches_nexus(&link.domain)) else {
        return set_msg(
            ui,
            format!("O link é de \"{}\", que não está entre os jogos detectados com loader de mods.", link.domain),
            true,
        );
    };
    ui.set_page(0);
    ui.set_current(i as i32);
    select(ui, state, i);
    let game = state.games[i].clone();
    set_msg(ui, "Baixando do Nexus…", false);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let appid = game.steam.appid as i32;
        let finish = |w: &slint::Weak<AppWindow>, msg: String, err: bool| {
            log::line(&msg);
            let _ = w.upgrade_in_event_loop(move |ui| ui.invoke_nxm_finished(appid, msg.into(), err));
        };
        let Some(key) = nexus::load_key() else {
            return finish(&weak, "Falta a chave da API do Nexus: veja em Configurações.".into(), true);
        };
        let mut last = u64::MAX;
        let w2 = weak.clone();
        let dl = nexus::download(&link, &key, |done, total| {
            let step = if total > 0 { done * 100 / total } else { done / (1024 * 1024) };
            if step != last {
                last = step;
                let txt = if total > 0 {
                    format!("Baixando do Nexus… {step}%")
                } else {
                    format!("Baixando do Nexus… {step} MB")
                };
                let _ = w2.upgrade_in_event_loop(move |ui| set_msg(&ui, txt, false));
            }
        });
        let dl = match dl {
            Ok(d) => d,
            Err(e) => return finish(&weak, format!("Download falhou: {e:#}"), true),
        };
        let _ = weak.upgrade_in_event_loop(|ui| set_msg(&ui, "Instalando…", false));
        match install::install_archive(&game, &dl.path, Some(dl.mod_name.clone()), Some(dl.source)) {
            Ok(rec) => {
                let _ = std::fs::remove_file(&dl.path);
                finish(&weak, format!("Instalado: {}", rec.name), false)
            }
            Err(e) => finish(
                &weak,
                format!("{}: {e:#} (o arquivo ficou em {})", dl.mod_name, dl.path.display()),
                true,
            ),
        }
    });
}

/// Estado da chave do Nexus, lido fora da thread da janela (o chaveiro pode demorar).
fn refresh_nexus_status(ui: &AppWindow) {
    ui.set_handler_ok(desktop::is_nxm_handler());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let (txt, ok) = match nexus::load_key() {
            None => ("Nenhuma chave salva. Sem ela o botão do site não consegue baixar.".to_string(), false),
            Some(k) => match nexus::validate(&k) {
                Ok((name, premium)) => (connected(&name, premium), true),
                Err(e) => (format!("Há uma chave salva, mas não funcionou: {e:#}"), false),
            },
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_nexus_status(txt.into());
            ui.set_nexus_ok(ok);
        });
    });
}

fn connected(name: &str, premium: bool) -> String {
    format!("Conectado como {name} · conta {}", if premium { "premium" } else { "gratuita" })
}

pub fn run(initial_nxm: Option<String>) -> anyhow::Result<()> {
    desktop::refresh_desktop_file();
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
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_remove_mod(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let st = state.borrow();
            let Some(g) = st.game() else { return };
            match mods::uninstall(g, &id) {
                Ok(name) => {
                    ui.set_message(format!("{name} foi para a Lixeira.").into());
                    ui.set_message_error(false);
                }
                Err(e) => {
                    ui.set_message(format!("Não deu para remover {id}: {e:#}").into());
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
                    match install::install_archive(&game, f, None, None) {
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

    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.on_nxm_received(move |url| {
            if let Some(ui) = weak.upgrade() {
                handle_nxm(&ui, &mut state.borrow_mut(), &url);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_nxm_finished(move |appid, msg, err| {
            let Some(ui) = weak.upgrade() else { return };
            set_msg(&ui, msg, err);
            let st = state.borrow();
            if st.game().is_some_and(|g| g.steam.appid as i32 == appid) {
                refresh_mods(&ui, &st);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_show_settings(move || {
            if let Some(ui) = weak.upgrade() {
                set_msg(&ui, "", false);
                refresh_nexus_status(&ui);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_save_key(move |key| {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_key_busy(true);
            let weak = weak.clone();
            let key = key.to_string();
            std::thread::spawn(move || {
                // Só guarda chave que o Nexus aceita.
                let (txt, ok) = match nexus::validate(&key) {
                    Ok((name, premium)) => match nexus::save_key(&key) {
                        Ok(place) => (format!("{}. Chave guardada {place}.", connected(&name, premium)), true),
                        Err(e) => (format!("A chave funciona, mas não deu para guardar: {e:#}"), false),
                    },
                    Err(e) => (format!("Chave não aceita: {e:#}"), false),
                };
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_key_busy(false);
                    ui.set_nexus_status(txt.into());
                    ui.set_nexus_ok(ok);
                });
            });
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_clear_key(move || {
            nexus::clear_key();
            if let Some(ui) = weak.upgrade() {
                ui.set_nexus_ok(false);
                ui.set_nexus_status("Chave apagada.".into());
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_register_handler(move || {
            let Some(ui) = weak.upgrade() else { return };
            match desktop::register_nxm_handler() {
                Ok(()) => set_msg(&ui, "Pronto: o botão \"Mod Manager Download\" agora abre aqui.", false),
                Err(e) => set_msg(&ui, format!("Não deu para registrar: {e:#}"), true),
            }
            ui.set_handler_ok(desktop::is_nxm_handler());
        });
    }
    ui.on_open_key_page(|| open(nexus::API_KEYS_PAGE));

    // Links que chegarem por outra instância (clique no site com a janela aberta).
    {
        let weak = ui.as_weak();
        ipc::listen(move |msg| {
            if msg.starts_with("nxm://") {
                let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_nxm_received(msg.into()));
            }
        });
    }

    if !state.borrow().games.is_empty() {
        ui.set_current(0);
        select(&ui, &mut state.borrow_mut(), 0);
    }
    if let Some(url) = initial_nxm {
        ui.invoke_nxm_received(url.into());
    }
    let r = ui.run();
    ipc::cleanup();
    r?;
    Ok(())
}
