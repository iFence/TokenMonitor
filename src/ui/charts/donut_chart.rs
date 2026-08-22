use std::f32::consts::TAU;

use gpui::{
    point, px, App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, PathBuilder, Pixels, Point, Size, Window,
};
use gpui::{size, Style};

/// A donut chart drawn as filled arc segments (one per category).
#[derive(Debug, Clone)]
pub struct DonutChart {
    data: Vec<(String, u64)>,
    colors: Vec<Hsla>,
    inner_ratio: f32,
    size: Size<Pixels>,
    id: ElementId,
}

impl DonutChart {
    pub fn new(data: Vec<(String, u64)>) -> Self {
        DonutChart {
            data,
            colors: default_colors(),
            inner_ratio: 0.55,
            size: size(px(200.0), px(200.0)),
            id: ElementId::Name("tokenmonitor-donut-chart".into()),
        }
    }

    pub fn data(mut self, data: Vec<(String, u64)>) -> Self {
        self.data = data;
        self
    }

    pub fn colors(mut self, colors: Vec<Hsla>) -> Self {
        self.colors = colors;
        self
    }

    pub fn with_size(mut self, size: Size<Pixels>) -> Self {
        self.size = size;
        self
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
}

fn default_colors() -> Vec<Hsla> {
    // GPUI stores hue normalized to 0..=1 (not degrees).
    [
        (15.0 / 360.0, 0.75, 0.55),
        (45.0 / 360.0, 0.8, 0.55),
        (90.0 / 360.0, 0.6, 0.5),
        (160.0 / 360.0, 0.55, 0.5),
        (210.0 / 360.0, 0.65, 0.55),
        (270.0 / 360.0, 0.6, 0.55),
    ]
    .into_iter()
    .map(|(h, s, l)| Hsla { h, s, l, a: 1.0 })
    .collect()
}

impl IntoElement for DonutChart {
    type Element = DonutChart;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DonutChart {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            size: Size::new(self.size.width.into(), self.size.height.into()),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        let total: u64 = self.data.iter().map(|(_, v)| *v).sum();
        if total == 0 {
            return;
        }
        let center = bounds.center();
        let outer = bounds.size.width.min(bounds.size.height) / 2.0;
        let inner = outer * self.inner_ratio;

        let mut start_angle = -TAU / 4.0; // start at 12 o'clock
        for (i, (_, value)) in self.data.iter().enumerate() {
            let sweep = TAU * (*value as f64 / total as f64) as f32;
            let end_angle = start_angle + sweep;
            let color = self.colors[i % self.colors.len()];

            let outer_start = point_on_circle(center, outer, start_angle);
            let outer_end = point_on_circle(center, outer, end_angle);
            let inner_start = point_on_circle(center, inner, start_angle);
            let inner_end = point_on_circle(center, inner, end_angle);
            let large_arc = sweep > TAU / 2.0;

            if let Ok(path) = {
                let mut builder = PathBuilder::fill();
                builder.move_to(outer_start);
                builder.arc_to(point(outer, outer), px(0.0), large_arc, true, outer_end);
                builder.line_to(inner_end);
                builder.arc_to(point(inner, inner), px(0.0), large_arc, false, inner_start);
                builder.close();
                builder.build()
            } {
                window.paint_path(path, color);
            }

            start_angle = end_angle;
        }
    }
}

fn point_on_circle(center: Point<Pixels>, radius: Pixels, angle: f32) -> Point<Pixels> {
    point(
        center.x + radius * angle.cos(),
        center.y + radius * angle.sin(),
    )
}
