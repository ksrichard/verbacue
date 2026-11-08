//! The Converting screen's barbecue: the 4 verb chunks on a skewer over flames, with
//! sparks and smoke. Sizes follow `Convert.dc.html` (560 x 250).

use iced::{
    Color, Point, Rectangle, Renderer, Size, Theme, Vector, alignment, mouse,
    widget::canvas::{self, Frame, Geometry, Path, Text, gradient::Linear},
};

use super::{cycle, wave};
use crate::theme;

pub struct Skewer {
    /// Seconds since the screen opened.
    pub t: f32,
    /// How many chunks are done (cooked) and which one cooks now.
    pub cooked: usize,
    pub cooking: Option<usize>,
}

/// (word, color, text color, width, height) of each chunk.
const CHUNKS: [(&str, Color, Color, f32, f32); 4] = [
    (
        "say",
        theme::MEAT,
        Color::from_rgb(1.0, 0.957, 0.878),
        78.0,
        72.0,
    ),
    (
        "tell",
        theme::PEPPER,
        Color::from_rgb(0.949, 1.0, 0.91),
        74.0,
        78.0,
    ),
    (
        "talk",
        theme::ONION,
        Color::from_rgb(0.353, 0.196, 0.0),
        78.0,
        72.0,
    ),
    (
        "sing",
        theme::RED_PEPPER,
        Color::from_rgb(1.0, 0.941, 0.918),
        78.0,
        78.0,
    ),
];

/// (x, width, height, period, delay) of each flame.
const FLAMES: [(f32, f32, f32, f32, f32); 4] = [
    (118.0, 30.0, 54.0, 0.9, 0.0),
    (210.0, 36.0, 70.0, 1.1, 0.2),
    (306.0, 32.0, 60.0, 0.8, 0.45),
    (412.0, 26.0, 48.0, 1.25, 0.1),
];

impl<Message> canvas::Program<Message> for Skewer {
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
        let scale = (bounds.width / 560.0).min(bounds.height / 250.0);
        frame.translate(Vector::new(
            (bounds.width - 560.0 * scale) / 2.0,
            (bounds.height - 250.0 * scale) / 2.0,
        ));
        frame.scale(scale);
        let t = self.t;

        // Smoke: three subtitle bars rise 120 px and shrink every 3.6 s.
        for (x, w, delay) in [
            (123.0, 314.0, 0.0),
            (168.0, 224.0, 1.2),
            (213.0, 134.0, 2.4),
        ] {
            let p = cycle(t, 3.6, delay);
            let alpha = if p < 0.2 {
                p / 0.2 * 0.8
            } else {
                0.8 * (1.0 - (p - 0.2) / 0.8)
            };
            let shrink = 1.0 - 0.4 * p;
            let width = w * shrink;
            frame.fill(
                &Path::rounded_rectangle(
                    Point::new(x + (w - width) / 2.0, 20.0 - 120.0 * p + 40.0),
                    Size::new(width, 12.0),
                    6.0.into(),
                ),
                Color {
                    a: alpha * 0.85,
                    ..theme::TEXT
                },
            );
        }

        // Flames under the skewer.
        for (x, w, h, period, delay) in FLAMES {
            let height = h * (1.0 + 0.25 * wave(t, period, delay));
            let base = 222.0;
            let top = base - height;
            let r = w / 2.0;
            let flame = Path::new(|b| {
                b.move_to(Point::new(x, base));
                b.line_to(Point::new(x, top + r));
                b.arc_to(Point::new(x, top), Point::new(x + r, top), r);
                b.arc_to(Point::new(x + w, top), Point::new(x + w, top + r), r);
                b.line_to(Point::new(x + w, base));
                b.close();
            });
            let fill = Linear::new(Point::new(0.0, base), Point::new(0.0, top))
                .add_stop(0.0, theme::FLAME_LOW)
                .add_stop(1.0, theme::FLAME_HIGH);
            frame.fill(&flame, fill);
        }

        // Sparks fly up from the flames every 1.6 s.
        for (x, dx, delay) in [(196.0, -30.0, 0.0), (291.0, 20.0, 0.5), (358.0, 40.0, 1.0)] {
            let p = cycle(t, 1.6, delay);
            frame.fill(
                &Path::circle(Point::new(x + dx * p, 180.0 - 90.0 * p), 2.5),
                Color {
                    a: 1.0 - p,
                    ..theme::FLAME_HIGH
                },
            );
        }

        // The skewer and its tip.
        frame.fill(
            &Path::rounded_rectangle(Point::new(45.0, 120.0), Size::new(470.0, 8.0), 4.0.into()),
            theme::STEEL,
        );
        frame.fill(
            &Path::new(|b| {
                b.move_to(Point::new(515.0, 116.0));
                b.line_to(Point::new(537.0, 124.0));
                b.line_to(Point::new(515.0, 132.0));
                b.close();
            }),
            theme::STEEL,
        );

        // The chunks sizzle; the current one glows, cooked ones darken.
        let mut x = 67.0;
        for (i, (word, color, ink, w, h)) in CHUNKS.into_iter().enumerate() {
            let jiggle = (std::f32::consts::TAU * (t - 0.3 * i as f32) / 1.4).sin();
            let y = 124.0 - h / 2.0 - 3.0 * jiggle.abs();
            let cooking = self.cooking == Some(i);
            let cooked = i < self.cooked;
            if cooking {
                let glow = 0.35 + 0.25 * wave(t, 1.2, 0.0);
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(x - 8.0, y - 8.0),
                        Size::new(w + 16.0, h + 16.0),
                        20.0.into(),
                    ),
                    Color {
                        a: glow,
                        ..theme::FLAME_MID
                    },
                );
            }
            let fill = if cooked || cooking {
                color
            } else {
                Color { a: 0.35, ..color }
            };
            let fill = if cooked {
                Color {
                    r: fill.r * 0.75,
                    g: fill.g * 0.75,
                    b: fill.b * 0.75,
                    a: 1.0,
                }
            } else {
                fill
            };
            frame.fill(
                &Path::rounded_rectangle(Point::new(x, y), Size::new(w, h), 14.0.into()),
                fill,
            );
            if cooked {
                // Grill marks on cooked chunks.
                for k in 0..3 {
                    let mx = x + 12.0 + 20.0 * k as f32;
                    frame.stroke(
                        &Path::line(
                            Point::new(mx, y + 10.0),
                            Point::new(mx + 18.0, y + h - 10.0),
                        ),
                        canvas::Stroke::default()
                            .with_width(4.0)
                            .with_color(Color::from_rgba(0.0, 0.0, 0.0, 0.25)),
                    );
                }
            }
            frame.fill_text(Text {
                content: word.to_string(),
                position: Point::new(x + w / 2.0, y + h / 2.0),
                color: if cooked || cooking {
                    ink
                } else {
                    Color { a: 0.6, ..ink }
                },
                size: 20.0.into(),
                font: theme::HEADING,
                align_x: alignment::Horizontal::Center.into(),
                align_y: alignment::Vertical::Center,
                ..Text::default()
            });
            x += w + 10.0;
        }

        // The grate.
        frame.fill(
            &Path::rounded_rectangle(Point::new(78.0, 226.0), Size::new(404.0, 10.0), 5.0.into()),
            theme::GRATE,
        );
        vec![frame.into_geometry()]
    }
}
