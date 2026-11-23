use crate::asana::{self, Task};
use crate::buckets::{self, Bucket};
use crate::rex::{self, Session};
use chrono::{Duration, Local, NaiveDate, Utc};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

/// Wait for the cursor to settle before fetching, so holding j doesn't fan out requests.
const DETAIL_DEBOUNCE: StdDuration = StdDuration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Today,
    MyTasks,
    Sessions,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Today, Tab::MyTasks, Tab::Sessions];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Today => "Today",
            Tab::MyTasks => "My Tasks",
            Tab::Sessions => "Sessions",
        }
    }
}

pub enum Row {
    Header(String, usize),
    Blank,
    Task(usize),
    Session(usize),
}

impl Row {
    fn selectable(&self) -> bool {
        matches!(self, Row::Task(_) | Row::Session(_))
    }
}

pub enum Msg {
    Tasks(Result<(String, Vec<Task>), String>),
    Reload,
    Sessions(Vec<Session>),
    Status(String),
    Detail(String, Result<Box<asana::Detail>, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

pub enum DetailState {
    Loading,
    Ready(Box<asana::Detail>),
    Failed(String),
}

pub struct App {
    pub tab: Tab,
    pub tasks: Vec<Task>,
    pub sessions: Vec<Session>,
    pub status: String,
    pub error: Option<String>,
    pub loading: bool,
    pub cursor: usize,
    pub quit: bool,
    pub detail_open: bool,
    pub details: HashMap<String, DetailState>,
    pub detail_scroll: u16,
    /// Written by the renderer, which is the only place that knows the wrapped height.
    pub detail_max_scroll: Cell<u16>,
    pub focus: Focus,
    pub help: bool,
    hovered: Option<(String, Instant)>,
    workspace: Option<String>,
    cwd: String,
    tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let cwd = std::env::var("ASANA_TUI_CWD").unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| format!("{h}/work"))
                .unwrap_or_else(|_| ".".into())
        });
        Self {
            tab: Tab::Today,
            tasks: vec![],
            sessions: vec![],
            status: String::new(),
            error: None,
            loading: false,
            cursor: 0,
            quit: false,
            detail_open: false,
            details: HashMap::new(),
            detail_scroll: 0,
            detail_max_scroll: Cell::new(0),
            focus: Focus::List,
            help: false,
            hovered: None,
            workspace: None,
            cwd,
            tx,
            rx,
        }
    }

    pub fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    pub fn bucket(&self, task: &Task) -> Bucket {
        buckets::classify(task, Self::today())
    }

    pub fn done_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.completed).count()
    }

    pub fn refresh(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.details.clear();
        let tx = self.tx.clone();
        let cached = self.workspace.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Sessions(rex::sessions()));
            let result = cached
                .map(Ok)
                .unwrap_or_else(asana::workspace)
                .and_then(|ws| {
                    let since = Utc::now() - Duration::days(1);
                    asana::my_tasks(&ws, since).map(|t| (ws, t))
                });
            let _ = tx.send(Msg::Tasks(result));
        });
    }

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Tasks(Ok((ws, tasks))) => {
                self.loading = false;
                self.error = None;
                self.workspace = Some(ws);
                self.tasks = tasks;
            }
            Msg::Tasks(Err(e)) => {
                self.loading = false;
                self.error = Some(e);
            }
            Msg::Sessions(s) => self.sessions = s,
            Msg::Status(s) => self.status = s,
            Msg::Reload => self.refresh(),
            Msg::Detail(gid, r) => {
                let state = r.map_or_else(DetailState::Failed, DetailState::Ready);
                self.details.insert(gid, state);
            }
        }
        self.clamp_cursor();
    }

    pub fn rows(&self) -> Vec<Row> {
        self.rows_for(self.tab)
    }

    pub fn rows_for(&self, tab: Tab) -> Vec<Row> {
        match tab {
            Tab::Today => self.today_rows(),
            Tab::MyTasks => self.section_rows(),
            Tab::Sessions => (0..self.sessions.len()).map(Row::Session).collect(),
        }
    }

    fn today_rows(&self) -> Vec<Row> {
        let mut rows = vec![];
        for bucket in Bucket::TODAY_ORDER {
            let idx: Vec<usize> = (0..self.tasks.len())
                .filter(|&i| self.bucket(&self.tasks[i]) == bucket)
                .collect();
            if idx.is_empty() {
                continue;
            }
            rows.push(Row::Header(bucket.title().into(), idx.len()));
            rows.extend(idx.into_iter().map(Row::Task));
            rows.push(Row::Blank);
        }
        rows
    }

    /// Groups open tasks by My Tasks section, keeping Asana's section order.
    fn section_rows(&self) -> Vec<Row> {
        let mut sections: Vec<(&str, Vec<usize>)> = vec![];
        for (i, t) in self.tasks.iter().enumerate().filter(|(_, t)| !t.completed) {
            match sections.iter_mut().find(|(name, _)| *name == t.section()) {
                Some((_, v)) => v.push(i),
                None => sections.push((t.section(), vec![i])),
            }
        }
        let mut rows = vec![];
        for (name, idx) in sections {
            rows.push(Row::Header(name.into(), idx.len()));
            rows.extend(idx.into_iter().map(Row::Task));
            rows.push(Row::Blank);
        }
        rows
    }

    fn selectable(&self) -> Vec<usize> {
        self.rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| r.selectable())
            .map(|(i, _)| i)
            .collect()
    }

    /// Index into `rows()` of the highlighted row.
    pub fn selected_row(&self) -> Option<usize> {
        self.selectable().get(self.cursor).copied()
    }

    fn clamp_cursor(&mut self) {
        let n = self.selectable().len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let n = self.selectable().len() as isize;
        if n == 0 {
            return;
        }
        self.cursor = (self.cursor as isize + delta).clamp(0, n - 1) as usize;
    }

    pub fn jump(&mut self, end: bool) {
        self.cursor = if end {
            self.selectable().len().saturating_sub(1)
        } else {
            0
        };
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.cursor = 0;
        self.focus = Focus::List;
    }

    pub fn cycle_tab(&mut self, forward: bool) {
        let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let n = Tab::ALL.len();
        self.set_tab(
            Tab::ALL[if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            }],
        );
    }

    pub fn selected_task(&self) -> Option<&Task> {
        match self.rows().get(self.selected_row()?)? {
            Row::Task(i) => self.tasks.get(*i),
            _ => None,
        }
    }

    fn selected_session(&self) -> Option<&Session> {
        let rows = self.rows();
        match rows.get(self.selected_row()?)? {
            Row::Session(i) => self.sessions.get(*i),
            Row::Task(i) => rex::session_for(&self.tasks[*i], &self.sessions),
            _ => None,
        }
    }

    pub fn toggle_detail(&mut self) {
        self.detail_open = !self.detail_open;
        self.focus = Focus::List;
    }

    /// Tab: move j/k between the list and the detail pane while it's open.
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::List if self.detail_open && self.selected_task().is_some() => Focus::Detail,
            _ => Focus::List,
        };
    }

    pub fn detail_focused(&self) -> bool {
        self.focus == Focus::Detail && self.detail_open && self.selected_task().is_some()
    }

    pub fn scroll_detail(&mut self, delta: i16) {
        self.detail_scroll = self
            .detail_scroll
            .min(self.detail_max_scroll.get())
            .saturating_add_signed(delta)
            .min(self.detail_max_scroll.get());
    }

    pub fn scroll_detail_to(&mut self, end: bool) {
        self.detail_scroll = if end { self.detail_max_scroll.get() } else { 0 };
    }

    /// Fetches details for the highlighted task once; called every tick while the pane is open.
    pub fn ensure_detail(&mut self) {
        if !self.detail_open {
            return;
        }
        let Some(gid) = self.selected_task().map(|t| t.gid.clone()) else {
            return;
        };
        match &self.hovered {
            Some((g, _)) if *g == gid => {}
            _ => {
                self.hovered = Some((gid, Instant::now()));
                self.detail_scroll = 0;
                return;
            }
        }
        let settled = self
            .hovered
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() >= DETAIL_DEBOUNCE);
        if !settled || self.details.contains_key(&gid) {
            return;
        }
        self.details.insert(gid.clone(), DetailState::Loading);
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = asana::detail(&gid).map(Box::new);
            let _ = tx.send(Msg::Detail(gid, result));
        });
    }

    /// Enter: focus the task's Rex session, or start one running Claude on it.
    pub fn launch_or_focus(&mut self) {
        let tx = self.tx.clone();
        if let Some(s) = self.selected_session().cloned() {
            thread::spawn(move || {
                let msg = rex::focus(&s.session_id)
                    .map_or_else(|e| e, |_| format!("focused {}", s.label));
                let _ = tx.send(Msg::Status(msg));
            });
            return;
        }
        let Some(task) = self.selected_task().cloned() else {
            return;
        };
        let cwd = self.cwd.clone();
        self.status = format!("launching {}…", task.name);
        thread::spawn(move || {
            let msg =
                rex::launch(&task, &cwd).map_or_else(|e| e, |_| format!("launched {}", task.name));
            let _ = tx.send(Msg::Status(msg));
            let _ = tx.send(Msg::Sessions(rex::sessions()));
        });
    }

    pub fn open_in_browser(&mut self) {
        let Some(task) = self.selected_task() else {
            return;
        };
        let _ = std::process::Command::new("open")
            .arg(&task.permalink_url)
            .spawn();
        self.status = format!("opened {}", task.name);
    }

    /// Optimistic toggle; reloads from Asana on failure.
    pub fn toggle_complete(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let Some(Row::Task(i)) = self.rows().into_iter().nth(row) else {
            return;
        };
        let task = &mut self.tasks[i];
        task.completed = !task.completed;
        task.completed_at = task.completed.then(Utc::now);
        let (gid, done, name) = (task.gid.clone(), task.completed, task.name.clone());
        let tx = self.tx.clone();
        thread::spawn(move || {
            let msg = match asana::set_completed(&gid, done) {
                Ok(()) => format!("{} {name}", if done { "✓ completed" } else { "reopened" }),
                Err(e) => {
                    let _ = tx.send(Msg::Reload);
                    format!("failed to update {name}: {e}")
                }
            };
            let _ = tx.send(Msg::Status(msg));
        });
        self.clamp_cursor();
    }
}
