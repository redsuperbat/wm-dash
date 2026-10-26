use crate::app::{self, App};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use std::time::{Duration, SystemTime};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const KEYS: &[(&str, &str)] = &[
    ("j/k ↑/↓", "move the selection (or click an agent)"),
    ("ctrl+u/d", "scroll the preview a page"),
    ("ctrl+y/e", "scroll the preview a little (or the mouse wheel)"),
    ("tab", "next agent that needs you (shift+tab: previous)"),
    ("/", "filter by name, branch or title (enter keeps)"),
    ("esc", "clear the filter"),
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
    let l = &crate::config::get().layout_ratio;
    (width as u32 * l.list as u32 / (l.list as u32 + l.preview as u32)) as u16
}

/// The agent rows: the list column but for its last row, which holds the mode and key hints.
fn list_area(area: Rect) -> Rect {
    Rect { width: list_width(area.width), height: area.height.saturating_sub(1), ..area }
}

/// Where the preview's text goes: right of the list and the divider line.
pub fn preview_area(area: Rect) -> Rect {
    let x = area.x + list_width(area.width) + 1;
    Rect { x, width: (area.x + area.width).saturating_sub(x), ..area }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let list = list_area(area);
    draw_list(f, app, list);

    let (footer, style) = if app.insert {
        (" INSERT · ctrl+q back to list".into(), Style::new().fg(Color::Green).bold())
    } else if app.filtering {
        (format!(" /{}", app.filter), Style::new().fg(Color::Yellow))
    } else if let Some(e) = &app.error {
        (format!(" workmux failed: {e}"), Style::new().fg(Color::Red))
    } else if !app.filter.is_empty() {
        (format!(" /{} · esc clears", app.filter), Style::new().fg(Color::Yellow).dim())
    } else {
        (" i to type · / filter · ? keys".into(), Style::new().dim())
    };
    let footer_row = Rect { y: list.y + list.height, height: 1, ..list };
    f.render_widget(Paragraph::new(shorten(&footer, list.width as usize)).style(style), footer_row);
    if app.filtering {
        let x = (footer.width() as u16).min(footer_row.width.saturating_sub(1));
        f.set_cursor_position((footer_row.x + x, footer_row.y));
    }
    let inner = preview_area(area);
    let divider = Rect { x: inner.x - 1, width: 1, ..area };
    f.render_widget(Block::new().borders(Borders::LEFT).border_style(style), divider);

    let Some(p) = app.preview.as_ref().filter(|p| Some(&p.pane) == app.selected().map(|a| &a.pane_id)) else {
        return;
    };
    f.render_widget(&p.text, inner);
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
/// Each row is (text, style, agent index, status color, right-aligned text). The status color
/// tints the icon, the right text and the selection bar.
pub type Row = (String, Style, Option<usize>, Option<Color>, String);

pub fn list_rows(app: &App) -> Vec<Row> {
    let mut rows = Vec::new();
    for (n, a) in app.agents.iter().enumerate() {
        // An empty session is just its header, and selectable.
        if a.session_only {
            rows.push((format!(" {}", a.worktree), Style::new().bold(), Some(n), None, String::new()));
            continue;
        }
        if n == 0 || app.agents[n - 1].project != a.project {
            let project = format!(" {}", a.project.as_deref().unwrap_or("other"));
            rows.push((project, Style::new().bold(), None, None, String::new()));
        }
        let title = app::clean_title(a.title.as_deref().unwrap_or_default());
        let status = a.status();
        let (icon, color) = status.icon();
        rows.push((format!(" {icon} {title}"), Style::new(), Some(n), Some(color), status.label().into()));
        let branch = a.branch.as_deref().unwrap_or(&a.worktree);
        let since = a.since.map(|t| ago(t.elapsed().as_secs())).unwrap_or_default();
        rows.push((format!("   \u{e0a0} {branch}"), Style::new().dim(), Some(n), Some(color), since));
        // Spins only while the agent works; otherwise a blank keeps the text lined up.
        let icon = if status == app::Status::Working { spinner() } else { ' ' };
        let activity = a.activity.as_deref().map(|t| format!("   {icon} {t}")).unwrap_or_default();
        rows.push((activity, Style::new().dim(), Some(n), Some(color), String::new()));
    }
    rows
}

/// "42s", "5m", "3h", "2d".
fn ago(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// First list row shown, scrolled so all of the selection's rows stay above the last row.
fn list_offset(rows: &[Row], sel: usize, height: u16) -> usize {
    let sel_row = rows.iter().position(|r| r.2 == Some(sel)).unwrap_or(0);
    (sel_row + 4).saturating_sub(height as usize)
}

/// The agent drawn at screen row `y` of the list, if any.
pub fn agent_at(app: &App, area: Rect, y: u16) -> Option<usize> {
    let area = list_area(area);
    let rows = list_rows(app);
    let off = list_offset(&rows, app.sel, area.height);
    rows.get(off + y.checked_sub(area.y)? as usize)?.2
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    if app.agents.is_empty() {
        let msg = if !app.filter.is_empty() {
            format!(" no agents match /{}", app.filter)
        } else if app.error.is_some() {
            " could not list agents".into()
        } else if !app.loaded {
            " loading…".into()
        } else {
            " no agents or sessions".into()
        };
        f.render_widget(Paragraph::new(shorten(&msg, area.width as usize)).style(Style::new().dim()), area);
        return;
    }
    let rows = list_rows(app);
    let h = area.height as usize;
    let off = list_offset(&rows, app.sel, area.height);
    for r in 0..h {
        let row = Rect { y: area.y + r as u16, height: 1, ..area };
        let Some((text, style, n, color, right)) = rows.get(off + r) else { continue };
        let selected = *n == Some(app.sel);
        if selected {
            f.render_widget(Block::new().style(Style::new().bg(SELECTED_BG)), row);
        }
        // Keep a gap between the text and the right column; drop the right text when it won't fit.
        let rw = right.chars().count() as u16;
        let rw = if rw > 0 && rw + 10 <= area.width { rw + 2 } else { 0 };
        let left = Rect { width: area.width - rw, ..row };
        f.render_widget(Paragraph::new(shorten(text, left.width as usize)).style(*style), left);
        let Some(color) = *color else { continue };
        // The title row's icon and label take the status color; the rows under it stay dim.
        let title = !style.add_modifier.contains(Modifier::DIM);
        if rw > 0 {
            let r = Rect { x: area.x + area.width - rw, width: rw, ..row };
            let st = if title { Style::new().fg(color).bold() } else { *style };
            f.render_widget(Paragraph::new(format!(" {right} ")).style(st), r);
        }
        let buf = f.buffer_mut();
        if title && area.width > 1 {
            buf[(area.x + 1, row.y)].set_fg(color);
        }
        if selected {
            buf[(area.x, row.y)].set_symbol("▌").set_fg(color);
        }
    }
}

/// Background of the selected agent's rows: faint, so the status colors stay readable.
const SELECTED_BG: Color = Color::Indexed(236);

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
    fn divider_and_footer() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 6)).unwrap();
        for insert in [false, true] {
            let app = crate::app::App { insert, ..Default::default() };
            term.draw(|f| super::draw(f, &app)).unwrap();
            let buf = term.backend().buffer();
            // The list takes 30% (12 columns), then one column of divider, then the preview.
            assert!((0..6).all(|y| buf[(12, y)].symbol() == "│"));
            assert_eq!(super::preview_area(buf.area), ratatui::layout::Rect::new(13, 0, 27, 6));
            let footer: String = (0..12).map(|x| buf[(x, 5)].symbol()).collect();
            let want = if insert { " INSERT · c…" } else { " i to type …" };
            assert_eq!(footer, want);
            let green = buf[(12, 0)].fg == ratatui::style::Color::Green;
            assert_eq!(green, insert, "the divider turns green in insert mode");
        }
    }

    #[test]
    fn filter_footer() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 4)).unwrap();
        let footer = |term: &ratatui::Terminal<ratatui::backend::TestBackend>| {
            (0..18).map(|x| term.backend().buffer()[(x, 3)].symbol()).collect::<String>()
        };
        // Typing: the filter, with the cursor right after it.
        let app = crate::app::App { filter: "api".into(), filtering: true, ..Default::default() };
        term.draw(|f| super::draw(f, &app)).unwrap();
        assert_eq!(footer(&term), " /api             ");
        term.backend_mut().assert_cursor_position((5, 3));
        // Kept: the filter and how to clear it.
        let app = crate::app::App { filter: "api".into(), ..Default::default() };
        term.draw(|f| super::draw(f, &app)).unwrap();
        assert_eq!(footer(&term), " /api · esc clears");
    }

    #[test]
    fn empty_and_error() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 4)).unwrap();
        let mut rows = |app: &crate::app::App| {
            term.draw(|f| super::draw(f, app)).unwrap();
            let buf = term.backend().buffer();
            let row = |y| (0..18).map(|x| buf[(x, y)].symbol()).collect::<String>();
            (row(0).trim_end().to_string(), row(3).trim_end().to_string(), buf[(1, 3)].fg)
        };
        assert_eq!(rows(&crate::app::App::default()).0, " loading…");
        let app = crate::app::App { loaded: true, ..Default::default() };
        assert_eq!(rows(&app).0, " no agents or ses…");
        let app = crate::app::App { loaded: true, filter: "x".into(), ..Default::default() };
        assert_eq!(rows(&app).0, " no agents match …");
        // A failed poll: said in the list and, in red, in the footer.
        let app = crate::app::App { error: Some("boom".into()), ..Default::default() };
        let (list, footer, fg) = rows(&app);
        assert_eq!((list.as_str(), footer.as_str()), (" could not list a…", " workmux failed: …"));
        assert_eq!(fg, ratatui::style::Color::Red);
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
        let since = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(300));
        let agent = crate::app::Agent { since, ..agent };
        let app = crate::app::App { agents: vec![agent.clone(), other], ..Default::default() };
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 9)).unwrap();
        term.draw(|f| super::draw_list(f, &app, f.area())).unwrap();
        let buf = term.backend().buffer();
        let row = |y| (0..30).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(row(0).starts_with(" p "), "{:?}", row(0));
        // Agent 0 is selected: a bar in its status color, then its status and age on the right.
        assert!(row(1).starts_with("▌\u{f06a9} Fix login  ") && row(1).ends_with(" WORKING "), "{:?}", row(1));
        assert_eq!(buf[(0, 1)].fg, ratatui::style::Color::Magenta);
        assert_eq!(buf[(0, 1)].bg, super::SELECTED_BG);
        assert_eq!(buf[(22, 1)].fg, ratatui::style::Color::Magenta);
        // Icons keep their color, selected or not.
        assert_eq!(buf[(1, 1)].fg, ratatui::style::Color::Magenta);
        assert_eq!(buf[(1, 5)].fg, ratatui::style::Color::Green);
        assert!(row(5).ends_with(" DONE "), "{:?}", row(5));
        assert_eq!(buf[(0, 5)].symbol(), " ");
        assert!(row(2).starts_with("▌  \u{e0a0} feat  ") && row(2).ends_with(" 5m "), "{:?}", row(2));
        // No activity yet: an empty row (but for the bar), so every agent takes the same height.
        assert_eq!(row(3).trim(), "▌");
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
