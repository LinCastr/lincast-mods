//! Janela principal.

use crate::games::{self, Game};
use crate::{config, desktop, install, ipc, loaders, log, mods, nexus, steam, theme};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
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
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some() || WAYLAND_DISPLAY.get().is_some();
    let cmds: &[(&str, &[&str])] = if wayland {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    } else {
        &[("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])]
    };
    for (cmd, args) in cmds {
        let mut command = Command::new(cmd);
        command.args(*args).stdin(std::process::Stdio::piped());
        if let Some(w) = WAYLAND_DISPLAY.get() {
            command.env("WAYLAND_DISPLAY", w);
        }
        let child = command.spawn();
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
    ui.set_game_missing_loader(if g.installed_loader.is_none() { g.loader.label().into() } else { SharedString::new() });
    let appid = g.steam.appid;
    ui.set_hero(load_image(steam::art(appid, "library_hero.jpg").or_else(|| steam::art(appid, "library_hero_blur.jpg"))));
    ui.set_logo(load_image(steam::art(g.steam.appid, "logo.png")));
    ui.set_filter(SharedString::new());
    ui.set_filter_mode(0);
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

/// Avisos do app: pilha no canto (cada um some em 2,5 s) e histórico no sino.
struct Notes {
    toasts: Rc<VecModel<Notice>>,
    history: Rc<VecModel<Notice>>,
    /// Hora (segundos) de cada aviso do histórico, para o "há 3 min".
    stamps: Vec<u64>,
    next: i32,
    unseen: i32,
}

thread_local! {
    static NOTES: RefCell<Option<Notes>> = const { RefCell::new(None) };
}

const TOAST_MS: u64 = 2500;
const HISTORY_MAX: usize = 50;

fn notes_init(ui: &AppWindow) {
    let n = Notes {
        toasts: Rc::new(VecModel::default()),
        history: Rc::new(VecModel::default()),
        stamps: Vec::new(),
        next: 1,
        unseen: 0,
    };
    ui.set_toasts(ModelRc::from(n.toasts.clone()));
    ui.set_notices(ModelRc::from(n.history.clone()));
    NOTES.with(|c| *c.borrow_mut() = Some(n));
}

fn ago(t: u64) -> String {
    let d = crate::records::now().saturating_sub(t);
    match d {
        0..=59 => "agora".into(),
        60..=3599 => format!("há {} min", d / 60),
        3600..=86399 => format!("há {} h", d / 3600),
        _ => format!("há {} dias", d / 86400),
    }
}

fn remove_toast(id: i32) {
    NOTES.with(|c| {
        if let Some(n) = c.borrow().as_ref() {
            if let Some(i) = (0..n.toasts.row_count()).find(|&i| n.toasts.row_data(i).is_some_and(|t| t.id == id)) {
                n.toasts.remove(i);
            }
        }
    });
}

/// Aviso: entra na pilha (some em 2,5 s) e no histórico. Mensagem vazia só limpa o
/// andamento.
fn set_msg(ui: &AppWindow, msg: impl Into<SharedString>, err: bool) {
    let msg: SharedString = msg.into();
    ui.set_progress(SharedString::new());
    if msg.is_empty() {
        return;
    }
    let open = ui.get_panel_open();
    let id = NOTES.with(|c| {
        let mut c = c.borrow_mut();
        let n = c.as_mut()?;
        let id = n.next;
        n.next += 1;
        let notice = Notice { id, text: msg.clone(), error: err, when: "agora".into() };
        n.toasts.push(notice.clone());
        n.history.insert(0, notice);
        n.stamps.insert(0, crate::records::now());
        if n.history.row_count() > HISTORY_MAX {
            n.history.remove(HISTORY_MAX);
            n.stamps.truncate(HISTORY_MAX);
        }
        if !open {
            n.unseen += 1;
        }
        Some((id, n.unseen))
    });
    if let Some((id, unseen)) = id {
        ui.set_unseen(unseen);
        slint::Timer::single_shot(std::time::Duration::from_millis(TOAST_MS), move || remove_toast(id));
    }
}

/// Andamento (download, instalação): fica no topo da pilha até o próximo aviso.
fn set_progress(ui: &AppWindow, msg: impl Into<SharedString>) {
    ui.set_progress(msg.into());
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
    match std::env::var("LINEX_PAGE").as_deref() {
        Ok("1") => {
            ui.set_page(1);
            ui.invoke_show_settings();
        }
        Ok("2") => {
            ui.set_page(2);
            ui.invoke_show_add_game();
        }
        _ => {}
    }
    if let Ok(m) = std::env::var("LINEX_MESSAGE") {
        for (i, part) in m.split('|').enumerate() {
            set_msg(ui, part.to_string(), i == 1);
        }
    }
    if std::env::var("LINEX_PANEL").as_deref() == Ok("1") {
        ui.set_panel_open(true);
        ui.invoke_open_panel();
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

/// WAYLAND_DISPLAY original, quando o app abre pelo XWayland (ver `prefer_x11`).
static WAYLAND_DISPLAY: std::sync::OnceLock<std::ffi::OsString> = std::sync::OnceLock::new();

/// Arrastar arquivos para a janela só funciona no X11: o winit (camada de janelas
/// do Slint) ainda não implementa arrastar e soltar no Wayland. Com o XWayland
/// disponível, o app abre por ele (LINEX_WAYLAND=1 força o Wayland). Tem que rodar
/// antes de qualquer thread.
pub fn prefer_x11() {
    if std::env::var_os("LINEX_WAYLAND").is_some() || std::env::var_os("DISPLAY").is_none() {
        return;
    }
    if let Some(w) = std::env::var_os("WAYLAND_DISPLAY") {
        let _ = WAYLAND_DISPLAY.set(w);
        // Sem Xft.dpi, o winit calcula a escala pelo tamanho físico do monitor e a
        // janela abria uns 33% maior que no Wayland. Com Xft.dpi (o KDE define quando
        // há escala), vale o que o sistema escolheu.
        let xft = Command::new("xrdb")
            .arg("-query")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("Xft.dpi"));
        // SAFETY: chamado no início do main, com uma thread só.
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
            if !xft && std::env::var_os("WINIT_X11_SCALE_FACTOR").is_none() {
                std::env::set_var("WINIT_X11_SCALE_FACTOR", "1");
            }
        }
    }
}

/// Uma instalação por vez: arrastar vários arquivos dispara várias threads, e o
/// registro de cada jogo é lido e regravado inteiro.
static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn is_archive(p: &std::path::Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "zip" | "7z" | "rar"))
}

/// Instala arquivos soltos na janela no jogo aberto.
fn install_files(ui: &AppWindow, state: &State, files: Vec<PathBuf>) {
    if ui.get_page() != 0 {
        return set_msg(ui, "Abra um jogo antes de soltar o arquivo.", true);
    }
    let Some(game) = state.game().cloned() else {
        return set_msg(ui, "Abra um jogo antes de soltar o arquivo.", true);
    };
    let (ok, bad): (Vec<_>, Vec<_>) = files.into_iter().partition(|f| is_archive(f));
    for b in bad {
        set_msg(ui, format!("{}: não é .zip, .7z nem .rar.", b.file_name().unwrap_or_default().to_string_lossy()), true);
    }
    if ok.is_empty() {
        return;
    }
    set_progress(ui, format!("Instalando em {}…", game.steam.name));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let _lock = INSTALL_LOCK.lock();
        let mut results = Vec::new();
        for f in &ok {
            let name = install::clean_stem(f);
            results.push(match install::install_archive(&game, f, None, None) {
                Ok(rec) => match rec.warning {
                    Some(w) => (format!("{} instalado, mas {w}", rec.name), true),
                    None => (format!("Instalado: {}", rec.name), false),
                },
                Err(e) => (format!("{name}: {e:#}"), true),
            });
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_progress(SharedString::new());
            for (m, e) in results {
                set_msg(&ui, m, e);
            }
            ui.invoke_filter_changed(ui.get_filter());
        });
    });
}

fn unity_items() -> Vec<UnityItem> {
    loaders::unity_games_without_loader()
        .into_iter()
        .map(|u| UnityItem {
            appid: u.steam.appid as i32,
            name: u.steam.name.clone().into(),
            kind: if u.il2cpp { "Unity · IL2CPP".into() } else { "Unity · Mono".into() },
            icon: load_image(steam::icon(u.steam.appid)),
        })
        .collect()
}

fn install_loader(ui: &AppWindow, state: &State, appid: i32, which: &str) {
    let loader = match which.to_lowercase().as_str() {
        "melonloader" => games::Loader::MelonLoader,
        "bepinex" => games::Loader::BepInEx,
        "smapi" => {
            open("https://smapi.io");
            return set_msg(ui, "O SMAPI tem instalador próprio: a página dele abriu no navegador.", false);
        }
        _ => return,
    };
    let dir = state
        .games
        .iter()
        .find(|g| g.steam.appid as i32 == appid)
        .map(|g| g.steam.dir.clone())
        .or_else(|| steam::installed_games().into_iter().find(|g| g.appid as i32 == appid).map(|g| g.dir));
    let Some(dir) = dir else { return };
    let Some(il2cpp) = loaders::unity_kind(&dir) else {
        return set_msg(ui, "Este jogo não parece ser um jogo Unity de Windows: o loader não serve para ele.", true);
    };
    ui.set_installing(appid);
    set_progress(ui, format!("Baixando o {}…", loader.label()));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let mut last = u64::MAX;
        let w2 = weak.clone();
        let label = loader.label();
        let r = loaders::install(&dir, loader, il2cpp, |done, total| {
            let step = if total > 0 { done * 100 / total } else { done / (1024 * 1024) };
            if step != last {
                last = step;
                let txt = if total > 0 { format!("Baixando o {label}… {step}%") } else { format!("Baixando o {label}… {step} MB") };
                let _ = w2.upgrade_in_event_loop(move |ui| set_progress(&ui, txt));
            }
        });
        let (msg, err) = match r {
            Ok(file) => {
                log::line(&format!("loader instalado: {file} em {}", dir.display()));
                (format!("{label} instalado. Abra o jogo uma vez para ele terminar de se configurar."), false)
            }
            Err(e) => (format!("Não deu para instalar o {label}: {e:#}"), true),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_loader_finished(appid, msg.into(), err));
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
    notes_init(&ui);
    {
        let weak = ui.as_weak();
        ui.on_open_panel(move || {
            let Some(ui) = weak.upgrade() else { return };
            NOTES.with(|c| {
                if let Some(n) = c.borrow_mut().as_mut() {
                    n.unseen = 0;
                    for (i, t) in n.stamps.iter().enumerate() {
                        if let Some(mut row) = n.history.row_data(i) {
                            row.when = ago(*t).into();
                            n.history.set_row_data(i, row);
                        }
                    }
                }
            });
            ui.set_unseen(0);
        });
    }
    ui.on_clear_notices(|| {
        NOTES.with(|c| {
            if let Some(n) = c.borrow_mut().as_mut() {
                n.history.clear();
                n.stamps.clear();
            }
        });
    });
    ui.on_dismiss_toast(remove_toast);
    {
        let weak = ui.as_weak();
        ui.on_show_add_game(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_unity_games(ModelRc::new(VecModel::from(unity_items())));
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_install_loader(move |appid, which| {
            if let Some(ui) = weak.upgrade() {
                install_loader(&ui, &state.borrow(), appid, &which);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_loader_finished(move |appid, msg, err| {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_installing(-1);
            set_msg(&ui, msg, err);
            if err {
                return;
            }
            // O jogo passa a ter loader: entra (ou se atualiza) na lista e abre.
            let mut st = state.borrow_mut();
            st.games = games::moddable_games();
            ui.set_games(ModelRc::new(VecModel::from(game_items(&st.games))));
            ui.set_unity_games(ModelRc::new(VecModel::from(unity_items())));
            if let Some(i) = st.games.iter().position(|g| g.steam.appid as i32 == appid) {
                ui.set_page(0);
                ui.set_current(i as i32);
                select(&ui, &mut st, i);
            }
        });
    }
    {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
        let (weak, state) = (ui.as_weak(), state.clone());
        // O X11 manda um evento por arquivo; junta os que chegam juntos.
        let pending: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
        ui.window().on_winit_window_event(move |_, ev| {
            let Some(ui) = weak.upgrade() else { return EventResult::Propagate };
            match ev {
                WindowEvent::HoveredFile(_) => ui.set_drop_hover(true),
                WindowEvent::HoveredFileCancelled => ui.set_drop_hover(false),
                WindowEvent::DroppedFile(p) => {
                    ui.set_drop_hover(false);
                    let first = pending.borrow().is_empty();
                    pending.borrow_mut().push(p.clone());
                    if first {
                        let (weak, state, pending) = (weak.clone(), state.clone(), pending.clone());
                        slint::Timer::single_shot(std::time::Duration::from_millis(150), move || {
                            let files = std::mem::take(&mut *pending.borrow_mut());
                            if let Some(ui) = weak.upgrade() {
                                install_files(&ui, &state.borrow(), files);
                            }
                        });
                    }
                }
                _ => return EventResult::Propagate,
            }
            EventResult::Propagate
        });
    }
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
                let _lock = INSTALL_LOCK.lock();
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
