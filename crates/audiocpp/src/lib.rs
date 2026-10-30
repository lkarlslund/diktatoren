//! Safe bindings to the audio.cpp C API.
//!
//! The C API keeps parents alive from their children (a session holds its model, a model its
//! registry), so the wrappers here carry no lifetimes and may be dropped in any order.

mod ffi;

use std::ffi::{CStr, CString, c_char};
use std::fmt;
use std::path::Path;
use std::ptr;

#[derive(Debug, Clone)]
pub struct Error {
    pub status: i32,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            write!(f, "audio.cpp error {}", self.status)
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn borrowed(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

fn check(status: ffi::audiocpp_status) -> Result<()> {
    if status == ffi::AUDIOCPP_OK {
        return Ok(());
    }
    let detail = borrowed(unsafe { ffi::audiocpp_last_error() });
    let message = if detail.is_empty() {
        borrowed(unsafe { ffi::audiocpp_status_string(status) })
    } else {
        detail
    };
    Err(Error { status, message })
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error {
        status: 1,
        message: format!("string contains a NUL byte: {s:?}"),
    })
}

/// The library version string, such as `0.9.3 (141ec6f)`.
pub fn build_version() -> String {
    borrowed(unsafe { ffi::audiocpp_build_version() })
}

/// True when the loaded library speaks the ABI these bindings were written against.
pub fn abi_compatible() -> bool {
    unsafe { ffi::audiocpp_abi_version() >> 16 == ffi::AUDIOCPP_ABI_VERSION_MAJOR }
}

struct Options(*mut ffi::audiocpp_options);

impl Options {
    fn new(pairs: &[(&str, &str)]) -> Result<Self> {
        let options = Options(unsafe { ffi::audiocpp_options_create() });
        for (key, value) in pairs {
            let (key, value) = (cstring(key)?, cstring(value)?);
            check(unsafe { ffi::audiocpp_options_set(options.0, key.as_ptr(), value.as_ptr()) })?;
        }
        Ok(options)
    }
}

impl Drop for Options {
    fn drop(&mut self) {
        unsafe { ffi::audiocpp_options_free(self.0) }
    }
}

/// The set of model loaders compiled into the library.
pub struct Registry(*mut ffi::audiocpp_registry);

unsafe impl Send for Registry {}

impl Registry {
    pub fn new() -> Result<Self> {
        if !abi_compatible() {
            return Err(Error {
                status: 1,
                message: "audio.cpp library has an incompatible ABI version".into(),
            });
        }
        let mut registry = ptr::null_mut();
        check(unsafe { ffi::audiocpp_registry_create(ptr::null(), &mut registry) })?;
        Ok(Registry(registry))
    }

    /// Model families this build can load.
    pub fn families(&self) -> Vec<String> {
        let count = unsafe { ffi::audiocpp_registry_family_count(self.0) };
        (0..count)
            .filter_map(|i| {
                let mut name = ptr::null();
                check(unsafe { ffi::audiocpp_registry_family(self.0, i, &mut name) }).ok()?;
                Some(borrowed(name))
            })
            .collect()
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        unsafe { ffi::audiocpp_registry_free(self.0) }
    }
}

/// Which option map a [`ModelOption`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionScope {
    Request = 0,
    Session = 1,
    Load = 2,
}

#[derive(Debug, Clone)]
pub struct ModelOption {
    pub name: String,
    pub description: String,
    pub default_value: String,
    pub min_value: String,
    pub max_value: String,
    pub required: bool,
}

/// Loaded model weights.
pub struct Model(*mut ffi::audiocpp_model);

unsafe impl Send for Model {}

impl Model {
    /// Loads a model from a GGUF file or a model directory. `family` may be `None` to let the
    /// library identify the model from its files.
    pub fn load(registry: &Registry, path: &Path, family: Option<&str>) -> Result<Self> {
        let path = cstring(&path.to_string_lossy())?;
        let family = family.map(cstring).transpose()?;
        let config = ffi::audiocpp_model_config {
            family_hint: family.as_ref().map_or(ptr::null(), |f| f.as_ptr()),
            config_id: ptr::null(),
            weight_id: ptr::null(),
            model_spec_override: ptr::null(),
        };
        let mut model = ptr::null_mut();
        check(unsafe {
            ffi::audiocpp_model_load(registry.0, path.as_ptr(), &config, ptr::null(), &mut model)
        })?;
        Ok(Model(model))
    }

    pub fn family(&self) -> String {
        borrowed(unsafe { ffi::audiocpp_model_family(self.0) })
    }

    /// `task` is a C API task token such as `asr`, `vad` or `wake`; `mode` is `offline` or
    /// `streaming`.
    pub fn supports(&self, task: &str, mode: &str) -> bool {
        let (Ok(task), Ok(mode)) = (cstring(task), cstring(mode)) else {
            return false;
        };
        unsafe { ffi::audiocpp_model_supports(self.0, task.as_ptr(), mode.as_ptr()) == 1 }
    }

    pub fn languages(&self) -> Vec<String> {
        let count = unsafe { ffi::audiocpp_model_language_count(self.0) };
        (0..count)
            .filter_map(|i| {
                let mut lang = ptr::null();
                check(unsafe { ffi::audiocpp_model_language(self.0, i, &mut lang) }).ok()?;
                Some(borrowed(lang))
            })
            .collect()
    }

    pub fn options(&self, scope: OptionScope) -> Vec<ModelOption> {
        let count = unsafe { ffi::audiocpp_model_option_count(self.0, scope as i32) };
        (0..count)
            .filter_map(|i| {
                let mut name = ptr::null();
                let mut description = ptr::null();
                let mut default_value = ptr::null();
                let mut min_value = ptr::null();
                let mut max_value = ptr::null();
                let mut required = 0;
                check(unsafe {
                    ffi::audiocpp_model_option(
                        self.0,
                        scope as i32,
                        i,
                        &mut name,
                        ptr::null_mut(),
                        &mut description,
                        &mut default_value,
                        &mut min_value,
                        &mut max_value,
                        &mut required,
                    )
                })
                .ok()?;
                Some(ModelOption {
                    name: borrowed(name),
                    description: borrowed(description),
                    default_value: borrowed(default_value),
                    min_value: borrowed(min_value),
                    max_value: borrowed(max_value),
                    required: required != 0,
                })
            })
            .collect()
    }

    /// True when the model declares a request option with this name.
    pub fn has_request_option(&self, name: &str) -> bool {
        self.options(OptionScope::Request).iter().any(|o| o.name == name)
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        unsafe { ffi::audiocpp_model_free(self.0) }
    }
}

/// Where a session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    /// `cpu`, `cuda`, `hip`, `vulkan`, `metal` or `best`.
    pub name: String,
    pub device: i32,
    pub threads: i32,
}

impl Default for Backend {
    fn default() -> Self {
        Backend {
            name: "best".into(),
            device: 0,
            threads: std::thread::available_parallelism().map_or(4, |n| n.get().min(8) as i32),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Offline,
    Streaming,
}

impl Mode {
    fn token(self) -> &'static str {
        match self {
            Mode::Offline => "offline",
            Mode::Streaming => "streaming",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Segment {
    pub start_sample: i64,
    pub end_sample: i64,
    pub confidence: f32,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct TaskResult {
    pub text: Option<String>,
    pub language: String,
    pub segments: Vec<Segment>,
}

impl TaskResult {
    fn read(result: *const ffi::audiocpp_result) -> Self {
        let mut text = ptr::null();
        let mut language = ptr::null();
        let status = unsafe { ffi::audiocpp_result_text(result, &mut text, &mut language) };
        let (text, language) = if status == ffi::AUDIOCPP_OK {
            (Some(borrowed(text)), borrowed(language))
        } else {
            (None, String::new())
        };
        let count = unsafe { ffi::audiocpp_result_segment_count(result) };
        let segments = (0..count)
            .filter_map(|i| {
                let mut segment = Segment::default();
                let mut text = ptr::null();
                let status = unsafe {
                    ffi::audiocpp_result_segment(
                        result,
                        i,
                        &mut segment.start_sample,
                        &mut segment.end_sample,
                        &mut segment.confidence,
                        &mut text,
                    )
                };
                (status == ffi::AUDIOCPP_OK).then(|| {
                    segment.text = borrowed(text);
                    segment
                })
            })
            .collect();
        TaskResult { text, language, segments }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceActivityKind {
    SpeechStart,
    SpeechEnd,
    /// A complete segment, which is also how wake-word detections are reported.
    SpeechSegment,
}

#[derive(Debug, Clone, Copy)]
pub struct VoiceActivity {
    pub kind: VoiceActivityKind,
    pub sample: i64,
    pub probability: f32,
}

/// One streaming event.
#[derive(Debug, Clone, Default)]
pub struct Event {
    pub is_final: bool,
    pub result: TaskResult,
    pub voice_activity: Vec<VoiceActivity>,
}

impl Event {
    fn take(event: *mut ffi::audiocpp_event) -> Option<Self> {
        if event.is_null() {
            return None;
        }
        let is_final = unsafe { ffi::audiocpp_event_is_final(event) } != 0;
        let result = TaskResult::read(unsafe { ffi::audiocpp_event_as_result(event) });
        let count = unsafe { ffi::audiocpp_event_voice_activity_count(event) };
        let voice_activity = (0..count)
            .filter_map(|i| {
                let mut kind = 0;
                let mut sample = 0;
                let mut probability = 0.0;
                let status = unsafe {
                    ffi::audiocpp_event_voice_activity(
                        event,
                        i,
                        &mut kind,
                        &mut sample,
                        &mut probability,
                    )
                };
                let kind = match kind {
                    ffi::AUDIOCPP_VOICE_ACTIVITY_SPEECH_START => VoiceActivityKind::SpeechStart,
                    ffi::AUDIOCPP_VOICE_ACTIVITY_SPEECH_END => VoiceActivityKind::SpeechEnd,
                    ffi::AUDIOCPP_VOICE_ACTIVITY_SPEECH_SEGMENT => VoiceActivityKind::SpeechSegment,
                    _ => return None,
                };
                (status == ffi::AUDIOCPP_OK).then_some(VoiceActivity { kind, sample, probability })
            })
            .collect();
        unsafe { ffi::audiocpp_event_free(event) };
        Some(Event { is_final, result, voice_activity })
    }

    pub fn is_empty(&self) -> bool {
        self.voice_activity.is_empty()
            && self.result.segments.is_empty()
            && self.result.text.as_deref().is_none_or(str::is_empty)
    }
}

struct Request(*mut ffi::audiocpp_request);

impl Request {
    fn new(audio: Option<(&[f32], i32)>, options: &[(&str, &str)]) -> Result<Self> {
        let request = Request(unsafe { ffi::audiocpp_request_create() });
        if let Some((samples, rate)) = audio {
            check(unsafe {
                ffi::audiocpp_request_set_audio(request.0, samples.as_ptr(), samples.len(), rate, 1)
            })?;
        }
        for (key, value) in options {
            let (key, value) = (cstring(key)?, cstring(value)?);
            check(unsafe {
                ffi::audiocpp_request_set_option(request.0, key.as_ptr(), value.as_ptr())
            })?;
        }
        Ok(request)
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        unsafe { ffi::audiocpp_request_free(self.0) }
    }
}

/// A task session on a loaded model. All audio is mono.
pub struct Session {
    raw: *mut ffi::audiocpp_session,
    mode: Mode,
}

unsafe impl Send for Session {}

impl Session {
    /// `task` is a C API task token such as `asr`, `vad` or `wake`.
    pub fn new(
        model: &Model,
        task: &str,
        mode: Mode,
        backend: &Backend,
        options: &[(&str, &str)],
    ) -> Result<Self> {
        let task = cstring(task)?;
        let mode_token = cstring(mode.token())?;
        let backend_name = cstring(&backend.name)?;
        let backend_config = ffi::audiocpp_backend_config {
            backend: backend_name.as_ptr(),
            device: backend.device,
            threads: backend.threads,
        };
        let options = Options::new(options)?;
        let mut raw = ptr::null_mut();
        check(unsafe {
            ffi::audiocpp_session_create(
                model.0,
                task.as_ptr(),
                mode_token.as_ptr(),
                &backend_config,
                options.0,
                &mut raw,
            )
        })?;
        Ok(Session { raw, mode })
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Runs one offline request over `samples` at `sample_rate`.
    pub fn run(
        &mut self,
        samples: &[f32],
        sample_rate: i32,
        options: &[(&str, &str)],
    ) -> Result<TaskResult> {
        let request = Request::new(Some((samples, sample_rate)), options)?;
        let mut result = ptr::null_mut();
        check(unsafe { ffi::audiocpp_session_run(self.raw, request.0, &mut result) })?;
        let read = TaskResult::read(result);
        unsafe { ffi::audiocpp_result_free(result) };
        Ok(read)
    }

    /// Frames per push the family prefers, when it states one.
    pub fn preferred_chunk(&self) -> Option<usize> {
        let mut samples = 0i64;
        check(unsafe {
            ffi::audiocpp_stream_policy(
                self.raw,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut samples,
                ptr::null_mut(),
            )
        })
        .ok()?;
        (samples > 0).then_some(samples as usize)
    }

    /// Starts a stream. Some families validate the sample format while preparing, so the
    /// request carries a short silent buffer at `sample_rate`.
    pub fn stream_start(&mut self, sample_rate: i32, options: &[(&str, &str)]) -> Result<()> {
        let silence = vec![0.0f32; (sample_rate / 10).max(1) as usize];
        let request = Request::new(Some((&silence, sample_rate)), options)?;
        check(unsafe { ffi::audiocpp_stream_start(self.raw, request.0) })
    }

    /// Pushes one contiguous chunk; `start_sample` counts samples since `stream_start`.
    pub fn stream_push(
        &mut self,
        samples: &[f32],
        sample_rate: i32,
        start_sample: i64,
    ) -> Result<Option<Event>> {
        let mut event = ptr::null_mut();
        check(unsafe {
            ffi::audiocpp_stream_push(
                self.raw,
                samples.as_ptr(),
                samples.len(),
                sample_rate,
                1,
                start_sample,
                &mut event,
            )
        })?;
        Ok(Event::take(event))
    }

    /// Drains events the family queued on its own.
    pub fn stream_next_event(&mut self) -> Result<Option<Event>> {
        let mut event = ptr::null_mut();
        check(unsafe { ffi::audiocpp_stream_next_event(self.raw, &mut event) })?;
        Ok(Event::take(event))
    }

    pub fn stream_finish(&mut self) -> Result<TaskResult> {
        let mut result = ptr::null_mut();
        check(unsafe { ffi::audiocpp_stream_finish(self.raw, &mut result) })?;
        let read = TaskResult::read(result);
        unsafe { ffi::audiocpp_result_free(result) };
        Ok(read)
    }

    pub fn stream_reset(&mut self) -> Result<()> {
        check(unsafe { ffi::audiocpp_stream_reset(self.raw) })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { ffi::audiocpp_session_free(self.raw) }
    }
}
