//! Colors, fonts and widget styles from the approved mockups.
#![expect(dead_code, reason = "the screens of the later tasks use the rest")]

use iced::{
    Background, Border, Color, Font, Shadow, Vector,
    font::{Family, Weight},
    widget::{button, container},
};

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn rgba(hex: u32, alpha: f32) -> Color {
    Color::from_rgba8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, alpha)
}

pub const BG: Color = rgb(0x120e0d);
pub const GLOW: Color = rgb(0x2b1a14);
pub const CARD: Color = rgba(0xf3e9dc, 0.04);
pub const CARD_HOVER: Color = rgba(0xf3e9dc, 0.06);
pub const LINE: Color = rgba(0xf3e9dc, 0.07);
pub const TEXT: Color = rgb(0xf3e9dc);
pub const TEXT_SOFT: Color = rgb(0xcdbfb2);
pub const MUTED: Color = rgb(0xa89a8d);
pub const FAINT: Color = rgb(0x8f8277);
pub const ACCENT: Color = rgb(0xe8622a);
pub const ACCENT_TEXT: Color = rgb(0xffb38a);
pub const ON_ACCENT: Color = rgb(0x1a0f0a);
pub const MEAT: Color = rgb(0x7a3b26);
pub const PEPPER: Color = rgb(0x3f8f3a);
pub const ONION: Color = rgb(0xf2c14e);
pub const RED_PEPPER: Color = rgb(0xc8402f);
pub const FLAME_LOW: Color = rgb(0xff7a1a);
pub const FLAME_MID: Color = rgb(0xffb627);
pub const FLAME_HIGH: Color = rgb(0xffe680);
pub const GRATE: Color = rgb(0x5b524d);
pub const STEEL: Color = rgb(0xc9c2b8);
pub const BOWL_TOP: Color = rgb(0xd8432e);
pub const BOWL_BOTTOM: Color = rgb(0x8e2417);
pub const SUCCESS: Color = rgb(0x5dbb55);
pub const SUCCESS_TEXT: Color = rgb(0xbfe3b5);

/// Space Grotesk, for headings and the logo.
pub const HEADING: Font = Font {
    family: Family::Name("Space Grotesk"),
    weight: Weight::Bold,
    ..Font::DEFAULT
};

/// Inter, for all other text.
pub const BODY: Font = Font {
    family: Family::Name("Inter"),
    ..Font::DEFAULT
};

pub const BODY_MEDIUM: Font = Font {
    weight: Weight::Medium,
    ..BODY
};

pub const BODY_BOLD: Font = Font {
    weight: Weight::Semibold,
    ..BODY
};

/// The font files the app loads at start.
pub const FONTS: [&[u8]; 5] = [
    include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
];

fn radius(r: f32) -> Border {
    Border {
        radius: r.into(),
        ..Border::default()
    }
}

/// A side navigation link. `active` marks the current page.
pub fn nav_link(active: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (background, text_color) = match (active, hovered) {
            (true, _) => (Some(rgba(0xe8622a, 0.14).into()), ACCENT_TEXT),
            (false, true) => (Some(CARD_HOVER.into()), TEXT),
            (false, false) => (None, TEXT_SOFT),
        };
        button::Style {
            background,
            text_color,
            border: radius(12.0),
            ..button::Style::default()
        }
    }
}

/// The orange main button.
pub fn primary(_: &iced::Theme, status: button::Status) -> button::Style {
    let lift = matches!(status, button::Status::Hovered);
    button::Style {
        background: Some(Background::Color(ACCENT)),
        text_color: ON_ACCENT,
        border: radius(14.0),
        shadow: Shadow {
            color: rgba(0xe8622a, if lift { 0.45 } else { 0.25 }),
            offset: Vector::new(0.0, if lift { 10.0 } else { 6.0 }),
            blur_radius: 24.0,
        },
        snap: true,
    }
}

/// A button with a thin outline.
pub fn ghost(_: &iced::Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: None,
        text_color: if hovered { TEXT } else { TEXT_SOFT },
        border: Border {
            color: rgba(0xf3e9dc, if hovered { 0.35 } else { 0.18 }),
            width: 1.0,
            radius: 14.0.into(),
        },
        ..button::Style::default()
    }
}

/// A recent file on the start screen.
pub fn recent_card(_: &iced::Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(if hovered { CARD_HOVER } else { CARD }.into()),
        text_color: TEXT,
        border: radius(16.0),
        ..button::Style::default()
    }
}

/// A drop-down list on the dark background.
pub fn pick(
    _: &iced::Theme,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    let open = !matches!(status, iced::widget::pick_list::Status::Active);
    iced::widget::pick_list::Style {
        text_color: TEXT,
        placeholder_color: MUTED,
        handle_color: if open { ACCENT_TEXT } else { MUTED },
        background: rgb(0x1f1714).into(),
        border: Border {
            color: rgba(0xf3e9dc, if open { 0.3 } else { 0.14 }),
            width: 1.0,
            radius: 12.0.into(),
        },
    }
}

/// The app theme: the mockup colors, so menus and lists match.
pub fn app_theme() -> iced::Theme {
    iced::Theme::custom(
        "verbacue",
        iced::theme::Palette {
            background: rgb(0x1f1714),
            text: TEXT,
            primary: ACCENT,
            success: SUCCESS,
            warning: ONION,
            danger: RED_PEPPER,
        },
    )
}

/// A rounded card on the dark background.
pub fn card(radius_px: f32) -> impl Fn(&iced::Theme) -> container::Style {
    move |_| container::Style {
        background: Some(CARD.into()),
        border: radius(radius_px),
        ..container::Style::default()
    }
}

/// The thin line between the navigation and the page.
pub fn divider(_: &iced::Theme) -> iced::widget::rule::Style {
    iced::widget::rule::Style {
        color: LINE,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
        snap: true,
    }
}
