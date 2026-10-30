//! Matching a spoken wake phrase in a transcript.
//!
//! Recognisers spell unusual words inconsistently ("Diktatoren", "dictator en"), so the
//! comparison ignores spacing, case and punctuation and allows a few letters of difference.

/// The `wake_word_package` value that selects a spoken phrase instead of a wake-word model.
pub const PHRASE: &str = "phrase";

fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = prev[j] + usize::from(ca != cb);
            row.push(substitution.min(prev[j + 1] + 1).min(row[j] + 1));
        }
        prev = row;
    }
    prev[b.len()]
}

/// Recognises a wake phrase at the start of a transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Matcher {
    /// Accepted spellings, as lowercase letters only.
    variants: Vec<Vec<char>>,
    /// The most words a spelling spans, so a transcript is only searched that far.
    words: usize,
    tolerance: usize,
}

impl Matcher {
    /// Matches `phrase` and any `learned` spellings. Without a learned tolerance, a quarter of
    /// the phrase's letters may differ.
    pub fn new(phrase: &str, learned: &[String], tolerance: Option<usize>) -> Option<Self> {
        let mut variants: Vec<Vec<char>> = Vec::new();
        let mut words = 0;
        for spelling in std::iter::once(phrase).chain(learned.iter().map(String::as_str)) {
            let v: Vec<char> = letters(spelling).chars().collect();
            if !v.is_empty() && !variants.contains(&v) {
                words = words.max(spelling.split_whitespace().count());
                variants.push(v);
            }
        }
        let shortest = variants.iter().map(Vec::len).min()?;
        let tolerance = tolerance.unwrap_or((shortest / 4).max(1));
        Some(Matcher { variants, words, tolerance })
    }

    /// When `transcript` starts with the phrase, returns what was said after it.
    pub fn find(&self, transcript: &str) -> Option<String> {
        let words: Vec<&str> = transcript.split_whitespace().collect();
        let longest = self.variants.iter().map(Vec::len).max().unwrap_or(0);
        let mut best: Option<(usize, usize)> = None;
        let mut heard = String::new();
        for (k, word) in words.iter().enumerate().take(self.words + 2) {
            heard.push_str(&letters(word));
            let candidate: Vec<char> = heard.chars().collect();
            let distance = self.variants.iter().map(|v| levenshtein(&candidate, v)).min()?;
            if distance <= self.tolerance && best.is_none_or(|(_, d)| distance < d) {
                best = Some((k + 1, distance));
            }
            if candidate.len() > longest + self.tolerance {
                break;
            }
        }
        let (taken, _) = best?;
        let rest = words[taken..].join(" ");
        Some(rest.trim_start_matches([',', '.', '!', '?', ':', ';', ' ']).to_string())
    }
}

/// What training on repeated recordings of the wake phrase produced.
// Not wired to the settings yet; the training flow is still being decided.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    /// The first usable recording, as the recogniser spelled it.
    pub phrase: String,
    /// Every usable spelling heard.
    pub variants: Vec<String>,
    /// Letters of difference to accept.
    pub tolerance: usize,
}

/// Learns the phrase from transcripts of the user saying it several times.
///
/// A transcript far from all the others (a cough, a misheard take) is dropped. The tolerance
/// is one letter more than the largest gap between a take and its nearest neighbour, so it is
/// tight when the recogniser is consistent and looser when it is not.
#[allow(dead_code)]
pub fn calibrate(samples: &[String]) -> Option<Calibration> {
    let cleaned: Vec<String> = samples
        .iter()
        .map(|s| s.trim().trim_end_matches(['.', '!', '?', ',']).trim().to_string())
        .filter(|s| !letters(s).is_empty())
        .collect();
    let spelled: Vec<Vec<char>> = cleaned.iter().map(|s| letters(s).chars().collect()).collect();
    let nearest: Vec<usize> = (0..spelled.len())
        .map(|i| {
            (0..spelled.len())
                .filter(|&j| j != i)
                .map(|j| levenshtein(&spelled[i], &spelled[j]))
                .min()
                .unwrap_or(0)
        })
        .collect();
    let typical = {
        let mut lengths: Vec<usize> = spelled.iter().map(Vec::len).collect();
        lengths.sort_unstable();
        *lengths.get(lengths.len() / 2)?
    };
    let outlier = (typical / 3).max(2);
    let kept: Vec<usize> = (0..cleaned.len()).filter(|&i| nearest[i] <= outlier).collect();
    let first = *kept.first()?;
    let largest_gap = kept.iter().map(|&i| nearest[i]).max().unwrap_or(0);
    let mut variants: Vec<String> = Vec::new();
    for &i in &kept {
        if !variants.iter().any(|v| letters(v) == letters(&cleaned[i])) {
            variants.push(cleaned[i].clone());
        }
    }
    Some(Calibration {
        phrase: cleaned[first].clone(),
        variants,
        tolerance: (largest_gap + 1).clamp(1, (typical / 3).max(1)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(transcript: &str, phrase: &str) -> Option<String> {
        Matcher::new(phrase, &[], None)?.find(transcript)
    }

    #[test]
    fn matches_with_spelling_variation() {
        assert_eq!(find("Hey Diktatoren.", "Hey Diktatoren"), Some(String::new()));
        assert_eq!(find("hey dictatoren", "Hey Diktatoren"), Some(String::new()));
        assert_eq!(find("Hej, Diktatoren!", "Hej Diktatoren"), Some(String::new()));
        assert_eq!(find("Okay computer", "OK computer"), Some(String::new()));
    }

    #[test]
    fn returns_what_follows() {
        assert_eq!(
            find("Hey Diktatoren, send the report today.", "Hey Diktatoren"),
            Some("send the report today.".into())
        );
    }

    #[test]
    fn ignores_other_speech() {
        assert_eq!(find("Hey, how are you doing?", "Hey Diktatoren"), None);
        assert_eq!(find("The dictator spoke.", "Hey Diktatoren"), None);
        assert_eq!(find("", "Hey Diktatoren"), None);
        assert!(Matcher::new("", &[], None).is_none());
    }

    #[test]
    fn calibration_learns_spellings_and_tightens() {
        let takes = ["Hej Diktatoren.", "Hej diktatoren", "Hej, Diktatoren!", "Hej Diktatoren", "Hej diktatoren.", "Hej Diktatoren"]
            .map(String::from);
        let c = calibrate(&takes).unwrap();
        assert_eq!(c.phrase, "Hej Diktatoren");
        assert_eq!(c.tolerance, 1);
        let m = Matcher::new(&c.phrase, &c.variants, Some(c.tolerance)).unwrap();
        assert!(m.find("Hej diktatoren").is_some());
        assert!(m.find("Her er diktatoren").is_none(), "a consistent voice gets a tight match");
    }

    #[test]
    fn calibration_widens_for_variable_takes_and_drops_outliers() {
        let takes = ["Her i diktatoren", "Her er diktatoren", "Hej diktatoren", "Hedy Thessum", "Her er diktatoren", "Hej diktatoren"]
            .map(String::from);
        let c = calibrate(&takes).unwrap();
        assert!(!c.variants.iter().any(|v| v.contains("Thessum")), "outlier dropped: {c:?}");
        let m = Matcher::new(&c.phrase, &c.variants, Some(c.tolerance)).unwrap();
        for heard in ["Her i diktatoren", "Her er diktatoren", "Hej diktatoren", "Her diktatoren"] {
            assert!(m.find(heard).is_some(), "{heard} should wake ({c:?})");
        }
        assert!(m.find("Hvad siger du").is_none());
    }
}
