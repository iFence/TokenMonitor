use gpui::{
    point, px, App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, PathBuilder, Pixels, Size, Window,
};
use gpui::{size, Style};

/// A simple vertical bar chart drawn with GPUI paths.
#[derive(Debug, Clone)]
pub struct BarChart {
    data: Vec<(String, u64)>,
    color: Hsla,
    size: Size<Pixels>,
    id: ElementId,
}

impl BarChart {
    pub fn new(data: Vec<(String, u64)>) -> Self {
        BarChart {
            data,
            color: Hsla {
                h: 220.0,
                s: 0.6,
                l: 0.55,
                a: 1.0,
            },
            size: size(px(320.0), px(200.0)),
            id: ElementId::Name("rtoken-bar-chart".into()),
        }
    }

    pub fn data(mut self, data: Vec<(String, u64)>) -> Self {
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

impl IntoElement for BarChart {
    type Element = BarChart;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for BarChart {
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
        let max = self.data.iter().map(|(_, v)| *v).max().unwrap_or(1).max(1);
        let n = self.data.len().max(1) as f32;
        let slot = (bounds.size.width - px(16.0)) / n;
        let mut builder = PathBuilder::fill();
        for (i, (_, value)) in self.data.iter().enumerate() {
            let h = (bounds.size.height * (*value as f32 / max as f32)).max(px(1.0));
            let x0 = bounds.origin.x + px(8.0) + slot * i as f32;
            let y0 = bounds.origin.y + bounds.size.height - h;
            builder.move_to(point(x0 + px(2.0), y0));
            builder.line_to(point(x0 + slot - px(2.0), y0));
            builder.line_to(point(
                x0 + slot - px(2.0),
                bounds.origin.y + bounds.size.height,
            ));
            builder.line_to(point(x0 + px(2.0), bounds.origin.y + bounds.size.height));
            builder.close();
        }
        if let Ok(path) = builder.build() {
            window.paint_path(path, self.color);
        }
    }
}
