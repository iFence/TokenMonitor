//! Settings page: app selection (which apps to track and their order) and about.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::{h_flex, v_flex, Disableable, IconName, StyledExt};

use crate::app::app::RTokenApp;
use crate::app::state::SettingsGroup;
use crate::core::model::Provider;

use crate::ui::page_shell;

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let weak = app.weak_self.clone();
    let group = app.state.settings_group;

    page_shell(cx, "设置", "应用配置与关于")
        .child(
            h_flex()
                .gap_4()
                .items_start()
                .child(sidebar(&weak, group, cx))
                .child(div().flex_1().min_w_0().child(group_content(app, group, cx))),
        )
        .into_any_element()
}

/// Left-hand group navigation.
fn sidebar(
    weak: &WeakEntity<RTokenApp>,
    current: SettingsGroup,
    cx: &Context<RTokenApp>,
) -> impl IntoElement {
    let p = crate::ui::palette(cx);
    v_flex()
        .w(px(160.0))
        .flex_shrink_0()
        .gap_1()
        .children(SettingsGroup::ALL.iter().map(|&group| {
            let selected = group == current;
            let weak = weak.clone();
            div()
                .id(format!("settings-group-{:?}", group))
                .cursor_pointer()
                .px_3()
                .py_2()
                .rounded(p.radius)
                .text_sm()
                .font_medium()
                .text_color(if selected { p.foreground } else { p.muted_foreground })
                .when(selected, |this| this.bg(p.muted).font_semibold())
                .child(group.label())
                .on_click(move |_, _: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| this.select_settings_group(group, cx));
                })
        }))
}

/// Right-hand content for the selected group.
fn group_content(
    app: &mut RTokenApp,
    group: SettingsGroup,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    match group {
        SettingsGroup::Applications => applications_panel(app, cx),
        SettingsGroup::About => about_panel(cx),
    }
}

/// Panel listing every tracked app with a keep toggle and up/down reordering.
fn applications_panel(app: &mut RTokenApp, cx: &mut Context<RTokenApp>) -> AnyElement {
    let p = crate::ui::palette(cx);
    let weak = app.weak_self.clone();
    let entries = app.state.provider_selection.entries.clone();
    let last = entries.len().saturating_sub(1);

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
                .child("应用"),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("选择要追踪的应用，以及它们在仪表盘上的显示顺序。"),
        )
        .children(entries.iter().enumerate().map(|(i, entry)| {
            let provider = entry.provider;
            let enabled = entry.enabled;

            h_flex()
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .child(
                    Checkbox::new(format!("app-toggle-{}", provider.id()))
                        .checked(enabled)
                        .label(provider.display_name())
                        .on_click({
                            let weak = weak.clone();
                            move |checked, _: &mut Window, cx: &mut App| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.set_provider_enabled(provider, *checked, cx);
                                });
                            }
                        }),
                )
                .child(div().flex_1())
                .child(reorder_button(&weak, provider, -1, i == 0))
                .child(reorder_button(&weak, provider, 1, i == last))
                .into_any_element()
        }))
        .into_any_element()
}

/// About panel: app name, version, and a short description.
fn about_panel(cx: &Context<RTokenApp>) -> AnyElement {
    let p = crate::ui::palette(cx);
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
                .child("关于"),
        )
        .child(
            div()
                .text_lg()
                .font_bold()
                .text_color(p.foreground)
                .child("rToken"),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(format!("版本 {}", env!("CARGO_PKG_VERSION"))),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(
                    "本地运行的 AI 编程工具 Token 用量追踪桌面应用，聚合展示 Claude Code、Codex、Gemini CLI、CodeBuddy、OpenCode、Qoder 的用量与成本。",
                ),
        )
        .into_any_element()
}

fn reorder_button(
    weak: &WeakEntity<RTokenApp>,
    provider: Provider,
    dir: isize,
    disabled: bool,
) -> Button {
    let icon = if dir < 0 {
        IconName::ChevronUp
    } else {
        IconName::ChevronDown
    };
    let weak = weak.clone();
    Button::new(format!("app-move-{}-{}", provider.id(), dir))
        .ghost()
        .icon(icon)
        .disabled(disabled)
        .on_click(move |_, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.move_provider(provider, dir, cx));
        })
}
