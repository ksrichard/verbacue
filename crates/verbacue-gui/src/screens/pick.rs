//! Pick a video: the animated drop zone, the file button and the recent files.

use std::path::PathBuf;

use iced::{
    Alignment, Border, Color, Element, Length, Point, Rectangle, Renderer, Shadow, Size, Theme,
    Vector, mouse,
    widget::{
        button, canvas,
        canvas::{Frame, Geometry, LineDash, Path, Stroke},
        column, container, row, stack, text,
    },
};

use crate::{
    anim::{grill::Grill, wave},
    app::{App, Message},
    theme,
};

/// The state of this screen.
#[derive(Debug, Default)]
pub struct Pick {
    /// The file whose facts are being read.
    pub checking: Option<PathBuf>,
    /// Why the last file cannot be used.
    pub error: Option<String>,
    /// A file is dragged over the window.
    pub hovering: bool,
}

pub fn view(app: &App) -> Element<'_, Message> {
    let t = app.seconds();
    let header = column![
        text("What are we grilling today?")
            .font(theme::HEADING)
            .size(46)
            .color(theme::TEXT),
        text("Drop a video. verbacue listens, translates and serves the subtitles.")
            .size(17)
            .color(theme::MUTED),
    ]
    .spacing(10);

    let (title, detail): (String, String) = match (&app.pick.checking, &app.pick.error) {
        (Some(path), _) => (
            "Lighting the grill…".into(),
            path.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        ),
        (None, Some(error)) => ("This one will not cook".into(), error.clone()),
        (None, None) => (
            "Drop a video on the grill".into(),
            "MKV, MP4, AVI, WebM and anything else FFmpeg can open".into(),
        ),
    };
    let detail_color = if app.pick.error.is_some() && app.pick.checking.is_none() {
        theme::ACCENT_TEXT
    } else {
        theme::MUTED
    };
    let inner = column![
        canvas(Grill { t }).width(180).height(150),
        column![
            text(title).font(theme::HEADING).size(24).color(theme::TEXT),
            text(detail).size(15).color(detail_color),
        ]
        .spacing(8)
        .align_x(Alignment::Center),
        button(
            row![
                text("+").size(20).font(theme::BODY_BOLD),
                text("Choose a video").font(theme::BODY_BOLD).size(16)
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([14, 26])
        .style(theme::primary)
        .on_press(Message::PickFile),
    ]
    .spacing(22)
    .align_x(Alignment::Center);

    let pulse = wave(t, 3.0, 0.0);
    let hovering = app.pick.hovering;
    let zone = stack![
        canvas(DashedBorder {
            alpha: if hovering { 1.0 } else { 0.55 },
        })
        .width(Length::Fill)
        .height(Length::Fill),
        container(inner).center(Length::Fill).padding(24),
    ];
    let zone = container(zone)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(
                iced::gradient::Linear::new(std::f32::consts::PI)
                    .add_stop(
                        0.0,
                        Color::from_rgba8(0x2b, 0x1a, 0x14, if hovering { 0.95 } else { 0.7 }),
                    )
                    .add_stop(1.0, Color::from_rgba8(0x17, 0x12, 0x11, 0.7))
                    .into(),
            ),
            border: Border {
                radius: 28.0.into(),
                ..Border::default()
            },
            shadow: Shadow {
                color: Color::from_rgba8(0xe8, 0x62, 0x2a, 0.3 + 0.25 * pulse),
                offset: Vector::new(0.0, 30.0),
                blur_radius: 80.0 + 10.0 * pulse,
            },
            ..container::Style::default()
        });

    let mut page = column![header, zone].spacing(32).height(Length::Fill);
    if !app.settings.recent.is_empty() {
        page = page.push(recent(app));
    }
    container(page)
        .padding([48, 64])
        .height(Length::Fill)
        .into()
}

fn recent(app: &App) -> Element<'_, Message> {
    let cards = app.settings.recent.iter().enumerate().map(|(i, r)| {
        let name = r
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let mode = if r.burn { "burned in" } else { "SRT file" };
        let detail = format!("{} · {mode}", verbacue::translate::language_name(&r.target));
        button(
            column![
                text(name)
                    .font(theme::BODY_BOLD)
                    .size(15)
                    .color(theme::TEXT),
                text(detail).size(13).color(theme::MUTED),
            ]
            .spacing(4),
        )
        .padding([14, 16])
        .width(240)
        .style(theme::recent_card)
        .on_press(Message::OpenRecent(i))
        .into()
    });
    column![
        text("RECENT")
            .font(theme::BODY_BOLD)
            .size(13)
            .color(theme::FAINT),
        row(cards).spacing(12).wrap(),
    ]
    .spacing(12)
    .into()
}

/// The dashed orange outline of the drop zone (iced borders cannot be dashed).
struct DashedBorder {
    alpha: f32,
}

impl canvas::Program<Message> for DashedBorder {
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
        let outline = Path::rounded_rectangle(
            Point::new(1.0, 1.0),
            Size::new(bounds.width - 2.0, bounds.height - 2.0),
            28.0.into(),
        );
        frame.stroke(
            &outline,
            Stroke {
                line_dash: LineDash {
                    segments: &[9.0, 7.0],
                    offset: 0,
                },
                ..Stroke::default().with_width(2.0).with_color(Color {
                    a: self.alpha,
                    ..theme::ACCENT
                })
            },
        );
        vec![frame.into_geometry()]
    }
}
