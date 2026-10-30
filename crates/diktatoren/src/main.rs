//! Diktatoren: local on-demand dictation.

// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod audio;
mod bus;
mod catalog;
mod commands;
mod config;
mod document;
mod download;
mod engine;
mod hotkey;
mod icon;
mod ipc;
mod output;
#[cfg(target_os = "linux")]
mod portal;
mod resample;
mod tray;
mod wake;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};

use crate::catalog::Task;
use crate::config::Settings;

const USAGE: &str = "\
usage: diktatoren [command]

Without a command, starts Diktatoren in the tray (or opens the settings of the one running).

Control a running instance:
  toggle | start | stop     start or stop dictation
  insert | copy | cancel    finish the open dictation
  settings | quit

Models and testing:
  models                    list downloadable models (* = installed)
  download <package-id>     download a model
  transcribe <wav> [id]     transcribe a WAV file
  listen [wav]              dictate in the terminal, from the microphone or a WAV file
";

fn main() -> Result<()> {
    // Release builds on Windows have no console of their own; borrow the terminal's so
    // command-line use still prints.
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,diktatoren=info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let settings = Settings::load();
    let command = args.first().map(String::as_str);
    match command {
        None => run_app(settings, None, "settings"),
        Some(word) if ipc::parse(word).is_some() => run_app(settings, ipc::parse(word), word),
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
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

/// Hands `command` to a running instance, or becomes the instance.
fn run_app(settings: Settings, action: Option<bus::Action>, command: &str) -> Result<()> {
    if ipc::send_to_running(command) {
        return Ok(());
    }
    if action == Some(bus::Action::Quit) {
        return Ok(());
    }
    if let Err(e) = ipc::serve() {
        log::warn!("control socket unavailable: {e:#}");
    }
    // A bare start shows nothing until the hotkey or tray is used.
    let initial = action.filter(|a| *a != bus::Action::Settings || command != "settings" || std::env::args().len() > 1);
    app::run(settings, initial).context("running the user interface")
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
