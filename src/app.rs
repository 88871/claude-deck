use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crate::pty::{self, PtySession};
use crate::session::{SessionManager, SessionState};
use crate::{keys, ui, Tui};

pub enum AppEvent {
    Input(Event),
    Output,
    Exited { clean: bool },
}

pub struct App {
    pub should_quit: bool,
    pub manager: SessionManager,
    pub session: Option<(String, PtySession)>, // (id, live pty) — one for now
    pub leader: bool,                            // Ctrl-a pressed, awaiting command
    pub claude_path: Option<String>,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            should_quit: false,
            manager: SessionManager::default(),
            session: None,
            leader: false,
            claude_path: pty::resolve_claude_path(),
            tx,
            rx,
        }
    }

    pub fn run(&mut self, terminal: &mut Tui) -> io::Result<()> {
        // Input thread: forward crossterm events into the unified channel.
        let input_tx = self.tx.clone();
        std::thread::spawn(move || loop {
            if event::poll(Duration::from_millis(200)).unwrap_or(false) {
                if let Ok(ev) = event::read() {
                    if input_tx.send(AppEvent::Input(ev)).is_err() { break; }
                }
            }
        });

        // Start one session on launch (in the current dir) if claude is present.
        let size = terminal.size()?;
        let (rows, cols) = pane_dims(size.width, size.height);
        self.start_session(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")), rows, cols);

        terminal.draw(|f| ui::draw(f, self))?;
        while !self.should_quit {
            match self.rx.recv() {
                Ok(AppEvent::Input(Event::Key(k))) if k.kind == KeyEventKind::Press => self.on_key(k),
                Ok(AppEvent::Input(Event::Resize(w, h))) => self.on_resize(w, h),
                Ok(AppEvent::Input(_)) => {}
                Ok(AppEvent::Output) => {}
                Ok(AppEvent::Exited { clean }) => {
                    if let Some((id, _)) = &self.session {
                        let id = id.clone();
                        self.manager.set_state(&id, if clean { SessionState::Closed } else { SessionState::Error });
                    }
                }
                Err(_) => break,
            }
            terminal.draw(|f| ui::draw(f, self))?;
        }
        Ok(())
    }

    fn start_session(&mut self, cwd: PathBuf, rows: u16, cols: u16) {
        let Some(path) = self.claude_path.clone() else { return };
        let id = self.manager.create(cwd.clone());
        match pty::spawn(&path, &cwd, rows, cols, self.tx.clone()) {
            Ok(pty) => {
                self.manager.set_state(&id, SessionState::Running);
                self.session = Some((id, pty));
            }
            Err(_) => { self.manager.set_state(&id, SessionState::Error); }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        // Leader: Ctrl-a starts a command sequence.
        if self.leader {
            self.leader = false;
            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.leader = true;
            return;
        }
        // Otherwise forward to the focused session.
        if let Some((_, pty)) = &mut self.session {
            if let Some(bytes) = keys::encode(&key) {
                let _ = pty.writer.write_all(&bytes);
                let _ = pty.writer.flush();
            }
        }
    }

    fn on_resize(&mut self, w: u16, h: u16) {
        let (rows, cols) = pane_dims(w, h);
        if let Some((_, pty)) = &self.session {
            let _ = pty.master.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            pty.parser.lock().unwrap().set_size(rows, cols);
        }
    }
}

/// Interior size of the main pane given the full terminal size: subtract the
/// 26-wide sidebar and 1-cell borders on each side.
pub fn pane_dims(term_w: u16, term_h: u16) -> (u16, u16) {
    let cols = term_w.saturating_sub(26).saturating_sub(2).max(1);
    let rows = term_h.saturating_sub(2).max(1);
    (rows, cols)
}
