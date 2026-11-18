//! Converting: the skewer animation, a funny line, the progress bar, the steps, the
//! newest transcribed lines, and Cancel. A failed job shows an error card here.

use std::{
    collections::VecDeque,
    sync::{Arc, atomic::AtomicBool},
};

use iced::{
    Alignment, Border, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse,
    widget::{
        button, canvas,
        canvas::{Frame, Geometry, Path, gradient::Linear},
        column, container, row, scrollable, space, text, text_editor,
    },
};
use verbacue::{
    pipeline::{Job, Step},
    subtitle::Cue,
};

use crate::{
    anim::{cycle, skewer::Skewer, wave},
    app::{App, Message},
    queue::Status,
    theme,
    worker::{Failure, Update, friendly},
};

/// The narrowest a step may get: wide enough for its longest word.
const STEP_MIN_WIDTH: f32 = 120.0;
/// The gap between the steps.
const STEP_GAP: f32 = 8.0;

/// The side padding of the screen.
const SIDE_PADDING: f32 = 56.0;

/// The width that the content of this screen gets: the window without the
/// navigation, the padding and the scrollbar.
fn content_width(app: &App) -> f32 {
    app.window.width - crate::widgets::nav::WIDTH - 2.0 * SIDE_PADDING - SCROLLBAR_GAP
}

/// The room kept free for the scrollbar, so that it never covers the content.
const SCROLLBAR_GAP: f32 = 16.0;

/// How many steps of `count` fit side by side in `width`. Rows are filled evenly, so
/// that the last row is not much shorter than the others.
fn steps_per_row(width: f32, count: usize) -> usize {
    let fit = (((width + STEP_GAP) / (STEP_MIN_WIDTH + STEP_GAP)).floor() as usize).max(1);
    let rows = count.max(1).div_ceil(fit);
    count.max(1).div_ceil(rows)
}

/// The funny lines; one shows at a time, in turn.
const QUIPS: [&str; 8] = [
    "Marinating the verbs…",
    "Listening very carefully…",
    "Flipping the nouns…",
    "Adding a pinch of grammar…",
    "Basting the adjectives…",
    "Turning the skewer…",
    "Seasoning with commas…",
    "Letting the sentences rest…",
];

/// One step in the steps row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepView {
    pub step: Step,
    pub label: String,
}

/// The state of this screen.
#[derive(Debug)]
pub struct Convert {
    pub steps: Vec<StepView>,
    /// The index of the running step in `steps`.
    pub current: Option<usize>,
    done: u64,
    total: u64,
    /// The newest transcribed lines, oldest first.
    pub cues: VecDeque<Cue>,
    pub cancel: Arc<AtomicBool>,
    pub stopping: bool,
    pub error: Option<Failure>,
    pub show_details: bool,
    pub confirm_close: bool,
    /// Close the window when the cancelled job ends.
    pub close_after: bool,
    /// How many cues came so far, for the slide-in of the newest one.
    pub cue_count: usize,
    /// The video name and options, for the line under the title.
    pub summary: String,
    /// A message from the pipeline, for example "no translation needed".
    pub note: Option<String>,
    /// The film notes, while the user checks them.
    pub review: Option<text_editor::Content>,
    /// Sends the answers of the user back to the pipeline.
    pub reply: Option<std::sync::mpsc::Sender<crate::worker::Reply>>,
    /// A model and its licence, while the user decides.
    pub licence: Option<(String, String)>,
}

impl Convert {
    /// The steps that `job` will run, before any model download.
    pub fn for_job(job: &Job, summary: String) -> Self {
        let mut steps = vec![
            view(Step::Decode, "Read audio"),
            view(Step::Transcribe, "Transcribe"),
        ];
        if let Some(translation) = &job.translation {
            if translation.film_notes {
                steps.push(view(Step::Notes, "Read the story"));
            }
            steps.push(view(Step::Translate, "Translate"));
        }
        steps.extend(output_steps(job));
        Self {
            steps,
            current: None,
            done: 0,
            total: 0,
            cues: VecDeque::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            stopping: false,
            error: None,
            show_details: false,
            confirm_close: false,
            close_after: false,
            cue_count: 0,
            summary,
            note: None,
            review: None,
            reply: None,
            licence: None,
        }
    }

    /// The name of the running step, with the line number while it speaks.
    pub fn step_name(&self) -> String {
        let Some(step) = self.current.and_then(|i| self.steps.get(i)) else {
            return "Getting ready".to_string();
        };
        if step.step == Step::Speak && self.total > 0 {
            let line = (self.done + 1).min(self.total);
            return format!("Speaking line {line} of {}", self.total);
        }
        // The steps row says only "Download models"; the bar names the file.
        if let Step::Download(name) = step.step {
            return format!("Download {name}");
        }
        step.label.clone()
    }

    /// The part of the current step that is done, 0 to 1.
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }

    /// Applies a pipeline update other than `Finished`.
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Step(step) => {
                let index = match self.steps.iter().position(|s| s.step == step) {
                    Some(i) => i,
                    // Downloads one after the other share one step: a model of
                    // several files would else fill the row with file names.
                    None if matches!(step, Step::Download(_))
                        && self
                            .current
                            .is_some_and(|c| matches!(self.steps[c].step, Step::Download(_))) =>
                    {
                        let c = self.current.unwrap_or_default();
                        self.steps[c].step = step;
                        c
                    }
                    None => {
                        let at = self.current.map_or(0, |c| c + 1);
                        let label = match step {
                            Step::Download(_) => "Download models".to_string(),
                            other => format!("{other:?}"),
                        };
                        self.steps.insert(at, StepView { step, label });
                        at
                    }
                };
                self.current = Some(index);
                self.done = 0;
                self.total = 0;
            }
            Update::Progress { done, total } => {
                self.done = done;
                self.total = total;
            }
            Update::Cue(cue) => {
                if !cue.text.is_empty() {
                    self.cues.push_back(cue);
                    while self.cues.len() > 3 {
                        self.cues.pop_front();
                    }
                    self.cue_count += 1;
                }
            }
            Update::Note(note) => self.note = Some(note),
            Update::Review(_) | Update::Licence { .. } | Update::Finished(_) => {}
        }
    }

    /// The part of the whole job that is done, 0 to 1. Each step counts the same.
    pub fn overall(&self) -> f32 {
        let Some(current) = self.current else {
            return 0.0;
        };
        (current as f32 + self.progress()) / self.steps.len().max(1) as f32
    }

    /// How many verb chunks are cooked, and which one cooks now (4 chunks for the
    /// steps, whatever their number).
    fn chunks(&self) -> (usize, Option<usize>) {
        if self.current.is_none() {
            return (0, None);
        }
        let progress = self.overall();
        let cooked = ((progress * 4.0).floor() as usize).min(4);
        (cooked, (cooked < 4).then_some(cooked))
    }
}

/// The steps after the translation: burning, then the dub steps. A job with only
/// subtitle files has a save step instead.
fn output_steps(job: &Job) -> Vec<StepView> {
    let mut steps = Vec::new();
    if job.burn {
        steps.push(view(Step::Burn, "Burn in"));
    }
    #[cfg(feature = "dub")]
    if let Some(options) = &job.dub {
        if options.original == verbacue::dub::Original::Remove {
            steps.push(view(Step::Separate, "Separate voices"));
        }
        if matches!(
            options.voice,
            verbacue::dub::VoiceChoice::Clone | verbacue::dub::VoiceChoice::Cast
        ) {
            steps.push(view(Step::Speakers, "Find speakers"));
        }
        steps.push(view(Step::Speak, "Speak"));
        steps.push(view(Step::Mix, "Mix"));
    }
    if steps.is_empty() {
        steps.push(view(Step::Save, "Save"));
    }
    steps
}

fn view(step: Step, label: &str) -> StepView {
    StepView {
        step,
        label: label.to_string(),
    }
}

/// "0:12:34" for a time in ms.
fn timestamp(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

pub fn view_screen(app: &App) -> Element<'_, Message> {
    let c = &app.convert;
    let t = app.seconds();
    if let Some(error) = &c.error {
        return error_card(c, error);
    }
    if let Some(content) = &c.review {
        return review_card(c, content);
    }
    if let Some((model, licence)) = &c.licence {
        return licence_card(c, model, licence);
    }

    // The quip changes every 3.2 s and fades over 0.4 s at both ends.
    let index = (t / 3.2) as usize % QUIPS.len();
    let phase = t % 3.2;
    let alpha = (phase / 0.4).min((3.2 - phase) / 0.4).clamp(0.0, 1.0);
    let title = if c.stopping {
        "Stopping…"
    } else {
        QUIPS[index]
    };
    let header = column![
        text(title).font(theme::HEADING).size(38).color(Color {
            a: if c.stopping { 1.0 } else { alpha },
            ..theme::TEXT
        }),
        text(match &c.note {
            Some(note) => format!("{} · {note}", c.summary),
            None => c.summary.clone(),
        })
        .color(theme::MUTED),
    ]
    .spacing(8)
    .align_x(Alignment::Center);

    let (cooked, cooking) = c.chunks();
    let skewer = canvas(Skewer {
        t,
        cooked,
        cooking: if c.stopping { None } else { cooking },
    })
    .width(Length::Fixed(560.0))
    .height(Length::Fixed(250.0));

    let step_name = c.step_name();
    let percent = (c.progress() * 100.0).round() as u32;
    let bar = canvas(Bar {
        t,
        progress: c.progress(),
    })
    .width(Length::Fill)
    .height(14);
    // The steps fill the width: as many side by side as fit, each at least
    // STEP_MIN_WIDTH wide, so that a label never gets narrower than its longest word.
    // (iced's `responsive` widget would measure the width itself, but it stops the
    // text next to it from drawing.)
    let cells: Vec<(String, Color, Color)> = c
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (mark, color, background) = match c.current {
                Some(cur) if i < cur => ("✓", theme::SUCCESS_TEXT, Color::TRANSPARENT),
                Some(cur) if i == cur => (
                    "●",
                    theme::ACCENT_TEXT,
                    Color::from_rgba8(0xe8, 0x62, 0x2a, 0.16),
                ),
                _ => ("○", theme::FAINT, Color::TRANSPARENT),
            };
            (format!("{mark} {}", s.label), color, background)
        })
        .collect();
    let per_row = steps_per_row(content_width(app), cells.len());
    let steps = column(cells.chunks(per_row).map(|line| {
        let mut items: Vec<Element<'_, Message>> = line
            .iter()
            .map(|(label, color, background)| {
                let background = *background;
                container(text(label.clone()).size(13).color(*color))
                    .padding([8, 10])
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(background.into()),
                        border: Border {
                            radius: 10.0.into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    })
                    .into()
            })
            .collect();
        // The last row keeps the column widths of the rows above it.
        while items.len() < per_row {
            items.push(space().width(Length::Fill).into());
        }
        row(items).spacing(STEP_GAP).into()
    }))
    .spacing(STEP_GAP);
    let progress = column![
        row![
            text(step_name).size(14).color(theme::TEXT_SOFT),
            space().width(Length::Fill),
            text(format!("{percent}%")).size(14).color(theme::TEXT_SOFT),
        ],
        bar,
        steps,
    ]
    .spacing(10)
    .width(Length::Fill);

    // The newest lines; the last one slides in.
    let live = column![
        row![
            container(text(""))
                .width(8)
                .height(8)
                .style(move |_| container::Style {
                    background: Some(
                        Color {
                            a: 0.35 + 0.65 * wave(t, 1.2, 0.0),
                            ..theme::ACCENT
                        }
                        .into()
                    ),
                    border: Border {
                        radius: 4.0.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }),
            text("FRESH OFF THE GRILL")
                .size(12)
                .font(theme::BODY_BOLD)
                .color(theme::FAINT),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    ]
    .push(
        column(c.cues.iter().map(|cue| {
            text(format!("{} · {}", timestamp(cue.start_ms), cue.text))
                .size(14)
                .color(theme::TEXT_SOFT)
                .into()
        }))
        .spacing(6),
    )
    .spacing(8);
    let live = container(live)
        .padding([16, 18])
        .width(Length::Fill)
        .style(theme::card(18.0));

    let mut actions = row![].spacing(12);
    if c.confirm_close {
        actions = actions
            .push(text("Stop the conversion and quit?").color(theme::TEXT))
            .push(
                button(text("Stop and quit"))
                    .padding([12, 20])
                    .style(theme::primary)
                    .on_press(Message::ConfirmClose(true)),
            )
            .push(
                button(text("Keep grilling"))
                    .padding([12, 20])
                    .style(theme::ghost)
                    .on_press(Message::ConfirmClose(false)),
            );
    } else if !c.stopping {
        actions = actions.push(
            button(text("Cancel"))
                .padding([12, 20])
                .style(theme::ghost)
                .on_press(Message::Cancel),
        );
    }

    let content = column![
        header,
        skewer,
        progress,
        live,
        actions.align_y(Alignment::Center),
        queue_panel(app),
    ]
    .spacing(26)
    .width(Length::Fill)
    .align_x(Alignment::Center);
    container(scrollable(content).spacing(SCROLLBAR_GAP - 6.0))
        .padding([40.0, SIDE_PADDING])
        .width(Length::Fill)
        .into()
}

/// The queue during a run: each video with its state, "Add videos", and × for the
/// waiting ones.
fn queue_panel(app: &App) -> Element<'_, Message> {
    let (position, total) = app.run_position();
    let rows = app
        .queue
        .iter()
        .enumerate()
        .map(|(i, item)| -> Element<'_, Message> {
            let color = match item.status {
                Status::Running | Status::Failed(_) => theme::ACCENT_TEXT,
                Status::Done(..) => theme::SUCCESS_TEXT,
                Status::Waiting => theme::TEXT_SOFT,
            };
            let mut line = row![
                text(item.name())
                    .size(14)
                    .color(theme::TEXT)
                    .width(Length::Fill),
                text(item.status.label()).size(13).color(color),
            ]
            .spacing(12)
            .align_y(Alignment::Center);
            if item.status == Status::Waiting {
                line = line.push(
                    button(text("×").size(14).color(theme::MUTED))
                        .padding([2, 8])
                        .style(|_, _| button::Style::default())
                        .on_press(Message::Remove(i)),
                );
            }
            line.into()
        });
    let title = if total > 1 {
        format!("QUEUE · VIDEO {position} OF {total}")
    } else {
        "QUEUE".to_string()
    };
    container(
        column![
            row![
                text(title)
                    .size(12)
                    .font(theme::BODY_BOLD)
                    .color(theme::FAINT)
                    .width(Length::Fill),
                button(text("+ Add videos").size(13))
                    .padding([6, 12])
                    .style(theme::ghost)
                    .on_press(Message::PickFile),
            ]
            .align_y(Alignment::Center),
            column(rows).spacing(6),
        ]
        .spacing(10),
    )
    .padding([14, 18])
    .width(Length::Fill)
    .style(theme::card(18.0))
    .into()
}

/// The film notes in an editor; the translation waits until the user continues.
fn review_card<'a>(c: &'a Convert, content: &'a text_editor::Content) -> Element<'a, Message> {
    let card = column![
        text("Check the notes")
            .font(theme::HEADING)
            .size(30)
            .color(theme::TEXT),
        text(&c.summary).color(theme::MUTED),
        text(
            "verbacue read the whole story and wrote these notes for the translator. \
             Correct names, relations and who speaks formally to whom, then continue."
        )
        .size(14)
        .color(theme::TEXT_SOFT),
        text_editor(content)
            .on_action(Message::ReviewEdit)
            .height(220)
            .padding(14),
        row![
            button(text("Translate with these notes →").font(theme::BODY_BOLD))
                .padding([12, 22])
                .style(theme::primary)
                .on_press(Message::ReviewDone),
            button(text("Cancel"))
                .padding([12, 22])
                .style(theme::ghost)
                .on_press(Message::Cancel),
        ]
        .spacing(12),
    ]
    .spacing(14);
    container(
        container(card)
            .padding(28)
            .max_width(720)
            .style(theme::card(22.0)),
    )
    .padding([60, 56])
    .center_x(Length::Fill)
    .into()
}

/// The licence of a voice model; the download waits until the user decides.
fn licence_card<'a>(c: &'a Convert, model: &'a str, licence: &'a str) -> Element<'a, Message> {
    let card = column![
        text(format!("Accept the {model} licence?"))
            .font(theme::HEADING)
            .size(30)
            .color(theme::TEXT),
        text(&c.summary).color(theme::MUTED),
        text(
            "Dubbing needs this voice model. Its licence allows non-commercial use only. \
             verbacue downloads the model after you accept."
        )
        .size(14)
        .color(theme::TEXT_SOFT),
        container(text(licence).size(13).color(theme::MUTED))
            .padding(14)
            .width(Length::Fill)
            .style(theme::card(12.0)),
        row![
            button(text("Accept and download").font(theme::BODY_BOLD))
                .padding([12, 22])
                .style(theme::primary)
                .on_press(Message::Licence(true)),
            button(text("Cancel"))
                .padding([12, 22])
                .style(theme::ghost)
                .on_press(Message::Licence(false)),
        ]
        .spacing(12),
    ]
    .spacing(14);
    container(
        container(card)
            .padding(28)
            .max_width(720)
            .style(theme::card(22.0)),
    )
    .padding([60, 56])
    .center_x(Length::Fill)
    .into()
}

fn error_card<'a>(c: &'a Convert, error: &'a Failure) -> Element<'a, Message> {
    let mut card = column![
        text("The grill went out")
            .font(theme::HEADING)
            .size(30)
            .color(theme::TEXT),
        text(friendly(error.kind)).size(16).color(theme::TEXT_SOFT),
        row![
            button(text("Try again").font(theme::BODY_BOLD))
                .padding([12, 22])
                .style(theme::primary)
                .on_press(Message::Start),
            button(text("Back"))
                .padding([12, 22])
                .style(theme::ghost)
                .on_press(Message::Show(crate::app::Screen::Choose)),
            button(
                text(if c.show_details {
                    "Hide details"
                } else {
                    "Details"
                })
                .color(theme::MUTED)
            )
            .padding([12, 12])
            .style(|_, _| button::Style::default())
            .on_press(Message::ToggleDetails),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    ]
    .spacing(14);
    if c.show_details {
        card = card.push(
            container(
                text(&error.message)
                    .size(13)
                    .font(iced::Font::MONOSPACE)
                    .color(theme::MUTED),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::card(12.0)),
        );
    }
    container(
        container(card)
            .padding(28)
            .max_width(640)
            .style(theme::card(22.0)),
    )
    .padding([80, 56])
    .center_x(Length::Fill)
    .into()
}

/// The progress bar with a moving shimmer.
struct Bar {
    t: f32,
    progress: f32,
}

impl canvas::Program<Message> for Bar {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let h = bounds.height;
        frame.fill(
            &Path::rounded_rectangle(Point::ORIGIN, bounds.size(), (h / 2.0).into()),
            Color::from_rgba8(0xf3, 0xe9, 0xdc, 0.08),
        );
        let width = (bounds.width * self.progress).max(if self.progress > 0.0 { h } else { 0.0 });
        if width > 0.0 {
            // The gradient repeats every 2 bar widths and moves one width in 2 s.
            let shift = bounds.width * 2.0 * cycle(self.t, 2.0, 0.0);
            let fill = Linear::new(
                Point::new(-shift, 0.0),
                Point::new(bounds.width * 2.0 - shift, 0.0),
            )
            .add_stop(0.0, theme::ACCENT)
            .add_stop(0.25, theme::FLAME_MID)
            .add_stop(0.5, theme::ACCENT)
            .add_stop(0.75, theme::FLAME_MID)
            .add_stop(1.0, theme::ACCENT);
            frame.fill(
                &Path::rounded_rectangle(Point::ORIGIN, Size::new(width, h), (h / 2.0).into()),
                fill,
            );
        }
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn labels(c: &Convert) -> Vec<&str> {
        c.steps.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn downloads_in_a_row_share_one_step() {
        let mut c = Convert::for_job(&Job::new(PathBuf::new(), "hu"), String::new());
        c.apply(Update::Step(Step::Download("xtts-v2/conditioning.onnx")));
        c.apply(Update::Step(Step::Download("xtts-v2/speaker.onnx")));
        assert_eq!(c.step_name(), "Download xtts-v2/speaker.onnx");
        c.apply(Update::Step(Step::Download("xtts-v2/gpt.onnx")));
        assert_eq!(c.step_name(), "Download xtts-v2/gpt.onnx");
        assert_eq!(labels(&c)[0], "Download models");
        assert_eq!(
            labels(&c)
                .iter()
                .filter(|l| l.starts_with("Download"))
                .count(),
            1
        );
        c.apply(Update::Step(Step::Decode));
        assert_eq!(c.step_name(), "Read audio");
        assert_eq!(c.current, Some(1));
    }

    #[test]
    fn steps_fill_the_rows_evenly() {
        // A narrow window: 2 fit, 5 steps make rows of 2, 2 and 1.
        assert_eq!(steps_per_row(250.0, 5), 2);
        // 640 px fit 5; 10 steps make two rows of 5.
        assert_eq!(steps_per_row(640.0, 10), 5);
        // 640 px fit 5; 6 steps make two rows of 3, not 5 and 1.
        assert_eq!(steps_per_row(640.0, 6), 3);
        // A wide window shows all steps in one row.
        assert_eq!(steps_per_row(1600.0, 10), 10);
        // Never zero, even without room or steps.
        assert_eq!(steps_per_row(10.0, 3), 1);
        assert_eq!(steps_per_row(640.0, 0), 1);
    }

    #[test]
    fn a_later_download_gets_its_own_step() {
        let mut c = Convert::for_job(&Job::new(PathBuf::new(), "hu"), String::new());
        c.apply(Update::Step(Step::Download("large-v3")));
        c.apply(Update::Step(Step::Decode));
        c.apply(Update::Step(Step::Transcribe));
        c.apply(Update::Step(Step::Download("translategemma-12b")));
        assert_eq!(c.step_name(), "Download translategemma-12b");
        assert_eq!(
            labels(&c)[..4],
            [
                "Download models",
                "Read audio",
                "Transcribe",
                "Download models"
            ]
        );
    }
}
