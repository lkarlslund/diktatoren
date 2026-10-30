mod audio;
mod catalog;
mod commands;
mod config;
mod document;
mod download;
mod engine;
mod resample;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};

use crate::catalog::Task;
use crate::config::Settings;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let settings = Settings::load();
    match args.first().map(String::as_str) {
        Some("models") => list_models(&settings),
        Some("download") => {
            let id = args.get(1).context("usage: diktatoren download <package-id>")?;
            download_package(&settings, id)
        }
        Some("transcribe") => {
            let wav = args.get(1).context("usage: diktatoren transcribe <file.wav> [package-id]")?;
            let package = args.get(2).cloned().unwrap_or(settings.asr_package.clone());
            transcribe(&settings, Path::new(wav), &package)
        }
        Some("listen") => listen(settings, args.get(1)),
        _ => {
            println!("usage: diktatoren models | download <id> | transcribe <wav> [id]");
            Ok(())
        }
    }
}

fn available_families() -> Result<Vec<String>> {
    Ok(audiocpp::Registry::new()?.families())
}

fn list_models(settings: &Settings) -> Result<()> {
    let available = available_families()?;
    let dir = settings.models_dir();
    for task in [Task::Asr, Task::WakeWord, Task::Vad] {
        println!("== {}", task.spec_name());
        for family in catalog::families(task, &available) {
            println!("  {} ({}) [{}]", family.name, family.family, family.language_summary());
            for p in &family.packages {
                let mark = if p.is_installed(&dir) { "*" } else { " " };
                println!("    {mark} {}  {}", p.id, p.name);
            }
        }
    }
    Ok(())
}

fn download_package(settings: &Settings, id: &str) -> Result<()> {
    let (_, package) = catalog::find(id).with_context(|| format!("unknown package {id}"))?;
    let cancel = AtomicBool::new(false);
    let mut last = 0u64;
    download::install(package, &settings.models_dir(), &cancel, |p| {
        if p.done - last > 20_000_000 || Some(p.done) == p.total {
            last = p.done;
            let total = p.total.map(download::human_size).unwrap_or_else(|| "?".into());
            eprintln!("{} / {total}", download::human_size(p.done));
        }
    })?;
    println!("installed {id} in {}", settings.models_dir().display());
    Ok(())
}

fn transcribe(settings: &Settings, wav: &Path, id: &str) -> Result<()> {
    let (family, package) = catalog::find(id).with_context(|| format!("unknown package {id}"))?;
    let dir = settings.models_dir();
    if !package.is_installed(&dir) {
        bail!("{id} is not installed; run `diktatoren download {id}`");
    }
    let audio = audio::read_wav(wav)?;

    let registry = audiocpp::Registry::new()?;
    let started = std::time::Instant::now();
    let model = audiocpp::Model::load(&registry, &package.load_path(&dir), Some(&family.family))?;
    let backend = audiocpp::Backend {
        name: settings.backend.clone(),
        device: 0,
        threads: settings.threads(),
    };
    let mut session =
        audiocpp::Session::new(&model, Task::Asr.token(), audiocpp::Mode::Offline, &backend, &[])?;
    eprintln!("loaded in {:.2?}", started.elapsed());
    let started = std::time::Instant::now();
    let result = session.run(&audio, 16_000, &[])?;
    eprintln!("transcribed {:.1}s of audio in {:.2?}", audio.len() as f32 / 16_000.0, started.elapsed());
    println!("{}", result.text.unwrap_or_default());
    Ok(())
}

/// Headless dictation for testing: listens until Ctrl+C and prints each utterance, applying
/// voice commands to a document.
fn listen(mut settings: Settings, input: Option<&String>) -> Result<()> {
    if let Some(input) = input {
        settings.input_device = format!("file:{input}");
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let sink: engine::Sink = std::sync::Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let wake = settings.wake_word_enabled;
    let handle = engine::spawn(settings.clone(), sink);
    if !wake {
        handle.send(engine::Command::Listen { from_wake: false });
    }
    let mut doc = document::Document::default();
    for event in rx {
        match event {
            engine::Event::Utterance(text) => {
                match commands::parse(&text, settings.require_command_prefix) {
                    Some(c) => eprintln!("[command {c:?} -> {:?}]", doc.apply(&c)),
                    None => doc.dictate(&text),
                }
                println!("{}", doc.text());
            }
            engine::Event::Preview(text) => eprintln!("… {text}"),
            engine::Event::Level(_) => {}
            other => eprintln!("[{other:?}]"),
        }
    }
    Ok(())
}
