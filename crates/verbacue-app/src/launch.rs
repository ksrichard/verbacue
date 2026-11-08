//! Decides whether `verbacue` runs the command line or opens the desktop app.

/// What `verbacue` does at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Cli,
    Gui,
    Help,
}

/// `args` is the number of arguments after the program name. `gui_command` is true for
/// `verbacue gui`. `gui_built` is true when this build has the desktop app.
pub fn mode(args: usize, gui_command: bool, has_display: bool, gui_built: bool) -> Mode {
    match (args, gui_command, gui_built) {
        (_, true, true) => Mode::Gui,
        (0, _, true) if has_display => Mode::Gui,
        (0, _, _) => Mode::Help,
        _ => Mode::Cli,
    }
}

/// True when a window can open: always on macOS and Windows, and on other systems
/// when an X11 or Wayland display is set (not over SSH or in a container).
pub fn has_display() -> bool {
    if cfg!(any(target_os = "macos", windows)) {
        return true;
    }
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_with_a_display_opens_the_gui() {
        assert_eq!(mode(0, false, true, true), Mode::Gui);
    }

    #[test]
    fn no_arguments_without_a_display_prints_help() {
        assert_eq!(mode(0, false, false, true), Mode::Help);
    }

    #[test]
    fn no_arguments_without_the_gui_prints_help() {
        assert_eq!(mode(0, false, true, false), Mode::Help);
    }

    #[test]
    fn gui_subcommand_opens_the_gui() {
        assert_eq!(mode(1, true, true, true), Mode::Gui);
    }

    #[test]
    fn gui_subcommand_without_the_feature_runs_the_cli() {
        // The CLI then stops with "this build has no GUI".
        assert_eq!(mode(1, true, true, false), Mode::Cli);
    }

    #[test]
    fn arguments_run_the_cli() {
        assert_eq!(mode(2, false, true, true), Mode::Cli);
    }
}
