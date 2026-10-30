# Diktatoren

On-demand dictation for the desktop. Say your wake phrase or press a hotkey, talk into the
input box, fix the text with your voice, then insert it into the window you were working in,
copy it to the clipboard, or throw it away.

Speech recognition, voice activity detection and wake-word detection all run locally through
[audio.cpp](https://github.com/lkarlslund/audio.cpp). No audio or text leaves your machine.

Runs on Windows, macOS and Linux (Wayland and X11) and lives in the system tray.

## Using it

1. Start Diktatoren. On first start the settings window opens on **Models**: download a speech
   model. Parakeet-TDT 0.6B v3 (25 European languages, Danish included) is a good default.
   Hviske v6 is a Danish-only alternative, and Moonshine Tiny a very fast English one.
2. Press the hotkey (default **Ctrl+Alt+Space**, **Cmd+Shift+Space** on macOS), or say the
   wake phrase if you have turned it on. A dictation box opens and listens.
3. Speak. Each pause ends an utterance, and it appears in the box. A grey live preview shows the
   utterance in progress.
4. Finish with **Enter** (insert into the previous window), **Ctrl+Enter** (copy), or **Esc**
   (cancel). You can also say "insert", "copy" or "cancel".

The tray menu has **Dictate**, **Settings…** and **Quit**.

### Hotkey modes

* **Toggle**: press to start listening, press again to pause. The box stays open until you
  finish.
* **Push to talk**: listens while the key is held.

On Wayland, applications cannot grab keys themselves. Diktatoren asks the desktop through the
GlobalShortcuts portal instead (KDE Plasma and GNOME 48 or newer support it), and the desktop
asks you to confirm once. Wherever that is unavailable, bind `diktatoren toggle` to a key in
your desktop's shortcut settings.

### Wake word

Turn it on under **Settings → General**. There are two kinds:

* **Spoken phrase.** Choose any words, in any language your speech model knows. The default is
  "Hey Diktatoren". While waiting, short utterances are recognised locally, compared with the
  phrase, and discarded. You can say the phrase on its own, or go straight on: "Hey
  Diktatoren, remind me to call the plumber" opens the box with "Remind me to call the
  plumber" already in it.
* **Wake-word models** ("Okay Nabu", "Alexa"). Tiny dedicated detectors that use almost no
  power, but each one listens for a fixed phrase.

After a wake word, listening stops by itself after a few seconds of silence (configurable).

### Editing by voice

Dictated text lands at the cursor, replacing any selection. Click or select text in the box,
or say "select …", to change where the next words go. A command must be said on its own, as
its own utterance. English and Danish phrasings both work:

| Say | Effect |
|---|---|
| insert · indsæt | insert the text into the previous window |
| copy · kopiér | copy to the clipboard |
| cancel · annuller | close without doing anything |
| pause · stop med at lytte | stop listening |
| new line · ny linje, new paragraph · nyt afsnit | line breaks |
| period, comma, question mark … · punktum, komma, spørgsmålstegn … | punctuation |
| undo · fortryd, redo · gentag | undo or redo |
| delete that · slet det | remove the selection, or the last dictated piece |
| delete last word / sentence · slet sidste ord / sætning | |
| delete X · slet X | remove the last occurrence of X |
| replace X with Y · erstat X med Y · ret X til Y | fix a word, keeping its capitalisation |
| select X · markér X | select X; what you say next replaces it |
| select all · markér alt, clear all · slet alt | |
| go to end / start · gå til slutningen / starten | move the cursor |

To avoid surprises you can require a prefix word ("command …" / "kommando …") under
**Settings → Voice commands**.

### Inserting text

Insertion puts the text on the clipboard and sends the paste shortcut to the window that had
focus. Afterwards the previous clipboard contents are put back. Choose **Type the text**
instead where pasting is blocked, and change the paste shortcut for terminals
(Ctrl+Shift+V or Shift+Insert).

* **Windows, X11**: works directly.
* **macOS**: grant Diktatoren Accessibility access (System Settings → Privacy & Security) so it
  can send the paste keystroke, and Microphone access when asked.
* **Wayland**: keystrokes go through the RemoteDesktop portal. The desktop asks for permission
  the first time and remembers it. Without the portal, `ydotool` is used if it is set up;
  otherwise the text stays on the clipboard for you to paste.

### Command line

```
diktatoren                  start in the tray (or open settings of the running instance)
diktatoren toggle           start or stop dictation; also start, stop, insert, copy, cancel,
                            settings, quit
diktatoren models           list downloadable models (* = installed)
diktatoren download <id>    download a model
diktatoren transcribe <wav> [id]
diktatoren listen [wav]     dictate in the terminal, from the microphone or a WAV file
```

Settings live in `config.toml` in the platform config directory
(`~/.config/diktatoren` on Linux), and models in the data directory
(`~/.local/share/diktatoren/models`). `DIKTATOREN_CONFIG` points at another config file.

## Installing

Daily builds of `main` are published as [releases](https://github.com/lkarlslund/diktatoren/releases)
named `rNNNNN`, where the number is the commit count on main.

* **Linux**: extract the `.tar.gz` and run `./install.sh`, which installs into `~/.local` with a
  menu entry. The `-vulkan` build uses the GPU through Vulkan.
* **Windows**: extract the `.zip` and run `diktatoren.exe`. Needs the Microsoft Visual C++
  runtime, which most systems already have.
* **macOS (Apple silicon)**: extract the `.zip` and run `diktatoren`. Uses Metal.

## Building

Requires Rust (stable), CMake and a C++17 compiler. On Debian/Ubuntu, also
`libasound2-dev libxkbcommon-dev libwayland-dev libxdo-dev`.

```
git clone --recurse-submodules https://github.com/lkarlslund/diktatoren
cd diktatoren
cargo build --release
./target/release/diktatoren
```

audio.cpp is built from the `third_party/audio.cpp` submodule, with only the speech
recognition, VAD and wake-word families compiled in. GPU backends are Cargo features of the
`audiocpp` crate: `cargo build --release --features audiocpp/vulkan` (also `cuda`, `hip`;
Metal is always on for macOS). `AUDIOCPP_MODELS` selects other model families, and
`AUDIOCPP_CMAKE_ARGS` passes extra CMake arguments.

`scripts/check-tree.sh` (run by CI and by the hook in `.githooks`) keeps binaries, model
weights, recordings and personal data out of the repository. Enable the hook with
`git config core.hooksPath .githooks`.

## License

MIT, see [LICENSE](LICENSE). audio.cpp is MIT licensed. Downloaded models keep the license
of their original release; the model pages on Hugging Face list them.
