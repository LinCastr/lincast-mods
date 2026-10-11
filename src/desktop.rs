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

/// O linex-mods já é quem abre os links nxm://?
pub fn is_nxm_handler() -> bool {
    Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/nxm"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == DESKTOP_ID)
}

pub fn register_nxm_handler() -> Result<()> {
    let exe = std::env::current_exe().context("caminho do executável")?;
    let exe = exe.to_string_lossy();
    if exe.contains(['"', '`', '$', '\\', '\n']) {
        bail!("o caminho do programa tem caracteres que o atalho não aceita: {exe}");
    }
    let dir = apps_dir();
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join(DESKTOP_ID),
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=linex-mods\n\
             Comment=Gerenciador de mods\n\
             Exec=\"{exe}\" %u\n\
             Icon=applications-games\n\
             Terminal=false\n\
             Categories=Game;Utility;\n\
             MimeType=x-scheme-handler/nxm;\n\
             StartupWMClass=linex-mods\n"
        ),
    )?;
    let ok = Command::new("xdg-mime")
        .args(["default", DESKTOP_ID, "x-scheme-handler/nxm"])
        .status()
        .is_ok_and(|s| s.success());
    let _ = Command::new("update-desktop-database").arg(&dir).status();
    if !ok {
        bail!("o xdg-mime falhou (o pacote xdg-utils está instalado?)");
    }
    Ok(())
}
