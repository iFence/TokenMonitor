//! Native GPUI contribution-style heatmap.
//!
//! Renders the last 365 East-8 calendar days as a GitHub-style grid: seven
//! weekday rows, one column per week, cell color mapped linearly from the
//! day's token count. Everything is drawn with plain `div` elements, so it
//! stays crisp at any DPI and follows the app theme (no image round-trip).
//! Hovering a cell shows that day's usage in a floating tooltip.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{Datelike, Duration, NaiveDate, Utc, Weekday};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, App, Bounds, Div, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, Stateful, StatefulInteractiveElement, Styled, Window,
};
use gpui_component::{h_flex, v_flex, ElementExt};

use crate::app::state::ReportHover;
use crate::core::aggregation::SumStats;
use crate::core::time::east8_local;
use crate::ui::format::{format_cost_f64, format_tokens_compact_f64};
use crate::ui::{hsla_from_hex, palette};

/// Called when a cell's hover state changes. `bounds` is the cell's window
/// coordinates (recorded at prepaint), so the caller can pin a tooltip to it.
pub type HoverCallback = dyn Fn(bool, Bounds<Pixels>, NaiveDate, SumStats, &mut Window, &mut App);

/// Called when the heatmap's measured width changes (window resize / card
/// reflow), so the page can request a re-render with the new cell size.
pub type ResizeCallback = dyn Fn(&mut Window, &mut App);

/// Minimum/maximum cell size (px) the grid scales between.
const CELL_MIN: f32 = 4.0;
const CELL_MAX: f32 = 16.0;
const GAP: f32 = 3.0;
/// The tooltip's two text rows + padding, used to float it just above the
/// hovered cell without measuring the rendered box.
const TOOLTIP_HEIGHT: f32 = 44.0;
/// Gap between the tooltip and the cell it annotates.
const TOOLTIP_GAP: f32 = 8.0;
/// Left gutter reserved for the weekday labels.
const GUTTER: f32 = 30.0;
const ROWS: i64 = 7;

/// A day-keyed usage heatmap for the report page.
#[derive(Debug, Clone, Default)]
pub struct ContributionHeatmap {
    days: Vec<(NaiveDate, SumStats)>,
}

impl ContributionHeatmap {
    /// Create a heatmap from per-day stats (East-8 calendar dates).
    pub fn new(days: Vec<(NaiveDate, SumStats)>) -> Self {
        Self { days }
    }

    pub fn render(
        &self,
        hover: Option<ReportHover>,
        on_hover: &Rc<HoverCallback>,
        on_resize: &Rc<ResizeCallback>,
        cx: &App,
    ) -> AnyElement {
        let p = palette(cx);
        let today = east8_local(Utc::now()).date_naive();
        let start = grid_start(today);
        let weeks = week_count(start, today);
        let map: HashMap<NaiveDate, SumStats> = self.days.iter().copied().collect();
        let max = map
            .values()
            .map(|stats| stats.total_tokens())
            .max()
            .unwrap_or(0);
        let colors = level_colors();
        let container_bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
        // Cell size for the width measured on the previous frame; the prepaint
        // callback below re-renders us when the width changes, so the grid
        // follows window resizes within a frame.
        let cell = cell_size(container_bounds.get().size.width.as_f32(), weeks);

        // Fixed-pixel square cells sized from the measured card width, so the
        // grid shrinks/grows with the window instead of overflowing.
        let grid = h_flex().gap(px(GAP)).children((0..weeks).map(|w| {
            let week_start = start + Duration::days(w as i64 * ROWS);
            v_flex().gap(px(GAP)).children((0..ROWS).map(|row| {
                let date = week_start + Duration::days(row);
                let stats = map.get(&date).copied().unwrap_or_default();
                let level = level_for(stats.total_tokens(), max);
                let color = if level == 0 {
                    p.muted
                } else {
                    colors[level - 1]
                };
                let hover_color = if level == 0 {
                    p.border
                } else {
                    Hsla {
                        l: color.l + 0.08,
                        ..color
                    }
                };
                heat_cell(
                    color,
                    hover_color,
                    cell,
                    w as usize * ROWS as usize + row as usize,
                    date,
                    stats,
                    on_hover.clone(),
                )
            }))
        }));

        let container_bounds_prepaint = container_bounds.clone();
        let last_width = Rc::new(Cell::new(0.0_f32));
        let on_resize = on_resize.clone();
        v_flex()
            .relative()
            .gap_1()
            .on_prepaint(move |bounds, window, cx| {
                container_bounds_prepaint.set(bounds);
                let width = bounds.size.width.as_f32();
                if (width - last_width.get()).abs() > 0.5 {
                    last_width.set(width);
                    on_resize(window, cx);
                }
            })
            .child(month_row(start, today, cell, &p))
            .child(h_flex().child(weekday_labels(cell, &p)).child(grid))
            .when_some(hover, |this, hover| {
                let origin = container_bounds.get().origin;
                let width = container_bounds.get().size.width.as_f32();
                let ox = (hover.bounds.origin.x - origin.x).as_f32();
                let oy = (hover.bounds.origin.y - origin.y).as_f32();
                // Flip to the left of the cell when the tooltip would cross
                // the grid's right edge.
                let x = if ox + 140.0 > width {
                    (ox - 130.0).max(0.0)
                } else {
                    ox + 14.0
                };
                // Float above the cell, close to it; flip below only when the
                // cell is too close to the grid's top edge.
                let y = if oy >= TOOLTIP_HEIGHT + TOOLTIP_GAP {
                    oy - TOOLTIP_HEIGHT - TOOLTIP_GAP
                } else {
                    oy + hover.bounds.size.height.as_f32() + TOOLTIP_GAP
                };
                this.child(tooltip(&hover, x, y, &p))
            })
            .into_any_element()
    }
}

/// Square cell size (px) that packs `weeks` columns plus the weekday gutter
/// into `available` width, clamped so cells stay usable at extreme sizes.
fn cell_size(available: f32, weeks: usize) -> f32 {
    let usable = available - GUTTER - weeks as f32 * GAP;
    (usable / weeks as f32).clamp(CELL_MIN, CELL_MAX)
}

/// The Sunday on or before the 365th day before `today`; the grid's first
/// column is always a full week.
fn grid_start(today: NaiveDate) -> NaiveDate {
    let anchor = today - Duration::days(364);
    anchor - Duration::days(anchor.weekday().num_days_from_sunday() as i64)
}

/// Number of grid columns: one per week from `start` through `today`.
fn week_count(start: NaiveDate, today: NaiveDate) -> usize {
    ((today - start).num_days() / 7) as usize + 1
}

/// Zero-based grid column for a date, measured from `start` (a Sunday).
fn week_index(date: NaiveDate, start: NaiveDate) -> usize {
    ((date - start).num_days() / 7) as usize
}

/// Map a day's token count to a 0..=4 intensity level, linear in the window's
/// maximum (mirrors the reference crate's `LinearStrategy`).
fn level_for(value: u64, max: u64) -> usize {
    if value == 0 {
        return 0;
    }
    if max == 0 {
        return 1;
    }
    (value * 4 / max).clamp(1, 4) as usize
}

/// `(column, "N月")` labels placed above the column containing each month's
/// first day. The partial first month (whose 1st falls before `start`) is
/// skipped, like GitHub's own grid.
fn month_labels(start: NaiveDate, today: NaiveDate) -> Vec<(usize, String)> {
    let mut labels = Vec::new();
    let mut year = start.year();
    let mut month = start.month();
    loop {
        let first = NaiveDate::from_ymd_opt(year, month, 1).expect("day 1 always exists");
        if first > today {
            break;
        }
        if first >= start {
            labels.push((week_index(first, start), format!("{month}月")));
        }
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
    }
    labels
}

/// GitHub dark-mode contribution-green levels (L1..L4: #0e4429, #006d32,
/// #26a641, #39d353), as exact hex values.
pub(crate) fn level_colors() -> [Hsla; 4] {
    [
        hsla_from_hex(0x0e4429),
        hsla_from_hex(0x006d32),
        hsla_from_hex(0x26a641),
        hsla_from_hex(0x39d353),
    ]
}

/// "少 ▢▢▢▢▢ 多" intensity legend for the heatmap card.
pub(crate) fn legend(p: &crate::ui::Palette) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_1()
        .child(div().text_xs().text_color(p.muted_foreground).child("少"))
        .child(div().w(px(11.0)).h(px(11.0)).rounded(px(2.0)).bg(p.muted))
        .children(
            level_colors()
                .into_iter()
                .map(|color| div().w(px(11.0)).h(px(11.0)).rounded(px(2.0)).bg(color)),
        )
        .child(div().text_xs().text_color(p.muted_foreground).child("多"))
}

/// One cell: hover brightens it and raises the app-level tooltip state.
#[allow(clippy::too_many_arguments)]
fn heat_cell(
    color: Hsla,
    hover_color: Hsla,
    cell: f32,
    idx: usize,
    date: NaiveDate,
    stats: SumStats,
    on_hover: Rc<HoverCallback>,
) -> Stateful<Div> {
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
    let hovered = Rc::new(Cell::new(false));

    div()
        .id(ElementId::named_usize("report-heat-cell", idx))
        .w(px(cell))
        .h(px(cell))
        .rounded(px(2.0))
        .bg(color)
        .hover(move |style| style.bg(hover_color))
        .on_prepaint({
            let on_hover = on_hover.clone();
            let bounds = bounds.clone();
            let hovered = hovered.clone();
            move |b, window, cx| {
                bounds.set(b);
                if hovered.get() {
                    on_hover(true, b, date, stats, window, cx);
                }
            }
        })
        .on_hover({
            let on_hover = on_hover.clone();
            move |is_hovered, window, cx| {
                hovered.set(*is_hovered);
                on_hover(*is_hovered, bounds.get(), date, stats, window, cx);
            }
        })
}

/// Floating tooltip pinned to the hovered cell, showing that day's usage.
fn tooltip(hover: &ReportHover, x: f32, y: f32, p: &crate::ui::Palette) -> impl IntoElement {
    let weekday = match hover.date.weekday() {
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
        Weekday::Sun => "周日",
    };
    let tokens = format_tokens_compact_f64(hover.stats.total_tokens() as f64);
    let cost = format_cost_f64(hover.stats.cost_micros as f64 / 1e6);
    v_flex()
        .absolute()
        .left(px(x))
        .top(px(y))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
        .shadow_md()
        .rounded(px(6.0))
        .px_2()
        .py_1()
        .gap(px(2.0))
        .child(
            div()
                .text_xs()
                .text_color(p.muted_foreground)
                .child(format!("{} {weekday}", hover.date.format("%Y-%m-%d"))),
        )
        .child(
            div()
                .text_xs()
                .text_color(p.foreground)
                .child(format!("总 Token {tokens} · 花费 {cost}")),
        )
}

/// Weekday labels for the Sunday-first rows (Mon/Wed/Fri, like GitHub). The
/// grid's first row is always a Sunday, so the Mon/Wed/Fri labels sit on the
/// second/fourth/sixth rows (rows 1, 3, 5).
fn weekday_labels(cell: f32, p: &crate::ui::Palette) -> impl IntoElement {
    v_flex().gap(px(GAP)).children((0..ROWS).map(|row| {
        let label = match row {
            1 => "一",
            3 => "三",
            5 => "五",
            _ => "",
        };
        div()
            .w(px(GUTTER))
            .h(px(cell))
            .flex()
            .items_center()
            .justify_end()
            .pr_1()
            .text_xs()
            .text_color(p.muted_foreground)
            .child(label.to_string())
    }))
}

/// Month labels above the grid columns; same fixed-width columns and gaps as
/// the grid, so labels stay aligned as the heatmap scales.
fn month_row(
    start: NaiveDate,
    today: NaiveDate,
    cell: f32,
    p: &crate::ui::Palette,
) -> impl IntoElement {
    let labels: HashMap<usize, String> = month_labels(start, today).into_iter().collect();
    h_flex()
        .gap(px(GAP))
        .child(div().w(px(GUTTER)))
        .children((0..week_count(start, today)).map(|col| {
            v_flex().w(px(cell)).relative().h(px(16.0)).when_some(
                labels.get(&col),
                |this, label| {
                    this.child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(label.clone()),
                    )
                },
            )
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use gpui::{point, AnyWindowHandle, AppContext, Context, Render, TestAppContext, Window};

    /// Stand-in for the report page: renders the heatmap and routes hover
    /// events back into its own state, mirroring `page.rs::hover_callback`.
    struct HeatmapHarness {
        heatmap: ContributionHeatmap,
        hover: Option<ReportHover>,
        /// `(hovered, date, total_tokens)` transitions observed so far.
        events: Rc<RefCell<Vec<(bool, NaiveDate, u64)>>>,
        /// The cell bounds recorded when the hover callback fired; must be
        /// non-zero so the tooltip can be pinned to the right spot.
        hover_bounds: Rc<RefCell<Bounds<Pixels>>>,
        /// True once a render included the tooltip (i.e. `hover` was set).
        tooltip_seen: Rc<Cell<bool>>,
        render_count: Rc<Cell<usize>>,
    }

    impl Render for HeatmapHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.render_count.set(self.render_count.get() + 1);
            if self.hover.is_some() {
                self.tooltip_seen.set(true);
            }
            let weak = cx.weak_entity();
            let on_hover: Rc<HoverCallback> =
                Rc::new(move |is_hovered, bounds, date, stats, _window, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        view.events
                            .borrow_mut()
                            .push((is_hovered, date, stats.total_tokens()));
                        if is_hovered {
                            *view.hover_bounds.borrow_mut() = bounds;
                        }
                        view.hover = if is_hovered {
                            Some(ReportHover {
                                date,
                                stats,
                                bounds,
                            })
                        } else {
                            None
                        };
                        cx.notify();
                    });
                });
            let on_resize: Rc<ResizeCallback> = Rc::new(|_window, _cx| {});
            // Nest the heatmap under an offset parent, mirroring the report
            // page's card/shell hierarchy, so coordinate-space bugs in tooltip
            // pinning show up as wrong positions here too.
            v_flex()
                .pt(px(200.0))
                .child(self.heatmap.render(self.hover, &on_hover, &on_resize, cx))
        }
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn level_maps_linear_intensity() {
        assert_eq!(level_for(0, 100), 0);
        assert_eq!(level_for(1, 100), 1);
        assert_eq!(level_for(50, 100), 2);
        assert_eq!(level_for(100, 100), 4);
        assert_eq!(level_for(7, 0), 1);
    }

    #[test]
    fn grid_start_is_a_sunday_within_365_days() {
        // 2026-08-19 is a Wednesday; anchor = 2025-08-20 (Wednesday), so the
        // grid starts on the Sunday before: 2025-08-17. The 53-week grid can
        // span up to 370 days before today.
        let start = grid_start(day(2026, 8, 19));
        assert_eq!(start.weekday(), chrono::Weekday::Sun);
        let span = day(2026, 8, 19) - start;
        assert!(span >= Duration::days(364) && span <= Duration::days(370));
        assert_eq!(week_count(start, day(2026, 8, 19)), 53);
    }

    #[test]
    fn week_count_rounds_up_partial_weeks() {
        let start = day(2026, 8, 16); // Sunday
        assert_eq!(week_count(start, start + Duration::days(6)), 1);
        assert_eq!(week_count(start, start + Duration::days(7)), 2);
        assert_eq!(week_count(start, start + Duration::days(364)), 53);
    }

    #[test]
    fn month_labels_skip_partial_first_month() {
        let start = day(2026, 1, 4); // Sunday, Jan 1 is before it
        let today = day(2026, 3, 31);
        let labels = month_labels(start, today);
        assert_eq!(labels, vec![(4, "2月".to_string()), (8, "3月".to_string())]);
    }

    #[gpui::test]
    fn hovering_a_cell_raises_and_renders_the_tooltip(cx: &mut TestAppContext) {
        // Mirror the app bootstrap: the theme global must exist before any UI
        // code (e.g. `palette`) reads it.
        cx.update(|cx| gpui_component::init(cx));

        let today = day(2026, 8, 19);
        let first_cell = grid_start(today);
        let events = Rc::new(RefCell::new(Vec::new()));
        let hover_bounds = Rc::new(RefCell::new(Bounds::default()));
        let tooltip_seen = Rc::new(Cell::new(false));
        let render_count = Rc::new(Cell::new(0));
        let view = cx.add_window({
            let events = events.clone();
            let hover_bounds = hover_bounds.clone();
            let tooltip_seen = tooltip_seen.clone();
            let render_count = render_count.clone();
            move |_window, _cx| HeatmapHarness {
                heatmap: ContributionHeatmap::new(vec![(
                    first_cell,
                    SumStats {
                        input_tokens: 100,
                        output_tokens: 50,
                        ..Default::default()
                    },
                )]),
                hover: None,
                events,
                hover_bounds,
                tooltip_seen,
                render_count,
            }
        });
        let any_window = AnyWindowHandle::from(view);

        // The harness nests the heatmap under a 200px-tall padding, so the
        // first cell sits at (GUTTER=30, 200 + month row 16 + gap 4) and is
        // 11x11px; (35, 225) is its center.
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_mouse_move(point(px(35.0), px(225.0)), cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();

        assert_eq!(
            *events.borrow(),
            vec![(true, first_cell, 150)],
            "hovering the cell should raise the day's tooltip state"
        );
        let bounds = *hover_bounds.borrow();
        assert!(
            bounds.size.width > px(0.0) && bounds.size.height > px(0.0),
            "hover should carry the cell's painted bounds for tooltip pinning"
        );

        // One more frame so the notify from the hover callback reaches the
        // element tree; the tooltip must then be part of the render.
        cx.update_window(any_window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert!(
            tooltip_seen.get(),
            "the tooltip should render once hover state is set"
        );
        assert!(
            render_count.get() >= 3,
            "hover should drive re-renders (got {})",
            render_count.get()
        );
    }
}
