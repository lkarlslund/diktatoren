//! Recognises spoken commands.
//!
//! A command must make up the whole utterance, so dictation that merely contains a command word
//! is left alone. Recognisers spell lone words inconsistently ("indsæt", "Indsæt!", "inset"),
//! so fixed phrases match with a few letters of tolerance.
//!
//! The phrases come in English and Danish presets and can be edited, which makes them
//! "custom". In [`Mode::Dictate`] only the control commands apply (finishing, line breaks,
//! punctuation, undo, switching mode); in [`Mode::Edit`] every command does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::wake::levenshtein;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Speech is text, apart from control commands said on their own.
    Dictate,
    /// Speech is editing commands.
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Insert,
    Copy,
    Cancel,
    StopListening,
    EditMode,
    DictateMode,
    /// Starts item `n` of a numbered list on a new line.
    ListItem(usize),
    /// Ends the sentence with a full stop if it has no end mark; the next words start a new one.
    NextSentence,
    NewLine,
    NewParagraph,
    Undo,
    Redo,
    /// Deletes the selection, or else the last dictated piece.
    DeleteThat,
    /// Deletes this many words before the cursor.
    DeleteLastWords(usize),
    /// Deletes this many sentences before the cursor.
    DeleteLastSentences(usize),
    ClearAll,
    Delete(String),
    Replace { from: String, to: String },
    Select(String),
    SelectAll,
    /// Moves the cursor to the end of the text.
    GoToEnd,
    GoToStart,
    Punctuation(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    English,
    Danish,
    /// Phrases the user edited.
    Custom,
}

impl Language {
    /// Danish when the system locale is Danish, otherwise English.
    pub fn from_locale() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
            .unwrap_or_default();
        if locale.starts_with("da") { Language::Danish } else { Language::English }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Language::English => "English",
            Language::Danish => "Dansk",
            Language::Custom => crate::i18n::tr("Custom"),
        })
    }
}

#[derive(Debug, Clone)]
enum Slot {
    Fixed(Command),
    ReplaceVerb,
    ReplaceSeparator,
    DeleteVerb,
    SelectVerb,
}

/// One editable phrase list.
pub struct Spec {
    pub key: &'static str,
    pub label: &'static str,
    /// Applies while dictating, not only while editing.
    pub control: bool,
    slot: Slot,
    english: &'static str,
    danish: &'static str,
}

pub fn specs() -> Vec<Spec> {
    use Command::*;
    let fixed = |key, label, control, command, english, danish| Spec {
        key,
        label,
        control,
        slot: Slot::Fixed(command),
        english,
        danish,
    };
    vec![
        fixed("insert", "Insert", true, Insert, "insert, insert it, paste, paste it", "indsæt, sæt ind, indsæt teksten"),
        fixed("copy", "Copy", true, Copy, "copy, copy it, copy that", "kopiér, kopier, kopiér det, kopier teksten"),
        fixed("cancel", "Cancel", true, Cancel, "cancel, cancel that, never mind", "annuller, annullér, afbryd, glem det"),
        fixed("pause", "Pause (only dictate and edit are heard)", true, StopListening, "pause, stop listening, microphone off", "pause, stop med at lytte, hold pause, mikrofon fra"),
        fixed("edit_mode", "Edit mode (also leaves a pause)", true, EditMode, "edit, edit mode, editing", "redigér, rediger, redigering"),
        fixed("dictate_mode", "Dictate mode (also leaves a pause)", true, DictateMode, "dictate, dictation, dictate mode", "diktér, dikter, diktat"),
        fixed("next_sentence", "Next sentence", true, NextSentence, "next sentence, new sentence", "næste sætning, ny sætning"),
        fixed("new_line", "New line", true, NewLine, "new line, line break, next line", "ny linje, linjeskift, næste linje"),
        fixed("new_paragraph", "New paragraph", true, NewParagraph, "new paragraph, next paragraph", "nyt afsnit, næste afsnit"),
        fixed("period", "Full stop", true, Punctuation("."), "period, full stop", "punktum"),
        fixed("comma", "Comma", true, Punctuation(","), "comma", "komma"),
        fixed("question_mark", "Question mark", true, Punctuation("?"), "question mark", "spørgsmålstegn"),
        fixed("exclamation_mark", "Exclamation mark", true, Punctuation("!"), "exclamation mark, exclamation point", "udråbstegn"),
        fixed("colon", "Colon", true, Punctuation(":"), "colon", "kolon"),
        fixed("semicolon", "Semicolon", true, Punctuation(";"), "semicolon", "semikolon"),
        fixed("dash", "Dash", true, Punctuation(" –"), "dash", "tankestreg"),
        fixed("undo", "Undo", true, Undo, "undo, undo that", "fortryd, fortryd det"),
        fixed("delete_that", "Delete the selection or last piece", true, DeleteThat, "delete that, scratch that, strike that", "slet det, slet det der, fjern det"),
        fixed("redo", "Redo", true, Redo, "redo, redo that", "gentag, annuller fortryd"),
        fixed("delete_last_word", "Delete the last word", false, DeleteLastWords(1), "delete last word, delete the last word", "slet sidste ord, fjern sidste ord"),
        fixed("delete_last_sentence", "Delete the last sentence", false, DeleteLastSentences(1), "delete last sentence, delete the last sentence", "slet sidste sætning, fjern sidste sætning"),
        fixed("clear_all", "Delete everything", false, ClearAll, "clear all, delete all, delete everything", "slet alt, ryd alt"),
        fixed("select_all", "Select everything", false, SelectAll, "select all, select everything", "markér alt, marker alt, vælg alt"),
        fixed("go_to_end", "Cursor to the end", false, GoToEnd, "go to end, go to the end", "gå til slutningen, til slutningen"),
        fixed("go_to_start", "Cursor to the start", false, GoToStart, "go to start, go to the start, go to the beginning", "gå til starten, gå til begyndelsen"),
        Spec { key: "replace", label: "Replace … (verb)", control: false, slot: Slot::ReplaceVerb, english: "replace, change", danish: "erstat, ret, skift, udskift" },
        Spec { key: "replace_with", label: "… with … (separator)", control: false, slot: Slot::ReplaceSeparator, english: "with, to", danish: "med, til, ud med" },
        Spec { key: "delete", label: "Delete … (verb)", control: false, slot: Slot::DeleteVerb, english: "delete, remove", danish: "slet, fjern" },
        Spec { key: "select", label: "Select … (verb)", control: false, slot: Slot::SelectVerb, english: "select", danish: "markér, marker, vælg" },
    ]
}

/// The phrases of a preset language, keyed by [`Spec::key`].
impl Grammar {
    /// The first phrase that triggers `key`, for showing to the user.
    pub fn first_phrase(&self, command: &Command) -> Option<&str> {
        self.fixed.iter().find(|(_, c, _)| c == command)?.0.first().map(String::as_str)
    }
}

pub fn preset(language: Language) -> BTreeMap<String, String> {
    specs()
        .into_iter()
        .map(|s| {
            let phrases = match language {
                Language::Danish => s.danish,
                Language::English | Language::Custom => s.english,
            };
            (s.key.to_string(), phrases.to_string())
        })
        .collect()
}

fn split_list(phrases: &str) -> Vec<String> {
    phrases.split(',').map(normalize).filter(|p| !p.is_empty()).collect()
}

/// Commands compiled from a phrase table.
pub struct Grammar {
    fixed: Vec<(Vec<String>, Command, bool)>,
    replace: Vec<String>,
    separators: Vec<String>,
    delete: Vec<String>,
    select: Vec<String>,
}

impl Grammar {
    pub fn new(phrases: &BTreeMap<String, String>) -> Self {
        let mut g = Grammar { fixed: Vec::new(), replace: Vec::new(), separators: Vec::new(), delete: Vec::new(), select: Vec::new() };
        for spec in specs() {
            let list = split_list(phrases.get(spec.key).map_or("", String::as_str));
            match spec.slot {
                Slot::Fixed(command) => g.fixed.push((list, command, spec.control)),
                Slot::ReplaceVerb => g.replace = list,
                Slot::ReplaceSeparator => g.separators = list,
                Slot::DeleteVerb => g.delete = list,
                Slot::SelectVerb => g.select = list,
            }
        }
        g
    }

    pub fn parse(&self, utterance: &str, mode: Mode) -> Option<Command> {
        let body = normalize(utterance);
        if body.is_empty() {
            return None;
        }
        if let Some(command) = self.fixed(&body, mode) {
            return Some(command);
        }
        if let Some(n) = list_number(&body) {
            return Some(Command::ListItem(n));
        }
        if mode == Mode::Dictate {
            return None;
        }
        self.argument(utterance.trim().trim_end_matches(['.', '!', '?']))
    }

    /// The fixed-phrase command `body` is, exactly or within a few letters.
    fn fixed(&self, body: &str, mode: Mode) -> Option<Command> {
        let heard = letters(body);
        let mut best: Option<(usize, &Command)> = None;
        for (list, command, control) in &self.fixed {
            if mode == Mode::Dictate && !control {
                continue;
            }
            for phrase in list {
                if phrase == body {
                    return Some(command.clone());
                }
                let target = letters(phrase);
                let distance = levenshtein(&heard, &target);
                if distance <= tolerance(target.len()) && best.is_none_or(|(d, _)| distance < d) {
                    best = Some((distance, command));
                }
            }
        }
        best.map(|(_, c)| c.clone())
    }

    /// Replace, delete and select. The argument keeps the speaker's casing.
    fn argument(&self, original: &str) -> Option<Command> {
        let words: Vec<&str> = original.split_whitespace().collect();
        let lower: Vec<String> = words.iter().map(|w| normalize(w)).collect();
        let verb_len = |verbs: &[String]| {
            verbs.iter().find_map(|verb| {
                let parts: Vec<&str> = verb.split(' ').collect();
                (lower.len() > parts.len() && lower.iter().zip(&parts).all(|(w, p)| w == p)).then_some(parts.len())
            })
        };
        if let Some(n) = verb_len(&self.replace) {
            for sep in &self.separators {
                if let Some((from, to)) = split_on_word(&words[n..], sep)
                    && !from.is_empty()
                    && !to.is_empty()
                {
                    return Some(Command::Replace { from, to });
                }
            }
        }
        if let Some(n) = verb_len(&self.delete) {
            if let Some(command) = counted_delete(&lower[n..]) {
                return Some(command);
            }
            return Some(Command::Delete(clean_argument(&words[n..].join(" "))));
        }
        if let Some(n) = verb_len(&self.select) {
            return Some(Command::Select(clean_argument(&words[n..].join(" "))));
        }
        None
    }
}

/// "(the) last (two) sentences" after a delete verb, in English or Danish.
fn counted_delete(words: &[String]) -> Option<Command> {
    let mut rest = words.iter().map(String::as_str).skip_while(|w| ["the", "de", "den", "det"].contains(w));
    if !matches!(rest.next()?, "last" | "sidste") {
        return None;
    }
    let mut word = rest.next()?;
    let count = match number(word) {
        Some(n) => {
            word = rest.next()?;
            n
        }
        None => 1,
    };
    if rest.next().is_some() {
        return None;
    }
    match word {
        "word" | "words" | "ord" | "ordene" => Some(Command::DeleteLastWords(count)),
        "sentence" | "sentences" | "sætning" | "sætninger" | "sætningen" | "sætningerne" => {
            Some(Command::DeleteLastSentences(count))
        }
        _ => None,
    }
}

/// A list number said on its own: "2", "to", "punkt to", "number two".
fn list_number(body: &str) -> Option<usize> {
    let words: Vec<&str> = body.split(' ').collect();
    match words.as_slice() {
        [n] => number(n),
        ["punkt" | "nummer" | "point" | "number" | "item", n] => number(n),
        _ => None,
    }
}

/// Splits dictation that starts with a list number and a capitalised word, as recognisers
/// write "1 Vi kan ikke se horisonten", into the number and the rest.
pub fn leading_list_number(spoken: &str) -> Option<(usize, &str)> {
    let (first, rest) = spoken.trim().split_once(' ')?;
    let n = first.trim_end_matches(['.', ')', ':']).parse::<usize>().ok().filter(|n| (1..=50).contains(n))?;
    rest.trim_start().starts_with(char::is_uppercase).then(|| (n, rest.trim_start()))
}

/// A small count, as digits or an English or Danish number word.
fn number(word: &str) -> Option<usize> {
    if let Ok(n) = word.parse::<usize>() {
        return (1..=100).contains(&n).then_some(n);
    }
    const WORDS: [(&[&str], usize); 10] = [
        (&["one", "en", "et"], 1),
        (&["two", "to"], 2),
        (&["three", "tre"], 3),
        (&["four", "fire"], 4),
        (&["five", "fem"], 5),
        (&["six", "seks"], 6),
        (&["seven", "syv"], 7),
        (&["eight", "otte"], 8),
        (&["nine", "ni"], 9),
        (&["ten", "ti"], 10),
    ];
    WORDS.iter().find(|(names, _)| names.contains(&word)).map(|(_, n)| *n)
}

/// Lowercases, trims the punctuation recognisers add, and collapses whitespace.
pub fn normalize(utterance: &str) -> String {
    let lower = utterance.to_lowercase();
    let words: Vec<&str> = lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\''))
        .filter(|w| !w.is_empty())
        .collect();
    words.join(" ")
}

fn letters(s: &str) -> Vec<char> {
    s.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Letters of difference tolerated between a phrase and what was heard.
fn tolerance(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// Splits `words` at the last occurrence of `sep` (which may be several words).
fn split_on_word(words: &[&str], sep: &str) -> Option<(String, String)> {
    let sep: Vec<&str> = sep.split(' ').collect();
    let lower: Vec<String> = words.iter().map(|w| normalize(w)).collect();
    (1..words.len().saturating_sub(sep.len()) + 1)
        .rev()
        .find(|&i| i + sep.len() <= words.len() && lower[i..i + sep.len()].iter().zip(&sep).all(|(w, s)| w == s))
        .map(|i| {
            (
                clean_argument(&words[..i].join(" ")),
                clean_argument(&words[i + sep.len()..].join(" ")),
            )
        })
}

fn clean_argument(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == '.' || c == '“' || c == '”')
        .trim()
        .to_string()
}

#[cfg(test)]
pub fn grammar(language: Language) -> Grammar {
    Grammar::new(&preset(language))
}

#[cfg(test)]
mod tests {
    use super::Command::*;
    use super::Mode::{Dictate, Edit};
    use super::*;

    fn en(s: &str, mode: Mode) -> Option<Command> {
        grammar(Language::English).parse(s, mode)
    }

    fn da(s: &str, mode: Mode) -> Option<Command> {
        grammar(Language::Danish).parse(s, mode)
    }

    #[test]
    fn control_commands_while_dictating() {
        assert_eq!(en("Insert.", Dictate), Some(Insert));
        assert_eq!(da("Indsæt!", Dictate), Some(Insert));
        assert_eq!(da("Kopiér det.", Dictate), Some(Copy));
        assert_eq!(da("Ny linje.", Dictate), Some(NewLine));
        assert_eq!(da("Punktum.", Dictate), Some(Punctuation(".")));
        assert_eq!(en("Scratch that.", Dictate), Some(DeleteThat));
        assert_eq!(da("Redigér.", Dictate), Some(EditMode));
        assert_eq!(da("Næste sætning.", Dictate), Some(NextSentence));
        assert_eq!(en("Next sentence", Dictate), Some(NextSentence));
        assert_eq!(da("To.", Dictate), Some(ListItem(2)));
        assert_eq!(da("2", Dictate), Some(ListItem(2)));
        assert_eq!(da("Punkt tre", Dictate), Some(ListItem(3)));
        assert_eq!(leading_list_number("1 Vi kan ikke se horisonten"), Some((1, "Vi kan ikke se horisonten")));
        assert_eq!(leading_list_number("2 personer kommer"), None);
    }

    #[test]
    fn only_the_chosen_language_counts() {
        assert_eq!(en("Indsæt.", Dictate), None);
        assert_eq!(da("Insert.", Dictate), None);
    }

    #[test]
    fn lone_words_match_loosely() {
        assert_eq!(en("Inset.", Dictate), Some(Insert));
        assert_eq!(en("Kancel.", Dictate), Some(Cancel));
        assert_eq!(da("Indsæt", Dictate), Some(Insert));
        assert_eq!(da("Annuler", Dictate), Some(Cancel));
        assert_eq!(da("Kopier", Dictate), Some(Copy));
    }

    #[test]
    fn dictation_is_left_alone() {
        assert_eq!(en("Please insert the coin.", Dictate), None);
        assert_eq!(da("Jeg skal kopiere filen i morgen.", Dictate), None);
        assert_eq!(da("Det var en god pause.", Dictate), None);
        assert_eq!(da("Hej.", Dictate), None);
        assert_eq!(da("Slet meget.", Dictate), None, "editing needs edit mode");
    }

    #[test]
    fn editing_commands() {
        assert_eq!(en("Replace Monday with Tuesday.", Edit), Some(Replace { from: "Monday".into(), to: "Tuesday".into() }));
        assert_eq!(da("Erstat hund med kat.", Edit), Some(Replace { from: "hund".into(), to: "kat".into() }));
        assert_eq!(da("Ret mandag til tirsdag.", Edit), Some(Replace { from: "mandag".into(), to: "tirsdag".into() }));
        assert_eq!(da("Slet meget.", Edit), Some(Delete("meget".into())));
        assert_eq!(da("Markér den røde bil.", Edit), Some(Select("den røde bil".into())));
        assert_eq!(da("Slet sidste sætning.", Edit), Some(DeleteLastSentences(1)));
        assert_eq!(da("Slet de sidste to sætninger", Edit), Some(DeleteLastSentences(2)));
        assert_eq!(da("Slet de sidste 3 ord.", Edit), Some(DeleteLastWords(3)));
        assert_eq!(en("Delete the last two sentences.", Edit), Some(DeleteLastSentences(2)));
        assert_eq!(en("Delete last word", Edit), Some(DeleteLastWords(1)));
        assert_eq!(da("Slet de sidste kager", Edit), Some(Delete("de sidste kager".into())));
        assert_eq!(da("Diktér.", Edit), Some(DictateMode));
        assert_eq!(en("Something else entirely", Edit), None);
    }

    #[test]
    fn custom_phrases() {
        let mut phrases = preset(Language::Danish);
        phrases.insert("insert".into(), "send den, afsted".into());
        let g = Grammar::new(&phrases);
        assert_eq!(g.parse("Afsted!", Dictate), Some(Insert));
        assert_eq!(g.parse("Send den.", Dictate), Some(Insert));
        assert_eq!(g.parse("Indsæt.", Dictate), None);
    }
}
