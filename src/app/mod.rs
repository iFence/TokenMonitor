//! GPUI application shell: bootstrap, root entity, state, actions.

pub mod actions;
pub mod app;
pub mod state;
pub mod update_check;

pub use actions::Quit;
pub use app::RTokenApp;
pub use state::{
    ActivePage, AppState, ChartApp, ChartMetric, ChartRange, ChartsSnapshot, ChartsState,
    ScanStatus, TimeTab, ViewSnapshot,
};

use std::sync::Arc;

use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};
use gpui_component::Root;

/// GPUI bootstrap: init components, open the main window, run the app.
pub fn run() -> anyhow::Result<()> {
    let application = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    application.run(move |cx: &mut App| {
        gpui_component::init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        // GitHub release check + installer download go through this client.
        let http_client = reqwest_client::ReqwestClient::user_agent(concat!(
            "rToken/",
            env!("CARGO_PKG_VERSION")
        ))
        .expect("failed to initialize rToken HTTP client");
        cx.set_http_client(Arc::new(http_client));
        let bounds = Bounds::centered(None, size(px(1100.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("rToken".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(600.0), px(600.0))),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| RTokenApp::new(window, cx));
                view.update(cx, |app, cx| app.maybe_check_for_updates_on_startup(cx));
                cx.new(|cx| Root::new(view, window, cx).bordered(false))
            },
        )
        .expect("failed to open rToken window");
        // Keep the native titlebar dark to match the dark panels, independent
        // of the OS light/dark theme. No-op on non-Windows platforms.
        crate::platform::apply_dark_titlebar();
        cx.activate(true);
    });
    Ok(())
}
