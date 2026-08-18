//! Charts page: daily usage trends as line/bar charts per provider and model.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window,
};
use gpui_component::date_picker::DatePicker;
use gpui_component::select::Select;
use gpui_component::{h_flex, v_flex, ActiveTheme, StyledExt};

use crate::app::app::RTokenApp;
use crate::app::state::{ChartApp, ChartMetric, ChartRange, ChartsSnapshot, ChartsState};
use crate::core::aggregation::SumStats;
use crate::core::model::Provider;

use super::compact::CompactBarChart;
use super::DonutChart;
use crate::ui::format::{format_cost_f64, format_percent_f64, format_tokens_compact_f64};

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);

    // Snapshot the state we render; controls dispatch via their own entities,
    // and the charts render from owned clones so no borrow outlives this frame.
    let data = app.state.charts.data.clone();
    let metric = app.state.charts.metric;
    let app_filter = app.state.charts.app;
    let colors = series_colors(cx);

    let metric_select = app.chart_metric_select.clone();
    let app_select = app.chart_app_select.clone();
    let range_select = app.chart_range_select.clone();
    let range_picker = app.chart_range_picker.clone();
    let custom_active = app.state.charts.range == ChartRange::Custom;
    let bucket = trend_bucket(&app.state.charts);

    v_flex()
        .id("rtoken-charts-page")
        .flex_1()
        .min_w_0()
        .p_4()
        .gap_3()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .flex_wrap()
                .child(seg_label(p.muted_foreground, "时间范围"))
                .child(div().w(px(200.0)).child(Select::new(&range_select)))
                .when(custom_active, |this| {
                    this.child(
                        div().w(px(240.0)).child(
                            DatePicker::new(&range_picker)
                                .cleanable(true)
                                .placeholder("请选择日期"),
                        ),
                    )
                })
                .child(seg_label(p.muted_foreground, "指标"))
                .child(div().w(px(140.0)).child(Select::new(&metric_select)))
                .child(seg_label(p.muted_foreground, "Agent"))
                .child(div().w(px(160.0)).child(Select::new(&app_select))),
        )
        .child(main_section(&data, metric, bucket, cx))
        .child(model_section(&data, metric, app_filter, &colors, cx))
        .into_any_element()
}

fn seg_label(color: Hsla, text: &str) -> impl IntoElement {
    div().text_xs().text_color(color).child(text.to_string())
}

/// X-axis bucket granularity for the trend chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Day,
    Week,
    Month,
}

/// Bucket granularity for the active charts range: short ranges stay daily,
/// 30/90-day ranges roll up to weeks, and a year rolls up to months.
fn trend_bucket(charts: &ChartsState) -> Bucket {
    let days = if let Some((start, end)) = charts.custom_range {
        (end - start).num_days() + 1
    } else {
        match charts.range {
            ChartRange::Last7 => 7,
            ChartRange::Last30 => 30,
            ChartRange::ThisYear => 366,
            // Custom without a chosen range yet: fall back to the placeholder
            // last-7-day window.
            ChartRange::Custom => 7,
        }
    };
    if days <= 14 {
        Bucket::Day
    } else if days <= 120 {
        Bucket::Week
    } else {
        Bucket::Month
    }
}

/// Map a `YYYY-MM-DD` day key onto its bucket label.
fn bucket_key(day: &str, bucket: Bucket) -> String {
    match bucket {
        Bucket::Day => day.to_string(),
        Bucket::Week => match NaiveDate::parse_from_str(day, "%Y-%m-%d") {
            Ok(date) => {
                let monday = date - Duration::days(date.weekday().num_days_from_monday() as i64);
                monday.format("%m-%d").to_string()
            }
            Err(_) => day.to_string(),
        },
        Bucket::Month => day.get(..7).unwrap_or(day).to_string(),
    }
}

fn main_section(
    data: &Option<ChartsSnapshot>,
    metric: ChartMetric,
    bucket: Bucket,
    cx: &Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let content = match data {
        Some(snap) => {
            let series = aggregate_bucketed(&snap.provider_series, bucket);
            if series.is_empty() {
                empty_hint("暂无数据", p.muted_foreground)
            } else {
                daily_bar_chart(&series, metric, "chart-main-bar")
            }
        }
        None => empty_hint("加载中…", p.muted_foreground),
    };
    chart_card(cx, "用量趋势", content)
}

fn model_section(
    data: &Option<ChartsSnapshot>,
    metric: ChartMetric,
    app: ChartApp,
    colors: &[Hsla],
    cx: &Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let content = match data {
        Some(snap) => {
            let totals = model_totals(&snap.model_series, metric);
            if totals.is_empty() {
                empty_hint("暂无数据", p.muted_foreground)
            } else {
                let donut_data: Vec<(String, u64)> = totals
                    .iter()
                    .map(|(m, s)| (m.clone(), metric_value_u64(metric, *s)))
                    .collect();
                h_flex()
                    .gap_6()
                    .items_center()
                    .h_full()
                    .child(
                        DonutChart::new(donut_data)
                            .colors(colors.to_vec())
                            .id("chart-model-donut"),
                    )
                    .child(model_list(&totals, metric, colors, cx))
                    .into_any_element()
            }
        }
        None => empty_hint("加载中…", p.muted_foreground),
    };
    chart_card(cx, &format!("按模型（{}）", app.label()), content)
}

fn chart_card(cx: &Context<RTokenApp>, title: &str, body: AnyElement) -> AnyElement {
    let p = crate::ui::palette(cx);
    v_flex()
        .flex_1()
        .min_h_0()
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .p_3()
        .gap_2()
        .child(div().font_semibold().text_sm().child(title.to_string()))
        .child(div().flex_1().min_h_0().child(body))
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

fn series_colors(cx: &Context<RTokenApp>) -> Vec<Hsla> {
    let t = cx.theme();
    vec![
        t.chart_1,
        t.chart_2,
        t.chart_3,
        t.chart_4,
        t.chart_5,
        Hsla {
            h: 200.0,
            s: 0.6,
            l: 0.55,
            a: 1.0,
        },
    ]
}

fn metric_value(metric: ChartMetric, s: SumStats) -> f64 {
    match metric {
        ChartMetric::TotalTokens => s.total_tokens() as f64,
        ChartMetric::OutputTokens => s.output_tokens as f64,
        ChartMetric::InputTokens => s.input_tokens as f64,
        ChartMetric::CacheRead => s.cache_read_tokens as f64,
        ChartMetric::CacheHitRate => {
            let denom = s.input_tokens + s.cache_read_tokens;
            if denom == 0 {
                0.0
            } else {
                s.cache_read_tokens as f64 / denom as f64 * 100.0
            }
        }
        ChartMetric::Cost => s.cost_micros as f64 / 1e6,
    }
}

/// Integer magnitude for the donut chart; cost uses micros so proportions
/// match, and the cache-hit rate uses its denominator (input + cache-read).
fn metric_value_u64(metric: ChartMetric, s: SumStats) -> u64 {
    match metric {
        ChartMetric::TotalTokens => s.total_tokens(),
        ChartMetric::OutputTokens => s.output_tokens,
        ChartMetric::InputTokens => s.input_tokens,
        ChartMetric::CacheRead => s.cache_read_tokens,
        ChartMetric::CacheHitRate => s.input_tokens + s.cache_read_tokens,
        ChartMetric::Cost => s.cost_micros,
    }
}

/// Sum every provider's per-day series onto one chronological bucket domain as
/// raw `SumStats`, so rate metrics are computed on the aggregate rather than
/// summed across providers.
fn aggregate_bucketed(
    provider_series: &[(Provider, Vec<(String, SumStats)>)],
    bucket: Bucket,
) -> Vec<(String, SumStats)> {
    let mut map: BTreeMap<String, SumStats> = BTreeMap::new();
    for (_, series) in provider_series {
        for (day, stats) in series {
            map.entry(bucket_key(day, bucket)).or_default().add(stats);
        }
    }
    map.into_iter().collect()
}

/// Per-model totals over the window, sorted by the selected metric descending.
fn model_totals(
    model_series: &BTreeMap<String, Vec<(String, SumStats)>>,
    metric: ChartMetric,
) -> Vec<(String, SumStats)> {
    let mut totals: Vec<(String, SumStats)> = model_series
        .iter()
        .map(|(model, series)| {
            let total = series.iter().fold(SumStats::default(), |mut acc, (_, s)| {
                acc.add(s);
                acc
            });
            (model.clone(), total)
        })
        .collect();
    totals.sort_by(|a, b| {
        metric_value(metric, b.1)
            .partial_cmp(&metric_value(metric, a.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    totals
}

/// Value formatter chosen by metric: token counts get the compact M/亿/K
/// treatment, cost is shown as dollars.
fn formatter_for(metric: ChartMetric) -> fn(f64) -> String {
    match metric {
        ChartMetric::Cost => format_cost_f64,
        ChartMetric::CacheHitRate => format_percent_f64,
        _ => format_tokens_compact_f64,
    }
}

/// Single-series daily bar chart (raw per-day stats → metric values).
fn daily_bar_chart(
    daily: &[(String, SumStats)],
    metric: ChartMetric,
    id: &'static str,
) -> AnyElement {
    let days: Vec<String> = daily.iter().map(|(d, _)| d.clone()).collect();
    let values: Vec<f64> = daily
        .iter()
        .map(|(_, s)| metric_value(metric, *s))
        .collect();
    CompactBarChart::new(days, values, formatter_for(metric))
        .id(id)
        .into_any_element()
}

/// Scrollable per-model list: a colour swatch + model name on the left, the
/// metric value on the right.
fn model_list(
    totals: &[(String, SumStats)],
    metric: ChartMetric,
    colors: &[Hsla],
    cx: &Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let fmt = formatter_for(metric);
    v_flex()
        .id("chart-model-list")
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_y_scroll()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .pb_1()
                .child(div().text_xs().text_color(p.muted_foreground).child("模型"))
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(metric.label()),
                ),
        )
        .children(totals.iter().enumerate().map(|(i, (model, stats))| {
            let value = metric_value(metric, *stats);
            let color = colors[i % colors.len()];
            h_flex()
                .gap_2()
                .items_center()
                .justify_between()
                .py_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .min_w_0()
                        .child(
                            div()
                                .w(px(10.0))
                                .h(px(10.0))
                                .rounded_full()
                                .bg(color)
                                .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.foreground)
                                .truncate()
                                .child(model.clone()),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .text_color(p.foreground)
                        .child(fmt(value)),
                )
                .into_any_element()
        }))
        .into_any_element()
}
