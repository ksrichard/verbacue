//! The videos the user added, each with its own options and state.

use iced::widget::image;
use verbacue::pipeline::Outcome;

use crate::{app::Selection, worker::Failure};

/// One video in the queue.
#[derive(Debug, Clone)]
pub struct Item {
    /// Stays the same when other videos are removed.
    pub id: u64,
    pub selection: Selection,
    /// The preview frames, ready to draw.
    pub frames: Vec<image::Handle>,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Waiting,
    Running,
    /// Done, with the number of subtitle lines.
    Done(Outcome, usize),
    Failed(Failure),
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Running => "running",
            Self::Done(..) => "done",
            Self::Failed(_) => "failed",
        }
    }
}

impl Item {
    pub fn name(&self) -> String {
        self.selection
            .video
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        match &self.status {
            Status::Done(outcome, _) => Some(outcome),
            _ => None,
        }
    }

    /// The subtitle lines of a finished video, else 0.
    pub fn lines(&self) -> usize {
        match &self.status {
            Status::Done(_, lines) => *lines,
            _ => 0,
        }
    }
}
