mod app;
mod asana;
mod buckets;
mod rex;
mod ui;

use app::{App, Tab};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::{Duration, Instant};

const AUTO_REFRESH: Duration = Duration::from_secs(120);

/// Keys while the detail pane has focus; returns false to fall through to the list bindings.
fn on_detail_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> bool {
    match code {
        KeyCode::Down | KeyCode::Char('j') => app.scroll_detail(1),
        KeyCode::Up | KeyCode::Char('k') => app.scroll_detail(-1),
        KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => app.scroll_detail(10),
        KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => app.scroll_detail(-10),
        KeyCode::PageDown => app.scroll_detail(10),
        KeyCode::PageUp => app.scroll_detail(-10),
        KeyCode::Char('g') | KeyCode::Home => app.scroll_detail_to(false),
        KeyCode::Char('G') | KeyCode::End => app.scroll_detail_to(true),
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc | KeyCode::Char('h') => app.toggle_focus(),
        _ => return false,
    }
    true
}

/// Keys while the comment input is open; it takes every key.
fn on_composer_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    let Some(c) = app.composer.as_mut() else {
        return;
    };
    match code {
        KeyCode::Char('s') if mods.contains(KeyModifiers::CONTROL) => app.post_comment(),
        KeyCode::Enter => c.text.push('\n'),
        KeyCode::Esc => app.composer = None,
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => app.composer = None,
        KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => c.text.clear(),
        KeyCode::Backspace => {
            c.text.pop();
        }
        KeyCode::Char(ch) => c.text.push(ch),
        _ => {}
    }
}

fn on_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    if app.composer.is_some() {
        return on_composer_key(app, code, mods);
    }
    if app.help {
        app.help = false;
        return;
    }
    if app.detail_focused() && on_detail_key(app, code, mods) {
        return;
    }
    match code {
        KeyCode::Tab if app.detail_open => app.toggle_focus(),
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => app.quit = true,
        KeyCode::Char('1') => app.set_tab(Tab::Today),
        KeyCode::Char('2') => app.set_tab(Tab::MyTasks),
        KeyCode::Char('3') => app.set_tab(Tab::Sessions),
        KeyCode::Char('4') => app.set_tab(Tab::Milestones),
        KeyCode::Char('5') => app.set_tab(Tab::Decisions),
        KeyCode::Char('6') => app.set_tab(Tab::Blockers),
        KeyCode::Char('7') => app.set_tab(Tab::Questions),
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => app.cycle_tab(true),
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => app.cycle_tab(false),
        KeyCode::Down | KeyCode::Char('j') => app.move_cursor(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_cursor(-1),
        KeyCode::Char('g') | KeyCode::Home => app.jump(false),
        KeyCode::Char('G') | KeyCode::End => app.jump(true),
        KeyCode::Enter => app.launch_or_focus(),
        KeyCode::Char('o') => app.open_in_browser(),
        KeyCode::Char('x') => app.toggle_complete(),
        KeyCode::Char('a') => app.start_comment(),
        KeyCode::Char('r') => app.refresh(),
        KeyCode::Char('?') => app.help = true,
        KeyCode::Char(' ') | KeyCode::Char('p') => app.toggle_detail(),
        KeyCode::Char('J') => app.scroll_detail(3),
        KeyCode::Char('K') => app.scroll_detail(-3),
        _ => {}
    }
}

fn main() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new();
    app.refresh();
    let mut last_refresh = Instant::now();

    while !app.quit {
        while let Ok(msg) = app.rx.try_recv() {
            app.handle(msg);
        }
        app.ensure_detail();
        terminal.draw(|f| ui::draw(f, &app))?;

        if event::poll(Duration::from_millis(150))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            on_key(&mut app, key.code, key.modifiers);
        }
        if last_refresh.elapsed() >= AUTO_REFRESH {
            app.refresh();
            last_refresh = Instant::now();
        }
    }

    ratatui::restore();
    Ok(())
}
