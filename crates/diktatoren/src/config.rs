//! Settings, persisted as TOML in the platform config directory.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::commands::{self, Language};

pub fn project_dirs() -> ProjectDirs {
    ProjectDirs::from("", "", "diktatoren").expect("no home directory")
}

/// `DIKTATOREN_CONFIG` overrides the location, which keeps test runs away from real settings.
pub fn config_path() -> PathBuf {
    std::env::var_os("DIKTATOREN_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_dirs().config_dir().join("config.toml"))
}

pub fn default_models_dir() -> PathBuf {
    project_dirs().data_dir().join("models")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    /// Press once to start listening, again to stop.
    Toggle,
    /// Listen while the key is held down.
    PushToTalk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishAction {
    Insert,
    Copy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertMethod {
    /// Put the text on the clipboard and send the paste shortcut.
    Paste,
    /// Type the text as synthetic key presses.
    Type,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub hotkey: String,
    pub hotkey_mode: HotkeyMode,
    pub wake_word_enabled: bool,
    /// A wake-word model package, or "phrase" for `wake_phrase`.
    pub wake_word_package: String,
    pub wake_phrase: String,
    pub wake_word_threshold: f32,
    /// The audio.cpp server that recognises speech, such as `http://127.0.0.1:8099`.
    pub asr_server: String,
    pub asr_server_model: String,
    /// Language code sent with each request, or empty for the server model's default.
    pub language: String,
    /// Silence that ends an utterance.
    pub utterance_silence_ms: u32,
    /// After a wake word, stop listening when nothing has been said for this long.
    pub wake_listen_timeout_s: u32,
    pub input_device: String,
    pub enter_action: FinishAction,
    pub insert_method: InsertMethod,
    pub paste_shortcut: String,
    pub restore_clipboard: bool,
    pub voice_commands: bool,
    pub command_language: Language,
    /// The edited phrase lists when `command_language` is custom, keyed by command.
    pub command_phrases: BTreeMap<String, String>,
    pub models_dir: Option<PathBuf>,
    /// Lets the Wayland remote-desktop portal skip its permission prompt after the first use.
    pub portal_restore_token: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hotkey: if cfg!(target_os = "macos") { "Cmd+Shift+Space" } else { "Ctrl+Alt+Space" }
                .into(),
            hotkey_mode: HotkeyMode::Toggle,
            wake_word_enabled: false,
            wake_word_package: crate::wake::PHRASE.into(),
            wake_phrase: "Hey Diktatoren".into(),
            wake_word_threshold: 0.0,
            asr_server: String::new(),
            asr_server_model: String::new(),
            language: String::new(),
            utterance_silence_ms: 700,
            wake_listen_timeout_s: 6,
            input_device: String::new(),
            enter_action: FinishAction::Insert,
            insert_method: InsertMethod::Paste,
            paste_shortcut: if cfg!(target_os = "macos") { "Cmd+V" } else { "Ctrl+V" }.into(),
            restore_clipboard: true,
            voice_commands: true,
            command_language: Language::from_locale(),
            command_phrases: BTreeMap::new(),
            models_dir: None,
            portal_restore_token: None,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring unreadable {}: {e}", path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }

    /// The phrases in force: a preset, or the custom lists filled up from English.
    pub fn command_phrases(&self) -> BTreeMap<String, String> {
        match self.command_language {
            Language::Custom => {
                let mut phrases = commands::preset(Language::English);
                phrases.extend(self.command_phrases.clone());
                phrases
            }
            preset => commands::preset(preset),
        }
    }

    pub fn grammar(&self) -> commands::Grammar {
        commands::Grammar::new(&self.command_phrases())
    }

    pub fn uses_server(&self) -> bool {
        !self.asr_server.trim().is_empty() && !self.asr_server_model.trim().is_empty()
    }

    pub fn models_dir(&self) -> PathBuf {
        self.models_dir.clone().unwrap_or_else(default_models_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_fills_missing_fields() {
        let settings = Settings::default();
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
        let partial: Settings = toml::from_str("hotkey = \"F9\"").unwrap();
        assert_eq!(partial.hotkey, "F9");
        assert_eq!(partial.wake_phrase, settings.wake_phrase);
    }
}
