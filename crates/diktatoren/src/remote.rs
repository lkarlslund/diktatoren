//! Speech recognition on an audio.cpp server (`audiocpp_server`), for models that live there
//! rather than in this process.

use std::io::Cursor;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::audio::SAMPLE_RATE;

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into()
}

fn endpoint(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim().trim_end_matches('/'), path)
}

/// 16-bit mono WAV at the engine's sample rate.
fn wav(samples: &[f32]) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut out, spec)?;
    for &s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(out.into_inner())
}

/// Transcribes `samples` (16 kHz mono) with `model` on the server at `base`.
pub fn transcribe(base: &str, model: &str, language: &str, samples: &[f32]) -> Result<String> {
    let boundary = format!("diktatoren{:016x}", rand_suffix());
    let mut body = Vec::new();
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    };
    field("model", model);
    if !language.is_empty() {
        field("language", language);
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&wav(samples)?);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let url = endpoint(base, "v1/audio/transcriptions");
    let response = agent()
        .post(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send(&body[..])
        .with_context(|| format!("contacting {base}"))?;
    let status = response.status();
    let text = response.into_body().read_to_string()?;
    if !status.is_success() {
        return Err(anyhow!("{base}: {status}: {}", text.trim()));
    }
    let json: serde_json::Value = serde_json::from_str(&text).context("reading server reply")?;
    json["text"].as_str().map(str::to_string).ok_or_else(|| anyhow!("server reply has no text"))
}

/// Speech recognition models the server offers.
pub fn asr_models(base: &str) -> Result<Vec<String>> {
    let url = endpoint(base, "v1/models");
    let body = agent()
        .get(&url)
        .call()
        .with_context(|| format!("contacting {base}"))?
        .into_body()
        .read_to_string()?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    Ok(json["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["task"] == "asr")
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect())
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}
