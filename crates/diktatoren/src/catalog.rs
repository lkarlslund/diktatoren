//! Downloadable models, generated at build time from audio.cpp's model specs.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Family {
    pub family: String,
    pub name: String,
    pub description: String,
    pub tasks: Vec<String>,
    pub modes: Vec<String>,
    pub languages: Vec<String>,
    pub packages: Vec<Package>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub precision: Option<String>,
    pub directory: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub strip_prefix: Option<String>,
    pub repo: String,
    pub revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Asr,
    Vad,
    WakeWord,
}

impl Task {
    pub fn spec_name(self) -> &'static str {
        match self {
            Task::Asr => "asr",
            Task::Vad => "vad",
            Task::WakeWord => "wake_word",
        }
    }

    /// The C API task token.
    pub fn token(self) -> &'static str {
        match self {
            Task::Asr => "asr",
            Task::Vad => "vad",
            Task::WakeWord => "wake",
        }
    }
}

pub fn all() -> &'static [Family] {
    static CATALOG: OnceLock<Vec<Family>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/catalog.json")))
            .expect("embedded catalog")
    })
}

/// Families for `task` that this build of audio.cpp can load.
pub fn families(task: Task, available: &[String]) -> Vec<&'static Family> {
    all()
        .iter()
        .filter(|f| f.tasks.iter().any(|t| t == task.spec_name()))
        .filter(|f| available.iter().any(|a| a == &f.family))
        .collect()
}

pub fn find(package_id: &str) -> Option<(&'static Family, &'static Package)> {
    all().iter().find_map(|f| f.packages.iter().find(|p| p.id == package_id).map(|p| (f, p)))
}

impl Family {
    pub fn streams(&self) -> bool {
        self.modes.iter().any(|m| m == "streaming")
    }

    pub fn language_summary(&self) -> String {
        match self.languages.len() {
            0 => String::new(),
            1..=6 => self.languages.join(", "),
            n => format!("{n} languages"),
        }
    }
}

impl Package {
    /// Where each repository file lands locally, relative to the models directory.
    pub fn local_files(&self) -> Vec<(String, PathBuf)> {
        self.files
            .iter()
            .map(|remote| {
                let relative = self
                    .strip_prefix
                    .as_deref()
                    .and_then(|prefix| remote.strip_prefix(prefix))
                    .map(|rest| rest.trim_start_matches('/'))
                    .unwrap_or(remote);
                (remote.clone(), Path::new(&self.directory).join(relative))
            })
            .collect()
    }

    pub fn url(&self, remote: &str) -> String {
        format!("https://huggingface.co/{}/resolve/{}/{}", self.repo, self.revision, remote)
    }

    pub fn is_installed(&self, models_dir: &Path) -> bool {
        self.local_files().iter().all(|(_, local)| models_dir.join(local).is_file())
    }

    /// The path handed to audio.cpp: the GGUF file for single-file packages, otherwise the
    /// package directory.
    pub fn load_path(&self, models_dir: &Path) -> PathBuf {
        let files = self.local_files();
        match files.as_slice() {
            [(_, single)] => models_dir.join(single),
            _ => files
                .iter()
                .find(|(_, local)| local.extension().is_some_and(|e| e == "gguf"))
                .map(|(_, local)| models_dir.join(local))
                .unwrap_or_else(|| models_dir.join(&self.directory)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_speech_and_wake_models() {
        let (family, package) = find("parakeet_tdt_q8_0").expect("parakeet in catalog");
        assert_eq!(family.family, "parakeet_tdt");
        let files = package.local_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, Path::new("Parakeet-TDT-0.6B-v3-GGUF/parakeet-tdt-0.6b-v3-q8_0.gguf"));
        assert!(find("micro_wake_word_okay_nabu_f32").is_some());
    }
}
