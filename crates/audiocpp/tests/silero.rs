//! Exercises the bindings against Silero VAD, which ships inside the audio.cpp tree.

use std::path::PathBuf;

use audiocpp::{Backend, Mode, Model, Registry, Session, VoiceActivityKind};

fn source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/audio.cpp")
}

fn sample() -> (Vec<f32>, u32) {
    let mut reader = hound::WavReader::open(source_dir().join("assets/resources/sample_16k.wav"))
        .expect("sample_16k.wav");
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(Result::unwrap).collect(),
        hound::SampleFormat::Int => reader
            .samples::<i32>()
            .map(|s| s.unwrap() as f32 / (1u32 << (spec.bits_per_sample - 1)) as f32)
            .collect(),
    };
    let mono = samples
        .chunks(spec.channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    (mono, spec.sample_rate)
}

fn cpu() -> Backend {
    Backend { name: "cpu".into(), device: 0, threads: 2 }
}

fn silero() -> Model {
    let registry = Registry::new().expect("registry");
    assert!(registry.families().iter().any(|f| f == "silero_vad"));
    let path = source_dir().join("assets/framework/models/silero_vad");
    Model::load(&registry, &path, Some("silero_vad")).expect("load silero")
}

#[test]
fn offline_vad_finds_speech() {
    let model = silero();
    assert!(model.supports("vad", "offline"));
    let (audio, rate) = sample();
    let mut session = Session::new(&model, "vad", Mode::Offline, &cpu(), &[]).expect("session");
    let result = session.run(&audio, rate as i32, &[]).expect("run");
    assert!(!result.segments.is_empty(), "no speech segments in the sample");
}

#[test]
fn streaming_vad_reports_activity() {
    let model = silero();
    let (audio, rate) = sample();
    let mut session = Session::new(&model, "vad", Mode::Streaming, &cpu(), &[]).expect("session");
    let chunk = session.preferred_chunk().unwrap_or(512);
    session.stream_start(rate as i32, &[]).expect("start");
    let mut starts = 0;
    for (i, block) in audio.chunks(chunk).enumerate() {
        let mut block = block.to_vec();
        block.resize(chunk, 0.0);
        let event = session.stream_push(&block, rate as i32, (i * chunk) as i64).expect("push");
        for e in event.into_iter().chain(std::iter::from_fn(|| session.stream_next_event().unwrap())) {
            starts += e
                .voice_activity
                .iter()
                .filter(|a| a.kind == VoiceActivityKind::SpeechStart)
                .count();
        }
    }
    session.stream_finish().expect("finish");
    assert!(starts > 0, "streaming VAD never reported speech start");
}
