//! Settings page: application selection (which apps to track and their order).

use gpui::{div, AnyElement, App, Context, IntoElement, ParentElement, Styled, WeakEntity, Window};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::{h_flex, v_flex, Disableable, IconName, StyledExt};

use crate::app::app::RTokenApp;
use crate::core::model::Provider;

use crate::ui::page_shell;

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    page_shell(cx, "Settings", "Application configuration")
        .child(applications_panel(app, cx))
        .into_any_element()
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
                .child("Applications"),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("Choose which apps to track and their order on the dashboard."),
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
