//! The details pane: task header, fields, notes, subtasks and comments.
use super::*;
use crate::app::DetailState;
use crate::asana::{Detail, Named};
use std::collections::HashMap;

const LABEL_W: usize = 10;
const COMMENTS_SHOWN: usize = 5;

pub(super) fn draw(f: &mut Frame, app: &App, task: &Task, area: Rect, stacked: bool) {
    let focused = app.detail_focused();
    let borders = if stacked { Borders::TOP } else { Borders::LEFT };
    let block = Block::new()
        .borders(borders)
        .border_style(Style::new().fg(if focused { ACCENT } else { RULE }))
        .padding(Padding::new(2, 2, u16::from(stacked), 0));
    let width = block.inner(area).width as usize;
    let mut lines = vec![Line::from(task.name.clone().bold().fg(TEXT))];
    lines.extend(detail_body(app, task, width));
    let (scroll, max) = draw_scrolled(f, app, lines, block, area);
    draw_scrollbar(f, area, scroll, max, focused);
}

/// The details-pane content for whichever state the fetch is in.
fn detail_body(app: &App, task: &Task, width: usize) -> Vec<Line<'static>> {
    match app.details.get(&task.gid) {
        None | Some(DetailState::Loading) => vec![Line::from("Loading…".fg(DIM))],
        Some(DetailState::Failed(e)) => vec![Line::from(e.clone().fg(LATE))],
        Some(DetailState::Ready(d)) => detail_lines(task, d, width),
    }
}

fn label(name: &str) -> Span<'static> {
    fit(name, LABEL_W).fg(DIM)
}

fn names(items: &[Named]) -> String {
    items
        .iter()
        .map(|n| n.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Greedy word wrap over styled words; continuation lines start under the first word.
fn wrap_words(
    words: impl IntoIterator<Item = (String, Style)>,
    width: usize,
) -> Vec<Vec<Span<'static>>> {
    let width = width.max(10);
    let mut rows = vec![];
    let mut cur: Vec<Span> = vec![];
    let mut used = 0;
    for (word, style) in words {
        let w = word.width();
        if used > 0 && used + 1 + w > width {
            rows.push(std::mem::take(&mut cur));
            used = 0;
        }
        if used > 0 {
            cur.push(" ".into());
            used += 1;
        }
        cur.push(Span::styled(word, style));
        used += w;
    }
    rows.push(cur);
    rows
}

/// A label/value row whose value wraps under itself rather than under the label.
fn labeled(name: &str, value: Vec<(String, Style)>, width: usize) -> Vec<Line<'static>> {
    wrap_words(value, width.saturating_sub(LABEL_W))
        .into_iter()
        .enumerate()
        .map(|(i, spans)| {
            let lead = if i == 0 {
                label(name)
            } else {
                " ".repeat(LABEL_W).into()
            };
            Line::from([vec![lead], spans].concat())
        })
        .collect()
}

fn words(text: &str, style: Style) -> Vec<(String, Style)> {
    text.split_whitespace()
        .map(|w| (w.to_string(), style))
        .collect()
}

/// Section heading matching the list's "TITLE  n  ────" group headers.
fn heading(title: &str, count: String, width: usize) -> Vec<Line<'static>> {
    let title = title.to_uppercase();
    let count = if count.is_empty() {
        "  ".to_string()
    } else {
        format!("  {count}  ")
    };
    let fill = width.saturating_sub(title.width() + count.width());
    vec![
        Line::from(""),
        Line::from(vec![
            title.bold().fg(Color::Gray),
            count.fg(DIM),
            rule(fill).fg(RULE),
        ]),
        Line::from(""),
    ]
}

/// `href` → link text for every anchor in Asana's HTML, so plain-text URLs can be named.
fn anchor_texts(html: &str, out: &mut HashMap<String, String>) {
    for chunk in html.split("<a ").skip(1) {
        let Some(href) = chunk
            .split_once("href=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(h, _)| unescape(h))
        else {
            continue;
        };
        let Some(text) = chunk
            .split_once('>')
            .and_then(|(_, rest)| rest.split_once("</a>"))
            .map(|(t, _)| unescape(t))
        else {
            continue;
        };
        out.entry(href).or_insert(text);
    }
}

fn unescape(s: &str) -> String {
    s.replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Asana writes mentions, attachments and task links as long URLs; show what they point at.
fn tidy_word(word: &str, links: &HashMap<String, String>) -> (String, Style) {
    let Some(at) = word.find("http") else {
        return (word.to_string(), Style::new().fg(TEXT));
    };
    let (pre, rest) = word.split_at(at);
    let url = rest.trim_end_matches([',', '.', ')', ']', ';', ':']);
    let post = &rest[url.len()..];
    let is_profile = url.contains("/profile/");
    let label = match links.get(url) {
        Some(text) if is_profile => format!("@{}", text.trim_start_matches('@')),
        Some(text) => format!("↗ {}", fit(text, 40).trim_end()),
        None if is_profile => "@someone".into(),
        None if url.contains("get_asset") || url.contains("/attachments/") => "▤ attachment".into(),
        None if url.contains("app.asana.com") => "↗ asana".into(),
        None => {
            let bare = url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_start_matches("www.")
                .trim_end_matches('/');
            format!("↗ {}", fit(bare, 36).trim_end())
        }
    };
    (format!("{pre}{label}{post}"), Style::new().fg(ACCENT))
}

/// Free text (notes, comments) with links tidied and wrapping under `indent`.
fn rich_text(
    text: &str,
    width: usize,
    indent: usize,
    links: &HashMap<String, String>,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = vec![];
    for line in text.trim().lines() {
        if line.trim().is_empty() {
            if out.last().is_some_and(|l| l.width() > 0) {
                out.push(Line::from(""));
            }
            continue;
        }
        let lead = indent + line.width() - line.trim_start().width();
        let words = line.split_whitespace().map(|w| tidy_word(w, links));
        for spans in wrap_words(words, width.saturating_sub(lead)) {
            out.push(Line::from([vec![" ".repeat(lead).into()], spans].concat()));
        }
    }
    out
}

fn breadcrumb(task: &Task, d: &Detail) -> Vec<Line<'static>> {
    let crumbs: Vec<Line> = d
        .memberships
        .iter()
        .filter_map(|m| {
            let project = m.project.as_ref()?.name.clone();
            let mut spans = vec![project.fg(DIM)];
            if let Some(s) = m.section.as_ref().filter(|s| !s.name.is_empty()) {
                spans.push("  ›  ".fg(FAINT));
                spans.push(s.name.clone().fg(DIM));
            }
            Some(Line::from(spans))
        })
        .collect();
    match (crumbs.is_empty(), task.project()) {
        (false, _) => crumbs,
        (true, "") => vec![Line::from("No project".fg(FAINT))],
        (true, p) => vec![Line::from(p.to_string().fg(DIM))],
    }
}

fn facts(task: &Task, d: &Detail, width: usize) -> Vec<Line<'static>> {
    let text = Style::new().fg(TEXT);
    let dim = Style::new().fg(DIM);
    let mut lines = vec![];
    if task.completed {
        let when = task.completed_at.map_or(String::new(), |t| {
            format!("{}", t.with_timezone(&Local).format("%b %-d %H:%M"))
        });
        let mut v = vec![("✓ done".to_string(), Style::new().fg(DONE))];
        v.extend(words(&when, dim));
        lines.extend(labeled("status", v, width));
    }
    if let Some(due) = task.due_on {
        let mut v = words(&due.format("%a, %b %-d").to_string(), text);
        if !task.completed {
            v.push(("·".into(), Style::new().fg(FAINT)));
            v.extend(words(
                &relative_due(due),
                Style::new().fg(due_color(due, false)),
            ));
        }
        lines.extend(labeled("due", v, width));
    }
    if let Some(by) = &d.created_by {
        let mut v = words(&by.name, text);
        if let Some(t) = d.created_at {
            v.push(("·".into(), Style::new().fg(FAINT)));
            v.extend(words(
                &t.with_timezone(&Local).format("%b %-d").to_string(),
                dim,
            ));
        }
        lines.extend(labeled("created", v, width));
    }
    if !d.tags.is_empty() {
        lines.extend(labeled("tags", words(&names(&d.tags), text), width));
    }
    for cf in &d.custom_fields {
        let Some(value) = cf.display_value.as_deref().filter(|v| !v.is_empty()) else {
            continue;
        };
        lines.extend(labeled(&cf.name.to_lowercase(), words(value, text), width));
    }
    if !d.followers.is_empty() {
        lines.extend(labeled(
            "followers",
            words(&names(&d.followers), dim),
            width,
        ));
    }
    lines
}

fn detail_lines(task: &Task, d: &Detail, width: usize) -> Vec<Line<'static>> {
    let mut links = HashMap::new();
    for c in &d.comments {
        anchor_texts(&c.html_text, &mut links);
    }

    let mut lines = breadcrumb(task, d);
    lines.push(Line::from(""));
    lines.extend(facts(task, d, width));

    if !d.notes.trim().is_empty() {
        lines.extend(heading("Notes", String::new(), width));
        lines.extend(rich_text(&d.notes, width, 0, &links));
    }

    if !d.subtasks.is_empty() {
        let done = d.subtasks.iter().filter(|s| s.completed).count();
        let count = format!("{done}/{}", d.subtasks.len());
        lines.extend(heading("Subtasks", count, width));
        for s in &d.subtasks {
            let (mark, color, style) = if s.completed {
                ("✓ ", DONE, Style::new().fg(DIM))
            } else {
                ("○ ", DIM, Style::new().fg(TEXT))
            };
            let words = words(&s.name, style);
            for (i, spans) in wrap_words(words, width.saturating_sub(2))
                .into_iter()
                .enumerate()
            {
                let lead = if i == 0 { mark.fg(color) } else { "  ".into() };
                lines.push(Line::from([vec![lead], spans].concat()));
            }
        }
    }

    if !d.comments.is_empty() {
        let n = d.comments.len();
        let count = if n > COMMENTS_SHOWN {
            format!("latest {COMMENTS_SHOWN} of {n}")
        } else {
            n.to_string()
        };
        lines.extend(heading("Comments", count, width));
        for (i, c) in d
            .comments
            .iter()
            .skip(n.saturating_sub(COMMENTS_SHOWN))
            .enumerate()
        {
            if i > 0 {
                lines.push(Line::from(""));
            }
            let who = c.created_by.as_ref().map_or("someone", |n| n.name.as_str());
            let when = c
                .created_at
                .with_timezone(&Local)
                .format("%b %-d, %H:%M")
                .to_string();
            let pad = width.saturating_sub(who.width() + when.width() + 2);
            lines.push(Line::from(vec![
                "● ".fg(ACCENT),
                who.to_string().bold().fg(TEXT),
                " ".repeat(pad).into(),
                when.fg(FAINT),
            ]));
            lines.extend(rich_text(&c.text, width, 2, &links));
        }
    }
    lines
}

/// Renders scrollable details and records how far they can scroll; returns (scroll, max).
fn draw_scrolled(
    f: &mut Frame,
    app: &App,
    lines: Vec<Line<'static>>,
    block: Block,
    area: Rect,
) -> (u16, u16) {
    let inner_w = block.inner(area).width;
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    let max = para
        .line_count(inner_w)
        .saturating_sub(area.height as usize);
    let max = u16::try_from(max).unwrap_or(u16::MAX);
    app.detail_max_scroll.set(max);
    let scroll = app.detail_scroll.min(max);
    f.render_widget(para.scroll((scroll, 0)), area);
    (scroll, max)
}

fn draw_scrollbar(f: &mut Frame, area: Rect, scroll: u16, max: u16, focused: bool) {
    if max == 0 {
        return;
    }
    let mut state = ScrollbarState::new(max as usize).position(scroll as usize);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(Style::new().fg(RULE))
        .thumb_symbol("┃")
        .thumb_style(Style::new().fg(if focused { ACCENT } else { DIM }));
    f.render_stateful_widget(bar, area, &mut state);
}

const BAR_W: usize = 20;

/// "████████░░░░  40%" from done/total.
fn bar(done: usize, total: usize) -> Vec<Span<'static>> {
    let filled = (done * BAR_W).checked_div(total).unwrap_or(0);
    let pct = (done * 100).checked_div(total).unwrap_or(0);
    vec![
        "█".repeat(filled).fg(DONE),
        "░".repeat(BAR_W - filled).fg(FAINT),
        format!("  {pct}%").fg(DIM),
    ]
}

fn project_facts(p: &Project, width: usize) -> Vec<Line<'static>> {
    let text = Style::new().fg(TEXT);
    let dim = Style::new().fg(DIM);
    let mut lines = vec![];
    if let Some(c) = &p.counts {
        let open = c.num_tasks - c.num_completed_tasks;
        let mut v = words(&format!("{open} open"), text);
        v.push(("·".into(), Style::new().fg(FAINT)));
        v.extend(words(&format!("{} done", c.num_completed_tasks), dim));
        if c.num_incomplete_milestones > 0 {
            v.push(("·".into(), Style::new().fg(FAINT)));
            v.extend(words(
                &format!("{} milestones to go", c.num_incomplete_milestones),
                dim,
            ));
        }
        lines.extend(labeled("tasks", v, width));
        if c.num_tasks > 0 {
            lines.push(Line::from(
                [
                    vec![" ".repeat(LABEL_W).into()],
                    bar(c.num_completed_tasks, c.num_tasks),
                ]
                .concat(),
            ));
        }
    }
    if let Some(start) = p.start_on {
        lines.extend(labeled(
            "start",
            words(&start.format("%a, %b %-d").to_string(), text),
            width,
        ));
    }
    match p.due_on {
        Some(due) => {
            let mut v = words(&due.format("%a, %b %-d").to_string(), text);
            v.push(("·".into(), Style::new().fg(FAINT)));
            v.extend(words(
                &relative_due(due),
                Style::new().fg(due_color(due, false)),
            ));
            lines.extend(labeled("due", v, width));
        }
        None => lines.extend(labeled("due", words("not set", dim), width)),
    }
    if !p.members.is_empty() {
        lines.extend(labeled("members", words(&names(&p.members), dim), width));
    }
    lines
}

/// The latest status update: verdict, title, who and when, then its text.
fn project_status_lines(p: &Project, width: usize) -> Vec<Line<'static>> {
    let Some(u) = &p.current_status_update else {
        return vec![Line::from(
            "No status update yet. Post one in Asana so people know where this stands.".fg(WARN),
        )];
    };
    let (label, color) = status_label(u);
    let age = status_age_days(u);
    let mut head = vec!["● ".fg(color), label.bold().fg(color)];
    if !u.title.trim().is_empty() {
        head.push("  ".into());
        head.push(u.title.trim().to_string().fg(TEXT));
    }
    let who = u.author.as_ref().map_or("someone", |a| a.name.as_str());
    let when = u
        .created_at
        .with_timezone(&Local)
        .format("%b %-d")
        .to_string();
    let age_color = if age > STATUS_STALE_DAYS { WARN } else { FAINT };
    let mut lines = vec![
        Line::from(head),
        Line::from(vec![
            format!("{who} · {when} · ").fg(FAINT),
            format!("{age}d ago").fg(age_color),
        ]),
    ];
    if !u.text.trim().is_empty() {
        lines.push(Line::from(""));
        lines.extend(rich_text(&u.text, width, 0, &HashMap::new()));
    }
    lines
}

pub(super) fn draw_project(f: &mut Frame, app: &App, p: &Project, area: Rect, stacked: bool) {
    let focused = app.detail_focused();
    let borders = if stacked { Borders::TOP } else { Borders::LEFT };
    let block = Block::new()
        .borders(borders)
        .border_style(Style::new().fg(if focused { ACCENT } else { RULE }))
        .padding(Padding::new(2, 2, u16::from(stacked), 0));
    let width = block.inner(area).width as usize;
    let mut lines = vec![Line::from(
        crate::asana::display_name(&p.name)
            .to_string()
            .bold()
            .fg(TEXT),
    )];
    if let Some(team) = &p.team {
        lines.push(Line::from(team.name.clone().fg(DIM)));
    }
    lines.push(Line::from(""));
    lines.extend(project_facts(p, width));
    lines.extend(heading("Status", String::new(), width));
    lines.extend(project_status_lines(p, width));
    if !p.notes.trim().is_empty() {
        lines.extend(heading("Overview", String::new(), width));
        lines.extend(rich_text(&p.notes, width, 0, &HashMap::new()));
    }
    let (scroll, max) = draw_scrolled(f, app, lines, block, area);
    draw_scrollbar(f, area, scroll, max, focused);
}
