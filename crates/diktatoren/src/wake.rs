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

/// When `transcript` starts with `phrase`, returns what was said after it.
pub fn match_phrase(transcript: &str, phrase: &str) -> Option<String> {
    let target: Vec<char> = letters(phrase).chars().collect();
    if target.is_empty() {
        return None;
    }
    let tolerance = (target.len() / 4).max(1);
    let words: Vec<&str> = transcript.split_whitespace().collect();
    let phrase_words = phrase.split_whitespace().count();
    let mut best: Option<(usize, usize)> = None;
    let mut heard = String::new();
    for (k, word) in words.iter().enumerate().take(phrase_words + 2) {
        heard.push_str(&letters(word));
        let candidate: Vec<char> = heard.chars().collect();
        let distance = levenshtein(&candidate, &target);
        if distance <= tolerance && best.is_none_or(|(_, d)| distance < d) {
            best = Some((k + 1, distance));
        }
        if candidate.len() > target.len() + tolerance {
            break;
        }
    }
    let (taken, _) = best?;
    let rest = words[taken..].join(" ");
    Some(rest.trim_start_matches([',', '.', '!', '?', ':', ';', ' ']).to_string())
}

#[cfg(test)]
mod tests {
    use super::match_phrase;

    #[test]
    fn matches_with_spelling_variation() {
        assert_eq!(match_phrase("Hey Diktatoren.", "Hey Diktatoren"), Some(String::new()));
        assert_eq!(match_phrase("hey dictatoren", "Hey Diktatoren"), Some(String::new()));
        assert_eq!(match_phrase("Hej, Diktatoren!", "Hej Diktatoren"), Some(String::new()));
        assert_eq!(match_phrase("Okay computer", "OK computer"), Some(String::new()));
    }

    #[test]
    fn returns_what_follows() {
        assert_eq!(
            match_phrase("Hey Diktatoren, send the report today.", "Hey Diktatoren"),
            Some("send the report today.".into())
        );
    }

    #[test]
    fn ignores_other_speech() {
        assert_eq!(match_phrase("Hey, how are you doing?", "Hey Diktatoren"), None);
        assert_eq!(match_phrase("The dictator spoke.", "Hey Diktatoren"), None);
        assert_eq!(match_phrase("", "Hey Diktatoren"), None);
        assert_eq!(match_phrase("anything", ""), None);
    }
}
