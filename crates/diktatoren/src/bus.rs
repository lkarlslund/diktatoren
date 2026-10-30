//! Carries events from background threads (engine, hotkeys, tray, IPC, downloads) into the UI.

use std::sync::{Mutex, OnceLock};

use futures::channel::mpsc;

use crate::download::Progress;
use crate::engine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Hotkey pressed, tray clicked or `diktatoren toggle`.
    Toggle,
    Start,
    Stop,
    Insert,
    Copy,
    Cancel,
    Settings,
    Quit,
}

#[derive(Debug, Clone)]
pub enum BusEvent {
    Engine(engine::Event),
    HotkeyPressed,
    HotkeyReleased,
    Action(Action),
    /// Dictate from this WAV file instead of the microphone, for testing.
    Feed(std::path::PathBuf),
    DownloadProgress(String, Progress),
    DownloadFinished(String, Result<(), String>),
    /// Speech models offered by an audio.cpp server, or why they could not be listed.
    ServerModels(String, Result<Vec<String>, String>),
    /// Models offered by the language-model server.
    AiModels(String, Result<Vec<String>, String>),
    /// A tidied stretch of dictation: where it started, what was sent, and the answer.
    Tidied { start: usize, original: String, result: Result<String, String> },
    /// A new restore token from the remote-desktop portal, to persist.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    PortalToken(String),
    Notice(String),
}

struct Bus {
    tx: mpsc::UnboundedSender<BusEvent>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<BusEvent>>>,
}

fn bus() -> &'static Bus {
    static BUS: OnceLock<Bus> = OnceLock::new();
    BUS.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded();
        Bus { tx, rx: Mutex::new(Some(rx)) }
    })
}

pub fn send(event: BusEvent) {
    let _ = bus().tx.unbounded_send(event);
}

/// The receiving end; available once.
pub fn take_receiver() -> Option<mpsc::UnboundedReceiver<BusEvent>> {
    bus().rx.lock().unwrap().take()
}
