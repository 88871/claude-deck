# Claude Deck — TUI Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A terminal application (TUI) — launched from the terminal, no window — that runs and manages multiple real `claude` CLI sessions, with a session sidebar and each session rendered live inside a pane.

**Architecture:** A single Rust binary. `ratatui` + `crossterm` draw the shell (sidebar + focused pane). Each session is the real `claude` binary in a `portable-pty` pseudo-terminal; its output feeds a per-session `vt100::Parser`, rendered into a pane with the `tui-term` `PseudoTerminal` widget. A unified `mpsc` event loop multiplexes crossterm input and PTY-output wakeups. This SUPERSEDES the Tauri foundation plan (`2026-07-18-claude-deck-foundation.md`); the Tauri/xterm shell is removed. The pane engine (`vt100`) is deliberately isolated so it can later be swapped for `wezterm-term` without touching the shell.

**Tech Stack:** Rust (stable), `ratatui`, `crossterm`, `portable-pty`, `vt100`, `tui-term`, `uuid`.

## Global Constraints

- **Terminal application only.** It runs inside the user's terminal (raw mode + alternate screen). There is NO window, NO WebView, NO GUI, NO web assets. (Pivot note in spec.)
- **Never call the Anthropic API or handle auth tokens.** Each session is the real `claude` CLI, which owns its own subscription auth. (Spec §2/§3)
- **Do not reimplement Claude Code.** Sessions are the real `claude` binary in a PTY. (Spec §1/§3)
- **Restore the terminal on every exit path** (normal quit, error, panic): leave raw mode and the alternate screen, or the user's terminal is left corrupted. Install a panic hook that restores first.
- **Set `TERM=xterm-256color`, `COLORTERM=truecolor`, `LANG=en_US.UTF-8`** on every spawned `claude` child (carried over from the Tauri build — without `TERM` the session renders black-and-white).
- **State enum** (`SessionState`) is the source of truth for sidebar status: `Starting · Running · WaitingOnYou · Idle · Parked · Closed · Error`. This plan drives it with a simple output-activity + exit heuristic; hook-driven precision and reaping are a later plan.
- **Pane-engine isolation:** all `vt100`/`tui-term` usage stays in the pane/render module so the emulator can be swapped later. No `vt100` types in the session-management or event-loop logic beyond the per-session parser handle.

---

### Task 1: Restructure to a Rust TUI binary + bare app loop

Remove the Tauri/JS shell, migrate the session model, and stand up a minimal ratatui app that draws an empty sidebar+main layout and quits cleanly.

**Files:**
- Create: `Cargo.toml` (repo root — binary crate `claude-deck`)
- Create: `src/main.rs`, `src/session.rs`, `src/app.rs`, `src/ui.rs`
- Delete: `src-tauri/` (whole dir), `index.html`, `package.json`, `pnpm-lock.yaml`, `vite.config.ts`, `tsconfig.json`, `src/main.ts`, `src/sessions.ts`, `src/styles.css`, `node_modules/` (if present)
- Migrate: `src-tauri/src/core/session.rs` → `src/session.rs`

**Interfaces:**
- Produces:
  - `enum SessionState { Starting, Running, WaitingOnYou, Idle, Parked, Closed, Error }` (derive `Clone, Copy, PartialEq, Eq, Debug`; drop the old serde derives — no serialization needed in a TUI).
  - `struct Session { id: String, label: String, cwd: PathBuf, state: SessionState }` and `struct SessionManager` with the SAME method set as the Tauri build (`new`, `create(cwd)->String`, `get`, `list()->Vec<Session>`, `set_state`, `remove`) — migrated with its unit tests intact. Add `#[derive(Default)]` on `SessionManager` (fixes the prior `clippy::new_without_default`).
  - `struct App { should_quit: bool }` (grows in later tasks) and `App::run(terminal) -> Result<()>` event loop.
  - `ui::draw(frame, app)` renders a two-region layout: left sidebar (width 26, bordered, title "SESSIONS"), right area (bordered, title "claude-deck"). Placeholder text for now.

- [ ] **Step 1: Remove the Tauri/JS shell**

```bash
cd ~/Desktop/claude-deck
git rm -r --quiet src-tauri index.html package.json pnpm-lock.yaml vite.config.ts tsconfig.json src/main.ts src/sessions.ts src/styles.css 2>/dev/null; true
rm -rf node_modules
```
(Keep `docs/`, `.git/`, `.gitignore`, `.superpowers/`. If a file above doesn't exist, ignore.)

- [ ] **Step 2: Create the root `Cargo.toml`**

```toml
[package]
name = "claude-deck"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "claude-deck"
path = "src/main.rs"

[dependencies]
ratatui = "0.29"
crossterm = "0.28"
portable-pty = "0.8"
vt100 = "0.15"
uuid = { version = "1", features = ["v4"] }
# tui-term must match the ratatui version — add via `cargo add tui-term`
# and let cargo resolve the compatible release (see Step 3).
```

- [ ] **Step 3: Add `tui-term` at a ratatui-0.29-compatible version**

Run: `cargo add tui-term`
Then `cargo tree -i ratatui` to confirm `tui-term` and the crate both resolve to the SAME `ratatui` 0.29.x (a mismatch causes "expected ratatui::… found ratatui::…" type errors). If `cargo add` picks a `tui-term` that pulls a different ratatui, pin `tui-term` to the release whose `Cargo.toml` requires `ratatui = "0.29"`.
Expected: `cargo tree -i ratatui` shows a single ratatui 0.29.x.

- [ ] **Step 4: Migrate `session.rs`**

Copy the body of the old `src-tauri/src/core/session.rs` into `src/session.rs` with these edits: remove `use serde::Serialize;`, remove `#[derive(Serialize)]` and `#[serde(rename_all = "camelCase")]` from `SessionState` and `Session`, add the `Closed` variant to `SessionState` (between `Parked` and `Error`), and add `#[derive(Default)]` to `SessionManager` (remove the hand-written `new` body only if it conflicts — keep `new()` returning `Self::default()`). Keep the four unit tests verbatim.

- [ ] **Step 5: Run the migrated tests**

Run: `cargo test session`
Expected: PASS (4 tests) — confirms the model migrated cleanly.

- [ ] **Step 6: Write the terminal setup + panic-safe teardown in `src/main.rs`**

```rust
mod app;
mod session;
mod ui;

use std::io::{self, Stdout};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use app::App;

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn init_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn main() -> io::Result<()> {
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
```

- [ ] **Step 7: Write the minimal app loop in `src/app.rs`**

```rust
use std::io;
use std::time::Duration;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crate::{ui, Tui};

pub struct App {
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self { should_quit: false }
    }

    pub fn run(&mut self, terminal: &mut Tui) -> io::Result<()> {
        while !self.should_quit {
            terminal.draw(|f| ui::draw(f, self))?;
            // Poll so the loop stays responsive; later tasks replace this with
            // an mpsc select over input + PTY output.
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                        self.should_quit = true;
                    }
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 8: Write the layout in `src/ui.rs`**

```rust
use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use crate::app::App;

pub fn draw(f: &mut Frame, _app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(0)])
        .split(f.area());

    let sidebar = Block::default().borders(Borders::ALL).title("SESSIONS");
    f.render_widget(Paragraph::new("no sessions\n\nq: quit").block(sidebar), chunks[0]);

    let main = Block::default().borders(Borders::ALL).title("claude-deck");
    f.render_widget(Paragraph::new("(no session selected)").block(main), chunks[1]);
}
```

- [ ] **Step 9: Build and verify**

Run: `cargo run`
Expected: the terminal enters an alternate screen showing a 26-wide "SESSIONS" sidebar and a "claude-deck" main pane; pressing `q` exits and the normal shell is fully restored (prompt intact, no raw-mode corruption). Then `cargo build` is clean.
(Headless note: a subagent can't drive an interactive TTY. If `cargo run` can't attach to a TTY in your environment, verify instead with `cargo build` + `cargo test` passing, and confirm by code inspection that teardown restores the terminal on all paths. Say which you did.)

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(tui): restructure to ratatui binary, migrate session model, bare app loop"
```

---

### Task 2: Embed one live `claude` session in a pane

Spawn the real `claude` in a PTY, feed its output into a `vt100` parser, render it with `tui-term`, forward keystrokes, and run everything under a unified event loop. A leader key (Ctrl-a) namespaces app commands so they don't collide with `claude`.

**Files:**
- Create: `src/pty.rs` (PTY spawn + reader thread → parser; migrated/adapted from the Tauri `pty.rs`)
- Create: `src/keys.rs` (crossterm `KeyEvent` → terminal input bytes)
- Modify: `src/app.rs` (mpsc event loop, one session, leader-key handling, input forwarding, resize)
- Modify: `src/ui.rs` (render the session's `vt100` screen into the main pane via `tui-term`)
- Modify: `Cargo.toml` if needed

**Interfaces:**
- Consumes: `SessionState` (Task 1), the `App` shell (Task 1).
- Produces:
  - `pty::resolve_claude_path() -> Option<String>` (login-shell probe, from the Tauri build).
  - `pty::PtySession { writer: Box<dyn Write + Send>, master: Box<dyn MasterPty + Send>, killer: Box<dyn ChildKiller + Send + Sync>, parser: Arc<Mutex<vt100::Parser>> }`.
  - `pty::spawn(claude_path, cwd, rows, cols, tx: Sender<AppEvent>) -> io::Result<PtySession>` — spawns `claude` with the color env, starts a reader thread that feeds `parser` and sends `AppEvent::Output`/`AppEvent::Exited{clean}`.
  - `enum AppEvent { Input(crossterm::event::Event), Output, Exited { clean: bool } }` (in `app.rs`).
  - `keys::encode(key: &KeyEvent) -> Option<Vec<u8>>` — maps keys to bytes (printable → UTF-8, Enter → `\r`, Backspace → `0x7f`, Tab → `\t`, Esc → `0x1b`, arrows/Home/End/PageUp/Down → CSI sequences, Ctrl-letter → control byte).

- [ ] **Step 1: Implement the key encoder in `src/keys.rs`**

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encode a key press into the bytes a PTY expects. Returns None for keys we
/// don't forward (e.g. the leader is handled before this is called).
pub fn encode(key: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let bytes = match key.code {
        KeyCode::Char(c) if ctrl => {
            // Ctrl-a..z -> 0x01..0x1a
            let lower = c.to_ascii_lowercase();
            if lower.is_ascii_alphabetic() {
                vec![(lower as u8 - b'a') + 1]
            } else {
                return None;
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        _ => return None,
    };
    Some(bytes)
}
```

- [ ] **Step 2: Implement `src/pty.rs`**

```rust
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use crate::app::AppEvent;

pub struct PtySession {
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
    #[allow(dead_code)] // used by later reaping/kill work
    pub killer: Box<dyn ChildKiller + Send + Sync>,
    pub parser: Arc<Mutex<vt100::Parser>>,
}

/// Login-shell probe for the `claude` binary (Finder/packaged launches get a
/// minimal PATH). Returns None if not found.
pub fn resolve_claude_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let out = Command::new(&shell).args(["-lc", "command -v claude"]).output().ok()?;
    if !out.status.success() { return None; }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() { None } else { Some(p) }
}

pub fn spawn(
    claude_path: &str,
    cwd: &Path,
    rows: u16,
    cols: u16,
    tx: Sender<AppEvent>,
) -> std::io::Result<PtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(to_io)?;

    let mut cmd = CommandBuilder::new(claude_path);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("LANG", "en_US.UTF-8");

    let child = pair.slave.spawn_command(cmd).map_err(to_io)?;
    drop(pair.slave);
    let killer = child.clone_killer();

    let mut reader = pair.master.try_clone_reader().map_err(to_io)?;
    let writer = pair.master.take_writer().map_err(to_io)?;
    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));

    // Reader thread: feed the vt100 parser and wake the UI on each chunk.
    let read_parser = parser.clone();
    let read_tx = tx.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    read_parser.lock().unwrap().process(&buf[..n]);
                    if read_tx.send(AppEvent::Output).is_err() { break; }
                }
            }
        }
    });

    // Waiter thread: report clean vs. abnormal exit.
    let mut child = child;
    std::thread::spawn(move || {
        let clean = child.wait().map(|s| s.success()).unwrap_or(false);
        let _ = tx.send(AppEvent::Exited { clean });
    });

    Ok(PtySession { writer, master: pair.master, killer, parser })
}

fn to_io<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}
```

- [ ] **Step 3: Rewrite the event loop in `src/app.rs` (single session)**

```rust
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
```

- [ ] **Step 4: Render the session in `src/ui.rs`**

```rust
use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_term::widget::PseudoTerminal;
use crate::app::App;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(0)])
        .split(f.area());

    let hint = if app.leader { "SESSIONS  [C-a]" } else { "SESSIONS" };
    let sidebar = Block::default().borders(Borders::ALL).title(hint);
    let label = app.session.as_ref()
        .and_then(|(id, _)| app.manager.get(id))
        .map(|s| format!("▸ {}  [{:?}]", s.label, s.state))
        .unwrap_or_else(|| "no sessions".to_string());
    f.render_widget(Paragraph::new(label).block(sidebar), chunks[0]);

    let main = Block::default().borders(Borders::ALL).title("claude-deck");
    if let Some((_, pty)) = &app.session {
        let parser = pty.parser.lock().unwrap();
        f.render_widget(PseudoTerminal::new(parser.screen()).block(main), chunks[1]);
    } else {
        f.render_widget(Paragraph::new("(no session — C-a q to quit)").block(main), chunks[1]);
    }
}
```

- [ ] **Step 5: Build and verify a live session**

Run: `cargo run`
Expected: the app launches straight into a live `claude` session rendered in the main pane, WITH colors; typing works, arrows/enter/backspace/Ctrl-C reach `claude`; the sidebar shows the session label + state; `Ctrl-a q` quits and restores the terminal cleanly. Then `cargo build` clean.
(Headless note: if no interactive TTY is available to the subagent, verify with `cargo build`/`cargo test` and inspect that: color env is set, the parser is fed on every read, keys are forwarded, resize updates both master and parser. State which you did; a human will confirm the live render.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): embed a live claude session in a pane (pty + vt100 + tui-term)"
```

---

### Task 3: Multi-session sidebar, switching, new-session prompt, kill

Generalize from one session to many: a sidebar list with state glyphs, leader-key commands to create (with a path prompt), switch, and kill sessions. Only the focused session renders in the main pane; all sessions keep parsing in the background.

**Files:**
- Modify: `src/app.rs` (hold `Vec`/map of sessions + `focused` index; leader commands `n`/`x`/digits/`[`/`]`; a path-input mode; route Output/Exited/resize per session)
- Modify: `src/ui.rs` (sidebar list with glyphs + highlight; render focused session; bottom input line when prompting)

**Interfaces:**
- Consumes: `SessionManager`, `pty::spawn`, `keys::encode`, `pane_dims`, `AppEvent` (Tasks 1–2).
- Produces:
  - `App.sessions: Vec<(String, PtySession)>` and `App.focused: usize` (replaces the single `session`).
  - `App.input: Option<String>` — Some(buffer) when the new-session path prompt is active.
  - Leader map: `n` → open path prompt; `x` → kill focused; `1..=9` → focus that session; `[` / `]` → focus prev/next; `q` → quit.
  - `SessionState` glyphs in `ui.rs`: `Starting ○ · Running ⏳ · WaitingOnYou ◍ · Idle ✓ · Parked ◌ · Closed ⏹ · Error ✗`.

- [ ] **Step 1: Convert `App` to multiple sessions**

Replace `session: Option<(String, PtySession)>` with `sessions: Vec<(String, PtySession)>` and `focused: usize`. Update `start_session` to `push` and set `focused` to the new index. Add helper `fn focused_id(&self) -> Option<String>` and `fn focused_pty(&mut self) -> Option<&mut PtySession>`. Route `AppEvent::Output` to a redraw only (all parsers are already fed by their own reader threads). For `AppEvent::Exited`, the waiter must identify WHICH session exited — change `AppEvent::Exited { clean }` to `AppEvent::Exited { id: String, clean: bool }` and have `pty::spawn` take the session `id` and include it in the event; set that session's state accordingly (do not remove it — leave it visible as Closed/Error until the user kills it).

```rust
// AppEvent
pub enum AppEvent { Input(crossterm::event::Event), Output, Exited { id: String, clean: bool } }
```
Update `pty::spawn(claude_path, cwd, rows, cols, id: String, tx)` to move `id` into the waiter thread: `let _ = tx.send(AppEvent::Exited { id, clean });`.

- [ ] **Step 2: Leader commands for switch / new / kill / quit**

In `on_key`, when `self.leader` is set, handle:
```rust
match key.code {
    KeyCode::Char('q') => self.should_quit = true,
    KeyCode::Char('n') => self.input = Some(std::env::current_dir()
        .map(|p| p.display().to_string()).unwrap_or_default()),
    KeyCode::Char('x') => self.kill_focused(),
    KeyCode::Char(c @ '1'..='9') => {
        let i = (c as u8 - b'1') as usize;
        if i < self.sessions.len() { self.focused = i; self.sync_focus_size(); }
    }
    KeyCode::Char('[') => { if !self.sessions.is_empty() {
        self.focused = (self.focused + self.sessions.len() - 1) % self.sessions.len(); self.sync_focus_size(); } }
    KeyCode::Char(']') => { if !self.sessions.is_empty() {
        self.focused = (self.focused + 1) % self.sessions.len(); self.sync_focus_size(); } }
    _ => {}
}
```
Add `kill_focused` (call `killer.kill()`, then `manager.remove(id)`, drop the `PtySession`, and clamp `focused`) and `sync_focus_size` (resize the newly focused session's master+parser to the current pane dims — sessions resized while unfocused may have a stale size).

- [ ] **Step 3: Path-input mode for new sessions**

When `self.input` is `Some`, `on_key` edits the buffer instead of forwarding to a session: printable chars append, Backspace pops, `Esc` cancels (`self.input = None`), `Enter` confirms — start a session at the typed path (if it exists as a dir) and clear `input`. Guard against a missing/invalid path by marking the new session `Error` or refusing with the prompt left open. Do not forward keystrokes to any PTY while `input.is_some()`.

- [ ] **Step 4: Sidebar list + focused render + input line in `src/ui.rs`**

Render the sidebar as one row per session: `"{glyph} {label}"`, the focused row highlighted (reversed style). Render the focused session's `parser.screen()` in the main pane (or a placeholder if none). When `app.input.is_some()`, draw a one-line input field at the bottom of the main area showing `new session path: {buffer}`.

```rust
fn glyph(state: crate::session::SessionState) -> &'static str {
    use crate::session::SessionState::*;
    match state { Starting => "○", Running => "⏳", WaitingOnYou => "◍",
        Idle => "✓", Parked => "◌", Closed => "⏹", Error => "✗" }
}
```

- [ ] **Step 5: Build and verify multi-session**

Run: `cargo run`
Expected: launches with one session; `Ctrl-a n` → path prompt → Enter starts a second session in that dir; the sidebar lists both with glyphs; `Ctrl-a 1/2`, `Ctrl-a [`/`]` switch the main pane between them (each keeps its own live screen); `Ctrl-a x` kills the focused one; `Ctrl-a q` quits with a clean terminal restore. Then `cargo build` clean.
(Headless note: same as prior tasks — if no TTY, verify build/tests + inspect the routing/kill/resize logic, and state what you did.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): multi-session sidebar with switching, new-session prompt, and kill"
```

---

## Self-Review

**Spec coverage (this plan's scope):**
- Terminal application, no window (Pivot note) → Task 1 (ratatui in alt-screen, no GUI/web). ✓
- Real `claude` in real panes (Spec §2/§4.1) → Task 2 (portable-pty + real binary). ✓
- Colors / terminal fidelity (carried fix) → Task 2 (TERM/COLORTERM/LANG env). ✓
- Sidebar with state glyphs, session switching, create/kill (Spec §4.3) → Task 3. ✓
- State enum source of truth (Spec §5) → Task 1 (migrated, `Closed` added). ✓ (heuristic drive; hooks later.)
- Clean auth (real CLI, no tokens/API) (Spec §2/§3) → Global Constraints; Task 2 spawns real `claude`. ✓
- Terminal restored on all exit paths → Task 1 panic hook + explicit restore. ✓
- Pane-engine isolation for future `wezterm-term` swap → `vt100`/`tui-term` confined to `pty.rs`/`ui.rs`. ✓
- **Deferred (later plan):** hook-driven precise states + reaping/`--resume` (Spec §4.2/§5/§6), scrollback/copy-mode, mouse. The confirmed CLI facts (`--session-id`/`--resume`/`--continue`/`--settings`, `--bare`) are recorded in `docs/superpowers/notes-cli-facts.md`.

**Placeholder scan:** No TBD/TODO. Task 1's poll-based loop is explicitly replaced by the mpsc loop in Task 2. ✓

**Type consistency:** `AppEvent` gains an `id` on `Exited` in Task 3 (Task 2 introduces the two-field form; Task 3's change is called out explicitly with the new signature). `SessionState` variants match the `glyph()` map exactly. `pane_dims` (subtract 26 sidebar + 2 border) is used for both initial spawn and resize. `pty::spawn` signature: Task 2 `(claude_path, cwd, rows, cols, tx)`; Task 3 adds `id` → `(claude_path, cwd, rows, cols, id, tx)` (flagged in Task 3 Step 1). ✓

**Known risks:**
- **`tui-term`/`ratatui` version compatibility** (Task 1 Step 3) — the most likely build snag; resolve versions before proceeding.
- **`vt100` fidelity for `claude`'s heavy TUI** — if mouse/exotic sequences or resize feel off, swap the pane engine to `wezterm-term` (isolated in `pty.rs`/`ui.rs`). This is the planned upgrade path, not a redesign.
- **Redraw coalescing** — a chatty session sends many `AppEvent::Output`s; each triggers a full `terminal.draw`. If CPU is high under heavy output, coalesce by draining the channel before drawing (drain all pending events, then draw once). Left simple here; optimize only if observed.
