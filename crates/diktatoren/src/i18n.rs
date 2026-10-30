//! Interface language. English strings are the keys; Danish translations are looked up by
//! them, and anything untranslated stays English.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::commands::Language;

static DANISH: AtomicBool = AtomicBool::new(false);

/// The interface follows the command language, so buttons read like what can be said.
pub fn set_language(commands: Language) {
    let danish = match commands {
        Language::Danish => true,
        Language::English => false,
        Language::Custom => Language::from_locale() == Language::Danish,
    };
    DANISH.store(danish, Ordering::Relaxed);
}

pub fn is_danish() -> bool {
    DANISH.load(Ordering::Relaxed)
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| DA.iter().copied().collect())
}

/// The text in the interface language.
pub fn tr(english: &'static str) -> &'static str {
    if is_danish() {
        table().get(english).copied().unwrap_or(english)
    } else {
        english
    }
}

/// Like [`tr`], filling each `{}` with the next argument.
pub fn trf(english: &'static str, args: &[&str]) -> String {
    let mut out = tr(english).to_string();
    for arg in args {
        out = out.replacen("{}", arg, 1);
    }
    out
}

/// Capitalises the first letter, for turning a spoken phrase into a button label.
pub fn label(phrase: &str) -> String {
    let mut chars = phrase.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

const DA: &[(&str, &str)] = &[
    // Dictation window
    ("Speak, or type…", "Tal, eller skriv…"),
    ("Paused. Say “{}” or “{}” to go on.", "På pause. Sig “{}” eller “{}” for at fortsætte."),
    ("Tidying up…", "Renskriver…"),
    ("Tidying up the text…", "Renskriver teksten…"),
    ("“{}” is not an edit command. Say “{}” to write.", "“{}” er ikke en redigeringskommando. Sig “{}” for at skrive."),
    ("Nothing to change for “{}”", "Intet at ændre for “{}”"),
    ("Could not insert the text ({}). It is on the clipboard.", "Kunne ikke indsætte teksten ({}). Den ligger på udklipsholderen."),
    ("Ready · {}", "Klar · {}"),
    ("Starting…", "Starter…"),
    ("Download failed: {}", "Download mislykkedes: {}"),
    // Window titles and tray
    ("Diktatoren settings", "Diktatoren indstillinger"),
    ("Dictate", "Diktér"),
    ("Settings…", "Indstillinger…"),
    ("Quit", "Afslut"),
    // Settings: tabs and footer
    ("General", "Generelt"),
    ("Speech", "Tale"),
    ("Output", "Indsættelse"),
    ("Voice commands", "Stemmekommandoer"),
    ("Saved", "Gemt"),
    ("Close", "Luk"),
    ("Save", "Gem"),
    // General
    ("Activation", "Aktivering"),
    ("Hotkey", "Genvejstast"),
    ("Hotkey works as", "Genvejstasten virker som"),
    ("Toggle", "Til/fra"),
    ("Push to talk", "Hold for at tale"),
    ("Toggle: press to start, press again to stop. Push to talk: listens while held.", "Til/fra: tryk for at starte, tryk igen for at stoppe. Hold for at tale: lytter, mens tasten holdes nede."),
    ("On Wayland the desktop owns global shortcuts: it asks to confirm this one the first time, and you change it later in its shortcut settings. You can also bind the command `diktatoren toggle`.", "På Wayland ejer skrivebordet de globale genveje: det beder dig bekræfte denne første gang, og du ændrer den siden i dets genvejsindstillinger. Du kan også binde kommandoen `diktatoren toggle` til en tast."),
    ("For example Ctrl+Alt+Space, Super+D or F9.", "For eksempel Ctrl+Alt+Space, Super+D eller F9."),
    ("Start dictation with a wake word", "Start diktat med et vækkeord"),
    ("Wake word", "Vækkeord"),
    ("Spoken phrase (your own words)", "Talt sætning (dine egne ord)"),
    ("Phrase", "Sætning"),
    ("Say the phrase on its own, or go straight on: “Hey Diktatoren, remind me to …”. While waiting, short utterances go to your speech server, are compared with the phrase, and are discarded.", "Sig sætningen for sig selv, eller fortsæt direkte: “Hej Diktatoren, mind mig om …”. Mens den venter, sendes korte ytringer til din taleserver, sammenlignes med sætningen og kasseres."),
    ("A wake-word model listens for one fixed phrase on this computer and uses very little power.", "En vækkeordsmodel lytter efter én fast sætning på denne computer og bruger meget lidt strøm."),
    ("{} (downloads when chosen)", "{} (hentes, når den vælges)"),
    ("Downloading…", "Henter…"),
    ("Sensitivity", "Følsomhed"),
    ("model default", "modellens standard"),
    ("threshold {}", "tærskel {}"),
    ("Close if silent for", "Luk ved stilhed i"),
    ("After a wake word, the dictation window closes again if nothing is said for this long, to undo false wakes. Once you have said something, it stays open.", "Efter et vækkeord lukker diktatvinduet igen, hvis der ikke bliver sagt noget så længe, så falske opvågninger forsvinder. Når du har sagt noget, bliver det åbent."),
    ("Microphone", "Mikrofon"),
    ("Input", "Indgang"),
    ("System default", "Systemets standard"),
    // Speech
    ("Speech recognition", "Talegenkendelse"),
    ("Server", "Server"),
    ("Connect", "Forbind"),
    ("Model", "Model"),
    ("Connect first", "Forbind først"),
    ("Connected.", "Forbundet."),
    ("The address of an audio.cpp server (audiocpp_server), such as http://127.0.0.1:8099.", "Adressen på en audio.cpp-server (audiocpp_server), for eksempel http://127.0.0.1:8099."),
    ("Press Connect to list the server's speech models.", "Tryk Forbind for at se serverens talemodeller."),
    ("Press Connect to list the server's models.", "Tryk Forbind for at se serverens modeller."),
    ("AI clean-up", "AI-renskrivning"),
    ("Clean up dictation", "Renskriv diktat"),
    ("Off", "Fra"),
    ("Punctuation only", "Kun tegnsætning"),
    ("Punctuation and fixes", "Tegnsætning og rettelser"),
    ("A language model adds punctuation and capitals to what you dictate, and with “fixes” also corrects words the recogniser misheard. Your words appear at once and are tidied a moment later; “undo” brings back what was heard. Answers that rewrite the text are ignored.", "En sprogmodel sætter tegn og store bogstaver i det, du dikterer, og med “rettelser” retter den også ord, som genkendelsen har hørt forkert. Dine ord vises med det samme og renskrives et øjeblik efter; “fortryd” henter det hørte tilbage. Svar, der omskriver teksten, bliver ignoreret."),
    ("An OpenAI-compatible server, such as llama.cpp's llama-server or LM Studio.", "En OpenAI-kompatibel server, for eksempel llama.cpp's llama-server eller LM Studio."),
    // Output
    ("Finishing", "Afslutning"),
    ("Enter does", "Enter"),
    ("Insert into the window", "Indsætter i vinduet"),
    ("Copy to the clipboard", "Kopierer til udklipsholderen"),
    ("Ctrl+Enter does the other. Shift+Enter starts a new line; Esc cancels.", "Ctrl+Enter gør det andet. Shift+Enter giver en ny linje; Esc annullerer."),
    ("Inserting", "Indsættelse"),
    ("Insert by", "Indsæt ved at"),
    ("Paste from the clipboard", "Sætte ind fra udklipsholderen"),
    ("Type the text", "Skrive teksten"),
    ("Pasting is fast and handles any characters. Typing works where pasting is blocked.", "At sætte ind er hurtigt og klarer alle tegn. At skrive virker, hvor indsættelse er blokeret."),
    ("Paste shortcut", "Genvej til indsæt"),
    ("Terminals often want Ctrl+Shift+V or Shift+Insert.", "Terminaler vil ofte have Ctrl+Shift+V eller Shift+Insert."),
    ("Put the previous clipboard contents back after pasting", "Læg det tidligere indhold tilbage på udklipsholderen efter indsættelse"),
    // Voice commands
    ("Recognise voice commands", "Genkend stemmekommandoer"),
    ("Language", "Sprog"),
    ("Say a command on its own, with a short pause before and after. Each command takes the phrases listed, separated by commas; editing one makes the set Custom. The dictation window is always paused, dictating or editing, switched by voice or with its buttons.", "Sig en kommando for sig selv, med en kort pause før og efter. Hver kommando reagerer på de viste vendinger, adskilt af kommaer; retter du én, bliver sættet Tilpasset. Diktatvinduet er altid på pause, dikterer eller redigerer, og skiftes med stemmen eller knapperne."),
    ("In every state", "Altid"),
    ("In Edit mode", "Når der redigeres"),
    ("While editing, nothing you say is written as text unless something is selected. “Replace mandag med tirsdag” combines a replace verb and a separator.", "Mens du redigerer, bliver intet skrevet som tekst, medmindre noget er markeret. “Erstat mandag med tirsdag” kombinerer et erstat-verbum og et skilleord."),
    ("Custom", "Tilpasset"),
    // Command names
    ("Insert", "Indsæt"),
    ("Copy", "Kopiér"),
    ("Cancel", "Annuller"),
    ("Pause (only dictate and edit are heard)", "Pause (kun diktér og redigér høres)"),
    ("Edit mode (also leaves a pause)", "Redigér (forlader også en pause)"),
    ("Dictate mode (also leaves a pause)", "Diktér (forlader også en pause)"),
    ("Next sentence", "Næste sætning"),
    ("New line", "Ny linje"),
    ("New paragraph", "Nyt afsnit"),
    ("Full stop", "Punktum"),
    ("Comma", "Komma"),
    ("Question mark", "Spørgsmålstegn"),
    ("Exclamation mark", "Udråbstegn"),
    ("Colon", "Kolon"),
    ("Semicolon", "Semikolon"),
    ("Dash", "Tankestreg"),
    ("Undo", "Fortryd"),
    ("Delete the selection or last piece", "Slet markeringen eller det sidste stykke"),
    ("Redo", "Gentag"),
    ("Delete the last word", "Slet det sidste ord"),
    ("Delete the last sentence", "Slet den sidste sætning"),
    ("Delete everything", "Slet alt"),
    ("Select everything", "Markér alt"),
    ("Cursor to the end", "Markør til slutningen"),
    ("Cursor to the start", "Markør til starten"),
    ("Replace … (verb)", "Erstat … (verbum)"),
    ("… with … (separator)", "… med … (skilleord)"),
    ("Delete … (verb)", "Slet … (verbum)"),
    ("Select … (verb)", "Markér … (verbum)"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_and_fills() {
        set_language(Language::Danish);
        assert_eq!(tr("Save"), "Gem");
        assert_eq!(tr("Something new"), "Something new");
        assert_eq!(trf("Ready · {}", &["Edda"]), "Klar · Edda");
        set_language(Language::English);
        assert_eq!(tr("Save"), "Save");
        assert_eq!(label("diktér"), "Diktér");
    }
}
