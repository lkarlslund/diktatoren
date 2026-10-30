//! Single-instance control. The first instance listens on a local socket; later invocations
//! such as `diktatoren toggle` forward their command to it and exit.

use std::io::{BufRead, BufReader, Write};
use std::thread;

use anyhow::Result;
#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(not(unix))]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{ListenerOptions, Name, Stream, prelude::*};

use crate::bus::{self, Action, BusEvent};

fn socket_base() -> String {
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    format!("diktatoren-{user}.sock")
}

/// Unix sockets live in the per-user runtime directory; Linux abstract sockets would be
/// shared across users.
#[cfg(unix)]
fn socket_path() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(socket_base())
}

fn socket_name() -> std::io::Result<Name<'static>> {
    #[cfg(unix)]
    return socket_path().to_fs_name::<GenericFilePath>();
    #[cfg(not(unix))]
    socket_base().to_ns_name::<GenericNamespaced>()
}

pub fn parse(word: &str) -> Option<Action> {
    Some(match word {
        "toggle" => Action::Toggle,
        "start" => Action::Start,
        "stop" => Action::Stop,
        "insert" => Action::Insert,
        "copy" => Action::Copy,
        "cancel" => Action::Cancel,
        "settings" => Action::Settings,
        "quit" => Action::Quit,
        _ => return None,
    })
}

/// Sends `command` to a running instance. Returns false when none is running.
pub fn send_to_running(command: &str) -> bool {
    let Ok(name) = socket_name() else { return false };
    match Stream::connect(name) {
        Ok(mut stream) => stream.write_all(format!("{command}\n").as_bytes()).is_ok(),
        Err(_) => false,
    }
}

/// Starts accepting commands from later invocations.
pub fn serve() -> Result<()> {
    // A stale socket from a crashed instance; send_to_running already failed on it.
    #[cfg(unix)]
    let _ = std::fs::remove_file(socket_path());
    let name = socket_name()?;
    let listener = ListenerOptions::new().name(name).create_sync()?;
    thread::Builder::new().name("ipc".into()).spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(conn).read_line(&mut line).is_err() {
                continue;
            }
            let line = line.trim();
            if let Some(path) = line.strip_prefix("feed ") {
                bus::send(BusEvent::Feed(path.into()));
            } else if let Some(action) = parse(line) {
                bus::send(BusEvent::Action(action));
            }
        }
    })?;
    Ok(())
}
