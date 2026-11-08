//! Preview and choose: a preview of the video, the output cards, the language and the
//! audio track, and "Start grilling".

use std::time::Instant;

use iced::{
    Alignment, Border, Color, Element, Length, Point, Rectangle, Renderer, Shadow, Size, Theme,
    Vector, mouse,
    widget::{
        button, canvas,
        canvas::{Frame, Geometry, Path, gradient::Linear},
        column, container, image, pick_list, row, stack, svg, text,
    },
};
use verbacue::audio::AudioTrack;

use crate::{
    anim::{cycle, wave},
    app::{App, KEEP, Message},
    theme,
};

const FLAME_ICON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="30" viewBox="0 0 24 24" fill="none" stroke="#ff8a4c" stroke-width="2"><path d="M12 3c2 4 6 5 6 10a6 6 0 0 1-12 0c0-3 2-4 2-7 2 1 3 3 4 5 1-3 0-6 0-8z"/></svg>"##;
const FILE_ICON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="30" viewBox="0 0 24 24" fill="none" stroke="#f2c14e" stroke-width="2"><path d="M6 3h9l4 4v14H6z"/><path d="M9 13h6M9 17h4"/></svg>"##;
const PLAY_ICON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="26" height="26" viewBox="0 0 24 24" fill="#1a0f0a"><path d="M8 5v14l11-7z"/></svg>"##;

/// What the user wants to get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// A new MP4 with the subtitles drawn into the picture.
    Burn,
    /// Only subtitle files next to the video.
    Srt,
}

/// The state of this screen.
#[derive(Debug)]
pub struct Choose {
    /// The preview frames, ready to draw.
    pub frames: Vec<image::Handle>,
    /// The output exists; the dialog asks before it is replaced.
    pub confirm_replace: bool,
    /// When the output mode last changed, for the border fade.
    pub mode_changed: Instant,
}

impl Default for Choose {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            confirm_replace: false,
            mode_changed: Instant::now(),
        }
    }
}

/// A language in the target list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    pub code: String,
    pub name: String,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// "Keep the original language" first, then every language Whisper knows, by name.
pub fn languages() -> Vec<Language> {
    let mut list: Vec<Language> = verbacue::translate::languages()
        .iter()
        .map(|(code, name)| Language {
            code: (*code).to_string(),
            name: (*name).to_string(),
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list.insert(
        0,
        Language {
            code: KEEP.to_string(),
            name: "Keep the original language".to_string(),
        },
    );
    list
}

/// An audio track in the track list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub index: usize,
    pub label: String,
}

impl std::fmt::Display for Track {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

fn track(t: &AudioTrack) -> Track {
    let language = t
        .language
        .as_deref()
        .map_or_else(|| "Unknown language".to_string(), language_label);
    let channels = match t.channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        6 => "5.1".to_string(),
        8 => "7.1".to_string(),
        n => format!("{n} ch"),
    };
    Track {
        index: t.index,
        label: format!(
            "{} · {language} ({} {channels})",
            t.index + 1,
            t.codec.to_uppercase()
        ),
    }
}

/// A container language tag such as "tur" or "en" as a language name, if known.
fn language_label(tag: &str) -> String {
    let name = verbacue::translate::language_name(tag);
    if name != tag {
        return name.to_string();
    }
    // Three-letter tags: match the English name's start, for example "tur" → Turkish.
    verbacue::translate::languages()
        .iter()
        .find(|(_, n)| n.to_lowercase().starts_with(&tag.to_lowercase()))
        .map_or_else(|| tag.to_string(), |(_, n)| (*n).to_string())
}

/// "1 h 42 min of video".
fn length(ms: u64) -> String {
    if ms < 60_000 {
        return format!("{} s of video", ms / 1000);
    }
    let min = ms / 60_000;
    match (min / 60, min % 60) {
        (0, m) => format!("{m} min of video"),
        (h, m) => format!("{h} h {m} min of video"),
    }
}

/// Fades in and lifts by 14 px over 0.6 s after `delay` seconds.
fn fade_up(t: f32, delay: f32) -> (f32, f32) {
    let p = ((t - delay) / 0.6).clamp(0.0, 1.0);
    let ease = 1.0 - (1.0 - p).powi(3);
    (ease, 14.0 * (1.0 - ease))
}

pub fn view(app: &App) -> Element<'_, Message> {
    let Some(selection) = &app.selection else {
        return text("").into();
    };
    let t = app.seconds();
    let info = &selection.info;

    let back = button(text("← Pick another video").size(14).color(theme::MUTED))
        .padding(0)
        .style(|_, _| button::Style::default())
        .on_press(Message::Show(crate::app::Screen::Pick));

    // Preview: the frames cross-fade over 1.2 s, one every 3 s.
    let frames = &app.choose.frames;
    let preview: Element<'_, Message> = if frames.is_empty() {
        container(text("Loading the preview…").color(theme::FAINT))
            .center(Length::Fill)
            .into()
    } else {
        let n = frames.len();
        let step = (t / 3.0).floor() as usize;
        let fade = ((t % 3.0 - 1.8) / 1.2).clamp(0.0, 1.0);
        stack![
            image(frames[step % n].clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Cover)
                .border_radius(22),
            image(frames[(step + 1) % n].clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Cover)
                .border_radius(22)
                .opacity(fade),
        ]
        .into()
    };
    let play = button(
        container(
            svg(svg::Handle::from_memory(PLAY_ICON))
                .width(26)
                .height(26),
        )
        .center(72),
    )
    .padding(0)
    .style(|_, status| button::Style {
        background: Some(
            Color::from_rgba8(
                0xf3,
                0xe9,
                0xdc,
                if matches!(status, button::Status::Hovered) {
                    1.0
                } else {
                    0.92
                },
            )
            .into(),
        ),
        border: Border {
            radius: 36.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    })
    .on_press(Message::PlayVideo);
    let preview = container(stack![
        preview,
        canvas(Scan { t }).width(Length::Fill).height(Length::Fill),
        container(play).center(Length::Fill),
    ])
    .width(Length::Fill)
    .height(Length::Fixed(300.0))
    .clip(true)
    .style(|_| container::Style {
        background: Some(theme::GLOW.into()),
        border: Border {
            radius: 22.0.into(),
            ..Border::default()
        },
        shadow: Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.8),
            offset: Vector::new(0.0, 30.0),
            blur_radius: 80.0,
        },
        ..container::Style::default()
    });

    let chip = |label: String, green: bool| {
        container(text(label).size(13).color(if green {
            theme::SUCCESS_TEXT
        } else {
            theme::TEXT
        }))
        .padding([6, 12])
        .style(move |_| container::Style {
            background: Some(
                if green {
                    Color::from_rgba8(0x3f, 0x8f, 0x3a, 0.18)
                } else {
                    theme::CARD_HOVER
                }
                .into(),
            ),
            border: Border {
                radius: 999.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
    };
    let name = selection
        .video
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let resolution = match info.video_codec.as_deref() {
        Some(codec) => format!("{}p · {}", info.height, codec.to_uppercase()),
        None => "Audio only".to_string(),
    };
    let tracks = match info.audio.len() {
        1 => "1 audio track".to_string(),
        n => format!("{n} audio tracks"),
    };
    let chips = row![
        chip(name, false),
        chip(resolution, false),
        chip(tracks, false),
        chip("Language: detected on start".to_string(), true),
    ]
    .spacing(8)
    .wrap();

    let (a0, y0) = fade_up(t, 0.0);
    let left = column![preview, chips].spacing(14);

    // The right side: title, cards, pickers, start.
    let (a1, y1) = fade_up(t, 0.08);
    let (a2, y2) = fade_up(t, 0.16);
    let (a3, y3) = fade_up(t, 0.24);
    let title = text("How do you want it served?")
        .font(theme::HEADING)
        .size(32)
        .color(Color {
            a: a1,
            ..theme::TEXT
        });
    let mode = if selection.burn {
        OutputMode::Burn
    } else {
        OutputMode::Srt
    };
    let change = app.choose.mode_changed.elapsed().as_secs_f32().min(0.25) / 0.25;
    let cards = row![
        card(
            FLAME_ICON,
            "Burned in",
            "A new MP4 with the subtitles drawn into the picture. Plays anywhere.",
            mode == OutputMode::Burn,
            change,
            OutputMode::Burn,
            a2,
        ),
        card(
            FILE_ICON,
            "Separate SRT file",
            "A subtitle file next to the video. Fast, and you can edit it later.",
            mode == OutputMode::Srt,
            change,
            OutputMode::Srt,
            a2,
        ),
    ]
    .spacing(14);

    let languages = languages();
    let selected = languages
        .iter()
        .find(|l| l.code == selection.target)
        .cloned();
    let track_options: Vec<Track> = info.audio.iter().map(track).collect();
    let selected_track = track_options.get(selection.track).cloned();
    let labelled = |label: &'static str, control: Element<'static, Message>| {
        column![text(label).size(13).color(theme::MUTED), control]
            .spacing(6)
            .width(Length::Fill)
    };
    let pickers = row![
        labelled(
            "Translate to",
            pick_list(languages, selected, |l: Language| Message::Target(l.code))
                .width(Length::Fill)
                .padding([12, 14])
                .style(theme::pick)
                .into(),
        ),
        labelled(
            "Audio track",
            pick_list(track_options, selected_track, |t: Track| Message::Track(
                t.index
            ))
            .width(Length::Fill)
            .padding([12, 14])
            .style(theme::pick)
            .into(),
        ),
    ]
    .spacing(14);

    let pulse = wave(t, 2.4, 0.0);
    let start = button(text("Start grilling →").font(theme::BODY_BOLD).size(17))
        .padding([16, 30])
        .style(move |theme, status| {
            let mut style = theme::primary(theme, status);
            style.shadow.blur_radius = 24.0 + 16.0 * pulse;
            style.shadow.color.a = 0.25 + 0.3 * pulse;
            style
        })
        .on_press(Message::Start);
    let start_row = row![
        start,
        text(length(info.duration_ms)).size(13).color(theme::FAINT)
    ]
    .spacing(16)
    .align_y(Alignment::Center);

    let right = column![
        container(title).padding(iced::Padding::ZERO.top(y1)),
        container(cards).padding(iced::Padding::ZERO.top(y2)),
        container(pickers).padding(iced::Padding::ZERO.top(y3)),
        container(start_row).padding(iced::Padding::ZERO.top(6.0 + y3)),
    ]
    .spacing(18);
    let _ = a3;

    let body = row![
        container(left)
            .padding(iced::Padding::ZERO.top(y0))
            .width(Length::FillPortion(11)),
        container(right).width(Length::FillPortion(9)),
    ]
    .spacing(28);
    let _ = a0;

    let mut page = column![back, body].spacing(26);
    if app.choose.confirm_replace {
        page = page.push(replace_dialog(selection));
    }
    container(page).padding([40, 56]).into()
}

fn card<'a>(
    icon: &'static [u8],
    title: &'a str,
    body: &'a str,
    selected: bool,
    change: f32,
    mode: OutputMode,
    alpha: f32,
) -> Element<'a, Message> {
    let border_alpha = if selected { 0.1 + 0.9 * change } else { 0.1 };
    let border_color = if selected {
        Color {
            a: border_alpha,
            ..theme::ACCENT
        }
    } else {
        Color::from_rgba8(0xf3, 0xe9, 0xdc, 0.1)
    };
    let background = if selected {
        Color::from_rgba8(0xe8, 0x62, 0x2a, 0.14 * change.max(0.3))
    } else {
        theme::CARD
    };
    button(
        column![
            svg(svg::Handle::from_memory(icon)).width(30).height(30),
            text(title).font(theme::HEADING).size(20).color(Color {
                a: alpha,
                ..theme::TEXT
            }),
            text(body).size(14).color(theme::MUTED).line_height(1.5),
        ]
        .spacing(10),
    )
    .padding(22)
    .width(Length::Fill)
    .style(move |_, status| {
        let lift = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(background.into()),
            text_color: theme::TEXT,
            border: Border {
                color: border_color,
                width: 2.0,
                radius: 20.0.into(),
            },
            shadow: Shadow {
                color: Color::from_rgba8(0, 0, 0, if lift { 0.35 } else { 0.0 }),
                offset: Vector::new(0.0, if lift { 6.0 } else { 0.0 }),
                blur_radius: 18.0,
            },
            snap: true,
        }
    })
    .on_press(Message::Mode(mode))
    .into()
}

fn replace_dialog(selection: &crate::app::Selection) -> Element<'_, Message> {
    let _ = selection;
    container(
        column![
            text("Replace it?").font(theme::HEADING).size(22).color(theme::TEXT),
            text("This video already has subtitles from an earlier run. verbacue makes them again and overwrites the files.")
                .size(14)
                .color(theme::MUTED),
            row![
                button(text("Replace").font(theme::BODY_BOLD))
                    .padding([12, 22])
                    .style(theme::primary)
                    .on_press(Message::ConfirmReplace(true)),
                button(text("Keep the old files"))
                    .padding([12, 22])
                    .style(theme::ghost)
                    .on_press(Message::ConfirmReplace(false)),
            ]
            .spacing(12),
        ]
        .spacing(12),
    )
    .padding(22)
    .width(Length::Fill)
    .style(theme::card(20.0))
    .into()
}

/// A light band that moves across the preview every 3.5 s.
struct Scan {
    t: f32,
}

impl canvas::Program<Message> for Scan {
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
        let width = bounds.width * 0.3;
        let x = -width + (bounds.width + width) * cycle(self.t, 3.5, 0.0);
        let glow = Color::from_rgba8(0xff, 0xb4, 0x78, 0.08);
        let fill = Linear::new(Point::new(x, 0.0), Point::new(x + width, 0.0))
            .add_stop(0.0, Color::TRANSPARENT)
            .add_stop(0.5, glow)
            .add_stop(1.0, Color::TRANSPARENT);
        frame.fill(
            &Path::rectangle(Point::new(x, 0.0), Size::new(width, bounds.height)),
            fill,
        );
        vec![frame.into_geometry()]
    }
}
