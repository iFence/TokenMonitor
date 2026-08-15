//! GPUI views: pages, shared UI helpers, and chart components.

pub mod charts;
pub mod dashboard;
pub mod format;
pub mod project;
pub mod settings;
pub mod topbar;

use gpui::{
    div, AnyElement, Context, Div, InteractiveElement, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, Window,
};
use gpui::{Hsla, Pixels};
use gpui_component::{v_flex, ActiveTheme, StyledExt};

use crate::app::app::RTokenApp;
use crate::app::state::ActivePage;

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

pub(crate) fn palette(cx: &Context<RTokenApp>) -> Palette {
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
    }
}

/// Standard page container: a scrollable column with a title header.
pub(crate) fn page_shell(cx: &Context<RTokenApp>, title: &str, subtitle: &str) -> Stateful<Div> {
    let p = palette(cx);
    v_flex()
        .id("rtoken-page")
        .flex_1()
        .min_w_0()
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
                .child(
                    div()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(subtitle.to_string()),
                ),
        )
}
