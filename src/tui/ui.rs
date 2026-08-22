//! Ratatui rendering for the TUI frontend: header, summary cards, and a
//! keyboard-navigable GitHub-style contribution heatmap.

use chrono::{Datelike, Duration, Weekday};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::core::aggregation::SumStats;
use crate::core::update::UpdateState;
use crate::format::{format_cost_f64, format_tokens_compact_f64};
use crate::report::heatmap::{level_for, month_labels, ROWS};
use crate::report::{report_stats, ReportStats};
use crate::tui::app::{TuiApp, TuiView};

/// The five grid levels' background colors, GitHub dark-mode green ramp.
fn level_color(level: usize) -> Color {
    match level {
        0 => Color::Rgb(0x16, 0x1b, 0x22),
        1 => Color::Rgb(0x0e, 0x44, 0x29),
        2 => Color::Rgb(0x00, 0x6d, 0x32),
        3 => Color::Rgb(0x26, 0xa6, 0x41),
        _ => Color::Rgb(0x39, 0xd3, 0x53),
    }
}

/// One summary card: grey label + bold colored value.
fn stat_card(label: &str, value: String, color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!("{label} "), Style::default().fg(Color::Gray)),
        Span::styled(
            value,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]
}

fn stats_line(cards: Vec<Vec<Span<'static>>>) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, card) in cards.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("    "));
        }
        spans.extend(card);
    }
    Line::from(spans)
}

fn weekday_label(row: i64) -> Span<'static> {
    let label = match row {
        1 => "一",
        3 => "三",
        5 => "五",
        _ => "  ",
    };
    Span::styled(label, Style::default().fg(Color::Gray))
}

/// Render the whole screen, dispatching on the selected panel.
pub fn draw(frame: &mut Frame, app: &TuiApp) {
    match app.view() {
        TuiView::Overview => draw_overview(frame, app),
        TuiView::TodayHourly => draw_hourly(frame, app),
        TuiView::Updates => draw_updates(frame, app),
    }
}

/// The overview: summary cards, heatmap, per-hour chart, and agent/model
/// breakdowns stacked in one screen.
fn draw_overview(frame: &mut Frame, app: &TuiApp) {
    let [header, stats_area, heatmap, _gap1, hours, _gap2, breakdown, detail, hint] =
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            // Month label + 7 weekday rows + legend.
            Constraint::Length(9),
            // Blank spacer between the heatmap and the hour chart.
            Constraint::Length(1),
            // Per-hour "today" bar chart: title + 4 bar rows + tick labels.
            Constraint::Length(6),
            // Blank spacer between the hour chart and the breakdown.
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

    let stats = report_stats(app.range_days(), app.today());

    // Header: title + selected range + status + last scan summary.
    let mut header_spans = vec![
        Span::styled(
            "TokenMonitor TUI",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" [{}]", app.range().label()),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("   "),
        Span::styled(&app.status, Style::default().fg(Color::Gray)),
    ];
    if let Some(last) = &app.last_scan {
        header_spans.push(Span::styled(
            format!("  |  上次：{last}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let UpdateState::Available { latest_version, .. } = app.update_state() {
        header_spans.push(Span::styled(
            format!("  |  ⬆ 新版本 v{latest_version} 可用"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header_spans)), header);

    // Summary cards (all on one line), with colored values.
    let summary = Text::from(vec![stats_line(vec![
        stat_card(
            "总Token",
            format_tokens_compact_f64(stats.total.total_tokens() as f64),
            Color::Cyan,
        ),
        stat_card(
            "总花费",
            format_cost_f64(stats.total.cost_micros as f64 / 1e6),
            Color::Yellow,
        ),
        stat_card(
            "活跃天数",
            format!("{} 天", stats.active_days),
            Color::Green,
        ),
        stat_card(
            "最长连续",
            format!("{} 天", stats.longest_streak),
            Color::Magenta,
        ),
        stat_card(
            "当前连续",
            format!("{} 天", stats.current_streak),
            Color::Magenta,
        ),
        stat_card("最忙一天", busiest_label(&stats), Color::Blue),
    ])]);
    frame.render_widget(Paragraph::new(summary), stats_area);

    // Heatmap. The grid doubles its cells when it fits the heatmap width, so
    // the heatmap fills wider terminals too.
    frame.render_widget(
        Paragraph::new(Text::from(heatmap_lines(app, heatmap.width as usize))),
        heatmap,
    );

    // Per-hour bar chart of today's (East-8) usage.
    frame.render_widget(
        Paragraph::new(Text::from(hour_chart(app, hours.width as usize))),
        hours,
    );

    // Per-agent and per-model breakdown, side by side.
    let [agents, models] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(breakdown);
    frame.render_widget(
        Paragraph::new(Text::from(agent_lines(app, agents.width as usize)))
            .block(breakdown_block("Agent 用量", app.range().label())),
        agents,
    );
    frame.render_widget(
        Paragraph::new(Text::from(model_lines(app, models.width as usize)))
            .block(breakdown_block("模型用量", app.range().label())),
        models,
    );

    // Selected-day detail.
    frame.render_widget(Paragraph::new(detail_line(app)), detail);

    // Key hints.
    frame.render_widget(Paragraph::new(hint_line(app)), hint);
}

/// The hint bar shared by all panels.
fn hint_line(app: &TuiApp) -> Line<'static> {
    let mut spans = vec![
        Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(" 退出", Style::default().fg(Color::Gray)),
        Span::raw("  |  "),
        Span::styled("r", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(" 重新扫描", Style::default().fg(Color::Gray)),
        Span::raw("  |  "),
        Span::styled("t/T", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(" 时间范围", Style::default().fg(Color::Gray)),
        Span::raw("  |  "),
        Span::styled("Tab", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(" 切换面板", Style::default().fg(Color::Gray)),
        Span::raw("  |  "),
        Span::styled("u", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(" 更新检查", Style::default().fg(Color::Gray)),
        Span::raw("  |  "),
        Span::styled(
            "←→↑↓ / hjkl / Home / End",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(" 移动选中", Style::default().fg(Color::Gray)),
    ];
    if app.update_state().has_update() {
        spans.extend([
            Span::raw("  |  "),
            Span::styled(
                "d 下载 · s 跳过",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
    }
    Line::from(spans)
}

fn busiest_label(stats: &ReportStats) -> String {
    match stats.busiest {
        Some((date, s)) => format!(
            "{} · {}",
            date.format("%m-%d"),
            format_tokens_compact_f64(s.total_tokens() as f64)
        ),
        None => "—".to_string(),
    }
}

/// Month-label row + the seven weekday rows + a legend.
///
/// Each day is one column by default; when the grid fits in `body_width` each
/// cell doubles to two columns so the heatmap fills the terminal and looks less
/// cramped.
fn heatmap_lines(app: &TuiApp, body_width: usize) -> Vec<Line<'static>> {
    let map = app.day_stats();
    let max = map.values().map(|s| s.total_tokens()).max().unwrap_or(0);
    let start = app.start();
    let weeks = app.weeks();
    let selected = app.selection();

    // Each day cell is `cell_w` columns wide; double up when there's room.
    let cell_w = if weeks * 2 + 3 <= body_width { 2 } else { 1 };
    let full_w = weeks * cell_w;

    // Month labels above the column that contains each month's 1st.
    let mut month_row: Vec<char> = vec![' '; full_w];
    for (col, label) in month_labels(start, app.today()) {
        let pos = col * cell_w;
        for (i, c) in label.chars().enumerate() {
            if pos + i < full_w {
                month_row[pos + i] = c;
            }
        }
    }
    let mut lines = vec![Line::from(vec![
        Span::raw("   "),
        Span::styled(
            month_row.into_iter().collect::<String>(),
            Style::default().fg(Color::Gray),
        ),
    ])];

    for row in 0..ROWS {
        let mut spans = vec![weekday_label(row), Span::raw(" ")];
        for week in 0..weeks {
            let date = start + Duration::days(week as i64 * ROWS + row);
            let level = map
                .get(&date)
                .map(|s| level_for(s.total_tokens(), max))
                .unwrap_or(0);
            let mut style = Style::default().bg(level_color(level));
            if (week, row as usize) == selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(" ".repeat(cell_w), style));
        }
        lines.push(Line::from(spans));
    }

    // Legend: swatches plus how many days fall into each intensity level.
    let mut counts = [0usize; 5];
    for (date, s) in &map {
        if *date >= start && *date <= app.today() {
            counts[level_for(s.total_tokens(), max)] += 1;
        }
    }
    let mut legend = vec![
        Span::styled("少", Style::default().fg(Color::Gray)),
        Span::raw(" "),
    ];
    for level in 0..=4 {
        legend.push(Span::styled(
            " ".repeat(cell_w),
            Style::default().bg(level_color(level)),
        ));
        legend.push(Span::raw(" "));
    }
    legend.push(Span::styled("多", Style::default().fg(Color::Gray)));
    let histogram = counts
        .iter()
        .enumerate()
        .map(|(level, count)| format!("{level}级 {count}天"))
        .collect::<Vec<_>>()
        .join("  ");
    legend.push(Span::styled(
        format!("  ·  {histogram}"),
        Style::default().fg(Color::DarkGray),
    ));
    lines.push(Line::from(legend));
    lines
}

/// Number of bar rows in the per-hour chart.
const BAR_ROWS: usize = 4;

/// Per-hour bar chart of today's East-8 usage: a title row, `BAR_ROWS` rows of
/// vertical bars, and an hour-tick row (every 6 hours). Each hour is one cell
/// by default, doubled to two when the terminal is wide enough. Bars are drawn
/// bottom-up and colored by the heatmap's green ramp; the current hour is
/// yellow.
fn hour_chart(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    use chrono::{Timelike, Utc};

    use crate::core::time::east8_local;

    let hours = app.hour_tokens();
    let total: u64 = hours.iter().map(|s| s.total_tokens()).sum();
    let max = hours.iter().map(|s| s.total_tokens()).max().unwrap_or(0);
    let now_hour = east8_local(Utc::now()).hour() as usize;

    let cell_w = if 24 * 2 <= width { 2 } else { 1 };
    let full_w = 24 * cell_w;

    let title = if total == 0 {
        "当日分时 · 今日 · 暂无记录".to_string()
    } else {
        format!(
            "当日分时 · 今日 · 峰值 {} / 时",
            format_tokens_compact_f64(max as f64)
        )
    };

    let mut tick_row: Vec<char> = vec![' '; full_w];
    for h in [0, 6, 12, 18, 23] {
        let col = h * cell_w;
        let label = format!("{h:02}");
        for (i, c) in label.chars().enumerate() {
            if col + i < full_w {
                tick_row[col + i] = c;
            }
        }
    }

    let bar_height = |v: u64| -> usize {
        if v == 0 {
            return 0;
        }
        let h = ((v as f64 / max as f64) * BAR_ROWS as f64).round() as usize;
        h.max(1).min(BAR_ROWS)
    };

    let mut bar_rows: Vec<Vec<Span<'static>>> = vec![Vec::with_capacity(24); BAR_ROWS];
    for (h, stats) in hours.iter().enumerate() {
        let v = stats.total_tokens();
        let height = bar_height(v);
        let style = if h == now_hour {
            Style::default().fg(Color::Yellow)
        } else {
            let level = ((v as f64 / max as f64) * 7.0).round() as usize;
            Style::default().fg(level_color((level * 4 / 7).clamp(1, 4)))
        };
        for (r, row) in bar_rows.iter_mut().enumerate() {
            let filled = height >= BAR_ROWS - r;
            if filled {
                row.push(Span::styled("█".repeat(cell_w), style));
            } else {
                row.push(Span::raw(" ".repeat(cell_w)));
            }
        }
    }

    let mut lines = vec![Line::from(vec![Span::styled(
        title,
        Style::default().fg(Color::Gray),
    )])];
    for row in bar_rows {
        lines.push(Line::from(row));
    }
    lines.push(Line::from(vec![Span::styled(
        tick_row.into_iter().collect::<String>(),
        Style::default().fg(Color::DarkGray),
    )]));
    lines
}

/// Full-screen "today hourly" panel: a two-column list of today's 24 hours
/// with tokens, cost, and a mini bar scaled to the busiest hour. The current
/// hour is highlighted in yellow.
fn draw_hourly(frame: &mut Frame, app: &TuiApp) {
    let [panel, hint] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

    let today = app.today();
    let max = app
        .hour_tokens()
        .iter()
        .map(|s| s.total_tokens())
        .max()
        .unwrap_or(0);
    let title = if max == 0 {
        format!(" 今日分时 · {} · 暂无记录 ", today.format("%Y-%m-%d"))
    } else {
        format!(
            " 今日分时 · {} · 峰值 {} / 时 ",
            today.format("%Y-%m-%d"),
            format_tokens_compact_f64(max as f64)
        )
    };
    frame.render_widget(
        Paragraph::new(Text::from(hourly_lines(app, panel.width as usize))).block(
            Block::bordered().title(Span::styled(
                title,
                Style::default().add_modifier(Modifier::BOLD),
            )),
        ),
        panel,
    );
    frame.render_widget(Paragraph::new(hint_line(app)), hint);
}

/// Full-screen "update check" panel: current version, available update with
/// release notes, download / skip actions, or the last check result.
fn draw_updates(frame: &mut Frame, app: &TuiApp) {
    let [panel, hint] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

    let inner_w = panel.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let push = |lines: &mut Vec<Line<'static>>, text: String, color: Color| {
        lines.extend(
            wrap_text(&text, inner_w)
                .into_iter()
                .map(|t| Line::from(vec![Span::styled(t, Style::default().fg(color))])),
        );
    };

    lines.push(Line::from(vec![Span::styled(
        format!(" 当前版本 v{} ", env!("CARGO_PKG_VERSION")),
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![]));

    match app.update_state() {
        UpdateState::Idle => {
            push(
                &mut lines,
                "尚未检查更新。按 u 立即检查。".into(),
                Color::Gray,
            );
        }
        UpdateState::Checking => {
            push(&mut lines, "正在检查更新…".into(), Color::Cyan);
        }
        UpdateState::UpToDate => {
            push(
                &mut lines,
                format!("当前已是最新版本 v{}。", env!("CARGO_PKG_VERSION")),
                Color::Green,
            );
        }
        UpdateState::Error(err) => {
            push(&mut lines, format!("检查更新失败：{err}"), Color::Red);
            push(&mut lines, "按 u 重试。".into(), Color::Gray);
        }
        UpdateState::Available {
            latest_version,
            release_notes,
            asset,
        } => {
            push(
                &mut lines,
                format!(
                    "发现新版本 v{latest_version}（当前 v{}）",
                    env!("CARGO_PKG_VERSION")
                ),
                Color::Yellow,
            );
            push(
                &mut lines,
                format!("更新包：{}（{}）", asset.name, format_bytes(asset.size)),
                Color::Cyan,
            );
            lines.push(Line::from(vec![]));
            lines.push(Line::from(vec![Span::styled(
                "更新说明".to_string(),
                Style::default().fg(Color::DarkGray),
            )]));
            push(&mut lines, release_notes.clone(), Color::Gray);
            lines.push(Line::from(vec![]));
            lines.push(Line::from(vec![Span::styled(
                "按 d 下载 · s 跳过",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
        }
        UpdateState::Downloading { .. } => {
            push(&mut lines, "正在下载更新包…".into(), Color::Cyan);
        }
        UpdateState::Installing => {
            push(&mut lines, "正在安装…".into(), Color::Cyan);
        }
        UpdateState::Downloaded {
            latest_version,
            file_name,
        } => {
            push(
                &mut lines,
                format!("已下载 v{latest_version} 的更新包 {file_name}"),
                Color::Green,
            );
            if let Some(dest) = app.update_dest() {
                push(
                    &mut lines,
                    format!("保存位置：{}", dest.display()),
                    Color::Gray,
                );
            }
            push(
                &mut lines,
                "请解压覆盖（免安装版）或运行安装程序后重启。".into(),
                Color::Gray,
            );
        }
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::bordered().title(Span::styled(
            " 更新检查 ",
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        panel,
    );
    frame.render_widget(Paragraph::new(hint_line(app)), hint);
}

/// Human-readable byte count for the update asset.
fn format_bytes(size: u64) -> String {
    if size >= 1 << 20 {
        format!("{:.1} MB", size as f64 / (1 << 20) as f64)
    } else if size >= 1 << 10 {
        format!("{:.1} KB", size as f64 / (1 << 10) as f64)
    } else {
        format!("{size} B")
    }
}

/// Wrap `text` to `width` display columns, preserving blank lines.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut line = String::new();
        let mut used = 0usize;
        for c in raw.chars() {
            let w = if c.is_ascii() { 1 } else { 2 };
            if used + w > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(c);
            used += w;
        }
        out.push(line);
    }
    out
}

/// The 24 hours laid out as two side-by-side columns of 12 rows each.
fn hourly_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    use crate::core::time::east8_local;

    use chrono::Timelike;
    use chrono::Utc;

    let hours = app.hour_tokens();
    let max = hours.iter().map(|s| s.total_tokens()).max().unwrap_or(0);
    let now_hour = east8_local(Utc::now()).hour() as usize;

    let inner = width.saturating_sub(2);
    let gap_w = 2;
    let col_w = inner.saturating_sub(gap_w) / 2;

    let mut lines = Vec::with_capacity(12);
    for r in 0..12 {
        let mut spans = Vec::new();
        spans.extend(hour_cell(&hours[r], r, max, now_hour, col_w));
        spans.push(Span::raw(" ".repeat(gap_w)));
        spans.extend(hour_cell(&hours[r + 12], r + 12, max, now_hour, col_w));
        lines.push(Line::from(spans));
    }
    lines
}

/// One hour cell: a `●` marker for the current hour, `HH时`, compact tokens,
/// cost, and a mini bar scaled to `max`. Empty hours show an em-dash.
fn hour_cell(
    stats: &SumStats,
    h: usize,
    max: u64,
    now_hour: usize,
    width: usize,
) -> Vec<Span<'static>> {
    let tokens = stats.total_tokens();
    let is_now = h == now_hour;

    let mut content = String::new();
    content.push(if is_now { '●' } else { ' ' });
    content.push_str(&format!("{h:02}时 "));
    if tokens > 0 {
        content.push_str(&format_tokens_compact_f64(tokens as f64));
        content.push_str("  ");
        content.push_str(&format_cost_f64(stats.cost_micros as f64 / 1e6));
        if max > 0 {
            let len = ((tokens as f64 / max as f64) * 8.0).ceil() as usize;
            content.push(' ');
            content.push_str(&"█".repeat(len.min(8)));
        }
    } else {
        content.push('—');
    }

    let fg = if is_now {
        Color::Yellow
    } else if tokens > 0 {
        Color::White
    } else {
        Color::DarkGray
    };
    vec![Span::styled(
        pad_right(&truncate(&content, width), width),
        Style::default().fg(fg),
    )]
}

/// Maximum number of rows shown per breakdown column.
const BREAKDOWN_ROWS: usize = 8;

/// Bordered block with a centered bold title, e.g. "Agent 用量 · 本月".
fn breakdown_block(title: &str, range: &str) -> Block<'static> {
    Block::bordered().title(Span::styled(
        format!(" {title} · {range} "),
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

/// Per-provider ("Agent") totals for the selected range.
fn agent_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    let entries = app
        .by_provider()
        .iter()
        .map(|(provider, s)| (provider.display_name().to_string(), *s))
        .collect::<Vec<_>>();
    breakdown_rows(&entries, width)
}

/// Per-model totals for the selected range, token-descending.
fn model_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    breakdown_rows(app.by_model(), width)
}

/// Three columns per row — name, tokens, cost — each right-padded/left-padded
/// to its own display-width column so the figures line up vertically. Tokens
/// and cost are separate columns, so a short token count never pushes the
/// price out of alignment (the `\t` the user suggested would not help: ratatui
/// renders tabs as zero-width, so explicit column padding is used instead).
fn breakdown_rows(entries: &[(String, SumStats)], width: usize) -> Vec<Line<'static>> {
    if entries.is_empty() {
        return vec![Line::from(vec![Span::styled(
            "暂无记录",
            Style::default().fg(Color::DarkGray),
        )])];
    }

    let token_strs: Vec<String> = entries
        .iter()
        .map(|(_, s)| format_tokens_compact_f64(s.total_tokens() as f64))
        .collect();
    let cost_strs: Vec<String> = entries
        .iter()
        .map(|(_, s)| format_cost_f64(s.cost_micros as f64 / 1e6))
        .collect();
    let token_w = token_strs
        .iter()
        .map(|s| display_width(s))
        .max()
        .unwrap_or(1)
        .min(8);
    let cost_w = cost_strs
        .iter()
        .map(|s| display_width(s))
        .max()
        .unwrap_or(1)
        .min(10);

    let inner = width.saturating_sub(2);
    let sep_w = 2;
    let name_w = inner.saturating_sub(token_w + cost_w + sep_w * 2);

    let mut lines = Vec::new();
    for (((name, _), token), cost) in entries
        .iter()
        .zip(&token_strs)
        .zip(&cost_strs)
        .take(BREAKDOWN_ROWS)
    {
        lines.push(Line::from(vec![
            Span::styled(
                pad_right(&truncate(name, name_w), name_w),
                Style::default().fg(Color::White),
            ),
            Span::raw("  "),
            Span::styled(pad_left(token, token_w), Style::default().fg(Color::Gray)),
            Span::raw("  "),
            Span::styled(pad_left(cost, cost_w), Style::default().fg(Color::Yellow)),
        ]));
    }
    lines
}

/// Display width of `s` in terminal columns (non-ASCII counts as 2).
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

/// Left-pad `s` to `width` display columns.
fn pad_left(s: &str, width: usize) -> String {
    let pad = width.saturating_sub(display_width(s));
    format!("{}{s}", " ".repeat(pad))
}

/// Right-pad `s` to `width` display columns.
fn pad_right(s: &str, width: usize) -> String {
    let pad = width.saturating_sub(display_width(s));
    format!("{s}{}", " ".repeat(pad))
}

/// Truncate `s` to at most `max` display columns, appending an ellipsis when
/// cut.
fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let w = if c.is_ascii() { 1 } else { 2 };
        if used + w >= max {
            break;
        }
        out.push(c);
        used += w;
    }
    if used < display_width(s) {
        out.push('…');
    }
    out
}

fn weekday_name(wd: Weekday) -> &'static str {
    match wd {
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
        Weekday::Sun => "周日",
    }
}

/// The selected day's details (or "no records").
fn detail_line(app: &TuiApp) -> Line<'static> {
    let date = app.selected_date();
    let mut spans = vec![
        Span::styled(
            format!("选中 {}", date.format("%Y-%m-%d")),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {weekday}", weekday = weekday_name(date.weekday())),
            Style::default().fg(Color::Gray),
        ),
    ];
    match app.day_stats().get(&date) {
        Some(s) => spans.push(Span::raw(format!(
            "  ·  总Token {} · 花费 {}",
            format_tokens_compact_f64(s.total_tokens() as f64),
            format_cost_f64(s.cost_micros as f64 / 1e6)
        ))),
        None => spans.push(Span::styled(
            "  ·  无记录",
            Style::default().fg(Color::DarkGray),
        )),
    }
    Line::from(spans)
}
