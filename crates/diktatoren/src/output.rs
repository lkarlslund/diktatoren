//! Hands the finished text over: to the clipboard, or into the window that had focus before
//! the dictation window opened.

use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use enigo::{Direction, Enigo, Key, Keyboard};

use crate::config::{InsertMethod, Settings};

/// The window to return focus to, captured when dictation starts.
#[derive(Debug, Clone, Default)]
pub struct Target {
    #[cfg(windows)]
    hwnd: isize,
    #[cfg(target_os = "macos")]
    app: Option<String>,
}

impl Target {
    pub fn capture() -> Self {
        #[cfg(windows)]
        {
            let hwnd = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
            Target { hwnd: hwnd as isize }
        }
        #[cfg(target_os = "macos")]
        {
            let app = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to get name of first application process whose frontmost is true"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty());
            Target { app }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        Target {}
    }

    fn restore(&self) {
        #[cfg(windows)]
        if self.hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(self.hwnd as _);
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(app) = &self.app {
            let _ = std::process::Command::new("osascript")
                .args(["-e", &format!("tell application \"{}\" to activate", app.replace('"', ""))])
                .status();
        }
    }
}

pub struct Clipboard {
    inner: Option<arboard::Clipboard>,
}

impl Clipboard {
    pub fn new() -> Self {
        let inner = arboard::Clipboard::new()
            .map_err(|e| log::warn!("clipboard unavailable: {e}"))
            .ok();
        Clipboard { inner }
    }

    pub fn get(&mut self) -> Option<String> {
        self.inner.as_mut()?.get_text().ok()
    }

    pub fn set(&mut self, text: &str) -> Result<()> {
        let clipboard = self.inner.as_mut().ok_or_else(|| anyhow!("no clipboard"))?;
        clipboard.set_text(text.to_string()).context("setting clipboard")
    }
}

/// Sends the text into the target window. Runs after the dictation window has closed, so
/// focus has gone back to the target.
pub fn deliver(text: &str, target: &Target, settings: &Settings) -> Result<()> {
    target.restore();
    thread::sleep(Duration::from_millis(60));
    #[cfg(target_os = "linux")]
    if crate::hotkey::is_wayland() {
        return match settings.insert_method {
            InsertMethod::Paste => crate::portal::press(&settings.paste_shortcut),
            InsertMethod::Type => {
                crate::portal::type_text(text);
                Ok(())
            }
        };
    }
    let mut enigo = Enigo::new(&enigo::Settings::default()).map_err(|e| anyhow!("keyboard: {e}"))?;
    match settings.insert_method {
        InsertMethod::Paste => press_combo(&mut enigo, &settings.paste_shortcut),
        InsertMethod::Type => enigo.text(text).map_err(|e| anyhow!("typing: {e}")),
    }
}

fn press_combo(enigo: &mut Enigo, combo: &str) -> Result<()> {
    let keys = combo
        .split('+')
        .map(|k| enigo_key(k.trim()).ok_or_else(|| anyhow!("unknown key {k:?} in {combo:?}")))
        .collect::<Result<Vec<_>>>()?;
    for &k in &keys {
        enigo.key(k, Direction::Press).map_err(|e| anyhow!("{e}"))?;
    }
    for &k in keys.iter().rev() {
        enigo.key(k, Direction::Release).map_err(|e| anyhow!("{e}"))?;
    }
    Ok(())
}

fn enigo_key(name: &str) -> Option<Key> {
    Some(match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Key::Control,
        "shift" => Key::Shift,
        "alt" | "option" => Key::Alt,
        "cmd" | "command" | "super" | "meta" | "win" => Key::Meta,
        "insert" | "ins" => {
            #[cfg(not(target_os = "macos"))]
            {
                Key::Insert
            }
            #[cfg(target_os = "macos")]
            return None;
        }
        "enter" | "return" => Key::Return,
        other if other.chars().count() == 1 => Key::Unicode(other.chars().next()?),
        _ => return None,
    })
}
