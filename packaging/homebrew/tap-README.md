# Homebrew tap for verbacue

[verbacue](https://github.com/ksrichard/verbacue) transcribes, translates and lays out
subtitles for any video, on your own computer.

## The command and the app (macOS and Linux)

```sh
brew install ksrichard/verbacue/verbacue
```

This works on macOS (Apple Silicon and Intel), on Linux (x86_64 and arm64) and on
Windows in WSL. `verbacue` with no arguments, or `verbacue gui`, opens the desktop app.
On Linux the app needs glibc 2.35 or newer and a desktop (X11 or Wayland); the command
line also works without one.

## The macOS app in /Applications

```sh
brew install --cask ksrichard/verbacue/verbacue-app
```

verbacue.app is not notarized by Apple. At the first start macOS blocks it: open it
once, then choose Open Anyway in System Settings > Privacy & Security. The cask also
installs the `verbacue` command, so install either the cask or the formula.

## Updates

```sh
brew update && brew upgrade verbacue          # or: brew upgrade --cask verbacue-app
```

The files in this repository are written by the release workflow of verbacue
(`packaging/homebrew/` there). Change them there, not here.
