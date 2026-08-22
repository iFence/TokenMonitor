//! Report section: usage summary panel plus the native contribution heatmap.
//! Embedded at the top of the dashboard page; the standalone report page is
//! gone, so this renders bare content (no page shell or scrolling).

use std::rc::Rc;

use chrono::{NaiveDate, Utc};
use gpui::{
    div, px, AnyElement, Bounds, Context, Hsla, IntoElement, ParentElement, Pixels, Styled,
};
use gpui_component::{h_flex, v_flex, StyledExt};

use super::heatmap::{legend, ContributionHeatmap, HoverCallback, ResizeCallback};
use super::stats::{report_stats, ReportStats};
use crate::app::app::RTokenApp;
use crate::app::state::ReportHover;
use crate::core::aggregation::SumStats;
use crate::core::time::east8_local;
use crate::ui::format::{format_cost_f64, format_tokens_compact_f64};

/// The report content: summary cards on top, 365-day heatmap below.
pub fn report_section(app: &RTokenApp, cx: &Context<RTokenApp>) -> AnyElement {
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
                .child(heatmap_card(
                    &snap.days,
                    app.state.report.heatmap_bounds,
                    hover,
                    &on_hover,
                    &on_resize,
                    cx,
                ))
                .into_any_element()
        }
        None => empty_hint("加载中…", p.muted_foreground),
    };

    v_flex().gap_4().child(content).into_any_element()
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

/// Re-render the page when the heatmap's measured bounds change (window
/// resize / card reflow), so the grid's cell size and tooltip anchor follow
/// the card. Change detection makes the loop converge: after the re-render
/// uses the stored bounds, prepaint reports the same bounds and stops
/// scheduling.
///
/// The callback runs inside `on_prepaint`, i.e. mid-draw, where `cx.notify()`
/// is dropped (GPUI skips scheduling a redraw while the window is drawing), so
/// the first render of the page would keep the grid at its minimum cell size
/// until some input forced a redraw. Deferring the notify to the next frame
/// via [`Window::on_next_frame`] guarantees the grid always re-renders at the
/// freshly measured size.
fn resize_callback(cx: &Context<RTokenApp>) -> Rc<ResizeCallback> {
    let weak = cx.weak_entity();
    Rc::new(move |bounds, window, cx| {
        let weak = weak.clone();
        let _ = weak.update(cx, |app, _cx| {
            let prev = app.state.report.heatmap_bounds;
            if (prev.size.width.as_f32() - bounds.size.width.as_f32()).abs() > 0.5
                || (prev.origin.x.as_f32() - bounds.origin.x.as_f32()).abs() > 0.5
                || (prev.origin.y.as_f32() - bounds.origin.y.as_f32()).abs() > 0.5
            {
                app.state.report.heatmap_bounds = bounds;
                let next = weak.clone();
                window.on_next_frame(move |_window, cx| {
                    let _ = next.update(cx, |_, cx| cx.notify());
                });
            }
        });
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
    bounds: Bounds<Pixels>,
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
            ContributionHeatmap::new(days.to_vec()).render(bounds, hover, on_hover, on_resize, cx)
        })
        .into_any_element()
}

pub(crate) fn empty_hint(text: &str, color: Hsla) -> AnyElement {
    div()
        .p_4()
        .text_sm()
        .text_color(color)
        .child(text.to_string())
        .into_any_element()
}
