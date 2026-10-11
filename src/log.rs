//! Log in ~/.local/state/linex-mods/linex-mods.log. The process the browser starts for
//! an nxm:// link has no window; without a file, a failed click leaves no trace.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

fn path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::steam::home().join(".local/state"))
        .join("linex-mods/linex-mods.log")
}

pub fn line(msg: &str) {
    let p = path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    // Don't let it grow without bound.
    if fs::metadata(&p).is_ok_and(|m| m.len() > 512 * 1024) {
        let _ = fs::rename(&p, p.with_extension("log.old"));
    }
    let ts = crate::records::now();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{ts} [{}] {msg}", std::process::id());
    }
}
