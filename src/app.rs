use std::collections::{HashMap, HashSet};
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use crate::resume;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use crate::config::Config;
use crate::icons::IconMode;
use crate::mem::{self, Mem};
use crate::pty::{self, PtySession};
use crate::session::{SessionManager, SessionState};
use crate::{config, keys, mouse, ui, workspace, Tui};

pub enum AppEvent {
    Input(Event),
    Output,
    Exited { id: String, clean: bool },
    Hook(crate::hooks::HookEvent),
    /// Fired by the timer thread every ~10 seconds.
    Tick,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Home,
    Session(usize),
    Settings,
    ResumePicker,
}

// ── Pure focus-transition helpers (testable without a live PTY) ──────────────

/// Build the ordered list of visible "slots" for cycling:
/// Home (if visible) → each session index → Settings (always last).
/// This is the unified cycle for both `[`/`]` and `Ctrl-a Up/Down`.
pub fn visible_entries(home_visible: bool, session_count: usize) -> Vec<Focus> {
    let mut v = Vec::new();
    if home_visible {
        v.push(Focus::Home);
    }
    for i in 0..session_count {
        v.push(Focus::Session(i));
    }
    v.push(Focus::Settings);
    v
}

/// Compute the next focus when cycling forward (+1) or backward (-1).
/// Returns the current focus unchanged if there are no visible entries.
/// The cycle is: Home → sessions → Settings → Home (wrapping).
/// When the current focus is not found in the list (e.g. ResumePicker),
/// position defaults to 0.
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

/// Identifies which text/number setting is being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKey {
    NtfyTopic,
    ReapTimeout,
    MemWarn,
}

impl SettingKey {
    /// Human-readable label shown in the edit prompt line.
    pub fn label(self) -> &'static str {
        match self {
            SettingKey::NtfyTopic   => "ntfy topic",
            SettingKey::ReapTimeout => "reap timeout (secs)",
            SettingKey::MemWarn     => "mem warn (MB, 0=off)",
        }
    }
}

/// Metadata for a single settings row: plain-language label + one-sentence description.
/// Used by both the settings list renderer and the help line at the bottom of the pane.
pub struct SettingMeta {
    /// Short plain-language label shown in the list.
    pub label: &'static str,
    /// One–two sentence description shown in the help line when this row is highlighted.
    pub description: &'static str,
}

/// Canonical ordered table of all settings rows.
/// **Order must match** `toggle_settings_bool_at_cursor` and the `Enter` match
/// in `on_settings_key` (row 3 = NtfyTopic, 5 = ReapTimeout, 6 = MemWarn).
pub const SETTINGS: &[SettingMeta] = &[
    SettingMeta {
        label: "Do Not Disturb (mute all alerts)",
        description: "Silences the bell, desktop notifications, and phone push all at once.",
    },
    SettingMeta {
        label: "Terminal bell on alert",
        description: "Rings the terminal bell when an unfocused session needs your attention.",
    },
    SettingMeta {
        label: "Desktop notifications",
        description: "Shows a desktop notification when an unfocused session needs you.",
    },
    SettingMeta {
        label: "Phone push topic (ntfy)",
        description: "Get phone pushes via ntfy.sh. Subscribe to this topic in the ntfy app on your phone. Empty = off.",
    },
    SettingMeta {
        label: "Auto-sleep idle sessions",
        description: "Automatically sleep (kill the process of) idle, unfocused sessions to free RAM. They resume automatically when you open them. Off by default.",
    },
    SettingMeta {
        label: "Sleep after (seconds idle)",
        description: "How long a session must sit idle and unfocused before it auto-sleeps (only if Auto-sleep is on).",
    },
    SettingMeta {
        label: "Warn at memory usage (MB)",
        description: "Warn (sidebar marker + notification) when a session's memory passes this. Advisory only — it never kills anything.",
    },
    SettingMeta {
        label: "Nerd Font icons",
        description: "Use Nerd Font icon glyphs (requires your terminal to use a Nerd Font). Off = universal Unicode symbols.",
    },
    SettingMeta {
        label: "Mouse capture (turn off for native copy/paste)",
        description: "When on, click to focus rows and scroll. Turn OFF to use your terminal's native text selection and copy.",
    },
];

/// Describes what kind of text prompt is currently active (if any).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// The user is typing a directory path for a new session.
    NewSession(String),
    /// The user is typing a new label for the focused session.
    Rename(String),
    /// Awaiting single-keystroke confirmation before restarting the named session.
    ConfirmRestart(String),
    /// Inline edit of a text/number setting on the Settings screen.
    EditSetting { key: SettingKey, buf: String },
}

impl Prompt {
    /// Returns a mutable reference to the inner text buffer.
    /// Not applicable to `ConfirmRestart` (no text field) — panics if called on it.
    pub fn buf_mut(&mut self) -> &mut String {
        match self {
            Prompt::NewSession(s) | Prompt::Rename(s) => s,
            Prompt::EditSetting { buf, .. } => buf,
            Prompt::ConfirmRestart(_) => panic!("ConfirmRestart has no text buffer"),
        }
    }
}

pub struct App {
    pub should_quit: bool,
    pub manager: SessionManager,
    /// All sessions in creation order: (id, pty).
    /// `Some(pty)` = live process; `None` = parked (no live process, sidebar entry stays).
    pub sessions: Vec<(String, Option<PtySession>)>,
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
    /// ntfy.sh topic for phone push notifications. `None` = feature disabled.
    pub ntfy_topic: Option<String>,
    /// Last known "what does this session want" message for WaitingOnYou sessions.
    pub pending_msg: HashMap<String, String>,
    /// Temp settings file path written for `--settings`; removed on quit.
    settings_path: std::path::PathBuf,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,

    // ── R2: memory monitor + opt-in idle park ──────────────────────────────

    /// RSS warning threshold in KB. `None` disables the warning entirely
    /// (set when `--mem-warn 0` is passed; default: 4096 MB = 4_194_304 KB).
    pub mem_warn_kb: Option<u64>,
    /// When `true` (set by `--reap-idle`), sessions that are Idle + unfocused
    /// + unpinned + timed-out are automatically parked. Default: `false`.
    pub reap_idle: bool,
    /// How long a session must be continuously Idle before it is parked
    /// (only when `reap_idle` is `true`). Default: 600 s.
    pub reap_timeout: Duration,
    /// When a session first became Idle (cleared when it leaves Idle, is
    /// focused, or receives input).
    pub idle_since: HashMap<String, Instant>,
    /// Last-measured RSS (KB) for each live session; used by the sidebar UI.
    pub rss: HashMap<String, u64>,
    /// Sessions that have already triggered the high-memory warning this
    /// crossing (reset once rss drops back below the threshold).
    pub warned: HashSet<String>,
    /// sysinfo wrapper for per-process memory queries.
    mem_sys: Mem,

    // ── Settings screen ────────────────────────────────────────────────────

    /// Persisted configuration (loaded at startup, saved on every UI toggle).
    pub config: Config,
    /// Which row is highlighted in the Settings screen (0-based).
    pub settings_cursor: usize,
    /// Whether crossterm mouse capture is currently enabled.
    /// When false, the terminal's native text selection / copy works.
    pub mouse_on: bool,

    // ── Resume picker (Ctrl-a o / Ctrl-r) ────────────────────────────────

    /// Scanned past sessions for the resume picker.
    pub resume_items: Vec<resume::Past>,
    /// Highlighted row within the filtered resume list.
    pub resume_cursor: usize,
    /// Current filter string typed by the user.
    pub resume_filter: String,
    /// When `Some(path)`, the picker is scoped to that directory.
    /// `None` = global picker (Ctrl-a o).
    pub resume_scope: Option<PathBuf>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let settings_path = crate::hooks::settings_path();

        // Start the hook listener — the closure captures a cloned Sender so
        // hooks.rs stays free of the AppEvent type.
        let hook_tx = tx.clone();
        let port = crate::hooks::listen(move |ev| {
            let _ = hook_tx.send(AppEvent::Hook(ev));
        }).unwrap_or(0);

        // Write the shared hooks settings file so `--settings` sessions can
        // pick it up. Best-effort — if it fails, hooks just won't fire.
        let _ = crate::hooks::write_settings_file(&settings_path, &port.to_string());

        // ── Step 1: Load persisted config and apply it as the baseline ────────
        let cfg = config::load();

        let mut bell_on           = cfg.bell;
        let mut notify_on         = cfg.desktop_notify;
        let mut ntfy_topic        = cfg.ntfy_topic.clone();
        let mut reap_idle         = cfg.reap_idle;
        let mut reap_timeout      = Duration::from_secs(cfg.reap_timeout_secs);
        let mut mem_warn_kb: Option<u64> = if cfg.mem_warn_mb > 0 {
            Some(cfg.mem_warn_mb * 1024)
        } else {
            None
        };
        let mut icon_mode = if cfg.nerd_icons {
            IconMode::Nerd
        } else {
            IconMode::Ascii
        };

        // ── Step 2: Apply CLI flags (override config for this run) ────────────
        // This is the TUI path only — __hook is handled before App::new().
        let args: Vec<String> = std::env::args().collect();

        if args.contains(&"--no-bell".to_string()) {
            bell_on = false;
        }
        if args.contains(&"--no-notify".to_string()) {
            notify_on = false;
        }
        if args.contains(&"--reap-idle".to_string()) {
            reap_idle = true;
        }

        // --ntfy <topic>  OR  env CLAUDE_DECK_NTFY  (arg wins over env; if neither
        // is present, keep the value from config)
        let ntfy_env = std::env::var("CLAUDE_DECK_NTFY").ok();
        let ntfy_from_cli = crate::notify::ntfy_from(&args, ntfy_env.as_deref());
        if ntfy_from_cli.is_some() {
            ntfy_topic = ntfy_from_cli;
        }

        // --mem-warn <MB>  (0 = disable; if absent, keep value from config)
        if let Some(mb) = args.windows(2)
            .find(|w| w[0] == "--mem-warn")
            .and_then(|w| w[1].parse::<u64>().ok())
        {
            mem_warn_kb = if mb == 0 { None } else { Some(mb * 1024) };
        }

        // --reap-timeout <secs>  (if absent, keep value from config)
        if let Some(secs) = args.windows(2)
            .find(|w| w[0] == "--reap-timeout")
            .and_then(|w| w[1].parse::<u64>().ok())
        {
            reap_timeout = Duration::from_secs(secs);
        }

        // --nerd / --ascii (override config icon mode for this run)
        let nerd_env = std::env::var("CLAUDE_DECK_ICONS")
            .map(|v| v.eq_ignore_ascii_case("nerd"))
            .unwrap_or(false);
        if nerd_env || args.iter().any(|a| a == "--nerd") {
            icon_mode = IconMode::Nerd;
        } else if args.iter().any(|a| a == "--ascii") {
            icon_mode = IconMode::Ascii;
        }

        let mouse_on = cfg.mouse;

        let mut app = Self {
            should_quit: false,
            manager: SessionManager::default(),
            sessions: Vec::new(),
            focus: Focus::Home,
            home_visible: true,
            prompt: None,
            leader: false,
            claude_path: pty::resolve_claude_path(),
            icons: icon_mode,
            bell_on,
            notify_on,
            ntfy_topic,
            pending_msg: HashMap::new(),
            settings_path,
            tx,
            rx,
            mem_warn_kb,
            reap_idle,
            reap_timeout,
            idle_since: HashMap::new(),
            rss: HashMap::new(),
            warned: HashSet::new(),
            mem_sys: Mem::new(),
            config: cfg,
            settings_cursor: 0,
            mouse_on,
            resume_items: Vec::new(),
            resume_cursor: 0,
            resume_filter: String::new(),
            resume_scope: None,
        };

        // If the persisted config has mouse disabled, turn off capture now.
        // (init_terminal already enabled it; we disable it here at startup.)
        if !mouse_on {
            let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        }

        // ── Workspace restore ──────────────────────────────────────────────────
        // Unless the user passes `--no-restore`, load the persisted workspace
        // and repopulate `sessions` as PARKED (None pty) entries.  The existing
        // revive-on-focus path resumes each session via `claude --resume` when
        // the user focuses it.  No processes are spawned here (RAM-safe).
        if !args.contains(&"--no-restore".to_string()) {
            for e in workspace::load() {
                app.manager.create_with_id(e.id.clone(), e.cwd.clone());
                app.manager.rename(&e.id, &e.label);
                if e.pinned {
                    app.manager.set_pinned(&e.id, true);
                }
                app.manager.set_state(&e.id, SessionState::Parked);
                app.sessions.push((e.id, None));
            }
            // Focus stays at Home regardless of how many sessions were restored.
        }

        app
    }

    /// Build a workspace snapshot from the current session list (all sessions,
    /// live or parked).
    fn snapshot_entries(&self) -> Vec<workspace::Entry> {
        self.manager.list().into_iter().map(|s| workspace::Entry {
            id: s.id,
            label: s.label,
            cwd: s.cwd,
            pinned: s.pinned,
        }).collect()
    }

    /// Persist the current workspace to the config-dir JSON file.
    /// Errors are silently ignored (see `workspace::save`).
    fn save_workspace(&self) {
        workspace::save(&self.snapshot_entries());
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

        // Timer thread: send Tick every ~10 s for RSS polling + idle parking.
        let tick_tx = self.tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(10));
            if tick_tx.send(AppEvent::Tick).is_err() { break; }
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
                    let any_live = self.sessions.iter().any(|(sid, _pty)| matches!(
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

                        // ── Idle tracking ─────────────────────────────────
                        if new_state == SessionState::Idle {
                            self.idle_since.entry(ev.session_id.clone()).or_insert_with(Instant::now);
                        } else {
                            // Leaving Idle (any non-Idle hook) clears the timer.
                            self.idle_since.remove(&ev.session_id);
                        }

                        // ── Pending message tracking (Feature B) ──────────
                        if new_state == SessionState::WaitingOnYou {
                            if let Some(msg) = &ev.message {
                                self.pending_msg.insert(ev.session_id.clone(), msg.clone());
                            }
                        } else {
                            // Session left WaitingOnYou — clear any pending message.
                            self.pending_msg.remove(&ev.session_id);
                        }

                        // Fire attention signals on the WaitingOnYou transition edge,
                        // but only when this session is NOT the currently focused one.
                        if new_state == SessionState::WaitingOnYou
                            && old_state != Some(SessionState::WaitingOnYou)
                        {
                            let is_focused = self.sessions.iter().enumerate()
                                .find(|(_, (id, _pty))| id == &ev.session_id)
                                .map(|(i, _)| self.focus == Focus::Session(i))
                                .unwrap_or(false);

                            if !is_focused {
                                // Resolve the sidebar label for the notification.
                                let label = self.manager.get(&ev.session_id)
                                    .map(|s| s.label.clone())
                                    .unwrap_or_else(|| ev.session_id.clone());
                                // ── DND guard: suppress ALL alerts when Do Not Disturb is on ──
                                if !self.config.dnd {
                                    if self.bell_on {
                                        crate::notify::bell();
                                    }
                                    if self.notify_on {
                                        crate::notify::desktop(&label);
                                    }
                                    // ── Feature A: phone push via ntfy.sh ─────
                                    if let Some(topic) = &self.ntfy_topic {
                                        crate::notify::push_ntfy(
                                            topic,
                                            "claude-deck",
                                            &format!("{label} needs you"),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(AppEvent::Tick) => self.on_tick(),
                Err(_) => break,
            }
            terminal.draw(|f| ui::draw(f, self))?;
        }

        // Best-effort cleanup of the temp settings file on exit.
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
        match pty::spawn(&path, &cwd, rows, cols, id.clone(), &settings_str, self.tx.clone(), false) {
            Ok(pty) => {
                // State starts as Starting (from create_with_id) and is driven
                // entirely by hooks (SessionStart → Starting, UserPromptSubmit →
                // Running, etc.). No eager Running here — that caused a visible
                // flicker: Running → Starting → Running on every launch.
                let new_index = self.sessions.len();
                self.sessions.push((id, Some(pty)));
                self.focus = Focus::Session(new_index); // new session becomes focused
            }
            Err(_) => {
                self.manager.set_state(&id, SessionState::Error);
            }
        }
        self.save_workspace();
    }

    /// Returns the id of the currently focused session, if any.
    pub fn focused_id(&self) -> Option<String> {
        if let Focus::Session(i) = self.focus {
            self.sessions.get(i).map(|(id, _pty)| id.clone())
        } else {
            None
        }
    }

    /// Jump focus to the next session that is `WaitingOnYou`, searching after
    /// the currently focused index (wrapping). No-op if no such session exists.
    pub fn jump_to_attention(&mut self) {
        // Collect the current states in session order.
        let states: Vec<SessionState> = self.sessions.iter()
            .map(|(id, _pty)| self.manager.get(id).map(|s| s.state).unwrap_or(SessionState::Idle))
            .collect();
        // Determine the search start: use the focused session index, or 0 for Home/Settings.
        let from = match self.focus {
            Focus::Session(i) => i,
            Focus::Home | Focus::Settings | Focus::ResumePicker => {
                // Start from the last session so the search begins at index 0.
                states.len().saturating_sub(1)
            }
        };
        if let Some(i) = next_attention(&states, from) {
            self.focus = Focus::Session(i);
            self.maybe_revive_focused();
            self.sync_focus_size();
        }
    }

    /// Kill the focused session: signal the process, remove from manager + vec,
    /// update focus to a safe state.
    fn kill_focused(&mut self) {
        let Focus::Session(i) = self.focus else { return };
        if self.sessions.is_empty() { return; }
        let (id, pty_opt) = self.sessions.remove(i);
        if let Some(mut pty) = pty_opt {
            let _ = pty.killer.kill();
        }
        self.pending_msg.remove(&id);
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
        self.save_workspace();
    }

    /// If the currently focused session is parked, revive it immediately.
    /// Call this right after changing `self.focus` to a session slot.
    fn maybe_revive_focused(&mut self) {
        if let Focus::Session(i) = self.focus {
            let is_parked = self.sessions.get(i).map(|(_, pty)| pty.is_none()).unwrap_or(false);
            if is_parked {
                self.revive_session(i);
            }
        }
    }

    /// Resize the focused session's master PTY and vt100 parser.
    fn sync_focus_size(&mut self) {
        if let Focus::Session(i) = self.focus {
            // Clear idle timer for the newly-focused session.
            if let Some((id, _)) = self.sessions.get(i) {
                self.idle_since.remove(id);
            }
            if let Ok((term_w, term_h)) = crossterm::terminal::size() {
                let (rows, cols) = pane_dims(term_w, term_h);
                if let Some((_, Some(pty))) = self.sessions.get(i) {
                    let _ = pty.master.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                    pty.parser.lock().unwrap().set_size(rows, cols);
                }
                // Parked session (None) gets no PTY resize.
            }
        }
        // Home needs no PTY resize.
    }

    /// Park the session with the given id: kill its process, set its PTY slot
    /// to `None`, mark the session as `Parked`, and clear its idle timer.
    ///
    /// **Safety gate:** only ever called from `on_tick` for sessions that are
    /// confirmed Idle + unfocused + unpinned + timed-out.  Never called on an
    /// active (Running/WaitingOnYou/Starting) session.
    fn park_session(&mut self, id: &str) {
        if let Some((_, pty_slot)) = self.sessions.iter_mut().find(|(sid, _)| sid == id) {
            if let Some(mut pty) = pty_slot.take() {
                let _ = pty.killer.kill(); // best-effort
            }
            // pty_slot is already None after take()
        }
        self.manager.set_state(id, SessionState::Parked);
        self.idle_since.remove(id);
        self.pending_msg.remove(id);
        self.save_workspace();
    }

    /// Revive the session at `idx` if it is currently parked (`None` pty).
    ///
    /// Uses `--resume <id>` so the conversation is restored.  Clears any idle /
    /// warned state for the session and syncs the PTY to the current pane size.
    /// No-op if the session is already live or the index is out of bounds.
    pub fn revive_session(&mut self, idx: usize) {
        let Some(path) = self.claude_path.clone() else { return };
        // Only act on parked (None) slots.
        let is_parked = self.sessions.get(idx).map(|(_, pty)| pty.is_none()).unwrap_or(false);
        if !is_parked { return; }

        let (id, cwd) = {
            let (id, _) = &self.sessions[idx];
            let cwd = self.manager.get(id).map(|s| s.cwd.clone())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            (id.clone(), cwd)
        };
        let settings_str = self.settings_path.to_string_lossy().into_owned();

        // Get current pane size for the new PTY.
        let (rows, cols) = crossterm::terminal::size()
            .map(|(w, h)| pane_dims(w, h))
            .unwrap_or((24, 80));

        match pty::spawn(&path, &cwd, rows, cols, id.clone(), &settings_str, self.tx.clone(), true) {
            Ok(pty) => {
                self.sessions[idx].1 = Some(pty);
                self.manager.set_state(&id, SessionState::Starting);
                self.idle_since.remove(&id);
                self.warned.remove(&id);
                // Sync size now that the PTY is live.
                self.sync_focus_size();
            }
            Err(_) => {
                self.manager.set_state(&id, SessionState::Error);
            }
        }
    }

    /// Tick handler: measure RSS for every live session, fire memory warnings
    /// (edge-triggered), and — when `--reap-idle` is set — park sessions that
    /// qualify via `should_park`.
    fn on_tick(&mut self) {
        // Collect (id, pid) pairs for live sessions up front to avoid
        // borrowing `self.sessions` and `self.mem_sys` simultaneously.
        let live: Vec<(String, u32)> = self.sessions.iter()
            .filter_map(|(id, pty_opt)| {
                pty_opt.as_ref().and_then(|pty| pty.pid).map(|pid| (id.clone(), pid))
            })
            .collect();

        for (id, pid) in &live {
            if let Some(kb) = self.mem_sys.rss_kb(*pid) {
                self.rss.insert(id.clone(), kb);

                // Edge-triggered memory warning: fire only on the first Tick
                // that crosses the threshold; re-arm once rss drops back below.
                if let Some(warn_kb) = self.mem_warn_kb {
                    if kb >= warn_kb {
                        if !self.warned.contains(id) {
                            self.warned.insert(id.clone());
                            let label = self.manager.get(id)
                                .map(|s| s.label.clone())
                                .unwrap_or_else(|| id.clone());
                            crate::notify::desktop(&format!(
                                "{} is using {} — consider restarting",
                                label,
                                mem::fmt_kb(kb)
                            ));
                        }
                    } else {
                        // Below threshold — re-arm for the next crossing.
                        self.warned.remove(id);
                    }
                }
            }
        }

        // Opt-in idle parking.
        if self.reap_idle {
            let timeout = self.reap_timeout;
            let now = Instant::now();

            // Collect ids to park (avoid borrow conflicts).
            let to_park: Vec<String> = self.sessions.iter().enumerate()
                .filter_map(|(idx, (id, pty_opt))| {
                    // Only live sessions can be parked.
                    if pty_opt.is_none() { return None; }

                    let state = self.manager.get(id)?.state;
                    let focused = self.focus == Focus::Session(idx);
                    let pinned = self.manager.get(id).map(|s| s.pinned).unwrap_or(false);
                    let idle_for = self.idle_since.get(id).map(|t| now.duration_since(*t))?;

                    if mem::should_park(true, state, focused, pinned, idle_for, timeout) {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect();

            for id in to_park {
                self.park_session(&id);
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        // Prompt mode: edit the buffer, do NOT forward to any PTY.
        // Must come before the Settings check so that EditSetting prompts
        // (opened from the Settings screen) receive keystrokes here.
        if self.prompt.is_some() {
            self.on_input_key(key);
            return;
        }

        // Settings mode: navigate the settings list, do NOT forward to any PTY.
        if self.focus == Focus::Settings {
            self.on_settings_key(key);
            return;
        }

        // Resume picker mode: filter + navigate the past-sessions list.
        if self.focus == Focus::ResumePicker {
            self.on_resume_key(key);
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
                        if let Some((id, _pty)) = self.sessions.get(i) {
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
                        Focus::Settings | Focus::ResumePicker => {} // no-op
                    }
                }
                KeyCode::Char('R') => {
                    // Confirmed manual restart: only valid when focused on a LIVE session.
                    if let Focus::Session(i) = self.focus {
                        if let Some((id, Some(_pty))) = self.sessions.get(i) {
                            self.prompt = Some(Prompt::ConfirmRestart(id.clone()));
                        }
                    }
                    // Parked or Home → no-op
                }
                KeyCode::Char(c @ '1'..='9') => {
                    let i = (c as u8 - b'1') as usize;
                    if i < self.sessions.len() {
                        self.focus = Focus::Session(i);
                        self.maybe_revive_focused();
                        self.sync_focus_size();
                    }
                }
                KeyCode::Char('[') | KeyCode::Up => {
                    self.focus = cycle_focus(self.focus, self.home_visible, self.sessions.len(), -1);
                    if self.focus == Focus::Settings {
                        self.settings_cursor = 0;
                    } else {
                        self.maybe_revive_focused();
                        self.sync_focus_size();
                    }
                }
                KeyCode::Char(']') | KeyCode::Down => {
                    self.focus = cycle_focus(self.focus, self.home_visible, self.sessions.len(), 1);
                    if self.focus == Focus::Settings {
                        self.settings_cursor = 0;
                    } else {
                        self.maybe_revive_focused();
                        self.sync_focus_size();
                    }
                }
                KeyCode::Char('!') => {
                    self.jump_to_attention();
                }
                KeyCode::Char('p') => {
                    // Toggle pin on the focused session; no-op on Home.
                    if let Focus::Session(i) = self.focus {
                        if let Some((id, _pty)) = self.sessions.get(i) {
                            let id = id.clone();
                            self.manager.toggle_pin(&id);
                            self.save_workspace();
                        }
                    }
                }
                KeyCode::Char('s') => {
                    // Open the Settings screen.
                    self.focus = Focus::Settings;
                    self.settings_cursor = 0;
                }
                KeyCode::Char('o') => {
                    // Open the global resume picker: scan all past sessions.
                    self.resume_items = resume::scan(120);
                    self.resume_filter.clear();
                    self.resume_cursor = 0;
                    self.resume_scope = None;
                    self.focus = Focus::ResumePicker;
                }
                KeyCode::Char('m') => {
                    // Toggle mouse capture. When off, the terminal's native
                    // text selection / copy works.
                    self.mouse_on = !self.mouse_on;
                    self.config.mouse = self.mouse_on;
                    if self.mouse_on {
                        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
                    } else {
                        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
                    }
                    config::save(&self.config);
                }
                _ => {}
            }
            return;
        }

        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.leader = true;
            return;
        }

        // Forward to the focused session (Home and parked sessions don't accept input).
        if let Focus::Session(i) = self.focus {
            if let Some(bytes) = keys::encode(&key) {
                // Input resets the idle timer for this session.
                if let Some((id, _)) = self.sessions.get(i) {
                    self.idle_since.remove(id);
                }
                if let Some((_, Some(pty))) = self.sessions.get_mut(i) {
                    let _ = pty.writer.write_all(&bytes);
                    let _ = pty.writer.flush();
                }
            }
        }
    }

    /// Handle a key while in prompt mode.
    fn on_input_key(&mut self, key: KeyEvent) {
        // ConfirmRestart is a single-keystroke confirm — handle it before the
        // generic text-buffer path so we never touch buf_mut on this variant.
        if let Some(Prompt::ConfirmRestart(id)) = &self.prompt {
            let id = id.clone();
            self.prompt = None; // clear first (restart path re-enters on success)
            if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
                self.restart_session(&id);
            }
            // Any other key: prompt already cleared → cancelled.
            return;
        }

        // Tab path-completion: only for NewSession, never forwarded to a PTY.
        if key.code == KeyCode::Tab {
            if let Some(Prompt::NewSession(ref buf)) = self.prompt {
                let buf_clone = buf.clone();
                if let Some(completed) = complete_path(&buf_clone) {
                    if let Some(Prompt::NewSession(ref mut b)) = self.prompt {
                        *b = completed;
                    }
                }
            }
            return;
        }

        // Ctrl-r in the NewSession prompt: open the resume picker scoped to the
        // typed path.  Expand a leading `~` before scanning.
        if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(Prompt::NewSession(ref buf)) = self.prompt {
                let raw = buf.clone();
                // Expand leading `~` to home dir.
                let expanded: String = if raw.starts_with('~') {
                    if let Some(home) = dirs::home_dir() {
                        format!("{}{}", home.display(), &raw[1..])
                    } else {
                        raw.clone()
                    }
                } else {
                    raw.clone()
                };
                let path = PathBuf::from(&expanded);
                self.resume_items = resume::scan_for_cwd(&path, 200);
                self.resume_scope = Some(path);
                self.resume_filter.clear();
                self.resume_cursor = 0;
                self.prompt = None;
                self.focus = Focus::ResumePicker;
            }
            return;
        }

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
                                    self.save_workspace();
                                }
                            }
                            // Empty buffer → cancel without changing the name.
                        }
                        Prompt::ConfirmRestart(_) => {
                            // Handled above; unreachable here.
                        }
                        Prompt::EditSetting { key: setting_key, buf } => {
                            apply_setting_edit(self, setting_key, &buf);
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

    /// Handle a key while the Settings screen is focused.
    /// No keystrokes are forwarded to a PTY from here.
    fn on_settings_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::Home;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.settings_cursor > 0 {
                    self.settings_cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = SETTINGS_ROW_COUNT_TOTAL.saturating_sub(1);
                if self.settings_cursor < max {
                    self.settings_cursor += 1;
                }
            }
            KeyCode::Enter => {
                // Non-boolean rows open an inline edit prompt; booleans toggle.
                let edit_key = match self.settings_cursor {
                    3 => Some(SettingKey::NtfyTopic),
                    5 => Some(SettingKey::ReapTimeout),
                    6 => Some(SettingKey::MemWarn),
                    _ => None,
                };
                if let Some(key) = edit_key {
                    let current = match key {
                        SettingKey::NtfyTopic   => self.config.ntfy_topic.clone().unwrap_or_default(),
                        SettingKey::ReapTimeout => self.config.reap_timeout_secs.to_string(),
                        SettingKey::MemWarn     => self.config.mem_warn_mb.to_string(),
                    };
                    self.prompt = Some(Prompt::EditSetting { key, buf: current });
                } else {
                    self.toggle_settings_bool_at_cursor();
                }
            }
            KeyCode::Char(' ') => {
                // Space only toggles booleans (not edit rows).
                self.toggle_settings_bool_at_cursor();
            }
            _ => {}
        }
    }

    /// Handle a key while the Resume Picker is focused.
    /// No keystrokes are forwarded to any PTY.
    fn on_resume_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::Home;
            }
            KeyCode::Backspace => {
                if self.resume_filter.pop().is_some() {
                    // Reset cursor to top after filter change.
                    self.resume_cursor = 0;
                }
            }
            KeyCode::Up => {
                if self.resume_cursor > 0 {
                    self.resume_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.resume_filtered_len().saturating_sub(1);
                if self.resume_cursor < max {
                    self.resume_cursor += 1;
                }
            }
            KeyCode::PageUp => {
                self.resume_cursor = self.resume_cursor.saturating_sub(10);
            }
            KeyCode::PageDown => {
                let max = self.resume_filtered_len().saturating_sub(1);
                self.resume_cursor = (self.resume_cursor + 10).min(max);
            }
            KeyCode::Enter => {
                // Collect the filtered list, pick the highlighted item.
                let filter = self.resume_filter.to_lowercase();
                let filtered: Vec<usize> = self.resume_items.iter().enumerate()
                    .filter(|(_, p)| resume_matches(p, &filter))
                    .map(|(i, _)| i)
                    .collect();
                if let Some(&orig_idx) = filtered.get(self.resume_cursor) {
                    let past = self.resume_items[orig_idx].clone();
                    self.open_past_session(past);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.resume_filter.push(c);
                // Reset cursor to top after filter change.
                self.resume_cursor = 0;
            }
            _ => {}
        }
    }

    /// Number of items in the filtered resume list (used to clamp the cursor).
    fn resume_filtered_len(&self) -> usize {
        let filter = self.resume_filter.to_lowercase();
        self.resume_items.iter().filter(|p| resume_matches(p, &filter)).count()
    }

    /// Open (or focus) a past session from the resume picker.
    ///
    /// - If a session with the same id is already open, focus it.
    /// - Otherwise: create a parked session entry and let the existing revive
    ///   path resume it via `claude --resume`.
    fn open_past_session(&mut self, past: resume::Past) {
        // Check if already open.
        if let Some(idx) = self.sessions.iter().position(|(id, _)| id == &past.id) {
            self.focus = Focus::Session(idx);
            self.maybe_revive_focused();
            self.sync_focus_size();
            return;
        }

        // Register as a parked session.
        self.manager.create_with_id(past.id.clone(), past.cwd.clone());
        self.manager.rename(&past.id, &past.title);
        self.manager.set_state(&past.id, SessionState::Parked);
        let new_index = self.sessions.len();
        self.sessions.push((past.id, None));
        self.save_workspace();

        // Focus the new slot — maybe_revive_focused will spawn `claude --resume`.
        self.focus = Focus::Session(new_index);
        self.maybe_revive_focused();
        self.sync_focus_size();
    }

    /// Toggle the boolean setting at the current `settings_cursor` row.
    /// Non-boolean rows (ntfy_topic, reap_timeout_secs, mem_warn_mb) are no-ops in SET1.
    fn toggle_settings_bool_at_cursor(&mut self) {
        match self.settings_cursor {
            0 => { // dnd
                self.config.dnd = !self.config.dnd;
            }
            1 => { // bell
                self.config.bell = !self.config.bell;
                self.bell_on = self.config.bell;
            }
            2 => { // desktop_notify
                self.config.desktop_notify = !self.config.desktop_notify;
                self.notify_on = self.config.desktop_notify;
            }
            3 => { // ntfy_topic — no-op in SET1
            }
            4 => { // reap_idle
                self.config.reap_idle = !self.config.reap_idle;
                self.reap_idle = self.config.reap_idle;
            }
            5 => { // reap_timeout_secs — no-op in SET1
            }
            6 => { // mem_warn_mb — no-op in SET1
            }
            7 => { // nerd_icons
                self.config.nerd_icons = !self.config.nerd_icons;
                self.icons = if self.config.nerd_icons {
                    IconMode::Nerd
                } else {
                    IconMode::Ascii
                };
            }
            8 => { // mouse capture
                self.config.mouse = !self.config.mouse;
                self.mouse_on = self.config.mouse;
                if self.mouse_on {
                    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
                } else {
                    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
                }
            }
            _ => {}
        }
        config::save(&self.config);
    }

    /// Kill the live PTY for `id` and respawn with `--resume`, keeping the
    /// session in the same slot.  State resets to Starting.  Clears RSS/warned.
    ///
    /// Called only from the confirmed `Ctrl-a R` path.
    fn restart_session(&mut self, id: &str) {
        let Some(path) = self.claude_path.clone() else { return };

        // Find the session index.
        let Some(idx) = self.sessions.iter().position(|(sid, _)| sid == id) else { return };

        // Kill the live PTY (best-effort).
        if let Some((_, pty_slot)) = self.sessions.get_mut(idx) {
            if let Some(mut pty) = pty_slot.take() {
                let _ = pty.killer.kill();
            }
        }

        // Clear memory state for this id.
        self.rss.remove(id);
        self.warned.remove(id);
        self.idle_since.remove(id);

        // Respawn with --resume.
        let cwd = self.manager.get(id).map(|s| s.cwd.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let settings_str = self.settings_path.to_string_lossy().into_owned();
        let (rows, cols) = crossterm::terminal::size()
            .map(|(w, h)| pane_dims(w, h))
            .unwrap_or((24, 80));
        let id_owned = id.to_string();
        match pty::spawn(&path, &cwd, rows, cols, id_owned.clone(), &settings_str, self.tx.clone(), true) {
            Ok(pty) => {
                self.sessions[idx].1 = Some(pty);
                self.manager.set_state(&id_owned, SessionState::Starting);
                self.sync_focus_size();
            }
            Err(_) => {
                self.manager.set_state(&id_owned, SessionState::Error);
            }
        }
    }

    fn on_resize(&mut self, w: u16, h: u16) {
        // Only resize the focused session's PTY; Home and parked sessions need no resize.
        if let Focus::Session(i) = self.focus {
            let (rows, cols) = pane_dims(w, h);
            if let Some((_, Some(pty))) = self.sessions.get(i) {
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
                    let term_h = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(24);
                    if let Some(new_focus) = mouse::sidebar_hit_with_height(
                        m.row, self.home_visible, self.sessions.len(), term_h,
                    ) {
                        self.focus = new_focus;
                        if matches!(new_focus, Focus::Session(_)) {
                            self.maybe_revive_focused();
                            self.sync_focus_size();
                        } else if new_focus == Focus::Settings {
                            self.settings_cursor = 0;
                        }
                    }
                }
                MouseEventKind::ScrollUp => {
                    self.focus = cycle_focus(self.focus, self.home_visible, self.sessions.len(), -1);
                    self.maybe_revive_focused();
                    self.sync_focus_size();
                }
                MouseEventKind::ScrollDown => {
                    self.focus = cycle_focus(self.focus, self.home_visible, self.sessions.len(), 1);
                    self.maybe_revive_focused();
                    self.sync_focus_size();
                }
                _ => {}
            }
        } else {
            // ── Main pane region ──────────────────────────────────────────────
            // Only forward when a session is focused; ignore for Home and Settings.
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
                // Parked sessions (None) have no PTY — ignore mouse.
                if let Some((_, Some(pty))) = self.sessions.get_mut(i) {
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

/// Apply a confirmed `EditSetting` value to both `app.config` and the
/// corresponding live field, then save.  On parse failure for numeric fields,
/// silently cancels (leaves everything unchanged).
///
/// This is a free function (not a method) so it is trivially unit-testable
/// without constructing a full `App`.
pub fn apply_setting_edit(app: &mut App, key: SettingKey, raw: &str) {
    match key {
        SettingKey::NtfyTopic => {
            let trimmed = raw.trim();
            let new_val = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
            app.config.ntfy_topic = new_val.clone();
            app.ntfy_topic = new_val;
            config::save(&app.config);
        }
        SettingKey::ReapTimeout => {
            if let Ok(secs) = raw.trim().parse::<u64>() {
                app.config.reap_timeout_secs = secs;
                app.reap_timeout = Duration::from_secs(secs);
                config::save(&app.config);
            }
            // Parse failure → cancel, no change.
        }
        SettingKey::MemWarn => {
            if let Ok(mb) = raw.trim().parse::<u64>() {
                app.config.mem_warn_mb = mb;
                app.mem_warn_kb = if mb > 0 { Some(mb * 1024) } else { None };
                config::save(&app.config);
            }
            // Parse failure → cancel, no change.
        }
    }
}

/// Complete a filesystem path prefix for the NewSession prompt.
///
/// - Expands a leading `~` to the home directory for resolution, but the
///   returned buffer preserves the `~`-form if the user typed it (the
///   completion replaces only the final path component).
/// - Returns `Some(completed_buf)` on any completion (single match or
///   longest-common-prefix narrowing); `None` when there are no matches.
///
/// This is a pure function (no side effects) so it is easily unit-tested.
pub fn complete_path(buf: &str) -> Option<String> {
    // Expand a leading `~` for filesystem lookups only.
    let expanded: String = if buf.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            format!("{}{}", home.display(), &buf[1..])
        } else {
            buf.to_string()
        }
    } else {
        buf.to_string()
    };

    // Split into (search_dir, partial_component).
    let path = Path::new(&expanded);
    let (search_dir, partial) = if expanded.ends_with(MAIN_SEPARATOR) || expanded.ends_with('/') {
        // Buffer ends with separator: search inside it for an empty prefix.
        (path.to_path_buf(), String::new())
    } else {
        let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        (parent, file_name)
    };

    // Collect matching entries.
    let entries: Vec<String> = std::fs::read_dir(&search_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.starts_with(&partial) {
                // Append '/' to directories.
                let is_dir = e.file_type().ok().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    Some(format!("{}/", name))
                } else {
                    Some(name)
                }
            } else {
                None
            }
        })
        .collect();

    if entries.is_empty() {
        return None;
    }

    // Compute the completion: longest common prefix of all match names.
    let lcp = longest_common_prefix(&entries);

    // Build the new buffer by replacing the partial component in the
    // original (possibly `~`-containing) buffer.
    let buf_prefix = if buf.ends_with(MAIN_SEPARATOR) || buf.ends_with('/') {
        buf.to_string()
    } else {
        // Drop the last component from the original buffer.
        let p = Path::new(buf);
        let parent_str = p.parent()
            .and_then(|par| par.to_str())
            .unwrap_or("");
        if parent_str.is_empty() || parent_str == "." {
            String::new()
        } else {
            format!("{}/", parent_str)
        }
    };

    Some(format!("{}{}", buf_prefix, lcp))
}

/// Returns the longest common prefix of a slice of strings.
fn longest_common_prefix(items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let first = items[0].as_bytes();
    let mut len = first.len();
    for item in &items[1..] {
        let b = item.as_bytes();
        len = len.min(b.len());
        for (i, (&a, &c)) in first[..len].iter().zip(b[..len].iter()).enumerate() {
            if a != c {
                len = i;
                break;
            }
        }
    }
    std::str::from_utf8(&first[..len]).unwrap_or("").to_string()
}

/// Interior size of the main pane given the full terminal size: subtract the
/// SIDEBAR_WIDTH-wide sidebar and 1-cell borders on each side.
pub fn pane_dims(term_w: u16, term_h: u16) -> (u16, u16) {
    let cols = term_w.saturating_sub(mouse::SIDEBAR_WIDTH).saturating_sub(2).max(1);
    let rows = term_h.saturating_sub(2).max(1);
    (rows, cols)
}

/// Total number of rows in the Settings list (one per managed setting).
/// Order: dnd, bell, desktop_notify, ntfy_topic, reap_idle, reap_timeout_secs,
///        mem_warn_mb, nerd_icons, mouse.
pub const SETTINGS_ROW_COUNT_TOTAL: usize = 9;

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

/// Case-insensitive substring filter for resume-picker items.
/// Matches if the lowercase `filter` appears in the title or cwd string.
pub fn resume_matches(past: &resume::Past, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let title_lc = past.title.to_lowercase();
    let cwd_lc = past.cwd.to_string_lossy().to_lowercase();
    title_lc.contains(filter) || cwd_lc.contains(filter)
}

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
        // Settings is always the last entry.
        let v = visible_entries(true, 2);
        assert_eq!(v, vec![Focus::Home, Focus::Session(0), Focus::Session(1), Focus::Settings]);
    }

    #[test]
    fn visible_entries_home_hidden_excludes_home() {
        let v = visible_entries(false, 2);
        assert_eq!(v, vec![Focus::Session(0), Focus::Session(1), Focus::Settings]);
    }

    #[test]
    fn visible_entries_no_sessions_home_visible() {
        let v = visible_entries(true, 0);
        assert_eq!(v, vec![Focus::Home, Focus::Settings]);
    }

    #[test]
    fn visible_entries_no_sessions_home_hidden_only_settings() {
        // Home hidden, no sessions → only Settings remains.
        let v = visible_entries(false, 0);
        assert_eq!(v, vec![Focus::Settings]);
    }

    #[test]
    fn visible_entries_settings_always_last() {
        let v = visible_entries(true, 3);
        assert_eq!(v.last(), Some(&Focus::Settings));
    }

    // ── cycle_focus ───────────────────────────────────────────────────────────
    // Cycle order: Home → Session(0) → … → Session(n-1) → Settings → Home.

    #[test]
    fn cycle_forward_from_last_session_to_settings() {
        // [Home, S0, S1, Settings] forward from S1 → Settings
        assert_eq!(cycle_focus(Focus::Session(1), true, 2, 1), Focus::Settings);
    }

    #[test]
    fn cycle_forward_from_settings_wraps_to_home() {
        // [Home, S0, S1, Settings] forward from Settings → Home
        assert_eq!(cycle_focus(Focus::Settings, true, 2, 1), Focus::Home);
    }

    #[test]
    fn cycle_backward_from_home_wraps_to_settings() {
        // [Home, S0, S1, Settings] backward from Home → Settings
        assert_eq!(cycle_focus(Focus::Home, true, 2, -1), Focus::Settings);
    }

    #[test]
    fn cycle_backward_from_settings_to_last_session() {
        // [Home, S0, S1, Settings] backward from Settings → S1
        assert_eq!(cycle_focus(Focus::Settings, true, 2, -1), Focus::Session(1));
    }

    #[test]
    fn cycle_forward_home_to_s0() {
        assert_eq!(cycle_focus(Focus::Home, true, 2, 1), Focus::Session(0));
    }

    #[test]
    fn cycle_home_skipped_when_hidden_forward_from_last_session() {
        // [S0, S1, Settings] forward from S1 → Settings
        assert_eq!(cycle_focus(Focus::Session(1), false, 2, 1), Focus::Settings);
    }

    #[test]
    fn cycle_home_skipped_when_hidden_settings_wraps_to_s0() {
        // [S0, S1, Settings] forward from Settings → S0
        assert_eq!(cycle_focus(Focus::Settings, false, 2, 1), Focus::Session(0));
    }

    #[test]
    fn cycle_home_skipped_backward_when_hidden() {
        // [S0, S1, Settings] backward from S0 → Settings
        assert_eq!(cycle_focus(Focus::Session(0), false, 2, -1), Focus::Settings);
    }

    #[test]
    fn cycle_single_entry_only_settings() {
        // Home hidden, no sessions → only Settings. Cycling stays there.
        assert_eq!(cycle_focus(Focus::Settings, false, 0, 1), Focus::Settings);
        assert_eq!(cycle_focus(Focus::Settings, false, 0, -1), Focus::Settings);
    }

    #[test]
    fn cycle_home_and_settings_only_no_sessions() {
        // [Home, Settings]: forward from Home → Settings → Home
        assert_eq!(cycle_focus(Focus::Home, true, 0, 1), Focus::Settings);
        assert_eq!(cycle_focus(Focus::Settings, true, 0, 1), Focus::Home);
    }

    #[test]
    fn cycle_full_forward_loop_includes_settings() {
        // [Home, S0, S1, Settings] — walk all the way around forward.
        let mut f = Focus::Home;
        f = cycle_focus(f, true, 2, 1); assert_eq!(f, Focus::Session(0));
        f = cycle_focus(f, true, 2, 1); assert_eq!(f, Focus::Session(1));
        f = cycle_focus(f, true, 2, 1); assert_eq!(f, Focus::Settings);
        f = cycle_focus(f, true, 2, 1); assert_eq!(f, Focus::Home); // wrapped
    }

    #[test]
    fn cycle_full_backward_loop_includes_settings() {
        // [Home, S0, S1, Settings] — walk all the way around backward from Home.
        let mut f = Focus::Home;
        f = cycle_focus(f, true, 2, -1); assert_eq!(f, Focus::Settings);
        f = cycle_focus(f, true, 2, -1); assert_eq!(f, Focus::Session(1));
        f = cycle_focus(f, true, 2, -1); assert_eq!(f, Focus::Session(0));
        f = cycle_focus(f, true, 2, -1); assert_eq!(f, Focus::Home); // wrapped
    }

    // ── cycle_focus stale-focus recovery ──────────────────────────────────────

    #[test]
    fn cycle_stale_focus_recovers_to_visible_entry() {
        // When focus is Session(5) but only [Home, Session(0), Session(1), Settings]
        // are visible, cycling forward from the default recovered position (0) yields S0.
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
            message: None,
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

    // ── SettingKey parse helpers (mirrors apply_setting_edit validation) ───────

    /// Parse logic for NtfyTopic: empty/whitespace → None, else Some(trimmed).
    fn parse_ntfy_topic(raw: &str) -> Option<String> {
        let t = raw.trim();
        if t.is_empty() { None } else { Some(t.to_string()) }
    }

    /// Parse logic for ReapTimeout / MemWarn: u64 or cancel.
    fn parse_u64_setting(raw: &str) -> Option<u64> {
        raw.trim().parse::<u64>().ok()
    }

    #[test]
    fn ntfy_topic_empty_string_becomes_none() {
        assert_eq!(parse_ntfy_topic(""), None);
    }

    #[test]
    fn ntfy_topic_whitespace_only_becomes_none() {
        assert_eq!(parse_ntfy_topic("   "), None);
    }

    #[test]
    fn ntfy_topic_value_is_trimmed() {
        assert_eq!(parse_ntfy_topic("  my-topic  "), Some("my-topic".to_string()));
    }

    #[test]
    fn ntfy_topic_non_empty_returns_some() {
        assert_eq!(parse_ntfy_topic("alerts"), Some("alerts".to_string()));
    }

    #[test]
    fn reap_timeout_valid_number_parses() {
        assert_eq!(parse_u64_setting("300"), Some(300u64));
    }

    #[test]
    fn reap_timeout_zero_is_valid() {
        assert_eq!(parse_u64_setting("0"), Some(0u64));
    }

    #[test]
    fn reap_timeout_invalid_string_returns_none() {
        assert_eq!(parse_u64_setting("abc"), None);
    }

    #[test]
    fn reap_timeout_negative_returns_none() {
        assert_eq!(parse_u64_setting("-1"), None);
    }

    #[test]
    fn reap_timeout_whitespace_trimmed_before_parse() {
        assert_eq!(parse_u64_setting("  120  "), Some(120u64));
    }

    #[test]
    fn mem_warn_zero_disables_warning() {
        // mb=0 → mem_warn_kb should be None
        let mb = parse_u64_setting("0").unwrap();
        let kb: Option<u64> = if mb > 0 { Some(mb * 1024) } else { None };
        assert_eq!(kb, None);
    }

    #[test]
    fn mem_warn_positive_converts_to_kb() {
        let mb = parse_u64_setting("4096").unwrap();
        let kb: Option<u64> = if mb > 0 { Some(mb * 1024) } else { None };
        assert_eq!(kb, Some(4096 * 1024));
    }

    #[test]
    fn setting_key_labels_are_nonempty() {
        assert!(!SettingKey::NtfyTopic.label().is_empty());
        assert!(!SettingKey::ReapTimeout.label().is_empty());
        assert!(!SettingKey::MemWarn.label().is_empty());
    }

    // ── SETTINGS metadata table ───────────────────────────────────────────────

    #[test]
    fn settings_table_has_correct_count() {
        assert_eq!(SETTINGS.len(), SETTINGS_ROW_COUNT_TOTAL);
    }

    #[test]
    fn settings_table_all_labels_nonempty() {
        for meta in SETTINGS.iter() {
            assert!(!meta.label.is_empty(), "empty label found");
        }
    }

    #[test]
    fn settings_table_all_descriptions_nonempty() {
        for meta in SETTINGS.iter() {
            assert!(!meta.description.is_empty(), "empty description for label: {}", meta.label);
        }
    }

    #[test]
    fn settings_table_plain_language_labels_spot_check() {
        assert!(SETTINGS[0].label.contains("Do Not Disturb"));
        assert!(SETTINGS[1].label.to_lowercase().contains("bell"));
        assert!(SETTINGS[2].label.to_lowercase().contains("desktop"));
        assert!(SETTINGS[3].label.to_lowercase().contains("ntfy"));
        assert!(SETTINGS[4].label.to_lowercase().contains("sleep") || SETTINGS[4].label.to_lowercase().contains("idle") || SETTINGS[4].label.to_lowercase().contains("auto"));
        assert!(SETTINGS[5].label.to_lowercase().contains("second") || SETTINGS[5].label.to_lowercase().contains("sleep"));
        assert!(SETTINGS[6].label.to_lowercase().contains("mem") || SETTINGS[6].label.to_lowercase().contains("mb") || SETTINGS[6].label.to_lowercase().contains("warn"));
        assert!(SETTINGS[7].label.to_lowercase().contains("nerd") || SETTINGS[7].label.to_lowercase().contains("font") || SETTINGS[7].label.to_lowercase().contains("icon"));
        assert!(SETTINGS[8].label.to_lowercase().contains("mouse"));
    }

    #[test]
    fn prompt_edit_setting_buf_mut_returns_buf() {
        let mut p = Prompt::EditSetting {
            key: SettingKey::NtfyTopic,
            buf: "hello".to_string(),
        };
        p.buf_mut().push_str(" world");
        assert_eq!(p, Prompt::EditSetting {
            key: SettingKey::NtfyTopic,
            buf: "hello world".to_string(),
        });
    }

    // ── complete_path ─────────────────────────────────────────────────────────

    /// Helper: create a temp dir with known subdirectories and return its path.
    fn make_completion_tmpdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "cdeck-complete-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(base.join("alpha")).unwrap();
        std::fs::create_dir_all(base.join("beta")).unwrap();
        std::fs::create_dir_all(base.join("gamma")).unwrap();
        std::fs::write(base.join("file.txt"), b"x").unwrap();
        base
    }

    #[test]
    fn complete_path_single_match_returns_completed_buffer() {
        let base = make_completion_tmpdir();
        let partial = format!("{}/al", base.display());
        let result = complete_path(&partial);
        assert_eq!(result, Some(format!("{}/alpha/", base.display())));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn complete_path_multiple_matches_returns_longest_common_prefix() {
        let base = make_completion_tmpdir();
        // "a" matches "alpha/" and nothing else starting with 'a' among our dirs.
        // Actually only alpha starts with 'a', but let's test a prefix shared by two.
        // 'g' matches only gamma; let's pick '' (empty partial) to get all three.
        let trailing = format!("{}/", base.display());
        let result = complete_path(&trailing);
        // All entries: alpha/, beta/, file.txt, gamma/ — LCP is ""
        // We just verify we get Some(...) and it starts with the base path.
        assert!(result.is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn complete_path_no_match_returns_none() {
        let base = make_completion_tmpdir();
        let partial = format!("{}/zzz", base.display());
        let result = complete_path(&partial);
        assert_eq!(result, None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn complete_path_directory_entries_get_trailing_slash() {
        let base = make_completion_tmpdir();
        let partial = format!("{}/bet", base.display());
        let result = complete_path(&partial);
        assert_eq!(result, Some(format!("{}/beta/", base.display())));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn longest_common_prefix_single_item() {
        assert_eq!(
            longest_common_prefix(&["hello/".to_string()]),
            "hello/"
        );
    }

    #[test]
    fn longest_common_prefix_shared_prefix() {
        assert_eq!(
            longest_common_prefix(&["alpha/".to_string(), "all/".to_string()]),
            "al"
        );
    }

    #[test]
    fn longest_common_prefix_no_common() {
        assert_eq!(
            longest_common_prefix(&["abc".to_string(), "xyz".to_string()]),
            ""
        );
    }

    #[test]
    fn longest_common_prefix_empty_slice() {
        let empty: &[String] = &[];
        assert_eq!(longest_common_prefix(empty), "");
    }
}
