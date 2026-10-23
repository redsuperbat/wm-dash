use crate::app::{self, App};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use std::time::{Duration, SystemTime};
use unicode_width::UnicodeWidthChar;

const HINTS: &str = " ? keys · q quit";

const KEYS: &[(&str, &str)] = &[
    ("j/k ↑/↓", "move the selection (or click an agent)"),
    ("ctrl+u/d", "scroll the preview a page"),
    ("ctrl+y/e", "scroll the preview a little (or the mouse wheel)"),
    ("i", "insert mode: keys go to the agent"),
    ("ctrl+q", "back to the list from insert mode"),
    ("enter", "jump to the agent's pane and quit"),
    ("l", "lazygit in the agent's workdir"),
    ("v", "nvim in the agent's workdir"),
    ("t", "shell in the agent's workdir (exit to come back)"),
    ("g", "open the branch's PR on GitHub"),
    ("r", "remove the agent and its worktree"),
    ("n", "new agent in a new window of this session"),
    ("?", "show this help"),
    ("q ctrl+c", "quit"),
];

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
pub const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// The spinner frame for now, from the clock so nothing has to keep count.
fn spinner() -> char {
    let ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis();
    SPINNER[(ms / SPINNER_INTERVAL.as_millis()) as usize % SPINNER.len()]
}

/// Width of the agent list column.
pub fn list_width(width: u16) -> u16 {
    width * 3 / 10
}

/// The preview box: everything right of the list.
pub fn preview_area(area: Rect) -> Rect {
    let lw = list_width(area.width);
    Rect { x: lw, width: area.width - lw, ..area }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    draw_list(f, app, Rect { width: list_width(area.width), ..area });

    let (title, style) = if app.insert {
        (" INSERT · keys go to the agent · ctrl+q back to list ", Style::new().fg(Color::Green).bold())
    } else {
        (" PREVIEW · i to type ", Style::new().dim())
    };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style).title(title);
    let boxed = preview_area(area);
    let inner = block.inner(boxed);
    f.render_widget(block, boxed);

    let Some(p) = app.preview.as_ref().filter(|p| Some(&p.pane) == app.selected().map(|a| &a.pane_id)) else {
        return;
    };
    f.render_widget(Paragraph::new(p.text.clone()), inner);
    if let Some((x, y)) = p.cursor.filter(|_| app.insert)
        && x < inner.width
        && y < inner.height
    {
        f.set_cursor_position((inner.x + x, inner.y + y));
    }
    if app.help {
        draw_help(f, area);
    }
}

/// Centered box listing every key; any key closes it.
fn draw_help(f: &mut Frame, area: Rect) {
    let text: Vec<String> = KEYS.iter().map(|(k, what)| format!(" {k:<9} {what}")).collect();
    let width = (text.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16 + 3).min(area.width);
    let height = (text.len() as u16 + 2).min(area.height);
    let r = Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height };
    let block = Block::bordered().border_type(BorderType::Rounded).title(" keys · any key closes ");
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(text.join("\n")).block(block), r);
}

/// A header row per project, then 3 rows per agent: icon + title, branch, latest activity.
/// Each row is (text, style, agent index, icon color).
type Row = (String, Style, Option<usize>, Option<Color>);

fn list_rows(app: &App) -> Vec<Row> {
    let mut rows = Vec::new();
    for (n, a) in app.agents.iter().enumerate() {
        // An empty session is just its header, and selectable.
        if a.session_only {
            rows.push((format!(" {}", a.worktree), Style::new().bold(), Some(n), None));
            continue;
        }
        if n == 0 || app.agents[n - 1].project != a.project {
            rows.push((format!(" {}", a.project.as_deref().unwrap_or("other")), Style::new().bold(), None, None));
        }
        let title = app::clean_title(a.title.as_deref().unwrap_or_default());
        let (icon, color) = a.status().icon();
        rows.push((format!(" {icon} {title}"), Style::new(), Some(n), Some(color)));
        let branch = a.branch.as_deref().unwrap_or(&a.worktree);
        rows.push((format!("   \u{e0a0} {branch}"), Style::new().dim(), Some(n), None));
        // Spins only while the agent works; otherwise a blank keeps the text lined up.
        let icon = if a.status() == app::Status::Working { spinner() } else { ' ' };
        let activity = a.activity.as_deref().map(|t| format!("   {icon} {t}")).unwrap_or_default();
        rows.push((activity, Style::new().dim(), Some(n), None));
    }
    rows
}

/// First list row shown, scrolled so all of the selection's rows stay above the last row.
fn list_offset(rows: &[Row], sel: usize, height: u16) -> usize {
    let sel_row = rows.iter().position(|r| r.2 == Some(sel)).unwrap_or(0);
    (sel_row + 4).saturating_sub(height as usize)
}

/// The agent drawn at screen row `y` of the list, if any.
pub fn agent_at(app: &App, area: Rect, y: u16) -> Option<usize> {
    let rows = list_rows(app);
    let off = list_offset(&rows, app.sel, area.height);
    rows.get(off + y.checked_sub(area.y)? as usize)?.2
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let rows = list_rows(app);
    let h = area.height as usize;
    let off = list_offset(&rows, app.sel, area.height);
    for r in 0..h {
        let (text, style) = match rows.get(off + r) {
            Some((_, _, Some(n), _)) if *n == app.sel => {
                (rows[off + r].0.clone(), Style::new().add_modifier(Modifier::REVERSED))
            }
            Some((text, style, _, _)) => (text.clone(), *style),
            None if r == h - 1 => (HINTS.into(), Style::new().dim()),
            None => continue,
        };
        let row = Rect { y: area.y + r as u16, height: 1, ..area };
        f.render_widget(Paragraph::new(shorten(&text, area.width as usize)).style(style), row);
        // Not on the selected row: reversed, the color would fill the icon's cell instead.
        if let Some((_, _, n, Some(color))) = rows.get(off + r)
            && *n != Some(app.sel)
            && area.width > 1
        {
            f.buffer_mut()[(area.x + 1, row.y)].set_fg(*color);
        }
    }
}

/// Cut to `width` columns, ending in "…" when cut.
fn shorten(s: &str, width: usize) -> String {
    let w = |c: char| c.width().unwrap_or(0);
    if s.chars().map(w).sum::<usize>() <= width {
        return s.into();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        if used + w(c) + 1 > width {
            break;
        }
        used += w(c);
        out.push(c);
    }
    if width > 0 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn shorten() {
        assert_eq!(super::shorten("abc", 3), "abc");
        assert_eq!(super::shorten("abcd", 3), "ab…");
        assert_eq!(super::shorten(" 🤖 x", 3), " …");
        assert_eq!(super::shorten("abc", 0), "");
    }

    #[test]
    fn list_rows() {
        let agent = crate::app::Agent {
            status: Some("working".into()),
            project: Some("p".into()),
            worktree: "wt".into(),
            branch: Some("feat".into()),
            title: Some("✳ Fix login".into()),
            ..Default::default()
        };
        // No branch from workmux: falls back to the worktree name.
        let other = crate::app::Agent {
            status: Some("done".into()),
            activity: Some("Now the docs.".into()),
            project: Some("q".into()),
            worktree: "wt2".into(),
            title: Some("Docs".into()),
            ..Default::default()
        };
        let app = crate::app::App { agents: vec![agent.clone(), other], ..Default::default() };
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 9)).unwrap();
        term.draw(|f| super::draw_list(f, &app, f.area())).unwrap();
        let buf = term.backend().buffer();
        let row = |y| (0..20).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(row(0).starts_with(" p "), "{:?}", row(0));
        assert!(row(1).starts_with(" \u{f06a9} Fix login"), "{:?}", row(1));
        // Agent 0 is selected, so its icon is plain; agent 1's is colored.
        assert_eq!(buf[(1, 1)].fg, ratatui::style::Color::Reset);
        assert_eq!(buf[(1, 5)].fg, ratatui::style::Color::Green);
        assert!(row(2).starts_with("   \u{e0a0} feat"), "{:?}", row(2));
        // No activity yet: an empty row, so every agent takes the same height.
        assert_eq!(row(3).trim(), "");
        // The next group's header follows right away.
        assert!(row(4).starts_with(" q "), "{:?}", row(4));
        assert!(row(6).starts_with("   \u{e0a0} wt2"), "{:?}", row(6));
        // Done, so no spinner.
        assert!(row(7).starts_with("     Now the docs."), "{:?}", row(7));
        assert_eq!(buf[(5, 7)].modifier, buf[(5, 6)].modifier); // styled like the branch

        let area = ratatui::layout::Rect::new(0, 0, 20, 9);
        assert_eq!(super::agent_at(&app, area, 0), None);
        assert_eq!(super::agent_at(&app, area, 3), Some(0));
        assert_eq!(super::agent_at(&app, area, 7), Some(1));
        assert_eq!(super::agent_at(&app, area, 8), None);

        // Working, so the activity row starts with a spinner frame.
        let busy = crate::app::Agent { activity: Some("Running tests.".into()), ..agent };
        let app = crate::app::App { agents: vec![busy], ..Default::default() };
        let rows = super::list_rows(&app);
        let frame = rows[3].0.chars().nth(3).unwrap();
        assert!(super::SPINNER.contains(&frame), "{:?}", rows[3].0);
        assert!(rows[3].0.ends_with(" Running tests."));

        // An empty session: one header row that belongs to its entry, so it can be selected.
        let empty = crate::app::Agent { worktree: "dots".into(), session_only: true, ..Default::default() };
        let app = crate::app::App { agents: vec![empty], ..Default::default() };
        let rows = super::list_rows(&app);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0.as_str(), rows[0].2), (" dots", Some(0)));
    }
}
