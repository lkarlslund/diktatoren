//! Placing the dictation bar on KDE Plasma. Wayland does not let applications position their
//! windows, so a small KWin script does it: bottom centre of the active screen, above other
//! windows, and out of the taskbar and window switcher.

use std::time::Duration;

/// The title the bar window carries, so the script can find it.
pub const BAR_TITLE: &str = "Diktatoren bar";

pub fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_lowercase().contains("kde"))
}

/// Moves the bar into place once it is on screen.
pub fn place_bar() {
    if !is_kde() || !crate::hotkey::is_wayland() {
        return;
    }
    std::thread::spawn(|| {
        // Give KWin a moment to map the new window.
        std::thread::sleep(Duration::from_millis(150));
        if let Err(e) = run_script() {
            log::warn!("placing the dictation bar: {e:#}");
        }
    });
}

fn run_script() -> anyhow::Result<()> {
    let script = format!(
        r#"
const title = "{BAR_TITLE}";
for (const w of workspace.windowList()) {{
    if (w.caption !== title) continue;
    const area = workspace.clientArea(KWin.PlacementArea, workspace.activeScreen, workspace.currentDesktop);
    const g = w.frameGeometry;
    w.frameGeometry = {{
        x: Math.round(area.x + (area.width - g.width) / 2),
        y: Math.round(area.y + area.height - g.height - 24),
        width: g.width,
        height: g.height,
    }};
    w.keepAbove = true;
    w.skipTaskbar = true;
    w.skipSwitcher = true;
    w.skipPager = true;
}}
"#
    );
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = dir.join("diktatoren-place-bar.js");
    std::fs::write(&path, script)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let bus = zbus::Connection::session().await?;
        let scripting = zbus::Proxy::new(&bus, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting").await?;
        let plugin = "diktatoren-place-bar";
        let _: zbus::Result<bool> = scripting.call("unloadScript", &(plugin,)).await;
        let id: i32 = scripting.call("loadScript", &(path.to_string_lossy().as_ref(), plugin)).await?;
        let script_path = format!("/Scripting/Script{id}");
        let script = zbus::Proxy::new(&bus, "org.kde.KWin", script_path.as_str(), "org.kde.kwin.Script").await?;
        let _: () = script.call("run", &()).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _: zbus::Result<bool> = scripting.call("unloadScript", &(plugin,)).await;
        anyhow::Ok(())
    })?;
    let _ = std::fs::remove_file(path);
    Ok(())
}
