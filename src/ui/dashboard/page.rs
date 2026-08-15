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

/// Two-column masonry of provider cards. Cards keep their natural height and
/// are dropped into the currently-shorter column, so a short (no-data) card
/// fills the vertical gap left by a taller neighbor instead of a rigid row grid
/// that stretches every card to its row's tallest card.
fn card_grid(app: &mut RTokenApp, cx: &mut Context<RTokenApp>) -> impl IntoElement {
    let tab_label = app.state.time_tab.label();
    let providers = app.state.provider_selection.enabled();

    // Greedy masonry: assign each card to the shorter column, using a height
    // hint so cards with a per-model section balance across both columns.
    let mut columns: [Vec<(Provider, SumStats)>; 2] = [Vec::new(), Vec::new()];
    let mut column_heights = [0u64, 0u64];
    for provider in providers {
        let stats = app
            .state
            .by_provider
            .iter()
            .find(|(p, _)| *p == provider)
            .map(|(_, s)| *s)
            .unwrap_or_default();
        let models = app.state.by_provider_model.get(&provider);
        let column = if column_heights[0] <= column_heights[1] { 0 } else { 1 };
        column_heights[column] += card_height_hint(models);
        columns[column].push((provider, stats));
    }

    v_flex()
        .id("dashboard-grid")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_4()
        .child(
            h_flex()
                .items_start()
                .gap_4()
                .children(columns.iter().filter(|c| !c.is_empty()).map(|column| {
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_4()
                        .children(column.iter().map(|(provider, stats)| {
                            provider_card(
                                cx,
                                app.weak_self.clone(),
                                *provider,
                                *stats,
                                app.state.by_provider_model.get(provider),
                                app.state.expanded_provider == Some(*provider),
                                tab_label,
                            )
                        }))
                })),
        )
}

/// Rough relative height of a card, used only to balance the masonry columns.
/// Cards carrying a per-model breakdown are noticeably taller than empty ones;
/// the expanded list is ignored so toggling it never reflows a card columns.
fn card_height_hint(models: Option<&Vec<(String, SumStats)>>) -> u64 {
    if models.is_some_and(|m| !m.is_empty()) {
        2
    } else {
        1
    }
}
