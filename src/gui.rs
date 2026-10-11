//! Main window.

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
    /// Newer versions on the Nexus: (appid, mod_id) → version. Checked once per game.
    updates: std::collections::HashMap<(u32, u64), String>,
    checked: std::collections::HashSet<u32>,
    icons: RefCell<std::collections::HashMap<PathBuf, Image>>,
}

impl State {
    fn game(&self) -> Option<&Game> {
        self.current.and_then(|i| self.games.get(i))
    }

    /// Icon image, loaded once (the list is rebuilt on every keystroke of the search).
    fn icon(&self, p: &std::path::Path) -> Image {
        self.icons
            .borrow_mut()
            .entry(p.to_path_buf())
            .or_insert_with(|| Image::load_from_path(p).unwrap_or_default())
            .clone()
    }
}

/// Proton only loads the loader's DLL if WINEDLLOVERRIDES says so; both the session
/// variable (the hollow-wired rice sets one) and the launch option count.
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

/// The game's status line, whether it is a warning, and the launch option to copy (if missing).
fn status_of(g: &Game) -> (String, bool, String) {
    if g.installed_loader.is_none() {
        return (tr!("{} is not installed: mods won't load.", "{} não está instalado: os mods não vão carregar.", g.loader.label()), true, String::new());
    }
    if let Some(opt) = proton_override_missing(g) {
        return (
            tr!("Proton will ignore {}. Copy the option and paste it in Steam → Properties → Launch options.", "O Proton vai ignorar o {}. Copie a opção e cole em Steam → Propriedades → Opções de inicialização.", g.loader.label()),
            true,
            opt,
        );
    }
    let proton = if g.steam.uses_proton() { " · Proton" } else { "" };
    (format!("{}{proton}", g.loader.label()), false, String::new())
}

/// Copies to the clipboard (wl-copy on Wayland, xclip/xsel on X11).
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
    let mut all = mods::list(g);
    if ui.get_sort_desc() {
        all.reverse();
    }
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
                .nexus_key
                .as_ref()
                .and_then(|(_, id)| state.updates.get(&(g.steam.appid, *id)))
                .cloned()
                .unwrap_or_default();
            let icon_path = m
                .icon_src
                .as_ref()
                .map(|p| crate::meta::local_icon(p))
                .filter(|p| p.is_file())
                .or_else(|| m.nexus_key.as_ref().map(|(d, id)| crate::meta::nexus_icon(d, *id)).filter(|p| p.is_file()));
            let icon = icon_path.map(|p| state.icon(&p)).unwrap_or_default();
            ModItem {
                has_icon: icon.size().width > 0,
                icon,
                page: m.nexus_key.as_ref().map(|(d, id)| nexus::mod_page(d, *id)).unwrap_or_default().into(),
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
        tr!("{} of {total}", "{} de {total}", items.len())
    } else {
        match total {
            0 => String::new(),
            1 => tr!("1 mod · {on} enabled", "1 mod · {on} ligado"),
            _ => tr!("{total} mods · {on} enabled", "{total} mods · {on} ligados"),
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
    check_updates(ui, state, appid, false);
    let mut cfg = config::load();
    if cfg.last_game != Some(appid) {
        cfg.last_game = Some(appid);
        config::save(&cfg);
    }
}

/// Icons and Nexus info for the game's mods, once per game per session, off the UI
/// thread: local icon.png thumbnails first, then Nexus version and picture (cached
/// for 6 h; without an API key only the cache is used). Ends by reporting mods with
/// a newer version.
fn check_updates(ui: &AppWindow, state: &mut State, appid: u32, force: bool) {
    if !state.checked.insert(appid) && !force {
        return;
    }
    if force {
        state.updates.retain(|(a, _), _| *a != appid);
        ui.set_checking(true);
        set_progress(ui, tr!("Checking for updates…", "Buscando atualizações…"));
    }
    let Some(g) = state.game() else { return };
    let list = mods::list(g);
    let locals: Vec<PathBuf> = list.iter().filter_map(|m| m.icon_src.clone()).collect();
    let remotes: Vec<(String, u64, Option<String>)> = list
        .iter()
        .filter_map(|m| m.nexus_key.clone().map(|(d, id)| (d, id, m.version.clone())))
        .collect();
    let weak = ui.as_weak();
    let ping = move |w: &slint::Weak<AppWindow>, json: String| {
        let _ = w.upgrade_in_event_loop(move |ui| ui.invoke_updates_found(appid as i32, json.into()));
    };
    std::thread::spawn(move || {
        if locals.iter().filter(|p| crate::meta::ensure_local_icon(p).is_some()).count() > 0 {
            ping(&weak, "{}".into());
        }
        let total = remotes.len();
        let key = nexus::load_key();
        if force && (remotes.is_empty() || key.is_none()) {
            let msg = if key.is_none() {
                tr!("Add your Nexus API key in Settings to check for updates.", "Coloque a sua chave da API do Nexus em Configurações para buscar atualizações.")
            } else {
                tr!("None of these mods has a Nexus link to check.", "Nenhum destes mods tem link do Nexus para conferir.")
            };
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_checking(false);
                set_msg(&ui, msg, key.is_none());
            });
            return;
        }
        if remotes.is_empty() {
            return;
        }
        let mut info = crate::meta::load_info();
        let mut found = serde_json::Map::new();
        let mut fresh_icons = 0;
        for (domain, id, local) in &remotes {
            let k = crate::meta::info_key(domain, *id);
            let cached = info.get(&k).filter(|i| !force && crate::meta::is_fresh(i)).cloned();
            let i = match (cached, &key) {
                (Some(i), _) => i,
                (None, Some(key)) => match nexus::mod_info(domain, *id, key) {
                    Ok(i) => {
                        info.insert(k, i.clone());
                        i
                    }
                    Err(e) => {
                        log::line(&format!("nexus {domain}/{id}: {e:#}"));
                        continue;
                    }
                },
                (None, None) => match info.get(&k) {
                    Some(i) => i.clone(),
                    None => continue,
                },
            };
            let icon = crate::meta::nexus_icon(domain, *id);
            if !icon.is_file()
                && let Some(url) = &i.picture
                && crate::meta::fetch_nexus_icon(url, &icon).is_ok() {
                fresh_icons += 1;
                if fresh_icons % 12 == 0 {
                    ping(&weak, "{}".into());
                }
            }
            if let (Some(remote), Some(local)) = (&i.version, local)
                && nexus::is_newer(remote, local) {
                found.insert(id.to_string(), remote.clone().into());
            }
        }
        crate::meta::save_info(&info);
        let n = found.len();
        ping(&weak, serde_json::Value::Object(found).to_string());
        if force {
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_checking(false);
                if n == 0 {
                    set_msg(&ui, tr!("All {total} mods linked to the Nexus are up to date.", "Os {total} mods ligados ao Nexus estão atualizados."), false);
                }
            });
        }
    });
}

/// "2026-10-11" from a Unix time (UTC), without pulling a date crate.
fn date(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn size_of(p: &std::path::Path) -> (u64, usize) {
    if p.is_file() {
        return (std::fs::metadata(p).map(|m| m.len()).unwrap_or(0), 1);
    }
    let mut total = (0, 0);
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let (b, n) = size_of(&e.path());
            total.0 += b;
            total.1 += n;
        }
    }
    total
}

fn human_size(b: u64) -> String {
    match b {
        0..1024 => format!("{b} B"),
        1024..1_048_576 => format!("{:.0} KB", b as f64 / 1024.0),
        _ => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

/// Rows of the Properties dialog.
fn properties(g: &Game, id: &str) -> Option<(String, Vec<PropRow>)> {
    let m = mods::list(g).into_iter().find(|m| m.id == id)?;
    let path = mods::mod_path(g, id)?;
    let mut rows = Vec::new();
    let mut add = |label: String, value: String| {
        if !value.is_empty() {
            rows.push(PropRow { label: label.to_uppercase().into(), value: value.into() });
        }
    };
    add(tr!("Status", "Estado"), if m.enabled { tr!("Enabled", "Ligado") } else { tr!("Disabled", "Desligado") });
    add(tr!("Version", "Versão"), m.version.clone().unwrap_or_default());
    add(tr!("Author", "Autor"), m.author.clone().unwrap_or_default());
    if let Some(man) = mods::manifest(&path) {
        add(tr!("Description", "Descrição"), mods::json_field(&man, "Description").unwrap_or_default());
        add("ID".into(), mods::json_field(&man, "UniqueID").unwrap_or_default());
    }
    let (bytes, files) = size_of(&path);
    add(tr!("Size", "Tamanho"), tr!("{} in {files} files", "{} em {files} arquivos", human_size(bytes)));
    add(tr!("Location", "Local"), path.display().to_string());
    let recs = crate::records::load(g.steam.appid);
    if let Some(r) = recs.owner(&format!("{}/{}", g.loader.mods_dir(), id)).map(|i| &recs.mods[i]) {
        add(tr!("Installed on", "Instalado em"), date(r.installed_at));
        add(tr!("From file", "Do arquivo"), r.archive.clone());
        if r.files.len() > 1 {
            add(tr!("Also installed", "Também instalou"), r.files.iter().skip(1).cloned().collect::<Vec<_>>().join("\n"));
        }
    }
    if let Some((d, nid)) = &m.nexus_key {
        add("Nexus".into(), nexus::mod_page(d, *nid).replace("?tab=files", ""));
    }
    Some((m.name, rows))
}

/// Selects the file in the file manager (FileManager1 over D-Bus, which Dolphin,
/// Nautilus and others implement); falls back to opening the folder.
fn show_in_folder(p: &std::path::Path) {
    let uri = format!("file://{}", p.display());
    let ok = Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("array:string:{uri}"),
            "string:",
        ])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        open(&p.parent().unwrap_or(p).to_string_lossy());
    }
}

fn color(c: theme::Rgb) -> slint::Color {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    slint::Color::from_rgb_u8(b(c.0), b(c.1), b(c.2))
}

/// Applies the chosen theme. Without a wallust palette, falls back to the default.
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
                None => tr!("{} not installed", "{} não instalado", g.loader.label()).into(),
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

/// App notices: a stack in the corner (each disappears after 2.5 s) and a history under the bell.
struct Notes {
    toasts: Rc<VecModel<Notice>>,
    history: Rc<VecModel<Notice>>,
    /// Time (seconds) of each notice in the history, for the "3 min ago".
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
        0..=59 => tr!("now", "agora"),
        60..=3599 => tr!("{} min ago", "há {} min", d / 60),
        3600..=86399 => tr!("{} h ago", "há {} h", d / 3600),
        _ => tr!("{} days ago", "há {} dias", d / 86400),
    }
}

fn remove_toast(id: i32) {
    NOTES.with(|c| {
        if let Some(n) = c.borrow().as_ref()
            && let Some(i) = (0..n.toasts.row_count()).find(|&i| n.toasts.row_data(i).is_some_and(|t| t.id == id)) {
            n.toasts.remove(i);
        }
    });
}

/// Notice: goes on the stack (disappears after 2.5 s) and into the history. An empty
/// message only clears the progress.
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
        let notice = Notice { id, text: msg.clone(), error: err, when: tr!("now", "agora").into() };
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

/// Progress (download, install): stays on top of the stack until the next notice.
fn set_progress(ui: &AppWindow, msg: impl Into<SharedString>) {
    ui.set_progress(msg.into());
}

/// System notification, only for what comes from outside the window (a click on the
/// website): at that moment the app is usually behind the browser.
fn notify(summary: &str, body: &str, err: bool) {
    let mut n = notify_rust::Notification::new();
    n.appname("linex-mods").summary(summary).body(body).icon("applications-games");
    if err {
        n.urgency(notify_rust::Urgency::Critical);
    }
    if let Err(e) = n.show() {
        log::line(&format!("notification failed: {e}"));
    }
}

/// nxm:// link received (from the browser or another instance): downloads and installs
/// off the UI thread, showing progress in the message line.
fn handle_nxm(ui: &AppWindow, state: &mut State, url: &str) {
    let link = match nexus::NxmLink::parse(url) {
        Ok(l) => l,
        Err(e) => {
            log::line(&format!("link rejected: {e:#}"));
            return set_msg(ui, tr!("Nexus link: {e:#}", "Link do Nexus: {e:#}"), true);
        }
    };
    log::line(&format!("received {}", link.redacted()));
    let Some(i) = state.games.iter().position(|g| g.matches_nexus(&link.domain)) else {
        return set_msg(
            ui,
            tr!("The link is for \"{}\", which is not among the detected games with a mod loader.", "O link é de \"{}\", que não está entre os jogos detectados com loader de mods.", link.domain),
            true,
        );
    };
    ui.set_page(0);
    ui.set_current(i as i32);
    select(ui, state, i);
    let game = state.games[i].clone();
    set_progress(ui, tr!("Downloading from the Nexus…", "Baixando do Nexus…"));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let appid = game.steam.appid as i32;
        let game_name = game.steam.name.clone();
        let finish = |w: &slint::Weak<AppWindow>, msg: String, err: bool| {
            log::line(&msg);
            if err {
                notify(&tr!("Mod not installed", "Mod não instalado"), &msg, true);
            } else {
                notify(msg.split_once(": ").map_or(msg.as_str(), |(_, n)| n), &tr!("Installed in {game_name}", "Instalado em {game_name}"), false);
            }
            let _ = w.upgrade_in_event_loop(move |ui| ui.invoke_nxm_finished(appid, msg.into(), err));
        };
        let Some(key) = nexus::load_key() else {
            return finish(&weak, tr!("The Nexus API key is missing: see Settings.", "Falta a chave da API do Nexus: veja em Configurações."), true);
        };
        let mut last = u64::MAX;
        let w2 = weak.clone();
        let dl = nexus::download(&link, &key, |done, total| {
            let step = (done * 100).checked_div(total).unwrap_or(done / (1024 * 1024));
            if step != last {
                last = step;
                let txt = if total > 0 {
                    tr!("Downloading from the Nexus… {step}%", "Baixando do Nexus… {step}%")
                } else {
                    tr!("Downloading from the Nexus… {step} MB", "Baixando do Nexus… {step} MB")
                };
                let _ = w2.upgrade_in_event_loop(move |ui| set_progress(&ui, txt));
            }
        });
        let dl = match dl {
            Ok(d) => d,
            Err(e) => return finish(&weak, tr!("Download failed: {e:#}", "Download falhou: {e:#}"), true),
        };
        let _ = weak.upgrade_in_event_loop(|ui| set_progress(&ui, tr!("Installing…", "Instalando…")));
        match install::install_archive(&game, &dl.path, Some(dl.mod_name.clone()), Some(dl.source)) {
            Ok(rec) => {
                let _ = std::fs::remove_file(&dl.path);
                match rec.warning {
                    Some(w) => finish(&weak, tr!("{} installed, but {w}", "{} instalado, mas {w}", rec.name), true),
                    None => finish(&weak, tr!("Installed: {}", "Instalado: {}", rec.name), false),
                }
            }
            Err(e) => finish(
                &weak,
                tr!("{}: {e:#} (the file was kept in {})", "{}: {e:#} (o arquivo ficou em {})", dl.mod_name, dl.path.display()),
                true,
            ),
        }
    });
}

/// Nexus key status, read off the UI thread (the keyring can be slow).
fn refresh_nexus_status(ui: &AppWindow) {
    ui.set_handler_ok(desktop::is_nxm_handler());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let (txt, ok) = match nexus::load_key() {
            None => (tr!("No key saved. Without it the website button can't download.", "Nenhuma chave salva. Sem ela o botão do site não consegue baixar."), false),
            Some(k) => match nexus::validate(&k) {
                Ok((name, premium)) => (connected(&name, premium), true),
                Err(e) => (tr!("A key is saved, but it didn't work: {e:#}", "Há uma chave salva, mas não funcionou: {e:#}"), false),
            },
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_nexus_status(txt.into());
            ui.set_nexus_ok(ok);
        });
    });
}

fn connected(name: &str, premium: bool) -> String {
    if premium { tr!("Signed in as {name} · premium account", "Conectado como {name} · conta premium") } else { tr!("Signed in as {name} · free account", "Conectado como {name} · conta gratuita") }
}

/// Test mode (development): LINEX_SNAPSHOT=file.png saves a screenshot of the window
/// and quits. LINEX_GAME picks the game (part of the name), LINEX_PAGE=1 opens
/// Settings, LINEX_MESSAGE shows a notice.
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
    if let Ok(id) = std::env::var("LINEX_PROPS") {
        ui.invoke_show_properties(id.into());
    }
    if std::env::var("LINEX_SORT_DESC").as_deref() == Ok("1") {
        ui.set_sort_desc(true);
        ui.invoke_filter_changed(ui.get_filter());
    }
    if std::env::var("LINEX_PANEL").as_deref() == Ok("1") {
        ui.set_panel_open(true);
        ui.invoke_open_panel();
    }
    let weak = ui.as_weak();
    let delay = std::env::var("LINEX_SNAPSHOT_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(1800);
    slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
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

/// Original WAYLAND_DISPLAY, when the app opens through XWayland (see `prefer_x11`).
static WAYLAND_DISPLAY: std::sync::OnceLock<std::ffi::OsString> = std::sync::OnceLock::new();

/// Dragging files onto the window only works on X11: winit (Slint's windowing layer)
/// doesn't implement drag and drop on Wayland yet. With XWayland available, the app
/// opens through it (LINEX_WAYLAND=1 forces Wayland). Must run before any thread.
pub fn prefer_x11() {
    if std::env::var_os("LINEX_WAYLAND").is_some() || std::env::var_os("DISPLAY").is_none() {
        return;
    }
    if let Some(w) = std::env::var_os("WAYLAND_DISPLAY") {
        let _ = WAYLAND_DISPLAY.set(w);
        // Without Xft.dpi, winit computes the scale from the monitor's physical size and
        // the window opened about 33% larger than on Wayland. With Xft.dpi (KDE sets it
        // when there is scaling), the system's choice wins.
        let xft = Command::new("xrdb")
            .arg("-query")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("Xft.dpi"));
        // SAFETY: called at the start of main, with a single thread.
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
            if !xft && std::env::var_os("WINIT_X11_SCALE_FACTOR").is_none() {
                std::env::set_var("WINIT_X11_SCALE_FACTOR", "1");
            }
        }
    }
}

/// One install at a time: dragging several files starts several threads, and each
/// game's record is read and rewritten whole.
static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn is_archive(p: &std::path::Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "zip" | "7z" | "rar"))
}

/// Installs files dropped on the window into the open game.
fn install_files(ui: &AppWindow, state: &State, files: Vec<PathBuf>) {
    if ui.get_page() != 0 {
        return set_msg(ui, tr!("Open a game before dropping the file.", "Abra um jogo antes de soltar o arquivo."), true);
    }
    let Some(game) = state.game().cloned() else {
        return set_msg(ui, tr!("Open a game before dropping the file.", "Abra um jogo antes de soltar o arquivo."), true);
    };
    let (ok, bad): (Vec<_>, Vec<_>) = files.into_iter().partition(|f| is_archive(f));
    for b in bad {
        set_msg(ui, tr!("{}: not a .zip, .7z or .rar file.", "{}: não é .zip, .7z nem .rar.", b.file_name().unwrap_or_default().to_string_lossy()), true);
    }
    if ok.is_empty() {
        return;
    }
    set_progress(ui, tr!("Installing in {}…", "Instalando em {}…", game.steam.name));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let _lock = INSTALL_LOCK.lock();
        let mut results = Vec::new();
        for f in &ok {
            let name = install::clean_stem(f);
            results.push(match install::install_archive(&game, f, None, None) {
                Ok(rec) => match rec.warning {
                    Some(w) => (tr!("{} installed, but {w}", "{} instalado, mas {w}", rec.name), true),
                    None => (tr!("Installed: {}", "Instalado: {}", rec.name), false),
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
            return set_msg(ui, tr!("SMAPI has its own installer: its page opened in your browser.", "O SMAPI tem instalador próprio: a página dele abriu no navegador."), false);
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
        return set_msg(ui, tr!("This doesn't look like a Windows Unity game: the loader won't work with it.", "Este jogo não parece ser um jogo Unity de Windows: o loader não serve para ele."), true);
    };
    ui.set_installing(appid);
    set_progress(ui, tr!("Downloading {}…", "Baixando o {}…", loader.label()));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let mut last = u64::MAX;
        let w2 = weak.clone();
        let label = loader.label();
        let r = loaders::install(&dir, loader, il2cpp, |done, total| {
            let step = (done * 100).checked_div(total).unwrap_or(done / (1024 * 1024));
            if step != last {
                last = step;
                let txt = if total > 0 { tr!("Downloading {label}… {step}%", "Baixando o {label}… {step}%") } else { tr!("Downloading {label}… {step} MB", "Baixando o {label}… {step} MB") };
                let _ = w2.upgrade_in_event_loop(move |ui| set_progress(&ui, txt));
            }
        });
        let (msg, err) = match r {
            Ok(file) => {
                log::line(&format!("loader installed: {file} in {}", dir.display()));
                (tr!("{label} installed. Start the game once so it can finish setting itself up.", "{label} instalado. Abra o jogo uma vez para ele terminar de se configurar."), false)
            }
            Err(e) => (tr!("Couldn't install {label}: {e:#}", "Não deu para instalar o {label}: {e:#}"), true),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_loader_finished(appid, msg.into(), err));
    });
}

pub fn run(initial_nxm: Option<String>) -> anyhow::Result<()> {
    desktop::refresh_desktop_file();
    let ui = AppWindow::new()?;
    // Only works after the platform exists (AppWindow::new creates it).
    let app_id = std::env::var("LINEX_APP_ID").unwrap_or_else(|_| "linex-mods".into());
    let _ = slint::set_xdg_app_id(app_id);
    let state = Rc::new(RefCell::new(State {
        games: games::moddable_games(),
        current: None,
        updates: Default::default(),
        checked: Default::default(),
        icons: Default::default(),
    }));

    {
        let lang = config::load().lang;
        crate::i18n::apply(&lang);
        ui.set_lang_mode(match lang.as_str() {
            "en" => 1,
            "pt" => 2,
            _ => 0,
        });
    }
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
        let state = state.clone();
        ui.on_open_mod_folder(move |id| {
            if let Some(p) = state.borrow().game().and_then(|g| mods::mod_path(g, &id)) {
                show_in_folder(&p);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_show_properties(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let Some((title, rows)) = state.borrow().game().and_then(|g| properties(g, &id)) else { return };
            ui.set_props_title(title.into());
            ui.set_props(ModelRc::new(VecModel::from(rows)));
            ui.set_props_open(true);
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_check_updates(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            let Some(appid) = st.game().map(|g| g.steam.appid) else { return };
            check_updates(&ui, &mut st, appid, true);
        });
    }
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
            // The game now has a loader: it joins (or updates in) the list and opens.
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
        // X11 sends one event per file; group the ones that arrive together.
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
                Err(e) => set_msg(&ui, if on { tr!("Couldn't enable {id}: {e:#}", "Não deu para ligar {id}: {e:#}") } else { tr!("Couldn't disable {id}: {e:#}", "Não deu para desligar {id}: {e:#}") }, true),
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
                Ok(name) => set_msg(&ui, tr!("{name} was moved to the trash.", "{name} foi para a Lixeira."), false),
                Err(e) => set_msg(&ui, tr!("Couldn't remove {id}: {e:#}", "Não deu para remover {id}: {e:#}"), true),
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
            // The file picker and the extraction run off the UI thread.
            std::thread::spawn(move || {
                let _lock = INSTALL_LOCK.lock();
                let files = rfd::FileDialog::new()
                    .set_title(tr!("Choose the mod files", "Escolha os arquivos do mod"))
                    .add_filter(tr!("Mod archives", "Arquivos de mod"), &["zip", "7z", "rar"])
                    .pick_files()
                    .unwrap_or_default();
                let mut ok = Vec::new();
                let mut errors = Vec::new();
                for f in &files {
                    let name = install::clean_stem(f);
                    match install::install_archive(&game, f, None, None) {
                        Ok(rec) => match rec.warning {
                            Some(w) => errors.push(tr!("{name} installed, but {w}", "{name} instalado, mas {w}")),
                            None => ok.push(name),
                        },
                        Err(e) => errors.push(format!("{name}: {e:#}")),
                    }
                }
                let (msg, is_err) = if !errors.is_empty() {
                    (tr!("Failed — {}", "Falhou — {}", errors.join(" | ")), true)
                } else if ok.len() == 1 {
                    (tr!("Installed: {}", "Instalado: {}", ok[0]), false)
                } else if !ok.is_empty() {
                    (tr!("{} mods installed", "{} mods instalados", ok.len()), false)
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
                // Only save a key the Nexus accepts.
                let (txt, ok) = match nexus::validate(&key) {
                    Ok((name, premium)) => match nexus::save_key(&key) {
                        Ok(place) => (tr!("{}. Key saved {place}.", "{}. Chave guardada {place}.", connected(&name, premium)), true),
                        Err(e) => (tr!("The key works, but it couldn't be saved: {e:#}", "A chave funciona, mas não deu para guardar: {e:#}"), false),
                    },
                    Err(e) => (tr!("Key not accepted: {e:#}", "Chave não aceita: {e:#}"), false),
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
                ui.set_nexus_status(tr!("Key deleted.", "Chave apagada.").into());
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_register_handler(move || {
            let Some(ui) = weak.upgrade() else { return };
            match desktop::register_nxm_handler() {
                Ok(()) => set_msg(&ui, tr!("Done: the \"Mod Manager Download\" button now opens here.", "Pronto: o botão \"Mod Manager Download\" agora abre aqui."), false),
                Err(e) => set_msg(&ui, tr!("Couldn't register: {e:#}", "Não deu para registrar: {e:#}"), true),
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
                if n > 0 {
                    set_msg(&ui, if n == 1 { tr!("1 mod has a new version on the Nexus.", "1 mod tem versão nova no Nexus.") } else { tr!("{n} mods have a new version on the Nexus.", "{n} mods têm versão nova no Nexus.") }, false);
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_copy_fix(move || {
            let Some(ui) = weak.upgrade() else { return };
            if copy_text(&ui.get_game_fix()) {
                set_msg(&ui, tr!("Copied. Paste it in the game's launch options and restart the game.", "Copiado. Cole nas opções de inicialização do jogo e reabra o jogo."), false);
            } else {
                set_msg(&ui, tr!("Couldn't copy (wl-copy or xclip is missing). Use: {}", "Não deu para copiar (falta o wl-copy ou xclip). Use: {}", ui.get_game_fix()), true);
            }
        });
    }
    // Refresh the list when the mods folder changes from outside (file manager,
    // another program): checks every 2 s.
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
            if changed
                && let Some(ui) = weak.upgrade() {
                refresh_mods(&ui, &st);
            }
        });
    }

    // Links handed over by another instance (a click on the website with the window open).
    {
        let weak = ui.as_weak();
        ipc::listen(move |msg| {
            if msg.starts_with("nxm://") {
                let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_nxm_received(msg.into()));
            }
        });
    }

    {
        let (weak, state) = (ui.as_weak(), state.clone());
        ui.on_lang_changed(move |i| {
            let mut cfg = config::load();
            cfg.lang = ["", "en", "pt"][i.clamp(0, 2) as usize].into();
            config::save(&cfg);
            crate::i18n::apply(&cfg.lang);
            // Texts built in Rust (status line, counts, game list) follow on the next
            // refresh; do it now.
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            ui.set_games(ModelRc::new(VecModel::from(game_items(&st.games))));
            if let Some(i) = st.current {
                select(&ui, &mut st, i);
            }
            refresh_nexus_status(&ui);
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
    // The palette changes when the wallpaper changes: checks every 3 s.
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

    // Open on the last game used.
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
