//! Microphone capture, delivered as 16 kHz mono f32 blocks.

use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use crate::resample::{Resampler, to_mono};

pub const SAMPLE_RATE: u32 = 16_000;

/// Names of the available input devices; the empty name means the system default.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| devices.map(|d| d.to_string()).collect())
        .unwrap_or_default()
}

/// A running capture. Dropping it closes the microphone.
pub struct Capture {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Capture {
    /// Opens `device` (or the default input when empty) and sends 16 kHz mono blocks to `sink`.
    pub fn start(device: &str, sink: mpsc::Sender<Vec<f32>>) -> Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        if let Some(path) = device.strip_prefix("file:") {
            return Ok(Self::from_file(path.into(), sink));
        }
        let device = device.to_string();
        // Streams are not Send on every platform, so the stream lives on its own thread.
        let thread = thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || match open(&device, sink) {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })?;
        ready_rx.recv().context("audio thread exited")??;
        Ok(Capture { stop: Some(stop_tx), thread: Some(thread) })
    }
}

impl Capture {
    /// Plays a WAV file through the pipeline in real time, as if spoken into the microphone,
    /// followed by silence. Selected with the input device name `file:<path>`.
    fn from_file(path: std::path::PathBuf, sink: mpsc::Sender<Vec<f32>>) -> Self {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let thread = thread::spawn(move || {
            let audio = match read_wav(&path) {
                Ok(a) => a,
                Err(e) => {
                    log::error!("{}: {e:#}", path.display());
                    return;
                }
            };
            let block = SAMPLE_RATE as usize / 50;
            let silence = std::iter::repeat_n(0.0f32, block);
            let blocks = audio.chunks(block).map(<[f32]>::to_vec).chain(std::iter::repeat_with(|| silence.clone().collect()));
            for b in blocks {
                if sink.send(b).is_err() || stop_rx.recv_timeout(std::time::Duration::from_millis(20)) != Err(mpsc::RecvTimeoutError::Timeout) {
                    return;
                }
            }
        });
        Capture { stop: Some(stop_tx), thread: Some(thread) }
    }
}

/// Reads a WAV file as 16 kHz mono.
pub fn read_wav(path: &std::path::Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>()?
        }
    };
    let mut mono = Vec::new();
    to_mono(&samples, spec.channels as usize, &mut mono);
    let mut out = Vec::new();
    Resampler::new(spec.sample_rate, SAMPLE_RATE).process(&mono, &mut out);
    out.extend(std::iter::repeat_n(0.0, 64));
    Ok(out)
}

impl Drop for Capture {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn open(name: &str, sink: mpsc::Sender<Vec<f32>>) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = if name.is_empty() {
        host.default_input_device().ok_or_else(|| anyhow!("no microphone found"))?
    } else {
        host.input_devices()?
            .find(|d| d.to_string() == name)
            .or_else(|| host.default_input_device())
            .ok_or_else(|| anyhow!("microphone {name:?} not found"))?
    };
    let config = device.default_input_config().context("reading microphone format")?;
    log::info!("capturing from {device} ({config:?})");
    let format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, stream_config, sink),
        SampleFormat::I16 => build::<i16>(&device, stream_config, sink),
        SampleFormat::I32 => build::<i32>(&device, stream_config, sink),
        SampleFormat::U16 => build::<u16>(&device, stream_config, sink),
        SampleFormat::I8 => build::<i8>(&device, stream_config, sink),
        other => Err(anyhow!("unsupported microphone sample format {other}")),
    }?;
    stream.play().context("starting microphone")?;
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sink: mpsc::Sender<Vec<f32>>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let mut resampler = Resampler::new(config.sample_rate, SAMPLE_RATE);
    let mut floats = Vec::new();
    let mut mono = Vec::new();
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &_| {
            floats.clear();
            floats.extend(data.iter().map(|&s| f32::from_sample(s)));
            mono.clear();
            to_mono(&floats, channels, &mut mono);
            let mut out = Vec::with_capacity(mono.len() / 2 + 16);
            resampler.process(&mono, &mut out);
            if !out.is_empty() {
                let _ = sink.send(out);
            }
        },
        |err| log::warn!("microphone: {err}"),
        None,
    )?;
    Ok(stream)
}
