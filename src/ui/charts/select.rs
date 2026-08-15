//! `SearchableListItem` impls so the charts-page enums can be used directly as
//! items in gpui-component's `Select` dropdowns.

use gpui::SharedString;
use gpui_component::searchable_list::SearchableListItem;

use crate::app::state::{ChartApp, ChartMetric};

impl SearchableListItem for ChartMetric {
    type Value = ChartMetric;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

impl SearchableListItem for ChartApp {
    type Value = ChartApp;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}
