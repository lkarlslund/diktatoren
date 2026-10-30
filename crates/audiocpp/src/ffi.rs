//! Raw declarations for the subset of `include/audiocpp.h` Diktatoren uses.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int};

pub type audiocpp_status = c_int;
pub const AUDIOCPP_OK: audiocpp_status = 0;

pub const AUDIOCPP_ABI_VERSION_MAJOR: u32 = 0;

#[repr(C)]
pub struct audiocpp_registry {
    _private: [u8; 0],
}
#[repr(C)]
pub struct audiocpp_model {
    _private: [u8; 0],
}
#[repr(C)]
pub struct audiocpp_session {
    _private: [u8; 0],
}
#[repr(C)]
pub struct audiocpp_options {
    _private: [u8; 0],
}
#[repr(C)]
pub struct audiocpp_request {
    _private: [u8; 0],
}
#[repr(C)]
pub struct audiocpp_result {
    _private: [u8; 0],
}
#[repr(C)]
pub struct audiocpp_event {
    _private: [u8; 0],
}

#[repr(C)]
pub struct audiocpp_model_config {
    pub family_hint: *const c_char,
    pub config_id: *const c_char,
    pub weight_id: *const c_char,
    pub model_spec_override: *const c_char,
}

#[repr(C)]
pub struct audiocpp_backend_config {
    pub backend: *const c_char,
    pub device: c_int,
    pub threads: c_int,
}

pub type audiocpp_option_scope = c_int;
pub type audiocpp_stream_input_kind = c_int;
pub type audiocpp_stream_output_kind = c_int;
pub type audiocpp_voice_activity_kind = c_int;
pub const AUDIOCPP_VOICE_ACTIVITY_SPEECH_START: audiocpp_voice_activity_kind = 0;
pub const AUDIOCPP_VOICE_ACTIVITY_SPEECH_END: audiocpp_voice_activity_kind = 1;
pub const AUDIOCPP_VOICE_ACTIVITY_SPEECH_SEGMENT: audiocpp_voice_activity_kind = 2;

unsafe extern "C" {
    pub fn audiocpp_abi_version() -> u32;
    pub fn audiocpp_build_version() -> *const c_char;
    pub fn audiocpp_last_error() -> *const c_char;
    pub fn audiocpp_status_string(status: audiocpp_status) -> *const c_char;

    pub fn audiocpp_options_create() -> *mut audiocpp_options;
    pub fn audiocpp_options_set(
        options: *mut audiocpp_options,
        key: *const c_char,
        value: *const c_char,
    ) -> audiocpp_status;
    pub fn audiocpp_options_free(options: *mut audiocpp_options);

    pub fn audiocpp_registry_create(
        config_path: *const c_char,
        out_registry: *mut *mut audiocpp_registry,
    ) -> audiocpp_status;
    pub fn audiocpp_registry_free(registry: *mut audiocpp_registry);
    pub fn audiocpp_registry_family_count(registry: *const audiocpp_registry) -> usize;
    pub fn audiocpp_registry_family(
        registry: *const audiocpp_registry,
        index: usize,
        out_family: *mut *const c_char,
    ) -> audiocpp_status;

    pub fn audiocpp_model_load(
        registry: *mut audiocpp_registry,
        model_path: *const c_char,
        config: *const audiocpp_model_config,
        options: *const audiocpp_options,
        out_model: *mut *mut audiocpp_model,
    ) -> audiocpp_status;
    pub fn audiocpp_model_free(model: *mut audiocpp_model);
    pub fn audiocpp_model_family(model: *const audiocpp_model) -> *const c_char;
    pub fn audiocpp_model_supports(
        model: *const audiocpp_model,
        task: *const c_char,
        mode: *const c_char,
    ) -> c_int;
    pub fn audiocpp_model_language_count(model: *const audiocpp_model) -> usize;
    pub fn audiocpp_model_language(
        model: *const audiocpp_model,
        index: usize,
        out_language: *mut *const c_char,
    ) -> audiocpp_status;
    pub fn audiocpp_model_option_count(
        model: *const audiocpp_model,
        scope: audiocpp_option_scope,
    ) -> usize;
    pub fn audiocpp_model_option(
        model: *const audiocpp_model,
        scope: audiocpp_option_scope,
        index: usize,
        out_name: *mut *const c_char,
        out_value_name: *mut *const c_char,
        out_description: *mut *const c_char,
        out_default_value: *mut *const c_char,
        out_min_value: *mut *const c_char,
        out_max_value: *mut *const c_char,
        out_required: *mut c_int,
    ) -> audiocpp_status;

    pub fn audiocpp_session_create(
        model: *const audiocpp_model,
        task: *const c_char,
        mode: *const c_char,
        backend_config: *const audiocpp_backend_config,
        options: *const audiocpp_options,
        out_session: *mut *mut audiocpp_session,
    ) -> audiocpp_status;
    pub fn audiocpp_session_free(session: *mut audiocpp_session);
    pub fn audiocpp_session_run(
        session: *mut audiocpp_session,
        request: *const audiocpp_request,
        out_result: *mut *mut audiocpp_result,
    ) -> audiocpp_status;

    pub fn audiocpp_request_create() -> *mut audiocpp_request;
    pub fn audiocpp_request_free(request: *mut audiocpp_request);
    pub fn audiocpp_request_set_audio(
        request: *mut audiocpp_request,
        samples: *const f32,
        frames: usize,
        sample_rate: c_int,
        channels: c_int,
    ) -> audiocpp_status;
    pub fn audiocpp_request_set_option(
        request: *mut audiocpp_request,
        key: *const c_char,
        value: *const c_char,
    ) -> audiocpp_status;

    pub fn audiocpp_result_free(result: *mut audiocpp_result);
    pub fn audiocpp_result_text(
        result: *const audiocpp_result,
        out_text: *mut *const c_char,
        out_language: *mut *const c_char,
    ) -> audiocpp_status;
    pub fn audiocpp_result_segment_count(result: *const audiocpp_result) -> usize;
    pub fn audiocpp_result_segment(
        result: *const audiocpp_result,
        index: usize,
        out_start_sample: *mut i64,
        out_end_sample: *mut i64,
        out_confidence: *mut f32,
        out_text: *mut *const c_char,
    ) -> audiocpp_status;

    pub fn audiocpp_stream_policy(
        session: *const audiocpp_session,
        out_input: *mut audiocpp_stream_input_kind,
        out_output: *mut audiocpp_stream_output_kind,
        out_preferred_chunk_samples: *mut i64,
        out_preferred_chunk_seconds: *mut f64,
    ) -> audiocpp_status;
    pub fn audiocpp_stream_start(
        session: *mut audiocpp_session,
        request: *const audiocpp_request,
    ) -> audiocpp_status;
    pub fn audiocpp_stream_push(
        session: *mut audiocpp_session,
        samples: *const f32,
        frames: usize,
        sample_rate: c_int,
        channels: c_int,
        start_sample: i64,
        out_event: *mut *mut audiocpp_event,
    ) -> audiocpp_status;
    pub fn audiocpp_stream_next_event(
        session: *mut audiocpp_session,
        out_event: *mut *mut audiocpp_event,
    ) -> audiocpp_status;
    pub fn audiocpp_stream_finish(
        session: *mut audiocpp_session,
        out_result: *mut *mut audiocpp_result,
    ) -> audiocpp_status;
    pub fn audiocpp_stream_reset(session: *mut audiocpp_session) -> audiocpp_status;

    pub fn audiocpp_event_free(event: *mut audiocpp_event);
    pub fn audiocpp_event_is_final(event: *const audiocpp_event) -> c_int;
    pub fn audiocpp_event_as_result(event: *const audiocpp_event) -> *const audiocpp_result;
    pub fn audiocpp_event_voice_activity_count(event: *const audiocpp_event) -> usize;
    pub fn audiocpp_event_voice_activity(
        event: *const audiocpp_event,
        index: usize,
        out_kind: *mut audiocpp_voice_activity_kind,
        out_sample: *mut i64,
        out_probability: *mut f32,
    ) -> audiocpp_status;
}

