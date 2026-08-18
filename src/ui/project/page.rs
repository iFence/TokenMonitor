use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, IntoElement, ParentElement, Styled, Window};
use gpui_component::{h_flex, v_flex};

use crate::app::app::RTokenApp;

use crate::ui::format::format_tokens_compact;
use crate::ui::page_shell;

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let rows = &app.state.by_project;

    page_shell(cx, "项目", Some("按代码项目分组的 Token 用量"))
        .child(
            v_flex()
                .rounded(p.radius)
                .border_1()
                .border_color(p.border)
                .flex_shrink_0()
                .overflow_hidden()
                .children(rows.iter().map(|(project, s)| {
                    h_flex()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(p.border)
                        .children([
                            div().flex_1().text_sm().child(project.clone()),
                            div()
                                .w(px(120.0))
                                .text_sm()
                                .child(format!("{} records", s.records)),
                            div().w(px(140.0)).text_sm().child(format!(
                                "{} tokens",
                                format_tokens_compact(s.total_tokens())
                            )),
                        ])
                }))
                .when(rows.is_empty(), |this| {
                    this.child(
                        div()
                            .p_3()
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child("No project usage recorded yet — run a scan."),
                    )
                }),
        )
        .into_any_element()
}
