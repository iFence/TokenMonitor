//! GPUI application shell: bootstrap, root entity, state, actions.

pub mod actions;
pub mod app;
pub mod state;

pub use actions::Quit;
pub use app::RTokenApp;
pub use state::{
    ActivePage, AppState, ChartKind, ChartMetric, ChartRange, ChartsSnapshot, ChartsState,
    ScanStatus, TimeTab, ViewSnapshot,
};

use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};
use gpui_component::Root;

/// GPUI bootstrap: init components, open the main window, run the app.
pub fn run() -> anyhow::Result<()> {
    let application = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    application.run(move |cx: &mut App| {
        gpui_component::init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("rToken".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| RTokenApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx).bordered(false))
            },
        )
        .expect("failed to open rToken window");
        cx.activate(true);
    });
    Ok(())
}
