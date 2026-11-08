//! The small grill of the start screen: flickering flames, smoke rising as subtitle
//! bars, a grate and the red kettle bowl. Sizes follow `Main.dc.html` (180 x 150).

use iced::{
    Color, Point, Rectangle, Renderer, Size, Theme, mouse,
    widget::canvas::{self, Frame, Geometry, Path, gradient::Linear},
};

use super::{cycle, wave};
use crate::theme;

pub struct Grill {
    /// Seconds since the screen opened.
    pub t: f32,
}

/// (x, width, height, period, delay) of each flame, from the mockup.
const FLAMES: [(f32, f32, f32, f32, f32); 4] = [
    (30.0, 22.0, 46.0, 1.1, 0.0),
    (62.0, 28.0, 64.0, 0.9, 0.25),
    (100.0, 24.0, 52.0, 1.3, 0.5),
    (134.0, 18.0, 38.0, 1.1, 0.0),
];

/// (x, width, delay) of each smoke bar.
const SMOKE: [(f32, f32, f32); 3] = [(40.0, 100.0, 0.0), (55.0, 70.0, 1.1), (70.0, 40.0, 2.2)];

impl<Message> canvas::Program<Message> for Grill {
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
        let scale = (bounds.width / 180.0).min(bounds.height / 150.0);
        frame.scale(scale);
        let t = self.t;

        // Smoke: rises 52 px and fades in, then out, every 3.2 s.
        for (x, w, delay) in SMOKE {
            let p = cycle(t, 3.2, delay);
            let alpha = if p < 0.3 {
                p / 0.3 * 0.85
            } else {
                0.85 * (1.0 - (p - 0.3) / 0.7)
            };
            let y = 4.0 + 12.0 - 52.0 * p;
            frame.fill(
                &Path::rounded_rectangle(Point::new(x, y), Size::new(w, 8.0), 4.0.into()),
                Color {
                    a: alpha,
                    ..theme::TEXT
                },
            );
        }

        // Flames: grow up to 18 % taller from their base.
        for (x, w, h, period, delay) in FLAMES {
            let grow = 1.0 + 0.18 * wave(t, period, delay);
            let height = h * grow;
            let top = 120.0 - height;
            let flame = Path::new(|b| {
                let r = w / 2.0;
                b.move_to(Point::new(x, 120.0));
                b.line_to(Point::new(x, top + r));
                b.arc_to(Point::new(x, top), Point::new(x + r, top), r);
                b.arc_to(Point::new(x + w, top), Point::new(x + w, top + r), r);
                b.line_to(Point::new(x + w, 120.0));
                b.close();
            });
            let fill = Linear::new(Point::new(0.0, 120.0), Point::new(0.0, top))
                .add_stop(0.0, theme::FLAME_LOW)
                .add_stop(1.0, theme::FLAME_HIGH);
            frame.fill(&flame, fill);
        }

        // Grate and bowl.
        frame.fill(
            &Path::rounded_rectangle(Point::new(10.0, 122.0), Size::new(160.0, 10.0), 5.0.into()),
            theme::GRATE,
        );
        let bowl = Path::new(|b| {
            b.move_to(Point::new(24.0, 132.0));
            b.line_to(Point::new(156.0, 132.0));
            b.quadratic_curve_to(Point::new(150.0, 150.0), Point::new(90.0, 150.0));
            b.quadratic_curve_to(Point::new(30.0, 150.0), Point::new(24.0, 132.0));
            b.close();
        });
        let fill = Linear::new(Point::new(0.0, 132.0), Point::new(0.0, 150.0))
            .add_stop(0.0, theme::BOWL_TOP)
            .add_stop(1.0, theme::BOWL_BOTTOM);
        frame.fill(&bowl, fill);
        vec![frame.into_geometry()]
    }
}
