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
    /// Inner radius as a fraction of the outer one: the higher the value, the
    /// thinner the band. A ring at 0.55 draws a 45%-thick band.
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

    /// How thin the ring is drawn: the inner radius as a fraction of the outer
    /// one, clamped so a ring always keeps a visible band and an open hole.
    pub fn inner_ratio(mut self, ratio: f32) -> Self {
        self.inner_ratio = ratio.clamp(0.0, 0.95);
        self
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Outer and inner radius of the ring drawn into a box `diameter` wide (the
    /// ring always fits the smaller side of its bounds).
    fn radii(&self, diameter: Pixels) -> (Pixels, Pixels) {
        let outer = diameter / 2.0;
        (outer, outer * self.inner_ratio)
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
        let (outer, inner) = self.radii(bounds.size.width.min(bounds.size.height));

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_ratio_is_clamped_to_a_drawable_ring() {
        assert_eq!(DonutChart::new(vec![]).inner_ratio(0.9).inner_ratio, 0.9);
        assert_eq!(DonutChart::new(vec![]).inner_ratio(-1.0).inner_ratio, 0.0);
        assert_eq!(DonutChart::new(vec![]).inner_ratio(2.0).inner_ratio, 0.95);
    }

    /// The band is what the card ring is tuned by: `outer - inner` is the ring's
    /// thickness, so the dashboard's 32px / 0.72 ring draws a 4.5px band where
    /// the chart default (200px / 0.55) draws 45px.
    #[test]
    fn radii_follow_the_diameter_and_inner_ratio() {
        let card_ring = DonutChart::new(vec![])
            .with_size(size(px(32.0), px(32.0)))
            .inner_ratio(0.72);
        let (outer, inner) = card_ring.radii(px(32.0));
        assert_eq!(outer, px(16.0));
        // 16 * 0.72 lands a hair above 11.52 in f32, compare with a tolerance.
        assert!((inner.as_f32() - 11.52).abs() < 0.01, "inner {inner}");
        assert!(
            ((outer - inner).as_f32() - 4.48).abs() < 0.01,
            "band {}",
            outer - inner
        );

        // A non-square box keeps the ring circular, sized by the shorter side.
        let (outer, _) = DonutChart::new(vec![]).radii(px(30.0));
        assert_eq!(outer, px(15.0));
    }
}
