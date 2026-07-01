//! Custom brand theme + reusable widget styles. Centralizes the app's "design tokens" (palette,
//! type scale, spacing, elevation) and per-widget style closures so the look stays consistent and a
//! re-skin is a one-file change. Built on the GUI toolkit's custom `Palette` (Oklch-derived extended
//! palette) + per-widget style closures.

use iced::border::Radius;
use iced::font::Weight;
use iced::theme::Palette;
use iced::widget::{button, checkbox, container, pick_list, progress_bar, slider, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme, Vector};

use qlipq_core::queue::QueueStatus;

// ---- Surface ramp (explicit so the dark theme reads with real depth, not one flat fill) ----
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}
const C_CANVAS: Color = rgb(0x0e, 0x0f, 0x13); // app background (deepest)
const C_SIDEBAR: Color = rgb(0x14, 0x15, 0x1c); // sidebar / top bar
const C_PANEL: Color = rgb(0x17, 0x19, 0x21); // flat inset panels
const C_SURFACE: Color = rgb(0x1c, 0x1e, 0x28); // raised cards / inputs
const C_SURFACE_HI: Color = rgb(0x24, 0x27, 0x33); // hover / pressed surface
const C_DIALOG: Color = rgb(0x20, 0x23, 0x2e); // modal surface
const C_BORDER: Color = rgb(0x2b, 0x2e, 0x3a); // hairline border
const C_BORDER_STRONG: Color = rgb(0x3b, 0x40, 0x50); // stronger border / unfilled rail
const C_TEXT: Color = rgb(0xe7, 0xe9, 0xee);
const C_MUTED: Color = rgb(0x9a, 0xa3, 0xb6); // solid muted text (≥4.5:1 on the surfaces above)

// ---- Radii ----
pub const RADIUS: f32 = 10.0;
pub const RADIUS_SM: f32 = 7.0;
pub const RADIUS_LG: f32 = 14.0;
pub const RADIUS_PILL: f32 = 999.0;

// ---- Spacing scale (4px base) ----
pub const XS: f32 = 4.0;
pub const SM: f32 = 8.0;
pub const MD: f32 = 12.0;
pub const LG: f32 = 16.0;
pub const XL: f32 = 24.0;

// ---- Type scale (f32: `text.size` takes `Into<Pixels>`, which covers f32 but not u16) ----
pub const DISPLAY: f32 = 22.0;
pub const TITLE: f32 = 18.0;
pub const HEADING: f32 = 15.0;
pub const BODY: f32 = 14.0;
pub const LABEL: f32 = 13.0;
pub const META: f32 = 12.0;
pub const SMALL: f32 = 11.0;

// ---- Fonts (Inter is bundled + registered in main(); weights select along its variable axis) ----
pub const FONT: Font = Font::with_name("Inter");
pub const FONT_MEDIUM: Font = Font {
    weight: Weight::Medium,
    ..Font::with_name("Inter")
};
pub const FONT_SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..Font::with_name("Inter")
};
pub const FONT_BOLD: Font = Font {
    weight: Weight::Bold,
    ..Font::with_name("Inter")
};

fn shadow(alpha: f32, y: f32, blur: f32) -> Shadow {
    Shadow {
        color: Color {
            a: alpha,
            ..Color::BLACK
        },
        offset: Vector::new(0.0, y),
        blur_radius: blur,
    }
}

/// The app's dark brand palette. Built once in `App::new` and cloned by `App::theme`.
pub fn dark() -> Theme {
    Theme::custom(
        "qlipq",
        Palette {
            background: C_CANVAS,
            text: C_TEXT,
            primary: rgb(0x7c, 0x93, 0xff),
            success: rgb(0x4f, 0xd1, 0x9a),
            warning: rgb(0xe7, 0xb4, 0x55),
            danger: rgb(0xf0, 0x71, 0x71),
        },
    )
}

// ---- Container surfaces (elevation ramp) ----

/// Root window background.
pub fn canvas(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(C_CANVAS)),
        ..container::Style::default()
    }
}

/// Queue sidebar column.
pub fn sidebar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(C_SIDEBAR)),
        ..container::Style::default()
    }
}

/// Top app bar: sidebar tone with a soft downward shadow to lift it off the content.
pub fn top_bar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(C_SIDEBAR)),
        shadow: shadow(0.35, 2.0, 10.0),
        ..container::Style::default()
    }
}

/// Raised surface for sections, panels and cards: tinted background, hairline border, soft shadow.
pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(C_SURFACE)),
        border: Border {
            color: C_BORDER,
            width: 1.0,
            radius: Radius::from(RADIUS),
        },
        shadow: shadow(0.22, 2.0, 8.0),
        ..container::Style::default()
    }
}

/// Flat inset panel (no shadow) — for nested groups inside a card.
pub fn panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(C_PANEL)),
        border: Border {
            color: C_BORDER,
            width: 1.0,
            radius: Radius::from(RADIUS_SM),
        },
        ..container::Style::default()
    }
}

/// Modal dialog surface: highest elevation, larger radius, deep shadow.
pub fn dialog(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(C_DIALOG)),
        border: Border {
            color: C_BORDER_STRONG,
            width: 1.0,
            radius: Radius::from(RADIUS_LG),
        },
        shadow: shadow(0.5, 18.0, 48.0),
        ..container::Style::default()
    }
}

/// Semi-opaque backdrop behind a modal so the app dims instead of vanishing.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color {
            a: 0.62,
            ..Color::BLACK
        })),
        ..container::Style::default()
    }
}

/// A tag chip: tinted accent pill.
pub fn chip(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.primary.base.color.scale_alpha(0.16))),
        border: Border {
            color: p.primary.base.color.scale_alpha(0.30),
            width: 1.0,
            radius: Radius::from(RADIUS_PILL),
        },
        text_color: Some(p.primary.base.color),
        ..container::Style::default()
    }
}

/// A small filled dot colored by queue status (for the queue card).
pub fn status_dot(status: QueueStatus) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        background: Some(Background::Color(status_color(theme, status))),
        border: Border {
            radius: Radius::from(RADIUS_PILL),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// A queue card, styled to reflect its selection state (accent border + tint when selected).
pub fn queue_card(selected: bool, hovered: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let p = theme.extended_palette();
        if selected {
            container::Style {
                background: Some(Background::Color(p.primary.base.color.scale_alpha(0.16))),
                border: Border {
                    color: p.primary.base.color,
                    width: 1.5,
                    radius: Radius::from(RADIUS),
                },
                shadow: shadow(0.25, 2.0, 10.0),
                ..container::Style::default()
            }
        } else if hovered {
            container::Style {
                background: Some(Background::Color(C_SURFACE_HI)),
                border: Border {
                    color: C_BORDER_STRONG,
                    width: 1.0,
                    radius: Radius::from(RADIUS),
                },
                shadow: shadow(0.22, 2.0, 10.0),
                ..container::Style::default()
            }
        } else {
            container::Style {
                background: Some(Background::Color(C_SURFACE)),
                border: Border {
                    color: C_BORDER,
                    width: 1.0,
                    radius: Radius::from(RADIUS),
                },
                shadow: shadow(0.18, 1.0, 6.0),
                ..container::Style::default()
            }
        }
    }
}

// ---- Text colors ----

/// Secondary / muted text color for meta lines and hints.
pub fn muted(_theme: &Theme) -> Color {
    C_MUTED
}

/// Accent color for a queue item's status label / dot.
pub fn status_color(theme: &Theme, status: QueueStatus) -> Color {
    let p = theme.extended_palette();
    match status {
        QueueStatus::Done => p.success.base.color,
        QueueStatus::Error => p.danger.base.color,
        QueueStatus::Exporting => p.primary.base.color,
        QueueStatus::Ready => p.success.base.color,
        QueueStatus::Editing => p.primary.base.color,
        QueueStatus::Pending => C_MUTED,
    }
}

// ---- Buttons ----

/// Primary call-to-action.
pub fn btn_primary(theme: &Theme, status: button::Status) -> button::Style {
    let mut s = button::primary(theme, status);
    s.border.radius = Radius::from(RADIUS_SM);
    s
}

/// Neutral secondary action.
pub fn btn_secondary(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    let bg = match status {
        button::Status::Hovered => C_SURFACE_HI,
        button::Status::Pressed => C_BORDER,
        button::Status::Disabled => C_PANEL,
        button::Status::Active => C_SURFACE,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: if matches!(status, button::Status::Disabled) {
            C_MUTED
        } else {
            p.background.base.text
        },
        border: Border {
            color: C_BORDER_STRONG,
            width: 1.0,
            radius: Radius::from(RADIUS_SM),
        },
        ..button::Style::default()
    }
}

/// Destructive action.
pub fn btn_danger(theme: &Theme, status: button::Status) -> button::Style {
    let mut s = button::danger(theme, status);
    s.border.radius = Radius::from(RADIUS_SM);
    s
}

/// A fully transparent click target (no background in any state) — used inside a card that supplies
/// its own background/hover, so the click region doesn't double-tint.
pub fn btn_plain(theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: theme.extended_palette().background.base.text,
        border: Border {
            radius: Radius::from(RADIUS_SM),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// Low-emphasis / icon button: no chrome at rest, faint surface on hover.
pub fn btn_ghost(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => Some(Background::Color(C_SURFACE_HI)),
        _ => None,
    };
    button::Style {
        background: bg,
        text_color: match status {
            button::Status::Disabled => C_MUTED,
            _ => p.background.base.text,
        },
        border: Border {
            radius: Radius::from(RADIUS_SM),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// A nav tab: filled accent tint when active, ghost when not.
pub fn nav(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let p = theme.extended_palette();
        if active {
            button::Style {
                background: Some(Background::Color(p.primary.base.color.scale_alpha(0.18))),
                text_color: p.primary.base.color,
                border: Border {
                    radius: Radius::from(RADIUS_SM),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        } else {
            btn_ghost(theme, status)
        }
    }
}

// ---- Inputs / controls ----

/// Text input with a focus ring.
pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let p = theme.extended_palette();
    let mut s = text_input::Style {
        background: Background::Color(C_SURFACE),
        border: Border {
            color: C_BORDER,
            width: 1.0,
            radius: Radius::from(RADIUS_SM),
        },
        icon: C_MUTED,
        placeholder: C_MUTED,
        value: C_TEXT,
        selection: p.primary.base.color.scale_alpha(0.35),
    };
    match status {
        text_input::Status::Focused { .. } => {
            s.border.color = p.primary.base.color;
            s.border.width = 1.5;
        }
        text_input::Status::Hovered => s.border.color = C_BORDER_STRONG,
        text_input::Status::Disabled => s.background = Background::Color(C_PANEL),
        text_input::Status::Active => {}
    }
    s
}

/// Slider rail + handle, with an accent fill and a focus ring on hover/drag.
pub fn slider_style(theme: &Theme, status: slider::Status) -> slider::Style {
    let p = theme.extended_palette();
    let engaged = !matches!(status, slider::Status::Active);
    slider::Style {
        rail: slider::Rail {
            backgrounds: (
                Background::Color(p.primary.base.color),
                Background::Color(C_BORDER_STRONG),
            ),
            width: 6.0,
            border: Border {
                radius: Radius::from(3.0),
                ..Border::default()
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle {
                radius: if engaged { 9.0 } else { 7.0 },
            },
            background: Background::Color(p.primary.base.color),
            border_width: if engaged { 4.0 } else { 0.0 },
            border_color: p.primary.base.color.scale_alpha(0.30),
        },
    }
}

/// Checkbox box: accent fill when checked.
pub fn checkbox_style(theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let p = theme.extended_palette();
    let checked = matches!(
        status,
        checkbox::Status::Active { is_checked: true }
            | checkbox::Status::Hovered { is_checked: true }
            | checkbox::Status::Disabled { is_checked: true }
    );
    let hovered = matches!(status, checkbox::Status::Hovered { .. });
    checkbox::Style {
        background: Background::Color(if checked {
            p.primary.base.color
        } else {
            C_SURFACE
        }),
        icon_color: p.primary.base.text,
        border: Border {
            color: if checked || hovered {
                p.primary.base.color
            } else {
                C_BORDER_STRONG
            },
            width: 1.5,
            radius: Radius::from(5.0),
        },
        text_color: None,
    }
}

/// Pick-list control (the dropdown menu keeps the theme default).
pub fn pick_list_style(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let p = theme.extended_palette();
    let mut s = pick_list::Style {
        text_color: C_TEXT,
        placeholder_color: C_MUTED,
        handle_color: C_MUTED,
        background: Background::Color(C_SURFACE),
        border: Border {
            color: C_BORDER,
            width: 1.0,
            radius: Radius::from(RADIUS_SM),
        },
    };
    match status {
        pick_list::Status::Hovered => s.border.color = C_BORDER_STRONG,
        pick_list::Status::Opened { .. } => s.border.color = p.primary.base.color,
        pick_list::Status::Active => {}
    }
    s
}

/// Progress bar: pill track + accent fill.
pub fn progress_style(theme: &Theme) -> progress_bar::Style {
    let p = theme.extended_palette();
    progress_bar::Style {
        background: Background::Color(C_BORDER),
        bar: Background::Color(p.primary.base.color),
        border: Border {
            radius: Radius::from(RADIUS_PILL),
            ..Border::default()
        },
    }
}
