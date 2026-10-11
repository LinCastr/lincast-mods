//! Atalho no menu de aplicativos e registro como tratador dos links nxm://.

use anyhow::{Context, Result, bail};
use std::fs;
use std::process::Command;

const DESKTOP_ID: &str = "linex-mods.desktop";

fn apps_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::steam::home().join(".local/share"))
        .join("applications")
}

/// Caminho do programa. Se o binário foi trocado com o app aberto (recompilação,
/// atualização), o Linux devolve ".../linex-mods (deleted)"; gravar isso no atalho
/// fazia o sistema pular o linex-mods e mandar o link para outro programa.
fn exe_path() -> Result<String> {
    let exe = std::env::current_exe().context(tr!("path of the program", "caminho do executável"))?;
    let exe = exe.to_string_lossy();
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe).to_string();
    if exe.contains(['"', '`', '$', '\\', '\n']) {
        bail!("{}", tr!("the program path has characters a desktop entry can't hold: {exe}", "o caminho do programa tem caracteres que o atalho não aceita: {exe}"));
    }
    Ok(exe)
}

/// O linex-mods já é quem abre os links nxm://? Confere também se o atalho aponta
/// para um programa que existe: com Exec quebrado o xdg-mime ignora a escolha.
pub fn is_nxm_handler() -> bool {
    let chosen = Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/nxm"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == DESKTOP_ID);
    chosen && desktop_exec_ok()
}

fn desktop_exec_ok() -> bool {
    let Ok(txt) = fs::read_to_string(apps_dir().join(DESKTOP_ID)) else { return false };
    txt.lines()
        .find_map(|l| l.strip_prefix("Exec="))
        .map(|l| match l.strip_prefix('"') {
            Some(q) => q.split('"').next().unwrap_or(""),
            None => l.split_whitespace().next().unwrap_or(""),
        })
        .is_some_and(|p| std::path::Path::new(p).is_file())
}

const ICON_SVG: &str = include_str!("../assets/linex-mods.svg");

fn icon_path() -> std::path::PathBuf {
    apps_dir().parent().unwrap().join("icons/hicolor/scalable/apps/linex-mods.svg")
}

/// Ao abrir: cria o atalho no menu de aplicativos (com ícone) se ainda não existe.
/// Se existe mas aponta para um programa que sumiu (mudou de lugar), regrava com o
/// caminho atual. Só nesses casos: regravar sempre fazia qualquer cópia aberta
/// (ex.: a de desenvolvimento) tomar os links para si.
pub fn refresh_desktop_file() {
    let icon = icon_path();
    if !icon.is_file() {
        let _ = fs::create_dir_all(icon.parent().unwrap());
        let _ = fs::write(&icon, ICON_SVG);
    }
    let file = apps_dir().join(DESKTOP_ID);
    let exists = file.is_file();
    // Atalho de versão antiga (ícone genérico): só troca o ícone, mantém o resto.
    if let Ok(txt) = fs::read_to_string(&file)
        && txt.contains("Icon=applications-games") {
        let _ = fs::write(&file, txt.replace("Icon=applications-games", "Icon=linex-mods"));
    }
    if (!exists || !desktop_exec_ok())
        && let Ok(exe) = exe_path() {
        let _ = write_desktop(&exe);
        let _ = Command::new("update-desktop-database").arg(apps_dir()).status();
    }
}

fn write_desktop(exe: &str) -> Result<()> {
    // Aspas só quando há espaço: o xdg-mime não entende Exec="..." e, ao não achar
    // o programa, ignora a escolha e manda o link para outro tratador.
    let exec = if exe.contains(char::is_whitespace) { format!("\"{exe}\"") } else { exe.to_string() };
    let dir = apps_dir();
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join(DESKTOP_ID),
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=linex-mods\n\
             Comment=Gerenciador de mods\n\
             Exec={exec} %u\n\
             Icon=linex-mods\n\
             Terminal=false\n\
             Categories=Game;Utility;\n\
             MimeType=x-scheme-handler/nxm;\n\
             StartupWMClass=linex-mods\n"
        ),
    )?;
    Ok(())
}

pub fn register_nxm_handler() -> Result<()> {
    write_desktop(&exe_path()?)?;
    let dir = apps_dir();
    let ok = Command::new("xdg-mime")
        .args(["default", DESKTOP_ID, "x-scheme-handler/nxm"])
        .status()
        .is_ok_and(|s| s.success());
    let _ = Command::new("update-desktop-database").arg(&dir).status();
    if !ok {
        bail!("{}", tr!("xdg-mime failed (is the xdg-utils package installed?)", "o xdg-mime falhou (o pacote xdg-utils está instalado?)"));
    }
    Ok(())
}
