//! Janela principal.

use crate::games::{self, Game};
use crate::{config, desktop, install, ipc, log, mods, nexus, steam, theme};
use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

slint::include_modules!();

struct State {
    games: Vec<Game>,
    current: Option<usize>,
    /// Versões novas no Nexus: (appid, mod_id) → versão. Checado uma vez por jogo.
    updates: std::collections::HashMap<(u32, u64), String>,
    checked: std::collections::HashSet<u32>,
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

/// Linha de estado do jogo, se é aviso, e a opção de inicialização a copiar (se faltar).
fn status_of(g: &Game) -> (String, bool, String) {
    if g.installed_loader.is_none() {
        return (format!("{} não está instalado: os mods não vão carregar.", g.loader.label()), true, String::new());
    }
    if let Some(opt) = proton_override_missing(g) {
        return (
            format!("O Proton vai ignorar o {}. Copie a opção e cole em Steam → Propriedades → Opções de inicialização.", g.loader.label()),
            true,
            opt,
        );
    }
    let proton = if g.steam.uses_proton() { " · Proton" } else { "" };
    (format!("{}{proton}", g.loader.label()), false, String::new())
}

/// Copia para a área de transferência (wl-copy no Wayland, xclip/xsel no X11).
fn copy_text(text: &str) -> bool {
    use std::io::Write;
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let cmds: &[(&str, &[&str])] = if wayland {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    } else {
        &[("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])]
    };
    for (cmd, args) in cmds {
        let child = Command::new(cmd).args(*args).stdin(std::process::Stdio::piped()).spawn();
        if let Ok(mut c) = child {
            if let Some(mut stdin) = c.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if c.wait().is_ok_and(|s| s.success()) {
                return true;
            }
        }
    }
    false
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
    let total = all.len();
    let on = all.iter().filter(|m| m.enabled).count();
    ui.set_show_author(all.iter().any(|m| m.author.is_some()));
    let q = ui.get_filter().to_lowercase();
    let mode = ui.get_filter_mode();
    let items: Vec<ModItem> = all
        .into_iter()
        .filter(|m| q.is_empty() || m.name.to_lowercase().contains(&q) || m.id.to_lowercase().contains(&q))
        .filter(|m| match mode {
            1 => m.enabled,
            2 => !m.enabled,
            _ => true,
        })
        .map(|m| {
            let update = m
                .nexus
                .as_ref()
                .and_then(|n| state.updates.get(&(g.steam.appid, n.mod_id)))
                .cloned()
                .unwrap_or_default();
            ModItem {
                page: m.nexus.as_ref().map(|n| nexus::mod_page(&n.domain, n.mod_id)).unwrap_or_default().into(),
                id: m.id.into(),
                name: m.name.into(),
                author: m.author.unwrap_or_default().into(),
                version: m.version.unwrap_or_else(|| "—".into()).into(),
                source: if m.from_nexus { "Nexus".into() } else { SharedString::new() },
                update: update.into(),
                enabled: m.enabled,
            }
        })
        .collect();
    let counts = if !q.is_empty() || mode != 0 {
        format!("{} de {total}", items.len())
    } else {
        match total {
            0 => String::new(),
            1 => format!("1 mod · {on} ligado"),
            _ => format!("{total} mods · {on} ligados"),
        }
    };
    ui.set_counts(counts.into());
    ui.set_mods(ModelRc::new(VecModel::from(items)));
}

fn select(ui: &AppWindow, state: &mut State, i: usize) {
    state.current = Some(i);
    let Some(g) = state.game() else { return };
    ui.set_game_name(g.steam.name.clone().into());
    let (status, warn, fix) = status_of(g);
    ui.set_game_status(status.into());
    ui.set_game_warning(warn);
    ui.set_game_fix(fix.into());
    let appid = g.steam.appid;
    ui.set_hero(load_image(steam::art(appid, "library_hero.jpg").or_else(|| steam::art(appid, "library_hero_blur.jpg"))));
    ui.set_logo(load_image(steam::art(g.steam.appid, "logo.png")));
    ui.set_filter(SharedString::new());
    ui.set_filter_mode(0);
    ui.set_message(SharedString::new());
    refresh_mods(ui, state);
    check_updates(ui, state, appid);
    let mut cfg = config::load();
    if cfg.last_game != Some(appid) {
        cfg.last_game = Some(appid);
        config::save(&cfg);
    }
}

/// Pergunta ao Nexus a versão atual dos mods baixados por lá (uma vez por jogo
/// por sessão, fora da thread da janela). Sem chave, não faz nada.
fn check_updates(ui: &AppWindow, state: &mut State, appid: u32) {
    if !state.checked.insert(appid) {
        return;
    }
    let sources: Vec<_> = crate::records::load(appid).mods.into_iter().filter_map(|r| r.nexus).collect();
    if sources.is_empty() {
        return;
    }
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(key) = nexus::load_key() else { return };
        let mut found = serde_json::Map::new();
        for s in sources {
            let (Some(local), Ok(Some(remote))) = (s.version.as_deref(), nexus::mod_version(&s.domain, s.mod_id, &key)) else {
                continue;
            };
            if nexus::is_newer(&remote, local) {
                found.insert(s.mod_id.to_string(), remote.into());
            }
        }
        if !found.is_empty() {
            let json = serde_json::Value::Object(found).to_string();
            let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_updates_found(appid as i32, json.into()));
        }
    });
}

fn color(c: theme::Rgb) -> slint::Color {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    slint::Color::from_rgb_u8(b(c.0), b(c.1), b(c.2))
}

/// Aplica o tema escolhido. Sem paleta do wallust, cai no padrão.
fn apply_theme(ui: &AppWindow) {
    let palette = theme::find_palette();
    ui.set_wallust_available(palette.is_some());
    ui.set_wallust_path(
        palette
            .as_ref()
            .map(|p| p.to_string_lossy().replace(&steam::home().to_string_lossy().to_string(), "~"))
            .unwrap_or_default()
            .into(),
    );
    let use_wallust = config::load().theme == "wallust" && palette.is_some();
    ui.set_theme_mode(if use_wallust { 1 } else { 0 });
    let c = palette
        .filter(|_| use_wallust)
        .and_then(|p| theme::load_palette(&p))
        .map(|p| theme::wallust_colors(&p))
        .unwrap_or_else(theme::default_colors);
    let t = ui.global::<Theme>();
    t.set_bg(color(c.bg));
    t.set_sidebar(color(c.sidebar));
    t.set_surface(color(c.surface));
    t.set_raised(color(c.raised));
    t.set_hover(color(c.hover));
    t.set_line(color(c.line));
    t.set_text(color(c.text));
    t.set_dim(color(c.dim));
    t.set_faint(color(c.faint));
    t.set_accent(color(c.accent));
    t.set_on_accent(color(c.on_accent));
}

fn game_items(games: &[Game]) -> Vec<GameItem> {
    games
        .iter()
        .map(|g| GameItem {
            appid: g.steam.appid as i32,
            name: g.steam.name.clone().into(),
            loader: match g.installed_loader {
                Some(l) => l.label().into(),
                None => format!("{} não instalado", g.loader.label()).into(),
            },
            icon: load_image(steam::icon(g.steam.appid)),
        })
        .collect()
}

fn open(target: &str) {
    if let Err(e) = Command::new("xdg-open").arg(target).spawn() {
        eprintln!("xdg-open {target}: {e}");
    }
}

fn set_msg(ui: &AppWindow, msg: impl Into<SharedString>, err: bool) {
    ui.set_message_sticky(false);
    ui.set_message_error(err);
    ui.set_message(msg.into());
}

/// Mensagem de andamento: fica no canto até a próxima.
fn set_progress(ui: &AppWindow, msg: impl Into<SharedString>) {
    ui.set_message_sticky(true);
    ui.set_message_error(false);
    ui.set_message(msg.into());
}

/// Notificação do sistema, só para o que chega de fora da janela (clique no site):
/// nessa hora o app costuma estar atrás do navegador.
fn notify(summary: &str, body: &str, err: bool) {
    let mut n = notify_rust::Notification::new();
    n.appname("linex-mods").summary(summary).body(body).icon("applications-games");
    if err {
        n.urgency(notify_rust::Urgency::Critical);
    }
    if let Err(e) = n.show() {
        log::line(&format!("notificação falhou: {e}"));
    }
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
    set_progress(ui, "Baixando do Nexus…");
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let appid = game.steam.appid as i32;
        let game_name = game.steam.name.clone();
        let finish = |w: &slint::Weak<AppWindow>, msg: String, err: bool| {
            log::line(&msg);
            if err {
                notify("Mod não instalado", &msg, true);
            } else {
                notify(msg.trim_start_matches("Instalado: "), &format!("Instalado em {game_name}"), false);
            }
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
                let _ = w2.upgrade_in_event_loop(move |ui| set_progress(&ui, txt));
            }
        });
        let dl = match dl {
            Ok(d) => d,
            Err(e) => return finish(&weak, format!("Download falhou: {e:#}"), true),
        };
        let _ = weak.upgrade_in_event_loop(|ui| set_progress(&ui, "Instalando…"));
        match install::install_archive(&game, &dl.path, Some(dl.mod_name.clone()), Some(dl.source)) {
            Ok(rec) => {
                let _ = std::fs::remove_file(&dl.path);
                match rec.warning {
                    Some(w) => finish(&weak, format!("{} instalado, mas {w}", rec.name), true),
                    None => finish(&weak, format!("Instalado: {}", rec.name), false),
                }
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

/// Modo de teste (desenvolvimento): LINEX_SNAPSHOT=arquivo.png salva um print da
/// janela e fecha. LINEX_GAME escolhe o jogo (parte do nome), LINEX_PAGE=1 abre as
/// Configurações, LINEX_MESSAGE mostra um aviso.
fn snapshot_mode(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    let Ok(out) = std::env::var("LINEX_SNAPSHOT") else { return };
    if let Ok(q) = std::env::var("LINEX_GAME") {
        let q = q.to_lowercase();
        let pos = state.borrow().games.iter().position(|g| g.steam.name.to_lowercase().contains(&q));
        if let Some(i) = pos {
            ui.set_current(i as i32);
            select(ui, &mut state.borrow_mut(), i);
        }
    }
    if std::env::var("LINEX_PAGE").as_deref() == Ok("1") {
        ui.set_page(1);
        ui.invoke_show_settings();
    }
    if let Ok(m) = std::env::var("LINEX_MESSAGE") {
        set_progress(ui, m);
    }
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(1800), move || {
        let Some(ui) = weak.upgrade() else { return };
        match ui.window().take_snapshot() {
            Ok(buf) => {
                let f = std::fs::File::create(&out).expect("arquivo do print");
                let mut enc = png::Encoder::new(std::io::BufWriter::new(f), buf.width(), buf.height());
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                let mut w = enc.write_header().expect("png");
                w.write_image_data(buf.as_bytes()).expect("png");
            }
            Err(e) => eprintln!("print falhou: {e}"),
        }
        let _ = slint::quit_event_loop();
    });
}

pub fn run(initial_nxm: Option<String>) -> anyhow::Result<()> {
    desktop::refresh_desktop_file();
    let ui = AppWindow::new()?;
    // Só funciona depois da plataforma existir (o AppWindow::new a cria).
    let app_id = std::env::var("LINEX_APP_ID").unwrap_or_else(|_| "linex-mods".into());
    let _ = slint::set_xdg_app_id(app_id);
    let state = Rc::new(RefCell::new(State {
        games: games::moddable_games(),
        current: None,
        updates: Default::default(),
        checked: Default::default(),
    }));

    apply_theme(&ui);
    ui.set_games(ModelRc::new(VecModel::from(game_items(&state.borrow().games))));

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
                Ok(()) => {}
                Err(e) => set_msg(&ui, format!("Não deu para {} {id}: {e:#}", if on { "ligar" } else { "desligar" }), true),
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
                Ok(name) => set_msg(&ui, format!("{name} foi para a Lixeira."), false),
                Err(e) => set_msg(&ui, format!("Não deu para remover {id}: {e:#}"), true),
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
                        Ok(rec) => match rec.warning {
                            Some(w) => errors.push(format!("{name} instalado, mas {w}")),
                            None => ok.push(name),
                        },
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
                    set_msg(&ui, msg, is_err);
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
    ui.on_open_url(|u| open(&u));
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_updates_found(move |appid, json| {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            let map: std::collections::HashMap<String, String> = serde_json::from_str(&json).unwrap_or_default();
            let n = map.len();
            for (id, v) in map {
                if let Ok(id) = id.parse() {
                    st.updates.insert((appid as u32, id), v);
                }
            }
            if st.game().is_some_and(|g| g.steam.appid as i32 == appid) {
                refresh_mods(&ui, &st);
                set_msg(&ui, if n == 1 { "1 mod tem versão nova no Nexus.".to_string() } else { format!("{n} mods têm versão nova no Nexus.") }, false);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_copy_fix(move || {
            let Some(ui) = weak.upgrade() else { return };
            if copy_text(&ui.get_game_fix()) {
                set_msg(&ui, "Copiado. Cole nas opções de inicialização do jogo e reabra o jogo.", false);
            } else {
                set_msg(&ui, format!("Não deu para copiar (falta o wl-copy ou xclip). Use: {}", ui.get_game_fix()), true);
            }
        });
    }
    // Atualiza a lista quando a pasta de mods muda por fora (gerenciador de
    // arquivos, outro programa): confere a cada 2 s.
    let watch_timer = slint::Timer::default();
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        let last = RefCell::new(None);
        watch_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(2), move || {
            let st = state.borrow();
            let Some(g) = st.game() else { return };
            let stamp = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let now = (
                g.steam.appid,
                stamp(g.mods_dir()),
                stamp(g.disabled_dir()),
                stamp(crate::records::data_dir().join(format!("{}.json", g.steam.appid))),
            );
            let changed = last.borrow().as_ref().is_some_and(|l| *l != now);
            *last.borrow_mut() = Some(now);
            if changed {
                if let Some(ui) = weak.upgrade() {
                    refresh_mods(&ui, &st);
                }
            }
        });
    }

    // Links que chegarem por outra instância (clique no site com a janela aberta).
    {
        let weak = ui.as_weak();
        ipc::listen(move |msg| {
            if msg.starts_with("nxm://") {
                let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_nxm_received(msg.into()));
            }
        });
    }

    {
        let weak = ui.as_weak();
        ui.on_theme_changed(move |i| {
            let mut cfg = config::load();
            cfg.theme = if i == 1 { "wallust".into() } else { "default".into() };
            config::save(&cfg);
            if let Some(ui) = weak.upgrade() {
                apply_theme(&ui);
            }
        });
    }
    // A paleta muda quando o papel de parede muda: confere a cada 3 s.
    let theme_timer = slint::Timer::default();
    {
        let weak = ui.as_weak();
        let last = RefCell::new(theme::find_palette().and_then(|p| theme::mtime(&p)));
        theme_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(3), move || {
            let now = theme::find_palette().and_then(|p| theme::mtime(&p));
            if *last.borrow() != now {
                *last.borrow_mut() = now;
                if let Some(ui) = weak.upgrade() {
                    apply_theme(&ui);
                }
            }
        });
    }

    // Abre no último jogo usado.
    let start = {
        let st = state.borrow();
        let last = config::load().last_game;
        st.games.iter().position(|g| Some(g.steam.appid) == last).or((!st.games.is_empty()).then_some(0))
    };
    if let Some(i) = start {
        ui.set_current(i as i32);
        select(&ui, &mut state.borrow_mut(), i);
    }
    if let Some(url) = initial_nxm {
        ui.invoke_nxm_received(url.into());
    }
    snapshot_mode(&ui, &state);
    let r = ui.run();
    ipc::cleanup();
    r?;
    Ok(())
}
