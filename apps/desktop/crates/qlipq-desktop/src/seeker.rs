//! Timeline scrubber that draws the in/out trim window and marker handles on the seeker bar itself
//! and reports clicks/drags as a seek. Playback is kept inside the in/out window elsewhere (see
//! `App::on_playback_tick` / `App::play_from_current`); this widget is the visual + seek surface that
//! replaces a plain slider so the kept region and its endpoints are visible at a glance.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{tree, Tree};
use iced::advanced::{Clipboard, Shell, Widget};
use iced::border::Radius;
use iced::{mouse, Border, Color, Element, Event, Length, Rectangle, Size, Theme};

const HEIGHT: f32 = 24.0;
const RAIL: f32 = 6.0;
const MARKER_W: f32 = 2.5;
const PLAYHEAD_W: f32 = 2.0;
const KNOB_R: f32 = 5.0;

/// A seeker bar spanning `[0, duration]` with the `[trim_start, trim_end]` window highlighted.
pub struct Seeker<'a, Message> {
    duration: f64,
    position: f64,
    trim_start: f64,
    trim_end: f64,
    on_seek: Box<dyn Fn(f64) -> Message + 'a>,
}

/// Build a [`Seeker`]. `on_seek` receives the clicked/dragged time in seconds (already clamped to the
/// bar; the receiver still clamps to the clip).
pub fn seeker<'a, Message>(
    duration: f64,
    position: f64,
    trim_start: f64,
    trim_end: f64,
    on_seek: impl Fn(f64) -> Message + 'a,
) -> Seeker<'a, Message> {
    Seeker {
        duration: duration.max(0.0),
        position,
        trim_start,
        trim_end,
        on_seek: Box::new(on_seek),
    }
}

#[derive(Default)]
struct State {
    dragging: bool,
}

impl<Message> Seeker<'_, Message> {
    fn publish_seek(&self, cursor_x: f32, bounds: Rectangle, shell: &mut Shell<'_, Message>) {
        if bounds.width <= 0.0 {
            return;
        }
        let percent = ((cursor_x - bounds.x) / bounds.width).clamp(0.0, 1.0);
        shell.publish((self.on_seek)(percent as f64 * self.duration));
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for Seeker<'_, Message>
where
    Renderer: renderer::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(HEIGHT))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fill, Length::Fixed(HEIGHT))
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_over(bounds) {
                    state.dragging = true;
                    self.publish_seek(p.x, bounds, shell);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging = false;
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.dragging {
                    if let Some(p) = cursor.position() {
                        self.publish_seek(p.x, bounds, shell);
                        shell.capture_event();
                    }
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let palette = theme.extended_palette();
        let accent = palette.primary.base.color;
        let rail_bg = palette.background.strong.color;
        let playhead = palette.background.base.text;

        let cy = bounds.y + bounds.height / 2.0;
        let dur = self.duration.max(f64::EPSILON);
        let x_of = |t: f64| bounds.x + (t / dur).clamp(0.0, 1.0) as f32 * bounds.width;
        let bar = |r: &mut Renderer, x: f32, w: f32, y: f32, h: f32, radius: f32, color: Color| {
            r.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x,
                        y,
                        width: w,
                        height: h,
                    },
                    border: Border {
                        radius: Radius::from(radius),
                        ..Border::default()
                    },
                    ..renderer::Quad::default()
                },
                color,
            );
        };

        // Full rail (muted), then the kept in/out window filled with the accent on top of it.
        bar(
            renderer,
            bounds.x,
            bounds.width,
            cy - RAIL / 2.0,
            RAIL,
            RAIL / 2.0,
            rail_bg,
        );
        let x_in = x_of(self.trim_start);
        let x_out = x_of(self.trim_end.max(self.trim_start));
        if x_out > x_in {
            bar(
                renderer,
                x_in,
                x_out - x_in,
                cy - RAIL / 2.0,
                RAIL,
                RAIL / 2.0,
                accent,
            );
        }

        // In/out marker handles: full-height accent ticks at each endpoint.
        for x in [x_in, x_out] {
            bar(
                renderer,
                x - MARKER_W / 2.0,
                MARKER_W,
                bounds.y,
                bounds.height,
                1.0,
                accent,
            );
        }

        // Playhead: full-height bar + a round knob at the rail.
        let px = x_of(self.position);
        bar(
            renderer,
            px - PLAYHEAD_W / 2.0,
            PLAYHEAD_W,
            bounds.y,
            bounds.height,
            1.0,
            playhead,
        );
        bar(
            renderer,
            px - KNOB_R,
            KNOB_R * 2.0,
            cy - KNOB_R,
            KNOB_R * 2.0,
            KNOB_R,
            playhead,
        );
    }
}

impl<'a, Message, Renderer> From<Seeker<'a, Message>> for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(seeker: Seeker<'a, Message>) -> Self {
        Element::new(seeker)
    }
}
