//! Recognises spoken edit commands in English and Danish.
//!
//! A command must make up the whole utterance, so ordinary dictation that merely contains a
//! command word is left alone. Saying "command …" / "kommando …" first forces command
//! interpretation, and is required when `require_prefix` is set.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Insert,
    Copy,
    Cancel,
    StopListening,
    NewLine,
    NewParagraph,
    Undo,
    Redo,
    /// Deletes the selection, or else the last dictated piece.
    DeleteThat,
    DeleteLastWord,
    DeleteLastSentence,
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

const PREFIXES: [&str; 3] = ["command", "kommando", "computer"];

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

pub fn parse(utterance: &str, require_prefix: bool) -> Option<Command> {
    let normalized = normalize(utterance);
    let (prefixed, body) = match PREFIXES
        .iter()
        .find_map(|p| normalized.strip_prefix(p).filter(|r| r.is_empty() || r.starts_with(' ')))
    {
        Some(rest) => (true, rest.trim().to_string()),
        None => (false, normalized),
    };
    if require_prefix && !prefixed {
        return None;
    }
    parse_body(&body, original_tail(utterance, prefixed))
}

/// The utterance with its prefix word removed but casing kept, so replacement text keeps the
/// speaker's capitalisation.
fn original_tail(utterance: &str, prefixed: bool) -> String {
    let trimmed = utterance.trim().trim_end_matches(['.', '!', '?']);
    if !prefixed {
        return trimmed.to_string();
    }
    trimmed
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest.trim_start_matches([',', ':']).trim().to_string())
        .unwrap_or_default()
}

fn parse_body(body: &str, original: String) -> Option<Command> {
    use Command::*;
    let exact = |phrases: &[&str]| phrases.contains(&body);
    if exact(&["insert", "insert it", "insert text", "paste", "paste it", "indsæt", "sæt ind", "indsæt tekst", "indsæt teksten"]) {
        return Some(Insert);
    }
    if exact(&["copy", "copy it", "copy that", "copy text", "kopier", "kopiér", "kopier det", "kopiér det", "kopier teksten", "kopiér teksten"]) {
        return Some(Copy);
    }
    if exact(&["cancel", "cancel that", "never mind", "nevermind", "annuller", "annullér", "afbryd", "glem det"]) {
        return Some(Cancel);
    }
    if exact(&["stop listening", "pause", "stop med at lytte", "hold pause"]) {
        return Some(StopListening);
    }
    if exact(&["new line", "newline", "line break", "next line", "ny linje", "linjeskift", "næste linje"]) {
        return Some(NewLine);
    }
    if exact(&["new paragraph", "next paragraph", "nyt afsnit", "næste afsnit"]) {
        return Some(NewParagraph);
    }
    if exact(&["undo", "undo that", "fortryd", "fortryd det"]) {
        return Some(Undo);
    }
    if exact(&["redo", "redo that", "gentag", "annuller fortryd", "annullér fortryd"]) {
        return Some(Redo);
    }
    if exact(&["delete that", "scratch that", "strike that", "remove that", "slet det", "slet det der", "fjern det", "slet det sidste"]) {
        return Some(DeleteThat);
    }
    if exact(&["delete last word", "delete the last word", "slet sidste ord", "slet det sidste ord", "fjern sidste ord"]) {
        return Some(DeleteLastWord);
    }
    if exact(&["delete last sentence", "delete the last sentence", "slet sidste sætning", "slet den sidste sætning", "fjern sidste sætning"]) {
        return Some(DeleteLastSentence);
    }
    if exact(&["clear all", "delete all", "delete everything", "clear everything", "slet alt", "ryd alt", "fjern alt", "slet alting"]) {
        return Some(ClearAll);
    }
    if exact(&["select all", "select everything", "marker alt", "markér alt", "vælg alt"]) {
        return Some(SelectAll);
    }
    if exact(&["go to end", "go to the end", "end of text", "gå til slutningen", "gå til enden", "til slutningen"]) {
        return Some(GoToEnd);
    }
    if exact(&["go to start", "go to the start", "go to beginning", "go to the beginning", "gå til starten", "gå til begyndelsen", "til starten"]) {
        return Some(GoToStart);
    }
    let punctuation: &[(&[&str], &'static str)] = &[
        (&["period", "full stop", "punktum"], "."),
        (&["comma", "komma"], ","),
        (&["question mark", "spørgsmålstegn"], "?"),
        (&["exclamation mark", "exclamation point", "udråbstegn"], "!"),
        (&["colon", "kolon"], ":"),
        (&["semicolon", "semikolon"], ";"),
        (&["dash", "tankestreg"], " –"),
    ];
    for (phrases, mark) in punctuation {
        if phrases.contains(&body) {
            return Some(Punctuation(mark));
        }
    }

    // Commands with an argument. The argument comes from the original utterance so that
    // replacement text keeps its casing.
    let words: Vec<&str> = original.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|w| normalize(w)).collect();
    let starts = |phrase: &[&str]| {
        lower.len() > phrase.len() && lower.iter().zip(phrase).all(|(w, p)| w == p)
    };
    let rest = |n: usize| clean_argument(&words[n..].join(" "));

    for verb in [&["replace"][..], &["change"], &["erstat"], &["ret"], &["skift"], &["udskift"]] {
        if starts(verb) {
            for sep in ["with", "to", "med", "til", "ud med"] {
                if let Some((from, to)) = split_on_word(&words[verb.len()..], sep)
                    && !from.is_empty() && !to.is_empty() {
                        return Some(Replace { from, to });
                    }
            }
        }
    }
    for verb in [&["delete"][..], &["remove"], &["slet"], &["fjern"]] {
        if starts(verb) {
            return Some(Delete(rest(verb.len())));
        }
    }
    for verb in [&["select"][..], &["marker"], &["markér"], &["vælg"]] {
        if starts(verb) {
            return Some(Select(rest(verb.len())));
        }
    }
    None
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
mod tests {
    use super::Command::*;
    use super::*;

    #[test]
    fn whole_utterance_commands() {
        assert_eq!(parse("Insert.", false), Some(Insert));
        assert_eq!(parse("Indsæt!", false), Some(Insert));
        assert_eq!(parse("Kopiér det.", false), Some(Copy));
        assert_eq!(parse("Ny linje.", false), Some(NewLine));
        assert_eq!(parse("New paragraph", false), Some(NewParagraph));
        assert_eq!(parse("Slet sidste sætning.", false), Some(DeleteLastSentence));
        assert_eq!(parse("Punktum.", false), Some(Punctuation(".")));
        assert_eq!(parse("Scratch that.", false), Some(DeleteThat));
    }

    #[test]
    fn ordinary_dictation_is_not_a_command() {
        assert_eq!(parse("Please insert the coin.", false), None);
        assert_eq!(parse("Jeg skal kopiere filen i morgen.", false), None);
        assert_eq!(parse("Det var en god pause.", false), None);
    }

    #[test]
    fn arguments_keep_casing() {
        assert_eq!(
            parse("Replace Monday with Tuesday.", false),
            Some(Replace { from: "Monday".into(), to: "Tuesday".into() })
        );
        assert_eq!(
            parse("Erstat hund med kat.", false),
            Some(Replace { from: "hund".into(), to: "kat".into() })
        );
        assert_eq!(
            parse("Ret mandag til tirsdag.", false),
            Some(Replace { from: "mandag".into(), to: "tirsdag".into() })
        );
        assert_eq!(parse("Slet meget.", false), Some(Delete("meget".into())));
        assert_eq!(parse("Markér den røde bil.", false), Some(Select("den røde bil".into())));
    }

    #[test]
    fn prefix() {
        assert_eq!(parse("Insert.", true), None);
        assert_eq!(parse("Kommando, indsæt.", true), Some(Insert));
        assert_eq!(parse("Command replace a with b", true), Some(Replace { from: "a".into(), to: "b".into() }));
    }
}
