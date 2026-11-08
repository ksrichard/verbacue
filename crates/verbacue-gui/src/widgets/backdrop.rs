//! The dark background with a warm glow, as in the mockups.

use std::cell::Cell;

use iced::{
    Color, Point, Rectangle, Renderer, Theme, mouse,
    widget::canvas::{self, Cache, Geometry, Path},
};

use crate::theme;

/// Where the glow sits, as a fraction of the size, and its color.
pub struct Backdrop {
    pub center: (f32, f32),
    pub glow: Color,
}

/// The drawn glow. It is drawn again only when its place or color changes; the cache
/// also redraws when the size changes.
#[derive(Default)]
pub struct Drawn {
    cache: Cache,
    key: Cell<Option<(f32, f32, [u8; 4])>>,
}

impl<Message> canvas::Program<Message> for Backdrop {
    type State = Drawn;

    fn draw(
        &self,
        state: &Drawn,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = Some((self.center.0, self.center.1, self.glow.into_rgba8()));
        if state.key.replace(key) != key {
            state.cache.clear();
        }
        vec![
            state
                .cache
                .draw(renderer, bounds.size(), |frame| self.paint(frame, bounds)),
        ]
    }
}

impl Backdrop {
    fn paint(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::BG);
        // A radial glow made from many soft circles, from large and faint to small.
        let center = Point::new(bounds.width * self.center.0, bounds.height * self.center.1);
        let max = bounds.width.max(bounds.height) * 0.75;
        let steps = 40;
        for i in 0..steps {
            let t = i as f32 / steps as f32;
            let radius = max * (1.0 - t);
            let color = Color {
                a: 0.035,
                ..self.glow
            };
            frame.fill(&Path::circle(center, radius), color);
        }
    }
}
