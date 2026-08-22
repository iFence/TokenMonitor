//! Ratatui rendering for the TUI frontend: header, summary cards, and a
//! keyboard-navigable GitHub-style contribution heatmap.

use chrono::{Datelike, Duration, Weekday};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::core::aggregation::SumStats;
use crate::format::{format_cost_f64, format_tokens_compact_f64};
use crate::report::heatmap::{level_for, month_labels, ROWS};
use crate::report::{report_stats, ReportStats};
use crate::tui::app::TuiApp;

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

/// One summary card: grey label + bold value.
fn stat_card(label: &str, value: String) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!("{label} "), Style::default().fg(Color::Gray)),
        Span::styled(value, Style::default().add_modifier(Modifier::BOLD)),
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

/// Render the whole screen.
pub fn draw(frame: &mut Frame, app: &TuiApp) {
    let [header, stats_area, heatmap, breakdown, detail, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        // Month label + 7 weekday rows + legend.
        Constraint::Length(9),
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
    frame.render_widget(Paragraph::new(Line::from(header_spans)), header);

    // Summary cards.
    let summary = Text::from(vec![
        stats_line(vec![
            stat_card(
                "总Token",
                format_tokens_compact_f64(stats.total.total_tokens() as f64),
            ),
            stat_card(
                "总花费",
                format_cost_f64(stats.total.cost_micros as f64 / 1e6),
            ),
            stat_card("活跃天数", format!("{} 天", stats.active_days)),
        ]),
        stats_line(vec![
            stat_card("最长连续", format!("{} 天", stats.longest_streak)),
            stat_card("当前连续", format!("{} 天", stats.current_streak)),
            stat_card("最忙一天", busiest_label(&stats)),
        ]),
    ]);
    frame.render_widget(Paragraph::new(summary), stats_area);

    // Heatmap. The grid doubles its cells when it fits the heatmap width, so
    // the heatmap fills wider terminals too.
    frame.render_widget(
        Paragraph::new(Text::from(heatmap_lines(app, heatmap.width as usize))),
        heatmap,
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
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(" 退出", Style::default().fg(Color::Gray)),
            Span::raw("  |  "),
            Span::styled("r", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(" 重新扫描", Style::default().fg(Color::Gray)),
            Span::raw("  |  "),
            Span::styled("t/T", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(" 时间范围", Style::default().fg(Color::Gray)),
            Span::raw("  |  "),
            Span::styled(
                "←→↑↓ / hjkl / Home / End",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(" 移动选中", Style::default().fg(Color::Gray)),
        ])),
        hint,
    );
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

/// Maximum number of rows shown per breakdown column.
const BREAKDOWN_ROWS: usize = 8;

/// Bordered block with a centered bold title, e.g. "Agent 用量 · 本月".
fn breakdown_block(title: &str, range: &str) -> Block<'static> {
    Block::bordered().title(Span::styled(
        format!(" {title} · {range} "),
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

/// Per-provider ("Agent") totals for the selected range: rank + name + totals.
fn agent_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    let entries = app
        .by_provider()
        .iter()
        .map(|(provider, s)| (provider.display_name().to_string(), *s))
        .collect::<Vec<_>>();
    breakdown_rows(&entries, width)
}

/// Per-model totals for the selected range, cost-descending.
fn model_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    breakdown_rows(app.by_model(), width)
}

/// Ranked `(name, stats)` rows with a rank badge, the name in white, and the
/// token/cost figures right-aligned to the column's inner width (borders take
/// two columns).
fn breakdown_rows(entries: &[(String, SumStats)], width: usize) -> Vec<Line<'static>> {
    if entries.is_empty() {
        return vec![Line::from(vec![Span::styled(
            "暂无记录",
            Style::default().fg(Color::DarkGray),
        )])];
    }

    let inner = width.saturating_sub(2);
    let mut lines = Vec::new();
    for (i, (name, stats)) in entries.iter().take(BREAKDOWN_ROWS).enumerate() {
        let rank_style = if i == 0 {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let stat = format!(
            "{}  {}",
            format_tokens_compact_f64(stats.total_tokens() as f64),
            format_cost_f64(stats.cost_micros as f64 / 1e6)
        );
        let prefix_w = 3; // rank (2) + separator space
        let stat_w = stat.chars().count();
        let name_max = inner.saturating_sub(prefix_w + stat_w);
        let name = truncate(name, name_max);
        let pad = inner.saturating_sub(prefix_w + name.chars().count() + stat_w);
        lines.push(Line::from(vec![
            Span::styled(format!("{:>2}", i + 1), rank_style),
            Span::raw(" "),
            Span::styled(name, Style::default().fg(Color::White)),
            Span::raw(" ".repeat(pad)),
            Span::styled(stat, Style::default().fg(Color::Gray)),
        ]));
    }
    lines
}

/// Truncate `s` to at most `max` display chars, appending an ellipsis when cut.
fn truncate(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }
    match max {
        0 => String::new(),
        1 => "…".to_string(),
        _ => {
            let mut out: String = s.chars().take(max - 1).collect();
            out.push('…');
            out
        }
    }
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
