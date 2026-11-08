//! Confetti and the popping check mark of the Done screen (`Done.dc.html`).

use iced::{
    Color, Point, Rectangle, Renderer, Size, Theme, Vector, mouse,
    widget::canvas::{self, Frame, Geometry, Path, Stroke},
};

use super::cycle;
use crate::theme;

/// Pieces of confetti that fall and spin over 2.4 s in a loop.
pub struct Confetti {
    pub t: f32,
}

/// (x as a share of the width, color, sideways drift, turns, delay).
const PIECES: [(f32, Color, f32, f32, f32); 5] = [
    (0.20, theme::ACCENT, -40.0, 0.83, 0.0),
    (0.32, theme::ONION, 30.0, -0.72, 0.4),
    (0.48, theme::PEPPER, -20.0, 0.56, 0.8),
    (0.61, theme::TEXT, 50.0, -0.89, 0.2),
    (0.74, theme::RED_PEPPER, -30.0, 0.78, 0.6),
];

impl<Message> canvas::Program<Message> for Confetti {
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
        for (share, color, drift, turns, delay) in PIECES {
            let p = cycle(self.t, 2.4, delay);
            let ease = p * p;
            frame.with_save(|f| {
                f.translate(Vector::new(
                    bounds.width * share + drift * ease,
                    220.0 * ease,
                ));
                f.rotate(turns * std::f32::consts::TAU * ease);
                f.fill(
                    &Path::rounded_rectangle(
                        Point::new(-5.0, -8.0),
                        Size::new(10.0, 16.0),
                        3.0.into(),
                    ),
                    Color {
                        a: 1.0 - ease,
                        ..color
                    },
                );
            });
        }
        vec![frame.into_geometry()]
    }
}

/// The green circle with a check mark. It grows to 112 % and settles at 100 % over
/// 0.7 s.
pub struct Check {
    pub t: f32,
}

impl Check {
    fn scale(&self) -> f32 {
        let p = (self.t / 0.7).clamp(0.0, 1.0);
        if p < 0.6 {
            0.4 + (1.12 - 0.4) * (p / 0.6)
        } else {
            1.12 - 0.12 * ((p - 0.6) / 0.4)
        }
    }
}

impl<Message> canvas::Program<Message> for Check {
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
        let center = frame.center();
        let scale = self.scale();
        let alpha = (self.t / 0.3).clamp(0.0, 1.0);
        frame.translate(Vector::new(center.x, center.y));
        frame.scale(scale);
        frame.fill(
            &Path::circle(Point::ORIGIN, 72.0),
            Color {
                a: 0.18 * alpha,
                ..theme::SUCCESS
            },
        );
        frame.fill(
            &Path::circle(Point::ORIGIN, 60.0),
            Color {
                a: alpha,
                ..theme::SUCCESS
            },
        );
        let check = Path::new(|b| {
            b.move_to(Point::new(-23.0, 0.0));
            b.line_to(Point::new(-7.0, 16.0));
            b.line_to(Point::new(23.0, -16.0));
        });
        frame.stroke(
            &check,
            Stroke::default()
                .with_width(10.0)
                .with_color(Color {
                    a: alpha,
                    ..Color::from_rgb8(0xf2, 0xff, 0xe8)
                })
                .with_line_cap(canvas::LineCap::Round)
                .with_line_join(canvas::LineJoin::Round),
        );
        vec![frame.into_geometry()]
    }
}
