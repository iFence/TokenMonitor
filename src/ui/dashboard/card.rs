//! Provider usage card for the dashboard grid.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, size, svg, AnyElement, App, Context, ElementId, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, WeakEntity,
};
use gpui_component::{h_flex, v_flex, ActiveTheme, StyledExt};

use crate::app::app::TokenMonitorApp;
use crate::core::aggregation::SumStats;
use crate::core::model::Provider;
use crate::format::{format_cost_usd, format_tokens_compact};
use crate::ui::charts::DonutChart;

/// One provider card: headline totals, cost, cache-hit ring, token details,
/// and an expandable per-model breakdown.
pub fn provider_card(
    cx: &mut Context<TokenMonitorApp>,
    weak: WeakEntity<TokenMonitorApp>,
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
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(provider_logo(cx, provider))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(p.foreground)
                                .child(provider.display_name()),
                        ),
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

/// 16px provider logo rendered before the card name. Each SVG is embedded at
/// compile time and drawn in the theme's foreground color (`currentColor`);
/// hardcoded fills (e.g. `#fff` on some brand assets) are remapped so logos
/// stay visible on both light and dark themes. Falls back to an empty spacer
/// when a tool has no bundled asset, keeping card headers aligned.
fn provider_logo(cx: &Context<TokenMonitorApp>, provider: Provider) -> AnyElement {
    let p = crate::ui::palette(cx);
    match provider_icon_bytes(provider) {
        Some(bytes) => {
            let mono = mono_svg(std::str::from_utf8(bytes).unwrap_or(""));
            svg()
                .data(mono.as_bytes())
                .w(px(16.0))
                .h(px(16.0))
                .text_color(p.foreground)
                .into_any_element()
        }
        None => div().w(px(16.0)).h(px(16.0)).into_any_element(),
    }
}

/// Raw SVG bytes for a provider's logo, or `None` when no asset is bundled.
fn provider_icon_bytes(provider: Provider) -> Option<&'static [u8]> {
    match provider.id() {
        "claude" => Some(include_bytes!("../../../assets/icons/claude.svg")),
        "codex" => Some(include_bytes!("../../../assets/icons/codex.svg")),
        "gemini" => Some(include_bytes!("../../../assets/icons/gemini.svg")),
        "antigravity" => Some(include_bytes!("../../../assets/icons/antigravity.svg")),
        "codebuddy" => Some(include_bytes!("../../../assets/icons/codebuddy.svg")),
        "workbuddy" => Some(include_bytes!("../../../assets/icons/workbuddy.svg")),
        "opencode" => Some(include_bytes!("../../../assets/icons/opencode.svg")),
        "qoder" => Some(include_bytes!("../../../assets/icons/qoder.svg")),
        "openclaw" => Some(include_bytes!("../../../assets/icons/openclaw.svg")),
        "deepseek" => Some(include_bytes!("../../../assets/icons/deepseek.svg")),
        "pi" => Some(include_bytes!("../../../assets/icons/pi.svg")),
        "trae" => Some(include_bytes!("../../../assets/icons/trae.svg")),
        _ => None,
    }
}

/// Force an SVG to a single themeable fill: remap hardcoded brand fills (e.g.
/// `#fff` on some assets) to `currentColor` so `gpui::svg().data(...)` paints
/// the logo in the theme's foreground color instead of a fixed white/black.
fn mono_svg(raw: &str) -> String {
    raw.replace("#ffffff", "currentColor")
        .replace("#fff", "currentColor")
        .replace("white", "currentColor")
}

/// Small donut ring showing cache-read share of (input + cache_read), with a
/// side label, panel.png style. The hit arc uses the app's accent color;
/// zero-usage cards get a grey placeholder ring.
fn cache_hit_ring(
    cx: &Context<TokenMonitorApp>,
    provider: Provider,
    stats: &SumStats,
) -> impl IntoElement {
    let p = crate::ui::palette(cx);
    let denom = stats.input_tokens + stats.cache_read_tokens;

    let (data, colors, pct_text) = if denom > 0 {
        let accent = cx.theme().primary;
        (
            vec![
                ("hit".to_string(), stats.cache_read_tokens),
                ("miss".to_string(), stats.input_tokens),
            ],
            vec![accent, p.background],
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

fn detail_cell(cx: &Context<TokenMonitorApp>, label: &str, value: u64) -> impl IntoElement {
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
    cx: &Context<TokenMonitorApp>,
    weak: WeakEntity<TokenMonitorApp>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tracked_provider_has_a_bundled_logo() {
        for provider in Provider::ALL {
            assert!(
                provider_icon_bytes(provider).is_some(),
                "missing logo for {}",
                provider.id()
            );
        }
    }

    #[test]
    fn mono_svg_remaps_hardcoded_fills_to_current_color() {
        assert_eq!(
            mono_svg(r##"<path fill="#fff"/>"##),
            r##"<path fill="currentColor"/>"##
        );
        assert_eq!(
            mono_svg(r##"<path fill="#ffffff"/>"##),
            r##"<path fill="currentColor"/>"##
        );
        assert_eq!(
            mono_svg(r##"<path fill="white"/>"##),
            r##"<path fill="currentColor"/>"##
        );
        // Already-themeable fills are left untouched.
        assert_eq!(
            mono_svg(r##"<path fill="currentColor"/>"##),
            r##"<path fill="currentColor"/>"##
        );
    }
}
