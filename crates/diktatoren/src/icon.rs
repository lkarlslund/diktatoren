//! The microphone icon, drawn in code so no image files are needed.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Listening,
}

/// Straight RGBA pixels, `size` × `size`.
pub fn rgba(size: u32, state: State) -> Vec<u8> {
    let background = match state {
        State::Idle => [0x2f, 0x6f, 0xeb],
        State::Listening => [0xe5, 0x3e, 0x3e],
    };
    let s = size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // Supersample 4×4 for smooth edges.
            let (mut bg, mut fg) = (0.0f32, 0.0f32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = (x as f32 + (sx as f32 + 0.5) / 4.0) / s;
                    let py = (y as f32 + (sy as f32 + 0.5) / 4.0) / s;
                    if in_circle(px, py) {
                        bg += 1.0;
                        if in_microphone(px, py) {
                            fg += 1.0;
                        }
                    }
                }
            }
            let coverage = bg / 16.0;
            let white = if bg > 0.0 { fg / bg } else { 0.0 };
            for c in background {
                out.push((c as f32 * (1.0 - white) + 255.0 * white).round() as u8);
            }
            out.push((coverage * 255.0).round() as u8);
        }
    }
    out
}

fn in_circle(x: f32, y: f32) -> bool {
    (x - 0.5).powi(2) + (y - 0.5).powi(2) <= 0.48f32.powi(2)
}

/// A capsule, a cradle arc below it and a stand, in unit coordinates.
fn in_microphone(x: f32, y: f32) -> bool {
    let capsule = {
        let (cx, top, bottom, r) = (0.5, 0.30, 0.50, 0.105);
        let cy = y.clamp(top, bottom);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    let cradle = {
        let (cx, cy) = (0.5, 0.48);
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        (0.16..=0.21).contains(&d) && y >= cy
    };
    let stem = (x - 0.5).abs() <= 0.025 && (0.68..=0.78).contains(&y);
    let base = (x - 0.5).abs() <= 0.11 && (0.76..=0.80).contains(&y);
    capsule || cradle || stem || base
}

/// ARGB32 in network byte order, as the StatusNotifierItem protocol wants.
#[cfg(target_os = "linux")]
pub fn argb(size: u32, state: State) -> Vec<u8> {
    rgba(size, state)
        .chunks(4)
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_round_opaque_icon() {
        let px = rgba(32, State::Idle);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0, "corner should be transparent");
        let center = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(px[center + 3], 255);
    }
}
