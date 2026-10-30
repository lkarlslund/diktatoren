//! Tidying dictated text with a language model on an OpenAI-compatible server: punctuation and
//! capitals, and optionally fixing words the recogniser misheard.
//!
//! The model's answer is checked before it is used: in punctuation mode the words must be
//! unchanged, and when fixing, only a few may differ. Anything else is treated as a rewrite and
//! dropped.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiMode {
    Off,
    /// Punctuation and capital letters only.
    Punctuate,
    /// Also fix words the recogniser clearly misheard.
    Correct,
}

impl std::fmt::Display for AiMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AiMode::Off => "Off",
            AiMode::Punctuate => "Punctuation only",
            AiMode::Correct => "Punctuation and fixes",
        })
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

fn endpoint(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim().trim_end_matches('/'), path)
}

/// Models the server offers.
pub fn models(base: &str) -> Result<Vec<String>> {
    let body = agent()
        .get(&endpoint(base, "v1/models"))
        .call()
        .with_context(|| format!("contacting {base}"))?
        .into_body()
        .read_to_string()?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    Ok(json["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect())
}

fn instructions(mode: AiMode, language: &str) -> String {
    let language = if language.is_empty() { "speech recognition".to_string() } else { format!("{language} speech recognition") };
    let task = match mode {
        AiMode::Correct => "Fix words the recogniser clearly misheard (sound-alikes that make no sense in context), put back small words it dropped (such as \"at\", \"og\", \"det\"), and add punctuation and capital letters. Do not rephrase, shorten, add content or change the style; keep every word that makes sense.",
        _ => "Add punctuation and capital letters. Keep every word exactly as given, in the same order; do not add, remove, correct or translate words.",
    };
    format!(
        "You clean up text from {language}. {task} The text may continue earlier text, which is given for context only and must not be repeated. Reply with the cleaned-up text only."
    )
}

/// Tidies `text`, which follows `context` in the document.
pub fn tidy(base: &str, model: &str, mode: AiMode, language: &str, context: &str, text: &str) -> Result<String> {
    let user = if context.trim().is_empty() {
        text.to_string()
    } else {
        format!("Earlier text (context only): {}\n\nText to clean up: {text}", context.trim())
    };
    let request = serde_json::json!({
        "model": model,
        "temperature": 0,
        "max_tokens": 50 + text.len(),
        "reasoning_effort": "none",
        "messages": [
            { "role": "system", "content": instructions(mode, language) },
            { "role": "user", "content": user },
        ],
    });
    let response = agent()
        .post(&endpoint(base, "v1/chat/completions"))
        .config()
        .http_status_as_error(false)
        .build()
        .header("Content-Type", "application/json")
        .send(request.to_string().as_bytes())
        .with_context(|| format!("contacting {base}"))?;
    let status = response.status();
    let body = response.into_body().read_to_string()?;
    if !status.is_success() {
        bail!("{base}: {status}: {}", body.trim());
    }
    let json: serde_json::Value = serde_json::from_str(&body).context("reading the model's reply")?;
    let answer = json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("the model's reply has no text"))?;
    let cleaned = clean_answer(answer);
    check(mode, text, &cleaned)?;
    Ok(cleaned)
}

/// Drops reasoning blocks and the quoting or labels models like to add.
fn clean_answer(answer: &str) -> String {
    let mut s = answer.to_string();
    while let (Some(a), Some(b)) = (s.find("<think>"), s.find("</think>")) {
        if b < a {
            break;
        }
        s.replace_range(a..b + "</think>".len(), "");
    }
    let s = s.trim();
    let s = s.strip_prefix("Text to clean up:").unwrap_or(s).trim();
    s.trim_matches(|c| c == '"' || c == '“' || c == '”').trim().to_string()
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect()
}

fn word_distance(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            row.push((prev[j] + usize::from(x != y)).min(prev[j + 1] + 1).min(row[j] + 1));
        }
        prev = row;
    }
    prev[b.len()]
}

/// Rejects answers that are rewrites rather than tidy-ups.
fn check(mode: AiMode, original: &str, answer: &str) -> Result<()> {
    let (before, after) = (words(original), words(answer));
    if after.is_empty() {
        bail!("the model returned no text");
    }
    let changed = word_distance(&before, &after);
    let allowed = match mode {
        AiMode::Correct => (before.len() / 3).max(2),
        _ => 0,
    };
    if changed > allowed {
        bail!("the model changed {changed} of {} words", before.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_mode_keeps_the_words() {
        assert!(check(AiMode::Punctuate, "kære ole tak for sidst", "Kære Ole, tak for sidst.").is_ok());
        assert!(check(AiMode::Punctuate, "husket tager kaffe med", "Husk at tage kaffe med.").is_err());
    }

    #[test]
    fn correct_mode_allows_a_few_fixes_but_no_rewrites() {
        let raw = "vi mødes på mandag klokken ti husket tager et kaffe med";
        assert!(check(AiMode::Correct, raw, "Vi mødes på mandag klokken ti. Husk at tage et kaffe med.").is_ok());
        assert!(check(AiMode::Correct, raw, "Lad os ses mandag kl. 10, og tag gerne kaffe med.").is_err());
        assert!(check(AiMode::Correct, raw, "").is_err());
    }

    #[test]
    fn cleans_model_noise() {
        assert_eq!(clean_answer("<think>hmm</think>\n\"Hej Ole.\""), "Hej Ole.");
        assert_eq!(clean_answer("Text to clean up: Hej."), "Hej.");
    }
}
