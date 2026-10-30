//! Diktatoren: local on-demand dictation.

// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod a11y;
#[cfg(not(target_os = "linux"))]
#[path = "a11y_stub.rs"]
mod a11y;
mod ai;
mod app;
mod audio;
mod bus;
mod catalog;
mod commands;
mod config;
#[cfg(target_os = "linux")]
mod desktop;
mod document;
mod download;
mod engine;
mod hotkey;
mod i18n;
mod icon;
mod ipc;
mod output;
#[cfg(target_os = "linux")]
mod portal;
mod remote;
mod resample;
mod theme;
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
  models                    list downloadable wake-word models (* = installed)
  download <package-id>     download a wake-word model
  transcribe <wav>          transcribe a WAV file on the speech server
  listen [wav]              dictate in the terminal, from the microphone or a WAV file
  mic-test                  show the microphone level for a few seconds (records nothing)
  feed <wav>                make the running instance dictate from a WAV file (testing)
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,diktatoren=info"))
        // The D-Bus layer warns about routine portal property lookups.
        .filter_module("zbus", log::LevelFilter::Error)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let settings = Settings::load();
    i18n::set_language(settings.command_language);
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
            let wav = args.get(1).context("usage: diktatoren transcribe <file.wav>")?;
            transcribe(&settings, Path::new(wav))
        }
        Some("listen") => listen(settings, args.get(1)),
        Some("mic-test") => mic_test(&settings),
        Some("feed") => {
            let wav = args.get(1).context("usage: diktatoren feed <file.wav>")?;
            let path = std::fs::canonicalize(wav).with_context(|| format!("reading {wav}"))?;
            if !ipc::send_to_running(&format!("feed {}", path.display())) {
                bail!("Diktatoren is not running");
            }
            Ok(())
        }
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
    #[cfg(target_os = "linux")]
    desktop::ensure_entry();
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
    for task in [Task::WakeWord] {
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

fn transcribe(settings: &Settings, wav: &Path) -> Result<()> {
    if !settings.uses_server() {
        bail!("no speech server configured; set asr_server and asr_server_model");
    }
    let audio = audio::read_wav(wav)?;
    let started = std::time::Instant::now();
    let text = remote::transcribe(&settings.asr_server, &settings.asr_server_model, &settings.language, &audio)?;
    eprintln!("transcribed {:.1}s of audio in {:.2?}", audio.len() as f32 / 16_000.0, started.elapsed());
    println!("{text}");
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
    let mut mode = commands::Mode::Dictate;
    let grammar = settings.grammar();
    for event in rx {
        match event {
            engine::Event::Utterance(text) => {
                match grammar.parse(&text, mode) {
                    Some(c) => {
                        let outcome = doc.apply(&c);
                        if let document::Outcome::SwitchMode(m) = outcome {
                            mode = m;
                        }
                        eprintln!("[command {c:?} -> {outcome:?}]");
                    }
                    None if mode == commands::Mode::Edit => eprintln!("[not a command: {text}]"),
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

/// Opens the configured microphone briefly and prints its level, keeping no audio.
fn mic_test(settings: &Settings) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _capture = audio::Capture::start(&settings.input_device, tx)?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let (mut samples, mut peak, mut blocks) = (0usize, 0f32, 0usize);
    let mut longest_gap = std::time::Duration::ZERO;
    let mut last = std::time::Instant::now();
    while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
        let Ok(block) = rx.recv_timeout(left) else { break };
        longest_gap = longest_gap.max(last.elapsed());
        last = std::time::Instant::now();
        blocks += 1;
        samples += block.len();
        peak = block.iter().fold(peak, |p, s| p.max(s.abs()));
    }
    println!(
        "received {:.1} s of audio in {blocks} blocks (longest gap {longest_gap:.0?}), peak level {peak:.3}",
        samples as f32 / audio::SAMPLE_RATE as f32,
    );
    if samples == 0 {
        bail!("the microphone delivered no audio");
    }
    Ok(())
}
