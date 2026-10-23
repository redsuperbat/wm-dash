//! Everything that talks to tmux and workmux, plus the background workers that do it.

use crate::app::{self, Agent, Preview};
use ansi_to_tui::IntoText;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

pub const LIST_INTERVAL: Duration = Duration::from_secs(2);
pub const PREVIEW_INTERVAL: Duration = Duration::from_millis(66);
/// Timer for a watched pane, which mostly updates on its own output.
const WATCHED_INTERVAL: Duration = Duration::from_millis(500);

/// Run tmux and return stdout, or None when it fails (e.g. the pane is gone).
pub fn tmux(args: &[&str]) -> Option<String> {
    let out = Command::new("tmux").args(args).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn fetch_agents() -> Result<Vec<Agent>> {
    #[derive(serde::Deserialize)]
    struct Status {
        agents: Vec<Agent>,
    }
    let out = Command::new("workmux")
        .args(["status", "--all", "--json"])
        .stderr(Stdio::null())
        .output()
        .context("running workmux")?;
    let mut agents = serde_json::from_slice::<Status>(&out.stdout).context("parsing workmux status")?.agents;
    for a in &mut agents {
        // -J joins wrapped lines, so a long activity line comes through whole.
        let capture = tmux(&["capture-pane", "-pJ", "-t", &a.pane_id]).unwrap_or_default();
        a.background = a.status.as_deref() != Some("working") && app::has_background_task(&capture);
        a.activity = app::last_activity(&capture).map(String::from);
    }
    let sessions = sessions();
    let paths = sessions.iter().map(|s| s.path.as_str());
    let paths = paths.chain(agents.iter().filter_map(|a| a.project_path.as_deref().or(a.workdir.as_deref())));
    let repos: HashMap<String, String> = paths.filter_map(|p| Some((p.to_string(), repo(p)?))).collect();
    let mut agents = app::group_by_session(agents, &sessions, &repos);
    let commands = tmux(&["list-panes", "-a", "-F", "#{pane_id} #{pane_current_command}"]).unwrap_or_default();
    let commands: HashMap<&str, &str> = commands.lines().filter_map(|l| l.split_once(' ')).collect();
    for a in &mut agents {
        a.exited = commands.get(a.pane_id.as_str()).is_some_and(|c| app::is_shell(c));
    }
    Ok(agents)
}

/// Every tmux session, oldest first.
fn sessions() -> Vec<app::Session> {
    let out = tmux(&["list-sessions", "-F", "#{session_created}\t#{session_name}\t#{pane_id}\t#{session_path}"]);
    let mut sessions: Vec<(u64, app::Session)> = out
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let [created, name, pane, path] = l.splitn(4, '\t').collect::<Vec<_>>()[..] else { return None };
            let s = app::Session { name: name.into(), path: path.into(), pane: pane.into() };
            Some((created.parse().unwrap_or(0), s))
        })
        .collect();
    sessions.sort_by_key(|(created, _)| *created);
    sessions.into_iter().map(|(_, s)| s).collect()
}

/// The git repo a folder belongs to, the same for all of a repo's worktrees.
fn repo(path: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["-C", path, "rev-parse", "--path-format=absolute", "--git-common-dir"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Refresh the agent list every 2s, or right away when `refresh` gets a message.
pub fn list_worker(tx: Sender<crate::Msg>, refresh: Receiver<()>) {
    loop {
        if let Ok(agents) = fetch_agents()
            && tx.send(crate::Msg::Agents(agents)).is_err()
        {
            return;
        }
        match refresh.recv_timeout(LIST_INTERVAL) {
            Ok(()) => while refresh.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// What to preview: the pane and the size of the preview box's inside.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewReq {
    pub pane: String,
    pub width: u16,
    pub height: u16,
}

/// What the preview worker waits for.
pub enum PreviewMsg {
    /// Preview this pane now (None: nothing selected).
    Req(Option<PreviewReq>),
    /// The watched pane printed something.
    Changed,
}

/// Capture the requested pane whenever it prints something, and send it when it changed. tmux
/// copies the pane's output into a FIFO (`watch`), so the preview follows the pane as it draws.
/// A slow timer also catches what prints nothing, like a resize; when the pane can't be watched
/// (it already has a pipe of its own), the timer runs about 15 times a second instead.
pub fn preview_worker(tx: Sender<crate::Msg>, msgs: Receiver<PreviewMsg>, notify: Sender<PreviewMsg>) {
    let fifo = fifo_path();
    let made = Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success());
    if made {
        std::thread::spawn({
            let fifo = fifo.clone();
            move || read_fifo(&fifo, notify)
        });
    }
    let mut req = None;
    let mut last = None;
    loop {
        let interval = if watching() { WATCHED_INTERVAL } else { PREVIEW_INTERVAL };
        match msgs.recv_timeout(interval) {
            Ok(m) => {
                // Take everything queued: a burst of output costs one capture.
                for m in std::iter::once(m).chain(msgs.try_iter()) {
                    if let PreviewMsg::Req(r) = m {
                        req = r;
                        last = None;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        watch(req.as_ref().map(|r: &PreviewReq| r.pane.as_str()).filter(|_| made), &fifo);
        let Some(r) = &req else {
            fit(None, 0, 0, None);
            continue;
        };
        let Some((raw, preview)) = capture(r) else { continue };
        if last.as_ref() != Some(&raw) {
            last = Some(raw);
            if tx.send(crate::Msg::Preview(preview)).is_err() {
                return;
            }
        }
    }
}

/// The FIFO tmux copies the watched pane's output into; one per dashboard.
fn fifo_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("wm-dash-{}.fifo", std::process::id()))
}

/// Tell the preview worker each time the watched pane prints. The data itself is not needed.
/// Opening blocks until tmux's `cat` opens the other end; each new pipe opens it again.
fn read_fifo(fifo: &std::path::Path, notify: Sender<PreviewMsg>) {
    use std::io::Read;
    let mut buf = [0; 4096];
    while let Ok(mut f) = std::fs::File::open(fifo) {
        while f.read(&mut buf).is_ok_and(|n| n > 0) {
            if notify.send(PreviewMsg::Changed).is_err() {
                return;
            }
        }
    }
}

/// The watched pane, and whether the pipe on it is ours. Global so exit, panic and signal
/// handlers can close it.
static WATCHED: Mutex<(Option<String>, bool)> = Mutex::new((None, false));

/// Point tmux's output pipe at `pane`, closing ours on the previous one. A pane that already has
/// a pipe (a logging plugin, say) keeps it, and is previewed on the timer instead.
fn watch(pane: Option<&str>, fifo: &std::path::Path) {
    let mut w = WATCHED.lock().unwrap_or_else(|e| e.into_inner());
    if w.0.as_deref() == pane {
        return;
    }
    unwatch_locked(&mut w);
    w.0 = pane.map(String::from);
    let Some(pane) = pane else { return };
    if tmux(&["display", "-p", "-t", pane, "#{pane_pipe}"]).is_some_and(|p| p.trim() == "0") {
        let cmd = format!("cat > '{}'", fifo.display());
        w.1 = tmux(&["pipe-pane", "-O", "-t", pane, &cmd]).is_some();
    }
}

fn watching() -> bool {
    WATCHED.lock().unwrap_or_else(|e| e.into_inner()).1
}

fn unwatch_locked(w: &mut (Option<String>, bool)) {
    if let (Some(pane), true) = (w.0.take(), w.1) {
        // No command: closes the pane's pipe.
        tmux(&["pipe-pane", "-t", &pane]);
    }
    w.1 = false;
}

/// Close our pipe and remove the FIFO, on exit.
pub fn stop_watching() {
    unwatch_locked(&mut WATCHED.lock().unwrap_or_else(|e| e.into_inner()));
    let _ = std::fs::remove_file(fifo_path());
}

/// One tmux call for pane sizes, cursor and contents. Returns the raw cropped capture (to
/// detect changes) and the parsed preview.
fn capture(r: &PreviewReq) -> Option<(String, Preview)> {
    let fmt = "#{pane_id} #{pane_width} #{pane_height} #{window_width} #{window_height} \
               #{cursor_x} #{cursor_y} #{cursor_flag}";
    let out = tmux(&["display", "-p", "-t", &r.pane, fmt, ";", "capture-pane", "-ep", "-t", &r.pane])?;
    let (head, body) = out.split_once('\n')?;
    let f: Vec<&str> = head.split(' ').collect();
    // The id check skips agents whose pane is gone.
    if f.len() != 8 || f[0] != r.pane {
        return None;
    }
    let n: Vec<u16> = f[1..].iter().map(|s| s.parse().unwrap_or(0)).collect();
    fit(Some(&r.pane), r.width, r.height, Some([n[0], n[1], n[2], n[3]]));
    let (cx, cy, visible) = (n[4], n[5] as usize, n[6] == 1);

    let lines: Vec<&str> = body.lines().collect();
    let rows = app::crop(&lines, cy, r.height as usize);
    let cursor = (visible && cy >= rows.start).then(|| (cx, (cy - rows.start) as u16));
    let shown = &lines[rows];
    let mut text = ratatui::text::Text::default();
    for l in shown {
        // Parse line by line so a style never leaks onto the next row.
        text.lines.extend(app::strip_osc(l).into_text().map(|t| t.lines).unwrap_or_default());
    }
    let raw = format!("{cursor:?}{}", shown.join("\n"));
    Some((raw, Preview { pane: r.pane.clone(), text, cursor }))
}

/// The pane whose window we resized, and the layout to restore. Global so exit, panic and
/// signal handlers can undo it.
struct Fitted {
    pane: Option<String>,
    layout: String,
    disabled: bool,
}

static FITTED: Mutex<Fitted> = Mutex::new(Fitted { pane: None, layout: String::new(), disabled: false });

/// Resize the previewed pane's window so the pane exactly fills the preview box and the agent
/// reflows to it. Skipped when the agent shares the dashboard's window: resizing it would resize
/// us too, in a loop. `sizes` is pane width/height and window width/height.
fn fit(pane: Option<&str>, bw: u16, bh: u16, sizes: Option<[u16; 4]>) {
    let mut f = FITTED.lock().unwrap_or_else(|e| e.into_inner());
    if f.disabled {
        return;
    }
    if f.pane.as_deref() != pane {
        unfit_locked(&mut f);
        let Some(pane) = pane else { return };
        f.pane = Some(pane.to_string());
        f.layout = tmux(&["display", "-p", "-t", pane, "#{window_layout}"]).unwrap_or_default().trim().into();
        if let Ok(own) = std::env::var("TMUX_PANE") {
            let window = |p: &str| tmux(&["display", "-p", "-t", p, "#{window_id}"]);
            if window(pane) == window(&own) {
                f.layout.clear();
            }
        }
    }
    let (Some(pane), Some([pw, ph, ww, wh])) = (pane, sizes) else { return };
    if f.layout.is_empty() || (pw == bw && ph == bh) {
        return;
    }
    let x = (ww + bw).saturating_sub(pw).to_string();
    let y = (wh + bh).saturating_sub(ph).to_string();
    let (bw, bh) = (bw.to_string(), bh.to_string());
    tmux(&["resize-window", "-t", pane, "-x", &x, "-y", &y, ";", "resize-pane", "-t", pane, "-x", &bw, "-y", &bh]);
}

/// Hand the size back to tmux and restore the old split layout.
fn unfit_locked(f: &mut Fitted) {
    if let Some(pane) = f.pane.take()
        && !f.layout.is_empty()
    {
        tmux(&["set", "-wu", "-t", &pane, "window-size", ";", "select-layout", "-t", &pane, &f.layout]);
    }
}

/// Undo the fit and stop fitting (while suspended or exiting) or start again.
pub fn set_fitting(on: bool) {
    let mut f = FITTED.lock().unwrap_or_else(|e| e.into_inner());
    if !on {
        unfit_locked(&mut f);
    }
    f.disabled = !on;
}

/// Start another agent in a new window right after the agent's own, in the same directory. A
/// window rather than a split, so previewing one agent never squeezes another. The command is
/// typed into a fresh shell, as workmux does, so the shell and direnv set things up first.
/// `; exit` closes the window when the agent quits. Returns the new pane.
pub fn new_agent(a: &Agent) -> Option<String> {
    let read = |path: &str| std::fs::read_to_string(path.trim()).ok();
    let project = a.project_path.as_deref().and_then(|p| read(&format!("{p}/.workmux.yaml")));
    let global = Command::new("workmux").args(["config", "path"]).output().ok();
    let global = global.and_then(|o| read(&String::from_utf8_lossy(&o.stdout)));
    let configs = [project, global];
    let cmd = configs.iter().flatten().find_map(|c| app::agent_cmd(c));
    let dir = a.workdir.as_deref().unwrap_or("#{pane_current_path}");
    // new-window wants a window, not a pane.
    let window = tmux(&["display", "-p", "-t", &a.pane_id, "#{window_id}"])?;
    let new = ["new-window", "-ad", "-t", window.trim(), "-c", dir, "-P", "-F", "#{pane_id}"];
    let pane = tmux(&new)?.trim().to_string();
    let cmd = format!("{}; exit", cmd.unwrap_or("claude"));
    tmux(&["send-keys", "-t", &pane, "-l", &cmd, ";", "send-keys", "-t", &pane, "Enter"]);
    Some(pane)
}

pub fn send_key(pane: &str, key: &str) {
    tmux(&["send-keys", "-t", pane, key]);
}

/// Type `text` into the pane as is, escape sequences included.
pub fn send_text(pane: &str, text: &str) {
    tmux(&["send-keys", "-t", pane, "-l", text]);
}

/// Paste as the terminal would: bracketed if the agent asked for it.
pub fn paste(pane: &str, text: &str) {
    let Ok(mut child) = Command::new("tmux")
        .args(["load-buffer", "-b", "wm-dash", "-", ";", "paste-buffer", "-p", "-d", "-b", "wm-dash", "-t", pane])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let _ = child.wait();
}
