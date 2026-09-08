//! Persistent top bar: app title, scan status, and page navigation.

use std::sync::Arc;

use gpui::{
    div, img, px, AnyElement, Context, Image, ImageFormat, ImageSource, InteractiveElement,
    IntoElement, ParentElement, Styled, Window, WindowControlArea,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex, IconName, Selectable, StyledExt,
};

use crate::app::app::TokenMonitorApp;
use crate::app::state::{ActivePage, ScanStatus};
use crate::core::time::east8;

/// Render the top bar shared by every page.
pub fn render_topbar(
    app: &mut TokenMonitorApp,
    _window: &mut Window,
    cx: &mut Context<TokenMonitorApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);

    let status_text = match &app.state.scan_status {
        ScanStatus::Idle => "尚未扫描".to_string(),
        ScanStatus::Scanning { .. } => "扫描中…".to_string(),
        ScanStatus::Done { at, .. } => {
            format!("更新 {}", at.with_timezone(&east8()).format("%H:%M"))
        }
        ScanStatus::Failed { .. } => "扫描失败".to_string(),
    };

    h_flex()
        .id("tokenmonitor-topbar")
        .w_full()
        .px_4()
        .py_2()
        .items_center()
        .border_b_1()
        .border_color(p.border)
        .child(
            h_flex().gap_2().items_center().child(app_icon()).child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(p.foreground)
                    .child("TokenMonitor"),
            ),
        )
        .child(
            div()
                .flex_1()
                .self_stretch()
                .window_control_area(WindowControlArea::Drag),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(status_text),
                )
                .child(nav_icon(
                    app,
                    cx,
                    "nav-dashboard",
                    IconName::LayoutDashboard,
                    ActivePage::Dashboard,
                ))
                .child(nav_icon(
                    app,
                    cx,
                    "nav-project",
                    IconName::Folder,
                    ActivePage::Project,
                ))
                .child(nav_icon(
                    app,
                    cx,
                    "nav-charts",
                    IconName::ChartPie,
                    ActivePage::Charts,
                ))
                .child(nav_icon(
                    app,
                    cx,
                    "nav-settings",
                    IconName::Settings,
                    ActivePage::Settings,
                ))
                .child(close_button(cx)),
        )
        .into_any_element()
}

/// Small app logo pinned to the top-left corner of the top bar. The bitmap
/// (orange ring + orange "T") is embedded at compile time so it ships inside
/// the binary, matching the window/taskbar icon in `resources/tokenmonitor.ico`.
fn app_icon() -> AnyElement {
    let image = Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../resources/tokenmonitor.png").to_vec(),
    ));
    img(ImageSource::Image(image))
        .w(px(22.0))
        .h(px(22.0))
        .into_any_element()
}

/// Window close (X) sitting just left of the settings icon. Closes the window
/// to the system tray on Windows (no-op elsewhere).
fn close_button(cx: &mut Context<TokenMonitorApp>) -> Button {
    Button::new("window-close")
        .ghost()
        .icon(IconName::Close)
        .on_click(cx.listener(|_, _, _, _| crate::platform::close_window()))
}

fn nav_icon(
    app: &TokenMonitorApp,
    cx: &mut Context<TokenMonitorApp>,
    id: &'static str,
    icon: IconName,
    page: ActivePage,
) -> Button {
    Button::new(id)
        .ghost()
        .icon(icon)
        .selected(app.state.active_page == page)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_page(page, cx);
        }))
}
