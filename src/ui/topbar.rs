//! Persistent top bar: app title, scan status, and page navigation.

use gpui::{
    div, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex, v_flex, IconName, Selectable, StyledExt,
};

use crate::app::app::RTokenApp;
use crate::app::state::{ActivePage, ScanStatus};
use crate::core::time::east8;

/// Render the top bar shared by every page.
pub fn render_topbar(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let p = crate::ui::palette(cx);

    let status_text = match &app.state.scan_status {
        ScanStatus::Idle => "尚未扫描".to_string(),
        ScanStatus::Scanning { .. } => "扫描中…".to_string(),
        ScanStatus::Done { at, .. } => {
            format!("更新 {}", at.with_timezone(&east8()).format("%H:%M:%S"))
        }
        ScanStatus::Failed { .. } => "扫描失败".to_string(),
    };

    h_flex()
        .id("rtoken-topbar")
        .w_full()
        .px_4()
        .py_2()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(p.border)
        .child(
            v_flex().child(
                div()
                    .text_lg()
                    .font_bold()
                    .text_color(p.foreground)
                    .child("TokenMonitor"),
            ),
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
                )),
        )
        .into_any_element()
}

fn nav_icon(
    app: &RTokenApp,
    cx: &mut Context<RTokenApp>,
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
