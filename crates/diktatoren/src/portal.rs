//! XDG desktop portals for Wayland, where applications may neither grab global keys nor send
//! keystrokes to other windows directly:
//!
//! * GlobalShortcuts binds the dictation hotkey and reports press and release.
//! * RemoteDesktop sends the paste shortcut (or types text) into the focused window. The
//!   desktop asks for permission once; the restore token keeps it across restarts.

use std::sync::OnceLock;
use std::thread;

use anyhow::{Context, Result};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::{PersistMode, Session};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::bus::{self, BusEvent};
use crate::hotkey::to_xdg_trigger;

enum Request {
    BindShortcut(String),
    Keys(Vec<i32>),
    Type(String),
    SetRestoreToken(Option<String>),
}

fn sender() -> &'static mpsc::UnboundedSender<Request> {
    static TX: OnceLock<mpsc::UnboundedSender<Request>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        thread::Builder::new()
            .name("portal".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
                match runtime {
                    Ok(rt) => rt.block_on(run(rx)),
                    Err(e) => log::error!("portal runtime: {e}"),
                }
            })
            .expect("spawning portal thread");
        tx
    })
}

pub fn bind_shortcut(spec: &str) {
    let _ = sender().send(Request::BindShortcut(to_xdg_trigger(spec)));
}

pub fn set_restore_token(token: Option<String>) {
    let _ = sender().send(Request::SetRestoreToken(token));
}

/// Presses `combo` (such as "Ctrl+V") in the focused window.
pub fn press(combo: &str) -> Result<()> {
    let codes = combo
        .split('+')
        .map(|k| evdev_code(k.trim()).with_context(|| format!("unknown key {k:?} in {combo:?}")))
        .collect::<Result<Vec<_>>>()?;
    let _ = sender().send(Request::Keys(codes));
    Ok(())
}

pub fn type_text(text: &str) {
    let _ = sender().send(Request::Type(text.to_string()));
}

struct Keyboard {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
}

async fn run(mut rx: mpsc::UnboundedReceiver<Request>) {
    // Unsandboxed apps must name themselves before the portal serves them.
    match crate::desktop::APP_ID.parse() {
        Ok(app_id) => {
            if let Err(e) = ashpd::register_host_app(app_id).await {
                log::warn!("registering with the portal: {e}");
            }
        }
        Err(e) => log::warn!("invalid app id: {e}"),
    }
    let mut shortcuts: Option<(GlobalShortcuts, Session<GlobalShortcuts>)> = None;
    let mut keyboard: Option<Keyboard> = None;
    let mut restore_token: Option<String> = None;
    while let Some(request) = rx.recv().await {
        match request {
            Request::BindShortcut(trigger) => {
                if shortcuts.is_some() {
                    // The desktop owns the binding once made; changes happen in its settings.
                    continue;
                }
                match bind(&trigger).await {
                    Ok(s) => shortcuts = Some(s),
                    Err(e) => {
                        log::warn!("global shortcut portal: {e:#}");
                        bus::send(BusEvent::Notice(
                            "The desktop did not grant a global shortcut. Bind `diktatoren toggle` to a key in your desktop settings.".into(),
                        ));
                    }
                }
            }
            Request::SetRestoreToken(token) => restore_token = token,
            Request::Keys(codes) => {
                let result = match ensure_keyboard(&mut keyboard, &restore_token).await {
                    Ok(k) => press_codes(k, &codes).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = result {
                    log::warn!("remote desktop portal: {e:#}");
                    keyboard = None;
                    if !fallback_keys(&codes) {
                        bus::send(BusEvent::Notice("Text copied. Press Ctrl+V to paste it.".into()));
                    }
                }
            }
            Request::Type(text) => {
                let result = match ensure_keyboard(&mut keyboard, &restore_token).await {
                    Ok(k) => type_keysyms(k, &text).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = result {
                    log::warn!("remote desktop portal: {e:#}");
                    keyboard = None;
                    bus::send(BusEvent::Notice("Could not type into the window; the text is on the clipboard.".into()));
                }
            }
        }
    }
}

async fn bind(trigger: &str) -> Result<(GlobalShortcuts, Session<GlobalShortcuts>)> {
    let proxy = GlobalShortcuts::new().await?;
    let session = proxy.create_session(Default::default()).await?;
    let shortcut = NewShortcut::new("dictate", "Start or stop dictation").preferred_trigger(trigger);
    proxy
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()?;
    let mut activated = proxy.receive_activated().await?;
    let mut deactivated = proxy.receive_deactivated().await?;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(a) = activated.next() => if a.shortcut_id() == "dictate" {
                    bus::send(BusEvent::HotkeyPressed);
                },
                Some(d) = deactivated.next() => if d.shortcut_id() == "dictate" {
                    bus::send(BusEvent::HotkeyReleased);
                },
                else => break,
            }
        }
    });
    log::info!("global shortcut bound through the portal ({trigger})");
    Ok((proxy, session))
}

/// Opens the remote-desktop session on first use.
async fn ensure_keyboard<'a>(
    keyboard: &'a mut Option<Keyboard>,
    restore_token: &Option<String>,
) -> Result<&'a Keyboard> {
    if keyboard.is_none() {
        let proxy = RemoteDesktop::new().await?;
        let session = proxy.create_session(Default::default()).await?;
        proxy
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(Some(DeviceType::Keyboard.into()))
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(restore_token.as_deref()),
            )
            .await?
            .response()?;
        let selected = proxy.start(&session, None, Default::default()).await?.response()?;
        if let Some(token) = selected.restore_token() {
            bus::send(BusEvent::PortalToken(token.to_string()));
        }
        *keyboard = Some(Keyboard { proxy, session });
    }
    Ok(keyboard.as_ref().unwrap())
}

async fn key(k: &Keyboard, code: i32, state: KeyState) -> Result<()> {
    k.proxy.notify_keyboard_keycode(&k.session, code, state, Default::default()).await?;
    Ok(())
}

async fn press_codes(k: &Keyboard, codes: &[i32]) -> Result<()> {
    for &c in codes {
        key(k, c, KeyState::Pressed).await?;
    }
    for &c in codes.iter().rev() {
        key(k, c, KeyState::Released).await?;
    }
    Ok(())
}

async fn type_keysyms(k: &Keyboard, text: &str) -> Result<()> {
    for ch in text.chars() {
        let sym = keysym(ch);
        for state in [KeyState::Pressed, KeyState::Released] {
            k.proxy.notify_keyboard_keysym(&k.session, sym, state, Default::default()).await?;
        }
    }
    Ok(())
}

fn keysym(ch: char) -> i32 {
    match ch {
        '\n' => 0xff0d,
        '\t' => 0xff09,
        c if (' '..='~').contains(&c) || ('\u{a0}'..='\u{ff}').contains(&c) => c as i32,
        c => 0x0100_0000 + c as i32,
    }
}

/// Command-line injectors for desktops without the RemoteDesktop portal.
fn fallback_keys(codes: &[i32]) -> bool {
    let ydotool: Vec<String> = codes
        .iter()
        .map(|c| format!("{c}:1"))
        .chain(codes.iter().rev().map(|c| format!("{c}:0")))
        .collect();
    std::process::Command::new("ydotool")
        .arg("key")
        .args(&ydotool)
        .status()
        .is_ok_and(|s| s.success())
}

/// Linux input event codes, which name physical keys and so do not depend on the layout.
pub fn evdev_code(key: &str) -> Option<i32> {
    let lower = key.to_ascii_lowercase();
    Some(match lower.as_str() {
        "ctrl" | "control" => 29,
        "shift" => 42,
        "alt" => 56,
        "super" | "meta" | "logo" | "win" | "cmd" => 125,
        "insert" | "ins" => 110,
        "enter" | "return" => 28,
        "space" => 57,
        "tab" => 15,
        _ => {
            let c = lower.chars().next().filter(|_| lower.len() == 1)?;
            const ROWS: [(&str, i32); 3] = [("qwertyuiop", 16), ("asdfghjkl", 30), ("zxcvbnm", 44)];
            ROWS.iter()
                .find_map(|(row, base)| row.find(c).map(|i| base + i as i32))?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evdev_codes() {
        assert_eq!(evdev_code("Ctrl"), Some(29));
        assert_eq!(evdev_code("V"), Some(47));
        assert_eq!(evdev_code("q"), Some(16));
        assert_eq!(evdev_code("m"), Some(50));
        assert_eq!(evdev_code("Shift"), Some(42));
        assert_eq!(evdev_code("Insert"), Some(110));
    }

    #[test]
    fn keysyms() {
        assert_eq!(keysym('a'), 0x61);
        assert_eq!(keysym('æ'), 0xe6);
        assert_eq!(keysym('€'), 0x0100_20ac);
        assert_eq!(keysym('\n'), 0xff0d);
    }
}
