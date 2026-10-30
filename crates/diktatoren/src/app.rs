//! The desktop application: a windowless daemon with a tray icon that opens the dictation
//! window on demand, plus the settings window.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, row, rule, scrollable, slider,
    space, text, text_editor, text_input,
};
use iced::{Alignment, Element, Font, Length, Size, Subscription, Task, keyboard, window};

use crate::bus::{self, Action, BusEvent};
use crate::catalog::{self, Package, Task as ModelTask};
use crate::commands;
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
    Models,
    Output,
    Commands,
}

#[derive(Debug, Clone)]
enum Message {
    Init(Option<Action>),
    Bus(BusEvent),
    Edit(text_editor::Action),
    Finish(Finish),
    ToggleListening,
    WindowClosed(window::Id),
    CloseRequested(window::Id),
    Deliver(String, Target),
    RestoreClipboard(String),
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
    AsrPackage(Choice),
    Language(String),
    Backend(String),
    Threads(String),
    Silence(f32),
    EnterAction(Choice),
    InsertMethod(Choice),
    PasteShortcut(String),
    RestoreClipboard(bool),
    VoiceCommands(bool),
    RequirePrefix(bool),
    Download(String),
    CancelDownload(String),
    Remove(String),
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
}

struct SettingsWindow {
    id: window::Id,
    draft: Settings,
    tab: Tab,
    microphones: Vec<String>,
    saved_at: Option<Instant>,
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
    sizes: HashMap<(String, String), u64>,
    status: String,
    listening: bool,
    level: f32,
}

impl App {
    fn boot(settings: Settings, initial: Option<Action>) -> (Self, Task<Message>) {
        let sink: engine::Sink = Arc::new(|event| bus::send(BusEvent::Engine(event)));
        let engine = engine::spawn(settings.clone(), sink);
        let available = audiocpp::Registry::new().map(|r| r.families()).unwrap_or_default();
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
            sizes: HashMap::new(),
            status: "Starting…".into(),
            listening: false,
            level: 0.0,
        };
        // Tray, hotkeys and clipboard need the event loop running on the main thread.
        (app, Task::done(Message::Init(initial)))
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
                let first_run = catalog::find(&self.settings.asr_package)
                    .is_none_or(|(_, p)| !p.is_installed(&self.settings.models_dir()));
                let mut tasks = Vec::new();
                if first_run {
                    tasks.push(self.open_settings(Tab::Models));
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
            Message::ToggleListening => {
                if self.listening {
                    self.engine.send(EngineCommand::StopListening);
                } else {
                    self.engine.send(EngineCommand::Listen { from_wake: false });
                }
                Task::none()
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
                    self.engine.send(EngineCommand::StopListening);
                }
                if self.settings_window.as_ref().is_some_and(|s| s.id == id) {
                    self.settings_window = None;
                }
                Task::none()
            }
            Message::Deliver(text, target) => {
                let settings = self.settings.clone();
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
            BusEvent::HotkeyPressed => match self.settings.hotkey_mode {
                HotkeyMode::Toggle => self.action(Action::Toggle),
                HotkeyMode::PushToTalk => self.action(Action::Start),
            },
            BusEvent::HotkeyReleased => {
                if self.settings.hotkey_mode == HotkeyMode::PushToTalk {
                    self.engine.send(EngineCommand::StopListening);
                }
                Task::none()
            }
            BusEvent::Action(action) => self.action(action),
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
                        // Use the first speech model downloaded, and load what is configured.
                        let installed = |pkg: &str| {
                            catalog::find(pkg).is_some_and(|(_, p)| p.is_installed(&self.settings.models_dir()))
                        };
                        if let Some((family, _)) = catalog::find(&id)
                            && family.tasks.iter().any(|t| t == "asr")
                            && !installed(&self.settings.asr_package)
                        {
                            self.settings.asr_package = id.clone();
                            if let Some(w) = &mut self.settings_window {
                                w.draft.asr_package = id.clone();
                            }
                            let _ = self.settings.save();
                        }
                        self.engine.send(EngineCommand::Configure(Box::new(self.settings.clone())));
                    }
                    Err(e) => self.status = format!("Download failed: {e}"),
                }
                Task::none()
            }
            BusEvent::RemoteSizes(sizes) => {
                for (repo, path, size) in sizes {
                    self.sizes.insert((repo, path), size);
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
            engine::Event::Level(level) => self.level = level,
            engine::Event::Preview(preview) => {
                if let Some(p) = &mut self.popup {
                    p.preview = preview;
                }
            }
            engine::Event::Utterance(utterance) => return self.utterance(utterance),
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
        let require_prefix = self.settings.require_command_prefix;
        let Some(p) = &mut self.popup else { return Task::none() };
        p.preview.clear();
        if spoken.trim().is_empty() {
            return Task::none();
        }
        let command = commands_enabled.then(|| commands::parse(&spoken, require_prefix)).flatten();
        let outcome = match command {
            Some(c) => {
                log::info!("voice command {c:?}");
                p.doc.apply(&c)
            }
            None => {
                p.doc.dictate(&spoken);
                Outcome::Edited
            }
        };
        p.notice = (outcome == Outcome::NotFound).then(|| format!("Nothing to change for “{spoken}”"));
        sync_editor(p);
        match outcome {
            Outcome::Insert => self.finish(Finish::Insert),
            Outcome::Copy => self.finish(Finish::Copy),
            Outcome::Cancel => self.finish(Finish::Cancel),
            Outcome::StopListening => {
                self.engine.send(EngineCommand::StopListening);
                Task::none()
            }
            Outcome::Edited | Outcome::NotFound => Task::none(),
        }
    }

    fn action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::Toggle => {
                if self.popup.is_none() {
                    self.engine.send(EngineCommand::Listen { from_wake: false });
                    self.open_popup()
                } else if self.listening {
                    self.engine.send(EngineCommand::StopListening);
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
                self.engine.send(EngineCommand::StopListening);
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

    fn finish(&mut self, finish: Finish) -> Task<Message> {
        let Some(popup) = self.popup.take() else { return Task::none() };
        self.engine.send(EngineCommand::StopListening);
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
        });
        if self.sizes.is_empty() {
            fetch_sizes();
        }
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
            M::WakePackage(c) => d.wake_word_package = c.id,
            M::WakePhrase(s) => d.wake_phrase = s,
            M::WakeThreshold(t) => d.wake_word_threshold = t,
            M::WakeTimeout(t) => d.wake_listen_timeout_s = t as u32,
            M::Microphone(c) => d.input_device = c.id,
            M::AsrPackage(c) => d.asr_package = c.id,
            M::Language(s) => d.language = s.trim().to_string(),
            M::Backend(s) => d.backend = s,
            M::Threads(s) => d.threads = s.trim().parse().unwrap_or(0),
            M::Silence(ms) => d.utterance_silence_ms = ms as u32,
            M::EnterAction(c) => {
                d.enter_action = if c.id == "copy" { FinishAction::Copy } else { FinishAction::Insert }
            }
            M::InsertMethod(c) => {
                d.insert_method = if c.id == "type" { InsertMethod::Type } else { InsertMethod::Paste }
            }
            M::PasteShortcut(s) => d.paste_shortcut = s,
            M::RestoreClipboard(on) => d.restore_clipboard = on,
            M::VoiceCommands(on) => d.voice_commands = on,
            M::RequirePrefix(on) => d.require_command_prefix = on,
            M::Download(id) => self.start_download(&id),
            M::CancelDownload(id) => {
                if let Some((_, cancel)) = self.downloads.get(&id) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            M::Remove(id) => {
                if let Some((_, package)) = catalog::find(&id)
                    && let Err(e) = download::remove(package, &self.settings.models_dir()) {
                        self.status = format!("{e:#}");
                    }
            }
            M::Save => {
                let hotkey_changed = d.hotkey != self.settings.hotkey;
                self.settings = d.clone();
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

        let state = if self.listening { "Listening" } else { "Paused" };
        let header = row![
            text(state).size(14),
            progress_bar(0.0..=1.0, if self.listening { self.level } else { 0.0 })
                .length(Length::Fixed(120.0))
                .girth(6),
            space::horizontal(),
            text(&self.status).size(12),
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
            button(if self.listening { "Pause" } else { "Listen" })
                .on_press(Message::ToggleListening)
                .style(button::secondary),
            space::horizontal(),
            button("Cancel (Esc)").on_press(Message::Finish(Finish::Cancel)).style(button::secondary),
            button(ctrl_label).on_press(Message::Finish(ctrl_finish)).style(button::secondary),
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
            tab_button("Models", Tab::Models),
            tab_button("Output", Tab::Output),
            tab_button("Voice commands", Tab::Commands),
        ]
        .spacing(4);

        let page = match w.tab {
            Tab::General => self.general_page(w),
            Tab::Speech => self.speech_page(w),
            Tab::Models => self.models_page(),
            Tab::Output => output_page(&w.draft),
            Tab::Commands => commands_page(&w.draft),
        };
        let dirty = w.draft != self.settings;
        let saved = w.saved_at.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) && !dirty;
        let footer = row![
            text(&self.status).size(12),
            space::horizontal(),
            text(if saved { "Saved" } else { "" }).size(13),
            button("Close").on_press(M::Close).style(button::secondary),
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
        wake_packages.extend(self.installed_choices(ModelTask::WakeWord));
        let wake_selected = wake_packages.iter().find(|c| c.id == d.wake_word_package).cloned();
        let phrase_mode = d.wake_word_package == wake::PHRASE;
        let sensitivity: Element<'_, SettingsMessage> = if phrase_mode {
            space().into()
        } else {
            labeled(
                    "Sensitivity",
                    row![
                        slider(0.0..=0.99, d.wake_word_threshold, M::WakeThreshold).step(0.01).width(200),
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
                    hint("Say the phrase on its own, or go straight on: “Hey Diktatoren, remind me to …”. While waiting, short utterances are recognised on this computer with your speech model, compared with the phrase and discarded."),
                ]
                .spacing(10)
                .into()
            } else {
                hint::<SettingsMessage>("Wake-word models listen for one fixed phrase and use very little power. Download them under Models.")
            },
            sensitivity,
            labeled(
                "Stop after silence",
                row![
                    slider(2.0..=30.0, d.wake_listen_timeout_s as f32, M::WakeTimeout).step(1.0).width(200),
                    text(format!("{} s", d.wake_listen_timeout_s)).size(13)
                ]
                .spacing(10),
            ),
            hint("After a wake word, listening stops when nothing has been said for this long."),
            section("Microphone"),
            labeled("Input", pick_list(mics, mic_selected, M::Microphone).width(320)),
        ]
        .spacing(10)
        .into()
    }

    fn speech_page<'a>(&'a self, w: &'a SettingsWindow) -> Element<'a, SettingsMessage> {
        use SettingsMessage as M;
        let d = &w.draft;
        let models = self.installed_choices(ModelTask::Asr);
        let selected = models.iter().find(|c| c.id == d.asr_package).cloned();
        let languages = catalog::find(&d.asr_package)
            .map(|(f, _)| f.language_summary())
            .unwrap_or_default();
        let backends = ["best", "cpu", "cuda", "vulkan", "metal"].map(String::from).to_vec();
        column![
            section("Recognition"),
            labeled(
                "Speech model",
                pick_list(models, selected, M::AsrPackage).placeholder("Download one under Models").width(360),
            ),
            hint(if languages.is_empty() { String::new() } else { format!("Languages: {languages}") }),
            labeled("Language", text_input("model default", &d.language).on_input(M::Language).width(160)),
            hint("A code such as da or en, for models that take one. Leave empty to let the model decide."),
            labeled(
                "End utterance after",
                row![
                    slider(300.0..=2000.0, d.utterance_silence_ms as f32, M::Silence).step(50.0).width(200),
                    text(format!("{} ms of silence", d.utterance_silence_ms)).size(13)
                ]
                .spacing(10),
            ),
            section("Performance"),
            labeled("Compute", pick_list(backends, Some(d.backend.clone()), M::Backend).width(160)),
            hint("best picks the GPU when this build supports one."),
            labeled(
                "CPU threads",
                text_input("auto", &if d.threads == 0 { String::new() } else { d.threads.to_string() })
                    .on_input(M::Threads)
                    .width(100),
            ),
        ]
        .spacing(10)
        .into()
    }

    fn models_page(&self) -> Element<'_, SettingsMessage> {
        let dir = self.settings.models_dir();
        let mut page = column![hint(format!("Models are stored in {}", dir.display()))].spacing(10);
        for (task, title) in [(ModelTask::Asr, "Speech recognition"), (ModelTask::WakeWord, "Wake words")] {
            page = page.push(section(title));
            for family in catalog::families(task, &self.available) {
                let mut block = column![
                    row![
                        text(&family.name).size(16),
                        text(family.language_summary()).size(13),
                    ]
                    .spacing(10)
                    .align_y(Alignment::End),
                    hint(&family.description),
                ]
                .spacing(4);
                for package in &family.packages {
                    block = block.push(self.package_row(package, &dir));
                }
                page = page.push(container(block).padding(8).style(container::rounded_box));
            }
        }
        page.into()
    }

    fn package_row<'a>(&'a self, package: &'a Package, dir: &std::path::Path) -> Element<'a, SettingsMessage> {
        use SettingsMessage as M;
        let size: Option<u64> = package
            .files
            .iter()
            .map(|f| self.sizes.get(&(package.repo.clone(), f.clone())).copied())
            .sum();
        let in_use = package.id == self.settings.asr_package
            || (self.settings.wake_word_enabled && package.id == self.settings.wake_word_package);
        let mut r = row![
            text(&package.name).size(14).width(Length::Fill),
            text(size.map(download::human_size).unwrap_or_default()).size(13).width(80),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        if let Some((progress, _)) = self.downloads.get(&package.id) {
            let fraction = progress.total.map_or(0.0, |t| progress.done as f32 / t.max(1) as f32);
            r = r
                .push(progress_bar(0.0..=1.0, fraction).length(Length::Fixed(140.0)).girth(8))
                .push(button("Cancel").on_press(M::CancelDownload(package.id.clone())).style(button::secondary));
        } else if package.is_installed(dir) {
            r = r.push(text(if in_use { "In use" } else { "Installed" }).size(13).width(70)).push(
                button("Remove")
                    .on_press_maybe((!in_use).then(|| M::Remove(package.id.clone())))
                    .style(button::danger),
            );
        } else {
            r = r.push(button("Download").on_press(M::Download(package.id.clone())));
        }
        r.into()
    }

    fn installed_choices(&self, task: ModelTask) -> Vec<Choice> {
        let dir = self.settings.models_dir();
        catalog::families(task, &self.available)
            .into_iter()
            .flat_map(|f| f.packages.iter())
            .filter(|p| p.is_installed(&dir))
            .map(|p| choice(&p.id, &p.name))
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
    let rows: &[(&str, &str)] = &[
        ("Insert · Indsæt", "insert the text into the window"),
        ("Copy · Kopiér", "copy the text to the clipboard"),
        ("Cancel · Annuller", "close without doing anything"),
        ("Pause · Stop med at lytte", "stop listening"),
        ("New line · Ny linje", "line break"),
        ("New paragraph · Nyt afsnit", "blank line"),
        ("Period, comma, question mark … · Punktum, komma, spørgsmålstegn …", "punctuation"),
        ("Undo · Fortryd   /   Redo · Gentag", "undo or redo the last change"),
        ("Delete that · Slet det", "remove the selection or the last dictated piece"),
        ("Delete last word / sentence · Slet sidste ord / sætning", ""),
        ("Delete X · Slet X", "remove the last occurrence of X"),
        ("Replace X with Y · Erstat X med Y · Ret X til Y", "fix a word"),
        ("Select X · Markér X", "select X; what you say next replaces it"),
        ("Select all · Markér alt   /   Clear all · Slet alt", ""),
        ("Go to end / start · Gå til slutningen / starten", "move the cursor"),
    ];
    let mut list = column![].spacing(6);
    for (phrase, meaning) in rows {
        list = list.push(row![text(*phrase).size(14).width(Length::FillPortion(3)), text(*meaning).size(13).width(Length::FillPortion(2))].spacing(10));
    }
    column![
        section("Editing by voice"),
        checkbox(d.voice_commands).label("Recognise voice commands").on_toggle(M::VoiceCommands),
        checkbox(d.require_command_prefix)
            .label("Only after saying “command” or “kommando” first")
            .on_toggle(M::RequirePrefix),
        hint("A command must be said on its own, with a short pause before and after. Dictated text lands at the cursor; click or select text in the box (or say “select …”) to change where it goes."),
        list,
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

fn window_icon() -> Option<window::Icon> {
    window::icon::from_rgba(icon::rgba(64, IconState::Idle), 64, 64).ok()
}

fn platform_specific() -> window::settings::PlatformSpecific {
    #[cfg(target_os = "linux")]
    return window::settings::PlatformSpecific {
        application_id: "diktatoren".into(),
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

/// Looks up file sizes for the models page in the background.
fn fetch_sizes() {
    std::thread::spawn(|| {
        let mut repos: Vec<(String, String)> = catalog::all()
            .iter()
            .flat_map(|f| f.packages.iter().map(|p| (p.repo.clone(), p.revision.clone())))
            .collect();
        repos.sort();
        repos.dedup();
        let mut sizes = Vec::new();
        for (repo, revision) in repos {
            let url = format!("https://huggingface.co/api/models/{repo}/tree/{revision}?recursive=true");
            let Ok(response) = ureq::get(&url).call() else { continue };
            let Ok(body) = response.into_body().read_to_string() else { continue };
            let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(&body) else { continue };
            for e in entries {
                if let (Some(path), Some(size)) = (e["path"].as_str(), e["size"].as_u64()) {
                    sizes.push((repo.clone(), path.to_string(), size));
                }
            }
        }
        bus::send(BusEvent::RemoteSizes(sizes));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
