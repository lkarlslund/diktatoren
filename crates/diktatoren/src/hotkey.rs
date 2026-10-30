//! The global dictation hotkey.
//!
//! Windows, macOS and X11 grab the key directly. Wayland compositors do not allow that, so
//! there the XDG GlobalShortcuts portal binds it (the desktop asks the user to confirm the
//! first time), and `diktatoren toggle` can be bound to a key in the desktop settings as a
//! fallback.

use std::str::FromStr;

use anyhow::{Result, anyhow};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::bus::{self, BusEvent};

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    current: Option<HotKey>,
}

pub fn is_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

impl Hotkeys {
    /// Must be called on the main thread once the event loop runs.
    pub fn new() -> Self {
        if is_wayland() {
            return Hotkeys { manager: None, current: None };
        }
        GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
            bus::send(match event.state {
                HotKeyState::Pressed => BusEvent::HotkeyPressed,
                HotKeyState::Released => BusEvent::HotkeyReleased,
            });
        }));
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| log::warn!("global hotkeys unavailable: {e}"))
            .ok();
        Hotkeys { manager, current: None }
    }

    pub fn bind(&mut self, spec: &str) -> Result<()> {
        if is_wayland() && !spec.trim().is_empty() {
            #[cfg(target_os = "linux")]
            crate::portal::bind_shortcut(spec);
            return Ok(());
        }
        if is_wayland() {
            return Ok(());
        }
        let Some(manager) = &self.manager else {
            return Err(anyhow!("global hotkeys are not available on this system"));
        };
        if let Some(old) = self.current.take() {
            let _ = manager.unregister(old);
        }
        if spec.trim().is_empty() {
            return Ok(());
        }
        let hotkey = HotKey::from_str(spec).map_err(|e| anyhow!("hotkey {spec:?}: {e}"))?;
        manager.register(hotkey).map_err(|e| anyhow!("registering {spec}: {e}"))?;
        self.current = Some(hotkey);
        Ok(())
    }
}

/// Turns "Ctrl+Alt+Space" into the XDG shortcut notation "CTRL+ALT+space".
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn to_xdg_trigger(spec: &str) -> String {
    spec.split('+')
        .map(str::trim)
        .map(|part| match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "CTRL".to_string(),
            "alt" | "option" => "ALT".to_string(),
            "shift" => "SHIFT".to_string(),
            "super" | "meta" | "cmd" | "command" | "win" | "logo" => "LOGO".to_string(),
            "space" => "space".to_string(),
            key if key.len() == 1 => key.to_string(),
            _ => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    #[test]
    fn xdg_trigger() {
        assert_eq!(super::to_xdg_trigger("Ctrl+Alt+Space"), "CTRL+ALT+space");
        assert_eq!(super::to_xdg_trigger("Super+Shift+D"), "LOGO+SHIFT+d");
        assert_eq!(super::to_xdg_trigger("F9"), "F9");
    }
}
