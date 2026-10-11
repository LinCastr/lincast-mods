//! "Browse" tab of a game: search the Nexus or Thunderstore and install from the results.

use super::{AppWindow, BrowseItem, State, install_package, open, set_msg};
use crate::browse::{self, Listing, Match, Sort};
use crate::mods::{self, Source};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;

/// A result with how it relates to the installed mods.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Result {
    pub listing: Listing,
    pub state: u8,
    pub note: String,
}

/// What the confirmation dialog will do.
#[derive(Debug, Clone)]
pub enum Pending {
    OpenPage(String),
    /// Download pages of missing dependencies.
    OpenPages(Vec<String>),
    Install(crate::thunderstore::PackageRef),
}

fn icon_path(l: &Listing, domain: &str) -> PathBuf {
    match l.source {
        Some(Source::Nexus) => crate::meta::nexus_icon(domain, l.id.parse().unwrap_or(0)),
        _ => crate::meta::icons_dir().join(format!("ts-{}.png", l.id)),
    }
}

/// "Similar to …" line, worded per site (Portuguese needs "pelo Nexus" / "pela Thunderstore").
fn similar_note(name: &str, from: Source) -> String {
    match from {
        Source::Nexus => tr!("Similar to \"{name}\", installed from the Nexus", "Parecido com \"{name}\", instalado pelo Nexus"),
        Source::Thunderstore => tr!("Similar to \"{name}\", installed from Thunderstore", "Parecido com \"{name}\", instalado pela Thunderstore"),
        Source::Unknown => tr!("Similar to \"{name}\", already installed", "Parecido com \"{name}\", já instalado"),
    }
}

fn downloads(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3).replace(".0k", "k"),
        _ => format!("{:.1}M", n as f64 / 1e6).replace(".0M", "M"),
    }
}

/// Marks each result against the installed mods.
fn classify(list: Vec<Listing>, installed: &[mods::Mod], installed_dirs: impl Fn(&str) -> bool) -> Vec<Result> {
    list.into_iter()
        .map(|l| {
            // "Owner-Some_Lib" → "Some Lib".
            let missing: Vec<String> = l
                .deps
                .iter()
                .filter(|d| !installed_dirs(d))
                .map(|d| d.split_once('-').map_or(d.as_str(), |(_, n)| n).replace('_', " "))
                .collect();
            let deps_note = match missing.len() {
                0 => String::new(),
                n => tr!("Also installs {n}: {}", "Instala junto {n}: {}", missing.join(", ")),
            };
            let (state, note) = match browse::match_installed(&l, installed) {
                Match::Installed { newer: true } => (2, String::new()),
                Match::Installed { .. } => (1, String::new()),
                Match::Similar { name, from } => (3, similar_note(&name, from)),
                Match::None => (0, deps_note),
            };
            Result { listing: l, state, note }
        })
        .collect()
}

thread_local! {
    static DEBOUNCE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Typing in the search box: search 450 ms after the last key.
pub fn query_edited(ui: &AppWindow) {
    let weak = ui.as_weak();
    DEBOUNCE.with(|d| {
        let t = slint::Timer::default();
        t.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(450), move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_browse_search();
            }
        });
        *d.borrow_mut() = Some(t);
    });
}

/// Sites available for the game, set when it is selected.
pub fn on_select(ui: &AppWindow, state: &mut State) {
    let Some(g) = state.game() else { return };
    let ts = crate::thunderstore::community_cached(g.steam.appid).is_some();
    ui.set_has_nexus(true);
    ui.set_has_thunderstore(ts);
    // BepInEx games live on Thunderstore more than on the Nexus.
    ui.set_browse_source(if ts && g.loader == crate::games::Loader::BepInEx { 2 } else { 1 });
    ui.set_browse_query(SharedString::new());
    ui.set_browse_items(ModelRc::default());
    ui.set_browse_status(SharedString::new());
    state.browse.clear();
    if ui.get_game_tab() == 1 {
        ui.invoke_browse_search();
    }
}

pub fn search(ui: &AppWindow, state: &mut State) {
    let Some(game) = state.game().cloned() else { return };
    state.browse_gen += 1;
    let generation = state.browse_gen;
    let source = ui.get_browse_source();
    let query = ui.get_browse_query().to_string();
    let sort = match ui.get_browse_sort() {
        1 => Sort::Updated,
        2 => Sort::Name,
        _ => Sort::Popular,
    };
    let hidden = ui.get_browse_hidden();
    ui.set_browse_loading(true);
    ui.set_browse_status(SharedString::new());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let domain = game.nexus_domain();
        let found = match source {
            2 => match crate::thunderstore::community_for(game.steam.appid) {
                Some(c) => browse::ts_search(&c, &query, sort, hidden),
                None => Err(anyhow::anyhow!("{}", tr!("This game is not on Thunderstore.", "Este jogo não está na Thunderstore."))),
            },
            _ => browse::nexus_search(&domain, &query, sort, hidden),
        };
        let installed = mods::list(&game);
        let plugins = game.dir().join("BepInEx/plugins");
        let off = game.dir().join(".linex-disabled/BepInEx/plugins");
        let (json, err) = match found {
            Ok(list) => {
                let results = classify(list, &installed, |d| plugins.join(d).exists() || off.join(d).exists());
                (serde_json::to_string(&results).unwrap_or_default(), String::new())
            }
            Err(e) => (String::new(), format!("{e:#}")),
        };
        let listings: Vec<Listing> = serde_json::from_str::<Vec<Result>>(&json).unwrap_or_default().into_iter().map(|r| r.listing).collect();
        let w = weak.clone();
        let _ = w.upgrade_in_event_loop(move |ui| ui.invoke_browse_results(generation as i32, json.into(), err.into()));
        // Icons after the list: each one a small download, cached for good.
        let mut fresh = 0;
        for l in &listings {
            let p = icon_path(l, &domain);
            if !p.is_file() && let Some(url) = &l.icon_url && crate::meta::fetch_nexus_icon(url, &p).is_ok() {
                fresh += 1;
                if fresh % 10 == 0 {
                    let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_browse_icons(generation as i32));
                }
            }
        }
        if fresh > 0 {
            let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_browse_icons(generation as i32));
        }
    });
}

pub fn on_results(ui: &AppWindow, state: &mut State, generation: i32, json: &str, err: &str) {
    if generation as u64 != state.browse_gen {
        return;
    }
    ui.set_browse_loading(false);
    state.browse = serde_json::from_str(json).unwrap_or_default();
    let status = if !err.is_empty() {
        tr!("Search failed: {err}", "A busca falhou: {err}")
    } else if state.browse.is_empty() {
        tr!("Nothing found.", "Nada encontrado.")
    } else {
        String::new()
    };
    ui.set_browse_status(status.into());
    refresh_items(ui, state);
}

pub fn refresh_items(ui: &AppWindow, state: &State) {
    let domain = state.game().map(|g| g.nexus_domain()).unwrap_or_default();
    let items: Vec<BrowseItem> = state
        .browse
        .iter()
        .map(|r| {
            let l = &r.listing;
            let p = icon_path(l, &domain);
            let icon = if p.is_file() { state.icon(&p) } else { Default::default() };
            BrowseItem {
                has_icon: icon.size().width > 0,
                icon,
                id: l.id.clone().into(),
                name: l.name.clone().into(),
                author: l.author.clone().into(),
                summary: l.summary.lines().next().unwrap_or("").into(),
                version: l.version.clone().into(),
                downloads: downloads(l.downloads).into(),
                state: r.state as i32,
                note: r.note.clone().into(),
                page: l.page.clone().into(),
            }
        })
        .collect();
    ui.set_browse_items(ModelRc::new(VecModel::from(items)));
}

/// After an install: the installed/update marks change.
pub fn reclassify(ui: &AppWindow, state: &mut State) {
    let Some(game) = state.game().cloned() else { return };
    if state.browse.is_empty() {
        return;
    }
    let installed = mods::list(&game);
    let plugins = game.dir().join("BepInEx/plugins");
    let list = state.browse.drain(..).map(|r| r.listing).collect();
    state.browse = classify(list, &installed, |d| plugins.join(d).exists());
    refresh_items(ui, state);
}

/// "Install" / "Download" / "Update" on a result.
pub fn install(ui: &AppWindow, state: &mut State, id: &str) {
    let Some(r) = state.browse.iter().find(|r| r.listing.id == id).cloned() else { return };
    let Some(game) = state.game().cloned() else { return };
    let l = &r.listing;
    let similar = (r.state == 3).then(|| r.note.clone());
    match l.source {
        Some(Source::Nexus) => {
            // Free accounts can only download through the website's button, which sends
            // the file back here.
            let page = l.page.clone();
            match similar {
                Some(note) => confirm(ui, state, &l.name, &tr!("{note}. Download it anyway?", "{note}. Baixar mesmo assim?"), &tr!("Open download page", "Abrir página de download"), Pending::OpenPage(page)),
                None => {
                    open(&page);
                    set_msg(ui, tr!("On the page, click \"Mod Manager Download\": the mod comes back here and installs.", "Na página, clique em \"Mod Manager Download\": o mod volta para cá e instala."), false);
                }
            }
        }
        _ => {
            let Ok(pkg) = crate::thunderstore::PackageRef::parse(&l.id) else { return };
            let deps_line = (!r.note.is_empty() && r.state == 0).then(|| r.note.clone());
            if r.state == 2 || (similar.is_none() && deps_line.is_none()) {
                return start_install(ui, state, pkg, game);
            }
            let mut text = Vec::new();
            if let Some(n) = similar {
                text.push(format!("{n}."));
            }
            if let Some(d) = deps_line {
                text.push(format!("{d}."));
            }
            confirm(ui, state, &l.name, &text.join("\n\n"), &tr!("Install", "Instalar"), Pending::Install(pkg));
        }
    }
}

fn start_install(ui: &AppWindow, state: &State, pkg: crate::thunderstore::PackageRef, game: crate::games::Game) {
    super::set_progress(ui, tr!("Installing {}…", "Instalando {}…", pkg.name.replace('_', " ")));
    install_package(ui, pkg, Some(game), state.games.clone());
}

pub fn confirm(ui: &AppWindow, state: &mut State, title: &str, text: &str, button: &str, action: Pending) {
    state.pending = Some(action);
    ui.set_confirm_title(title.into());
    ui.set_confirm_text(text.into());
    ui.set_confirm_button(button.into());
    ui.set_confirm_open(true);
}

pub fn confirm_accept(ui: &AppWindow, state: &mut State) {
    match state.pending.take() {
        Some(Pending::OpenPage(url)) => {
            open(&url);
            set_msg(ui, tr!("On the page, click \"Mod Manager Download\": the mod comes back here and installs.", "Na página, clique em \"Mod Manager Download\": o mod volta para cá e instala."), false);
        }
        Some(Pending::OpenPages(urls)) => {
            for u in &urls {
                open(u);
            }
            set_msg(ui, tr!("On each page, click \"Mod Manager Download\": they come back here and install.", "Em cada página, clique em \"Mod Manager Download\": eles voltam para cá e instalam."), false);
        }
        Some(Pending::Install(pkg)) => {
            if let Some(game) = state.game().cloned() {
                start_install(ui, state, pkg, game);
            }
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_download_counts() {
        assert_eq!(downloads(950), "950");
        assert_eq!(downloads(12_400), "12.4k");
        assert_eq!(downloads(3_000_000), "3M");
    }
}
