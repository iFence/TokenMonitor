//! Dashboard page: time-range tabs + provider usage card grid.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window,
};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{h_flex, v_flex};

use crate::app::app::RTokenApp;
use crate::app::state::TimeTab;
use crate::core::aggregation::SumStats;
use crate::core::model::Provider;

use super::card::provider_card;

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_h_0()
        .child(tab_bar(app, cx))
        .child(card_grid(app, cx))
        .into_any_element()
}

/// Segmented time-range tab bar (今日/昨日/本周/上周/本月/本年).
///
/// Rendered through gpui-component's `TabBar`: it assigns each `Tab` a distinct
/// per-index element id (`ix`) and a `TabList`/`Tab` role, so every tab gets a
/// unique a11y node id. Bare `Tab::new()` children would all default to
/// `ix == 0` and collide with the same a11y node id.
fn tab_bar(app: &RTokenApp, cx: &Context<RTokenApp>) -> impl IntoElement {
    let p = crate::ui::palette(cx);
    let weak = app.weak_self.clone();
    let selected_ix = TimeTab::ALL
        .iter()
        .position(|tab| *tab == app.state.time_tab)
        .unwrap_or(0);

    h_flex()
        .w_full()
        .px_4()
        .py_2()
        .gap_1()
        .border_b_1()
        .border_color(p.border)
        .child(
            TabBar::new("dashboard-time-tabs")
                .segmented()
                .selected_index(selected_ix)
                .children(TimeTab::ALL.iter().map(|tab| Tab::new().label(tab.label())))
                .on_click(move |ix, _, cx| {
                    if let Some(tab) = TimeTab::ALL.get(*ix) {
                        let _ = weak.update(cx, |this, cx| this.select_time_tab(*tab, cx));
                    }
                }),
        )
}

/// Two-column grid of provider cards over `Provider::ALL` (zero-usage
/// providers included).
fn card_grid(app: &mut RTokenApp, cx: &mut Context<RTokenApp>) -> impl IntoElement {
    let tab_label = app.state.time_tab.label();
    v_flex()
        .id("dashboard-grid")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_4()
        .gap_4()
        .children(Provider::ALL.chunks(2).map(|row| {
            h_flex()
                .gap_4()
                .children(row.iter().map(|provider| {
                    let stats = app
                        .state
                        .by_provider
                        .iter()
                        .find(|(p, _)| p == provider)
                        .map(|(_, s)| s.clone())
                        .unwrap_or_else(SumStats::default);
                    provider_card(
                        cx,
                        app.weak_self.clone(),
                        *provider,
                        stats,
                        app.state.by_provider_model.get(provider),
                        app.state.expanded_provider == Some(*provider),
                        tab_label,
                    )
                }))
                .into_any_element()
        }))
}
