use crate::app::{self, App};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthChar;

const HINTS: &str = " ? keys · q quit";

const KEYS: &[(&str, &str)] = &[
    ("j/k ↑/↓", "move the selection"),
    ("i", "insert mode: keys go to the agent"),
    ("ctrl+q", "back to the list from insert mode"),
    ("enter", "jump to the agent's pane and quit"),
    ("l", "lazygit in the agent's workdir"),
    ("v", "nvim in the agent's workdir"),
    ("g", "open the branch's PR on GitHub"),
    ("r", "remove the agent and its worktree"),
    ("?", "show this help"),
    ("q ctrl+c", "quit"),
];

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

    let Some(p) = app.preview.as_ref().filter(|p| Some(&p.pane) == app.selected().map(|a| &a.pane_id))
    else {
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

/// A header row per project, then 2 rows per agent: icon + title, then branch.
/// Scroll so the selection stays visible.
fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    // (text, style, agent index)
    let mut rows: Vec<(String, Style, Option<usize>)> = Vec::new();
    for (n, a) in app.agents.iter().enumerate() {
        if n == 0 || app.agents[n - 1].project != a.project {
            if n > 0 {
                rows.push((String::new(), Style::new(), None));
            }
            rows.push((format!(" {}", a.project.as_deref().unwrap_or("other")), Style::new().bold(), None));
        }
        let title = app::clean_title(a.title.as_deref().unwrap_or_default());
        rows.push((format!(" {} {title}", a.status().icon()), Style::new(), Some(n)));
        rows.push((format!("    \u{e0a0} {}", a.worktree), Style::new().dim(), Some(n)));
    }
    let h = area.height as usize;
    let sel_row = rows.iter().position(|r| r.2 == Some(app.sel)).unwrap_or(0);
    // Keep both of the selection's rows above the last row.
    let off = (sel_row + 3).saturating_sub(h);
    for r in 0..h {
        let (text, style) = match rows.get(off + r) {
            Some((_, _, Some(n))) if *n == app.sel => (rows[off + r].0.clone(), Style::new().add_modifier(Modifier::REVERSED)),
            Some((text, style, _)) => (text.clone(), *style),
            None if r == h - 1 => (HINTS.into(), Style::new().dim()),
            None => continue,
        };
        let row = Rect { y: area.y + r as u16, height: 1, ..area };
        f.render_widget(Paragraph::new(shorten(&text, area.width as usize)).style(style), row);
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
            worktree: "feat".into(),
            title: Some("✳ Fix login".into()),
            ..Default::default()
        };
        let app = crate::app::App { agents: vec![agent], ..Default::default() };
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 4)).unwrap();
        term.draw(|f| super::draw_list(f, &app, f.area())).unwrap();
        let buf = term.backend().buffer();
        let row = |y| (0..20).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(row(0).starts_with(" p "), "{:?}", row(0));
        // The emoji is 2 cells wide, so its second cell reads as an extra space.
        assert!(row(1).starts_with(" 🤖  Fix login"), "{:?}", row(1));
        assert!(row(2).starts_with("    \u{e0a0} feat"), "{:?}", row(2));
    }
}
