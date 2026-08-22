//! Dashboard page: time-range tabs, the report section (summary + heatmap) on
//! top, and the auto-discovered agent usage cards below.

use gpui::{
    div, AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window,
};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{h_flex, v_flex, StyledExt};

use crate::app::app::TokenMonitorApp;
use crate::app::state::TimeTab;
use crate::core::aggregation::SumStats;
use crate::core::model::Provider;
use crate::ui::report::section::{empty_hint, report_section};

use super::card::provider_card;

pub fn render_page(
    app: &mut TokenMonitorApp,
    _window: &mut Window,
    cx: &mut Context<TokenMonitorApp>,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_h_0()
        .child(tab_bar(app, cx))
        .child(content(app, cx))
        .into_any_element()
}

/// Scrollable page body: report section (fixed 365-day overview) on top, then
/// the per-agent usage cards for the selected time range.
fn content(app: &mut TokenMonitorApp, cx: &mut Context<TokenMonitorApp>) -> AnyElement {
    let report = report_section(app, cx);
    let agents = agent_section(app, cx);
    v_flex()
        .id("dashboard-content")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_4()
        .gap_4()
        .child(report)
        .child(agents)
        .into_any_element()
}

/// Segmented time-range tab bar (今日/昨日/本周/上周/本月/本年).
///
/// Rendered through gpui-component's `TabBar`: it assigns each `Tab` a distinct
/// per-index element id (`ix`) and a `TabList`/`Tab` role, so every tab gets a
/// unique a11y node id. Bare `Tab::new()` children would all default to
/// `ix == 0` and collide with the same a11y node id.
fn tab_bar(app: &TokenMonitorApp, cx: &Context<TokenMonitorApp>) -> impl IntoElement {
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

/// The "Agent 用量" section: only agents with recorded usage in the selected
/// window are shown, in cost-descending order (as returned by the DB query).
fn agent_section(app: &mut TokenMonitorApp, cx: &mut Context<TokenMonitorApp>) -> AnyElement {
    let p = crate::ui::palette(cx);
    let tab_label = app.state.time_tab.label();
    let used: Vec<(Provider, SumStats)> = app
        .state
        .by_provider
        .iter()
        .filter(|(_, s)| s.records > 0)
        .map(|(p, s)| (*p, *s))
        .collect();

    let grid: AnyElement = if used.is_empty() {
        empty_hint("所选时间范围内暂无 Agent 使用记录", p.muted_foreground)
    } else {
        // Greedy masonry: assign each card to the shorter column, using a
        // height hint so cards with a per-model section balance across columns.
        let mut columns: [Vec<(Provider, SumStats)>; 2] = [Vec::new(), Vec::new()];
        let mut column_heights = [0u64, 0u64];
        for (provider, stats) in &used {
            let models = app.state.by_provider_model.get(provider);
            let column = if column_heights[0] <= column_heights[1] {
                0
            } else {
                1
            };
            column_heights[column] += card_height_hint(models);
            columns[column].push((*provider, *stats));
        }

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
            }))
            .into_any_element()
    };

    v_flex()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(p.foreground)
                .child(format!("Agent 用量（{tab_label}）")),
        )
        .child(grid)
        .into_any_element()
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
