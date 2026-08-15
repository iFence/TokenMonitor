//! Charts page: daily usage trends as line/bar charts per provider and model.

use std::collections::{BTreeMap, BTreeSet};

use gpui::{
    div, px, AnyElement, Context, Hsla, IntoElement, ParentElement, Styled, Window,
};
use gpui_component::chart::{AreaChart, BarChart};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{v_flex, ActiveTheme, StyledExt};

use crate::app::app::RTokenApp;
use crate::app::state::{ChartKind, ChartMetric, ChartRange, ChartsSnapshot};
use crate::core::aggregation::SumStats;
use crate::core::model::Provider;

use crate::ui::page_shell;

/// Render-ready multi-series: a shared chronological x-domain with one value
/// per series per day (missing days are zero).
#[derive(Debug, Clone)]
struct NamedSeries {
    name: String,
    values: Vec<f64>,
}

#[derive(Debug, Clone)]
struct MultiSeries {
    days: Vec<String>,
    series: Vec<NamedSeries>,
}

/// One chart datum: an x label plus one y value per series.
struct DayRow {
    day: String,
    values: Vec<f64>,
}

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let weak = app.weak_self.clone();

    // Snapshot the state we render; controls dispatch via the weak handle, and
    // the charts render from owned clones so no borrow outlives this frame.
    let data = app.state.charts.data.clone();
    let metric = app.state.charts.metric;
    let kind = app.state.charts.kind;
    let provider = app.state.charts.provider;
    let colors = series_colors(cx);

    let range_ix = ChartRange::ALL
        .iter()
        .position(|r| *r == app.state.charts.range)
        .unwrap_or(0);
    let metric_ix = ChartMetric::ALL
        .iter()
        .position(|m| *m == app.state.charts.metric)
        .unwrap_or(0);
    let kind_ix = ChartKind::ALL
        .iter()
        .position(|k| *k == app.state.charts.kind)
        .unwrap_or(0);
    let provider_ix = Provider::ALL
        .iter()
        .position(|pr| *pr == provider)
        .unwrap_or(0);

    page_shell(cx, "图表统计", "按应用 / 模型 / 日期的 Token 用量趋势")
        .child(
            v_flex()
                .gap_2()
                .child(seg_label(p.muted_foreground, "时间范围"))
                .child({
                    let weak = weak.clone();
                    TabBar::new("charts-range")
                        .segmented()
                        .selected_index(range_ix)
                        .children(ChartRange::ALL.iter().map(|r| Tab::new().label(r.label())))
                        .on_click(move |ix, _, cx| {
                            if let Some(r) = ChartRange::ALL.get(*ix) {
                                let _ = weak.update(cx, |this, cx| this.select_chart_range(*r, cx));
                            }
                        })
                })
                .child(seg_label(p.muted_foreground, "指标"))
                .child({
                    let weak = weak.clone();
                    TabBar::new("charts-metric")
                        .segmented()
                        .selected_index(metric_ix)
                        .children(ChartMetric::ALL.iter().map(|m| Tab::new().label(m.label())))
                        .on_click(move |ix, _, cx| {
                            if let Some(m) = ChartMetric::ALL.get(*ix) {
                                let _ = weak.update(cx, |this, cx| this.select_chart_metric(*m, cx));
                            }
                        })
                })
                .child(seg_label(p.muted_foreground, "图类型"))
                .child({
                    let weak = weak.clone();
                    TabBar::new("charts-kind")
                        .segmented()
                        .selected_index(kind_ix)
                        .children(ChartKind::ALL.iter().map(|k| Tab::new().label(k.label())))
                        .on_click(move |ix, _, cx| {
                            if let Some(k) = ChartKind::ALL.get(*ix) {
                                let _ = weak.update(cx, |this, cx| this.select_chart_kind(*k, cx));
                            }
                        })
                })
                .child(seg_label(p.muted_foreground, "模型图 provider"))
                .child({
                    TabBar::new("charts-provider")
                        .segmented()
                        .selected_index(provider_ix)
                        .children(
                            Provider::ALL
                                .iter()
                                .map(|pr| Tab::new().label(pr.display_name())),
                        )
                        .on_click(move |ix, _, cx| {
                            if let Some(pr) = Provider::ALL.get(*ix) {
                                let _ = weak.update(cx, |this, cx| this.select_chart_provider(*pr, cx));
                            }
                        })
                }),
        )
        .child(main_section(&data, metric, kind, &colors, cx))
        .child(model_section(&data, metric, kind, provider, &colors, cx))
        .into_any_element()
}

fn seg_label(color: Hsla, text: &str) -> impl IntoElement {
    div().text_xs().text_color(color).child(text.to_string())
}

fn main_section(
    data: &Option<ChartsSnapshot>,
    metric: ChartMetric,
    kind: ChartKind,
    colors: &[Hsla],
    cx: &Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let content = match data {
        Some(snap) => {
            let ms = align_provider_series(&snap.provider_series, metric);
            if ms.days.is_empty() {
                empty_hint("暂无数据", p.muted_foreground)
            } else {
                match kind {
                    ChartKind::Line => line_chart(&ms, colors, "chart-main-line"),
                    ChartKind::Bar => daily_bar_chart(&ms, "chart-main-bar"),
                }
            }
        }
        None => empty_hint("加载中…", p.muted_foreground),
    };
    chart_card(cx, "每日用量趋势", content)
}

fn model_section(
    data: &Option<ChartsSnapshot>,
    metric: ChartMetric,
    kind: ChartKind,
    provider: Provider,
    colors: &[Hsla],
    cx: &Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);
    let content = match data {
        Some(snap) => {
            let ms = align_model_series(&snap.model_series, metric);
            if ms.days.is_empty() {
                empty_hint("暂无数据", p.muted_foreground)
            } else {
                match kind {
                    ChartKind::Line => line_chart(&ms, colors, "chart-model-line"),
                    ChartKind::Bar => model_bar_chart(&snap.model_series, metric, "chart-model-bar"),
                }
            }
        }
        None => empty_hint("加载中…", p.muted_foreground),
    };
    chart_card(cx, &format!("按模型（{}）", provider.display_name()), content)
}

fn chart_card(cx: &Context<RTokenApp>, title: &str, body: AnyElement) -> AnyElement {
    let p = crate::ui::palette(cx);
    v_flex()
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .p_3()
        .gap_2()
        .child(
            div()
                .font_semibold()
                .text_sm()
                .child(title.to_string()),
        )
        .child(div().h(px(320.0)).child(body))
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
        ChartMetric::Cost => s.cost_micros as f64 / 1e6,
    }
}

/// Align per-provider daily series onto a shared, chronological day domain.
fn align_provider_series(
    provider_series: &[(Provider, Vec<(String, SumStats)>)],
    metric: ChartMetric,
) -> MultiSeries {
    let mut day_set: BTreeSet<String> = BTreeSet::new();
    for (_, series) in provider_series {
        for (day, _) in series {
            day_set.insert(day.clone());
        }
    }
    let days: Vec<String> = day_set.into_iter().collect();
    let series = provider_series
        .iter()
        .map(|(provider, series)| {
            let map: BTreeMap<&str, SumStats> =
                series.iter().map(|(d, s)| (d.as_str(), *s)).collect();
            NamedSeries {
                name: provider.display_name().to_string(),
                values: days
                    .iter()
                    .map(|d| metric_value(metric, map.get(d.as_str()).copied().unwrap_or_default()))
                    .collect(),
            }
        })
        .collect();
    MultiSeries { days, series }
}

/// Align per-model daily series onto a shared, chronological day domain.
fn align_model_series(
    model_series: &BTreeMap<String, Vec<(String, SumStats)>>,
    metric: ChartMetric,
) -> MultiSeries {
    let mut day_set: BTreeSet<String> = BTreeSet::new();
    for (_, series) in model_series {
        for (day, _) in series {
            day_set.insert(day.clone());
        }
    }
    let days: Vec<String> = day_set.into_iter().collect();
    let series = model_series
        .iter()
        .map(|(model, series)| {
            let map: BTreeMap<&str, SumStats> =
                series.iter().map(|(d, s)| (d.as_str(), *s)).collect();
            NamedSeries {
                name: model.clone(),
                values: days
                    .iter()
                    .map(|d| metric_value(metric, map.get(d.as_str()).copied().unwrap_or_default()))
                    .collect(),
            }
        })
        .collect();
    MultiSeries { days, series }
}

/// Multi-series line chart (transparent area fill so only strokes show).
fn line_chart(ms: &MultiSeries, colors: &[Hsla], id: &'static str) -> AnyElement {
    let rows: Vec<DayRow> = ms
        .days
        .iter()
        .enumerate()
        .map(|(di, day)| DayRow {
            day: day.clone(),
            values: ms.series.iter().map(|s| s.values[di]).collect(),
        })
        .collect();

    let mut chart = AreaChart::new(rows)
        .x(|r: &DayRow| r.day.clone())
        .tick_margin(3);
    for (i, series) in ms.series.iter().enumerate() {
        let color = colors[i % colors.len()];
        chart = chart
            .y(move |r: &DayRow| r.values[i])
            .stroke(color)
            .fill(gpui::transparent_black())
            .name(series.name.clone());
    }
    chart.id(id).into_any_element()
}

/// Single-series daily bar chart (sum across series for the shared day domain).
fn daily_bar_chart(ms: &MultiSeries, id: &'static str) -> AnyElement {
    let data: Vec<(String, f64)> = ms
        .days
        .iter()
        .enumerate()
        .map(|(di, day)| {
            let total: f64 = ms.series.iter().map(|s| s.values[di]).sum();
            (day.clone(), total)
        })
        .collect();
    BarChart::new(data)
        .band(|(d, _)| d.clone())
        .value(|(_, v)| *v)
        .id(id)
        .into_any_element()
}

/// Per-model total (summed over the window) as a bar chart.
fn model_bar_chart(
    model_series: &BTreeMap<String, Vec<(String, SumStats)>>,
    metric: ChartMetric,
    id: &'static str,
) -> AnyElement {
    let data: Vec<(String, f64)> = model_series
        .iter()
        .map(|(model, series)| {
            let total = series.iter().fold(SumStats::default(), |mut acc, (_, s)| {
                acc.add(s);
                acc
            });
            (model.clone(), metric_value(metric, total))
        })
        .collect();
    BarChart::new(data)
        .band(|(m, _)| m.clone())
        .value(|(_, v)| *v)
        .id(id)
        .into_any_element()
}
