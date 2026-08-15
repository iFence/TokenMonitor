//! Provider usage card for the dashboard grid.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, rgb, size, AnyElement, App, Context, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, WeakEntity,
};
use gpui_component::{h_flex, v_flex, StyledExt};

use crate::app::app::RTokenApp;
use crate::core::aggregation::SumStats;
use crate::core::model::Provider;
use crate::ui::charts::DonutChart;
use crate::ui::format::{format_cost_usd, format_tokens_compact};

/// One provider card: headline totals, cost, cache-hit ring, token details,
/// and an expandable per-model breakdown.
pub fn provider_card(
    cx: &mut Context<RTokenApp>,
    weak: WeakEntity<RTokenApp>,
    provider: Provider,
    stats: SumStats,
    models: Option<&Vec<(String, SumStats)>>,
    expanded: bool,
    tab_label: &str,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let has_usage = stats.records > 0;

    v_flex()
        .w_full()
        .p_4()
        .gap_3()
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        // Header: provider name + record-count badge.
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(p.foreground)
                        .child(provider.display_name()),
                )
                .when(has_usage, |this| {
                    this.child(
                        div()
                            .px_2()
                            .rounded(p.radius)
                            .bg(p.muted.opacity(0.2))
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(format!("{}", stats.records)),
                    )
                }),
        )
        // Headline total tokens.
        .child(
            h_flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .text_2xl()
                        .font_bold()
                        .text_color(p.foreground)
                        .child(format_tokens_compact(stats.total_tokens())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(format!("{tab_label} 总量")),
                ),
        )
        // Cost + cache-hit ring.
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .child("≈成本"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(p.foreground)
                                .child(format_cost_usd(stats.cost_micros)),
                        ),
                )
                .child(cache_hit_ring(cx, provider, &stats)),
        )
        // Token detail cells.
        .child(
            h_flex()
                .gap_2()
                .child(detail_cell(cx, "输入", stats.input_tokens))
                .child(detail_cell(cx, "输出", stats.output_tokens))
                .child(detail_cell(cx, "缓存读", stats.cache_read_tokens))
                .child(detail_cell(cx, "缓存写", stats.cache_write_tokens)),
        )
        // Expandable per-model breakdown.
        .when_some(models.filter(|m| !m.is_empty()), |this, models| {
            this.child(model_section(cx, weak, provider, models, expanded))
        })
        .into_any_element()
}

/// Small donut ring showing cache-read share of (input + cache_read), with a
/// side label, panel.png style. Zero-usage cards get a grey placeholder ring.
fn cache_hit_ring(
    cx: &Context<RTokenApp>,
    provider: Provider,
    stats: &SumStats,
) -> impl IntoElement {
    let p = crate::ui::palette(cx);
    let denom = stats.input_tokens + stats.cache_read_tokens;

    let (data, colors, pct_text) = if denom > 0 {
        let hit_green: Hsla = rgb(0x4ade80).into();
        (
            vec![
                ("hit".to_string(), stats.cache_read_tokens),
                ("miss".to_string(), stats.input_tokens),
            ],
            vec![hit_green, p.background],
            format!(
                "{:.0}%",
                stats.cache_read_tokens as f64 * 100.0 / denom as f64
            ),
        )
    } else {
        (
            vec![("none".to_string(), 1)],
            vec![p.background],
            "—".to_string(),
        )
    };

    h_flex()
        .gap_2()
        .items_center()
        .child(
            DonutChart::new(data)
                .colors(colors)
                .with_size(size(px(48.0), px(48.0)))
                .id(ElementId::Name(
                    format!("cache-ring-{}", provider.id()).into(),
                )),
        )
        .child(
            v_flex()
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child("Cache Hit"),
                )
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(p.foreground)
                        .child(pct_text),
                ),
        )
}

fn detail_cell(cx: &Context<RTokenApp>, label: &str, value: u64) -> impl IntoElement {
    let p = crate::ui::palette(cx);
    v_flex()
        .flex_1()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(p.muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.foreground)
                .child(format_tokens_compact(value)),
        )
}

/// The "按模型 (N)" toggle row plus the expanded per-model list.
fn model_section(
    cx: &Context<RTokenApp>,
    weak: WeakEntity<RTokenApp>,
    provider: Provider,
    models: &[(String, SumStats)],
    expanded: bool,
) -> impl IntoElement {
    let p = crate::ui::palette(cx);
    let arrow = if expanded { "▾" } else { "▸" };

    v_flex()
        .gap_1()
        .pt_2()
        .border_t_1()
        .border_color(p.border)
        .child(
            div()
                .id(ElementId::Name(
                    format!("model-toggle-{}", provider.id()).into(),
                ))
                .cursor_pointer()
                .text_xs()
                .text_color(p.muted_foreground)
                .child(format!("按模型 ({})  {arrow}", models.len()))
                .on_click(move |_, _: &mut gpui::Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        this.toggle_provider_expanded(provider, cx);
                    });
                }),
        )
        .when(expanded, |this| {
            this.children(models.iter().map(|(model, s)| {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_xs()
                            .text_color(p.foreground)
                            .child(model.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(format_tokens_compact(s.total_tokens())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(format_cost_usd(s.cost_micros)),
                    )
                    .into_any_element()
            }))
        })
}
