//! Linux desktop integration. Portals only serve unsandboxed apps that register an app id
//! matching an installed desktop entry, so Diktatoren installs its own entry (pointing at the
//! running binary) when it is missing or points elsewhere.

use std::fs;
use std::path::PathBuf;

pub const APP_ID: &str = "io.github.lkarlslund.Diktatoren";

const ENTRY: &str = include_str!("../../../packaging/linux/io.github.lkarlslund.Diktatoren.desktop");
const ICON: &str = include_str!("../../../packaging/linux/io.github.lkarlslund.Diktatoren.svg");

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

/// Writes the desktop entry and icon to the user's data directory unless an entry for this
/// binary is already installed.
pub fn ensure_entry() {
    let Some(data) = data_home() else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    let exe = exe.to_string_lossy().into_owned();
    let entry_path = data.join("applications").join(format!("{APP_ID}.desktop"));
    let entry = ENTRY
        .lines()
        .map(|line| match line.strip_prefix("Exec=diktatoren") {
            Some(rest) => format!("Exec={exe}{rest}"),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    if fs::read_to_string(&entry_path).is_ok_and(|current| current == entry) {
        return;
    }
    let icon_path = data.join("icons/hicolor/scalable/apps").join(format!("{APP_ID}.svg"));
    let written = (|| -> std::io::Result<()> {
        fs::create_dir_all(entry_path.parent().unwrap())?;
        fs::create_dir_all(icon_path.parent().unwrap())?;
        fs::write(&entry_path, entry)?;
        fs::write(&icon_path, ICON)
    })();
    match written {
        Ok(()) => log::info!("installed desktop entry {}", entry_path.display()),
        Err(e) => log::warn!("could not install desktop entry: {e}"),
    }
}
