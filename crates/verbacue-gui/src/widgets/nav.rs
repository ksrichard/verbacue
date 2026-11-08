//! The side navigation: logo, "New subtitles", "Settings" and the info card.

use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, image, rich_text, row, span, text},
};

use crate::{app::Message, app::Screen, theme};

const LOGO: &[u8] = include_bytes!("../../assets/icon.png");

/// The navigation for `screen`. `models` is (downloaded, needed) for the info card.
pub fn view<'a>(screen: Screen, models: (usize, usize)) -> Element<'a, Message> {
    let logo = row![
        image(image::Handle::from_bytes(LOGO)).width(34).height(34),
        rich_text![
            span::<(), _>("verba").color(theme::TEXT),
            span("cue").color(theme::ACCENT),
        ]
        .font(theme::HEADING)
        .size(22),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .padding([0, 10]);

    let new_active = matches!(
        screen,
        Screen::Pick | Screen::Choose | Screen::Convert | Screen::Done
    );
    let link = |label: &'a str, active: bool, target: Screen| {
        button(text(label).font(theme::BODY_MEDIUM).size(15))
            .padding([10, 14])
            .width(Length::Fill)
            .style(theme::nav_link(active))
            .on_press(Message::Show(target))
    };

    let info = container(
        text(format!(
            "Everything runs on this computer. Models: {} of {} downloaded.",
            models.0, models.1
        ))
        .font(theme::BODY)
        .size(13)
        .color(theme::MUTED)
        .line_height(1.5),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::card(14.0));

    container(
        column![
            logo,
            container(text("")).height(16),
            link("New subtitles", new_active, Screen::Pick),
            link("Settings", screen == Screen::Settings, Screen::Settings),
            container(text("")).height(Length::Fill),
            info,
        ]
        .spacing(6),
    )
    .padding([28, 18])
    .width(240)
    .height(Length::Fill)
    .into()
}
