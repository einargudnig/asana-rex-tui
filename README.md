# asana-tui

A terminal view of your Asana tasks that opens a Claude Code session in Rex for any task.

## Requirements

- `asana-cli` on `PATH`. The app reads tasks through it.
- An Asana personal access token, either:
  - exported as `ASANA_ACCESS_TOKEN`, or
  - stored in the macOS Keychain under service `asana-access-token`:
    ```sh
    security add-generic-password -a "$USER" -s asana-access-token -w <token>
    ```
    The Keychain fallback lets the app start from Rex bindings and shells that never sourced the export.
- `rex` on `PATH`, for the Sessions tab and for launching Claude. The task views work without it.
- macOS. `o` uses `open`, and the token fallback uses `security`.

## Install

```sh
cargo install --path .     # installs ~/.cargo/bin/asana-tui
```

Re-run the same command after changing the code; it replaces the installed binary. Use `cargo run` to try a change without installing it.

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `ASANA_ACCESS_TOKEN` | Keychain | Asana token |
| `ASANA_WORKSPACE` | first workspace | Workspace gid |
| `ASANA_TUI_CWD` | `~/work` | Directory new Claude sessions start in |

## Views

- **1 Today**: three groups.
  - *In progress*: tasks in your My Tasks "Today" section.
  - *Up next*: open tasks due within 10 days, overdue ones included.
  - *Done since yesterday*.
- **2 My Tasks**: all open tasks, grouped by My Tasks section in Asana's order.
- **3 Sessions**: open Rex sessions that belong to a task.
- **8 Inbox**: your unread Asana comments and assignments, one row per task, newest first, with who sent the latest and how many more there are. Other notifications (likes, due date changes, …) are left out. `m` marks an item read in Asana, and replying with `a` marks it read too.
- **4 Milestones**, **5 Decisions**, **6 Blockers**, **7 Questions**: open items across initiative projects, grouped by project, soonest due first. The right-hand column shows the assignee instead of the project.
  - An initiative project is any open project that uses the `Attention type` custom field.
  - Decisions, Blockers and Questions are tasks whose `Attention type` is `Decision`, `Blocker` or `Question`. Milestones are milestone tasks.
  - The header shows how many decisions, blockers and questions are open.
  - Each project's header shows its latest Asana status (on track, at risk, off track) and how many days old it is. The age turns amber after 7 days, since owners are asked to set a status weekly. `no status` means none has been set.
  - Each Asana search returns at most 100 tasks, so very large initiatives may be cut short.

Due dates are relative: `2d late` and `today` are coloured, then `tmrw`, a weekday within the next week, and the date after that. A `●` after a task means it has an open Rex session.

Tasks reload every 2 minutes and on `r`.

## Keys

### List

| Key | Action |
|---|---|
| `j` / `k`, ↓ / ↑ | Move |
| `g` / `G` | First / last task |
| `1`–`8`, `h` / `l`, Tab | Switch view (Tab switches focus while details are open) |
| `enter` | Focus the task's Rex session, or start one running Claude |
| `space` / `p` | Toggle the details pane |
| `J` / `K` | Scroll the details pane by 3 lines |
| `o` | Open the task in the browser |
| `x` | Mark done / reopen |
| `m` | Mark read (Inbox) |
| `a` | Add a comment: `^s` posts, `enter` starts a new line (shown as `↵`), `esc` cancels, `^u` clears |
| `r` | Refresh |
| `?` | Show all keys |
| `q`, `Esc`, `^c` | Quit |

### Details pane

With the pane open, press **Tab** to focus it. The border turns blue.

| Key | Action |
|---|---|
| `j` / `k`, ↓ / ↑ | Scroll 1 line |
| `^d` / `^u`, PgDn / PgUp | Scroll 10 lines |
| `g` / `G` | Top / bottom |
| Tab, `Esc`, `h` | Back to the list |

Other keys (`o`, `x`, `enter`, `space`) still act on the selected task. A scrollbar appears on the right when the content is taller than the pane. The footer always shows the keys for whatever has focus.

The pane shows:

- the task name and each project › section it belongs to
- due date (with how late it is), completion time, creator, tags, custom fields and followers
- notes, subtasks, and the latest 5 comments

Links in notes and comments are shortened: mentions show as `@Name`, task links show the task's title, and attachments show as `▤ attachment`.

Details load once the cursor has rested on a task for 200 ms. The pane sits beside the list in windows at least 110 columns wide and below it otherwise.

## Rex sessions

`enter` on a task with no session runs:

```sh
rex new asana-<last 6 digits of gid>-<slug> -c $ASANA_TUI_CWD -- claude "<prompt>"
```

The prompt asks Claude to read the task through the Asana MCP and propose a plan before changing anything. Sessions are matched to tasks by the gid in their label, so renaming a task keeps its session. Focusing a session requires a connected Rex app.
