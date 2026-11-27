use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::process::Command;
use std::sync::OnceLock;

const TASK_FIELDS: &str = "name,completed,completed_at,due_on,assignee_section.name,projects.name,\
permalink_url,resource_subtype,assignee.name,custom_fields.name,custom_fields.display_value";

/// Initiative projects carry this enum field (Blocker, Decision, Risk, Question)
/// on the tasks in their "Needs attention" section.
const ATTENTION_FIELD: &str = "Attention type";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Named {
    #[serde(default)]
    pub gid: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Task {
    pub gid: String,
    pub name: String,
    #[serde(default)]
    pub completed: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub due_on: Option<NaiveDate>,
    pub assignee_section: Option<Named>,
    #[serde(default)]
    pub projects: Vec<Named>,
    #[serde(default)]
    pub permalink_url: String,
    #[serde(default)]
    pub resource_subtype: String,
    pub assignee: Option<Named>,
    #[serde(default)]
    pub custom_fields: Vec<CustomField>,
    /// Assigned to me; only these show in Today and My Tasks.
    #[serde(skip)]
    pub mine: bool,
    /// Name of the initiative project it lives in; only these show in the initiative tabs.
    #[serde(skip)]
    pub initiative: Option<String>,
}

impl Task {
    pub fn section(&self) -> &str {
        self.assignee_section
            .as_ref()
            .map_or("No section", |s| s.name.as_str())
    }

    pub fn is_milestone(&self) -> bool {
        self.resource_subtype == "milestone"
    }

    pub fn attention(&self) -> Option<&str> {
        self.custom_fields
            .iter()
            .find(|f| f.name == ATTENTION_FIELD)?
            .display_value
            .as_deref()
    }

    pub fn assignee_name(&self) -> &str {
        self.assignee.as_ref().map_or("", |a| a.name.as_str())
    }

    /// Cycle/sprint projects ("Cycle …", "Lota …") are noise next to the real home project.
    pub fn project(&self) -> &str {
        let is_cycle = |p: &&Named| p.name.contains("Cycle ") || p.name.contains("Lota ");
        self.projects
            .iter()
            .find(|p| !is_cycle(p))
            .or(self.projects.first())
            .map_or("", |p| display_name(&p.name))
    }
}

/// Drops leading emoji and punctuation from a project name.
pub fn display_name(name: &str) -> &str {
    name.trim_start_matches(|c: char| !c.is_alphanumeric())
}

const KEYCHAIN_SERVICE: &str = "asana-access-token";

/// `ASANA_ACCESS_TOKEN` from the env, else from the macOS Keychain, so the app
/// works from Rex bindings and shells that never sourced the export.
fn token() -> Option<&'static str> {
    static TOKEN: OnceLock<Option<String>> = OnceLock::new();
    TOKEN
        .get_or_init(|| {
            std::env::var("ASANA_ACCESS_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
                .or_else(|| {
                    let user = std::env::var("USER").unwrap_or_default();
                    let out = Command::new("security")
                        .args([
                            "find-generic-password",
                            "-a",
                            &user,
                            "-s",
                            KEYCHAIN_SERVICE,
                            "-w",
                        ])
                        .output()
                        .ok()?;
                    let t = String::from_utf8(out.stdout).ok()?.trim().to_string();
                    (out.status.success() && !t.is_empty()).then_some(t)
                })
        })
        .as_deref()
}

fn run(args: &[&str]) -> Result<Value, String> {
    let mut cmd = Command::new("asana-cli");
    if let Some(t) = token() {
        cmd.env("ASANA_ACCESS_TOKEN", t);
    }
    let out = cmd
        .args(args)
        .args(["--output", "json"])
        .output()
        .map_err(|e| format!("asana-cli: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed");
        return Err(msg.trim().trim_start_matches('›').trim().to_string());
    }
    parse_output(&out.stdout)
}

/// Accepts a JSON document (bare or wrapped in `data`) or newline-delimited JSON.
fn parse_output(bytes: &[u8]) -> Result<Value, String> {
    if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
        return Ok(match v {
            Value::Object(mut m) if m.contains_key("data") => {
                m.remove("data").unwrap_or(Value::Null)
            }
            v => v,
        });
    }
    let rows: Result<Vec<Value>, _> = String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect();
    rows.map(Value::Array)
        .map_err(|e| format!("bad asana-cli output: {e}"))
}

pub fn workspace() -> Result<String, String> {
    if let Ok(ws) = std::env::var("ASANA_WORKSPACE") {
        return Ok(ws);
    }
    let me = run(&[
        "users",
        "get",
        "--user",
        "me",
        "--opt-fields",
        "workspaces.name",
    ])?;
    me["workspaces"][0]["gid"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "no workspace found; set ASANA_WORKSPACE".into())
}

/// Open tasks plus anything completed since `since`, in My Tasks order.
pub fn my_tasks(workspace: &str, since: DateTime<Utc>) -> Result<Vec<Task>, String> {
    let since = since.to_rfc3339();
    let v = run(&[
        "tasks",
        "list",
        "--assignee",
        "me",
        "--workspace",
        workspace,
        "--completed-since",
        &since,
        "--opt-fields",
        TASK_FIELDS,
    ])?;
    let mut tasks: Vec<Task> =
        serde_json::from_value(v).map_err(|e| format!("unexpected task shape: {e}"))?;
    for t in &mut tasks {
        t.mine = true;
    }
    Ok(tasks)
}

/// Appends `more` to `tasks`, folding duplicates into the existing task's flags.
pub fn merge(mut tasks: Vec<Task>, more: Vec<Task>) -> Vec<Task> {
    for t in more {
        match tasks.iter_mut().find(|m| m.gid == t.gid) {
            Some(m) => m.initiative = m.initiative.take().or(t.initiative),
            None => tasks.push(t),
        }
    }
    tasks
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusUpdate {
    /// on_track, at_risk, off_track, on_hold or complete.
    pub status_type: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    pub author: Option<Named>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Initiative {
    pub gid: String,
    pub name: String,
    pub current_status_update: Option<StatusUpdate>,
}

/// Open projects that use the attention field.
fn initiative_projects(workspace: &str) -> Result<Vec<Initiative>, String> {
    let v = run(&[
        "projects",
        "search",
        "--workspace",
        workspace,
        "--no-completed",
        "--limit",
        "100",
        "--opt-fields",
        "name,custom_field_settings.custom_field.name,\
current_status_update.status_type,current_status_update.created_at",
    ])?;
    let uses_field = |p: &&Value| {
        p["custom_field_settings"].as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["custom_field"]["name"] == ATTENTION_FIELD)
        })
    };
    v.as_array()
        .into_iter()
        .flatten()
        .filter(uses_field)
        .map(|p| decode(p.clone()))
        .collect()
}

/// Open milestones and attention items across initiative projects. asana-cli can't
/// filter search by custom field, so attention items are picked out here; each
/// search returns at most 100 tasks.
pub fn initiative_tasks(workspace: &str) -> Result<(Vec<Initiative>, Vec<Task>), String> {
    let projects = initiative_projects(workspace)?;
    if projects.is_empty() {
        return Ok((projects, vec![]));
    }
    let any = projects
        .iter()
        .map(|p| p.gid.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let search = |subtype: &str| {
        run(&[
            "tasks",
            "search",
            "--workspace",
            workspace,
            "--projects.any",
            &any,
            "--resource-subtype",
            subtype,
            "--no-completed",
            "--limit",
            "100",
            "--opt-fields",
            TASK_FIELDS,
        ])
        .and_then(decode::<Vec<Task>>)
    };
    let (milestones, rest) = std::thread::scope(|s| {
        let m = s.spawn(|| search("milestone"));
        let rest = search("default_task");
        (
            m.join().unwrap_or_else(|_| Err("worker panicked".into())),
            rest,
        )
    });
    let mut tasks = milestones?;
    tasks.extend(rest?.into_iter().filter(|t| t.attention().is_some()));
    for t in &mut tasks {
        t.initiative = t
            .projects
            .iter()
            .find_map(|tp| projects.iter().find(|p| p.gid == tp.gid))
            .map(|p| display_name(&p.name).to_string());
    }
    Ok((projects, tasks))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaskCounts {
    #[serde(default)]
    pub num_tasks: usize,
    #[serde(default)]
    pub num_completed_tasks: usize,
    #[serde(default)]
    pub num_incomplete_milestones: usize,
}

/// A project I own, with what the Projects tab needs to give an overview of it.
#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub gid: String,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    pub due_on: Option<NaiveDate>,
    pub start_on: Option<NaiveDate>,
    #[serde(default)]
    pub permalink_url: String,
    pub team: Option<Named>,
    #[serde(default)]
    pub members: Vec<Named>,
    pub current_status_update: Option<StatusUpdate>,
    #[serde(default)]
    archived: bool,
    /// Fetched separately; `None` when that call failed.
    #[serde(skip)]
    pub counts: Option<TaskCounts>,
}

const PROJECT_FIELDS: &str = "name,archived,notes,due_on,start_on,permalink_url,team.name,\
members.name,current_status_update.status_type,current_status_update.created_at,\
current_status_update.title,current_status_update.text,current_status_update.author.name";

fn task_counts(project: &str) -> Result<TaskCounts, String> {
    run(&[
        "projects",
        "show",
        "--project",
        project,
        "--task-counts",
        "--opt-fields",
        "num_tasks,num_completed_tasks,num_incomplete_milestones",
    ])
    .and_then(decode)
}

/// Off track first, then at risk, on track, on hold; no status at all goes last.
fn urgency(p: &Project) -> u8 {
    match p
        .current_status_update
        .as_ref()
        .map(|u| u.status_type.as_str())
    {
        Some("off_track") => 0,
        Some("at_risk") => 1,
        Some("on_track") => 2,
        Some("on_hold") => 3,
        Some(_) => 4,
        None => 5,
    }
}

/// Open, unarchived projects I own, most urgent first, then soonest due, then by name.
pub fn my_projects(workspace: &str) -> Result<Vec<Project>, String> {
    let v = run(&[
        "projects",
        "search",
        "--workspace",
        workspace,
        "--owner.any",
        "me",
        "--no-completed",
        "--limit",
        "100",
        "--opt-fields",
        PROJECT_FIELDS,
    ])?;
    let mut projects: Vec<Project> = decode::<Vec<Project>>(v)?
        .into_iter()
        .filter(|p| !p.archived)
        .collect();
    projects.sort_by_key(|p| {
        (
            urgency(p),
            p.due_on.is_none(),
            p.due_on,
            display_name(&p.name).to_lowercase(),
        )
    });
    let counts: Vec<Option<TaskCounts>> = std::thread::scope(|s| {
        let handles: Vec<_> = projects
            .iter()
            .map(|p| s.spawn(|| task_counts(&p.gid).ok()))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().ok().flatten())
            .collect()
    });
    for (p, c) in projects.iter_mut().zip(counts) {
        p.counts = c;
    }
    Ok(projects)
}

pub fn add_comment(gid: &str, text: &str) -> Result<(), String> {
    run(&["stories", "create", "--task", gid, "--text", text]).map(|_| ())
}

/// Notification kinds the Inbox shows; the rest (likes, due date moves, …) is noise.
const INBOX_KINDS: [&str; 2] = ["comment_added", "assigned"];

#[derive(Debug, Clone, Deserialize)]
struct Notification {
    gid: String,
    created_at: DateTime<Utc>,
    created_by: Option<Named>,
    #[serde(default)]
    is_read: bool,
    #[serde(default)]
    story_resource_subtype: String,
    #[serde(default)]
    work_objects: Vec<WorkObject>,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkObject {
    gid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    resource_type: String,
}

/// One task's unread comments and assignments, newest first.
#[derive(Debug, Clone)]
pub struct InboxItem {
    pub task: Task,
    pub notification_gids: Vec<String>,
    pub actor: String,
    /// The newest notification's kind: "comment_added" or "assigned".
    pub kind: String,
    pub at: DateTime<Utc>,
}

/// Unread comments and assignments, grouped by task, newest first.
pub fn inbox(workspace: &str) -> Result<Vec<InboxItem>, String> {
    let v = run(&[
        "notifications",
        "list",
        "--user",
        "me",
        "--workspace",
        workspace,
        "--max-results",
        "200",
    ])?;
    let mut notes: Vec<Notification> = decode(v)?;
    notes.sort_by_key(|n| std::cmp::Reverse(n.created_at));
    let mut items: Vec<InboxItem> = vec![];
    for n in notes {
        if n.is_read || !INBOX_KINDS.contains(&n.story_resource_subtype.as_str()) {
            continue;
        }
        let Some(obj) = n.work_objects.iter().find(|o| o.resource_type == "task") else {
            continue;
        };
        if let Some(item) = items.iter_mut().find(|i| i.task.gid == obj.gid) {
            item.notification_gids.push(n.gid);
            continue;
        }
        items.push(InboxItem {
            task: Task {
                gid: obj.gid.clone(),
                name: obj.name.clone(),
                permalink_url: format!("https://app.asana.com/0/0/{}/f", obj.gid),
                ..Task::default()
            },
            notification_gids: vec![n.gid],
            actor: n.created_by.map(|u| u.name).unwrap_or_default(),
            kind: n.story_resource_subtype,
            at: n.created_at,
        });
    }
    Ok(items)
}

pub fn mark_read(workspace: &str, gids: &[String]) -> Result<(), String> {
    let mut args = vec![
        "notifications",
        "update",
        "--user",
        "me",
        "--workspace",
        workspace,
        "--read",
    ];
    for g in gids.iter().take(50) {
        args.extend(["--notification-gids", g.as_str()]);
    }
    run(&args).map(|_| ())
}

pub fn set_completed(gid: &str, done: bool) -> Result<(), String> {
    let flag = if done {
        "--completed"
    } else {
        "--no-completed"
    };
    run(&["tasks", "update", "--task", gid, flag]).map(|_| ())
}

#[derive(Debug, Clone, Deserialize)]
pub struct Membership {
    pub project: Option<Named>,
    pub section: Option<Named>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CustomField {
    pub name: String,
    pub display_value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Subtask {
    pub name: String,
    #[serde(default)]
    pub completed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Story {
    pub created_by: Option<Named>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub resource_subtype: String,
    #[serde(default)]
    pub text: String,
    /// Same comment as HTML; its anchors carry the names behind mention and task URLs.
    #[serde(default)]
    pub html_text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Detail {
    #[serde(default)]
    pub notes: String,
    pub created_by: Option<Named>,
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub memberships: Vec<Membership>,
    #[serde(default)]
    pub followers: Vec<Named>,
    #[serde(default)]
    pub tags: Vec<Named>,
    #[serde(default)]
    pub custom_fields: Vec<CustomField>,
    #[serde(skip)]
    pub subtasks: Vec<Subtask>,
    /// Comments only, oldest first; system activity is dropped.
    #[serde(skip)]
    pub comments: Vec<Story>,
}

const DETAIL_FIELDS: &str = "notes,created_by.name,created_at,memberships.project.name,\
memberships.section.name,followers.name,tags.name,custom_fields.name,custom_fields.display_value";

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| format!("unexpected shape: {e}"))
}

/// Task, subtasks and comments fetched concurrently (three asana-cli calls).
pub fn detail(gid: &str) -> Result<Detail, String> {
    std::thread::scope(|s| {
        let task = s.spawn(|| run(&["tasks", "get", "--task", gid, "--opt-fields", DETAIL_FIELDS]));
        let subs = s.spawn(|| {
            run(&[
                "tasks",
                "show",
                "--task",
                gid,
                "--subtasks",
                "--opt-fields",
                "name,completed",
            ])
        });
        let stories = s.spawn(|| {
            run(&[
                "stories",
                "list",
                "--task",
                gid,
                "--opt-fields",
                "created_by.name,created_at,resource_subtype,text,html_text",
            ])
        });
        let join = |h: std::thread::ScopedJoinHandle<'_, Result<Value, String>>| {
            h.join().unwrap_or_else(|_| Err("worker panicked".into()))
        };
        let mut detail: Detail = decode(join(task)?)?;
        detail.subtasks = join(subs).and_then(decode).unwrap_or_default();
        detail.comments = join(stories)
            .and_then(decode::<Vec<Story>>)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.resource_subtype == "comment_added")
            .collect();
        Ok(detail)
    })
}
