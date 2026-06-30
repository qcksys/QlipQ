//! Custom brand theme + reusable widget styles. Centralizes the app's "design tokens" (palette,
//! radius, surfaces) so the look stays consistent and a re-skin is a one-file change. Built on
//! iced 0.14's custom `Palette` (Oklch-derived extended palette) + per-widget style closures.

use iced::border::Radius;
use iced::theme::Palette;
use iced::widget::container;
use iced::{Background, Border, Color, Shadow, Theme, Vector};

use qlipq_core::queue::QueueStatus;

const RADIUS: f32 = 10.0;

/// The app's dark brand palette. Built once in `App::new` and cloned by `App::theme`.
pub fn dark() -> Theme {
    Theme::custom(
        "qlipq",
        Palette {
            background: Color::from_rgb8(0x15, 0x16, 0x1b),
            text: Color::from_rgb8(0xe7, 0xe9, 0xee),
            primary: Color::from_rgb8(0x7c, 0x93, 0xff),
            success: Color::from_rgb8(0x4f, 0xd1, 0x9a),
            warning: Color::from_rgb8(0xe7, 0xb4, 0x55),
            danger: Color::from_rgb8(0xf0, 0x71, 0x71),
        },
    )
}

/// Raised surface for sections, panels and cards: tinted background, hairline border, soft shadow.
pub fn card(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.weak.color)),
        border: Border { color: p.background.strong.color, width: 1.0, radius: Radius::from(RADIUS) },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.30),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 10.0,
        },
        ..container::Style::default()
    }
}

/// A queue card, styled to reflect its selection state (accent border + tint when selected).
pub fn queue_card(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let p = theme.extended_palette();
        if selected {
            container::Style {
                background: Some(Background::Color(p.primary.base.color.scale_alpha(0.18))),
                border: Border {
                    color: p.primary.base.color,
                    width: 1.5,
                    radius: Radius::from(RADIUS),
                },
                ..container::Style::default()
            }
        } else {
            container::Style {
                background: Some(Background::Color(p.background.weak.color)),
                border: Border {
                    color: p.background.strong.color,
                    width: 1.0,
                    radius: Radius::from(RADIUS),
                },
                ..container::Style::default()
            }
        }
    }
}

/// Secondary / muted text color for meta lines and hints.
pub fn muted(theme: &Theme) -> Color {
    theme.palette().text.scale_alpha(0.55)
}

/// Accent color for a queue item's status label.
pub fn status_color(theme: &Theme, status: QueueStatus) -> Color {
    let p = theme.extended_palette();
    match status {
        QueueStatus::Done => p.success.base.color,
        QueueStatus::Error => p.danger.base.color,
        QueueStatus::Exporting => p.primary.base.color,
        QueueStatus::Ready => p.success.weak.color,
        QueueStatus::Editing => p.primary.weak.color,
        QueueStatus::Pending => muted(theme),
    }
}
