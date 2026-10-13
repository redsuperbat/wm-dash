//! Workmux agent dashboard: agent list on the left, live preview of the selected agent on the right.

mod app;
mod tmux;
mod ui;

use anyhow::Result;
use app::{Agent, App, Preview};
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{BufRead, Stdout, Write, stdout};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::Duration;
use tmux::PreviewReq;

/// Everything the main loop reacts to. Workers send these; the loop blocks until one arrives.
pub enum Msg {
    Term(Event),
    Agents(Vec<Agent>),
    Preview(Preview),
}

/// What a key press asks the main loop to do besides updating state.
enum Action {
    Quit,
    Jump,
    Lazygit,
    Nvim,
    Remove,
}

fn tui_on() -> Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    Ok(())
}

fn tui_off() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), DisableBracketedPaste, crossterm::cursor::Show, LeaveAlternateScreen);
}

/// Undo the fit and give the terminal back. Safe to call more than once.
fn restore() {
    tmux::set_fitting(false);
    tui_off();
}

fn main() -> Result<()> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        prev(info);
        std::process::exit(101);
    }));
    // The popup closing sends SIGHUP; restore the tmux layout before we go.
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ])?;
    std::thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            restore();
            std::process::exit(128 + sig);
        }
    });

    tui_on()?;
    let res = run();
    restore();
    res
}

fn run() -> Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let (tx, rx) = mpsc::channel();
    let (refresh_tx, refresh_rx) = mpsc::channel();
    let (req_tx, req_rx) = mpsc::channel();
    let (pause_tx, pause_rx) = mpsc::sync_channel(0);
    std::thread::spawn({
        let tx = tx.clone();
        move || tmux::list_worker(tx, refresh_rx)
    });
    std::thread::spawn({
        let tx = tx.clone();
        move || tmux::preview_worker(tx, req_rx)
    });
    std::thread::spawn(move || input_worker(tx, pause_rx));

    let mut app = App::default();
    let mut req = None;
    terminal.draw(|f| ui::draw(f, &app))?;
    loop {
        let mut dirty = false;
        let mut action = None;
        // Block for the next message, then take whatever else is queued before drawing once.
        for msg in std::iter::once(rx.recv()?).chain(rx.try_iter()) {
            dirty = true;
            match msg {
                Msg::Agents(agents) => app.set_agents(agents),
                Msg::Preview(p) => app.preview = Some(p),
                Msg::Term(Event::Key(k)) if k.kind != KeyEventKind::Release => action = handle_key(&mut app, k),
                Msg::Term(Event::Paste(text)) if app.insert => {
                    if let Some(a) = app.selected() {
                        tmux::paste(&a.pane_id, &text);
                    }
                }
                Msg::Term(_) => {}
            }
            if action.is_some() {
                break;
            }
        }

        // Point the preview worker at the selected agent, sized to the preview box's inside.
        let size = terminal.size()?;
        let boxed = ui::preview_area(size.into());
        let new_req = app.selected().map(|a| PreviewReq {
            pane: a.pane_id.clone(),
            width: boxed.width.saturating_sub(2),
            height: boxed.height.saturating_sub(2),
        });
        if new_req != req {
            req = new_req;
            req_tx.send(req.clone())?;
        }

        let selected = app.selected().cloned();
        match (action, selected) {
            (Some(Action::Quit), _) => return Ok(()),
            (Some(Action::Jump), Some(a)) => {
                tmux::tmux(&["switch-client", "-t", &a.pane_id]);
                return Ok(());
            }
            (Some(action), Some(a)) => {
                suspend(&mut terminal, &pause_tx, || run_external(&action, &a))?;
                let _ = refresh_tx.send(());
            }
            _ => {}
        }
        if dirty {
            terminal.draw(|f| ui::draw(f, &app))?;
        }
    }
}

fn handle_key(app: &mut App, k: KeyEvent) -> Option<Action> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    if app.insert {
        if ctrl && k.code == KeyCode::Char('q') {
            app.insert = false;
        } else if let (Some(a), Some(key)) = (app.selected(), app::tmux_key(k)) {
            tmux::send_key(&a.pane_id, &key);
        }
        return None;
    }
    match k.code {
        KeyCode::Char('c') if ctrl => return Some(Action::Quit),
        _ if ctrl => {}
        KeyCode::Char('j') | KeyCode::Down => app.move_sel(true),
        KeyCode::Char('k') | KeyCode::Up => app.move_sel(false),
        KeyCode::Char('i') => app.insert = app.selected().is_some(),
        KeyCode::Enter => return Some(Action::Jump),
        KeyCode::Char('l') => return Some(Action::Lazygit),
        KeyCode::Char('v') => return Some(Action::Nvim),
        KeyCode::Char('r') => return Some(Action::Remove),
        KeyCode::Char('q') => return Some(Action::Quit),
        _ => {}
    }
    None
}

/// Read terminal events and forward them. A message on `pause` stops reading until the sender
/// it carries is dropped, so a suspended program (lazygit, nvim) gets the keys instead.
fn input_worker(tx: Sender<Msg>, pause: Receiver<Receiver<()>>) {
    loop {
        // The timeout only bounds how long a pause request waits; input wakes it right away.
        let ready = event::poll(Duration::from_millis(50));
        if let Ok(resume) = pause.try_recv() {
            let _ = resume.recv();
            continue;
        }
        match ready.and_then(|r| if r { event::read().map(Some) } else { Ok(None) }) {
            Ok(Some(ev)) => {
                if tx.send(Msg::Term(ev)).is_err() {
                    return;
                }
            }
            Ok(None) => {}
            Err(_) => return,
        }
    }
}

/// Give the terminal to `f`, then take it back with a full redraw.
fn suspend(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    pause: &SyncSender<Receiver<()>>,
    f: impl FnOnce(),
) -> Result<()> {
    let (resume, paused) = mpsc::channel();
    // A rendezvous channel: this returns once the input worker has stopped reading.
    pause.send(paused).map_err(|_| anyhow::anyhow!("input worker stopped"))?;
    restore();
    f();
    tui_on()?;
    terminal.clear()?;
    tmux::set_fitting(true);
    drop(resume);
    Ok(())
}

fn run_external(action: &Action, a: &Agent) {
    let Some(workdir) = a.workdir.as_deref() else { return };
    match action {
        Action::Lazygit => {
            let _ = Command::new("lazygit").args(["-p", workdir]).status();
        }
        Action::Nvim => {
            let _ = Command::new("nvim").arg(".").current_dir(workdir).status();
        }
        Action::Remove if a.is_main_checkout() => {
            if ask(&format!("Close agent {} (main checkout, no worktree to remove)? [y/N] ", a.name())) == "y" {
                tmux::tmux(&["kill-pane", "-t", &a.pane_id]);
            }
        }
        Action::Remove => {
            // workmux asks for confirmation and warns about uncommitted changes itself.
            let ok = Command::new("workmux")
                .args(["remove", &a.worktree])
                .current_dir(a.project_path.as_deref().unwrap_or(workdir))
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                ask("workmux remove failed, press enter to go back");
            }
        }
        Action::Quit | Action::Jump => {}
    }
}

fn ask(prompt: &str) -> String {
    print!("{prompt}");
    let _ = stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim().to_string()
}
