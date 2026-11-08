//! Progress reports from long tasks.

/// Receives progress reports from long tasks, for example to show a progress bar.
///
/// `()` ignores the reports, and any `Fn(u64, u64)` closure can receive them.
pub trait Progress: Sync {
    /// `done` of `total` units are finished. The unit depends on the task: percent for
    /// transcription and video output, cues for translation, bytes for downloads.
    fn update(&self, done: u64, total: u64);
}

impl Progress for () {
    fn update(&self, _done: u64, _total: u64) {}
}

impl<F: Fn(u64, u64) + Sync> Progress for F {
    fn update(&self, done: u64, total: u64) {
        self(done, total);
    }
}
