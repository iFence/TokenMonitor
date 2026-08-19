//! Report page: usage summary panel plus the native contribution heatmap.

use std::rc::Rc;

use chrono::{NaiveDate, Utc};
use gpui::{
    div, px, AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window,
};
use gpui_component::{h_flex, v_flex, StyledExt};

use super::heatmap::{legend, ContributionHeatmap, HoverCallback, ResizeCallback};
use super::stats::{report_stats, ReportStats};
use crate::app::app::RTokenApp;
use crate::app::state::ReportHover;
use crate::core::aggregation::SumStats;
use crate::core::time::east8_local;
use crate::ui::format::{format_cost_f64, format_tokens_compact_f64};

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let data = app.state.report.data.clone();

    let content = match &data {
        Some(snap) => {
            let today = east8_local(Utc::now()).date_naive();
            let stats = report_stats(&snap.days, today);
            let hover = app.state.report_hover;
            let on_hover = hover_callback(cx);
            let on_resize = resize_callback(cx);
            v_flex()
                .gap_4()
                .child(summary_panel(&stats, cx))
                .child(heatmap_card(&snap.days, hover, &on_hover, &on_resize, cx))
                .into_any_element()
        }
        None => empty_hint("加载中…", p.muted_foreground),
    };

    v_flex()
        .id("rtoken-page")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .p_4()
        .gap_4()
        .child(content)
        .into_any_element()
}

/// Wire heatmap hover events into the app-level tooltip state so the view
/// re-renders with the floating tooltip pinned to the hovered cell.
fn hover_callback(cx: &Context<RTokenApp>) -> Rc<HoverCallback> {
    let weak = cx.weak_entity();
    Rc::new(move |is_hovered, bounds, date, stats, _window, cx| {
        let _ = weak.update(cx, |app, cx| {
            if is_hovered {
                app.state.report_hover = Some(ReportHover {
                    date,
                    stats,
                    bounds,
                });
            } else {
                app.state.report_hover = None;
            }
            cx.notify();
        });
    })
}

/// Re-render the page when the heatmap measures a new width (window resize),
/// so the grid's cell size follows the card.
fn resize_callback(cx: &Context<RTokenApp>) -> Rc<ResizeCallback> {
    let weak = cx.weak_entity();
    Rc::new(move |_window, cx| {
        let _ = weak.update(cx, |_, cx| cx.notify());
    })
}

/// Six summary cards: totals, activity, streaks, and the busiest day.
fn summary_panel(stats: &ReportStats, cx: &Context<RTokenApp>) -> impl IntoElement {
    let cards = [
        (
            "总 Token",
            format_tokens_compact_f64(stats.total.total_tokens() as f64),
        ),
        (
            "总花费",
            format_cost_f64(stats.total.cost_micros as f64 / 1e6),
        ),
        ("活跃天数", format!("{} 天", stats.active_days)),
        ("最长连续", format!("{} 天", stats.longest_streak)),
        ("当前连续", format!("{} 天", stats.current_streak)),
        ("最忙一天", busiest_label(stats)),
    ];
    h_flex().gap_3().flex_wrap().children(
        cards
            .into_iter()
            .map(|(label, value)| stat_card(cx, label, value)),
    )
}

fn stat_card(cx: &Context<RTokenApp>, label: &str, value: String) -> impl IntoElement {
    let p = crate::ui::palette(cx);
    v_flex()
        .flex_1()
        .min_w(px(150.0))
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .p_3()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(p.muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_lg()
                .font_bold()
                .text_color(p.foreground)
                .child(value),
        )
}

fn busiest_label(stats: &ReportStats) -> String {
    match stats.busiest {
        Some((date, s)) => format!(
            "{} · {}",
            date.format("%m-%d"),
            format_tokens_compact_f64(s.total_tokens() as f64)
        ),
        None => "—".to_string(),
    }
}

/// The heatmap card: header with legend on the right, grid below.
fn heatmap_card(
    days: &[(NaiveDate, SumStats)],
    hover: Option<ReportHover>,
    on_hover: &Rc<HoverCallback>,
    on_resize: &Rc<ResizeCallback>,
    cx: &Context<RTokenApp>,
) -> impl IntoElement {
    let p = crate::ui::palette(cx);

    v_flex()
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .p_3()
        .gap_3()
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().font_semibold().text_sm().child("每日用量热力图")),
                )
                .child(legend(&p)),
        )
        .child(if days.is_empty() {
            empty_hint("暂无数据，扫描完成后将在这里显示热力图", p.muted_foreground)
        } else {
            ContributionHeatmap::new(days.to_vec()).render(hover, on_hover, on_resize, cx)
        })
        .into_any_element()
}

fn empty_hint(text: &str, color: Hsla) -> AnyElement {
    div()
        .p_4()
        .text_sm()
        .text_color(color)
        .child(text.to_string())
        .into_any_element()
}
