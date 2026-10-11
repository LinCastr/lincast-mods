//! Single instance. Clicking "Mod Manager Download" makes the browser start another
//! process with the link; it hands the link to the open window over a Unix socket.
//! Tries more than one path because the browser's process may not have the same
//! XDG_RUNTIME_DIR (a problem Amethyst documents).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Sockets this process created; only these are removed on exit, so closing a second
/// window doesn't take the first window's socket with it.
static BOUND: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn uid() -> u32 {
    fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0)
}

/// The /tmp fallback is shared with other users: trust a path only if it is our own
/// socket (or doesn't exist yet).
fn ours(p: &Path) -> bool {
    match fs::symlink_metadata(p) {
        Ok(m) => m.file_type().is_socket() && m.uid() == uid(),
        Err(_) => true,
    }
}

fn paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()) {
        v.push(d.join("linex-mods.sock"));
    }
    v.push(PathBuf::from(format!("/tmp/linex-mods-{}.sock", uid())));
    v
}

/// Sends the message to the open instance. `false` = there is none.
pub fn send(msg: &str) -> bool {
    for p in paths() {
        if p.exists()
            && ours(&p)
            && let Ok(mut s) = UnixStream::connect(&p)
            && s.write_all(format!("{msg}\n").as_bytes()).is_ok() {
            crate::log::line(&format!("handed over to the open window ({})", p.display()));
            return true;
        }
    }
    false
}

/// Listens on every path; each line received goes to `on_message`.
pub fn listen(on_message: impl Fn(String) + Send + Sync + Clone + 'static) {
    for p in paths() {
        if !ours(&p) {
            continue;
        }
        // Socket left over from a crashed run: nobody answers, safe to remove.
        if p.exists() && UnixStream::connect(&p).is_err() {
            let _ = fs::remove_file(&p);
        }
        let Ok(listener) = UnixListener::bind(&p) else { continue };
        // Only this user may connect and hand us links.
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
        BOUND.lock().unwrap_or_else(|e| e.into_inner()).push(p.clone());
        let cb = on_message.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        cb(line);
                    }
                }
            }
        });
    }
}

/// Removes the sockets when the window closes.
pub fn cleanup() {
    for p in BOUND.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        let _ = fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_own_sockets_are_trusted() {
        let base = std::env::temp_dir().join(format!("linex-ipc-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let (missing, file, sock) = (base.join("missing"), base.join("file"), base.join("sock"));
        fs::write(&file, "").unwrap();
        let _l = UnixListener::bind(&sock).unwrap();
        assert!(ours(&missing));
        assert!(!ours(&file), "a regular file in place of the socket");
        assert!(ours(&sock));
        fs::remove_dir_all(base).unwrap();
    }
}
