//! Plain data types and pure logic: no I/O here, so it is easy to test and extend.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Color;
use ratatui::text::Text;
use serde::Deserialize;
use std::ops::Range;

/// One agent as reported by `workmux status --all --json`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Agent {
    pub pane_id: String,
    pub status: Option<String>,
    pub project: Option<String>,
    pub worktree: String,
    pub branch: Option<String>,
    pub title: Option<String>,
    pub project_path: Option<String>,
    pub workdir: Option<String>,
    /// Idle, but the Claude Code footer shows a background task (filled in after parsing).
    #[serde(skip)]
    pub background: bool,
    /// The agent's latest `⏺` line: what it says it is doing (filled in after parsing).
    #[serde(skip)]
    pub activity: Option<String>,
}

/// Status in sort order: agents that need you come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Waiting,
    Done,
    Background,
    Working,
    Unknown,
}

impl Status {
    /// Nerd Font icon and its color: comment, check, hourglass, robot.
    pub fn icon(self) -> (&'static str, Color) {
        match self {
            Status::Waiting => ("\u{f075}", Color::Yellow),
            Status::Done => ("\u{f00c}", Color::Green),
            Status::Background => ("\u{f252}", Color::Blue),
            Status::Working => ("\u{f06a9}", Color::Magenta),
            Status::Unknown => (" ", Color::Reset),
        }
    }
}

impl Agent {
    pub fn status(&self) -> Status {
        match self.status.as_deref() {
            Some("working") => Status::Working,
            _ if self.background => Status::Background,
            Some("waiting") => Status::Waiting,
            Some("done") => Status::Done,
            _ => Status::Unknown,
        }
    }

    /// "project/worktree", or just the worktree when workmux has no project for it.
    pub fn name(&self) -> String {
        match &self.project {
            Some(p) => format!("{p}/{}", self.worktree),
            None => self.worktree.clone(),
        }
    }

    /// Runs in the main checkout, so there is no worktree to remove.
    pub fn is_main_checkout(&self) -> bool {
        self.workdir.is_some() && self.workdir == self.project_path
    }
}

/// Grouped by project, then by status inside each project.
pub fn sort_agents(agents: &mut [Agent]) {
    agents.sort_by_cached_key(|a| (a.project.clone(), a.status(), a.worktree.clone()));
}

/// Claude Code prefixes titles with ✳ or a spinner char; drop leading non-letter/non-digit symbols.
pub fn clean_title(title: &str) -> &str {
    title.trim_start_matches(|c: char| !c.is_alphanumeric())
}

/// True when one of the last 2 non-blank lines matches `· \d+ (monitor|shell)s?\b`.
/// The last line Claude Code marks with `⏺`, its note on the current step or tool call.
pub fn last_activity(capture: &str) -> Option<&str> {
    capture.lines().rev().find_map(|l| l.trim_start().strip_prefix('⏺')).map(str::trim)
}

pub fn has_background_task(capture: &str) -> bool {
    let lines: Vec<&str> = capture.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(2)..].iter().any(|l| {
        l.match_indices("· ").any(|(i, m)| {
            let rest = &l[i + m.len()..];
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            let Some(rest) = rest[digits..].strip_prefix(' ').filter(|_| digits > 0) else {
                return false;
            };
            ["monitor", "shell"]
                .iter()
                .any(|w| rest.strip_prefix(w).is_some_and(|r| !starts_with_word(r.strip_prefix('s').unwrap_or(r))))
        })
    })
}

fn starts_with_word(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Line is blank once ANSI escape sequences (CSI and OSC) are removed.
pub fn is_blank(line: &str) -> bool {
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next() == Some('\\')) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            c if !c.is_whitespace() => return false,
            _ => {}
        }
    }
    true
}

/// A mouse event at 0-based (`x`, `y`) in SGR form, as a terminal sends it to a program.
/// None for plain moves, which nothing in the agent needs.
pub fn mouse_seq(kind: MouseEventKind, mods: KeyModifiers, x: u16, y: u16) -> Option<String> {
    let button = |b| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut code, end) = match kind {
        MouseEventKind::Down(b) => (button(b), 'M'),
        MouseEventKind::Up(b) => (button(b), 'm'),
        MouseEventKind::Drag(b) => (button(b) + 32, 'M'),
        MouseEventKind::ScrollUp => (64, 'M'),
        MouseEventKind::ScrollDown => (65, 'M'),
        MouseEventKind::ScrollLeft => (66, 'M'),
        MouseEventKind::ScrollRight => (67, 'M'),
        MouseEventKind::Moved => return None,
    };
    for (m, bit) in [(KeyModifiers::SHIFT, 4), (KeyModifiers::ALT, 8), (KeyModifiers::CONTROL, 16)] {
        if mods.contains(m) {
            code += bit;
        }
    }
    Some(format!("\x1b[<{code};{};{}{end}", x + 1, y + 1))
}

/// Drop OSC sequences, like OSC 8 links (`ESC ] … ESC \` or `… BEL`). ansi-to-tui stops parsing the
/// line at the `ESC \` form, so the link and the rest of the line would vanish.
pub fn strip_osc(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("\x1b]") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 2..];
        rest = &rest[rest.find(['\x07', '\x1b']).unwrap_or(rest.len())..];
        rest = rest.strip_prefix('\x07').or_else(|| rest.strip_prefix("\x1b\\")).unwrap_or(rest);
    }
    out.push_str(rest);
    out
}

/// The top-level `agent:` command in a workmux config file.
pub fn agent_cmd(config: &str) -> Option<&str> {
    let cmd = config.lines().find_map(|l| l.strip_prefix("agent:"))?.trim();
    Some(cmd.trim_matches(['"', '\'']).trim()).filter(|c| !c.is_empty())
}

/// Rows of the pane to show: ending at the last non-blank line or the cursor line, whichever is
/// lower, and at most `height` rows.
pub fn crop(lines: &[&str], cursor_y: usize, height: usize) -> Range<usize> {
    let last_text = lines.iter().rposition(|l| !is_blank(l)).map_or(0, |i| i + 1);
    let end = last_text.max(cursor_y + 1).min(lines.len());
    end.saturating_sub(height)..end
}

/// The tmux `send-keys` name for a key press, or None for keys we don't forward.
pub fn tmux_key(key: KeyEvent) -> Option<String> {
    let m = key.modifiers;
    let mut prefix = String::new();
    if m.contains(KeyModifiers::CONTROL) {
        prefix.push_str("C-");
    }
    if m.contains(KeyModifiers::ALT) {
        prefix.push_str("M-");
    }
    let name = match key.code {
        KeyCode::Char(' ') => "Space".into(),
        // A trailing ';' is a command separator to tmux; "\;" is a literal one.
        KeyCode::Char(';') => "\\;".into(),
        KeyCode::Char(c) if m.contains(KeyModifiers::CONTROL) => c.to_lowercase().to_string(),
        KeyCode::Char(c) => c.to_string(),
        code => {
            if m.contains(KeyModifiers::SHIFT) && code != KeyCode::BackTab {
                prefix.push_str("S-");
            }
            match code {
                KeyCode::Enter => "Enter".into(),
                KeyCode::Esc => "Escape".into(),
                KeyCode::Backspace => "BSpace".into(),
                KeyCode::Tab => "Tab".into(),
                KeyCode::BackTab => "BTab".into(),
                KeyCode::Up => "Up".into(),
                KeyCode::Down => "Down".into(),
                KeyCode::Left => "Left".into(),
                KeyCode::Right => "Right".into(),
                KeyCode::Home => "Home".into(),
                KeyCode::End => "End".into(),
                KeyCode::PageUp => "PPage".into(),
                KeyCode::PageDown => "NPage".into(),
                KeyCode::Delete => "DC".into(),
                KeyCode::Insert => "IC".into(),
                KeyCode::F(n) => format!("F{n}"),
                _ => return None,
            }
        }
    };
    Some(prefix + &name)
}

/// The previewed pane, already cropped and parsed.
#[derive(Debug, Clone)]
pub struct Preview {
    pub pane: String,
    pub text: Text<'static>,
    /// Cursor (x, y) relative to the first shown row, when tmux says it is visible.
    pub cursor: Option<(u16, u16)>,
}

#[derive(Debug, Default)]
pub struct App {
    pub agents: Vec<Agent>,
    pub sel: usize,
    pub insert: bool,
    /// The key help popover is open.
    pub help: bool,
    pub preview: Option<Preview>,
    /// A new agent's placeholder, listed until workmux lists its pane.
    pub pending: Option<Agent>,
}

impl App {
    pub fn selected(&self) -> Option<&Agent> {
        self.agents.get(self.sel)
    }

    /// Replace the list; the selection follows the same pane_id, else stays at the same index.
    pub fn set_agents(&mut self, mut agents: Vec<Agent>) {
        if let Some(p) = &self.pending {
            if agents.iter().any(|a| a.pane_id == p.pane_id) {
                self.pending = None;
            } else {
                agents.push(p.clone());
                sort_agents(&mut agents);
            }
        }
        let current = self.selected().map(|a| a.pane_id.clone());
        self.sel = current
            .and_then(|id| agents.iter().position(|a| a.pane_id == id))
            .unwrap_or_else(|| self.sel.min(agents.len().saturating_sub(1)));
        self.agents = agents;
    }

    /// List a just-split pane as a placeholder agent like `like`, and select it.
    pub fn add_pending(&mut self, like: &Agent, pane: String) {
        let p = Agent { pane_id: pane, title: Some("starting…".into()), ..like.clone() };
        let p = Agent { status: None, background: false, activity: None, ..p };
        self.pending = Some(p.clone());
        let agents = std::mem::take(&mut self.agents);
        self.set_agents(agents);
        self.sel = self.agents.iter().position(|a| a.pane_id == p.pane_id).unwrap_or(self.sel);
    }

    pub fn move_sel(&mut self, down: bool) {
        if down {
            if self.sel + 1 < self.agents.len() {
                self.sel += 1;
            }
        } else {
            self.sel = self.sel.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(pane: &str, status: &str, project: &str, worktree: &str) -> Agent {
        Agent {
            pane_id: pane.into(),
            status: Some(status.into()),
            project: Some(project.into()),
            worktree: worktree.into(),
            ..Default::default()
        }
    }

    #[test]
    fn sort_order() {
        let mut bg = agent("%5", "done", "a", "bg");
        bg.background = true;
        let mut working_bg = agent("%6", "working", "a", "w2");
        working_bg.background = true; // working always wins over ⏳
        let mut agents = vec![
            agent("%1", "working", "b", "x"),
            agent("%2", "", "a", "x"),
            bg,
            agent("%3", "done", "z", "x"),
            agent("%4", "waiting", "b", "x"),
            agent("%7", "waiting", "a", "x"),
            working_bg,
        ];
        sort_agents(&mut agents);
        let ids: Vec<_> = agents.iter().map(|a| a.pane_id.as_str()).collect();
        assert_eq!(ids, ["%7", "%5", "%6", "%2", "%4", "%1", "%3"]);
        assert_eq!(agents[1].status(), Status::Background);
    }

    #[test]
    fn titles() {
        assert_eq!(clean_title("✳ Port wm-dash"), "Port wm-dash");
        assert_eq!(clean_title("⠂ 2 tasks"), "2 tasks");
        assert_eq!(clean_title("Ölfass"), "Ölfass");
        assert_eq!(clean_title("✳ "), "");
    }

    #[test]
    fn background_task() {
        assert!(has_background_task("out\n\n  ⏵⏵ accept edits · 1 shell\n  \n"));
        assert!(has_background_task("x · 2 monitors\nlast line\n"));
        assert!(has_background_task("· 12 shells · foo"));
        assert!(!has_background_task("x · 2 monitors\nline\nlast line\n")); // not in the last 2
        assert!(!has_background_task("· 1 shellfish"));
        assert!(!has_background_task("· shell"));
        assert!(!has_background_task("·1 shell"));
        assert!(!has_background_task(""));
    }

    #[test]
    fn activity() {
        let capture = "⏺ Now the MCP server tools.\n  Ran 2 shell commands\n⏺ Now the frontend.\n  Running…\n❯ \n";
        assert_eq!(last_activity(capture), Some("Now the frontend."));
        assert_eq!(last_activity("no marks\n"), None);
    }

    #[test]
    fn blank_lines() {
        assert!(is_blank("  \x1b[0m \x1b[38;5;1m"));
        assert!(is_blank("\x1b]8;;http://x\x1b\\\x1b]8;;\x07"));
        assert!(!is_blank("\x1b[1m a"));
    }

    #[test]
    fn crop_rows() {
        let lines = ["a", "b", "c", "", "", ""];
        assert_eq!(crop(&lines, 0, 10), 0..3); // ends at the last text
        assert_eq!(crop(&lines, 4, 10), 0..5); // or the cursor, if lower
        assert_eq!(crop(&lines, 1, 2), 1..3); // keeps the bottom
        assert_eq!(crop(&lines, 9, 2), 4..6); // cursor past the end
        assert_eq!(crop(&["", ""], 0, 5), 0..1);
    }

    #[test]
    fn keys() {
        let k = |code, m| tmux_key(KeyEvent::new(code, m));
        assert_eq!(k(KeyCode::Char('a'), KeyModifiers::NONE).unwrap(), "a");
        assert_eq!(k(KeyCode::Char('A'), KeyModifiers::SHIFT).unwrap(), "A");
        assert_eq!(k(KeyCode::Char('c'), KeyModifiers::CONTROL).unwrap(), "C-c");
        assert_eq!(k(KeyCode::Char('x'), KeyModifiers::ALT).unwrap(), "M-x");
        assert_eq!(k(KeyCode::Char(' '), KeyModifiers::NONE).unwrap(), "Space");
        assert_eq!(k(KeyCode::Char(';'), KeyModifiers::NONE).unwrap(), "\\;");
        assert_eq!(k(KeyCode::Enter, KeyModifiers::NONE).unwrap(), "Enter");
        assert_eq!(k(KeyCode::Esc, KeyModifiers::NONE).unwrap(), "Escape");
        assert_eq!(k(KeyCode::Backspace, KeyModifiers::NONE).unwrap(), "BSpace");
        assert_eq!(k(KeyCode::Up, KeyModifiers::NONE).unwrap(), "Up");
        assert_eq!(k(KeyCode::Left, KeyModifiers::CONTROL).unwrap(), "C-Left");
        assert_eq!(k(KeyCode::Up, KeyModifiers::SHIFT).unwrap(), "S-Up");
        assert_eq!(k(KeyCode::BackTab, KeyModifiers::SHIFT).unwrap(), "BTab");
        assert_eq!(k(KeyCode::F(5), KeyModifiers::NONE).unwrap(), "F5");
        assert_eq!(k(KeyCode::CapsLock, KeyModifiers::NONE), None);
    }

    #[test]
    fn mouse_seq() {
        let seq = |k, m| super::mouse_seq(k, m, 9, 4);
        let none = KeyModifiers::NONE;
        assert_eq!(super::mouse_seq(MouseEventKind::ScrollUp, none, 0, 0).unwrap(), "\x1b[<64;1;1M");
        assert_eq!(seq(MouseEventKind::ScrollDown, none).unwrap(), "\x1b[<65;10;5M");
        assert_eq!(seq(MouseEventKind::Down(MouseButton::Left), none).unwrap(), "\x1b[<0;10;5M");
        assert_eq!(seq(MouseEventKind::Drag(MouseButton::Left), none).unwrap(), "\x1b[<32;10;5M");
        assert_eq!(seq(MouseEventKind::Up(MouseButton::Left), none).unwrap(), "\x1b[<0;10;5m");
        assert_eq!(seq(MouseEventKind::Down(MouseButton::Right), KeyModifiers::SHIFT).unwrap(), "\x1b[<6;10;5M");
        assert_eq!(seq(MouseEventKind::Moved, none), None);
    }

    #[test]
    fn agent_cmd() {
        let config = "nerdfont: true\nagent: claude --permission-mode auto\npanes:\n  - command: <agent>\n";
        assert_eq!(super::agent_cmd(config), Some("claude --permission-mode auto"));
        assert_eq!(super::agent_cmd("agent: \"codex\"\n"), Some("codex"));
        assert_eq!(super::agent_cmd("panes: []\n"), None);
    }

    #[test]
    fn strip_osc() {
        let link = "a \x1b[94m\x1b]8;id=1;https://x.com\x1b\\https://x.com\x1b[39m\x1b]8;;\x1b\\ b";
        assert_eq!(super::strip_osc(link), "a \x1b[94mhttps://x.com\x1b[39m b");
        assert_eq!(super::strip_osc("x\x1b]0;title\x07y"), "xy");
        assert_eq!(super::strip_osc("plain"), "plain");
    }

    #[test]
    fn reads_branch_from_workmux() {
        let a: Agent = serde_json::from_str(r#"{"pane_id":"%1","worktree":"wm-dash","branch":"main"}"#).unwrap();
        assert_eq!(a.branch.as_deref(), Some("main"));
    }

    #[test]
    fn selection_follows_pane() {
        let mut app = App::default();
        app.set_agents(vec![agent("%1", "", "a", "1"), agent("%2", "", "a", "2")]);
        app.move_sel(true);
        app.set_agents(vec![agent("%9", "", "a", "9"), agent("%3", "", "a", "3"), agent("%2", "", "a", "2")]);
        assert_eq!(app.selected().unwrap().pane_id, "%2");
        app.set_agents(vec![agent("%1", "", "a", "1")]); // gone: clamp
        assert_eq!(app.sel, 0);
        app.set_agents(vec![]);
        assert!(app.selected().is_none());
    }

    #[test]
    fn selects_new_agent() {
        let mut app = App::default();
        app.set_agents(vec![agent("%1", "", "a", "1"), agent("%2", "", "a", "2")]);
        app.add_pending(&app.agents[0].clone(), "%5".into());
        assert_eq!(app.selected().unwrap().pane_id, "%5");
        // Not listed by workmux yet: the placeholder stays, and stays selected.
        app.set_agents(vec![agent("%1", "", "a", "1"), agent("%2", "", "a", "2")]);
        assert_eq!(app.selected().unwrap().title.as_deref(), Some("starting…"));
        app.set_agents(vec![agent("%1", "", "a", "1"), agent("%5", "working", "a", "1")]);
        assert_eq!(app.selected().unwrap().status(), Status::Working);
        assert!(app.pending.is_none());
    }
}
