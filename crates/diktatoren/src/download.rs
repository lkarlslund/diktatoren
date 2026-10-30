//! Resumable model downloads from Hugging Face.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

use crate::catalog::Package;

#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    pub done: u64,
    pub total: Option<u64>,
}

/// Downloads every file of `package` into `models_dir`. Partial files are kept as `.part`
/// and resumed on the next attempt.
pub fn install(
    package: &Package,
    models_dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<()> {
    let files = package.local_files();
    let mut totals: Vec<Option<u64>> = vec![None; files.len()];
    let mut done_before = 0u64;
    for (index, (remote, local)) in files.iter().enumerate() {
        let dest = models_dir.join(local);
        if dest.is_file() {
            let len = fs::metadata(&dest)?.len();
            done_before += len;
            totals[index] = Some(len);
            continue;
        }
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir)?;
        }
        let part = dest.with_extension(format!(
            "{}.part",
            dest.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
        ));
        let resume_from = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let url = package.url(remote);
        let mut request = ureq::get(&url).header("User-Agent", concat!("diktatoren/", env!("CARGO_PKG_VERSION")));
        if resume_from > 0 {
            request = request.header("Range", &format!("bytes={resume_from}-"));
        }
        let response = request.call().with_context(|| format!("fetching {url}"))?;
        let resumed = response.status() == 206;
        let length = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let start = if resumed { resume_from } else { 0 };
        totals[index] = length.map(|l| l + start);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)
            .with_context(|| format!("opening {}", part.display()))?;
        let mut reader = response.into_body().into_reader();
        let mut buf = vec![0u8; 1 << 16];
        let mut written = start;
        let total = || -> Option<u64> {
            totals.iter().copied().sum::<Option<u64>>()
        };
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("download cancelled");
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            written += n as u64;
            progress(Progress { done: done_before + written, total: total() });
        }
        file.sync_all()?;
        drop(file);
        if let Some(expected) = totals[index]
            && written != expected
        {
            bail!("{remote}: got {written} of {expected} bytes");
        }
        fs::rename(&part, &dest)?;
        done_before += written;
    }
    progress(Progress { done: done_before, total: Some(done_before) });
    Ok(())
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}
