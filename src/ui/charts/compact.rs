//! Compact-formatted bar chart for the charts page.
//!
//! `gpui_component`'s `BarChart` hardcodes `format!("{}", value)` in its hover
//! tooltip and draws no value axis, so token counts show up as unreadable raw
//! digits (e.g. `9123456`). This element draws the same marks on top of
//! `gpui_component::plot`'s public primitives, but formats every value label —
//! the value-axis ticks and the tooltip rows — through a caller-supplied
//! `fn(f64) -> String` (typically `format_tokens_compact_f64`, so `9.1M` /
//! `3.5亿`, or `format_cost_f64` for the cost metric).

use gpui::{
    point, px, AnyElement, App, Bounds, ElementId, Hsla, IntoElement, Pixels, Point, SharedString,
    Size, TextAlign, Window,
};
use gpui_component::plot::{
    scale::{Scale, ScaleBand, ScaleLinear},
    shape::{Bar, BarAlignment},
    tooltip::{CrossLine, Tooltip, TooltipState},
    AxisLabelSide, AxisText, Grid, Plot, PlotAxis, AXIS_GAP,
};
use gpui_component::ActiveTheme;

/// Left gutter reserved for the compact value-axis labels.
const Y_GUTTER: f32 = 60.0;

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
        // Anchor the scale's upper bound to the highest "nice" tick rather than
        // the raw data max. `nice_ticks` rounds up past `max`, so a data-max
        // domain makes the top tick extrapolate above the plot area and overlap
        // the controls above the chart.
        let top = nice_ticks(self.max()).last().copied().unwrap_or(0.0);
        ScaleLinear::new(vec![0.0, top], vec![plot_h, 10.0])
    }
}

impl Plot for CompactBarChart {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32();
        let plot_h = height - AXIS_GAP;
        let plot_width = width - Y_GUTTER;

        // The plot area excludes the left gutter reserved for y-axis labels.
        let plot_bounds = Bounds {
            origin: point(bounds.origin.x + px(Y_GUTTER), bounds.origin.y),
            size: Size::new(px(plot_width), px(height)),
        };

        let band_scale = ScaleBand::new(self.x.clone(), vec![0.0, plot_width])
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
        let x_labels = band_x_labels(
            &self.x,
            &band_scale,
            band_width,
            1,
            cx.theme().muted_foreground,
        );

        let axis = PlotAxis::new()
            .stroke(cx.theme().border)
            .x(plot_h)
            .x_label(x_labels)
            .y(0.0)
            .y_label_side(AxisLabelSide::Start)
            .y_label(y_labels);

        let grid_ys: Vec<Pixels> = ticks
            .iter()
            .filter_map(|t| y_scale.tick(t).map(px))
            .collect();
        Grid::new()
            .y(grid_ys)
            .stroke(cx.theme().border)
            .dash_array(&[px(4.), px(2.)])
            .paint(&plot_bounds, window);
        axis.paint(&plot_bounds, window, cx);

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
            .paint(&plot_bounds, window, cx);
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

        let plot_width = width - Y_GUTTER;
        let band_scale = ScaleBand::new(self.x.clone(), vec![0.0, plot_width])
            .padding_inner(0.4)
            .padding_outer(0.2);
        let band_width = band_scale.band_width();
        let index = band_scale.least_index(position.x.as_f32() - Y_GUTTER);
        let d = self.x.get(index)?;
        let center = Y_GUTTER + band_scale.tick(d)? + band_width / 2.;

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
        let plot_width = width - Y_GUTTER;
        let band_scale = ScaleBand::new(self.x.clone(), vec![0.0, plot_width])
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
                .row(
                    cx.theme().chart_2,
                    SharedString::default(),
                    (self.formatter)(value),
                )
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
                AxisText::new(label.clone(), px(t + band_width / 2.), color)
                    .align(TextAlign::Center)
            })
        })
        .collect()
}
