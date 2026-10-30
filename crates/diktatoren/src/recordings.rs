//! Microphone recordings of dictation sessions, kept only while `record_sessions` is on, for
//! replaying with `diktatoren feed` when something goes wrong.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::audio::SAMPLE_RATE;

/// How many recordings to keep; older ones are deleted.
const KEEP: usize = 20;

pub fn dir() -> PathBuf {
    let state = crate::config::project_dirs()
        .state_dir()
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::project_dirs().data_dir().to_path_buf());
    state.join("recordings")
}

pub fn save(audio: &[f32]) -> Result<()> {
    if audio.len() < SAMPLE_RATE as usize / 2 {
        return Ok(());
    }
    let dir = dir();
    fs::create_dir_all(&dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let path = dir.join(format!("session-{stamp}.wav"));
    let spec = hound::WavSpec { channels: 1, sample_rate: SAMPLE_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut writer = hound::WavWriter::create(&path, spec)?;
    for &s in audio {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    log::info!("recorded {:.1}s to {}", audio.len() as f32 / SAMPLE_RATE as f32, path.display());
    prune(&dir)
}

fn prune(dir: &std::path::Path) -> Result<()> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("session-")))
        .collect();
    files.sort();
    let excess = files.len().saturating_sub(KEEP);
    for old in &files[..excess] {
        let _ = fs::remove_file(old);
    }
    Ok(())
}
