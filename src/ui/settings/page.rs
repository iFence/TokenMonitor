//! Settings page: app selection (which apps to track and their order) and about.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::searchable_list::SearchableListItem;
use gpui_component::select::Select;
use gpui_component::text::TextView;
use gpui_component::{h_flex, v_flex, Disableable, IconName, StyledExt};

use crate::app::app::RTokenApp;
use crate::app::state::{ScanInterval, SettingsGroup};
use crate::app::update_check::UpdateState;
use crate::core::model::Provider;

use crate::ui::page_shell;

pub fn render_page(
    app: &mut RTokenApp,
    _window: &mut Window,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    let weak = app.weak_self.clone();
    let group = app.state.settings_group;

    page_shell(cx, "设置", "应用配置与关于")
        .child(
            h_flex()
                .gap_4()
                .items_start()
                .child(sidebar(&weak, group, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(group_content(app, group, cx)),
                ),
        )
        .into_any_element()
}

/// Left-hand group navigation.
fn sidebar(
    weak: &WeakEntity<RTokenApp>,
    current: SettingsGroup,
    cx: &Context<RTokenApp>,
) -> impl IntoElement {
    let p = crate::ui::palette(cx);
    v_flex()
        .w(px(160.0))
        .flex_shrink_0()
        .gap_1()
        .children(SettingsGroup::ALL.iter().map(|&group| {
            let selected = group == current;
            let weak = weak.clone();
            div()
                .id(format!("settings-group-{:?}", group))
                .cursor_pointer()
                .px_3()
                .py_2()
                .rounded(p.radius)
                .text_sm()
                .font_medium()
                .text_color(if selected {
                    p.foreground
                } else {
                    p.muted_foreground
                })
                .when(selected, |this| this.bg(p.muted).font_semibold())
                .child(group.label())
                .on_click(move |_, _: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| this.select_settings_group(group, cx));
                })
        }))
}

/// Right-hand content for the selected group.
fn group_content(
    app: &mut RTokenApp,
    group: SettingsGroup,
    cx: &mut Context<RTokenApp>,
) -> AnyElement {
    match group {
        SettingsGroup::General => general_panel(app, cx),
        SettingsGroup::Applications => applications_panel(app, cx),
        SettingsGroup::About => about_panel(app, cx),
    }
}

/// General panel: app-wide behavior settings (rescan interval).
fn general_panel(app: &mut RTokenApp, cx: &mut Context<RTokenApp>) -> AnyElement {
    let p = crate::ui::palette(cx);
    let interval_select = app.scan_interval_select.clone();

    v_flex()
        .gap_2()
        .p_3()
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(p.foreground)
                .child("通用"),
        )
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .justify_between()
                .child(
                    v_flex()
                        .gap_1()
                        .child(div().text_sm().text_color(p.foreground).child("扫描间隔"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .child("自动重新扫描数据的间隔；应用启动时会立即扫描一次。"),
                        ),
                )
                .child(div().w(px(160.0)).child(Select::new(&interval_select))),
        )
        .into_any_element()
}

/// Panel listing every tracked app with a keep toggle and up/down reordering.
fn applications_panel(app: &mut RTokenApp, cx: &mut Context<RTokenApp>) -> AnyElement {
    let p = crate::ui::palette(cx);
    let weak = app.weak_self.clone();
    let entries = app.state.provider_selection.entries.clone();
    let last = entries.len().saturating_sub(1);

    v_flex()
        .gap_2()
        .p_3()
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(p.foreground)
                .child("应用"),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("选择要追踪的应用，以及它们在仪表盘上的显示顺序。"),
        )
        .children(entries.iter().enumerate().map(|(i, entry)| {
            let provider = entry.provider;
            let enabled = entry.enabled;

            h_flex()
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .child(
                    Checkbox::new(format!("app-toggle-{}", provider.id()))
                        .checked(enabled)
                        .label(provider.display_name())
                        .on_click({
                            let weak = weak.clone();
                            move |checked, window: &mut Window, cx: &mut App| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.set_provider_enabled(provider, *checked, window, cx);
                                });
                            }
                        }),
                )
                .child(div().flex_1())
                .child(reorder_button(&weak, provider, -1, i == 0))
                .child(reorder_button(&weak, provider, 1, i == last))
                .into_any_element()
        }))
        .into_any_element()
}

/// About panel: app name, version, description, and the auto-update controls.
fn about_panel(app: &mut RTokenApp, cx: &mut Context<RTokenApp>) -> AnyElement {
    let p = crate::ui::palette(cx);
    let portable = crate::platform::is_portable();
    let weak = app.weak_self.clone();
    let update = &app.update_check;
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
        UpdateState::Available { release_notes, .. } if !release_notes.trim().is_empty() => Some(
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
                        .w_full()
                        .min_w_0(),
                )
                .into_any_element(),
        ),
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
        .gap_2()
        .p_3()
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(p.foreground)
                .child("关于"),
        )
        .child(
            div()
                .text_lg()
                .font_bold()
                .text_color(p.foreground)
                .child("rToken"),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(format!("版本 {}", env!("CARGO_PKG_VERSION"))),
        )
        .child(
            v_flex()
                .gap_2()
                .pt_3()
                .border_t_1()
                .border_color(p.border)
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
                }),
        )
        .into_any_element()
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

fn reorder_button(
    weak: &WeakEntity<RTokenApp>,
    provider: Provider,
    dir: isize,
    disabled: bool,
) -> Button {
    let icon = if dir < 0 {
        IconName::ChevronUp
    } else {
        IconName::ChevronDown
    };
    let weak = weak.clone();
    Button::new(format!("app-move-{}-{}", provider.id(), dir))
        .ghost()
        .icon(icon)
        .disabled(disabled)
        .on_click(move |_, window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.move_provider(provider, dir, window, cx));
        })
}

impl SearchableListItem for ScanInterval {
    type Value = ScanInterval;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}
