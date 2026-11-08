//! An on/off switch whose knob slides with a small bounce over 0.3 s.

use iced::{
    Border, Color, Element, Length,
    widget::{button, container, row, space},
};

use crate::theme;

/// The knob position, 0 (off) to 1 (on), `since` seconds after the last change. It
/// overshoots a little, then settles.
fn position(on: bool, since: f32) -> f32 {
    let p = (since / 0.3).clamp(0.0, 1.0);
    // An ease-out curve that overshoots by about 10 %.
    let c = 1.7;
    let eased = 1.0 + (c + 1.0) * (p - 1.0).powi(3) + c * (p - 1.0).powi(2);
    if on { eased } else { 1.0 - eased }
}

pub fn view<'a, Message: Clone + 'a>(
    on: bool,
    since: f32,
    message: Message,
) -> Element<'a, Message> {
    let x = position(on, since);
    let knob = container(space())
        .width(24)
        .height(24)
        .style(|_| container::Style {
            background: Some(theme::TEXT.into()),
            border: Border {
                radius: 12.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        });
    let track = Color {
        r: 0.29 + (0.91 - 0.29) * x.clamp(0.0, 1.0),
        g: 0.25 + (0.38 - 0.25) * x.clamp(0.0, 1.0),
        b: 0.23 + (0.16 - 0.23) * x.clamp(0.0, 1.0),
        a: 1.0,
    };
    button(row![space().width(Length::Fixed(22.0 * x.max(0.0))), knob])
        .padding(3)
        .width(52)
        .height(30)
        .style(move |_, _| button::Style {
            background: Some(track.into()),
            border: Border {
                radius: 15.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        })
        .on_press(message)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_moves_and_settles() {
        assert_eq!(position(true, 0.0), 0.0);
        assert!((position(true, 1.0) - 1.0).abs() < 1e-6);
        assert!((position(false, 1.0)).abs() < 1e-6);
        assert!((0..30).any(|i| position(true, i as f32 * 0.01) > 1.0));
    }
}
