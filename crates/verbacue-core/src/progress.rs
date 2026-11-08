//! Progress reports from long tasks, and a way to stop them.

use crate::{
    error::{Error, ErrorKind, Result},
    subtitle::Cue,
};

/// Receives progress reports from long tasks, for example to show a progress bar.
///
/// `()` ignores the reports, and any `Fn(u64, u64)` closure can receive them.
pub trait Progress: Sync {
    /// `done` of `total` units are finished. The unit depends on the task: percent for
    /// transcription and video output, cues for translation, bytes for downloads.
    fn update(&self, done: u64, total: u64);

    /// True when the task must stop. Long tasks check it often and then return an
    /// [`ErrorKind::Cancelled`] error.
    fn cancelled(&self) -> bool {
        false
    }

    /// A new cue, as soon as transcription has it. The final result can differ, because
    /// layout and filters run after transcription.
    fn cue(&self, _cue: &Cue) {}
}

/// Returns an [`ErrorKind::Cancelled`] error if `progress` says the task must stop.
///
/// # Errors
/// Returns the error when [`Progress::cancelled`] is true.
pub fn check_cancel(progress: &(impl Progress + ?Sized)) -> Result<()> {
    if progress.cancelled() {
        return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
    }
    Ok(())
}

impl Progress for () {
    fn update(&self, _done: u64, _total: u64) {}
}

impl<F: Fn(u64, u64) + Sync> Progress for F {
    fn update(&self, done: u64, total: u64) {
        self(done, total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    struct Stopped;

    impl Progress for Stopped {
        fn update(&self, _done: u64, _total: u64) {}

        fn cancelled(&self) -> bool {
            true
        }
    }

    #[test]
    fn unit_and_closures_are_never_cancelled() {
        assert!(!().cancelled());
        assert!(!(|_: u64, _: u64| {}).cancelled());
    }

    #[test]
    fn check_cancel_returns_a_cancelled_error() {
        let err = check_cancel(&Stopped).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Cancelled);
        assert!(check_cancel(&()).is_ok());
    }
}
