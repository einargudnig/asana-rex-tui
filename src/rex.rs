use crate::asana::Task;
use serde::Deserialize;
use std::process::Command;

pub const PREFIX: &str = "asana-";

#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    pub session_id: String,
    #[serde(default)]
    pub label: String,
}

impl Session {
    /// Only labels shaped `asana-<6 digits>-…` are ours; `asana-tui` is not.
    fn is_task_session(&self) -> bool {
        self.label.strip_prefix(PREFIX).is_some_and(|rest| {
            let b = rest.as_bytes();
            b.len() > 7 && b[..6].iter().all(u8::is_ascii_digit) && b[6] == b'-'
        })
    }

    pub fn task_name(&self) -> &str {
        self.label
            .strip_prefix(PREFIX)
            .and_then(|s| s.split_once('-'))
            .map_or(&self.label, |(_, n)| n)
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
    rex(&["ls", "--json"])
        .ok()
        .and_then(|s| serde_json::from_str::<Ls>(&s).ok())
        .map(|ls| {
            ls.sessions
                .into_iter()
                .filter(Session::is_task_session)
                .collect()
        })
        .unwrap_or_default()
}

pub fn session_for<'a>(task: &Task, sessions: &'a [Session]) -> Option<&'a Session> {
    let key = key(task);
    sessions.iter().find(|s| s.label.starts_with(&key))
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
    focus(&created.session_id)?;
    Ok(created.session_id)
}
