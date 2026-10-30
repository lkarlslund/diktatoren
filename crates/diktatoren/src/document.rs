//! The text being dictated: dictation lands at the cursor (replacing any selection) and spoken
//! commands edit it, with undo.

use std::ops::Range;

use crate::commands::{Command, Mode};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
}

/// What a command asks of the surrounding app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Edited,
    Insert,
    Copy,
    Cancel,
    StopListening,
    Resume,
    SwitchMode(Mode),
    /// The command had nothing to act on, such as deleting a word that is not there.
    NotFound,
}

#[derive(Debug, Default)]
pub struct Document {
    text: String,
    /// Byte range; empty means a plain cursor at `selection.start`.
    selection: Range<usize>,
    /// The most recent dictated piece, for "delete that".
    last_insert: Option<Range<usize>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl Document {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    /// Replaces the whole state after the user edited the text by hand.
    pub fn set_from_editor(&mut self, text: String, selection: Range<usize>) {
        if text != self.text {
            self.checkpoint();
            self.last_insert = None;
        }
        self.text = text;
        self.selection = clamp(&self.text, selection);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { text: self.text.clone(), selection: self.selection.clone() }
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.snapshot());
        self.redo.clear();
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.text = s.text;
        self.selection = s.selection;
        self.last_insert = None;
    }

    /// Inserts recognised speech at the cursor, fixing up spacing and capitalisation against
    /// the surrounding text.
    pub fn dictate(&mut self, spoken: &str) {
        let spoken = spoken.trim();
        if spoken.is_empty() {
            return;
        }
        self.checkpoint();
        let start = self.selection.start;
        let end = self.selection.end;
        let before = &self.text[..start];
        let after = &self.text[end..];

        let mut piece = String::new();
        let attaches = spoken.starts_with(|c: char| ",.;:!?)]}%".contains(c));
        if !before.is_empty() && !before.ends_with(char::is_whitespace) && !attaches {
            piece.push(' ');
        }
        let sentence_start = before.trim_end().is_empty()
            || before.trim_end().ends_with(['.', '!', '?', '\n'])
            || before.ends_with('\n');
        let mut body = if sentence_start { capitalize(spoken) } else { decapitalize(spoken) };
        // Mid-text insertion: drop the full stop the recogniser added if the text continues.
        if !after.trim().is_empty() && after.trim_start().starts_with(char::is_lowercase) {
            while body.ends_with('.') {
                body.pop();
            }
        }
        piece.push_str(&body);
        if !after.is_empty() && !after.starts_with(char::is_whitespace) && !after.starts_with(|c: char| ",.;:!?".contains(c)) {
            piece.push(' ');
        }
        self.text.replace_range(start..end, &piece);
        let inserted = start..start + piece.len();
        self.selection = inserted.end..inserted.end;
        self.last_insert = Some(inserted);
    }

    pub fn apply(&mut self, command: &Command) -> Outcome {
        use Command::*;
        match command {
            Insert => return Outcome::Insert,
            Copy => return Outcome::Copy,
            Cancel => return Outcome::Cancel,
            StopListening => return Outcome::StopListening,
            Resume => return Outcome::Resume,
            EditMode => return Outcome::SwitchMode(Mode::Edit),
            DictateMode => return Outcome::SwitchMode(Mode::Dictate),
            Undo => {
                let Some(prev) = self.undo.pop() else { return Outcome::NotFound };
                self.redo.push(self.snapshot());
                self.restore(prev);
            }
            Redo => {
                let Some(next) = self.redo.pop() else { return Outcome::NotFound };
                self.undo.push(self.snapshot());
                self.restore(next);
            }
            NextSentence => {
                let at = self.selection.start;
                let head = self.text[..at].trim_end();
                if head.is_empty() {
                    return Outcome::NotFound;
                }
                let end = head.len();
                let mark = if head.ends_with(['.', '!', '?', ':']) { "" } else { "." };
                self.checkpoint();
                self.text.replace_range(end..self.selection.end, mark);
                let cursor = end + mark.len();
                self.selection = cursor..cursor;
                self.last_insert = None;
            }
            NewLine => self.insert_raw("\n"),
            NewParagraph => self.insert_raw("\n\n"),
            Punctuation(mark) => {
                self.checkpoint();
                let at = self.selection.start;
                let trimmed = self.text[..at].trim_end().len();
                self.text.replace_range(trimmed..self.selection.end, mark);
                let end = trimmed + mark.len();
                self.selection = end..end;
                self.last_insert = None;
            }
            DeleteThat => {
                let range = if !self.selection.is_empty() {
                    self.selection.clone()
                } else if let Some(r) = self.last_insert.clone() {
                    r
                } else {
                    return Outcome::NotFound;
                };
                self.delete_range(range);
            }
            DeleteLastWord => {
                let end = self.selection.start;
                let head = self.text[..end].trim_end();
                let start = head
                    .char_indices()
                    .rev()
                    .find(|(_, c)| c.is_whitespace())
                    .map_or(0, |(i, c)| i + c.len_utf8());
                if start == end {
                    return Outcome::NotFound;
                }
                self.delete_range(start..end);
            }
            DeleteLastSentence => {
                let end = self.selection.start;
                let head = self.text[..end].trim_end();
                if head.is_empty() {
                    return Outcome::NotFound;
                }
                // Skip the terminator of the sentence being deleted.
                let body = head.trim_end_matches(['.', '!', '?']);
                let start = body
                    .char_indices()
                    .rev()
                    .find(|(_, c)| matches!(c, '.' | '!' | '?' | '\n'))
                    .map_or(0, |(i, c)| i + c.len_utf8());
                self.delete_range(start..end);
            }
            ClearAll => {
                if self.text.is_empty() {
                    return Outcome::NotFound;
                }
                self.checkpoint();
                self.text.clear();
                self.selection = 0..0;
                self.last_insert = None;
            }
            Delete(what) => {
                let Some(range) = find_last(&self.text, what) else { return Outcome::NotFound };
                self.delete_range(range);
            }
            Replace { from, to } => {
                let Some(range) = find_last(&self.text, from) else { return Outcome::NotFound };
                let original = &self.text[range.clone()];
                let replacement = if original.starts_with(char::is_uppercase) {
                    capitalize(to)
                } else {
                    to.clone()
                };
                self.checkpoint();
                self.splice_keep_cursor(range, &replacement);
                self.last_insert = None;
            }
            Select(what) => {
                let Some(range) = find_last(&self.text, what) else { return Outcome::NotFound };
                self.selection = range;
                return Outcome::Edited;
            }
            SelectAll => self.selection = 0..self.text.len(),
            GoToEnd => self.selection = self.text.len()..self.text.len(),
            GoToStart => self.selection = 0..0,
        }
        Outcome::Edited
    }

    /// Replaces `range` while keeping the cursor where it was relative to the surrounding text.
    fn splice_keep_cursor(&mut self, range: Range<usize>, replacement: &str) {
        let map = |p: usize| {
            if p <= range.start {
                p
            } else if p >= range.end {
                p - range.len() + replacement.len()
            } else {
                range.start + replacement.len()
            }
        };
        let selection = map(self.selection.start)..map(self.selection.end);
        self.text.replace_range(range, replacement);
        self.selection = selection;
    }

    fn insert_raw(&mut self, s: &str) {
        self.checkpoint();
        let start = self.selection.start;
        // A line break replaces trailing spaces before it.
        let trimmed = self.text[..start].trim_end_matches(' ').len();
        self.text.replace_range(trimmed..self.selection.end, s);
        let end = trimmed + s.len();
        self.selection = end..end;
        self.last_insert = None;
    }

    fn delete_range(&mut self, range: Range<usize>) {
        self.checkpoint();
        let mut start = range.start;
        let mut end = range.end;
        // Take one adjoining space with the deleted text so words do not run together or
        // leave double spaces.
        if self.text[end..].starts_with(' ') && (start == 0 || self.text[..start].ends_with(char::is_whitespace)) {
            end += 1;
        } else if self.text[..start].ends_with(' ') && (end == self.text.len() || !self.text[end..].starts_with(char::is_alphanumeric)) {
            start -= 1;
        }
        self.splice_keep_cursor(start..end, "");
        self.last_insert = None;
    }
}

fn clamp(text: &str, r: Range<usize>) -> Range<usize> {
    let fix = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let (a, b) = (fix(r.start), fix(r.end));
    a.min(b)..a.max(b)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Lowercases the first letter, except for "I" and all-caps acronyms. Names can't be told
/// apart from a capitalised sentence start, so they lose their capital here.
fn decapitalize(s: &str) -> String {
    let first_word = s.split_whitespace().next().unwrap_or("");
    let letters: Vec<char> = first_word.chars().take_while(|c| c.is_alphabetic()).collect();
    let is_i = letters == ['I'];
    let acronym = letters.len() > 1 && letters.iter().all(|c| c.is_uppercase());
    if is_i || acronym {
        return s.to_string();
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Last case-insensitive occurrence of `needle`, preferring whole-word matches.
pub fn find_last(haystack: &str, needle: &str) -> Option<Range<usize>> {
    let needle = needle.trim();
    if needle.is_empty() {
        return None;
    }
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let hay: Vec<(usize, char)> = haystack.char_indices().collect();
    let lower: Vec<char> = hay.iter().map(|&(_, c)| c.to_lowercase().next().unwrap_or(c)).collect();
    let byte_at = |i: usize| hay.get(i).map_or(haystack.len(), |&(b, _)| b);
    let mut partial = None;
    if needle.len() > lower.len() {
        return None;
    }
    for start in (0..=lower.len() - needle.len()).rev() {
        if lower[start..start + needle.len()] == needle[..] {
            let end = start + needle.len();
            let left_ok = start == 0 || !lower[start - 1].is_alphanumeric();
            let right_ok = end == lower.len() || !lower[end].is_alphanumeric();
            let range = byte_at(start)..byte_at(end);
            if left_ok && right_ok {
                return Some(range);
            }
            partial.get_or_insert(range);
        }
    }
    partial
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{Grammar, Language, Mode, preset};

    /// English and Danish phrases at once, since the tests mix them.
    fn both() -> Grammar {
        let danish = preset(Language::Danish);
        let mut phrases = preset(Language::English);
        for (key, value) in phrases.iter_mut() {
            *value = format!("{value}, {}", danish[key]);
        }
        Grammar::new(&phrases)
    }

    fn doc(parts: &[&str]) -> Document {
        let grammar = both();
        let mut d = Document::default();
        for p in parts {
            match grammar.parse(p, Mode::Edit) {
                Some(c) => {
                    d.apply(&c);
                }
                None => d.dictate(p),
            }
        }
        d
    }

    #[test]
    fn joins_utterances() {
        let d = doc(&["Hello there.", "How are you?"]);
        assert_eq!(d.text(), "Hello there. How are you?");
        let d = doc(&["Jeg tror", "Det bliver godt."]);
        assert_eq!(d.text(), "Jeg tror det bliver godt.");
    }

    #[test]
    fn keeps_i_and_acronyms() {
        assert_eq!(doc(&["So", "I think so."]).text(), "So I think so.");
        assert_eq!(doc(&["Ask", "NASA."]).text(), "Ask NASA.");
    }

    #[test]
    fn new_line_and_punctuation() {
        let d = doc(&["Kære Ole", "komma", "ny linje", "tak for sidst."]);
        assert_eq!(d.text(), "Kære Ole,\nTak for sidst.");
    }

    #[test]
    fn next_sentence() {
        let d = doc(&["Vi mødes på mandag", "næste sætning", "husk kaffe"]);
        assert_eq!(d.text(), "Vi mødes på mandag. Husk kaffe");
        let d = doc(&["Is it done?", "next sentence", "yes"]);
        assert_eq!(d.text(), "Is it done? Yes", "no second mark after a question");
        let d = doc(&["Vi mødes", "næste sætning", "næste sætning", "ja"]);
        assert_eq!(d.text(), "Vi mødes. Ja", "repeating it does nothing more");
    }

    #[test]
    fn delete_that_and_undo() {
        let mut d = doc(&["One.", "Two."]);
        d.apply(&Command::DeleteThat);
        assert_eq!(d.text(), "One.");
        d.apply(&Command::Undo);
        assert_eq!(d.text(), "One. Two.");
        d.apply(&Command::Redo);
        assert_eq!(d.text(), "One.");
    }

    #[test]
    fn delete_last_word_and_sentence() {
        let d = doc(&["Det er en rigtig god dag.", "Vi ses i morgen.", "slet sidste sætning"]);
        assert_eq!(d.text(), "Det er en rigtig god dag.");
        let d = doc(&["Det er en rigtig god dag", "slet sidste ord"]);
        assert_eq!(d.text(), "Det er en rigtig god");
    }

    #[test]
    fn replace_and_delete_words() {
        let d = doc(&["Vi mødes mandag klokken ti.", "Ret mandag til tirsdag."]);
        assert_eq!(d.text(), "Vi mødes tirsdag klokken ti.");
        let d = doc(&["Monday is fine.", "Replace monday with tuesday."]);
        assert_eq!(d.text(), "Tuesday is fine.");
        let d = doc(&["Det er meget meget godt.", "Slet meget."]);
        assert_eq!(d.text(), "Det er meget godt.");
    }

    #[test]
    fn edits_by_word_keep_the_cursor_at_the_end() {
        let d = doc(&["Meet me on Monday at ten.", "Replace Monday with Tuesday.", "Thanks."]);
        assert_eq!(d.text(), "Meet me on Tuesday at ten. Thanks.");
        let d = doc(&["Det er meget meget godt.", "Slet meget.", "Ja."]);
        assert_eq!(d.text(), "Det er meget godt. Ja.");
    }

    #[test]
    fn select_then_dictate_replaces() {
        let mut d = doc(&["Jeg kører i den røde bil i dag.", "Markér den røde bil."]);
        assert_eq!(&d.text()[d.selection()], "den røde bil");
        d.dictate("Det blå tog.");
        assert_eq!(d.text(), "Jeg kører i det blå tog i dag.");
    }

    #[test]
    fn whole_word_match_preferred() {
        assert_eq!(find_last("cat concatenate", "cat"), Some(0..3));
        assert_eq!(find_last("Æble og ÆBLE", "æble"), Some(9..14));
    }
}
