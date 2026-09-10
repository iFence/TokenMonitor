//! GPUI application shell: bootstrap, root entity, state, actions.

pub mod actions;
pub mod app;
pub mod state;
pub mod update_check;
pub mod window_state;

pub use actions::Quit;
pub use app::TokenMonitorApp;
pub use state::{
    ActivePage, AppState, ChartApp, ChartMetric, ChartRange, ChartsSnapshot, ChartsState,
    ReportSnapshot, ReportState, ScanStatus, TimeTab, ViewSnapshot,
};

use std::sync::Arc;

use gpui::{px, size, App, AppContext, WindowBounds, WindowOptions};
use gpui_component::Root;

/// GPUI bootstrap: init components, open the main window, run the app.
pub fn run() -> anyhow::Result<()> {
    // Single-instance: a second launch (double-clicked shortcut, auto-start
    // entry, taskbar pin) hands off to the copy that is already running and
    // exits, so there is only ever one window and one tray icon.
    if !crate::platform::acquire_single_instance() {
        crate::platform::activate_running_instance();
        return Ok(());
    }
    let application = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    application.run(move |cx: &mut App| {
        gpui_component::init(cx);
        // The settings/search/select widgets pull their placeholder and tooltip
        // text from gpui_component's rust_i18n bundles. Default to the zh-CN
        // locale so the settings sidebar search box reads "搜索..." instead of
        // the English "Search...".
        gpui_component::set_locale("zh-CN");
        cx.on_action(|_: &Quit, cx| cx.quit());
        // GitHub release check + installer download go through this client.
        let http_client = reqwest_client::ReqwestClient::user_agent(concat!(
            "TokenMonitor/",
            env!("CARGO_PKG_VERSION")
        ))
        .expect("failed to initialize TokenMonitor HTTP client");
        cx.set_http_client(Arc::new(http_client));
        // Reopen at the size the window had when the app was last closed.
        let bounds = window_state::initial_bounds(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Frameless window: the native title bar / control bar is
                // removed, and the app's own top bar supplies the draggable
                // region (see `WindowControlArea::Drag` in the top bar).
                titlebar: None,
                // The comfort floor is roughly two-thirds of the originally
                // measured 836px width (scale factor 1.0). The report heatmap
                // scales its cells to the card width, so this is a comfort
                // floor, not a hard requirement. Shared with the remembered
                // window size, which is never restored below it.
                window_min_size: Some(size(
                    px(window_state::MIN_SIZE.0),
                    px(window_state::MIN_SIZE.1),
                )),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| TokenMonitorApp::new(window, cx));
                view.update(cx, |app, cx| app.maybe_check_for_updates_on_startup(cx));
                // Remember every resize so the next launch reopens at this
                // size. Writes are coalesced off the UI thread.
                view.update(cx, |app, cx| {
                    let writer = window_state::start_writer(app.collector.clone());
                    let mut watcher = window_state::SizeWatcher::new(writer);
                    cx.observe_window_bounds(window, move |_app, window, _cx| {
                        watcher.observe(window);
                    })
                    .detach();
                });
                // Windows system-tray icon: left-click shows the window,
                // right-click opens a menu (打开 / 退出), and the title-bar X
                // hides it to tray (quit via the tray menu). The window exists
                // here, so we grab its native HWND and attach the icon.
                #[cfg(target_os = "windows")]
                if let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(&*window) {
                    if let raw_window_handle::RawWindowHandle::Win32(win) = handle.as_raw() {
                        crate::platform::start_tray(win.hwnd.get());
                    }
                }
                cx.new(|cx| Root::new(view, window, cx).bordered(false))
            },
        )
        .expect("failed to open TokenMonitor window");
        // Keep the native titlebar dark to match the dark panels, independent
        // of the OS light/dark theme. No-op on non-Windows platforms.
        crate::platform::apply_dark_titlebar();
        cx.activate(true);
    });
    Ok(())
}
