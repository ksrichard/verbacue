//! Runs the pipeline on its own thread, so the window never waits for it.
//!
//! One thread owns the [`Pipeline`] for the whole life of the app. That keeps the
//! translation model loaded between videos. Jobs come in through a channel, and the
//! pipeline events go back to the app as [`Update`]s.

use std::sync::{
    Arc, Mutex,
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
    Progress {
        done: u64,
        total: u64,
    },
    Cue(Cue),
    Note(String),
    /// The film notes; the pipeline waits for [`Reply::Notes`].
    Review(String),
    /// A model licence to accept before the download; the pipeline waits for
    /// [`Reply::Licence`].
    Licence {
        model: String,
        text: String,
    },
    Finished(Result<Outcome, Failure>),
}

/// The answer of the user to a question of the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// The corrected film notes.
    Notes(String),
    /// True when the user accepts the licence.
    Licence(bool),
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

type Request = (
    Job,
    Arc<AtomicBool>,
    UnboundedSender<Update>,
    mpsc::Receiver<Reply>,
);

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
                for (job, cancel, updates, replies) in requests {
                    let replies = Mutex::new(replies);
                    let sink = Forward {
                        updates: &updates,
                        cancel: &cancel,
                        replies: &replies,
                    };
                    let result = pipeline
                        .run(&job, &sink)
                        .map_err(|e| Failure::new(e.kind(), verbacue::error::report(&e)));
                    let _ = updates.send(Update::Finished(result));
                }
            })
            .expect("cannot start the pipeline thread");
        Self { jobs }
    }

    /// Starts `job` and returns its updates. The last one is always `Finished`.
    /// `replies` brings the answers to [`Update::Review`] and [`Update::Licence`].
    pub fn run(
        &self,
        job: Job,
        cancel: Arc<AtomicBool>,
        replies: mpsc::Receiver<Reply>,
    ) -> impl Stream<Item = Update> + use<> {
        let (updates, mut received) = unbounded_channel();
        if let Err(mpsc::SendError((_, _, updates, _))) =
            self.jobs.send((job, cancel, updates, replies))
        {
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
    /// In a mutex, because a sink must be `Sync`.
    replies: &'a Mutex<mpsc::Receiver<Reply>>,
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

    fn review_notes(&self, notes: &str) -> Option<String> {
        match self.ask(Update::Review(notes.to_string()))? {
            Reply::Notes(text) => Some(text),
            Reply::Licence(_) => None,
        }
    }

    fn accept_licence(&self, model: &str, licence: &str) -> bool {
        let question = Update::Licence {
            model: model.to_string(),
            text: licence.to_string(),
        };
        matches!(self.ask(question), Some(Reply::Licence(true)))
    }
}

impl Forward<'_> {
    /// Sends `question` to the app and waits for the answer. Stops waiting when the
    /// job is cancelled or the app is gone.
    fn ask(&self, question: Update) -> Option<Reply> {
        self.updates.send(question).ok()?;
        loop {
            if self.cancelled() {
                return None;
            }
            let replies = self.replies.lock().ok()?;
            match replies.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(reply) => return Some(reply),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_review_waits_for_the_answer() {
        let (updates, mut sent) = unbounded_channel();
        let cancel = AtomicBool::new(false);
        let (reply, replies) = mpsc::channel();
        let replies = Mutex::new(replies);
        let sink = Forward {
            updates: &updates,
            cancel: &cancel,
            replies: &replies,
        };
        reply.send(Reply::Notes("edited".to_string())).unwrap();
        assert_eq!(sink.review_notes("automatic"), Some("edited".to_string()));
        assert!(matches!(sent.try_recv(), Ok(Update::Review(t)) if t == "automatic"));
    }

    #[test]
    fn notes_review_stops_waiting_when_cancelled() {
        let (updates, _sent) = unbounded_channel();
        let cancel = AtomicBool::new(true);
        let (_reply, replies) = mpsc::channel::<Reply>();
        let replies = Mutex::new(replies);
        let sink = Forward {
            updates: &updates,
            cancel: &cancel,
            replies: &replies,
        };
        assert_eq!(sink.review_notes("automatic"), None);
    }

    #[test]
    fn licence_waits_for_the_answer() {
        let (updates, mut sent) = unbounded_channel();
        let cancel = AtomicBool::new(false);
        let (reply, replies) = mpsc::channel();
        let replies = Mutex::new(replies);
        let sink = Forward {
            updates: &updates,
            cancel: &cancel,
            replies: &replies,
        };
        reply.send(Reply::Licence(true)).unwrap();
        assert!(sink.accept_licence("xtts-v2", "CPML"));
        assert!(matches!(
            sent.try_recv(),
            Ok(Update::Licence { model, text }) if model == "xtts-v2" && text == "CPML"
        ));
        reply.send(Reply::Licence(false)).unwrap();
        assert!(!sink.accept_licence("xtts-v2", "CPML"));
    }
}
