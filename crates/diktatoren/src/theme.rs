//! Colours that match the desktop: the KDE colour scheme, GNOME's style and accent, or the
//! light/dark mode and accent colour of Windows and macOS. Elsewhere iced follows the system's
//! light or dark preference on its own.

use iced::theme::Palette;
use iced::{Color, Theme};

/// The desktop's theme, read fresh so that changes apply to the next window.
pub fn current() -> Option<Theme> {
    let palette = platform_palette()?;
    Some(Theme::custom("Desktop", palette))
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

fn hex(value: u32) -> Color {
    rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// Background, text and accent, with conventional status colours.
fn palette(background: Color, text: Color, primary: Color) -> Palette {
    Palette {
        background,
        text,
        primary,
        success: hex(0x27ae60),
        warning: hex(0xf67400),
        danger: hex(0xda4453),
    }
}

#[cfg(target_os = "linux")]
fn platform_palette() -> Option<Palette> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase();
    if desktop.contains("kde") {
        let home = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
        let text = std::fs::read_to_string(home.join("kdeglobals")).ok()?;
        return kde_palette(&text);
    }
    if desktop.contains("gnome") {
        return gnome_palette();
    }
    None
}

/// Reads a KDE colour scheme (`kdeglobals`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn kde_palette(ini: &str) -> Option<Palette> {
    let mut section = "";
    let mut get = std::collections::HashMap::new();
    for line in ini.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
        } else if let Some((key, value)) = line.split_once('=') {
            get.insert((section.to_string(), key.trim().to_string()), value.trim().to_string());
        }
    }
    let color = |section: &str, key: &str| -> Option<Color> {
        let value = get.get(&(section.to_string(), key.to_string()))?;
        let parts: Vec<u8> = value.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (parts.len() >= 3).then(|| rgb(parts[0], parts[1], parts[2]))
    };
    let mut p = palette(
        color("Colors:Window", "BackgroundNormal")?,
        color("Colors:Window", "ForegroundNormal")?,
        color("Colors:Selection", "BackgroundNormal")?,
    );
    if let Some(c) = color("Colors:View", "ForegroundPositive") {
        p.success = c;
    }
    if let Some(c) = color("Colors:View", "ForegroundNeutral") {
        p.warning = c;
    }
    if let Some(c) = color("Colors:View", "ForegroundNegative") {
        p.danger = c;
    }
    Some(p)
}

#[cfg(target_os = "linux")]
fn gnome_palette() -> Option<Palette> {
    let get = |key: &str| -> String {
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", key])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_string())
            .unwrap_or_default()
    };
    let dark = get("color-scheme") == "prefer-dark";
    // libadwaita's accent colours.
    let accent = match get("accent-color").as_str() {
        "teal" => 0x2190a4,
        "green" => 0x3a944a,
        "yellow" => 0xc88800,
        "orange" => 0xed5b00,
        "red" => 0xe62d42,
        "pink" => 0xd56199,
        "purple" => 0x9141ac,
        "slate" => 0x6f8396,
        _ => 0x3584e4,
    };
    Some(if dark {
        palette(hex(0x222226), hex(0xffffff), hex(accent))
    } else {
        palette(hex(0xfafafb), hex(0x1e1e22), hex(accent))
    })
}

#[cfg(windows)]
fn platform_palette() -> Option<Palette> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let dword = |key: &str, value: &str| -> Option<u32> {
        let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
        let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let mut data = 0u32;
        let mut size = 4u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        (status == 0).then_some(data)
    };
    let light = dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme")
        .is_none_or(|v| v != 0);
    // Stored as 0xAABBGGRR.
    let accent = dword(r"Software\Microsoft\Windows\DWM", "AccentColor")
        .map(|abgr| rgb(abgr as u8, (abgr >> 8) as u8, (abgr >> 16) as u8))
        .unwrap_or(hex(0x0067c0));
    Some(if light {
        palette(hex(0xf3f3f3), hex(0x1b1b1b), accent)
    } else {
        palette(hex(0x202020), hex(0xffffff), accent)
    })
}

#[cfg(target_os = "macos")]
fn platform_palette() -> Option<Palette> {
    let read = |key: &str| -> Option<String> {
        let out = std::process::Command::new("defaults").args(["read", "-g", key]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let dark = read("AppleInterfaceStyle").is_some_and(|s| s == "Dark");
    let accent = match read("AppleAccentColor").and_then(|s| s.parse::<i32>().ok()) {
        Some(-1) => 0x8c8c8c,
        Some(0) => 0xff3b30,
        Some(1) => 0xff9500,
        Some(2) => 0xffcc00,
        Some(3) => 0x28cd41,
        Some(5) => 0xaf52de,
        Some(6) => 0xff2d55,
        _ => 0x007aff,
    };
    Some(if dark {
        palette(hex(0x1e1e1e), hex(0xe5e5e7), hex(accent))
    } else {
        palette(hex(0xececec), hex(0x1d1d1f), hex(accent))
    })
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn platform_palette() -> Option<Palette> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_kde_colour_scheme() {
        let ini = "[Colors:Selection]\nBackgroundNormal=61,174,233\n\n[Colors:View]\nForegroundNegative=218,68,83\n\n[Colors:Window]\nBackgroundNormal=32,35,38\nForegroundNormal=252,252,252\n";
        let p = kde_palette(ini).unwrap();
        assert_eq!(p.background, rgb(32, 35, 38));
        assert_eq!(p.text, rgb(252, 252, 252));
        assert_eq!(p.primary, rgb(61, 174, 233));
        assert_eq!(p.danger, rgb(218, 68, 83));
        assert!(kde_palette("[General]\nName=x\n").is_none());
    }
}
