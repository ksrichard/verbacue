# verbacue desktop GUI: design

Date: 2026-11-08. Branch: `feature/gui`.

## Intent

verbacue is a command-line tool today. Many people who want subtitles for a video
do not use a terminal. The GUI gives them the same result with a few clicks: pick
a video, see a preview, choose burned-in subtitles or a separate SRT file, and
press one button. The wait during a conversion can be long (minutes to hours), so
the GUI makes that wait pleasant with playful barbecue animations and clear
progress. All settings live on a separate page, so the main flow stays simple.

The approved mockups are the visual specification. They are 5 HTML files in
`docs/superpowers/specs/gui-mockups/` (open them in a browser; the inline styles
hold the exact layout, sizes, spacing, copy, icons and animation timings). The
same mockups are also at https://claude.ai/artifact/TyWwPY54UfF18UcawZuFX8. The
owner said the mockups are "perfect"; the GUI must match them closely. Texts in
square brackets (`[FILE NAME]`) are placeholders for real data.

## Goals

- A new Rust library crate, `verbacue-gui`, that implements the whole GUI with
  `iced`, `tokio` and `thiserror`.
- The `verbacue` binary opens the GUI:
  - with `verbacue gui`, and
  - with no arguments (for example a double-click in a file manager).
  With any other arguments, the binary works as the CLI does today.
- The flow and the look of the 5 mockup screens:
  1. Pick a video: an animated grill drop zone, a "Choose a video" button, drag
     and drop, and a list of recent files.
  2. Preview and choose: a preview of the video, file facts, 2 large cards
     ("Burned in" and "Separate SRT file"), target language, audio track, and
     "Start grilling".
  3. Converting: the verb skewer over animated flames with sparks and smoke, a
     funny line that changes every few seconds, a shimmering progress bar, the 4
     steps, the newest transcribed lines, and Cancel.
  4. Done: a check mark that pops in, confetti, the output files, and "Play
     video", "Show in folder" and "Grill another one".
  5. Settings: all options of the CLI that make sense in a GUI (see Settings).
- Many animations: every screen change, every card and every button has a
  short, smooth transition. The target is 60 fps while the pipeline runs. It is a
  target, not a hard limit: the pipeline must not slow down for it. Whisper and
  llama.cpp already use at most 8 threads, which leaves cores free on most
  machines.

## Non-goals

- Batch conversion of many videos at once. The GUI handles one video at a time.
- A subtitle editor.
- Video playback inside the app. "Play" opens the system video player.
- A macOS `.app` bundle, a Windows installer, code signing, auto-update.
- The MKV output with soft subtitle tracks (`--embed`). It stays CLI-only for now.
- Translating existing SRT files and burning existing SRT files (`verbacue
  translate`, `verbacue burn`). CLI-only for now.

## Anti-goals (a failure even if it works)

- A GUI that looks like a generic form or a default iced theme.
- Animations that stutter, or a window that freezes while work runs.
- A second copy of the pipeline logic. The CLI and the GUI must run the same
  code, so a fix in one place fixes both.
- A Linux CPU binary that is no longer fully static.

## Decisions by the owner

1. The CLI crate `verbacue-cli` is renamed to `verbacue-app`. The binary stays
   `verbacue`.
2. `verbacue gui` and `verbacue` with no arguments both open the GUI.
3. The Linux CPU binaries stay fully static and have no GUI. All other release
   binaries include the GUI: macOS, Windows CPU/CUDA/Vulkan, Linux CUDA/Vulkan.
4. The GUI is a separate crate (`verbacue-gui`); `verbacue-app` depends on it.
5. The GUI uses `iced`, `tokio` and `thiserror`.

## Architecture

```
verbacue-app (binary "verbacue")
 ├── verbacue          (library facade, with the new `pipeline` module)
 └── verbacue-gui      (optional, feature "gui", on by default)
      └── verbacue     (the same facade)
```

### Crates

- **`verbacue-app`** (renamed from `verbacue-cli`). Feature `gui` (default) adds
  the optional dependency `verbacue-gui`. Without `gui`, `verbacue gui` stops with
  the error "this build has no GUI", and no arguments prints the help as today.
  The features `cuda` and `vulkan` also turn on the same features in
  `verbacue-gui` when `gui` is on (use `verbacue-gui?/cuda`).
- **`verbacue-gui`** (new, library). Public API: one function,
  `pub fn run() -> Result<(), verbacue_gui::Error>`, that opens the window and
  returns when it closes. It depends on the `verbacue` facade with the features
  it needs. It does not depend on `verbacue-app`.
- **`verbacue`** facade: gets a new `pipeline` module (below).

### Shared pipeline (new public API in the facade)

The step order that `process_video` in the CLI has today (decode → transcribe →
lay out → write the transcript → translate → write → embed → burn, with the
"skip if done" rule) moves into `verbacue::pipeline`. The CLI and the GUI both
call it. The CLI keeps its flags, file names and messages.

The module reports progress as events, not as progress bars:

- which step runs (`Decode`, `Transcribe`, `Translate`, `Embed`, `Burn`, and
  model downloads),
- the position and total of that step,
- each new transcript cue (for the live lines on the Converting screen),
- each written file.

The CLI turns the events into its `indicatif` bars; the GUI turns them into
messages. The builder decides the exact types.

### Cancel (additive change to the core API)

The GUI needs a working Cancel button. Today the long functions cannot stop
early. The change:

- `verbacue_core::Progress` gets a new method with a default:
  `fn cancelled(&self) -> bool { false }`. Existing implementations stay valid.
- `ErrorKind` gets a new variant `Cancelled`.
- The long loops check `cancelled()` and return a `Cancelled` error: FFmpeg
  decode, burn and embed loops (per packet or frame), Whisper (its abort
  callback), the translation loop (between batches), and model downloads (per
  chunk).
- A cancelled run deletes its partial output files, the same as a failed run.

### Video preview (additive change to `verbacue-media`)

A new function in `verbacue_media::video` decodes a few frames (for example 6,
spread over the length of the video) and returns them as RGBA images scaled to a
maximum width. It runs on the blocking thread pool. The Preview screen shows the
first frame at once and cross-fades through the others in a slow loop. The play
button on the preview opens the file in the system player.

The same module (or `audio::tracks`, which exists) gives the file facts on the
Preview screen: container, resolution, video codec, duration, audio tracks.

### Threads and tokio

- iced runs with its `tokio` feature, so `Task`s run on a tokio runtime.
- The pipeline is blocking code. The GUI runs it with
  `tokio::task::spawn_blocking` and sends its events through a
  `tokio::sync::mpsc` channel. A `Task` or `Subscription` turns the channel into
  iced messages.
- The cancel flag is an `Arc<AtomicBool>` that the GUI's `Progress`
  implementation reads in `cancelled()`.
- File dialogs use `rfd` with its async API.
- The window must never block: no pipeline, FFmpeg or file system work in
  `update` or `view`.

### Errors

`verbacue-gui` has its own error enum made with `thiserror`, for example:
`Pipeline(verbacue::Error)`, `Settings` (read or write of the settings file),
`Window` (iced start-up). The GUI shows pipeline errors as a friendly card on
the Converting screen with "Try again" and "Back", and the full message in a
details area. The `ErrorKind` decides the friendly text: for example
`MissingKey` → "Add your DeepL key in Settings".

## Look and animation

The mockups set the look. The builder copies these values from the mockups:

- Colors: background `#120e0d` with a warm radial glow (`#2b1a14`), cards
  `rgba(243,233,220,.04)`, text `#f3e9dc`, muted text `#a89a8d`/`#b9aa9c`, accent
  `#e8622a` (hover/active text `#ffb38a`), verb chunk colors `#7a3b26`,
  `#3f8f3a`, `#f2c14e`, `#c8402f`, flame gradient `#ff7a1a` → `#ffe680`, success
  green `#5dbb55`.
- Fonts: Space Grotesk (headings, logo) and Inter (text). Both are under the SIL
  Open Font License; bundle them in the crate and add them to
  `THIRD-PARTY-LICENSES.txt` and `licenses/generate.sh`.
- Shapes: large radii (14–28 px), a side navigation with the logo, "New
  subtitles" and "Settings".
- Animations, built with iced's `animation` support and `canvas`:
  - Grill (Pick a video): flickering flames, smoke as 3 rising subtitle bars, a
    glowing dashed drop zone. The drop zone lights up when a file is dragged
    over the window.
  - Preview: cards and controls fade up one after another; the chosen card
    gets the accent border with a smooth change; the start button pulses.
  - Converting: the 4 verb chunks sizzle on the skewer; the chunk for the
    current step glows and the chunks for done steps look "cooked"; flames,
    sparks and smoke move all the time; the progress bar has a moving shimmer;
    a funny line changes every 3–4 s with a cross-fade; new transcript lines
    slide in.
  - Done: the check mark pops in with an overshoot; confetti falls.
  - Settings: switches slide with a small bounce; cards fade up.
- Animations run only while their screen is visible, so an idle window uses no
  CPU.

The funny lines are a fixed list in the code, in English, for example
"Marinating the verbs…", "Listening very carefully…", "Flipping the nouns…",
"Adding a pinch of grammar…". The builder may add more in the same tone.

## Settings

A TOML file `gui.toml` in the verbacue config folder (`~/.config/verbacue` on
macOS and Linux, `%APPDATA%\verbacue` on Windows; the folder that the online
translators already use). A missing or broken file gives the defaults; a broken
file is renamed to `gui.toml.bad` and the GUI shows a short notice.

| Setting | Values | Default |
|---|---|---|
| Whisper model | the known Whisper models | `large-v3` |
| Spoken language | Auto or a language | Auto |
| Skip silence and music (VAD) | on/off | on |
| Translator | Local, DeepL, Google, Azure, Server, None | Local |
| Language model (Local) | the known LLMs, or a GGUF file | `translategemma-12b` |
| Server URL and model (Server) | text | as the CLI |
| Form of address | Informal, Formal | Informal |
| Glossary | a file or none | none |
| Subtitle format | SRT, WebVTT | SRT |
| Save to | next to the video, or a folder | next to the video |
| Last target language | a language | English |
| Recent files | up to 8 paths with their last options | empty |

- The Translator control shows Local, DeepL, Google and Server as in the mockup;
  Azure and None go in a "More" menu.
- API keys stay where the CLI reads them (environment variables or key files).
  The GUI does not store keys. For an online translator, Settings shows whether
  a key is found and a button that opens the config folder.
- "Models on this computer" lists the known models with size and state, and
  downloads (with a progress bar) or removes them.

## Screen behaviour

- Pick a video accepts a file from the button, drag and drop, or the recent
  list. A file that FFmpeg cannot open, or that has no audio stream, shows a
  short error on the drop zone and stays on this screen.
- Preview and choose: "Burned in" writes the MP4 and also keeps the SRT files,
  as the CLI does with `--burn`. "Separate SRT file" writes only the subtitle
  files. The target language list has all languages of `languages()`, plus
  "Keep the original language" (no translation). The audio track list comes from
  the file.
- If the output file already exists, the GUI asks "Replace it?" before it
  starts. "Replace" runs the whole pipeline again and overwrites all its files,
  the same as `--force` in the CLI. The pipeline does not reuse old transcript
  files today, and this change does not add that.
- Converting: the steps in the mockup are "Read audio, Transcribe, Translate,
  Burn in"; the last step is "Save" for the SRT choice. A model download that is
  needed shows as an extra step before the step that needs it.
- Closing the window during a conversion asks for confirmation and then
  cancels.
- Cancel stops the work within a few seconds: the local translator checks the
  flag inside its token loop, not only between batches, and HTTP requests keep
  their existing timeouts (60 s for the online services). Waiting for one open
  HTTP request to end is acceptable.
- The recent list shows the file name, the folder and the last options, without
  thumbnails. Entries whose file no longer exists are removed when the list
  loads.

## Build and CI

- Linux CPU builds (`docker/build-linux.sh`, CPU case) build `verbacue-app` with
  `--no-default-features`, so they stay static. Their tests exclude
  `verbacue-gui`.
- All other builds keep the default features and include the GUI.
- The link checks must still pass. iced loads X11/Wayland, Vulkan and OpenGL at
  run time on Linux, so the Linux allow-lists should not grow. The Windows DLL
  allow-list grows by the system DLLs that the window and the GPU need (for
  example `d3d12`, `dwmapi`, `uxtheme`, `imm32`, `opengl32`); only DLLs that are
  part of Windows are allowed.
- macOS still links only system frameworks.
- New dependencies must have a version that is at least 2 weeks old.

## Testing

- The GUI's state logic (`update`) is plain Rust; unit tests cover the screen
  flow, the settings defaults, the load and save of `gui.toml`, and the error
  cards.
- `verbacue::pipeline` gets unit tests for the step order, the skip rule and the
  cancel path. The existing CLI end-to-end tests must still pass unchanged.
- The preview function gets a test with a short generated video.
- The owner checks the look against the mockups by hand on macOS.

## Left to the builder

- The module layout inside `verbacue-gui`.
- The exact types of the pipeline events and of the preview frames.
- How the animations are drawn (canvas, widgets, or both), as long as they look
  like the mockups.
- The exact wording of error texts, in the same plain and friendly tone.
- The renderer choice inside iced (wgpu with tiny-skia fallback is the default).

## Calls I made (not decided by the owner)

- The pipeline moves into the facade as a new public module, so the CLI and the
  GUI share it.
- Cancel support is an additive change to `Progress` and `ErrorKind`.
- The preview is a cross-fade of still frames, not real playback; "Play" opens
  the system player.
- `--embed`, `verbacue translate` and `verbacue burn` stay CLI-only.
- Settings are stored in `gui.toml`; keys are never stored by the GUI.
- On Windows, `verbacue` with no arguments started from Explorer would show an
  empty console window next to the GUI. The GUI detaches from that console
  (`FreeConsole`) when the process is the only one attached to it. That needs a
  small `unsafe` call, in its own module with `#![allow(unsafe_code)]`, the same
  as `hwdecode.rs`. A console window can still show for a moment at start; that
  is accepted. The binary stays a console program, so the CLI output keeps
  working on Windows.
- With no arguments and no display (Linux without `DISPLAY` and
  `WAYLAND_DISPLAY`, for example over SSH or in a container), `verbacue` prints
  the help as today instead of failing to open a window. If the window cannot
  open for another reason, the error says so and names `verbacue --help`.
- Recent files have no thumbnails (see Screen behaviour).
