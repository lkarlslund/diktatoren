//! The speech pipeline. One thread owns the microphone and runs wake-word and voice-activity
//! detection; finished utterances go to a recogniser thread.
//!
//! The microphone is only open while listening, or while idle when the wake word is enabled.
//!
//! A wake word is either a dedicated detector model, or a phrase of the user's choosing: then
//! short idle utterances are recognised locally, compared with the phrase and discarded.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use audiocpp::{Backend, Mode, Model, Registry, Session, VoiceActivityKind};

use crate::audio::{self, Capture, SAMPLE_RATE};
use crate::catalog::{self, Task};
use crate::config::Settings;
use crate::{remote, wake};

const RATE: i32 = SAMPLE_RATE as i32;
/// Audio kept before the detected speech start.
const PREROLL: usize = SAMPLE_RATE as usize * 3 / 10;
/// Idle utterances longer than this cannot be a wake phrase.
const MAX_WAKE_UTTERANCE: usize = SAMPLE_RATE as usize * 8;
/// Utterances longer than this are cut and recognised in pieces.
const MAX_UTTERANCE: usize = SAMPLE_RATE as usize * 25;
/// How often a live preview of the utterance in progress is recognised.
const PREVIEW_EVERY: Duration = Duration::from_millis(900);

#[derive(Debug, Clone)]
pub enum Event {
    /// The wake word was heard; the engine has started listening.
    WakeWord,
    Listening(bool),
    /// Microphone level, 0..1, while listening.
    Level(f32),
    /// Recognition of the utterance in progress; replaced by the next preview or final text.
    Preview(String),
    /// A finished utterance.
    Utterance(String),
    /// Models are loaded and the engine is ready.
    Ready { asr: String },
    Status(String),
    Error(String),
}

#[derive(Debug)]
pub enum Command {
    Configure(Box<Settings>),
    /// Starts listening. `from_wake` enables the silence timeout.
    Listen { from_wake: bool },
    /// Listens to a WAV file instead of the microphone until listening stops.
    ListenFrom(PathBuf),
    /// Stops listening; speech in progress is still recognised.
    StopListening,
    Shutdown,
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Command>,
}

impl Handle {
    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }
}

pub fn spawn(settings: Settings, sink: Sink) -> Handle {
    let (tx, rx) = mpsc::channel();
    let handle = Handle { tx };
    handle.send(Command::Configure(Box::new(settings)));
    thread::Builder::new()
        .name("engine".into())
        .spawn({
            let tx = handle.tx.clone();
            move || Engine::new(sink, tx).run(rx)
        })
        .expect("spawning engine thread");
    handle
}

/// Where the bundled Silero VAD lives: beside the executable, or in the source tree when run
/// from a development build.
pub fn silero_path() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|d| d.join("models/silero_vad"));
    if let Some(p) = beside.filter(|p| p.exists()) {
        return p;
    }
    // macOS app bundle: Contents/MacOS/diktatoren -> Contents/Resources/models
    let bundle = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent()?.parent().map(|c| c.join("Resources/models/silero_vad")));
    bundle.unwrap_or_else(|| PathBuf::from("models/silero_vad"))
}

struct Loaded {
    /// The settings the loaded models were built from.
    settings: Settings,
    vad: Session,
    vad_chunk: usize,
    wake: Option<Wake>,
}

enum Wake {
    /// A detector model and its preferred chunk size.
    Model(Session, usize),
    Phrase(String),
}

/// Idle segmentation for the spoken wake phrase.
#[derive(Default)]
struct Idle {
    audio: Vec<f32>,
    /// Stream position of `audio[0]`.
    offset: usize,
    vad_pos: i64,
    vad_pending: Vec<f32>,
    speech_start: Option<usize>,
}

struct Engine {
    sink: Sink,
    registry: Option<Registry>,
    loaded: Option<Loaded>,
    recogniser: Option<Recogniser>,
    capture: Option<Capture>,
    audio_tx: mpsc::Sender<Vec<f32>>,
    audio_rx: mpsc::Receiver<Vec<f32>>,
    listening: Option<Listening>,
    idle: Option<Idle>,
    /// A WAV file standing in for the microphone during one listening period.
    input_override: Option<String>,
    commands: mpsc::Sender<Command>,
    wake_pos: i64,
    wake_pending: Vec<f32>,
}

/// State of one listening period.
struct Listening {
    from_wake: bool,
    /// All audio since listening started.
    audio: Vec<f32>,
    vad_pos: i64,
    vad_pending: Vec<f32>,
    /// Start of the utterance in progress, in samples since listening started.
    speech_start: Option<usize>,
    last_activity: Instant,
    last_preview: Instant,
    level_at: Instant,
}

impl Engine {
    fn new(sink: Sink, commands: mpsc::Sender<Command>) -> Self {
        let (audio_tx, audio_rx) = mpsc::channel();
        Engine {
            sink,
            registry: None,
            loaded: None,
            recogniser: None,
            capture: None,
            audio_tx,
            audio_rx,
            listening: None,
            idle: None,
            input_override: None,
            commands,
            wake_pos: 0,
            wake_pending: Vec::new(),
        }
    }

    fn emit(&self, event: Event) {
        (self.sink)(event)
    }

    fn run(mut self, rx: mpsc::Receiver<Command>) {
        loop {
            loop {
                match rx.try_recv() {
                    Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => return,
                    Ok(command) => self.handle(command),
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            if self.capture.is_none() {
                // Nothing to read; block on commands instead of spinning.
                match rx.recv() {
                    Ok(Command::Shutdown) | Err(_) => return,
                    Ok(command) => self.handle(command),
                }
                continue;
            }
            match self.audio_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(block) => {
                    if let Err(e) = self.process(block) {
                        self.emit(Event::Error(format!("{e:#}")));
                        self.stop_listening();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.check_timeout();
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Configure(settings) => {
                if let Err(e) = self.configure(*settings) {
                    self.emit(Event::Error(format!("{e:#}")));
                }
            }
            Command::Listen { from_wake } => {
                if let Err(e) = self.start_listening(from_wake) {
                    self.emit(Event::Error(format!("{e:#}")));
                }
            }
            Command::ListenFrom(path) => {
                log::info!("listening to {} instead of the microphone", path.display());
                self.stop_listening();
                self.input_override = Some(format!("file:{}", path.display()));
                self.capture = None;
                if let Err(e) = self.start_listening(false) {
                    self.emit(Event::Error(format!("{e:#}")));
                }
            }
            Command::StopListening => self.stop_listening(),
            Command::Shutdown => {}
        }
    }

    fn settings(&self) -> Option<&Settings> {
        self.loaded.as_ref().map(|l| &l.settings)
    }

    fn configure(&mut self, settings: Settings) -> Result<()> {
        if self.registry.is_none() {
            self.registry = Some(Registry::new().context("starting audio.cpp")?);
        }
        let (previous, vad, wake) = match self.loaded.take() {
            Some(l) => (Some(l.settings), Some((l.vad, l.vad_chunk)), l.wake),
            None => (None, None, None),
        };
        // Silence settings travel with each stream start, so the VAD never needs reloading.
        let (vad, vad_chunk) = match vad {
            Some(v) => v,
            None => {
                let registry = self.registry.as_ref().unwrap();
                let model = Model::load(registry, &silero_path(), Some("silero_vad"))
                    .context("loading Silero VAD")?;
                let session = Session::new(&model, "vad", Mode::Streaming, &cpu(), &[])?;
                let chunk = session.preferred_chunk().unwrap_or(512);
                (session, chunk)
            }
        };
        let wake_unchanged = previous.as_ref().is_some_and(|p| {
            p.wake_word_package == settings.wake_word_package
                && p.wake_word_threshold == settings.wake_word_threshold
                && p.wake_phrase == settings.wake_phrase
                && p.models_dir() == settings.models_dir()
        }) && wake.is_some() == settings.wake_word_enabled;
        let wake = if wake_unchanged {
            wake
        } else {
            drop(wake);
            self.wake_pos = 0;
            self.wake_pending.clear();
            match self.load_wake(&settings) {
                Ok(w) => w,
                Err(e) => {
                    self.emit(Event::Error(format!("{e:#}")));
                    None
                }
            }
        };
        let microphone_changed =
            previous.as_ref().is_some_and(|p| p.input_device != settings.input_device);
        self.loaded = Some(Loaded { settings, vad, vad_chunk, wake });
        self.reload_recogniser();
        if self.phrase().is_some() && self.recogniser.is_none() {
            self.emit(Event::Error("A spoken wake phrase needs a speech model.".into()));
        }
        if microphone_changed {
            self.capture = None;
        }
        self.start_idle();
        self.update_microphone();
        Ok(())
    }

    fn phrase(&self) -> Option<&str> {
        match self.loaded.as_ref()?.wake.as_ref()? {
            Wake::Phrase(p) => Some(p),
            Wake::Model(..) => None,
        }
    }

    /// Starts idle segmentation when waiting for a spoken wake phrase.
    fn start_idle(&mut self) {
        self.idle = None;
        if self.listening.is_some() || self.phrase().is_none() {
            return;
        }
        let loaded = self.loaded.as_mut().unwrap();
        let options = [("min_silence_duration_ms", "400"), ("speech_pad_ms", "150")];
        match loaded.vad.stream_start(RATE, &options) {
            Ok(()) => self.idle = Some(Idle::default()),
            Err(e) => self.emit(Event::Error(format!("voice detection: {e}"))),
        }
    }

    fn load_wake(&self, s: &Settings) -> Result<Option<Wake>> {
        if !s.wake_word_enabled {
            return Ok(None);
        }
        if s.wake_word_package == wake::PHRASE {
            if s.wake_phrase.trim().is_empty() {
                return Err(anyhow!("the wake phrase is empty"));
            }
            return Ok(Some(Wake::Phrase(s.wake_phrase.trim().to_string())));
        }
        let (family, package) = catalog::find(&s.wake_word_package)
            .ok_or_else(|| anyhow!("unknown wake word model {}", s.wake_word_package))?;
        let dir = s.models_dir();
        if !package.is_installed(&dir) {
            return Err(anyhow!("wake word model {} is not downloaded", package.name));
        }
        let registry = self.registry.as_ref().unwrap();
        let model = Model::load(registry, &package.load_path(&dir), Some(&family.family))
            .with_context(|| format!("loading {}", package.name))?;
        let mut session = Session::new(&model, Task::WakeWord.token(), Mode::Streaming, &cpu(), &[])?;
        let chunk = session.preferred_chunk().unwrap_or(480);
        let threshold = (s.wake_word_threshold > 0.0).then(|| s.wake_word_threshold.to_string());
        let options: Vec<(&str, &str)> = threshold.iter().map(|t| ("threshold", t.as_str())).collect();
        session.stream_start(RATE, &options)?;
        Ok(Some(Wake::Model(session, chunk)))
    }

    fn reload_recogniser(&mut self) {
        let settings = self.loaded.as_ref().unwrap().settings.clone();
        if let Some(r) = &self.recogniser
            && r.settings.asr_server == settings.asr_server
            && r.settings.asr_server_model == settings.asr_server_model
            && r.settings.language == settings.language
        {
            return;
        }
        self.recogniser = None;
        if !settings.uses_server() {
            self.emit(Event::Error("Choose a speech server in Settings → Speech.".into()));
            return;
        }
        self.recogniser = Some(Recogniser::spawn(settings, self.sink.clone(), self.commands.clone()));
    }

    fn wake_active(&self) -> bool {
        self.listening.is_none()
            && self.loaded.as_ref().is_some_and(|l| l.wake.is_some())
    }

    fn update_microphone(&mut self) {
        let wanted = self.listening.is_some() || self.wake_active();
        if wanted && self.capture.is_none() {
            let device = match &self.input_override {
                Some(file) => file.clone(),
                None => self.settings().map(|s| s.input_device.clone()).unwrap_or_default(),
            };
            while self.audio_rx.try_recv().is_ok() {}
            match Capture::start(&device, self.audio_tx.clone()) {
                Ok(c) => self.capture = Some(c),
                Err(e) => {
                    self.emit(Event::Error(format!("microphone: {e:#}")));
                    self.listening = None;
                    self.emit(Event::Listening(false));
                }
            }
        } else if !wanted && self.capture.is_some() {
            self.capture = None;
        }
    }

    fn start_listening(&mut self, from_wake: bool) -> Result<()> {
        log::info!("listen requested{}", if from_wake { " by wake word" } else { "" });
        if self.listening.is_some() {
            if let Some(l) = self.listening.as_mut() {
                l.from_wake &= from_wake;
                l.last_activity = Instant::now();
            }
            return Ok(());
        }
        let loaded = self.loaded.as_mut().ok_or_else(|| anyhow!("speech models are not loaded"))?;
        let silence = loaded.settings.utterance_silence_ms.to_string();
        loaded.vad.stream_start(
            RATE,
            &[("min_silence_duration_ms", silence.as_str()), ("speech_pad_ms", "150")],
        )?;
        let now = Instant::now();
        self.idle = None;
        self.listening = Some(Listening {
            from_wake,
            audio: Vec::new(),
            vad_pos: 0,
            vad_pending: Vec::new(),
            speech_start: None,
            last_activity: now,
            last_preview: now,
            level_at: now,
        });
        self.update_microphone();
        if self.capture.is_some() {
            self.emit(Event::Listening(true));
        }
        Ok(())
    }

    fn stop_listening(&mut self) {
        let Some(mut listening) = self.listening.take() else { return };
        log::info!(
            "stopped listening after {:.1}s{}",
            listening.audio.len() as f32 / SAMPLE_RATE as f32,
            if listening.speech_start.is_some() { ", finishing the utterance in progress" } else { "" }
        );
        if let Some(start) = listening.speech_start.take() {
            self.finish_utterance(&listening.audio, start, listening.audio.len());
        }
        if let Some(l) = self.loaded.as_mut() {
            let _ = l.vad.stream_finish();
        }
        self.emit(Event::Listening(false));
        if self.input_override.take().is_some() {
            self.capture = None;
        }
        self.start_idle();
        self.update_microphone();
    }

    fn finish_utterance(&self, audio: &[f32], start: usize, end: usize) {
        let start = start.saturating_sub(PREROLL);
        let end = end.min(audio.len());
        // Ignore blips shorter than a quarter second.
        if end <= start + SAMPLE_RATE as usize / 4 {
            log::info!("utterance of {} ms ignored as too short", (end - start) * 1000 / SAMPLE_RATE as usize);
            return;
        }
        log::info!(
            "utterance {:.1}s–{:.1}s sent for recognition",
            start as f32 / SAMPLE_RATE as f32,
            end as f32 / SAMPLE_RATE as f32
        );
        if self.recogniser.is_none() {
            log::warn!("no speech server configured; utterance dropped");
        }
        if let Some(r) = &self.recogniser {
            r.submit(Job::Final(audio[start..end].to_vec()));
        }
    }

    fn check_timeout(&mut self) {
        let Some(l) = &self.listening else { return };
        let Some(settings) = self.settings() else { return };
        let timeout = Duration::from_secs(settings.wake_listen_timeout_s.max(1) as u64);
        if l.from_wake && l.speech_start.is_none() && l.last_activity.elapsed() > timeout {
            log::info!("silence timeout after wake word");
            self.stop_listening();
        }
    }

    fn process(&mut self, block: Vec<f32>) -> Result<()> {
        if self.listening.is_some() {
            return self.process_listening(block);
        }
        if !self.wake_active() {
            return Ok(());
        }
        if self.phrase().is_some() {
            return self.process_idle(block);
        }
        self.wake_pending.extend_from_slice(&block);
        let loaded = self.loaded.as_mut().unwrap();
        let Some(Wake::Model(session, chunk)) = loaded.wake.as_mut() else { return Ok(()) };
        let chunk = *chunk;
        let mut detected = false;
        while self.wake_pending.len() >= chunk {
            let piece: Vec<f32> = self.wake_pending.drain(..chunk).collect();
            let event = session.stream_push(&piece, RATE, self.wake_pos)?;
            self.wake_pos += chunk as i64;
            let mut events: Vec<_> = event.into_iter().collect();
            while let Some(e) = session.stream_next_event()? {
                events.push(e);
            }
            detected |= events
                .iter()
                .any(|e| e.voice_activity.iter().any(|a| a.kind == VoiceActivityKind::SpeechSegment));
        }
        if detected {
            log::info!("wake word model detected its phrase");
            // Restart the detector so the same utterance does not trigger twice.
            session.stream_reset()?;
            session.stream_start(RATE, &[])?;
            self.wake_pos = 0;
            self.wake_pending.clear();
            self.emit(Event::WakeWord);
            self.start_listening(true)?;
        }
        Ok(())
    }

    /// Cuts idle speech into utterances and hands short ones over to be checked for the phrase.
    fn process_idle(&mut self, block: Vec<f32>) -> Result<()> {
        let phrase = self.phrase().unwrap_or_default().to_string();
        let (Some(loaded), Some(idle)) = (self.loaded.as_mut(), self.idle.as_mut()) else {
            return Ok(());
        };
        idle.audio.extend_from_slice(&block);
        idle.vad_pending.extend_from_slice(&block);
        let chunk = loaded.vad_chunk;
        let mut finished = Vec::new();
        while idle.vad_pending.len() >= chunk {
            let piece: Vec<f32> = idle.vad_pending.drain(..chunk).collect();
            let event = loaded.vad.stream_push(&piece, RATE, idle.vad_pos)?;
            idle.vad_pos += chunk as i64;
            let mut events: Vec<_> = event.into_iter().collect();
            while let Some(e) = loaded.vad.stream_next_event()? {
                events.push(e);
            }
            for activity in events.iter().flat_map(|e| e.voice_activity.iter()) {
                let at = activity.sample.max(0) as usize;
                match activity.kind {
                    VoiceActivityKind::SpeechStart => {
                        idle.speech_start.get_or_insert(at);
                    }
                    VoiceActivityKind::SpeechEnd => {
                        if let Some(start) = idle.speech_start.take() {
                            finished.push((start, at));
                        }
                    }
                    VoiceActivityKind::SpeechSegment => {}
                }
            }
        }
        let end_of_audio = idle.offset + idle.audio.len();
        for (start, end) in finished {
            let from = start.saturating_sub(PREROLL).max(idle.offset) - idle.offset;
            let to = end.min(end_of_audio) - idle.offset;
            let length = to.saturating_sub(from);
            if length > SAMPLE_RATE as usize / 4 && length <= MAX_WAKE_UTTERANCE
                && let Some(r) = &self.recogniser {
                    r.submit(Job::Wake(idle.audio[from..to].to_vec(), phrase.clone()));
                }
        }
        // Long speech is not a wake phrase; stop holding on to it.
        if idle.speech_start.is_some_and(|s| end_of_audio - s > MAX_WAKE_UTTERANCE) {
            idle.speech_start = None;
        }
        let keep_from = idle.speech_start.unwrap_or(end_of_audio).saturating_sub(PREROLL);
        if keep_from > idle.offset {
            idle.audio.drain(..keep_from - idle.offset);
            idle.offset = keep_from;
        }
        Ok(())
    }

    fn process_listening(&mut self, block: Vec<f32>) -> Result<()> {
        let preview_enabled = self.recogniser.as_ref().is_some_and(|r| r.previews);
        let loaded = self.loaded.as_mut().unwrap();
        let l = self.listening.as_mut().unwrap();
        l.audio.extend_from_slice(&block);
        l.vad_pending.extend_from_slice(&block);

        if l.level_at.elapsed() >= Duration::from_millis(30) {
            let rms = (block.iter().map(|s| s * s).sum::<f32>() / block.len().max(1) as f32).sqrt();
            l.level_at = Instant::now();
            // -50 dBFS (room noise) to 0 dBFS maps onto 0..1.
            let db = 20.0 * rms.max(1e-6).log10();
            (self.sink)(Event::Level(((db + 50.0) / 50.0).clamp(0.0, 1.0)));
        }

        let chunk = loaded.vad_chunk;
        let mut finished = Vec::new();
        while l.vad_pending.len() >= chunk {
            let piece: Vec<f32> = l.vad_pending.drain(..chunk).collect();
            let event = loaded.vad.stream_push(&piece, RATE, l.vad_pos)?;
            l.vad_pos += chunk as i64;
            let mut events: Vec<_> = event.into_iter().collect();
            while let Some(e) = loaded.vad.stream_next_event()? {
                events.push(e);
            }
            for activity in events.iter().flat_map(|e| e.voice_activity.iter()) {
                let at = activity.sample.max(0) as usize;
                match activity.kind {
                    VoiceActivityKind::SpeechStart => {
                        l.speech_start.get_or_insert(at);
                        l.last_activity = Instant::now();
                    }
                    VoiceActivityKind::SpeechEnd => {
                        if let Some(start) = l.speech_start.take() {
                            finished.push((start, at));
                        }
                        l.last_activity = Instant::now();
                    }
                    VoiceActivityKind::SpeechSegment => {}
                }
            }
        }
        // Cut overly long utterances so recognition keeps up.
        if let Some(start) = l.speech_start
            && l.audio.len() - start > MAX_UTTERANCE
        {
            finished.push((start, l.audio.len()));
            l.speech_start = Some(l.audio.len());
        }
        let preview = match l.speech_start {
            Some(start) if preview_enabled && l.last_preview.elapsed() > PREVIEW_EVERY => {
                l.last_preview = Instant::now();
                Some(l.audio[start.saturating_sub(PREROLL)..].to_vec())
            }
            _ => None,
        };
        let audio = std::mem::take(&mut l.audio);
        for (start, end) in finished {
            self.finish_utterance(&audio, start, end);
        }
        self.listening.as_mut().unwrap().audio = audio;
        if let (Some(samples), Some(r)) = (preview, &self.recogniser) {
            r.submit(Job::Preview(samples));
        }
        Ok(())
    }
}

/// Detection models are tiny and run on the CPU.
fn cpu() -> Backend {
    Backend { name: "cpu".into(), device: 0, threads: 1 }
}

/// The speech server and model that recognise utterances.
struct Asr {
    url: String,
    model: String,
    language: String,
}

impl Asr {
    /// Fails early with a clear message when the server is down or lacks the model.
    fn open(url: String, model: String, language: &str) -> Result<Self> {
        let models = remote::asr_models(&url)?;
        if !models.contains(&model) {
            return Err(anyhow!("{url} has no speech model {model:?} (it has: {})", models.join(", ")));
        }
        Ok(Asr { url, model, language: language.to_string() })
    }

    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        remote::transcribe(&self.url, &self.model, &self.language, samples)
    }
}

enum Job {
    Final(Vec<f32>),
    Preview(Vec<f32>),
    /// Idle speech to compare with the wake phrase.
    Wake(Vec<f32>, String),
}

#[derive(Default)]
struct Queue {
    finals: VecDeque<Vec<f32>>,
    /// Only the newest preview is worth running.
    preview: Option<Vec<f32>>,
    wake: Option<(Vec<f32>, String)>,
    closed: bool,
}

/// Runs speech recognition on its own thread so detection never stalls.
struct Recogniser {
    settings: Settings,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    previews: bool,
}

impl Recogniser {
    fn spawn(settings: Settings, sink: Sink, commands: mpsc::Sender<Command>) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let worker_queue = queue.clone();
        let language = settings.language.clone();
        let url = settings.asr_server.trim().to_string();
        let model = settings.asr_server_model.trim().to_string();
        let name = format!("{model} on {url}");
        thread::Builder::new()
            .name("recogniser".into())
            .spawn(move || {
                sink(Event::Status(format!("Connecting to {name}…")));
                let started = Instant::now();
                let mut asr = match Asr::open(url, model, &language) {
                    Ok(a) => a,
                    Err(e) => {
                        sink(Event::Error(format!("{e:#}")));
                        return;
                    }
                };
                log::info!("{name} ready in {:.1?}", started.elapsed());
                sink(Event::Ready { asr: name.clone() });
                let (lock, cvar) = &*worker_queue;
                loop {
                    let job = {
                        let mut q = lock.lock().unwrap();
                        loop {
                            if q.closed {
                                return;
                            }
                            if let Some(f) = q.finals.pop_front() {
                                break Job::Final(f);
                            }
                            if let Some((w, phrase)) = q.wake.take() {
                                break Job::Wake(w, phrase);
                            }
                            if let Some(p) = q.preview.take() {
                                break Job::Preview(p);
                            }
                            q = cvar.wait(q).unwrap();
                        }
                    };
                    let (samples, is_final) = match job {
                        Job::Final(s) => (s, true),
                        Job::Preview(s) => (s, false),
                        Job::Wake(samples, phrase) => {
                            let started = Instant::now();
                            let heard = asr.transcribe(&samples).unwrap_or_default();
                            let matched = wake::match_phrase(&heard, &phrase);
                            log::info!(
                                "idle {:.1}s heard in {:.0?}: {heard:?} → {}",
                                samples.len() as f32 / SAMPLE_RATE as f32,
                                started.elapsed(),
                                if matched.is_some() { "wake phrase" } else { "ignored" }
                            );
                            if let Some(rest) = matched {
                                sink(Event::WakeWord);
                                let _ = commands.send(Command::Listen { from_wake: true });
                                if !rest.is_empty() {
                                    sink(Event::Utterance(rest));
                                }
                            }
                            continue;
                        }
                    };
                    let started = Instant::now();
                    match asr.transcribe(&samples) {
                        Ok(text) => {
                            let text = text.trim().to_string();
                            log::info!(
                                "{} {:.1}s recognised in {:.0?}: {text:?}",
                                if is_final { "final" } else { "preview" },
                                samples.len() as f32 / SAMPLE_RATE as f32,
                                started.elapsed()
                            );
                            if is_final {
                                // A final supersedes any preview queued for the same speech.
                                lock.lock().unwrap().preview = None;
                                sink(Event::Utterance(text));
                            } else if lock.lock().unwrap().finals.is_empty() {
                                sink(Event::Preview(text));
                            }
                        }
                        Err(e) => {
                            log::warn!("recognition of {:.1}s failed after {:.0?}: {e:#}", samples.len() as f32 / SAMPLE_RATE as f32, started.elapsed());
                            sink(Event::Error(format!("recognition failed: {e:#}")));
                        }
                    }
                }
            })
            .expect("spawning recogniser");
        // A preview re-sends the whole utterance each time; too much for a shared server.
        Recogniser { settings, queue, previews: false }
    }

    fn submit(&self, job: Job) {
        let (lock, cvar) = &*self.queue;
        let mut q = lock.lock().unwrap();
        match job {
            Job::Final(s) => {
                q.preview = None;
                q.finals.push_back(s);
            }
            Job::Preview(s) => q.preview = Some(s),
            // Only the latest idle utterance matters if recognition falls behind.
            Job::Wake(s, phrase) => q.wake = Some((s, phrase)),
        }
        cvar.notify_one();
    }
}

impl Drop for Recogniser {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.queue;
        lock.lock().unwrap().closed = true;
        cvar.notify_all();
    }
}

/// Lists microphones for the settings window.
pub fn microphones() -> Vec<String> {
    audio::input_devices()
}
