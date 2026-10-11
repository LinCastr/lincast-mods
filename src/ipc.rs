//! Instância única. Clicar em "Mod Manager Download" faz o navegador abrir outro
//! processo com o link; ele repassa o link para a janela aberta por um socket Unix.
//! Tenta mais de um caminho porque o processo do navegador pode não ter o mesmo
//! XDG_RUNTIME_DIR (problema que o Amethyst documenta).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

fn uid() -> u32 {
    fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0)
}

fn paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()) {
        v.push(d.join("linex-mods.sock"));
    }
    v.push(PathBuf::from(format!("/tmp/linex-mods-{}.sock", uid())));
    v
}

/// Manda a mensagem para a instância aberta. `false` = não há nenhuma.
pub fn send(msg: &str) -> bool {
    for p in paths() {
        if let Ok(mut s) = UnixStream::connect(&p) {
            if s.write_all(format!("{msg}\n").as_bytes()).is_ok() {
                crate::log::line(&format!("repassado para a janela aberta ({})", p.display()));
                return true;
            }
        }
    }
    false
}

/// Escuta em todos os caminhos; cada linha recebida vai para `on_message`.
pub fn listen(on_message: impl Fn(String) + Send + Sync + Clone + 'static) {
    for p in paths() {
        // Socket que sobrou de uma execução que caiu: ninguém atende, pode apagar.
        if p.exists() && UnixStream::connect(&p).is_err() {
            let _ = fs::remove_file(&p);
        }
        let Ok(listener) = UnixListener::bind(&p) else { continue };
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

/// Apaga os sockets ao fechar a janela.
pub fn cleanup() {
    for p in paths() {
        let _ = fs::remove_file(p);
    }
}
