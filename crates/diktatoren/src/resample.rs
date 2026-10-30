//! Streaming windowed-sinc resampler for turning microphone audio into 16 kHz mono.

use std::f64::consts::PI;

const HALF_TAPS: usize = 16;

pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    cutoff: f64,
    /// Position of the next output sample in `buf`.
    pos: f64,
    buf: Vec<f32>,
    passthrough: bool,
}

impl Resampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        let step = input_rate as f64 / output_rate as f64;
        Resampler {
            step,
            // Leave a little room below Nyquist when downsampling.
            cutoff: if step > 1.0 { 0.95 / step } else { 1.0 },
            pos: HALF_TAPS as f64,
            buf: vec![0.0; HALF_TAPS],
            passthrough: input_rate == output_rate,
        }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        while self.pos + (HALF_TAPS as f64) < self.buf.len() as f64 {
            out.push(self.sample_at(self.pos));
            self.pos += self.step;
        }
        let keep_from = (self.pos.floor() as usize).saturating_sub(HALF_TAPS);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
    }

    fn sample_at(&self, pos: f64) -> f32 {
        let center = pos.floor() as isize;
        let mut acc = 0.0f64;
        let mut norm = 0.0f64;
        for k in (center - HALF_TAPS as isize + 1)..=(center + HALF_TAPS as isize) {
            if k < 0 || k as usize >= self.buf.len() {
                continue;
            }
            let d = pos - k as f64;
            let w = window(d / HALF_TAPS as f64);
            let h = self.cutoff * sinc(self.cutoff * d) * w;
            acc += self.buf[k as usize] as f64 * h;
            norm += h;
        }
        if norm.abs() > 1e-9 { (acc / norm) as f32 } else { 0.0 }
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 { 1.0 } else { (PI * x).sin() / (PI * x) }
}

/// Hann window over [-1, 1].
fn window(x: f64) -> f64 {
    if x.abs() >= 1.0 { 0.0 } else { 0.5 * (1.0 + (PI * x).cos()) }
}

/// Averages interleaved frames down to mono.
pub fn to_mono(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(interleaved.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, freq: f64, seconds: f64) -> Vec<f32> {
        (0..(rate as f64 * seconds) as usize)
            .map(|i| (2.0 * PI * freq * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn keeps_length_and_passband_level() {
        let input = tone(48_000, 440.0, 1.0);
        let mut r = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        for chunk in input.chunks(441) {
            r.process(chunk, &mut out);
        }
        assert!((out.len() as i64 - 16_000).abs() < 40, "len {}", out.len());
        let level = rms(&out[200..out.len() - 200]);
        assert!((level - 0.7071).abs() < 0.02, "rms {level}");
    }

    #[test]
    fn suppresses_content_above_nyquist() {
        let input = tone(48_000, 12_000.0, 0.5);
        let mut r = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert!(rms(&out[200..]) < 0.05, "aliased rms {}", rms(&out[200..]));
    }

    #[test]
    fn upsamples() {
        let input = tone(8_000, 300.0, 1.0);
        let mut r = Resampler::new(8_000, 16_000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert!((out.len() as i64 - 16_000).abs() < 40);
        assert!((rms(&out[200..out.len() - 200]) - 0.7071).abs() < 0.02);
    }
}
