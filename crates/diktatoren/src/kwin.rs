//! The dictation bar on KDE Plasma. Wayland lets applications neither place their windows nor
//! open one without it taking focus, so a KWin script does both: it is loaded before the bar
//! opens, remembers the active window, places the bar at the bottom centre of the screen, and
//! hands the focus straight back, also after clicks on the bar. It is unloaded with the bar.

use std::time::Duration;

/// The title the bar window carries, so the script can find it (the app uses the same).
const BAR_TITLE: &str = "Diktatoren bar";
const PLUGIN: &str = "diktatoren-bar";

fn applies() -> bool {
    crate::hotkey::is_wayland()
        && std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_lowercase().contains("kde"))
}

fn script() -> String {
    format!(
        r#"
const title = "{BAR_TITLE}";
const previous = workspace.activeWindow;
function giveBack() {{
    if (previous && !previous.deleted) workspace.activeWindow = previous;
}}
function hook(w) {{
    if (w.caption !== title) return;
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
    giveBack();
    w.activeChanged.connect(() => {{ if (w.active) giveBack(); }});
}}
for (const w of workspace.windowList()) hook(w);
workspace.windowAdded.connect(hook);
"#
    )
}

async fn scripting(bus: &zbus::Connection) -> zbus::Result<zbus::Proxy<'_>> {
    zbus::Proxy::new(bus, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting").await
}

/// Loads the script; call before opening the bar. Waits briefly so it is running in time.
pub fn prepare_bar() {
    if !applies() {
        return;
    }
    let done = std::thread::spawn(|| {
        if let Err(e) = load() {
            log::warn!("preparing the dictation bar: {e:#}");
        }
    });
    let deadline = std::time::Instant::now() + Duration::from_millis(400);
    while !done.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Unloads the script once the bar has closed.
pub fn release_bar() {
    if !applies() {
        return;
    }
    std::thread::spawn(|| {
        let result = runtime().and_then(|rt| {
            rt.block_on(async {
                let bus = zbus::Connection::session().await?;
                let _: zbus::Result<bool> = scripting(&bus).await?.call("unloadScript", &(PLUGIN,)).await;
                anyhow::Ok(())
            })
        });
        if let Err(e) = result {
            log::warn!("releasing the dictation bar script: {e:#}");
        }
    });
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread().enable_all().build()?)
}

fn load() -> anyhow::Result<()> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = dir.join("diktatoren-bar.js");
    std::fs::write(&path, script())?;
    runtime()?.block_on(async {
        let bus = zbus::Connection::session().await?;
        let scripting = scripting(&bus).await?;
        let _: zbus::Result<bool> = scripting.call("unloadScript", &(PLUGIN,)).await;
        let id: i32 = scripting.call("loadScript", &(path.to_string_lossy().as_ref(), PLUGIN)).await?;
        let script_path = format!("/Scripting/Script{id}");
        let script = zbus::Proxy::new(&bus, "org.kde.KWin", script_path.as_str(), "org.kde.kwin.Script").await?;
        let _: () = script.call("run", &()).await?;
        anyhow::Ok(())
    })
}
