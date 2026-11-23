use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::process::Command;
use std::sync::OnceLock;

const TASK_FIELDS: &str =
    "name,completed,completed_at,due_on,assignee_section.name,projects.name,permalink_url";

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
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
}

impl Task {
    pub fn section(&self) -> &str {
        self.assignee_section
            .as_ref()
            .map_or("No section", |s| s.name.as_str())
    }

    /// Cycle/sprint projects ("Cycle …", "Lota …") are noise next to the real home project.
    pub fn project(&self) -> &str {
        let is_cycle = |p: &&Named| p.name.contains("Cycle ") || p.name.contains("Lota ");
        self.projects
            .iter()
            .find(|p| !is_cycle(p))
            .or(self.projects.first())
            .map_or("", |p| {
                p.name.trim_start_matches(|c: char| !c.is_alphanumeric())
            })
    }
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
    serde_json::from_value(v).map_err(|e| format!("unexpected task shape: {e}"))
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

#[derive(Debug, Clone, Deserialize)]
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
