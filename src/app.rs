use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use crate::icons::{self, IconMode};
use crate::pty::{self, PtySession};
use crate::session::{SessionManager, SessionState};
use crate::{keys, mouse, ui, Tui};

pub enum AppEvent {
    Input(Event),
    Output,
    Exited { id: String, clean: bool },
    Hook(crate::hooks::HookEvent),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Home,
    Session(usize),
}

// ── Pure focus-transition helpers (testable without a live PTY) ──────────────

/// Build the ordered list of visible "slots": Home (if visible) then each
/// session index.  Returns indices as `Focus` values.
pub fn visible_entries(home_visible: bool, session_count: usize) -> Vec<Focus> {
    let mut v = Vec::new();
    if home_visible {
        v.push(Focus::Home);
    }
    for i in 0..session_count {
        v.push(Focus::Session(i));
    }
    v
}

/// Compute the next focus when cycling forward (+1) or backward (-1).
/// Returns the current focus unchanged if there are no visible entries.
pub fn cycle_focus(focus: Focus, home_visible: bool, session_count: usize, delta: i32) -> Focus {
    let entries = visible_entries(home_visible, session_count);
    if entries.is_empty() {
        return focus;
    }
    let pos = entries.iter().position(|&e| e == focus).unwrap_or(0);
    let n = entries.len() as i32;
    let new_pos = ((pos as i32 + delta).rem_euclid(n)) as usize;
    entries[new_pos]
}

// ─────────────────────────────────────────────────────────────────────────────

/// Describes what kind of text prompt is currently active (if any).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// The user is typing a directory path for a new session.
    NewSession(String),
    /// The user is typing a new label for the focused session.
    Rename(String),
}

impl Prompt {
    /// Returns a mutable reference to the inner buffer.
    pub fn buf_mut(&mut self) -> &mut String {
        match self {
            Prompt::NewSession(s) | Prompt::Rename(s) => s,
        }
    }


}

pub struct App {
    pub should_quit: bool,
    pub manager: SessionManager,
    /// All active sessions in creation order: (id, live pty).
    pub sessions: Vec<(String, PtySession)>,
    /// Current focus: either Home or a session by index.
    pub focus: Focus,
    /// Whether the Home pane is shown in the sidebar.
    pub home_visible: bool,
    /// When Some, we're in a text-input prompt.
    pub prompt: Option<Prompt>,
    pub leader: bool, // Ctrl-a pressed, awaiting command
    pub claude_path: Option<String>,
    pub icons: IconMode,
    /// Whether to ring the terminal bell when a session needs attention.
    pub bell_on: bool,
    /// Whether to fire a macOS desktop notification when a session needs attention.
    pub notify_on: bool,
    /// Unix socket path used by the hook listener; removed on quit.
    socket_path: std::path::PathBuf,
    /// Temp settings file path written for `--settings`; removed on quit.
    settings_path: std::path::PathBuf,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let (socket_path, settings_path) = crate::hooks::paths();

        // Start the hook listener — the closure captures a cloned Sender so
        // hooks.rs stays free of the AppEvent type.
        let hook_tx = tx.clone();
        let _ = crate::hooks::listen(socket_path.clone(), move |ev| {
            let _ = hook_tx.send(AppEvent::Hook(ev));
        });

        // Write the shared hooks settings file so `--settings` sessions can
        // pick it up. Best-effort — if it fails, hooks just won't fire.
        let socket_str = socket_path.to_string_lossy().into_owned();
        let _ = crate::hooks::write_settings_file(&settings_path, &socket_str);

        // Parse --no-bell / --no-notify flags from this process's args.
        // This is the TUI path only — __hook is handled before App::new().
        let args: Vec<String> = std::env::args().collect();
        let bell_on = !args.contains(&"--no-bell".to_string());
        let notify_on = !args.contains(&"--no-notify".to_string());

        Self {
            should_quit: false,
            manager: SessionManager::default(),
            sessions: Vec::new(),
            focus: Focus::Home,
            home_visible: true,
            prompt: None,
            leader: false,
            claude_path: pty::resolve_claude_path(),
            icons: icons::detect_mode(),
            bell_on,
            notify_on,
            socket_path,
            settings_path,
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

        // Launch on Home — do NOT auto-start a session.
        terminal.draw(|f| ui::draw(f, self))?;
        while !self.should_quit {
            match self.rx.recv() {
                Ok(AppEvent::Input(Event::Key(k))) if k.kind == KeyEventKind::Press => self.on_key(k),
                Ok(AppEvent::Input(Event::Resize(w, h))) => self.on_resize(w, h),
                Ok(AppEvent::Input(Event::Mouse(m))) => self.on_mouse(m),
                Ok(AppEvent::Input(_)) => {}
                Ok(AppEvent::Output) => {}
                Ok(AppEvent::Exited { id, clean }) => {
                    self.manager.set_state(&id, if clean { SessionState::Closed } else { SessionState::Error });
                    // Leave exited sessions visible; only quit when nothing live remains
                    // AND we're not on Home.
                    let any_live = self.sessions.iter().any(|(sid, _)| matches!(
                        self.manager.get(sid).map(|s| s.state),
                        Some(SessionState::Running) | Some(SessionState::Starting)
                    ));
                    if !any_live && !self.home_visible && self.sessions.is_empty() {
                        self.should_quit = true;
                    }
                }
                Ok(AppEvent::Hook(ev)) => {
                    if let Some(new_state) = state_for_hook(&ev) {
                        // Capture the OLD state before updating.
                        let old_state = self.manager.get(&ev.session_id).map(|s| s.state);
                        self.manager.set_state(&ev.session_id, new_state);

                        // Fire attention signals on the WaitingOnYou transition edge,
                        // but only when this session is NOT the currently focused one.
                        if new_state == SessionState::WaitingOnYou
                            && old_state != Some(SessionState::WaitingOnYou)
                        {
                            let is_focused = self.sessions.iter().enumerate()
                                .find(|(_, (id, _))| id == &ev.session_id)
                                .map(|(i, _)| self.focus == Focus::Session(i))
                                .unwrap_or(false);

                            if !is_focused {
                                // Resolve the sidebar label for the notification.
                                let label = self.manager.get(&ev.session_id)
                                    .map(|s| s.label.clone())
                                    .unwrap_or_else(|| ev.session_id.clone());
                                if self.bell_on {
                                    crate::notify::bell();
                                }
                                if self.notify_on {
                                    crate::notify::desktop(&label);
                                }
                            }
                        }
                    }
                }
                Err(_) => break,
            }
            terminal.draw(|f| ui::draw(f, self))?;
        }

        // Best-effort cleanup of temp files on exit.
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(&self.settings_path);

        Ok(())
    }

    /// Spawn a new session in `cwd` and push it onto `sessions`.
    fn start_session(&mut self, cwd: PathBuf, rows: u16, cols: u16) {
        let Some(path) = self.claude_path.clone() else { return };
        // Generate the uuid ONCE so it is equal to the `--session-id` arg
        // passed to `claude` and the id stored in the SessionManager — the
        // hook listener uses this to correlate incoming events.
        let id = uuid::Uuid::new_v4().to_string();
        let settings_str = self.settings_path.to_string_lossy().into_owned();
        self.manager.create_with_id(id.clone(), cwd.clone());
        match pty::spawn(&path, &cwd, rows, cols, id.clone(), &settings_str, self.tx.clone()) {
            Ok(pty) => {
                // State starts as Starting (from create_with_id) and is driven
                // entirely by hooks (SessionStart → Starting, UserPromptSubmit →
                // Running, etc.). No eager Running here — that caused a visible
                // flicker: Running → Starting → Running on every launch.
                let new_index = self.sessions.len();
                self.sessions.push((id, pty));
                self.focus = Focus::Session(new_index); // new session becomes focused
            }
            Err(_) => {
                self.manager.set_state(&id, SessionState::Error);
            }
        }
    }

    /// Returns the id of the currently focused session, if any.
    pub fn focused_id(&self) -> Option<String> {
        if let Focus::Session(i) = self.focus {
            self.sessions.get(i).map(|(id, _)| id.clone())
        } else {
            None
        }
    }

    /// Jump focus to the next session that is `WaitingOnYou`, searching after
    /// the currently focused index (wrapping). No-op if no such session exists.
    pub fn jump_to_attention(&mut self) {
        // Collect the current states in session order.
        let states: Vec<SessionState> = self.sessions.iter()
            .map(|(id, _)| self.manager.get(id).map(|s| s.state).unwrap_or(SessionState::Idle))
            .collect();
        // Determine the search start: use the focused session index, or 0 for Home/unknown.
        let from = match self.focus {
            Focus::Session(i) => i,
            Focus::Home => {
                // Start from the last session so the search begins at index 0.
                states.len().saturating_sub(1)
            }
        };
        if let Some(i) = next_attention(&states, from) {
            self.focus = Focus::Session(i);
            self.sync_focus_size();
        }
    }

    /// Kill the focused session: signal the process, remove from manager + vec,
    /// update focus to a safe state.
    fn kill_focused(&mut self) {
        let Focus::Session(i) = self.focus else { return };
        if self.sessions.is_empty() { return; }
        let (id, mut pty) = self.sessions.remove(i);
        let _ = pty.killer.kill();
        self.manager.remove(&id);
        // Move focus to a safe state.
        if !self.sessions.is_empty() {
            let clamped = i.min(self.sessions.len() - 1);
            self.focus = Focus::Session(clamped);
        } else if self.home_visible {
            self.focus = Focus::Home;
        } else {
            // No sessions, Home hidden — show Home again as a safety net.
            self.home_visible = true;
            self.focus = Focus::Home;
        }
    }

    /// Resize the focused session's master PTY and vt100 parser.
    fn sync_focus_size(&mut self) {
        if let Focus::Session(i) = self.focus {
            if let Ok((term_w, term_h)) = crossterm::terminal::size() {
                let (rows, cols) = pane_dims(term_w, term_h);
                if let Some((_, pty)) = self.sessions.get(i) {
                    let _ = pty.master.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                    pty.parser.lock().unwrap().set_size(rows, cols);
                }
            }
        }
        // Home needs no PTY resize.
    }

    fn on_key(&mut self, key: KeyEvent) {
        // Prompt mode: edit the buffer, do NOT forward to any PTY.
        if self.prompt.is_some() {
            self.on_input_key(key);
            return;
        }

        // Leader: Ctrl-a starts a command sequence.
        if self.leader {
            self.leader = false;
            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('n') => {
                    self.prompt = Some(Prompt::NewSession(
                        std::env::current_dir()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    ));
                }
                KeyCode::Char('r') => {
                    if let Focus::Session(i) = self.focus {
                        if let Some((id, _)) = self.sessions.get(i) {
                            if let Some(s) = self.manager.get(id) {
                                self.prompt = Some(Prompt::Rename(s.label.clone()));
                            }
                        }
                    }
                    // Focus::Home → no-op
                }
                KeyCode::Char('h') => {
                    self.home_visible = true;
                    self.focus = Focus::Home;
                }
                KeyCode::Char('x') => {
                    match self.focus {
                        Focus::Home => {
                            // Hide Home only if sessions exist; otherwise stay at Home.
                            if !self.sessions.is_empty() {
                                self.home_visible = false;
                                self.focus = Focus::Session(0);
                            }
                        }
                        Focus::Session(_) => self.kill_focused(),
                    }
                }
                KeyCode::Char(c @ '1'..='9') => {
                    let i = (c as u8 - b'1') as usize;
                    if i < self.sessions.len() {
                        self.focus = Focus::Session(i);
                        self.sync_focus_size();
                    }
                }
                KeyCode::Char('[') => {
                    self.focus = cycle_focus(self.focus, self.home_visible, self.sessions.len(), -1);
                    self.sync_focus_size();
                }
                KeyCode::Char(']') => {
                    self.focus = cycle_focus(self.focus, self.home_visible, self.sessions.len(), 1);
                    self.sync_focus_size();
                }
                KeyCode::Char('!') => {
                    self.jump_to_attention();
                }
                _ => {}
            }
            return;
        }

        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.leader = true;
            return;
        }

        // Forward to the focused session (Home doesn't accept input).
        if let Focus::Session(i) = self.focus {
            if let Some((_, pty)) = self.sessions.get_mut(i) {
                if let Some(bytes) = keys::encode(&key) {
                    let _ = pty.writer.write_all(&bytes);
                    let _ = pty.writer.flush();
                }
            }
        }
    }

    /// Handle a key while in prompt mode.
    fn on_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
            }
            KeyCode::Enter => {
                if let Some(prompt) = self.prompt.take() {
                    match prompt {
                        Prompt::NewSession(buf) => {
                            let path = PathBuf::from(&buf);
                            if path.is_dir() {
                                if let Ok((term_w, term_h)) = crossterm::terminal::size() {
                                    let (rows, cols) = pane_dims(term_w, term_h);
                                    self.start_session(path, rows, cols);
                                }
                            }
                            // If not a valid directory, close the prompt without creating a session.
                        }
                        Prompt::Rename(buf) => {
                            let trimmed = buf.trim().to_string();
                            if !trimmed.is_empty() {
                                if let Some(id) = self.focused_id() {
                                    self.manager.rename(&id, &trimmed);
                                }
                            }
                            // Empty buffer → cancel without changing the name.
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(p) = self.prompt.as_mut() {
                    p.buf_mut().pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(p) = self.prompt.as_mut() {
                    p.buf_mut().push(c);
                }
            }
            _ => {}
        }
    }

    fn on_resize(&mut self, w: u16, h: u16) {
        // Only resize the focused session's PTY; Home needs no resize.
        if let Focus::Session(i) = self.focus {
            let (rows, cols) = pane_dims(w, h);
            if let Some((_, pty)) = self.sessions.get(i) {
                let _ = pty.master.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                pty.parser.lock().unwrap().set_size(rows, cols);
            }
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if m.column < mouse::SIDEBAR_WIDTH {
            // ── Sidebar region ────────────────────────────────────────────────
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(new_focus) = mouse::sidebar_hit(m.row, self.home_visible, self.sessions.len()) {
                        self.focus = new_focus;
                        if matches!(new_focus, Focus::Session(_)) {
                            self.sync_focus_size();
                        }
                    }
                }
                MouseEventKind::ScrollUp => {
                    self.focus = cycle_focus(self.focus, self.home_visible, self.sessions.len(), -1);
                    self.sync_focus_size();
                }
                MouseEventKind::ScrollDown => {
                    self.focus = cycle_focus(self.focus, self.home_visible, self.sessions.len(), 1);
                    self.sync_focus_size();
                }
                _ => {}
            }
        } else {
            // ── Main pane region ──────────────────────────────────────────────
            // Only forward when a session is focused; ignore when Home is focused.
            if let Focus::Session(i) = self.focus {
                // Translate terminal coordinates to 1-based pane-interior coords.
                // The pane interior starts after the 26-wide sidebar and the
                // main block's left border (1 cell), and below the top border
                // (1 cell at row 0).
                // col: terminal col 27 → pane col 1.  saturating_sub(26) gives 1-based.
                // row: terminal row 1 → pane row 1.  m.row is already 0-based terminal,
                //      so row 0 is inside the top border; row 1 is pane row 1 (1-based).
                let pane_col = m.column.saturating_sub(mouse::SIDEBAR_WIDTH);
                let pane_row = m.row;

                // Bounds check: col 0 means we're on the left border — ignore.
                if pane_col == 0 || pane_row == 0 {
                    return;
                }

                // Check against the session's parser screen size.
                if let Some((_, pty)) = self.sessions.get_mut(i) {
                    let (screen_rows, screen_cols) = {
                        let parser = pty.parser.lock().unwrap();
                        let screen = parser.screen();
                        (screen.size().0, screen.size().1)
                    };
                    if pane_col > screen_cols || pane_row > screen_rows {
                        return; // out of bounds
                    }
                    if let Some(bytes) = mouse::encode_sgr(&m, pane_col, pane_row) {
                        let _ = pty.writer.write_all(&bytes);
                        let _ = pty.writer.flush();
                    }
                }
            }
            // Home focused: ignore main-pane mouse.
        }
    }
}

/// Interior size of the main pane given the full terminal size: subtract the
/// SIDEBAR_WIDTH-wide sidebar and 1-cell borders on each side.
pub fn pane_dims(term_w: u16, term_h: u16) -> (u16, u16) {
    let cols = term_w.saturating_sub(mouse::SIDEBAR_WIDTH).saturating_sub(2).max(1);
    let rows = term_h.saturating_sub(2).max(1);
    (rows, cols)
}

// ─────────────────────────────────────────────────────────────────────────────

/// Map a `HookEvent` to the `SessionState` it implies, or `None` for events
/// that don't drive a state transition (e.g. unknown event names).
pub fn state_for_hook(ev: &crate::hooks::HookEvent) -> Option<SessionState> {
    match ev.event.as_str() {
        "SessionStart"      => Some(SessionState::Starting),
        "UserPromptSubmit"  => Some(SessionState::Running),
        "Stop"              => Some(SessionState::Idle),
        "Notification" => {
            if ev.notification_type.as_deref() == Some("idle_prompt") {
                Some(SessionState::Idle)
            } else {
                Some(SessionState::WaitingOnYou)
            }
        }
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────

/// Returns the index of the next session (searching AFTER `from`, wrapping)
/// whose state is `WaitingOnYou`; `None` if there are no such sessions.
pub fn next_attention(states: &[SessionState], from: usize) -> Option<usize> {
    let n = states.len();
    if n == 0 { return None; }
    for offset in 1..=n {
        let i = (from + offset) % n;
        if states[i] == SessionState::WaitingOnYou {
            return Some(i);
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    // ── visible_entries ───────────────────────────────────────────────────────

    #[test]
    fn visible_entries_home_visible_includes_home_first() {
        let v = visible_entries(true, 2);
        assert_eq!(v, vec![Focus::Home, Focus::Session(0), Focus::Session(1)]);
    }

    #[test]
    fn visible_entries_home_hidden_excludes_home() {
        let v = visible_entries(false, 2);
        assert_eq!(v, vec![Focus::Session(0), Focus::Session(1)]);
    }

    #[test]
    fn visible_entries_no_sessions_home_visible() {
        let v = visible_entries(true, 0);
        assert_eq!(v, vec![Focus::Home]);
    }

    #[test]
    fn visible_entries_empty_when_home_hidden_no_sessions() {
        let v = visible_entries(false, 0);
        assert!(v.is_empty());
    }

    // ── cycle_focus ───────────────────────────────────────────────────────────

    #[test]
    fn cycle_forward_wraps_home_sessions() {
        // [Home, S0, S1] forward from S1 → Home
        assert_eq!(cycle_focus(Focus::Session(1), true, 2, 1), Focus::Home);
    }

    #[test]
    fn cycle_backward_wraps_home_sessions() {
        // [Home, S0, S1] backward from Home → S1
        assert_eq!(cycle_focus(Focus::Home, true, 2, -1), Focus::Session(1));
    }

    #[test]
    fn cycle_forward_home_to_s0() {
        assert_eq!(cycle_focus(Focus::Home, true, 2, 1), Focus::Session(0));
    }

    #[test]
    fn cycle_home_skipped_when_hidden() {
        // [S0, S1] forward from S1 → S0 (Home not in list)
        assert_eq!(cycle_focus(Focus::Session(1), false, 2, 1), Focus::Session(0));
    }

    #[test]
    fn cycle_home_skipped_backward_when_hidden() {
        // [S0, S1] backward from S0 → S1
        assert_eq!(cycle_focus(Focus::Session(0), false, 2, -1), Focus::Session(1));
    }

    #[test]
    fn cycle_single_entry_stays_put() {
        // Only Home visible, no sessions — stays Home
        assert_eq!(cycle_focus(Focus::Home, true, 0, 1), Focus::Home);
        assert_eq!(cycle_focus(Focus::Home, true, 0, -1), Focus::Home);
    }

    #[test]
    fn cycle_no_entries_returns_current() {
        // Home hidden, no sessions — returns current unchanged
        assert_eq!(cycle_focus(Focus::Session(0), false, 0, 1), Focus::Session(0));
    }

    // ── cycle_focus stale-focus recovery ──────────────────────────────────────

    #[test]
    fn cycle_stale_focus_recovers_to_visible_entry() {
        // When focus is Session(5) but only [Home, Session(0), Session(1)] are visible,
        // cycling forward from the default recovered position (0) yields Session(0).
        assert_eq!(
            cycle_focus(Focus::Session(5), true, 2, 1),
            Focus::Session(0)
        );
    }

    // ── state_for_hook ────────────────────────────────────────────────────────

    fn make_ev(event: &str, notification_type: Option<&str>) -> crate::hooks::HookEvent {
        crate::hooks::HookEvent {
            session_id: "test-session".to_string(),
            event: event.to_string(),
            notification_type: notification_type.map(|s| s.to_string()),
        }
    }

    #[test]
    fn state_for_hook_session_start_returns_starting() {
        let ev = make_ev("SessionStart", None);
        assert_eq!(state_for_hook(&ev), Some(SessionState::Starting));
    }

    #[test]
    fn state_for_hook_user_prompt_submit_returns_running() {
        let ev = make_ev("UserPromptSubmit", None);
        assert_eq!(state_for_hook(&ev), Some(SessionState::Running));
    }

    #[test]
    fn state_for_hook_stop_returns_idle() {
        let ev = make_ev("Stop", None);
        assert_eq!(state_for_hook(&ev), Some(SessionState::Idle));
    }

    #[test]
    fn state_for_hook_notification_permission_prompt_returns_waiting() {
        let ev = make_ev("Notification", Some("permission_prompt"));
        assert_eq!(state_for_hook(&ev), Some(SessionState::WaitingOnYou));
    }

    #[test]
    fn state_for_hook_notification_idle_prompt_returns_idle() {
        let ev = make_ev("Notification", Some("idle_prompt"));
        assert_eq!(state_for_hook(&ev), Some(SessionState::Idle));
    }

    #[test]
    fn state_for_hook_notification_unknown_type_returns_waiting() {
        let ev = make_ev("Notification", Some("some_unknown_type"));
        assert_eq!(state_for_hook(&ev), Some(SessionState::WaitingOnYou));
    }

    #[test]
    fn state_for_hook_notification_no_type_returns_waiting() {
        let ev = make_ev("Notification", None);
        assert_eq!(state_for_hook(&ev), Some(SessionState::WaitingOnYou));
    }

    #[test]
    fn state_for_hook_unknown_event_returns_none() {
        let ev = make_ev("SomeUnknownEvent", None);
        assert_eq!(state_for_hook(&ev), None);
    }

    // ── next_attention ────────────────────────────────────────────────────────

    #[test]
    fn next_attention_finds_next_waiting_after_from() {
        use SessionState::*;
        let states = [Running, WaitingOnYou, Idle, WaitingOnYou];
        // from=0: next waiting is index 1
        assert_eq!(next_attention(&states, 0), Some(1));
    }

    #[test]
    fn next_attention_wraps_around() {
        use SessionState::*;
        // from=2, only waiting at 1 — must wrap
        let states = [Idle, WaitingOnYou, Running, Idle];
        assert_eq!(next_attention(&states, 2), Some(1));
    }

    #[test]
    fn next_attention_returns_none_when_none_waiting() {
        use SessionState::*;
        let states = [Running, Idle, Running];
        assert_eq!(next_attention(&states, 0), None);
    }

    #[test]
    fn next_attention_skips_non_waiting_states() {
        use SessionState::*;
        // WaitingOnYou only at 3; from=0 should skip 1 and 2
        let states = [Idle, Running, Starting, WaitingOnYou];
        assert_eq!(next_attention(&states, 0), Some(3));
    }

    #[test]
    fn next_attention_does_not_return_from_itself_unless_only_one() {
        use SessionState::*;
        // from=1 (WaitingOnYou itself); should wrap and still find 1 if it's the only one
        let states = [Running, WaitingOnYou, Idle];
        assert_eq!(next_attention(&states, 1), Some(1));
    }

    #[test]
    fn next_attention_empty_returns_none() {
        assert_eq!(next_attention(&[], 0), None);
    }
}
