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
        column, container, row, text,
    },
};
use verbacue::{
    pipeline::{Job, Step},
    subtitle::Cue,
};

use crate::{
    anim::{cycle, skewer::Skewer, wave},
    app::{App, Message},
    theme,
    worker::{Failure, Update, friendly},
};

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
}

impl Convert {
    /// The steps that `job` will run, before any model download.
    pub fn for_job(job: &Job, summary: String) -> Self {
        let mut steps = vec![
            view(Step::Decode, "Read audio"),
            view(Step::Transcribe, "Transcribe"),
        ];
        if job.translation.is_some() {
            steps.push(view(Step::Translate, "Translate"));
        }
        steps.push(if job.burn {
            view(Step::Burn, "Burn in")
        } else {
            view(Step::Save, "Save")
        });
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
        }
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
                    None => {
                        let at = self.current.map_or(0, |c| c + 1);
                        let label = match step {
                            Step::Download(name) => format!("Download {name}"),
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
            Update::Finished(_) => {}
        }
    }

    /// How many verb chunks are cooked, and which one cooks now (4 chunks for the
    /// steps, whatever their number).
    fn chunks(&self) -> (usize, Option<usize>) {
        let Some(current) = self.current else {
            return (0, None);
        };
        let n = self.steps.len().max(1);
        let progress = (current as f32 + self.progress()) / n as f32;
        let cooked = ((progress * 4.0).floor() as usize).min(4);
        (cooked, (cooked < 4).then_some(cooked))
    }
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

    let step_name = c
        .current
        .and_then(|i| c.steps.get(i))
        .map_or("Getting ready", |s| s.label.as_str());
    let percent = (c.progress() * 100.0).round() as u32;
    let bar = canvas(Bar {
        t,
        progress: c.progress(),
    })
    .width(Length::Fill)
    .height(14);
    let steps = row(c.steps.iter().enumerate().map(|(i, s)| {
        let (mark, color, background) = match c.current {
            Some(cur) if i < cur => ("✓", theme::SUCCESS_TEXT, Color::TRANSPARENT),
            Some(cur) if i == cur => (
                "●",
                theme::ACCENT_TEXT,
                Color::from_rgba8(0xe8, 0x62, 0x2a, 0.16),
            ),
            _ => ("○", theme::FAINT, Color::TRANSPARENT),
        };
        container(text(format!("{mark} {}", s.label)).size(13).color(color))
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
    }))
    .spacing(8);
    let progress = column![
        row![
            text(step_name)
                .size(14)
                .color(theme::TEXT_SOFT)
                .width(Length::Fill),
            text(format!("{percent}%")).size(14).color(theme::TEXT_SOFT),
        ],
        bar,
        steps,
    ]
    .spacing(10)
    .max_width(640);

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
        .max_width(640)
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

    container(
        column![
            header,
            skewer,
            progress,
            live,
            actions.align_y(Alignment::Center)
        ]
        .spacing(26)
        .align_x(Alignment::Center),
    )
    .padding([40, 56])
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
