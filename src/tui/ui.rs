//! Ratatui rendering for the TUI frontend: header, summary cards, and a
//! keyboard-navigable GitHub-style contribution heatmap.

use chrono::{Datelike, Duration, Weekday};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

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
        _ => " ",
    };
    Span::styled(label, Style::default().fg(Color::Gray))
}

/// Render the whole screen.
pub fn draw(frame: &mut Frame, app: &TuiApp) {
    let [header, stats_area, body, detail, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let stats = report_stats(app.days(), app.today());

    // Header: title + status + last scan summary.
    let mut header_spans = vec![
        Span::styled(
            "TokenMonitor TUI",
            Style::default().add_modifier(Modifier::BOLD),
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

    // Heatmap.
    frame.render_widget(Paragraph::new(Text::from(heatmap_lines(app))), body);

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
            Span::styled(
                "←→↑↓ / Home / End",
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
fn heatmap_lines(app: &TuiApp) -> Vec<Line<'static>> {
    let map = app.day_stats();
    let max = map.values().map(|s| s.total_tokens()).max().unwrap_or(0);
    let start = app.start();
    let weeks = app.weeks();
    let selected = app.selection();

    // Month labels above the columns that contain each month's 1st.
    let mut month_row: Vec<char> = vec![' '; weeks];
    for (col, label) in month_labels(start, app.today()) {
        for (i, c) in label.chars().enumerate() {
            if col + i < weeks {
                month_row[col + i] = c;
            }
        }
    }
    let mut lines = vec![Line::from(vec![
        Span::raw("  "),
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
            spans.push(Span::styled(" ", style));
        }
        lines.push(Line::from(spans));
    }

    let mut legend = vec![
        Span::styled("少", Style::default().fg(Color::Gray)),
        Span::raw(" "),
    ];
    for level in 0..=4 {
        legend.push(Span::styled("  ", Style::default().bg(level_color(level))));
    }
    legend.push(Span::styled(" 多", Style::default().fg(Color::Gray)));
    lines.push(Line::from(legend));
    lines
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
