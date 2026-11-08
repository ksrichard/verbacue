//! Done: the check mark, confetti, the written files, and what to do next.

use std::path::{Path, PathBuf};

use iced::{
    Alignment, Color, Element, Length,
    widget::{button, canvas, column, container, row, scrollable, stack, text},
};
use verbacue::pipeline::Outcome;

use crate::{
    anim::confetti::{Check, Confetti},
    app::{App, KEEP, Message},
    queue::{Item, Status},
    theme,
    worker::friendly,
};

/// The file names of the written files, in the order the pipeline wrote them.
pub fn file_names(outcome: &Outcome) -> Vec<String> {
    outcome.files.iter().map(|p| name(p)).collect()
}

fn name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// The file to play: the video, else the last subtitle file.
pub fn main_file(outcome: &Outcome) -> Option<&PathBuf> {
    outcome
        .files
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "mp4" || e == "mkv"))
        .or_else(|| outcome.files.last())
}

/// How many cues a subtitle file has (the lines with a time range).
pub fn count_cues(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map_or(0, |text| text.lines().filter(|l| l.contains("-->")).count())
}

fn fade(t: f32, delay: f32) -> f32 {
    ((t - delay) / 0.6).clamp(0.0, 1.0)
}

pub fn view(app: &App) -> Element<'_, Message> {
    let items = app.run_items();
    let t = app.seconds();
    let a1 = fade(t, 0.2);
    let a2 = fade(t, 0.35);
    let done = items
        .iter()
        .filter(|(_, item)| item.outcome().is_some())
        .count();

    let subtitle = match (items.as_slice(), done) {
        ([(_, item)], _) => format!(
            "{} subtitle lines in {}, ready to watch.",
            item.lines(),
            language(app, item)
        ),
        (all, n) if n == all.len() => format!("{n} videos, ready to watch."),
        (all, n) => format!("{n} of {} videos are ready.", all.len()),
    };
    let heading = column![
        text("Served hot!")
            .font(theme::HEADING)
            .size(44)
            .color(theme::TEXT),
        text(subtitle).color(Color {
            a: a1,
            ..theme::MUTED
        }),
    ]
    .spacing(8)
    .align_x(Alignment::Center);

    let cards = column(items.iter().map(|(index, item)| card(*index, item, a1, a2)))
        .spacing(12)
        .max_width(620);

    let again = button(text("Grill another one").font(theme::BODY_BOLD))
        .padding([14, 24])
        .style(theme::ghost)
        .on_press(Message::Again);

    let content = column![
        canvas(Check { t }).width(150).height(150),
        heading,
        scrollable(cards).height(Length::Shrink),
        again
    ]
    .spacing(26)
    .align_x(Alignment::Center);
    stack![
        canvas(Confetti { t })
            .width(Length::Fill)
            .height(Length::Fill),
        container(content).padding([56, 56]).center_x(Length::Fill),
    ]
    .into()
}

/// The language name of a finished video.
fn language(app: &App, item: &Item) -> String {
    let _ = app;
    let code = if item.selection.target == KEEP {
        item.outcome()
            .map_or_else(String::new, |o| o.source_language.clone())
    } else {
        item.selection.target.clone()
    };
    verbacue::translate::language_name(&code).to_string()
}

/// One video: its files and buttons, or its error.
fn card<'a>(index: usize, item: &'a Item, a1: f32, a2: f32) -> Element<'a, Message> {
    let body: Element<'a, Message> = match &item.status {
        Status::Done(outcome, _) => {
            let files = column(file_names(outcome).into_iter().map(|n| {
                text(n)
                    .size(14)
                    .color(Color {
                        a: a1,
                        ..theme::TEXT_SOFT
                    })
                    .into()
            }))
            .spacing(4);
            let button_with = move |label: &'static str, message: Message, primary: bool| {
                button(text(label).font(theme::BODY_BOLD).size(14))
                    .padding([10, 18])
                    .style(move |theme, status| {
                        let mut style = if primary {
                            theme::primary(theme, status)
                        } else {
                            theme::ghost(theme, status)
                        };
                        style.text_color.a = a2;
                        style
                    })
                    .on_press(message)
            };
            column![
                text(main_file(outcome).map_or_else(String::new, |p| name(p)))
                    .font(theme::BODY_BOLD)
                    .color(Color {
                        a: a1,
                        ..theme::TEXT
                    }),
                files,
                row![
                    button_with("Play video", Message::OpenFile(index), true),
                    button_with("Show in folder", Message::ShowInFolder(index), false),
                ]
                .spacing(10),
            ]
            .spacing(10)
            .into()
        }
        Status::Failed(failure) => column![
            text(item.name()).font(theme::BODY_BOLD).color(theme::TEXT),
            text(friendly(failure.kind))
                .size(14)
                .color(theme::ACCENT_TEXT),
        ]
        .spacing(6)
        .into(),
        Status::Waiting | Status::Running => text(item.name()).color(theme::MUTED).into(),
    };
    container(body)
        .padding(18)
        .width(Length::Fill)
        .style(theme::card(20.0))
        .into()
}
