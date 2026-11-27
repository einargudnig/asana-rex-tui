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
    Milestones,
    Decisions,
    Blockers,
    Questions,
    Inbox,
    Projects,
}

fn join<T>(h: thread::ScopedJoinHandle<'_, Result<T, String>>) -> Result<T, String> {
    h.join().unwrap_or_else(|_| Err("worker panicked".into()))
}

impl Tab {
    pub const ALL: [Tab; 9] = [
        Tab::Today,
        Tab::MyTasks,
        Tab::Sessions,
        Tab::Milestones,
        Tab::Decisions,
        Tab::Blockers,
        Tab::Questions,
        Tab::Inbox,
        Tab::Projects,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Today => "Today",
            Tab::MyTasks => "My Tasks",
            Tab::Sessions => "Sessions",
            Tab::Milestones => "Milestones",
            Tab::Decisions => "Decisions",
            Tab::Blockers => "Blockers",
            Tab::Questions => "Questions",
            Tab::Inbox => "Inbox",
            Tab::Projects => "Projects",
        }
    }

    pub fn is_initiative(self) -> bool {
        matches!(
            self,
            Tab::Milestones | Tab::Decisions | Tab::Blockers | Tab::Questions
        )
    }

    /// Whether an open initiative task belongs on this tab.
    fn includes(self, task: &Task) -> bool {
        match self {
            Tab::Milestones => task.is_milestone(),
            Tab::Decisions => task.attention() == Some("Decision"),
            Tab::Blockers => task.attention() == Some("Blocker"),
            Tab::Questions => task.attention() == Some("Question"),
            _ => false,
        }
    }
}

pub enum Row {
    Header(String, usize),
    Blank,
    Task(usize),
    Session(usize),
    Inbox(usize),
    Project(usize),
}

impl Row {
    fn selectable(&self) -> bool {
        matches!(
            self,
            Row::Task(_) | Row::Session(_) | Row::Inbox(_) | Row::Project(_)
        )
    }
}

pub struct Loaded {
    workspace: String,
    tasks: Vec<Task>,
    initiatives: Vec<asana::Initiative>,
    projects: Vec<asana::Project>,
}

pub enum Msg {
    Tasks(Result<Loaded, String>),
    Reload,
    Sessions(Vec<Session>),
    Status(String),
    Detail(String, Result<Box<asana::Detail>, String>),
    Inbox(Result<Vec<asana::InboxItem>, String>),
    /// A comment was posted; drop the cached details so the pane refetches.
    Commented(String),
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

pub struct Composer {
    pub gid: String,
    pub task_name: String,
    pub text: String,
}

pub struct App {
    pub tab: Tab,
    pub tasks: Vec<Task>,
    pub sessions: Vec<Session>,
    pub initiatives: Vec<asana::Initiative>,
    /// Projects I own; the Projects tab gives an overview of each, not their tasks.
    pub projects: Vec<asana::Project>,
    pub inbox: Vec<asana::InboxItem>,
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
    /// Comment being typed, and the task it goes on; `Some` while the input is open.
    pub composer: Option<Composer>,
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
            initiatives: vec![],
            projects: vec![],
            inbox: vec![],
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
            composer: None,
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
        self.tasks.iter().filter(|t| t.mine && t.completed).count()
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
                    let (mine, initiative, projects, inbox) = thread::scope(|s| {
                        let initiative = s.spawn(|| asana::initiative_tasks(&ws));
                        let projects = s.spawn(|| asana::my_projects(&ws));
                        let inbox = s.spawn(|| asana::inbox(&ws));
                        let mine = asana::my_tasks(&ws, since);
                        (mine, join(initiative), join(projects), join(inbox))
                    });
                    let _ = tx.send(Msg::Inbox(inbox));
                    let or_empty = |label: &str, e: String| {
                        let _ = tx.send(Msg::Status(format!("{label}: {e}")));
                    };
                    let (initiatives, initiative_tasks) = initiative.unwrap_or_else(|e| {
                        or_empty("initiatives", e);
                        (vec![], vec![])
                    });
                    let projects = projects.unwrap_or_else(|e| {
                        or_empty("projects", e);
                        vec![]
                    });
                    mine.map(|m| Loaded {
                        workspace: ws,
                        tasks: asana::merge(m, initiative_tasks),
                        initiatives,
                        projects,
                    })
                });
            let _ = tx.send(Msg::Tasks(result));
        });
    }

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Tasks(Ok(loaded)) => {
                self.initiatives = loaded.initiatives;
                self.projects = loaded.projects;
                self.loading = false;
                self.error = None;
                self.workspace = Some(loaded.workspace);
                self.tasks = loaded.tasks;
            }
            Msg::Tasks(Err(e)) => {
                self.loading = false;
                self.error = Some(e);
            }
            Msg::Sessions(s) => self.sessions = s,
            Msg::Status(s) => self.status = s,
            Msg::Reload => self.refresh(),
            Msg::Inbox(Ok(items)) => {
                self.inbox = items;
                for item in &mut self.inbox {
                    if let Some(t) = self.tasks.iter().find(|t| t.gid == item.task.gid) {
                        item.task = t.clone();
                    }
                }
            }
            Msg::Inbox(Err(e)) => self.status = format!("inbox: {e}"),
            Msg::Commented(gid) => {
                self.details.remove(&gid);
                self.mark_read(&gid);
            }
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
            Tab::Inbox => (0..self.inbox.len()).map(Row::Inbox).collect(),
            Tab::Projects => self.project_rows(),
            tab => self.initiative_rows(tab),
        }
    }

    fn today_rows(&self) -> Vec<Row> {
        let mut rows = vec![];
        for bucket in Bucket::TODAY_ORDER {
            let idx: Vec<usize> = (0..self.tasks.len())
                .filter(|&i| self.tasks[i].mine && self.bucket(&self.tasks[i]) == bucket)
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
        for (i, t) in self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.mine && !t.completed)
        {
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

    /// Open initiative tasks for `tab`, grouped by project, soonest due first.
    fn initiative_rows(&self, tab: Tab) -> Vec<Row> {
        let mut idx: Vec<usize> = (0..self.tasks.len())
            .filter(|&i| {
                let t = &self.tasks[i];
                t.initiative.is_some() && !t.completed && tab.includes(t)
            })
            .collect();
        idx.sort_by_key(|&i| {
            let t = &self.tasks[i];
            (t.initiative.as_deref(), t.due_on.is_none(), t.due_on)
        });
        let mut rows = vec![];
        for group in idx.chunk_by(|&a, &b| self.tasks[a].initiative == self.tasks[b].initiative) {
            rows.push(Row::Header(
                self.tasks[group[0]].initiative.clone().unwrap_or_default(),
                group.len(),
            ));
            rows.extend(group.iter().copied().map(Row::Task));
            rows.push(Row::Blank);
        }
        rows
    }

    fn project_rows(&self) -> Vec<Row> {
        (0..self.projects.len()).map(Row::Project).collect()
    }

    /// Open items on an initiative tab, for the header badge.
    pub fn initiative_count(&self, tab: Tab) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.initiative.is_some() && !t.completed && tab.includes(t))
            .count()
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
            Row::Inbox(i) => self.inbox.get(*i).map(|item| &item.task),
            _ => None,
        }
    }

    pub fn selected_project(&self) -> Option<&asana::Project> {
        match self.rows().get(self.selected_row()?)? {
            Row::Project(i) => self.projects.get(*i),
            _ => None,
        }
    }

    /// Whether the highlighted row has anything to show in the details pane.
    pub fn has_detail(&self) -> bool {
        self.selected_task().is_some() || self.selected_project().is_some()
    }

    fn selected_session(&self) -> Option<&Session> {
        let rows = self.rows();
        match rows.get(self.selected_row()?)? {
            Row::Session(i) => self.sessions.get(*i),
            Row::Task(i) => rex::session_for(&self.tasks[*i], &self.sessions),
            Row::Inbox(i) => rex::session_for(&self.inbox[*i].task, &self.sessions),
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
            Focus::List if self.detail_open && self.has_detail() => Focus::Detail,
            _ => Focus::List,
        };
    }

    pub fn detail_focused(&self) -> bool {
        self.focus == Focus::Detail && self.detail_open && self.has_detail()
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
    /// Projects arrive with their overview, so they only reset the scroll.
    pub fn ensure_detail(&mut self) {
        if !self.detail_open {
            return;
        }
        let project = self.selected_project().map(|p| p.gid.clone());
        let Some(gid) = self
            .selected_task()
            .map(|t| t.gid.clone())
            .or(project.clone())
        else {
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
        if project.is_some() {
            return;
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
        let Some((url, name)) = self
            .selected_task()
            .map(|t| (&t.permalink_url, &t.name))
            .or(self.selected_project().map(|p| (&p.permalink_url, &p.name)))
        else {
            return;
        };
        let _ = std::process::Command::new("open").arg(url).spawn();
        self.status = format!("opened {name}");
    }

    /// m: mark the selected inbox item read.
    pub fn mark_selected_read(&mut self) {
        if self.tab != Tab::Inbox {
            return;
        }
        if let Some(gid) = self.selected_task().map(|t| t.gid.clone()) {
            self.mark_read(&gid);
        }
    }

    /// Drops the task's inbox item and marks its notifications read in Asana.
    fn mark_read(&mut self, task_gid: &str) {
        let Some(pos) = self.inbox.iter().position(|i| i.task.gid == task_gid) else {
            return;
        };
        let Some(ws) = self.workspace.clone() else {
            return;
        };
        let item = self.inbox.remove(pos);
        self.clamp_cursor();
        let tx = self.tx.clone();
        thread::spawn(move || {
            if let Err(e) = asana::mark_read(&ws, &item.notification_gids) {
                let _ = tx.send(Msg::Status(format!("couldn't mark read: {e}")));
                let _ = tx.send(Msg::Reload);
            }
        });
    }

    /// a: open the comment input for the selected task.
    pub fn start_comment(&mut self) {
        let Some(task) = self.selected_task() else {
            return;
        };
        self.composer = Some(Composer {
            gid: task.gid.clone(),
            task_name: task.name.clone(),
            text: String::new(),
        });
    }

    /// ^s in the comment input: posts in the background; empty input just closes.
    pub fn post_comment(&mut self) {
        let Some(c) = self.composer.take() else {
            return;
        };
        let text = c.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.status = format!("posting comment on {}…", c.task_name);
        let tx = self.tx.clone();
        thread::spawn(move || {
            let msg = match asana::add_comment(&c.gid, &text) {
                Ok(()) => {
                    let _ = tx.send(Msg::Commented(c.gid));
                    format!("commented on {}", c.task_name)
                }
                Err(e) => format!("comment failed on {}: {e}", c.task_name),
            };
            let _ = tx.send(Msg::Status(msg));
        });
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
