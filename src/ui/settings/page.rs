//! Settings page: app selection (which apps to track and their order) and about.
//!
//! Built on `gpui_component::setting` (`Settings` / `SettingPage` /
//! `SettingGroup` / `SettingItem`), which provides the searchable, resizable
//! sidebar and scrolling group layout.

use std::sync::atomic::Ordering;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, Anchor, AnyElement, App, Context, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, StyleRefinement, Styled, WeakEntity,
    Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_component::switch::Switch;
use gpui_component::text::TextView;
use gpui_component::{h_flex, v_flex, Disableable, IconName, StyledExt};

use crate::app::app::TokenMonitorApp;
use crate::app::state::ScanInterval;
use crate::core::model::ThemeColor;
use crate::core::update::UpdateState;

use crate::ui::page_shell;

/// Narrowest the sidebar may become: the search field and the page labels still
/// have to fit.
const SIDEBAR_MIN_WIDTH: f32 = 140.0;
/// Default sidebar width, matching gpui-component's own default.
const SIDEBAR_DEFAULT_WIDTH: f32 = 250.0;
/// Widest the sidebar ever gets (gpui-component's default drag-range end).
const SIDEBAR_MAX_WIDTH: f32 = 360.0;
/// Room the settings form keeps for itself when the window narrows.
const FORM_MIN_WIDTH: f32 = 400.0;
/// Horizontal padding `page_shell` puts around the settings panel.
const PAGE_PADDING: f32 = 32.0;

/// Widest the sidebar may be inside a settings panel `panel_width` wide.
///
/// The panel is split into a resizable sidebar and the settings form. The form
/// is what actually needs the width — a menu holding two page labels does not —
/// so the menu gives way first: it is capped to leave [`FORM_MIN_WIDTH`] for the
/// form, which makes it narrow together with the window instead of holding its
/// default and squeezing the form. On a wide window the cap is inert (the menu
/// keeps its full drag range) and below [`SIDEBAR_MIN_WIDTH`] it stops giving
/// way, because a narrower menu can no longer show its search field.
fn sidebar_max_width(panel_width: f32) -> f32 {
    (panel_width - FORM_MIN_WIDTH).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
}

/// A gpui-component [`Settings`] whose sidebar gives way to the form as the
/// panel narrows: its drag range is capped at [`sidebar_max_width`], and the
/// initial width follows the same cap so a window opened small starts out
/// balanced instead of 250px wide in a 525px panel.
fn responsive_settings(id: impl Into<gpui::ElementId>, panel_width: f32) -> Settings {
    let max_sidebar = sidebar_max_width(panel_width);
    Settings::new(id)
        .sidebar_width(px(SIDEBAR_DEFAULT_WIDTH.min(max_sidebar)))
        .sidebar_size_range(px(SIDEBAR_MIN_WIDTH)..px(max_sidebar))
}

pub fn render_page(
    app: &mut TokenMonitorApp,
    window: &mut Window,
    cx: &mut Context<TokenMonitorApp>,
) -> AnyElement {
    let weak = app.weak_self.clone();
    let panel_bg = crate::ui::palette(cx).background;
    // The settings panel is the page width minus `page_shell`'s padding.
    let panel_width = window.viewport_size().width.as_f32() - PAGE_PADDING;

    page_shell(cx, "设置", None)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(settings(&weak, panel_bg, panel_width)),
        )
        .into_any_element()
}

/// The full settings instance: pages rendered through gpui_component's
/// `Settings` sidebar + scrollable group layout.
///
/// The sidebar is painted with the panel background so the sidebar blends
/// seamlessly with the rest of the page rather than it reading as a darker
/// (near-black) column.
fn settings(
    weak: &WeakEntity<TokenMonitorApp>,
    panel_bg: Hsla,
    panel_width: f32,
) -> impl IntoElement {
    let sidebar_style = StyleRefinement::default().bg(panel_bg);

    responsive_settings("tokenmonitor-settings", panel_width)
        .default_selected_index(Default::default())
        .sidebar_style(&sidebar_style)
        .pages([general_page(weak), about_page(weak)])
}

/// "通用": app-wide behavior settings (autostart, rescan interval).
fn general_page(weak: &WeakEntity<TokenMonitorApp>) -> SettingPage {
    let weak = weak.clone();
    SettingPage::new("通用").icon(IconName::Settings).group(
        SettingGroup::new()
            .item(autostart_item(&weak))
            .item(scan_interval_item(&weak))
            .item(SettingItem::new("主题色", theme_color_field(&weak))),
    )
}

/// One settings row: label on the left, control on the right.
///
/// gpui-component stacks a `SettingItem`'s label above its field once the form
/// is narrower than 480px. The app's rows are short enough to stay side by
/// side, so they are built as custom items that keep this layout at any width
/// (a wrapped row wastes a full line per setting on exactly the narrow windows
/// where space is tight).
fn setting_row(title: &str, control: AnyElement, p: &crate::ui::Palette) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .text_sm()
                .text_color(p.foreground)
                .child(title.to_string()),
        )
        .child(control)
        .into_any_element()
}

/// "开机自启" row: switch reading/writing the OS auto-start registration. The
/// value is cached on the app entity (kept in sync with the OS), so the switch
/// reflects the actual launch-at-login state without spawning `reg.exe` on
/// every render.
fn autostart_item(weak: &WeakEntity<TokenMonitorApp>) -> SettingItem {
    let weak = weak.clone();
    SettingItem::render(move |_, _, cx: &mut App| {
        let p = crate::ui::palette(cx);
        let enabled = weak
            .read_with(cx, |app, _| app.autostart_enabled)
            .unwrap_or(false);
        let weak = weak.clone();
        setting_row(
            "开机自启",
            Switch::new("autostart-switch")
                .checked(enabled)
                .on_click(move |checked: &bool, _, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| this.set_autostart(*checked, cx));
                })
                .into_any_element(),
            &p,
        )
    })
    .keywords(["开机自启", "自启", "autostart"])
}

/// "扫描间隔" row: dropdown of the rescan intervals, reading/writing the live
/// [`TokenMonitorApp::scan_interval`].
fn scan_interval_item(weak: &WeakEntity<TokenMonitorApp>) -> SettingItem {
    let weak = weak.clone();
    SettingItem::render(move |_, _, cx: &mut App| {
        let p = crate::ui::palette(cx);
        let current = weak
            .read_with(cx, |app, _| {
                ScanInterval::from_seconds(app.scan_interval.load(Ordering::Relaxed))
            })
            .unwrap_or(ScanInterval::Min5);
        let weak = weak.clone();
        setting_row("扫描间隔", interval_dropdown(current, weak), &p)
    })
    .keywords(["扫描间隔", "间隔", "interval"])
}

/// Dropdown button listing every [`ScanInterval`], marking the active one.
fn interval_dropdown(current: ScanInterval, weak: WeakEntity<TokenMonitorApp>) -> AnyElement {
    Button::new("scan-interval")
        .label(current.label())
        .dropdown_caret(true)
        .outline()
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            ScanInterval::ALL.iter().fold(menu, |menu, interval| {
                let interval = *interval;
                let weak = weak.clone();
                menu.item(
                    PopupMenuItem::new(interval.label())
                        .checked(interval == current)
                        .on_click(move |_, _, cx: &mut App| {
                            let _ =
                                weak.update(cx, |this, cx| this.select_scan_interval(interval, cx));
                        }),
                )
            })
        })
        .into_any_element()
}

/// Dropdown field reading/writing the app accent [`ThemeColor`]. The option
/// value is the color's persisted key; the label is its display name. Reads go
/// through the captured `WeakEntity` so the field reflects the current color.
fn theme_color_field(weak: &WeakEntity<TokenMonitorApp>) -> SettingField<SharedString> {
    let options = ThemeColor::ALL
        .map(|color| {
            (
                SharedString::from(color.key()),
                SharedString::from(color.label()),
            )
        })
        .to_vec();
    let weak_read = weak.clone();
    let weak_write = weak.clone();
    SettingField::scrollable_dropdown(
        options,
        move |cx: &App| {
            let color = weak_read
                .read_with(cx, |app, _| app.theme_color)
                .unwrap_or_default();
            SharedString::from(color.key())
        },
        move |value: SharedString, cx: &mut App| {
            let color = ThemeColor::from_key(&value);
            let _ = weak_write.update(cx, |this, cx| {
                this.select_theme_color(color, cx);
            });
        },
    )
}

/// "关于": version row and the auto-update controls.
fn about_page(weak: &WeakEntity<TokenMonitorApp>) -> SettingPage {
    SettingPage::new("关于").icon(IconName::Info).group(
        SettingGroup::new()
            .item(version_item())
            .item(about_item(weak)),
    )
}

/// "版本" row: the prefixed version number (e.g. `v0.3.5`) on the right.
fn version_item() -> SettingItem {
    SettingItem::render(|_, _, cx: &mut App| {
        let p = crate::ui::palette(cx);
        setting_row(
            "版本",
            div()
                .text_sm()
                .text_color(p.foreground)
                .child(format!("v{}", env!("CARGO_PKG_VERSION")))
                .into_any_element(),
            &p,
        )
    })
    .keywords(["版本", "version"])
}

/// The about update controls as a custom element: the check-updates button, its
/// status, the download progress, release notes, and the download / skip
/// actions. State is read live through the weak handle each render so the
/// panel always reflects the latest `update_check`.
fn about_item(weak: &WeakEntity<TokenMonitorApp>) -> SettingItem {
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
                        .id("about-release-notes-scroll")
                        .w_full()
                        .max_h(px(220.0))
                        .overflow_y_scroll()
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

fn check_updates_button(weak: &WeakEntity<TokenMonitorApp>, busy: bool) -> Button {
    let weak = weak.clone();
    Button::new("about-check-updates")
        .label("检查更新")
        .disabled(busy)
        .on_click(move |_, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.check_for_updates(true, cx));
        })
}

fn download_install_button(weak: &WeakEntity<TokenMonitorApp>, portable: bool) -> Button {
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

fn skip_update_button(weak: &WeakEntity<TokenMonitorApp>) -> Button {
    let weak = weak.clone();
    Button::new("about-skip-update")
        .label("跳过此版本")
        .ghost()
        .on_click(move |_, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.skip_update(cx));
        })
}

#[cfg(test)]
mod tests {
    use gpui::{
        div, px, Context, InteractiveElement as _, IntoElement, ParentElement, Render, Styled,
        TestAppContext, VisualTestContext, Window,
    };
    use gpui_component::setting::{SettingGroup, SettingItem, SettingPage};

    use super::{
        responsive_settings, setting_row, sidebar_max_width, FORM_MIN_WIDTH, SIDEBAR_DEFAULT_WIDTH,
        SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
    };

    /// Renders the real `Settings` widget — through the same
    /// [`responsive_settings`] helper the page uses — inside a panel of a given
    /// width, with a debug-tagged stand-in for the settings form so the
    /// sidebar / form split can be measured.
    struct SidebarProbe {
        panel_width: f32,
    }

    impl Render for SidebarProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.panel_width)).h(px(600.0)).child(
                responsive_settings("probe-settings", self.panel_width).pages([SettingPage::new(
                    "通用",
                )
                .group(SettingGroup::new().item(SettingItem::render(|_, _, _cx| {
                    div()
                        .w_full()
                        .h(px(20.0))
                        .debug_selector(|| "probe-form".to_string())
                        .into_any_element()
                })))]),
            )
        }
    }

    /// Width of the settings form inside a panel `panel_width` wide.
    fn form_width(cx: &mut TestAppContext, panel_width: f32) -> f32 {
        let (_, cx) = cx.add_window_view(|_, _| SidebarProbe { panel_width });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.debug_bounds("probe-form")
            .expect("the probe form should be laid out")
            .size
            .width
            .as_f32()
    }

    /// One custom settings row at a given panel width, with its control tagged
    /// so the laid-out position can be measured.
    struct RowProbe {
        panel_width: f32,
    }

    impl Render for RowProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.panel_width)).h(px(600.0)).child(
                responsive_settings("row-probe", self.panel_width).pages([SettingPage::new(
                    "通用",
                )
                .group(SettingGroup::new().item(SettingItem::render(|_, _, cx| {
                    setting_row(
                        "扫描间隔",
                        div()
                            .w(px(60.0))
                            .h(px(20.0))
                            .debug_selector(|| "probe-control".to_string())
                            .into_any_element(),
                        &crate::ui::palette(cx),
                    )
                })))]),
            )
        }
    }

    /// gpui-component stacks a row's label above its control once the form is
    /// narrower than 480px, which costs a line per setting exactly on the
    /// windows where space is tight. The app's rows are custom items, so they
    /// have to stay side by side at any width.
    #[gpui::test]
    fn settings_rows_stay_on_one_line_when_the_form_is_narrow(cx: &mut TestAppContext) {
        cx.update(|cx| gpui_component::init(cx));

        let (_, cx) = cx.add_window_view(|_, _| RowProbe { panel_width: 380.0 });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        // A 380px panel leaves the form far below the stacking threshold: a
        // stacked row would put the control at the form's left edge (~170px
        // into the panel), a side-by-side row keeps it at the right one.
        let control = cx
            .debug_bounds("probe-control")
            .expect("the probe control should be laid out");
        assert!(
            control.left() > px(190.0),
            "the control should stay on the label's line: {control:?}"
        );
    }

    #[test]
    fn sidebar_gives_way_to_the_settings_form() {
        // Wide window: the menu may use its whole default range.
        assert_eq!(sidebar_max_width(1200.0), SIDEBAR_MAX_WIDTH);
        assert_eq!(
            sidebar_max_width(FORM_MIN_WIDTH + SIDEBAR_MAX_WIDTH),
            SIDEBAR_MAX_WIDTH
        );

        // Narrower panel: the menu narrows instead of squeezing the form.
        assert_eq!(sidebar_max_width(700.0), 300.0);
        assert_eq!(sidebar_max_width(600.0), 200.0);
        assert_eq!(
            sidebar_max_width(FORM_MIN_WIDTH + SIDEBAR_DEFAULT_WIDTH),
            250.0
        );

        // At the app's minimum window width the menu is at its floor, and it
        // stays there however narrow the panel gets.
        assert_eq!(sidebar_max_width(525.0), SIDEBAR_MIN_WIDTH);
        assert_eq!(sidebar_max_width(300.0), SIDEBAR_MIN_WIDTH);
    }

    #[test]
    fn sidebar_max_never_grows_when_the_panel_narrows() {
        let mut previous = f32::MAX;
        let mut width = 1600.0;
        while width >= 260.0 {
            let max = sidebar_max_width(width);
            assert!(max <= previous, "width {width} produced {max}");
            assert!(max >= SIDEBAR_MIN_WIDTH && max <= SIDEBAR_MAX_WIDTH);
            previous = max;
            width -= 20.0;
        }
    }

    /// The whole point of the cap: shrinking the panel must take width from the
    /// menu, not from the settings form. Before the cap the sidebar held its
    /// ~250px at every window size, so a panel narrowed by 440px lost all 440px
    /// to the form (718 -> 278) while the menu did not move at all.
    #[gpui::test]
    fn sidebar_narrows_so_the_form_keeps_its_room(cx: &mut TestAppContext) {
        cx.update(|cx| gpui_component::init(cx));

        let wide_form = form_width(cx, 1000.0);
        let narrow_form = form_width(cx, 560.0);
        let wide_sidebar = 1000.0 - wide_form;
        let narrow_sidebar = 560.0 - narrow_form;

        assert!(
            narrow_sidebar < wide_sidebar,
            "the menu should give way: {wide_sidebar} -> {narrow_sidebar}"
        );
        assert!(
            narrow_form > wide_form - 400.0,
            "the form should keep most of its room: {wide_form} -> {narrow_form}"
        );
    }
}
