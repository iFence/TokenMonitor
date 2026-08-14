use gpui::{
    point, px, App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, PathBuilder, Pixels, Size, Window,
};
use gpui::{size, Style};

/// A simple polyline chart (e.g. token usage over time) drawn with GPUI paths.
#[derive(Debug, Clone)]
pub struct LineChart {
    data: Vec<u64>,
    color: Hsla,
    stroke_width: Pixels,
    size: Size<Pixels>,
    id: ElementId,
}

impl LineChart {
    pub fn new(data: Vec<u64>) -> Self {
        LineChart {
            data,
            color: Hsla {
                h: 160.0,
                s: 0.55,
                l: 0.5,
                a: 1.0,
            },
            stroke_width: px(2.0),
            size: size(px(320.0), px(200.0)),
            id: ElementId::Name("rtoken-line-chart".into()),
        }
    }

    pub fn data(mut self, data: Vec<u64>) -> Self {
        self.data = data;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = color;
        self
    }

    pub fn with_size(mut self, size: Size<Pixels>) -> Self {
        self.size = size;
        self
    }
}

impl IntoElement for LineChart {
    type Element = LineChart;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LineChart {
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
        if self.data.is_empty() {
            return;
        }
        let max = self.data.iter().max().copied().unwrap_or(1).max(1);
        let n = self.data.len().max(2) as f32;
        let step_x = (bounds.size.width - px(16.0)) / (n - 1.0);
        let mut builder = PathBuilder::stroke(self.stroke_width);
        for (i, value) in self.data.iter().enumerate() {
            let x = bounds.origin.x + px(8.0) + step_x * i as f32;
            let y = bounds.origin.y + bounds.size.height
                - (bounds.size.height * (*value as f32 / max as f32));
            let p = point(x, y);
            if i == 0 {
                builder.move_to(p);
            } else {
                builder.line_to(p);
            }
        }
        if let Ok(path) = builder.build() {
            window.paint_path(path, self.color);
        }
    }
}
