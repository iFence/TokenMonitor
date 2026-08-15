//! Compact-formatted line and bar charts for the charts page.
//!
//! `gpui_component`'s `AreaChart`/`BarChart` hardcode `format!("{}", value)`
//! in their hover tooltips and draw no value axis, so token counts show up as
//! unreadable raw digits (e.g. `9123456`). These two elements draw the same
//! marks on top of `gpui_component::plot`'s public primitives, but format every
//! value label — the value-axis ticks and the tooltip rows — through a caller
//! supplied `fn(f64) -> String` (typically `format_tokens_compact_f64`, so
//! `9.1M` / `3.5亿`, or `format_cost_f64` for the cost metric).

use gpui::{
    point, px, AnyElement, App, Bounds, ElementId, Hsla, IntoElement, Pixels, Point, SharedString,
    TextAlign, Window,
};
use gpui_component::plot::{
    scale::{Scale, ScaleBand, ScaleLinear, ScalePoint},
    shape::{Bar, BarAlignment, Line},
    tooltip::{CrossLine, Dot, Tooltip, TooltipState},
    AxisLabelSide, AxisText, Grid, Plot, PlotAxis, StrokeStyle, AXIS_GAP,
};
use gpui_component::ActiveTheme;

/// Left gutter reserved for the compact value-axis labels.
const Y_GUTTER: f32 = 60.0;

/// A multi-series line chart whose value labels are compact-formatted.
#[derive(Clone, gpui_component::plot::IntoPlot)]
pub struct CompactLineChart {
    x: Vec<String>,
    series: Vec<LineSeries>,
    formatter: fn(f64) -> String,
    tick_margin: usize,
    id: Option<ElementId>,
}

#[derive(Debug, Clone)]
pub struct LineSeries {
    name: String,
    values: Vec<f64>,
    color: Hsla,
}

impl LineSeries {
    pub fn new(name: String, values: Vec<f64>, color: Hsla) -> Self {
        Self { name, values, color }
    }
}

impl CompactLineChart {
    pub fn new(x: Vec<String>, series: Vec<LineSeries>, formatter: fn(f64) -> String) -> Self {
        Self {
            x,
            series,
            formatter,
            tick_margin: 1,
            id: None,
        }
    }

    pub fn tick_margin(mut self, tick_margin: usize) -> Self {
        self.tick_margin = tick_margin;
        self
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    fn max(&self) -> f64 {
        self.series
            .iter()
            .flat_map(|s| s.values.iter().copied())
            .fold(0.0f64, f64::max)
    }

    /// Shared y scale construction so `paint` and `tooltip_state` agree.
    fn y_scale(&self, plot_h: f32) -> ScaleLinear<f64> {
        let domain: Vec<f64> = self
            .series
            .iter()
            .flat_map(|s| s.values.iter().copied())
            .chain(Some(0.0))
            .collect();
        ScaleLinear::new(domain, vec![plot_h, 10.0])
    }
}

impl Plot for CompactLineChart {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32();
        let plot_h = height - AXIS_GAP;

        let x_scale = ScalePoint::new(self.x.clone(), vec![Y_GUTTER, width]);
        let y_scale = self.y_scale(plot_h);
        let ticks = nice_ticks(self.max());

        let y_labels: Vec<AxisText> = ticks
            .iter()
            .filter_map(|t| {
                y_scale.tick(t).map(|py| {
                    AxisText::new((self.formatter)(*t), px(py), cx.theme().muted_foreground)
                        .align(TextAlign::Right)
                })
            })
            .collect();
        let x_labels = point_x_labels(&self.x, &x_scale, self.tick_margin, cx.theme().muted_foreground);

        let axis = PlotAxis::new()
            .stroke(cx.theme().border)
            .x(plot_h)
            .x_label(x_labels)
            .y(Y_GUTTER)
            .y_label_side(AxisLabelSide::Start)
            .y_label(y_labels);

        let grid_ys: Vec<Pixels> = ticks.iter().filter_map(|t| y_scale.tick(t).map(px)).collect();
        Grid::new()
            .y(grid_ys)
            .stroke(cx.theme().border)
            .dash_array(&[px(4.), px(2.)])
            .paint(&bounds, window);
        axis.paint(&bounds, window, cx);

        for series in &self.series {
            let x_scale = x_scale.clone();
            let y_scale = y_scale.clone();
            let points: Vec<(f32, f32)> = self
                .x
                .iter()
                .zip(series.values.iter())
                .filter_map(|(xv, v)| {
                    let px = x_scale.tick(xv)?;
                    let py = y_scale.tick(v)?;
                    Some((px, py))
                })
                .collect();
            Line::new()
                .data(points)
                .x(|p| Some(p.0))
                .y(|p| Some(p.1))
                .stroke(series.color)
                .stroke_style(StrokeStyle::Natural)
                .paint(&bounds, window);
        }
    }

    fn id(&self) -> Option<ElementId> {
        self.id.clone()
    }

    fn tooltip_state(
        &self,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _cx: &App,
    ) -> Option<TooltipState> {
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32();
        let plot_h = height - AXIS_GAP;
        // Ignore the x-axis label gutter so hovering the labels shows nothing.
        if position.y.as_f32() > plot_h {
            return None;
        }

        let x_scale = ScalePoint::new(self.x.clone(), vec![Y_GUTTER, width]);
        let y_scale = self.y_scale(plot_h);
        let index = x_scale.least_index(position.x.as_f32());
        let x_tick = x_scale.tick(self.x.get(index)?)?;

        let dots = self
            .series
            .iter()
            .filter_map(|s| {
                let v = s.values.get(index)?;
                Some(point(px(x_tick), px(y_scale.tick(v)?)))
            })
            .collect();

        Some(TooltipState::new(index, point(px(x_tick), position.y), dots))
    }

    fn tooltip(
        &self,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let title: SharedString = self.x.get(state.index)?.clone().into();
        let default_color = cx.theme().chart_2;
        let dot_stroke = cx.theme().background;
        let color = |i: usize| self.series.get(i).map(|s| s.color).unwrap_or(default_color);

        let mut tooltip = Tooltip::new(cursor, bounds.size)
            .gap(px(8.))
            .cross_line(
                CrossLine::new(state.cross_line)
                    .height(bounds.size.height.as_f32() - AXIS_GAP),
            )
            .dots(
                state
                    .dots
                    .iter()
                    .enumerate()
                    .map(|(i, p)| Dot::new(*p).stroke(dot_stroke).fill(color(i))),
            )
            .title(title);

        for (i, series) in self.series.iter().enumerate() {
            let value = series.values.get(state.index).copied().unwrap_or(0.0);
            tooltip = tooltip.row(color(i), series.name.clone(), (self.formatter)(value));
        }

        Some(tooltip.into_any_element())
    }
}

/// A single-series bar chart whose value labels are compact-formatted.
#[derive(Clone, gpui_component::plot::IntoPlot)]
pub struct CompactBarChart {
    x: Vec<String>,
    values: Vec<f64>,
    formatter: fn(f64) -> String,
    id: Option<ElementId>,
}

impl CompactBarChart {
    pub fn new(x: Vec<String>, values: Vec<f64>, formatter: fn(f64) -> String) -> Self {
        Self {
            x,
            values,
            formatter,
            id: None,
        }
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    fn max(&self) -> f64 {
        self.values.iter().copied().fold(0.0f64, f64::max)
    }

    fn y_scale(&self, plot_h: f32) -> ScaleLinear<f64> {
        let domain: Vec<f64> = self.values.iter().copied().chain(Some(0.0)).collect();
        ScaleLinear::new(domain, vec![plot_h, 10.0])
    }
}

impl Plot for CompactBarChart {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32();
        let plot_h = height - AXIS_GAP;

        let band_scale = ScaleBand::new(self.x.clone(), vec![Y_GUTTER, width])
            .padding_inner(0.4)
            .padding_outer(0.2);
        let band_width = band_scale.band_width();
        let y_scale = self.y_scale(plot_h);
        let ticks = nice_ticks(self.max());

        let y_labels: Vec<AxisText> = ticks
            .iter()
            .filter_map(|t| {
                y_scale.tick(t).map(|py| {
                    AxisText::new((self.formatter)(*t), px(py), cx.theme().muted_foreground)
                        .align(TextAlign::Right)
                })
            })
            .collect();
        let x_labels =
            band_x_labels(&self.x, &band_scale, band_width, 1, cx.theme().muted_foreground);

        let axis = PlotAxis::new()
            .stroke(cx.theme().border)
            .x(plot_h)
            .x_label(x_labels)
            .y(Y_GUTTER)
            .y_label_side(AxisLabelSide::Start)
            .y_label(y_labels);

        let grid_ys: Vec<Pixels> = ticks.iter().filter_map(|t| y_scale.tick(t).map(px)).collect();
        Grid::new()
            .y(grid_ys)
            .stroke(cx.theme().border)
            .dash_array(&[px(4.), px(2.)])
            .paint(&bounds, window);
        axis.paint(&bounds, window, cx);

        let data: Vec<(String, f64)> = self
            .x
            .iter()
            .cloned()
            .zip(self.values.iter().copied())
            .collect();
        let bar_fill = cx.theme().chart_2;
        Bar::new()
            .data(data)
            .alignment(BarAlignment::Bottom)
            .band_width(band_width)
            .cross(move |d| band_scale.tick(&d.0))
            .base(move |_| plot_h)
            .value(move |d| y_scale.tick(&d.1))
            .fill(move |_, _, _| bar_fill)
            .paint(&bounds, window, cx);
    }

    fn id(&self) -> Option<ElementId> {
        self.id.clone()
    }

    fn tooltip_state(
        &self,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _cx: &App,
    ) -> Option<TooltipState> {
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32();
        let plot_h = height - AXIS_GAP;
        if position.y.as_f32() > plot_h {
            return None;
        }

        let band_scale = ScaleBand::new(self.x.clone(), vec![Y_GUTTER, width])
            .padding_inner(0.4)
            .padding_outer(0.2);
        let band_width = band_scale.band_width();
        let index = band_scale.least_index(position.x.as_f32());
        let d = self.x.get(index)?;
        let center = band_scale.tick(d)? + band_width / 2.;

        Some(TooltipState::new(
            index,
            point(px(center), position.y),
            vec![],
        ))
    }

    fn tooltip(
        &self,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let title: SharedString = self.x.get(state.index)?.clone().into();
        let value = self.values.get(state.index).copied().unwrap_or(0.0);

        let width = bounds.size.width.as_f32();
        let band_scale = ScaleBand::new(self.x.clone(), vec![Y_GUTTER, width])
            .padding_inner(0.4)
            .padding_outer(0.2);
        let band_width = band_scale.band_width();
        let cross_line = CrossLine::new(state.cross_line)
            .span(0., bounds.size.height.as_f32() - AXIS_GAP)
            .band(px(band_width));

        Some(
            Tooltip::new(cursor, bounds.size)
                .gap(px(8.))
                .cross_line(cross_line)
                .title(title)
                .row(cx.theme().chart_2, SharedString::default(), (self.formatter)(value))
                .into_any_element(),
        )
    }
}

/// ~4-5 "nice" ticks covering `[0, max]`, at 1/2/5×10^k steps.
fn nice_ticks(max: f64) -> Vec<f64> {
    if max <= 0.0 {
        return vec![0.0];
    }
    let target = max / 4.0;
    let pow = 10f64.powf(target.log10().floor());
    let norm = target / pow;
    let nice = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    let step = nice * pow;
    let n = (max / step).ceil() as usize;
    (0..=n).map(|i| i as f64 * step).collect()
}

fn point_x_labels(
    x: &[String],
    scale: &ScalePoint<String>,
    tick_margin: usize,
    color: Hsla,
) -> Vec<AxisText> {
    let n = x.len();
    x.iter()
        .enumerate()
        .filter_map(|(i, label)| {
            if (i + 1) % tick_margin != 0 {
                return None;
            }
            scale.tick(label).map(|t| {
                let align = match i {
                    0 if n == 1 => TextAlign::Center,
                    0 => TextAlign::Left,
                    i if i == n - 1 => TextAlign::Right,
                    _ => TextAlign::Center,
                };
                AxisText::new(label.clone(), px(t), color).align(align)
            })
        })
        .collect()
}

fn band_x_labels(
    x: &[String],
    scale: &ScaleBand<String>,
    band_width: f32,
    tick_margin: usize,
    color: Hsla,
) -> Vec<AxisText> {
    x.iter()
        .enumerate()
        .filter_map(|(i, label)| {
            if (i + 1) % tick_margin != 0 {
                return None;
            }
            scale.tick(label).map(|t| {
                AxisText::new(label.clone(), px(t + band_width / 2.), color).align(TextAlign::Center)
            })
        })
        .collect()
}
