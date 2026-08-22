//! Settings page: app selection (which apps to track and their order) and about.
//!
//! Built on `gpui_component::setting` (`Settings` / `SettingPage` /
//! `SettingGroup` / `SettingItem`), which provides the searchable, resizable
//! sidebar and scrolling group layout.

use std::sync::atomic::Ordering;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, App, Context, Hsla, IntoElement, ParentElement, SharedString,
    StyleRefinement, Styled, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_component::text::TextView;
use gpui_component::{h_flex, v_flex, Disableable, IconName, StyledExt};

use crate::app::app::RTokenApp;
use crate::app::state::ScanInterval;
use crate::app::update_check::UpdateState;

use crate::ui::page_shell;

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let weak = app.weak_self.clone();
    let panel_bg = crate::ui::palette(cx).background;

    page_shell(cx, "设置", None)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(settings(&weak, panel_bg)),
        )
        .into_any_element()
}

/// The full settings instance: pages rendered through gpui_component's
/// `Settings` sidebar + scrollable group layout.
///
/// The sidebar is painted with the panel background so the sidebar blends
/// seamlessly with the rest of the page rather than it reading as a darker
/// (near-black) column.
fn settings(weak: &WeakEntity<RTokenApp>, panel_bg: Hsla) -> impl IntoElement {
    let sidebar_style = StyleRefinement::default().bg(panel_bg);

    Settings::new("rtoken-settings")
        .default_selected_index(Default::default())
        .sidebar_style(&sidebar_style)
        .pages([general_page(weak), about_page(weak)])
}

/// "通用": app-wide behavior settings (rescan interval).
fn general_page(weak: &WeakEntity<RTokenApp>) -> SettingPage {
    let weak = weak.clone();
    SettingPage::new("通用")
        .icon(IconName::Settings)
        .description("应用整体行为设置")
        .group(
            SettingGroup::new().title("通用").item(
                SettingItem::new("扫描间隔", scan_interval_field(&weak))
                    .description("自动重新扫描数据的间隔；应用启动时会立即扫描一次。"),
            ),
        )
}

/// Dropdown field reading/writing the live [`RTokenApp::scan_interval`].
///
/// The option value is the interval's second count; the label is its human
/// readable text. Reads/writes go through the captured `WeakEntity` so the
/// field always reflects the current persisted interval.
fn scan_interval_field(weak: &WeakEntity<RTokenApp>) -> SettingField<SharedString> {
    let options = ScanInterval::ALL
        .map(|interval| {
            (
                SharedString::from(interval.seconds().to_string()),
                SharedString::from(interval.label()),
            )
        })
        .to_vec();
    let weak_read = weak.clone();
    let weak_write = weak.clone();
    SettingField::scrollable_dropdown(
        options,
        move |cx: &App| {
            let secs = weak_read
                .read_with(cx, |app, _| app.scan_interval.load(Ordering::Relaxed))
                .unwrap_or(ScanInterval::Min5.seconds());
            SharedString::from(ScanInterval::from_seconds(secs).seconds().to_string())
        },
        move |value: SharedString, cx: &mut App| {
            if let Ok(secs) = value.parse::<u64>() {
                let interval = ScanInterval::from_seconds(secs);
                let _ = weak_write.update(cx, |this, cx| {
                    this.select_scan_interval(interval, cx);
                });
            }
        },
    )
}

/// "关于": app name, version, description, and the auto-update controls.
fn about_page(weak: &WeakEntity<RTokenApp>) -> SettingPage {
    let weak = weak.clone();
    SettingPage::new("关于")
        .icon(IconName::Info)
        .group(SettingGroup::new().title("关于").item(about_item(&weak)))
}

/// The about content as a single custom element: name, version, and the update
/// check / download controls. State is read live through the weak handle each
/// render so the panel always reflects the latest `update_check`.
fn about_item(weak: &WeakEntity<RTokenApp>) -> SettingItem {
    let portable = crate::platform::is_portable();
    let weak = weak.clone();
    SettingItem::render(move |_, _window: &mut Window, cx: &mut App| {
        let p = crate::ui::palette(cx);
        let weak = weak.clone();
        // Snapshot the update state so bindings below are owned values, avoiding
        // borrows across the `WeakEntity` (which only offers `read_with`).
        let update = weak
            .read_with(cx, |app, _| app.update_check.clone())
            .unwrap_or_default();
        let is_busy = update.is_busy();
        let has_update = update.has_update();

        let status: AnyElement = match &update.state {
            UpdateState::Idle => div().into_any_element(),
            UpdateState::Checking => div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("正在检查更新…")
                .into_any_element(),
            UpdateState::Available { latest_version, .. } => div()
                .text_sm()
                .font_semibold()
                .text_color(p.foreground)
                .child(format!("发现新版本 v{latest_version}"))
                .into_any_element(),
            UpdateState::Downloading {
                downloaded_bytes,
                total_bytes,
                ..
            } => {
                let pct = total_bytes
                    .filter(|&total| total > 0)
                    .map(|total| (*downloaded_bytes as f32 / total as f32 * 100.0) as u32)
                    .unwrap_or(0);
                div()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(format!("正在下载… {pct}%"))
                    .into_any_element()
            }
            UpdateState::Installing => div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("正在启动安装程序…")
                .into_any_element(),
            UpdateState::Downloaded {
                latest_version,
                file_name,
            } => div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(format!(
                    "便携版 v{latest_version} 已下载（{file_name}），请退出本程序后解压覆盖"
                ))
                .into_any_element(),
            UpdateState::UpToDate => div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("已是最新版本")
                .into_any_element(),
            UpdateState::Error(message) => div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(format!("检查更新失败：{message}"))
                .into_any_element(),
        };

        let release_notes: Option<AnyElement> = match &update.state {
            UpdateState::Available { release_notes, .. } if !release_notes.trim().is_empty() => {
                Some(
                    div()
                        .w_full()
                        .max_h(px(220.0))
                        .overflow_y_hidden()
                        .rounded(p.radius)
                        .border_1()
                        .border_color(p.border)
                        .p_3()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(
                            TextView::markdown("about-release-notes", release_notes.clone())
                                .w_full(),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        };

        let progress: Option<AnyElement> = match &update.state {
            UpdateState::Downloading {
                downloaded_bytes,
                total_bytes,
                ..
            } => {
                let ratio = total_bytes
                    .filter(|&total| total > 0)
                    .map(|total| *downloaded_bytes as f32 / total as f32)
                    .unwrap_or(0.0);
                let fill = (ratio * 280.0).max(2.0).min(280.0);
                Some(
                    div()
                        .w(px(280.0))
                        .h(px(6.0))
                        .rounded(p.radius)
                        .bg(p.border)
                        .child(
                            div()
                                .h_full()
                                .w(px(fill))
                                .rounded(p.radius)
                                .bg(p.foreground),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        };

        v_flex()
            .gap_3()
            .child(div().text_lg().font_bold().child("TokenMonitor"))
            .child(
                div()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(format!("版本 {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(h_flex().h_px().w_full().bg(p.border))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(check_updates_button(&weak, is_busy))
                    .child(status),
            )
            .when_some(progress, |this, bar| this.child(bar))
            .when_some(release_notes, |this, notes| this.child(notes))
            .when(has_update, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(download_install_button(&weak, portable))
                        .child(skip_update_button(&weak)),
                )
            })
            .into_any_element()
    })
    .keywords(["关于", "更新", "版本"])
}

fn check_updates_button(weak: &WeakEntity<RTokenApp>, busy: bool) -> Button {
    let weak = weak.clone();
    Button::new("about-check-updates")
        .label("检查更新")
        .disabled(busy)
        .on_click(move |_, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.check_for_updates(true, cx));
        })
}

fn download_install_button(weak: &WeakEntity<RTokenApp>, portable: bool) -> Button {
    let weak = weak.clone();
    Button::new("about-download-install")
        .label(if portable {
            "下载更新"
        } else {
            "下载并安装"
        })
        .primary()
        .on_click(move |_, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.download_and_install(cx));
        })
}

fn skip_update_button(weak: &WeakEntity<RTokenApp>) -> Button {
    let weak = weak.clone();
    Button::new("about-skip-update")
        .label("跳过此版本")
        .ghost()
        .on_click(move |_, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.skip_update(cx));
        })
}
