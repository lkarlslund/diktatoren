//! The desktop application: a windowless daemon with a tray icon that opens the dictation
//! window on demand, plus the settings window.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, row, rule, scrollable, slider,
    space, text, text_editor, text_input,
};
use iced::{Alignment, Element, Font, Length, Size, Subscription, Task, keyboard, window};

use crate::ai::AiMode;
use crate::bus::{self, Action, BusEvent};
use crate::catalog::{self, Task as ModelTask};
use crate::commands::{self, Language, Mode};
use crate::config::{FinishAction, HotkeyMode, InsertMethod, Settings};
use crate::document::{Document, Outcome};
use crate::download::{self, Progress};
use crate::engine::{self, Command as EngineCommand};
use crate::hotkey::{self, Hotkeys};
use crate::icon::{self, State as IconState};
use crate::output::{self, Clipboard, Target};
use crate::tray::Tray;
use crate::wake;

const EDITOR: &str = "dictation";

pub fn run(settings: Settings, initial: Option<Action>) -> iced::Result {
    iced::daemon(move || App::boot(settings.clone(), initial), App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finish {
    Insert,
    Copy,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Speech,
    Output,
    Commands,
}

#[derive(Debug, Clone)]
enum Message {
    Init(Option<Action>),
    Bus(BusEvent),
    Edit(text_editor::Action),
    Finish(Finish),
    Pause,
    SetMode(Mode),
    OpenSettings,
    WindowClosed(window::Id),
    CloseRequested(window::Id),
    Deliver(String, Target),
    RestoreClipboard(String),
    /// Finishing stops waiting for the tidy-up.
    TidyTimeout,
    Settings(SettingsMessage),
}

#[derive(Debug, Clone)]
enum SettingsMessage {
    Tab(Tab),
    Hotkey(String),
    HotkeyMode(HotkeyMode),
    WakeEnabled(bool),
    WakePackage(Choice),
    WakePhrase(String),
    WakeThreshold(f32),
    WakeTimeout(f32),
    Microphone(Choice),
    AsrServer(String),
    AsrServerModel(Choice),
    FetchServerModels,
    AiMode(AiMode),
    AiServer(String),
    AiModel(Choice),
    FetchAiModels,
    EnterAction(Choice),
    InsertMethod(Choice),
    PasteShortcut(String),
    RestoreClipboard(bool),
    VoiceCommands(bool),
    CommandLanguage(Language),
    Phrase(&'static str, String),
    Save,
    Close,
}

/// An entry in a pick list: an id with a label.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    id: String,
    label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

fn choice(id: &str, label: &str) -> Choice {
    Choice { id: id.into(), label: label.into() }
}

struct Popup {
    id: window::Id,
    content: text_editor::Content,
    doc: Document,
    preview: String,
    notice: Option<String>,
    target: Target,
    mode: Mode,
    /// A tidy-up request is with the language model.
    tidying: bool,
    /// Finishing waits for the tidy-up to come back.
    waiting: Option<Finish>,
}

struct SettingsWindow {
    id: window::Id,
    draft: Settings,
    tab: Tab,
    microphones: Vec<String>,
    saved_at: Option<Instant>,
    /// Models listed by the server in `draft.asr_server`, or the error from asking.
    server_models: Result<Vec<String>, String>,
    ai_models: Result<Vec<String>, String>,
}

struct App {
    settings: Settings,
    engine: engine::Handle,
    hotkeys: Option<Hotkeys>,
    tray: Option<Tray>,
    clipboard: Option<Clipboard>,
    popup: Option<Popup>,
    settings_window: Option<SettingsWindow>,
    available: Vec<String>,
    downloads: HashMap<String, (Progress, Arc<AtomicBool>)>,
    status: String,
    grammar: commands::Grammar,
    /// The desktop's colours, read when a window opens.
    theme: Option<iced::Theme>,
    listening: bool,
    level: f32,
}

impl App {
    fn boot(settings: Settings, initial: Option<Action>) -> (Self, Task<Message>) {
        let sink: engine::Sink = Arc::new(|event| bus::send(BusEvent::Engine(event)));
        let engine = engine::spawn(settings.clone(), sink);
        let available = audiocpp::Registry::new().map(|r| r.families()).unwrap_or_default();
        let grammar = settings.grammar();
        let app = App {
            settings,
            engine,
            hotkeys: None,
            tray: None,
            clipboard: None,
            popup: None,
            settings_window: None,
            available,
            downloads: HashMap::new(),
            status: "Starting…".into(),
            grammar,
            theme: crate::theme::current(),
            listening: false,
            level: 0.0,
        };
        // Tray, hotkeys and clipboard need the event loop running on the main thread.
        (app, Task::done(Message::Init(initial)))
    }

    fn theme(&self, _id: window::Id) -> Option<iced::Theme> {
        self.theme.clone()
    }

    fn title(&self, id: window::Id) -> String {
        if self.settings_window.as_ref().is_some_and(|s| s.id == id) {
            "Diktatoren settings".into()
        } else {
            "Diktatoren".into()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(bus_stream).map(Message::Bus),
            window::close_events().map(Message::WindowClosed),
            window::close_requests().map(Message::CloseRequested),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Init(initial) => {
                self.tray = Some(Tray::new());
                self.clipboard = Some(Clipboard::new());
                let mut hotkeys = Hotkeys::new();
                if let Err(e) = hotkeys.bind(&self.settings.hotkey) {
                    self.status = format!("{e:#}");
                }
                self.hotkeys = Some(hotkeys);
                #[cfg(target_os = "linux")]
                crate::portal::set_restore_token(self.settings.portal_restore_token.clone());
                let first_run = !self.settings.uses_server();
                let mut tasks = Vec::new();
                if first_run {
                    tasks.push(self.open_settings(Tab::Speech));
                }
                if let Some(action) = initial {
                    tasks.push(self.action(action));
                }
                Task::batch(tasks)
            }
            Message::Bus(event) => self.bus_event(event),
            Message::Edit(action) => {
                if let Some(p) = &mut self.popup {
                    p.content.perform(action);
                    let text = p.content.text();
                    let selection = selection_range(&text, p.content.cursor());
                    p.doc.set_from_editor(text, selection);
                }
                Task::none()
            }
            Message::Finish(finish) => self.finish(finish),
            Message::OpenSettings => {
                // Let the settings window come in front of the dictation window.
                let lower = match &self.popup {
                    Some(p) => window::set_level(p.id, window::Level::Normal),
                    None => Task::none(),
                };
                Task::batch([lower, self.open_settings(Tab::General)])
            }
            Message::SetMode(mode) => {
                if let Some(p) = &mut self.popup {
                    p.mode = mode;
                    p.notice = None;
                }
                if !self.listening {
                    self.engine.send(EngineCommand::Listen { from_wake: false });
                }
                self.focus_popup()
            }
            Message::Pause => {
                self.engine.send(EngineCommand::StopListening("Pause button"));
                self.focus_popup()
            }
            Message::CloseRequested(id) => {
                if self.popup.as_ref().is_some_and(|p| p.id == id) {
                    return self.finish(Finish::Cancel);
                }
                window::close(id)
            }
            Message::WindowClosed(id) => {
                if self.popup.as_ref().is_some_and(|p| p.id == id) {
                    self.popup = None;
                    self.engine.send(EngineCommand::Standby(false));
                    self.engine.send(EngineCommand::StopListening("window closed"));
                }
                if self.settings_window.as_ref().is_some_and(|s| s.id == id) {
                    self.settings_window = None;
                    if let Some(p) = &self.popup {
                        return Task::batch([
                            window::set_level(p.id, window::Level::AlwaysOnTop),
                            self.focus_popup(),
                        ]);
                    }
                }
                Task::none()
            }
            Message::Deliver(text, target) => {
                let settings = self.settings.clone();
                log::info!("inserting by {:?} ({})", settings.insert_method, settings.paste_shortcut);
                std::thread::spawn(move || {
                    if let Err(e) = output::deliver(&text, &target, &settings) {
                        log::warn!("inserting text: {e:#}");
                        bus::send(BusEvent::Notice(format!(
                            "Could not insert the text ({e}). It is on the clipboard."
                        )));
                    }
                });
                Task::none()
            }
            Message::TidyTimeout => {
                if let Some(finish) = self.popup.as_mut().and_then(|p| p.waiting.take()) {
                    log::warn!("tidy-up did not come back in time; finishing without it");
                    return self.finish_now(finish);
                }
                Task::none()
            }
            Message::RestoreClipboard(previous) => {
                if let Some(c) = &mut self.clipboard {
                    let _ = c.set(&previous);
                }
                Task::none()
            }
            Message::Settings(m) => self.settings_message(m),
        }
    }

    fn bus_event(&mut self, event: BusEvent) -> Task<Message> {
        match event {
            BusEvent::Engine(e) => self.engine_event(e),
            BusEvent::HotkeyPressed => {
                log::info!("hotkey pressed");
                match self.settings.hotkey_mode {
                    HotkeyMode::Toggle => self.action(Action::Toggle),
                    HotkeyMode::PushToTalk => self.action(Action::Start),
                }
            }
            BusEvent::HotkeyReleased => {
                log::info!("hotkey released");
                if self.settings.hotkey_mode == HotkeyMode::PushToTalk {
                    self.engine.send(EngineCommand::StopListening("push-to-talk key released"));
                }
                Task::none()
            }
            BusEvent::Feed(path) => {
                log::info!("feed {}", path.display());
                self.engine.send(EngineCommand::ListenFrom(path));
                if self.popup.is_none() { self.open_popup() } else { self.focus_popup() }
            }
            BusEvent::Action(action) => {
                log::info!("action {action:?}");
                self.action(action)
            }
            BusEvent::DownloadProgress(id, progress) => {
                if let Some(entry) = self.downloads.get_mut(&id) {
                    entry.0 = progress;
                }
                Task::none()
            }
            BusEvent::DownloadFinished(id, result) => {
                self.downloads.remove(&id);
                match result {
                    Ok(()) => {
                        self.engine.send(EngineCommand::Configure(Box::new(self.settings.clone())));
                    }
                    Err(e) => self.status = format!("Download failed: {e}"),
                }
                Task::none()
            }
            BusEvent::AiModels(url, models) => {
                if let Some(w) = &mut self.settings_window
                    && w.draft.ai_server.trim() == url.trim()
                {
                    if let Ok(list) = &models
                        && w.draft.ai_model.is_empty()
                        && let Some(first) = list.first()
                    {
                        w.draft.ai_model = first.clone();
                    }
                    w.ai_models = models;
                }
                Task::none()
            }
            BusEvent::Tidied { start, original, result } => self.tidied(start, original, result),
            BusEvent::ServerModels(url, models) => {
                if let Some(w) = &mut self.settings_window
                    && w.draft.asr_server.trim() == url.trim()
                {
                    if let Ok(list) = &models
                        && w.draft.asr_server_model.is_empty()
                        && let Some(first) = list.first()
                    {
                        w.draft.asr_server_model = first.clone();
                    }
                    w.server_models = models;
                }
                Task::none()
            }
            BusEvent::PortalToken(token) => {
                self.settings.portal_restore_token = Some(token.clone());
                if let Some(w) = &mut self.settings_window {
                    w.draft.portal_restore_token = Some(token);
                }
                let _ = self.settings.save();
                Task::none()
            }
            BusEvent::Notice(notice) => {
                log::info!("{notice}");
                self.status = notice;
                Task::none()
            }
        }
    }

    fn engine_event(&mut self, event: engine::Event) -> Task<Message> {
        match event {
            engine::Event::WakeWord => {
                log::info!("woken by wake word");
                if self.popup.is_none() {
                    return self.open_popup();
                }
            }
            engine::Event::Listening(on) => {
                self.listening = on;
                self.level = 0.0;
                if let Some(t) = &self.tray {
                    t.set_state(if on { IconState::Listening } else { IconState::Idle });
                }
            }
            // Rise at once, fall back smoothly.
            engine::Event::Level(level) => {
                self.level = if level > self.level { level } else { self.level * 0.7 + level * 0.3 };
            }
            engine::Event::Preview(preview) => {
                if let Some(p) = &mut self.popup {
                    p.preview = preview;
                }
            }
            engine::Event::Utterance(utterance) => return self.utterance(utterance),
            engine::Event::Overheard(heard) => return self.overheard(heard),
            engine::Event::WakeTimedOut => {
                if self.popup.as_ref().is_some_and(|p| p.doc.text().trim().is_empty()) {
                    log::info!("closing: nothing said after the wake word");
                    return self.finish(Finish::Cancel);
                }
            }
            engine::Event::Ready { asr } => self.status = format!("Ready · {asr}"),
            engine::Event::Status(s) => self.status = s,
            engine::Event::Error(e) => {
                log::warn!("{e}");
                if let Some(p) = &mut self.popup {
                    p.notice = Some(e.clone());
                }
                self.status = e;
            }
        }
        Task::none()
    }

    fn utterance(&mut self, spoken: String) -> Task<Message> {
        let commands_enabled = self.settings.voice_commands;
        let Some(p) = &mut self.popup else { return Task::none() };
        p.preview.clear();
        if spoken.trim().is_empty() {
            return Task::none();
        }
        let mode = if commands_enabled { p.mode } else { Mode::Dictate };
        let command = commands_enabled.then(|| self.grammar.parse(&spoken, mode)).flatten();
        p.notice = None;
        let outcome = match command {
            Some(c) => {
                let outcome = p.doc.apply(&c);
                log::info!("{mode:?} mode: {spoken:?} → command {c:?} → {outcome:?}");
                outcome
            }
            // While editing, speech that is no command replaces the selection, if any.
            None if mode == Mode::Edit && p.doc.selection().is_empty() => {
                log::info!("Edit mode: {spoken:?} → no command, ignored");
                p.notice = Some(format!("“{spoken}” is not an edit command. Say “dictate” or “diktér” to write."));
                Outcome::Edited
            }
            None => {
                p.doc.dictate(&spoken);
                log::info!("{mode:?} mode: {spoken:?} → dictated");
                Outcome::Edited
            }
        };
        if outcome == Outcome::NotFound {
            p.notice = Some(format!("Nothing to change for “{spoken}”"));
        }
        if let Outcome::SwitchMode(m) = outcome {
            p.mode = m;
        }
        sync_editor(p);
        match outcome {
            Outcome::Insert => self.finish(Finish::Insert),
            Outcome::Copy => self.finish(Finish::Copy),
            Outcome::Cancel => self.finish(Finish::Cancel),
            Outcome::StopListening => {
                self.engine.send(EngineCommand::StopListening("voice command"));
                Task::none()
            }
            Outcome::Edited => self.start_tidy(),
            Outcome::NotFound | Outcome::SwitchMode(_) => Task::none(),
        }
    }

    /// Speech heard while paused: only switching to Dictate or Edit acts on it, and resumes
    /// listening in that mode.
    fn overheard(&mut self, heard: String) -> Task<Message> {
        if self.popup.is_none() || self.listening || !self.settings.voice_commands {
            return Task::none();
        }
        let command = self.grammar.parse(&heard, Mode::Dictate);
        log::info!("paused: {heard:?} → {command:?}");
        let mode = match command {
            Some(commands::Command::DictateMode) => Mode::Dictate,
            Some(commands::Command::EditMode) => Mode::Edit,
            _ => return Task::none(),
        };
        if let Some(p) = &mut self.popup {
            p.mode = mode;
        }
        self.engine.send(EngineCommand::Listen { from_wake: false });
        Task::none()
    }

    fn action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::Toggle => {
                if self.popup.is_none() {
                    self.engine.send(EngineCommand::Listen { from_wake: false });
                    self.open_popup()
                } else if self.listening {
                    self.engine.send(EngineCommand::StopListening("hotkey"));
                    Task::none()
                } else {
                    self.engine.send(EngineCommand::Listen { from_wake: false });
                    self.focus_popup()
                }
            }
            Action::Start => {
                self.engine.send(EngineCommand::Listen { from_wake: false });
                if self.popup.is_none() { self.open_popup() } else { self.focus_popup() }
            }
            Action::Stop => {
                self.engine.send(EngineCommand::StopListening("stop command"));
                Task::none()
            }
            Action::Insert => self.finish(Finish::Insert),
            Action::Copy => self.finish(Finish::Copy),
            Action::Cancel => self.finish(Finish::Cancel),
            Action::Settings => self.open_settings(Tab::General),
            Action::Quit => {
                self.engine.send(EngineCommand::Shutdown);
                iced::exit()
            }
        }
    }

    fn open_popup(&mut self) -> Task<Message> {
        if self.popup.is_some() {
            return self.focus_popup();
        }
        let target = Target::capture();
        log::info!("dictation window opened");
        self.theme = crate::theme::current();
        self.engine.send(EngineCommand::Standby(true));
        let (id, open) = window::open(window::Settings {
            size: Size::new(640.0, 280.0),
            min_size: Some(Size::new(360.0, 180.0)),
            position: window::Position::Centered,
            level: window::Level::AlwaysOnTop,
            exit_on_close_request: false,
            icon: window_icon(),
            platform_specific: platform_specific(),
            ..Default::default()
        });
        self.popup = Some(Popup {
            id,
            content: text_editor::Content::new(),
            doc: Document::default(),
            preview: String::new(),
            notice: None,
            target,
            mode: Mode::Dictate,
            tidying: false,
            waiting: None,
        });
        open.then(move |id| {
            Task::batch([window::gain_focus(id), iced::widget::operation::focus(EDITOR)])
        })
    }

    fn focus_popup(&self) -> Task<Message> {
        match &self.popup {
            Some(p) => Task::batch([window::gain_focus(p.id), iced::widget::operation::focus(EDITOR)]),
            None => Task::none(),
        }
    }

    /// Finishes, after waiting for a tidy-up of the latest dictation when one is due.
    fn finish(&mut self, finish: Finish) -> Task<Message> {
        let uses_ai = self.settings.uses_ai();
        if let Some(p) = &mut self.popup
            && finish != Finish::Cancel
            && uses_ai
            && (p.tidying || p.doc.has_untidy())
        {
            if p.waiting.is_none() {
                p.waiting = Some(finish);
                p.notice = Some("Tidying up the text…".into());
                self.engine.send(EngineCommand::StopListening("finishing"));
                let start = self.start_tidy();
                let timeout = Task::perform(tokio::time::sleep(Duration::from_secs(15)), |_| Message::TidyTimeout);
                return Task::batch([start, timeout]);
            }
            return Task::none();
        }
        self.finish_now(finish)
    }

    /// Sends the untidied dictation to the language model, unless a request is out already.
    fn start_tidy(&mut self) -> Task<Message> {
        if !self.settings.uses_ai() {
            return Task::none();
        }
        let Some(p) = &mut self.popup else { return Task::none() };
        if p.tidying {
            return Task::none();
        }
        let Some((start, original, context)) = p.doc.tidy_job() else { return Task::none() };
        p.tidying = true;
        let s = self.settings.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = crate::ai::tidy(&s.ai_server, &s.ai_model, s.ai_mode, s.text_language(), &context, &original)
                .map_err(|e| format!("{e:#}"));
            log::info!("tidy-up took {:.1?}", started.elapsed());
            bus::send(BusEvent::Tidied { start, original, result });
        });
        Task::none()
    }

    fn tidied(&mut self, start: usize, original: String, result: Result<String, String>) -> Task<Message> {
        let Some(p) = &mut self.popup else { return Task::none() };
        p.tidying = false;
        match result {
            Ok(tidied) => {
                if p.doc.apply_tidy(start, &original, &tidied) {
                    log::info!("tidied {:?} → {:?}", original.trim(), tidied);
                    sync_editor(p);
                } else {
                    log::info!("tidy-up dropped: the text changed meanwhile");
                }
            }
            Err(e) => {
                log::warn!("tidy-up of {:?} rejected: {e}", original.trim());
                // Leave it as dictated rather than retrying the same text forever.
                p.doc.forget_untidy();
            }
        }
        if let Some(finish) = p.waiting
            && !p.doc.has_untidy()
        {
            p.waiting = None;
            return self.finish_now(finish);
        }
        self.start_tidy()
    }

    fn finish_now(&mut self, finish: Finish) -> Task<Message> {
        let Some(popup) = self.popup.take() else { return Task::none() };
        self.engine.send(EngineCommand::Standby(false));
        log::info!("finished with {finish:?}: {} characters", popup.doc.text().trim().chars().count());
        self.engine.send(EngineCommand::StopListening("finished"));
        let close = window::close(popup.id);
        let text = popup.doc.text().trim().to_string();
        if finish == Finish::Cancel || text.is_empty() {
            return close;
        }
        let clipboard = self.clipboard.get_or_insert_with(Clipboard::new);
        if finish == Finish::Copy {
            if let Err(e) = clipboard.set(&text) {
                self.status = format!("{e:#}");
            }
            return close;
        }
        let mut tasks = vec![close];
        if self.settings.insert_method == InsertMethod::Paste {
            let previous = self.settings.restore_clipboard.then(|| clipboard.get()).flatten();
            if let Err(e) = clipboard.set(&text) {
                self.status = format!("{e:#}");
                return Task::batch(tasks);
            }
            if let Some(previous) = previous {
                tasks.push(Task::perform(tokio::time::sleep(Duration::from_millis(1500)), move |_| {
                    Message::RestoreClipboard(previous.clone())
                }));
            }
        }
        // Give the window manager a moment to return focus to the target window.
        let target = popup.target;
        tasks.push(Task::perform(tokio::time::sleep(Duration::from_millis(250)), move |_| {
            Message::Deliver(text.clone(), target.clone())
        }));
        Task::batch(tasks)
    }

    fn open_settings(&mut self, tab: Tab) -> Task<Message> {
        if let Some(w) = &mut self.settings_window {
            w.tab = tab;
            return window::gain_focus(w.id);
        }
        self.theme = crate::theme::current();
        let (id, open) = window::open(window::Settings {
            size: Size::new(760.0, 620.0),
            min_size: Some(Size::new(560.0, 420.0)),
            position: window::Position::Centered,
            icon: window_icon(),
            platform_specific: platform_specific(),
            ..Default::default()
        });
        self.settings_window = Some(SettingsWindow {
            id,
            draft: self.settings.clone(),
            tab,
            microphones: engine::microphones(),
            saved_at: None,
            server_models: Ok(Vec::new()),
            ai_models: Ok(Vec::new()),
        });
        fetch_server_models(&self.settings.asr_server);
        fetch_ai_models(&self.settings.ai_server);
        open.then(window::gain_focus)
    }

    fn settings_message(&mut self, message: SettingsMessage) -> Task<Message> {
        use SettingsMessage as M;
        let Some(w) = &mut self.settings_window else { return Task::none() };
        let d = &mut w.draft;
        match message {
            M::Tab(tab) => w.tab = tab,
            M::Hotkey(s) => d.hotkey = s,
            M::HotkeyMode(m) => d.hotkey_mode = m,
            M::WakeEnabled(on) => d.wake_word_enabled = on,
            M::WakePackage(c) => {
                let needs_download = catalog::find(&c.id)
                    .is_some_and(|(_, p)| !p.is_installed(&self.settings.models_dir()));
                d.wake_word_package = c.id.clone();
                if needs_download {
                    self.start_download(&c.id);
                }
            }
            M::WakePhrase(s) => d.wake_phrase = s,
            M::WakeThreshold(t) => d.wake_word_threshold = t,
            M::WakeTimeout(t) => d.wake_listen_timeout_s = t as u32,
            M::Microphone(c) => d.input_device = c.id,
            M::AsrServer(url) => d.asr_server = url,
            M::AsrServerModel(c) => d.asr_server_model = c.id,
            M::AiMode(mode) => d.ai_mode = mode,
            M::AiServer(url) => d.ai_server = url,
            M::AiModel(c) => d.ai_model = c.id,
            M::FetchAiModels => {
                w.ai_models = Ok(Vec::new());
                fetch_ai_models(&d.ai_server);
            }
            M::FetchServerModels => {
                w.server_models = Ok(Vec::new());
                fetch_server_models(&d.asr_server);
            }
            M::EnterAction(c) => {
                d.enter_action = if c.id == "copy" { FinishAction::Copy } else { FinishAction::Insert }
            }
            M::InsertMethod(c) => {
                d.insert_method = if c.id == "type" { InsertMethod::Type } else { InsertMethod::Paste }
            }
            M::PasteShortcut(s) => d.paste_shortcut = s,
            M::RestoreClipboard(on) => d.restore_clipboard = on,
            M::VoiceCommands(on) => d.voice_commands = on,
            M::CommandLanguage(language) => {
                d.command_phrases = d.command_phrases();
                d.command_language = language;
                if language != Language::Custom {
                    d.command_phrases.clear();
                }
            }
            M::Phrase(key, phrases) => {
                if d.command_language != Language::Custom {
                    d.command_phrases = d.command_phrases();
                    d.command_language = Language::Custom;
                }
                d.command_phrases.insert(key.to_string(), phrases);
            }
            M::Save => {
                let hotkey_changed = d.hotkey != self.settings.hotkey;
                self.settings = d.clone();
                self.grammar = self.settings.grammar();
                w.saved_at = Some(Instant::now());
                if let Err(e) = self.settings.save() {
                    self.status = format!("Saving settings: {e:#}");
                }
                if hotkey_changed
                    && let Some(h) = &mut self.hotkeys
                    && let Err(e) = h.bind(&self.settings.hotkey)
                {
                    self.status = format!("{e:#}");
                }
                self.engine.send(EngineCommand::Configure(Box::new(self.settings.clone())));
            }
            M::Close => return window::close(w.id),
        }
        Task::none()
    }

    fn start_download(&mut self, id: &str) {
        let Some((_, package)) = catalog::find(id) else { return };
        if self.downloads.contains_key(id) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.downloads.insert(id.to_string(), (Progress::default(), cancel.clone()));
        let dir = self.settings.models_dir();
        let id = id.to_string();
        std::thread::spawn(move || {
            let mut last = Instant::now();
            let result = download::install(package, &dir, &cancel, |p| {
                if last.elapsed() > Duration::from_millis(200) {
                    last = Instant::now();
                    bus::send(BusEvent::DownloadProgress(id.clone(), p));
                }
            });
            bus::send(BusEvent::DownloadFinished(id, result.map_err(|e| format!("{e:#}"))));
        });
    }

    fn view(&self, id: window::Id) -> Element<'_, Message> {
        if let Some(p) = self.popup.as_ref().filter(|p| p.id == id) {
            return self.popup_view(p);
        }
        if let Some(w) = self.settings_window.as_ref().filter(|w| w.id == id) {
            return self.settings_view(w).map(Message::Settings);
        }
        space().into()
    }

    fn popup_view<'a>(&'a self, p: &'a Popup) -> Element<'a, Message> {
        let enter = self.settings.enter_action;
        let editor = text_editor(&p.content)
            .id(EDITOR)
            .placeholder("Speak, or type…")
            .on_action(Message::Edit)
            .height(Length::Fill)
            .size(17)
            .padding(10)
            .key_binding(move |key_press| {
                use keyboard::key::Named;
                let ctrl = key_press.modifiers.command();
                match key_press.key.as_ref() {
                    keyboard::Key::Named(Named::Enter) if !key_press.modifiers.shift() => {
                        let insert = (enter == FinishAction::Insert) != ctrl;
                        Some(text_editor::Binding::Custom(Message::Finish(if insert {
                            Finish::Insert
                        } else {
                            Finish::Copy
                        })))
                    }
                    keyboard::Key::Named(Named::Escape) => {
                        Some(text_editor::Binding::Custom(Message::Finish(Finish::Cancel)))
                    }
                    _ => text_editor::Binding::from_key_press(key_press),
                }
            });

        // Pause | Dictate | Edit: one of them is always lit.
        let state_button = |label: &'static str, on: bool, message: Message| {
            button(text(label).size(13))
                .padding([3, 10])
                .on_press(message)
                .style(if on { button::primary } else { secondary_button })
        };
        let listening = self.listening;
        let states = row![
            state_button("Pause", !listening, Message::Pause),
            state_button("Dictate", listening && p.mode == Mode::Dictate, Message::SetMode(Mode::Dictate)),
            state_button("Edit", listening && p.mode == Mode::Edit, Message::SetMode(Mode::Edit)),
        ]
        .spacing(2);
        let header = row![
            states,
            progress_bar(0.0..=1.0, if listening { self.level } else { 0.0 })
                .length(Length::Fixed(110.0))
                .girth(6),
            space::horizontal(),
            text(&self.status).size(12),
            button(gear()).padding(4).on_press(Message::OpenSettings).style(button::text),
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        let mut body = column![header, editor].spacing(8);
        if !p.preview.is_empty() {
            body = body.push(text(format!("… {}", p.preview)).size(14).font(Font {
                style: iced::font::Style::Italic,
                ..Font::DEFAULT
            }));
        }
        if let Some(n) = &p.notice {
            body = body.push(text(n).size(13));
        } else if !listening && self.settings.voice_commands {
            let say = |c: commands::Command| self.grammar.first_phrase(&c).unwrap_or("").to_string();
            body = body.push(
                text(format!(
                    "Paused. Say “{}” or “{}” to go on.",
                    say(commands::Command::DictateMode),
                    say(commands::Command::EditMode)
                ))
                .size(12),
            );
        } else if p.tidying {
            body = body.push(text("Tidying up…").size(12));
        }
        let (enter_label, ctrl_label) = match enter {
            FinishAction::Insert => ("Insert (Enter)", "Copy (Ctrl+Enter)"),
            FinishAction::Copy => ("Copy (Enter)", "Insert (Ctrl+Enter)"),
        };
        let (enter_finish, ctrl_finish) = match enter {
            FinishAction::Insert => (Finish::Insert, Finish::Copy),
            FinishAction::Copy => (Finish::Copy, Finish::Insert),
        };
        let buttons = row![
            space::horizontal(),
            button("Cancel (Esc)").on_press(Message::Finish(Finish::Cancel)).style(secondary_button),
            button(ctrl_label).on_press(Message::Finish(ctrl_finish)).style(secondary_button),
            button(enter_label).on_press(Message::Finish(enter_finish)),
        ]
        .spacing(8);
        container(body.push(buttons)).padding(12).into()
    }

    fn settings_view<'a>(&'a self, w: &'a SettingsWindow) -> Element<'a, SettingsMessage> {
        use SettingsMessage as M;
        let tab_button = |label: &'static str, tab: Tab| {
            button(label)
                .on_press(M::Tab(tab))
                .style(if w.tab == tab { button::primary } else { button::text })
        };
        let tabs = row![
            tab_button("General", Tab::General),
            tab_button("Speech", Tab::Speech),
            tab_button("Output", Tab::Output),
            tab_button("Voice commands", Tab::Commands),
        ]
        .spacing(4);

        let page = match w.tab {
            Tab::General => self.general_page(w),
            Tab::Speech => self.speech_page(w),
            Tab::Output => output_page(&w.draft),
            Tab::Commands => commands_page(&w.draft),
        };
        let dirty = w.draft != self.settings;
        let saved = w.saved_at.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) && !dirty;
        let footer = row![
            text(&self.status).size(12),
            space::horizontal(),
            text(if saved { "Saved" } else { "" }).size(13),
            button("Close").on_press(M::Close).style(secondary_button),
            button("Save").on_press_maybe(dirty.then_some(M::Save)),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        container(
            column![tabs, rule::horizontal(1), scrollable(container(page).padding([4, 12])).height(Length::Fill), footer]
                .spacing(10),
        )
        .padding(12)
        .into()
    }

    fn general_page<'a>(&'a self, w: &'a SettingsWindow) -> Element<'a, SettingsMessage> {
        use SettingsMessage as M;
        let d = &w.draft;
        let modes = [HotkeyMode::Toggle, HotkeyMode::PushToTalk];
        let hotkey_hint = if hotkey::is_wayland() {
            "On Wayland the desktop owns global shortcuts: it asks to confirm this one the first time, and you change it later in its shortcut settings. You can also bind the command `diktatoren toggle`."
        } else {
            "For example Ctrl+Alt+Space, Super+D or F9."
        };
        let mut wake_packages = vec![choice(wake::PHRASE, "Spoken phrase (your own words)")];
        wake_packages.extend(self.wake_choices());
        let wake_selected = wake_packages.iter().find(|c| c.id == d.wake_word_package).cloned();
        let phrase_mode = d.wake_word_package == wake::PHRASE;
        let sensitivity: Element<'_, SettingsMessage> = if phrase_mode {
            space().into()
        } else {
            labeled(
                    "Sensitivity",
                    row![
                        slider(0.0_f32..=0.99_f32, d.wake_word_threshold, M::WakeThreshold).step(0.01_f32).width(200),
                        text(if d.wake_word_threshold == 0.0 {
                            "model default".to_string()
                        } else {
                            format!("threshold {:.2}", d.wake_word_threshold)
                        })
                        .size(13)
                    ]
                    .spacing(10),
                )
        };
        let mut mics = vec![choice("", "System default")];
        mics.extend(w.microphones.iter().map(|m| choice(m, m)));
        let mic_selected = mics.iter().find(|c| c.id == d.input_device).cloned();

        column![
            section("Activation"),
            labeled("Hotkey", text_input("Ctrl+Alt+Space", &d.hotkey).on_input(M::Hotkey).width(240)),
            hint(hotkey_hint),
            labeled(
                "Hotkey works as",
                pick_list(modes, Some(d.hotkey_mode), M::HotkeyMode).width(240),
            ),
            hint("Toggle: press to start, press again to stop. Push to talk: listens while held."),
            checkbox(d.wake_word_enabled).label("Start dictation with a wake word").on_toggle(M::WakeEnabled),
            labeled("Wake word", pick_list(wake_packages, wake_selected, M::WakePackage).width(320)),
            if phrase_mode {
                column![
                    labeled("Phrase", text_input("Hey Diktatoren", &d.wake_phrase).on_input(M::WakePhrase).width(320)),
                    hint("Say the phrase on its own, or go straight on: “Hey Diktatoren, remind me to …”. While waiting, short utterances go to your speech server, are compared with the phrase, and are discarded."),
                ]
                .spacing(10)
                .into()
            } else if let Some((progress, _)) = self.downloads.get(&d.wake_word_package) {
                let fraction = progress.total.map_or(0.0, |t| progress.done as f32 / t.max(1) as f32);
                row![text("Downloading…").size(13), progress_bar(0.0..=1.0, fraction).length(Length::Fixed(200.0)).girth(6)]
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .into()
            } else {
                hint::<SettingsMessage>("A wake-word model listens for one fixed phrase on this computer and uses very little power.")
            },
            sensitivity,
            labeled(
                "Close if silent for",
                row![
                    slider(5.0_f32..=120.0_f32, d.wake_listen_timeout_s as f32, M::WakeTimeout).step(5.0_f32).width(200),
                    text(format!("{} s", d.wake_listen_timeout_s)).size(13)
                ]
                .spacing(10),
            ),
            hint("After a wake word, the dictation window closes again if nothing is said for this long, to undo false wakes. Once you have said something, it stays open."),
            section("Microphone"),
            labeled("Input", pick_list(mics, mic_selected, M::Microphone).width(320)),
        ]
        .spacing(10)
        .into()
    }

    fn speech_page<'a>(&'a self, w: &'a SettingsWindow) -> Element<'a, SettingsMessage> {
        use SettingsMessage as M;
        let d = &w.draft;
        let models: Vec<Choice> = match &w.server_models {
            Ok(list) => list.iter().map(|m| choice(m, m)).collect(),
            Err(_) => Vec::new(),
        };
        let selected = models.iter().find(|c| c.id == d.asr_server_model).cloned();
        let status = match &w.server_models {
            _ if d.asr_server.trim().is_empty() => {
                "The address of an audio.cpp server (audiocpp_server), such as http://127.0.0.1:8099.".to_string()
            }
            Ok(list) if list.is_empty() => "Press Connect to list the server's speech models.".into(),
            Ok(_) => "Connected.".into(),
            Err(e) => e.clone(),
        };
        let connect = (!d.asr_server.trim().is_empty()).then_some(M::FetchServerModels);
        let ai_models: Vec<Choice> = match &w.ai_models {
            Ok(list) => list.iter().map(|m| choice(m, m)).collect(),
            Err(_) => Vec::new(),
        };
        let ai_selected = ai_models.iter().find(|c| c.id == d.ai_model).cloned();
        let ai_status = match &w.ai_models {
            _ if d.ai_server.trim().is_empty() => {
                "An OpenAI-compatible server, such as llama.cpp's llama-server or LM Studio.".to_string()
            }
            Ok(list) if list.is_empty() => "Press Connect to list the server's models.".into(),
            Ok(_) => "Connected.".into(),
            Err(e) => e.clone(),
        };
        let ai_connect = (!d.ai_server.trim().is_empty()).then_some(M::FetchAiModels);
        column![
            section("Speech recognition"),
            labeled(
                "Server",
                row![
                    text_input("http://127.0.0.1:8099", &d.asr_server)
                        .on_input(M::AsrServer)
                        .on_submit_maybe(connect.clone())
                        .width(300),
                    button("Connect").on_press_maybe(connect).style(secondary_button),
                ]
                .spacing(8),
            ),
            labeled(
                "Model",
                pick_list(models, selected, M::AsrServerModel).placeholder("Connect first").width(300),
            ),
            hint(status),
            section("AI clean-up"),
            labeled(
                "Clean up dictation",
                pick_list([AiMode::Off, AiMode::Punctuate, AiMode::Correct], Some(d.ai_mode), M::AiMode).width(300),
            ),
            hint("A language model adds punctuation and capitals to what you dictate, and with “fixes” also corrects words the recogniser misheard. Your words appear at once and are tidied a moment later; “undo” brings back what was heard. Answers that rewrite the text are ignored."),
            labeled(
                "Server",
                row![
                    text_input("http://127.0.0.1:8003", &d.ai_server)
                        .on_input(M::AiServer)
                        .on_submit_maybe(ai_connect.clone())
                        .width(300),
                    button("Connect").on_press_maybe(ai_connect).style(secondary_button),
                ]
                .spacing(8),
            ),
            labeled(
                "Model",
                pick_list(ai_models, ai_selected, M::AiModel).placeholder("Connect first").width(300),
            ),
            hint(ai_status),
        ]
        .spacing(10)
        .into()
    }

    /// Wake-word models this build can run; ones not yet downloaded download when chosen.
    fn wake_choices(&self) -> Vec<Choice> {
        let dir = self.settings.models_dir();
        catalog::families(ModelTask::WakeWord, &self.available)
            .into_iter()
            .flat_map(|f| f.packages.iter())
            .map(|p| {
                let name = p.name.trim_end_matches(" GGUF F32").to_string();
                if p.is_installed(&dir) {
                    choice(&p.id, &name)
                } else {
                    choice(&p.id, &format!("{name} (downloads when chosen)"))
                }
            })
            .collect()
    }
}

fn output_page(d: &Settings) -> Element<'_, SettingsMessage> {
    use SettingsMessage as M;
    let actions = vec![choice("insert", "Insert into the window"), choice("copy", "Copy to the clipboard")];
    let enter = actions[if d.enter_action == FinishAction::Copy { 1 } else { 0 }].clone();
    let methods = vec![choice("paste", "Paste from the clipboard"), choice("type", "Type the text")];
    let method = methods[if d.insert_method == InsertMethod::Type { 1 } else { 0 }].clone();
    column![
        section("Finishing"),
        labeled("Enter does", pick_list(actions, Some(enter), M::EnterAction).width(260)),
        hint("Ctrl+Enter does the other. Shift+Enter starts a new line; Esc cancels."),
        section("Inserting"),
        labeled("Insert by", pick_list(methods, Some(method), M::InsertMethod).width(260)),
        hint("Pasting is fast and handles any characters. Typing works where pasting is blocked."),
        labeled("Paste shortcut", text_input("Ctrl+V", &d.paste_shortcut).on_input(M::PasteShortcut).width(160)),
        hint("Terminals often want Ctrl+Shift+V or Shift+Insert."),
        checkbox(d.restore_clipboard)
            .label("Put the previous clipboard contents back after pasting")
            .on_toggle(M::RestoreClipboard),
    ]
    .spacing(10)
    .into()
}

fn commands_page(d: &Settings) -> Element<'_, SettingsMessage> {
    use SettingsMessage as M;
    let phrases = d.command_phrases();
    let languages = [Language::English, Language::Danish, Language::Custom];
    let list = |control: bool| {
        let mut list = column![].spacing(6);
        for spec in commands::specs().into_iter().filter(|s| s.control == control) {
            let key = spec.key;
            let value = phrases.get(key).cloned().unwrap_or_default();
            list = list.push(
                row![
                    text(spec.label).size(14).width(230),
                    text_input("", &value).on_input(move |v| M::Phrase(key, v)).size(14),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            );
        }
        list
    };
    column![
        checkbox(d.voice_commands).label("Recognise voice commands").on_toggle(M::VoiceCommands),
        labeled("Language", pick_list(languages, Some(d.command_language), M::CommandLanguage).width(200)),
        hint("Say a command on its own, with a short pause before and after. Each command takes the phrases listed, separated by commas; editing one makes the set Custom. The dictation window has two modes, switched by voice or with its Dictate / Edit buttons."),
        section("In both modes"),
        list(true),
        section("In Edit mode"),
        hint("While editing, nothing you say is written as text unless something is selected. “Replace mandag med tirsdag” combines a replace verb and a separator."),
        list(false),
    ]
    .spacing(10)
    .into()
}

fn section<'a, M: 'a>(title: &'a str) -> Element<'a, M> {
    column![space().height(6), text(title).size(18)].into()
}

fn hint<'a, M: 'a>(s: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(s).size(12).into()
}

fn labeled<'a, M: 'a>(label: &'a str, control: impl Into<Element<'a, M>>) -> Element<'a, M> {
    row![text(label).width(170), control.into()].spacing(10).align_y(Alignment::Center).into()
}

impl std::fmt::Display for HotkeyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HotkeyMode::Toggle => "Toggle",
            HotkeyMode::PushToTalk => "Push to talk",
        })
    }
}

fn bus_stream() -> impl futures::Stream<Item = BusEvent> {
    bus::take_receiver().unwrap_or_else(|| futures::channel::mpsc::unbounded().1)
}

/// A desktop-style button: the window background lifted slightly, with a thin border.
fn secondary_button(theme: &iced::Theme, status: button::Status) -> button::Style {
    let palette = theme.palette();
    let mix = |a: iced::Color, b: iced::Color, t: f32| iced::Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    };
    let lift = match status {
        button::Status::Hovered => 0.14,
        button::Status::Pressed => 0.18,
        _ => 0.07,
    };
    let border = match status {
        button::Status::Hovered | button::Status::Pressed => palette.primary,
        _ => mix(palette.background, palette.text, 0.22),
    };
    button::Style {
        background: Some(mix(palette.background, palette.text, lift).into()),
        text_color: if status == button::Status::Disabled {
            mix(palette.background, palette.text, 0.45)
        } else {
            palette.text
        },
        border: iced::Border { color: border, width: 1.0, radius: 4.0.into() },
        ..Default::default()
    }
}

/// A settings cog, drawn in the theme's text colour.
fn gear<'a>() -> Element<'a, Message> {
    static SVG: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let svg = SVG.get_or_init(|| gear_svg().into_bytes());
    iced::widget::svg(iced::widget::svg::Handle::from_memory(svg.clone()))
        .width(18)
        .height(18)
        .style(|theme: &iced::Theme, _| iced::widget::svg::Style { color: Some(theme.palette().text) })
        .into()
}

/// The outline of eight trapezoid teeth around (12, 12).
fn gear_outline() -> Vec<(f32, f32)> {
    let (outer, inner) = (11.0f32, 8.2f32);
    (0..8)
        .flat_map(|tooth| {
            let base = tooth as f32 * 45.0;
            [(-15.0, inner), (-9.0, outer), (9.0, outer), (15.0, inner)].map(|(offset, r)| {
                let a = (base + offset).to_radians();
                (12.0 + r * a.cos(), 12.0 + r * a.sin())
            })
        })
        .collect()
}

/// The gear with a round hole, which even-odd filling cuts out.
fn gear_svg() -> String {
    let hole = 3.4f32;
    let mut path = String::new();
    for (i, (x, y)) in gear_outline().into_iter().enumerate() {
        path.push_str(&format!("{}{x:.2} {y:.2}", if i == 0 { "M" } else { "L" }));
    }
    path.push_str(&format!(
        "ZM{:.2} 12A{hole} {hole} 0 1 0 {:.2} 12A{hole} {hole} 0 1 0 {:.2} 12Z",
        12.0 + hole,
        12.0 - hole,
        12.0 + hole
    ));
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="black" fill-rule="evenodd" d="{path}"/></svg>"#
    )
}

fn window_icon() -> Option<window::Icon> {
    window::icon::from_rgba(icon::rgba(64, IconState::Idle), 64, 64).ok()
}

fn platform_specific() -> window::settings::PlatformSpecific {
    #[cfg(target_os = "linux")]
    return window::settings::PlatformSpecific {
        application_id: crate::desktop::APP_ID.into(),
        ..Default::default()
    };
    #[cfg(not(target_os = "linux"))]
    Default::default()
}

/// Rebuilds the editor from the document after a voice edit.
fn sync_editor(p: &mut Popup) {
    let text = p.doc.text();
    p.content = text_editor::Content::with_text(text);
    let sel = p.doc.selection();
    let head = position(text, sel.end);
    let anchor = (!sel.is_empty()).then(|| position(text, sel.start));
    p.content.move_to(text_editor::Cursor { position: head, selection: anchor });
}

fn position(text: &str, offset: usize) -> text_editor::Position {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count();
    let column = before.rfind('\n').map_or(before.len(), |i| before.len() - i - 1);
    text_editor::Position { line, column }
}

fn offset(text: &str, pos: text_editor::Position) -> usize {
    let mut start = 0;
    for (i, line) in text.split('\n').enumerate() {
        if i == pos.line {
            return start + pos.column.min(line.len());
        }
        start += line.len() + 1;
    }
    text.len()
}

fn selection_range(text: &str, cursor: text_editor::Cursor) -> std::ops::Range<usize> {
    let head = offset(text, cursor.position);
    let anchor = cursor.selection.map_or(head, |s| offset(text, s));
    head.min(anchor)..head.max(anchor)
}

/// Lists the models of a language-model server in the background.
fn fetch_ai_models(url: &str) {
    let url = url.trim().to_string();
    if url.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let models = crate::ai::models(&url).map_err(|e| format!("{e:#}"));
        bus::send(BusEvent::AiModels(url, models));
    });
}

/// Lists the speech models of an audio.cpp server in the background.
fn fetch_server_models(url: &str) {
    let url = url.trim().to_string();
    if url.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let models = crate::remote::asr_models(&url).map_err(|e| format!("{e:#}"));
        bus::send(BusEvent::ServerModels(url, models));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gear_is_centred() {
        let outline = gear_outline();
        let n = outline.len() as f32;
        let (x, y) = outline.iter().fold((0.0, 0.0), |(x, y), p| (x + p.0, y + p.1));
        assert!((x / n - 12.0).abs() < 0.01 && (y / n - 12.0).abs() < 0.01, "centre {} {}", x / n, y / n);
        assert!(gear_svg().contains("M15.40 12A3.4"), "hole centred on (12, 12)");
    }

    #[test]
    fn offsets_round_trip() {
        let text = "første linje\nanden æøå\n\ntredje";
        for o in [0, 5, 13, 16, 24, 25, text.len()] {
            if text.is_char_boundary(o) {
                assert_eq!(offset(text, position(text, o)), o, "offset {o}");
            }
        }
    }
}
