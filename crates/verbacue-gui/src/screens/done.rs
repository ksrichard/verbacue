//! Done: the check mark, confetti, the written files, and what to do next.

use std::path::{Path, PathBuf};

use iced::{
    Alignment, Color, Element, Length,
    widget::{button, canvas, column, container, row, stack, text},
};
use verbacue::pipeline::Outcome;

use crate::{
    anim::confetti::{Check, Confetti},
    app::{App, KEEP, Message},
    theme,
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
    let Some(outcome) = &app.outcome else {
        return text("").into();
    };
    let t = app.seconds();
    let language = app.selection.as_ref().map_or_else(String::new, |s| {
        if s.target == KEEP {
            verbacue::translate::language_name(&outcome.source_language).to_string()
        } else {
            verbacue::translate::language_name(&s.target).to_string()
        }
    });
    let a1 = fade(t, 0.2);
    let a2 = fade(t, 0.35);

    let heading = column![
        text("Served hot!")
            .font(theme::HEADING)
            .size(44)
            .color(theme::TEXT),
        text(format!(
            "{} subtitle lines in {language}, ready to watch.",
            app.done_lines
        ))
        .color(Color {
            a: a1,
            ..theme::MUTED
        }),
    ]
    .spacing(8)
    .align_x(Alignment::Center);

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
    let card = container(
        column![
            text(main_file(outcome).map_or_else(String::new, |p| name(p)))
                .font(theme::BODY_BOLD)
                .color(Color {
                    a: a1,
                    ..theme::TEXT
                }),
            files,
        ]
        .spacing(8),
    )
    .padding(18)
    .width(Length::Fill)
    .max_width(620)
    .style(theme::card(20.0));

    let alpha_button = move |label: &'static str, message: Message, primary: bool| {
        button(text(label).font(theme::BODY_BOLD))
            .padding([14, 24])
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
    let actions = row![
        alpha_button("Play video", Message::OpenFile, true),
        alpha_button("Show in folder", Message::ShowInFolder, false),
        alpha_button("Grill another one", Message::Again, false),
    ]
    .spacing(12);

    let content = column![
        canvas(Check { t }).width(150).height(150),
        heading,
        card,
        actions
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
