//! GPUI views: pages, shared UI helpers, and chart components.

pub mod charts;
pub mod dashboard;
pub mod format;
pub mod project;
pub mod report;
pub mod settings;
pub mod topbar;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, App, Context, Div, InteractiveElement, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, Window,
};
use gpui::{Hsla, Pixels};
use gpui_component::{v_flex, ActiveTheme, StyledExt};

use crate::app::app::RTokenApp;
use crate::app::state::ActivePage;

/// Convert an `#rrggbb` hex color to GPUI's `Hsla`. GPUI stores the hue
/// normalized to 0..=1 (not degrees), so values written as degrees render as
/// the wrong hue — this helper keeps every color literal in hex and converts
/// once.
pub(crate) fn hsla_from_hex(hex: u32) -> Hsla {
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        (h, s)
    };
    Hsla { h, s, l, a: 1.0 }
}

/// Copied theme colors used by the pages, avoiding long-lived `cx` borrows.
pub(crate) struct Palette {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
    /// Muted surface (background) color — not for text.
    pub muted: Hsla,
    /// Text color for secondary/label text (readable on dark backgrounds).
    pub muted_foreground: Hsla,
    /// Card-level surface, one step lighter than `background`.
    pub card: Hsla,
    pub radius: Pixels,
}

pub(crate) fn palette(cx: &App) -> Palette {
    let theme = cx.theme();
    Palette {
        background: theme.background,
        foreground: theme.foreground,
        border: theme.border,
        muted: theme.muted,
        muted_foreground: theme.muted_foreground,
        card: theme.secondary,
        radius: theme.radius,
    }
}

/// Render the page active in `app.state`.
pub fn router(app: &mut RTokenApp, window: &mut Window, cx: &mut Context<RTokenApp>) -> AnyElement {
    match app.state.active_page {
        ActivePage::Dashboard => dashboard::render_page(app, window, cx),
        ActivePage::Project => project::render_page(app, window, cx),
        ActivePage::Settings => settings::render_page(app, window, cx),
        ActivePage::Charts => charts::page::render_page(app, window, cx),
        ActivePage::Report => report::page::render_page(app, window, cx),
    }
}

/// Standard page container: a scrollable column with a title header.
pub(crate) fn page_shell(
    cx: &Context<RTokenApp>,
    title: &str,
    subtitle: Option<&str>,
) -> Stateful<Div> {
    let p = palette(cx);
    v_flex()
        .id("rtoken-page")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .p_4()
        .gap_4()
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_lg()
                        .font_bold()
                        .text_color(p.foreground)
                        .child(title.to_string()),
                )
                .when_some(subtitle, |this, subtitle| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child(subtitle.to_string()),
                    )
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_converts_to_normalized_hsla() {
        let red = hsla_from_hex(0xff0000);
        assert!((red.h - 0.0).abs() < 1e-4);
        assert!((red.s - 1.0).abs() < 1e-4);
        assert!((red.l - 0.5).abs() < 1e-4);

        let green = hsla_from_hex(0x00ff00);
        assert!((green.h - 1.0 / 3.0).abs() < 1e-4);

        let white = hsla_from_hex(0xffffff);
        assert_eq!(white.s, 0.0);
        assert_eq!(white.l, 1.0);

        // GitHub dark green #0e4429 is hue ~150 degrees => ~0.417 normalized.
        let github_green = hsla_from_hex(0x0e4429);
        assert!((github_green.h - 150.0 / 360.0).abs() < 1e-3);
    }
}
