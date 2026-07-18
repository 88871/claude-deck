use std::io;
use std::io::Write;
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
    Exited { id: String, clean: bool },
}

pub struct App {
    pub should_quit: bool,
    pub manager: SessionManager,
    /// All active sessions in creation order: (id, live pty).
    pub sessions: Vec<(String, PtySession)>,
    /// Index into `sessions` for the currently focused session.
    pub focused: usize,
    /// When Some, we're in path-input mode (new-session prompt).
    pub input: Option<String>,
    pub leader: bool, // Ctrl-a pressed, awaiting command
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
            sessions: Vec::new(),
            focused: 0,
            input: None,
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
                Ok(AppEvent::Exited { id, clean }) => {
                    self.manager.set_state(&id, if clean { SessionState::Closed } else { SessionState::Error });
                    // Do NOT remove the session — leave it visible as Closed/Error
                    // until the user explicitly kills it with Ctrl-a x.
                }
                Err(_) => break,
            }
            terminal.draw(|f| ui::draw(f, self))?;
        }
        Ok(())
    }

    /// Spawn a new session in `cwd` and push it onto `sessions`.
    fn start_session(&mut self, cwd: PathBuf, rows: u16, cols: u16) {
        let Some(path) = self.claude_path.clone() else { return };
        let id = self.manager.create(cwd.clone());
        match pty::spawn(&path, &cwd, rows, cols, id.clone(), self.tx.clone()) {
            Ok(pty) => {
                self.manager.set_state(&id, SessionState::Running);
                self.focused = self.sessions.len(); // new session becomes focused
                self.sessions.push((id, pty));
            }
            Err(_) => {
                self.manager.set_state(&id, SessionState::Error);
                // Push a tombstone entry so the user can see the failure in the sidebar.
                // We use a dummy PtySession — but we can't create one without a real PTY,
                // so we simply leave the manager entry as Error and don't push to sessions.
                // The user can start another session manually.
            }
        }
    }

    /// Returns the id of the currently focused session, if any.
    pub fn focused_id(&self) -> Option<String> {
        self.sessions.get(self.focused).map(|(id, _)| id.clone())
    }

    /// Kill the focused session: signal the process, remove from manager + vec, clamp focused.
    fn kill_focused(&mut self) {
        if self.sessions.is_empty() { return; }
        let (id, mut pty) = self.sessions.remove(self.focused);
        // Signal the child to exit.
        let _ = pty.killer.kill();
        // Remove from the session manager.
        self.manager.remove(&id);
        // Clamp focused so it's still a valid index (or 0 if the list is empty).
        if !self.sessions.is_empty() {
            self.focused = self.focused.min(self.sessions.len() - 1);
        } else {
            self.focused = 0;
        }
    }

    /// Resize the focused session's master PTY and vt100 parser to match the
    /// current pane dimensions. Call this after switching focus.
    fn sync_focus_size(&mut self) {
        // We need to know the terminal size. We don't store it, so we query it.
        // crossterm::terminal::size() returns the full terminal size.
        if let Ok((term_w, term_h)) = crossterm::terminal::size() {
            let (rows, cols) = pane_dims(term_w, term_h);
            if let Some((_, pty)) = self.sessions.get(self.focused) {
                let _ = pty.master.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                pty.parser.lock().unwrap().set_size(rows, cols);
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        // Path-input mode: edit the buffer, do NOT forward to any PTY.
        if self.input.is_some() {
            self.on_input_key(key);
            return;
        }

        // Leader: Ctrl-a starts a command sequence.
        if self.leader {
            self.leader = false;
            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('n') => {
                    self.input = Some(
                        std::env::current_dir()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    );
                }
                KeyCode::Char('x') => self.kill_focused(),
                KeyCode::Char(c @ '1'..='9') => {
                    let i = (c as u8 - b'1') as usize;
                    if i < self.sessions.len() {
                        self.focused = i;
                        self.sync_focus_size();
                    }
                }
                KeyCode::Char('[') => {
                    if !self.sessions.is_empty() {
                        self.focused = (self.focused + self.sessions.len() - 1) % self.sessions.len();
                        self.sync_focus_size();
                    }
                }
                KeyCode::Char(']') => {
                    if !self.sessions.is_empty() {
                        self.focused = (self.focused + 1) % self.sessions.len();
                        self.sync_focus_size();
                    }
                }
                _ => {}
            }
            return;
        }

        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.leader = true;
            return;
        }

        // Otherwise forward to the focused session.
        if let Some((_, pty)) = self.sessions.get_mut(self.focused) {
            if let Some(bytes) = keys::encode(&key) {
                let _ = pty.writer.write_all(&bytes);
                let _ = pty.writer.flush();
            }
        }
    }

    /// Handle a key while in path-input mode.
    fn on_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.input = None;
            }
            KeyCode::Enter => {
                if let Some(buf) = self.input.take() {
                    let path = PathBuf::from(&buf);
                    if path.is_dir() {
                        if let Ok((term_w, term_h)) = crossterm::terminal::size() {
                            let (rows, cols) = pane_dims(term_w, term_h);
                            self.start_session(path, rows, cols);
                        }
                    }
                    // If the path is not a valid directory, close the prompt
                    // without creating a session (silently cancel).
                }
            }
            KeyCode::Backspace => {
                if let Some(buf) = self.input.as_mut() {
                    buf.pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(buf) = self.input.as_mut() {
                    buf.push(c);
                }
            }
            _ => {}
        }
    }

    fn on_resize(&mut self, w: u16, h: u16) {
        let (rows, cols) = pane_dims(w, h);
        if let Some((_, pty)) = self.sessions.get(self.focused) {
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
