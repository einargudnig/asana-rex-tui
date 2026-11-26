mod detail;

use crate::app::{App, Row, Tab};
use crate::asana::Task;
use crate::buckets::Bucket;
use crate::rex;
use chrono::{Local, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const ACCENT: Color = Color::Rgb(122, 162, 247);
const DIM: Color = Color::Rgb(110, 118, 140);
const FAINT: Color = Color::Rgb(68, 75, 98);
const TEXT: Color = Color::Rgb(192, 202, 245);
const DONE: Color = Color::Rgb(158, 206, 106);
const WARN: Color = Color::Rgb(224, 175, 104);
const LATE: Color = Color::Rgb(247, 118, 142);
const SELECTED_BG: Color = Color::Rgb(41, 46, 66);
const RULE: Color = Color::Rgb(52, 58, 80);

/// Below this width the details pane stacks under the list instead of beside it.
const SIDE_BY_SIDE_W: u16 = 110;

pub fn draw(f: &mut Frame, app: &App) {
    draw_screen(f, app);
    if app.help {
        draw_help(f);
    }
}

/// Pads or truncates to exactly `width` display columns.
fn fit(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if s.width() <= width {
        return format!("{s}{}", " ".repeat(width - s.width()));
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    format!("{out}…{}", " ".repeat(width.saturating_sub(used + 1)))
}

fn rule(width: usize) -> String {
    "─".repeat(width)
}

/// "3d late", "today", "tmrw", a weekday within the week, else "Dec 3".
fn relative_due(due: NaiveDate) -> String {
    let days = (due - App::today()).num_days();
    match days {
        d if d < 0 => format!("{}d late", -d),
        0 => "today".into(),
        1 => "tmrw".into(),
        2..=6 => due.format("%a").to_string(),
        _ => due.format("%b %-d").to_string(),
    }
}

fn due_color(due: NaiveDate, done: bool) -> Color {
    let today = App::today();
    match () {
        _ if done => FAINT,
        _ if due < today => LATE,
        _ if due == today => WARN,
        _ => DIM,
    }
}

/// The page on initiative projects asks owners to set a status every week.
const STATUS_STALE_DAYS: i64 = 7;

/// "● on track · 3d", amber when older than a week, "no status" when never set.
fn project_status(app: &App, project: &str) -> Option<Line<'static>> {
    let initiative = app
        .initiatives
        .iter()
        .find(|i| crate::asana::display_name(&i.name) == project)?;
    let Some(update) = &initiative.current_status_update else {
        return Some(Line::from("no status".fg(WARN)));
    };
    let (label, color) = match update.status_type.as_str() {
        "on_track" => ("on track", DONE),
        "at_risk" => ("at risk", WARN),
        "off_track" => ("off track", LATE),
        "on_hold" => ("on hold", DIM),
        "complete" => ("complete", DONE),
        other => (other, DIM),
    };
    let age = (Local::now().date_naive() - update.created_at.with_timezone(&Local).date_naive())
        .num_days();
    let age_color = if age > STATUS_STALE_DAYS { WARN } else { DIM };
    Some(Line::from(vec![
        "● ".fg(color),
        label.to_string().fg(color),
        format!(" · {age}d").fg(age_color),
    ]))
}

/// Initiative tabs group by project, so the column shows who owns the item instead.
fn right_column<'a>(app: &App, task: &'a Task) -> &'a str {
    if app.tab.is_initiative() {
        task.assignee_name()
    } else {
        task.project()
    }
}

fn has_session(app: &App, task: &Task) -> bool {
    rex::session_for(task, &app.sessions).is_some()
}

fn is_done(app: &App, task: &Task) -> bool {
    app.bucket(task) == Bucket::Done
}

/// Splits the body into list and, when the details pane is showing, the pane.
fn split_body(app: &App, area: Rect, gap: u16) -> (Rect, Option<Rect>) {
    if !app.detail_open || app.selected_task().is_none() {
        return (area, None);
    }
    let [list, pane] = if area.width >= SIDE_BY_SIDE_W {
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .spacing(gap)
            .areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)])
            .spacing(gap.min(1))
            .areas(area)
    };
    (list, Some(pane))
}

fn draw_error(f: &mut Frame, err: &str, area: Rect) {
    let msg = vec![
        Line::from("Couldn't load tasks from asana-cli:".fg(LATE)),
        Line::from(err.to_string()),
        Line::from(""),
        Line::from(
            "Export ASANA_ACCESS_TOKEN or store it in Keychain (service asana-access-token), then press r."
                .fg(DIM),
        ),
    ];
    f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: false }), area);
}

fn empty_message(app: &App) -> &'static str {
    if app.loading {
        "Loading…"
    } else {
        "Nothing here."
    }
}

/// Context-dependent key hints, as (key, action) pairs.
fn hints(app: &App) -> Vec<(&'static str, &'static str)> {
    if app.detail_focused() {
        return vec![
            ("j/k", "scroll"),
            ("^d/^u", "page"),
            ("g/G", "top/bottom"),
            ("tab", "list"),
        ];
    }
    match app.tab {
        crate::app::Tab::Sessions => vec![("enter", "focus"), ("r", "refresh"), ("?", "keys")],
        _ if app.detail_open => vec![
            ("tab", "details"),
            ("enter", "launch"),
            ("a", "comment"),
            ("x", "done"),
            ("space", "close"),
            ("?", "keys"),
        ],
        _ => vec![
            ("enter", "launch"),
            ("space", "details"),
            ("a", "comment"),
            ("x", "done"),
            ("o", "open"),
            ("?", "keys"),
        ],
    }
}

fn hint_width(hints: &[(&str, &str)]) -> usize {
    hints
        .iter()
        .map(|(k, a)| k.width() + 1 + a.width() + 3)
        .sum::<usize>()
        .saturating_sub(3)
}

const HELP: [(&str, &str); 16] = [
    ("j / k", "move"),
    ("g / G", "first / last"),
    ("1–7, h / l", "switch view"),
    ("enter", "focus or launch a Claude session"),
    ("space / p", "toggle details"),
    ("tab", "focus details (when open)"),
    ("J / K", "scroll details from the list"),
    ("o", "open in browser"),
    ("x", "mark done / reopen"),
    ("a", "add a comment (^s posts)"),
    ("r", "refresh"),
    ("q / esc", "quit"),
    ("", ""),
    ("in details", ""),
    ("j / k, ^d / ^u", "scroll line / page"),
    ("tab / esc / h", "back to list"),
];

fn draw_help(f: &mut Frame) {
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, a)| match (*k, *a) {
            ("in details", _) => Line::from(k.to_uppercase().bold().fg(Color::Gray)),
            _ => Line::from(vec![format!("{k:<16}").fg(ACCENT), a.to_string().fg(TEXT)]),
        })
        .collect();
    let w = 54.min(f.area().width);
    let h = (lines.len() as u16 + 4).min(f.area().height);
    let area = Rect {
        x: f.area().width.saturating_sub(w) / 2,
        y: f.area().height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(RULE))
        .title(Line::from(" keys ".fg(DIM)))
        .title_bottom(Line::from(" any key to close ".fg(FAINT)).right_aligned())
        .padding(Padding::new(2, 2, 1, 1));
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .style(Style::new().bg(Color::Rgb(26, 27, 38))),
        area,
    );
}

fn selected_style() -> Style {
    Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD)
}

const DUE_W: usize = 9;
const PROJECT_W: usize = 20;
const PROJECT_MIN_LIST_W: usize = 80;
const STATUS_W: usize = 2;

fn draw_screen(f: &mut Frame, app: &App) {
    let [header, top_rule, _, body, bottom_rule, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .horizontal_margin(1)
    .areas(f.area());

    draw_header(f, app, header);
    for area in [top_rule, bottom_rule] {
        f.render_widget(Paragraph::new(rule(area.width as usize)).fg(RULE), area);
    }
    draw_body(f, app, body);
    draw_footer(f, app, footer);
}

/// Tab strip; `compact` shows inactive tabs by their number key so six tabs fit narrow panes.
fn tab_spans(app: &App, compact: bool) -> Vec<Span<'static>> {
    let sep = if compact { " · " } else { "  ·  " };
    let mut spans = vec![" ◆ ".fg(ACCENT), "asana".bold().fg(TEXT), "   ".into()];
    for (i, &t) in Tab::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(sep.fg(FAINT));
        }
        spans.push(match (t == app.tab, compact) {
            (true, _) => t.title().bold().fg(ACCENT),
            (false, true) => (i + 1).to_string().fg(DIM),
            (false, false) => t.title().fg(DIM),
        });
        let n = app.initiative_count(t);
        if matches!(t, Tab::Decisions | Tab::Blockers | Tab::Questions) && n > 0 {
            let color = match t {
                Tab::Blockers => LATE,
                Tab::Decisions => WARN,
                _ => ACCENT,
            };
            spans.push(format!(" {n}").fg(color));
        }
    }
    spans
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut right = vec![];
    if app.loading {
        right.push("⟳ syncing   ".fg(DIM));
    }
    if !app.sessions.is_empty() {
        right.push("● ".fg(WARN));
        right.push(format!("{} sessions ", app.sessions.len()).fg(DIM));
    }
    let right = Line::from(right);
    let full = Line::from(tab_spans(app, false));
    let left = if full.width() + right.width() + 2 <= area.width as usize {
        full
    } else {
        Line::from(tab_spans(app, true))
    };
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(right.right_aligned()), area);
}

/// The comment input replaces the footer while open; long text scrolls to keep the end visible.
fn draw_composer(f: &mut Frame, c: &crate::app::Composer, area: Rect) {
    let width = area.width as usize;
    let hint = if width >= 100 {
        "^s post · enter new line · esc cancel "
    } else {
        "^s post "
    };
    let room = width.saturating_sub(hint.width() + 2);
    let name_w = (width / 4).clamp(8, 24);
    let label = format!(" comment on {}: ", fit(&c.task_name, name_w).trim_end());
    let text_room = room.saturating_sub(label.width() + 1);
    let mut shown: Vec<char> = vec![];
    let mut used = 0;
    for ch in c
        .text
        .chars()
        .rev()
        .map(|ch| if ch == '\n' { '↵' } else { ch })
    {
        used += ch.width().unwrap_or(0);
        if used > text_room {
            break;
        }
        shown.push(ch);
    }
    let shown: String = shown.into_iter().rev().collect();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            label.fg(ACCENT),
            shown.fg(TEXT),
            "█".fg(ACCENT),
        ])),
        area,
    );
    f.render_widget(
        Paragraph::new(Line::from(hint.fg(DIM)).right_aligned()),
        area,
    );
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if let Some(c) = &app.composer {
        return draw_composer(f, c, area);
    }
    let left = if app.status.is_empty() {
        format!(
            " {} · {} done since yesterday",
            Local::now().format("%a, %b %-d"),
            app.done_count()
        )
    } else {
        format!(" {}", app.status)
    };
    let mut hints = hints(app);
    let room = (area.width as usize).saturating_sub(left.width() + 4);
    while !hints.is_empty() && hint_width(&hints) > room {
        hints.pop();
    }
    f.render_widget(Paragraph::new(left).fg(DIM), area);
    let mut spans = vec![];
    for (i, (k, a)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push("   ".into());
        }
        spans.push(k.fg(TEXT));
        spans.push(format!(" {a}").fg(DIM));
    }
    spans.push(" ".into());
    f.render_widget(Paragraph::new(Line::from(spans).right_aligned()), area);
}

fn draw_body(f: &mut Frame, app: &App, area: Rect) {
    if let Some(err) = &app.error {
        return draw_error(f, err, area);
    }
    let (list, pane) = split_body(app, area, 2);
    if let (Some(pane), Some(task)) = (pane, app.selected_task()) {
        detail::draw(f, app, task, pane, pane.x == list.x);
    }
    draw_list(f, app, list);
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let rows = app.rows();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(format!(" {}", empty_message(app))).fg(DIM),
            area,
        );
        return;
    }
    let width = area.width as usize;
    let project_w = if width >= PROJECT_MIN_LIST_W {
        PROJECT_W
    } else {
        0
    };
    let name_w = width.saturating_sub(2 + DUE_W + 2 + project_w + STATUS_W);
    let selected = app.selected_row();
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            ListItem::new(row_line(
                app,
                row,
                selected == Some(i),
                width,
                name_w,
                project_w,
            ))
        })
        .collect();
    let mut state = ListState::default().with_selected(selected);
    f.render_stateful_widget(
        List::new(items).highlight_style(selected_style()),
        area,
        &mut state,
    );
}

fn row_line<'a>(
    app: &'a App,
    row: &'a Row,
    selected: bool,
    width: usize,
    name_w: usize,
    project_w: usize,
) -> Line<'a> {
    let marker = if selected {
        " ›".fg(ACCENT)
    } else {
        "  ".into()
    };
    match row {
        Row::Blank => Line::from(""),
        Row::Header(title, n) => {
            let status = if app.tab.is_initiative() {
                project_status(app, title)
            } else {
                None
            };
            let title = title.to_uppercase();
            let count = format!("  {n}  ");
            let status_w = status.as_ref().map_or(0, |s| s.width() + 1);
            let used = 1 + title.width() + count.width() + status_w;
            let mut spans = vec![
                " ".into(),
                title.bold().fg(Color::Gray),
                count.fg(DIM),
                rule(width.saturating_sub(used)).fg(RULE),
            ];
            if let Some(status) = status {
                spans.push(" ".into());
                spans.extend(status.spans);
            }
            Line::from(spans)
        }
        Row::Task(i) => {
            let t = &app.tasks[*i];
            let done = is_done(app, t);
            let due = match (done, t.due_on) {
                (true, _) => format!(" {}", fit("✓", DUE_W)).fg(DONE),
                (false, Some(d)) => {
                    format!(" {}", fit(&relative_due(d), DUE_W)).fg(due_color(d, false))
                }
                (false, None) => " ".repeat(DUE_W + 1).into(),
            };
            let name = fit(&t.name, name_w);
            let session = if has_session(app, t) {
                " ●".fg(WARN)
            } else {
                "  ".into()
            };
            Line::from(vec![
                marker,
                due,
                " ".into(),
                if done { name.fg(DIM) } else { name.fg(TEXT) },
                " ".into(),
                fit(right_column(app, t), project_w).fg(DIM),
                session,
            ])
        }
        Row::Session(i) => {
            let s = &app.sessions[*i];
            Line::from(vec![
                marker,
                " ● ".fg(WARN),
                fit(s.task_name(), name_w + DUE_W - 2).fg(TEXT),
                "  ".into(),
                s.label.clone().fg(DIM),
            ])
        }
    }
}
