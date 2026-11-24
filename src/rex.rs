use crate::asana::Task;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

pub const PREFIX: &str = "asana-";

/// The task a session was launched for. Kept outside Rex because the label
/// isn't ours to keep: Claude renames its session on the first request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub gid: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    pub session_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(skip)]
    pub link: Option<Link>,
}

impl Session {
    /// Only labels shaped `asana-<6 digits>-…` are ours; `asana-tui` is not.
    fn has_task_label(&self) -> bool {
        self.label.strip_prefix(PREFIX).is_some_and(|rest| {
            let b = rest.as_bytes();
            b.len() > 7 && b[..6].iter().all(u8::is_ascii_digit) && b[6] == b'-'
        })
    }

    pub fn task_name(&self) -> &str {
        if let Some(link) = &self.link {
            return &link.name;
        }
        self.label
            .strip_prefix(PREFIX)
            .and_then(|s| s.split_once('-'))
            .map_or(&self.label, |(_, n)| n)
    }
}

type Registry = HashMap<String, Link>;

fn registry_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("asana-tui/sessions.json"))
}

fn load_registry() -> Registry {
    registry_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_registry(reg: &Registry) {
    let Some(path) = registry_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(reg) {
        let _ = std::fs::write(path, json);
    }
}

fn rex(args: &[&str]) -> Result<String, String> {
    let out = Command::new("rex")
        .args(args)
        .output()
        .map_err(|e| format!("rex: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Stable key per task: the last 6 digits of the gid survive renames.
fn key(task: &Task) -> String {
    let gid = &task.gid;
    format!("{PREFIX}{}-", &gid[gid.len().saturating_sub(6)..])
}

fn slug(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(4)
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

pub fn sessions() -> Vec<Session> {
    #[derive(Deserialize)]
    struct Ls {
        sessions: Vec<Session>,
    }
    let Some(ls) = rex(&["ls", "--json"])
        .ok()
        .and_then(|s| serde_json::from_str::<Ls>(&s).ok())
    else {
        return vec![];
    };
    let mut reg = load_registry();
    let before = reg.len();
    reg.retain(|id, _| ls.sessions.iter().any(|s| &s.session_id == id));
    if reg.len() != before {
        save_registry(&reg);
    }
    ls.sessions
        .into_iter()
        .filter_map(|mut s| {
            s.link = reg.get(&s.session_id).cloned();
            (s.link.is_some() || s.has_task_label()).then_some(s)
        })
        .collect()
}

pub fn session_for<'a>(task: &Task, sessions: &'a [Session]) -> Option<&'a Session> {
    let key = key(task);
    sessions.iter().find(|s| match &s.link {
        Some(link) => link.gid == task.gid,
        None => s.label.starts_with(&key),
    })
}
fn app_client() -> Option<String> {
    let json: serde_json::Value =
        serde_json::from_str(&rex(&["client", "ls", "--json"]).ok()?).ok()?;
    json["clients"]
        .as_array()?
        .iter()
        .find(|c| c["info"]["kind"] == "app" && c["connection_state"] == "connected")
        .and_then(|c| c["client_id"].as_str())
        .map(str::to_string)
}

pub fn focus(session_id: &str) -> Result<(), String> {
    let client = app_client().ok_or("no connected Rex app")?;
    rex(&[
        "-C",
        &client,
        "do",
        "session.select",
        &format!("session_id={session_id}"),
    ])
    .map(|_| ())
}

fn prompt(task: &Task) -> String {
    format!(
        "Asana task {gid}: {name}\n{url}\n\n\
         Read the full task (description, comments, subtasks) with the Asana MCP, \
         summarize what's needed, then propose a plan before changing anything.",
        gid = task.gid,
        name = task.name,
        url = task.permalink_url,
    )
}

pub fn launch(task: &Task, cwd: &str) -> Result<String, String> {
    let label = format!("{}{}", key(task), slug(&task.name));
    let out = rex(&[
        "new",
        &label,
        "-c",
        cwd,
        "--interactive",
        "--json",
        "--",
        "claude",
        &prompt(task),
    ])?;
    let created: Session = serde_json::from_str(&out).map_err(|e| format!("rex new: {e}"))?;
    let mut reg = load_registry();
    reg.insert(
        created.session_id.clone(),
        Link {
            gid: task.gid.clone(),
            name: task.name.clone(),
        },
    );
    save_registry(&reg);
    focus(&created.session_id)?;
    Ok(created.session_id)
}
