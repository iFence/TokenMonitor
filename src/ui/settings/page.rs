use gpui::prelude::FluentBuilder as _;
use gpui::{div, AnyElement, Context, IntoElement, ParentElement, Styled, Window};
use gpui_component::{v_flex, StyledExt};

use crate::app::app::RTokenApp;

use crate::ui::page_shell;

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let db_path = app.collector.db_path().display().to_string();
    let last_error = app.state.last_error.clone();

    page_shell(cx, "Settings", "Application configuration")
        .child(
            v_flex()
                .gap_2()
                .p_3()
                .rounded(p.radius)
                .border_1()
                .border_color(p.border)
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(p.foreground)
                        .child("SQLite database"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(db_path),
                )
                .when_some(last_error, |this, err| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(p.foreground)
                            .child(format!("Last error: {err}")),
                    )
                }),
        )
        .into_any_element()
}
