use crate::app::{self, App};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::UnicodeWidthChar;

const HINTS: &str = " j/k · i insert · enter jump · l lazygit · v nvim · r remove · q quit";

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
}

/// Each agent takes 2 rows: icon + name, then title. Scroll so the selection stays visible.
fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let h = area.height as usize;
    let off = (app.sel + 1).saturating_sub(h.saturating_sub(1) / 2);
    for r in 0..h {
        let n = off + r / 2;
        let (text, style) = match app.agents.get(n) {
            Some(a) if r % 2 == 0 => (format!(" {} {}", a.status().icon(), a.name()), Style::new()),
            Some(a) => {
                let title = app::clean_title(a.title.as_deref().unwrap_or_default());
                (format!("    {title}"), Style::new().dim())
            }
            None if r == h - 1 => (HINTS.into(), Style::new().dim()),
            None => continue,
        };
        let style = if n == app.sel && n < app.agents.len() { Style::new().add_modifier(Modifier::REVERSED) } else { style };
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
}
