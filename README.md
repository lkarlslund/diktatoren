# Diktatoren

On-demand dictation for the desktop. Say your wake phrase or press a hotkey, talk into the
input box, fix the text with your voice, then insert it into the window you were working in,
copy it to the clipboard, or throw it away.

Speech is recognised by an [audio.cpp](https://github.com/lkarlslund/audio.cpp) server
(`audiocpp_server`) that you run yourself, typically on the same machine with a GPU.
Wake-word and voice detection run inside Diktatoren. No audio or text goes to anyone else.

Runs on Windows, macOS and Linux (Wayland and X11) and lives in the system tray. Its windows
take their colours from the desktop: the KDE colour scheme, GNOME's style and accent colour, or
the light/dark mode and accent colour of Windows and macOS.

## Using it

1. Start Diktatoren. On first start the settings window opens on **Speech**: enter your
   audio.cpp server's address (for example `http://127.0.0.1:8099`), press **Connect**, pick
   one of its speech models, and press **Save**.
2. Press the hotkey (default **Ctrl+Alt+Space**, **Cmd+Shift+Space** on macOS), or say the
   wake phrase if you have turned it on. A dictation box opens and listens.
3. Speak. Each pause ends an utterance; it is sent to the server as a short WAV file and the
   text appears in the box.
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
  "Hey Diktatoren". While waiting, short utterances go to the speech server, are compared
  with the phrase, and are discarded. You can say the phrase on its own, or go straight on:
  "Hey Diktatoren, remind me to call the plumber" opens the box with "Remind me to call the
  plumber" already in it.
* **Wake-word models** ("Okay Nabu", "Alexa"). Tiny detectors that run inside Diktatoren with
  almost no power and no server traffic, but each listens for one fixed phrase. They download
  when you choose them.

After a wake word, listening stops by itself after a few seconds of silence (configurable).

### AI clean-up

Recognisers like Edda return words without punctuation and now and then mishear one. Under
**Settings → Speech → AI clean-up**, point Diktatoren at an OpenAI-compatible language-model
server (llama.cpp's `llama-server`, LM Studio, or similar) and pick a model:

* **Punctuation only** adds full stops, commas and capitals, and must leave every word as it is.
* **Punctuation and fixes** also corrects words that were clearly misheard and puts back small
  dropped words such as "at" or "og".

It also lays the text out: a line break after a salutation ("Hej Søren,"), the sign-off and
name on their own lines, and a blank line between paragraphs. A number said on its own
("et", "to", "punkt 3") starts a numbered list item on a new line.

Your words appear at once; a moment later the sentence is replaced with the tidied version. An
answer that changes too many words is treated as a rewrite and ignored, "undo" brings back
what was heard, and inserting waits for a tidy-up that is still running (up to 15 seconds).

### Dictate and Edit mode

The dictation window has two modes, shown as **Dictate / Edit** buttons next to the level
meter. Switch by clicking or by saying "edit" / "dictate" ("redigér" / "diktér" in Danish).

* **Dictate**: what you say becomes text at the cursor, replacing any selection. A few control
  commands said on their own still act: insert, copy, cancel, pause, new line, new paragraph,
  punctuation ("period", "comma" …), undo and "delete that".
* **Edit**: what you say is an editing command, and nothing is typed by accident. Besides the
  control commands: "replace X with Y", "delete X", "select X", "delete last word / sentence",
  "select all", "clear all", "go to end / start", "redo". After "select X", words that are no
  command replace the selection.

A command has to be said on its own, with a short pause before and after. Lone words match
loosely, so "Inset" or "Annuler" from the recogniser still count.

### Command language

Under **Settings → Voice commands**, choose **English** or **Dansk** and every command phrase
switches language (the default follows your system language). Each command is listed with its
phrases, separated by commas, and can be edited; editing one turns the set into **Custom**.
Picking English or Dansk again restores that preset.

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
diktatoren models           list wake-word models (* = installed)
diktatoren download <id>    download a wake-word model
diktatoren transcribe <wav> transcribe a WAV file on the speech server
diktatoren listen [wav]     dictate in the terminal, from the microphone or a WAV file
diktatoren mic-test         show the microphone level for a few seconds (records nothing)
```

Settings live in `config.toml` in the platform config directory
(`~/.config/diktatoren` on Linux), and wake-word models in the data directory
(`~/.local/share/diktatoren/models`). `DIKTATOREN_CONFIG` points at another config file.

## Installing

Daily builds of `main` are published as [releases](https://github.com/lkarlslund/diktatoren/releases)
named `rNNNNN`, where the number is the commit count on main.

* **Linux**: extract the `.tar.gz` and run `./install.sh`, which installs into `~/.local` with a
  menu entry.
* **Windows**: extract the `.zip` and run `diktatoren.exe`. Needs the Microsoft Visual C++
  runtime, which most systems already have.
* **macOS (Apple silicon)**: extract the `.zip` and run `diktatoren`.

## Building

Requires Rust (stable), CMake and a C++17 compiler. On Debian/Ubuntu, also
`libasound2-dev libxkbcommon-dev libwayland-dev libxdo-dev`.

```
git clone --recurse-submodules https://github.com/lkarlslund/diktatoren
cd diktatoren
cargo build --release
./target/release/diktatoren
```

audio.cpp is built from the `third_party/audio.cpp` submodule, with only voice detection and
the wake-word families compiled in. `AUDIOCPP_MODELS` selects other model families, and
`AUDIOCPP_CMAKE_ARGS` passes extra CMake arguments.

`scripts/check-tree.sh` (run by CI and by the hook in `.githooks`) keeps binaries, model
weights, recordings and personal data out of the repository. Enable the hook with
`git config core.hooksPath .githooks`.

## License

MIT, see [LICENSE](LICENSE). audio.cpp, which release builds bundle, is licensed under
Apache-2.0; its license ships beside it as `LICENSE-audio.cpp`. Downloaded models keep the license
of their original release; the model pages on Hugging Face list them.
