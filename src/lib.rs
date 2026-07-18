pub mod app;
pub mod home;
pub mod hooks;
pub mod icons;
pub mod keys;
pub mod mem;
pub mod mouse;
pub mod notify;
pub mod pty;
pub mod session;
pub mod ui;

use std::io::{self, Stdout};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use app::App;

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn init_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

/// Entry point shared by the `claude-deck` and `cdeck` binaries.
/// Sets up raw-mode terminal, runs the app, and restores the terminal on exit
/// or panic.
pub fn run() -> io::Result<()> {
    // `__hook` subcommand: read stdin, forward to socket, exit — no TUI.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("__hook") {
        let sock = args.get(2).cloned().unwrap_or_default();
        return hooks::forward(&sock);
    }

    // Restore the terminal even on panic, so a crash never corrupts the shell.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));

    let mut terminal = init_terminal()?;
    let result = App::new().run(&mut terminal);
    restore_terminal()?;
    result
}
