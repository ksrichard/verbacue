//! Runs the pipeline on its own thread, so the window never waits for it.
//!
//! One thread owns the [`Pipeline`] for the whole life of the app. That keeps the
//! translation model loaded between videos. Jobs come in through a channel, and the
//! pipeline events go back to the app as [`Update`]s.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use iced::futures::{Stream, stream};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use verbacue::{
    ErrorKind,
    pipeline::{Event, Job, Outcome, Pipeline, Sink, Step},
    subtitle::Cue,
};

/// A pipeline event as owned data, for the app.
#[derive(Debug, Clone)]
pub enum Update {
    Step(Step),
    Progress { done: u64, total: u64 },
    Cue(Cue),
    Note(String),
    Finished(Result<Outcome, Failure>),
}

/// Why a job did not finish.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub kind: ErrorKind,
    /// The whole error chain, for the details area.
    pub message: String,
}

impl Failure {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// A short, friendly text for each kind of error.
pub fn friendly(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::MissingKey => "Add your key for this service in Settings.",
        ErrorKind::KeyRefused => "The service refused your key. Check it in Settings.",
        ErrorKind::QuotaExceeded => "This key has no characters left this month.",
        ErrorKind::Network => "verbacue cannot reach the service. Check your internet connection.",
        ErrorKind::InvalidInput => "verbacue cannot read this video.",
        ErrorKind::NotFound => "A file or a model is missing.",
        _ => "Something went wrong.",
    }
}

type Request = (Job, Arc<AtomicBool>, UnboundedSender<Update>);

/// The pipeline thread.
pub struct Worker {
    jobs: mpsc::Sender<Request>,
}

impl Worker {
    pub fn new() -> Self {
        let (jobs, requests) = mpsc::channel::<Request>();
        std::thread::Builder::new()
            .name("verbacue-pipeline".into())
            .spawn(move || {
                let mut pipeline = Pipeline::new();
                for (job, cancel, updates) in requests {
                    let sink = Forward {
                        updates: &updates,
                        cancel: &cancel,
                    };
                    let result = pipeline
                        .run(&job, &sink)
                        .map_err(|e| Failure::new(e.kind(), format!("{e:#}")));
                    let _ = updates.send(Update::Finished(result));
                }
            })
            .expect("cannot start the pipeline thread");
        Self { jobs }
    }

    /// Starts `job` and returns its updates. The last one is always `Finished`.
    pub fn run(&self, job: Job, cancel: Arc<AtomicBool>) -> impl Stream<Item = Update> + use<> {
        let (updates, mut received) = unbounded_channel();
        if let Err(mpsc::SendError((_, _, updates))) = self.jobs.send((job, cancel, updates)) {
            let _ = updates.send(Update::Finished(Err(Failure::new(
                ErrorKind::Other,
                "the pipeline thread stopped",
            ))));
        }
        stream::poll_fn(move |cx| received.poll_recv(cx))
    }
}

/// Sends the pipeline events to the app.
struct Forward<'a> {
    updates: &'a UnboundedSender<Update>,
    cancel: &'a AtomicBool,
}

impl Sink for Forward<'_> {
    fn event(&self, event: Event<'_>) {
        let update = match event {
            Event::Step(step) => Update::Step(step),
            Event::Progress { done, total } => Update::Progress { done, total },
            Event::Cue(cue) => Update::Cue(cue.clone()),
            Event::Note(note) => Update::Note(note.to_string()),
            Event::Wrote(_) | Event::Skipped(_) => return,
        };
        let _ = self.updates.send(update);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}
