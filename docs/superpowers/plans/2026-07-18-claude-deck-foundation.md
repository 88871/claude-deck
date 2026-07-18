# Claude Deck — Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a lean, terminal-native desktop app that runs and manages multiple real `claude` CLI sessions from one window with a session sidebar.

**Architecture:** A Tauri v2 app. A Rust core owns session state and PTYs (each session is the real `claude` binary in a pseudo-terminal). The WebView renders each session with xterm.js. Rust ⇄ WebView communicate over Tauri IPC: Rust emits PTY output as events; the frontend invokes commands to send keystrokes and resize. This plan delivers scaffold → one live session → multi-session + sidebar + folder picker. Precise hook-driven state and idle-reaping are a follow-on plan (they depend on CLI facts this plan's spike confirms).

**Tech Stack:** Rust (stable), Tauri v2, `portable-pty`, `tokio`, `serde`/`serde_json`, `uuid`; frontend TypeScript + Vite, `@xterm/xterm`, `@xterm/addon-fit`; `pnpm`.

## Global Constraints

- **Never call the Anthropic API directly and never handle auth tokens.** Sessions are the real `claude` CLI, which owns its own subscription auth. (Spec §2, §3)
- **Do not reimplement any Claude Code UI or feature.** The app is a shell around real `claude` processes. (Spec §1, §3)
- **Do not mutate the user's global `~/.claude/settings.json` or `CLAUDE.md`.** (Spec §4.2)
- **Claude only. No multi-provider, no remote/hosting, no auth management.** (Spec §3)
- **Terminal-native aesthetic:** no chat bubbles/buttons/web chrome. The WebView renders a terminal, not a web page. (Spec §2, §7)
- **State enum is the single source of truth** for session status, shared by core and UI: `Starting · Running · WaitingOnYou · Idle · Parked · Error`. (Spec §5) — this plan drives it with a simple output-activity heuristic; Plan 2 replaces the heuristic with Claude Code hooks.

---

### Task 1: Scaffold the Tauri app with an empty terminal

**Files:**
- Create: `package.json`, `pnpm-lock.yaml`, `vite.config.ts`, `tsconfig.json`, `index.html`
- Create: `src/main.ts`, `src/styles.css`
- Create: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/build.rs`
- Create: `.gitignore`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: a runnable Tauri app (`pnpm tauri dev`) showing a full-window xterm.js terminal (not yet wired to any process).

- [ ] **Step 1: Scaffold with the Tauri v2 CLI**

Run:
```bash
cd ~/Desktop/claude-deck
pnpm create tauri-app@latest . --template vanilla-ts --manager pnpm --yes
pnpm install
```
Expected: `src/` (frontend) and `src-tauri/` (Rust) directories exist; `pnpm tauri dev` opens a default window.

- [ ] **Step 2: Add xterm.js to the frontend**

Run:
```bash
pnpm add @xterm/xterm @xterm/addon-fit
```

- [ ] **Step 3: Replace `index.html` body with a terminal mount point**

`index.html` body:
```html
<body>
  <div id="app">
    <div id="terminal"></div>
  </div>
  <script type="module" src="/src/main.ts"></script>
</body>
```

- [ ] **Step 4: Render an empty xterm terminal in `src/main.ts`**

```typescript
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";

const term = new Terminal({
  fontFamily: "Menlo, monospace",
  fontSize: 13,
  cursorBlink: true,
  theme: { background: "#0b0b0e" },
});
const fit = new FitAddon();
term.loadAddon(fit);
term.open(document.getElementById("terminal")!);
fit.fit();
window.addEventListener("resize", () => fit.fit());
term.writeln("claude-deck: terminal ready");
```

- [ ] **Step 5: Make the terminal fill the window in `src/styles.css`**

```css
html, body, #app, #terminal { height: 100%; margin: 0; }
body { background: #0b0b0e; }
#terminal { padding: 6px; box-sizing: border-box; }
```

- [ ] **Step 6: Run the app and verify**

Run: `pnpm tauri dev`
Expected: a dark window opens showing `claude-deck: terminal ready`. No console errors.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: scaffold Tauri app with empty xterm terminal"
```

---

### Task 2: Session core model and state machine (Rust, TDD)

Pure logic, no I/O — the single source of truth for sessions and their states. Kept free of PTY/Tauri side effects so it is unit-testable.

**Files:**
- Create: `src-tauri/src/core/mod.rs`
- Create: `src-tauri/src/core/session.rs`
- Modify: `src-tauri/src/main.rs` (add `mod core;`)
- Modify: `src-tauri/Cargo.toml` (add `uuid`, `serde`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `enum SessionState { Starting, Running, WaitingOnYou, Idle, Parked, Error }` (derives `Serialize`, `Clone`, `Copy`, `PartialEq`, `Debug`)
  - `struct Session { id: String, label: String, cwd: PathBuf, state: SessionState }` (derives `Serialize`, `Clone`)
  - `struct SessionManager` with:
    - `fn new() -> Self`
    - `fn create(&mut self, cwd: PathBuf) -> String` (returns new id; label = final path component; initial state `Starting`)
    - `fn get(&self, id: &str) -> Option<&Session>`
    - `fn list(&self) -> Vec<Session>` (creation order)
    - `fn set_state(&mut self, id: &str, state: SessionState) -> bool` (false if unknown id)
    - `fn remove(&mut self, id: &str) -> bool`

- [ ] **Step 1: Add dependencies to `src-tauri/Cargo.toml`**

Under `[dependencies]`:
```toml
serde = { version = "1", features = ["derive"] }
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 2: Write the failing tests in `src-tauri/src/core/session.rs`**

```rust
use std::path::PathBuf;
use serde::Serialize;
use std::collections::HashMap;

// (types + impl go here in Step 4)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_registers_session_with_label_from_dir() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/Users/m/Desktop/hp-app"));
        let s = m.get(&id).expect("session should exist");
        assert_eq!(s.label, "hp-app");
        assert_eq!(s.state, SessionState::Starting);
    }

    #[test]
    fn list_preserves_creation_order() {
        let mut m = SessionManager::new();
        let a = m.create(PathBuf::from("/tmp/a"));
        let b = m.create(PathBuf::from("/tmp/b"));
        let ids: Vec<String> = m.list().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![a, b]);
    }

    #[test]
    fn set_state_updates_known_session_and_rejects_unknown() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/tmp/a"));
        assert!(m.set_state(&id, SessionState::Running));
        assert_eq!(m.get(&id).unwrap().state, SessionState::Running);
        assert!(!m.set_state("nope", SessionState::Running));
    }

    #[test]
    fn remove_deletes_session() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/tmp/a"));
        assert!(m.remove(&id));
        assert!(m.get(&id).is_none());
        assert!(!m.remove(&id));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd src-tauri && cargo test core::session`
Expected: FAIL — `SessionManager` / `SessionState` not found.

- [ ] **Step 4: Implement the types above the `#[cfg(test)]` block**

```rust
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SessionState {
    Starting,
    Running,
    WaitingOnYou,
    Idle,
    Parked,
    Error,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub label: String,
    pub cwd: PathBuf,
    pub state: SessionState,
}

pub struct SessionManager {
    sessions: HashMap<String, Session>,
    order: Vec<String>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self { sessions: HashMap::new(), order: Vec::new() }
    }

    pub fn create(&mut self, cwd: PathBuf) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let label = cwd
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session".to_string());
        self.sessions.insert(
            id.clone(),
            Session { id: id.clone(), label, cwd, state: SessionState::Starting },
        );
        self.order.push(id.clone());
        id
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    pub fn list(&self) -> Vec<Session> {
        self.order.iter().filter_map(|id| self.sessions.get(id).cloned()).collect()
    }

    pub fn set_state(&mut self, id: &str, state: SessionState) -> bool {
        match self.sessions.get_mut(id) {
            Some(s) => { s.state = state; true }
            None => false,
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.order.retain(|x| x != id);
        self.sessions.remove(id).is_some()
    }
}
```

- [ ] **Step 5: Create `src-tauri/src/core/mod.rs`**

```rust
pub mod session;
```

- [ ] **Step 6: Register the module in `src-tauri/src/main.rs`**

Add near the top (after any existing attributes):
```rust
mod core;
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cd src-tauri && cargo test core::session`
Expected: PASS (4 tests).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: session core model and state machine with tests"
```

---

### Task 3: Spawn one real `claude` session in a PTY and stream it to xterm

Wires the Rust core to a live pseudo-terminal running the real `claude` binary, and streams it bidirectionally to xterm.js. Delivered as a single session (id fixed for now); Task 4 generalizes to many.

**Files:**
- Create: `src-tauri/src/pty.rs`
- Modify: `src-tauri/src/main.rs` (state, commands, event emission, `mod pty;`)
- Modify: `src-tauri/Cargo.toml` (add `portable-pty`, `tauri` event feature already present)
- Modify: `src/main.ts` (wire xterm input/output to Tauri IPC)

**Interfaces:**
- Consumes: `SessionManager`, `SessionState` (Task 2).
- Produces:
  - Tauri command `start_session(cwd: String) -> Result<String, String>` — spawns `claude` in `cwd`, returns session id.
  - Tauri command `write_to_pty(id: String, data: String) -> Result<(), String>`.
  - Tauri command `resize_pty(id: String, cols: u16, rows: u16) -> Result<(), String>`.
  - Tauri event `pty://data` with payload `{ id: String, chunk: String }` (UTF-8 lossy).
  - Tauri event `session://state` with payload `{ id: String, state: SessionState }`.
  - Rust struct `PtyHandle { writer: Box<dyn Write + Send>, master: Box<dyn MasterPty + Send> }` held per session in shared state.

- [ ] **Step 1: Add `portable-pty` to `src-tauri/Cargo.toml`**

```toml
portable-pty = "0.8"
```

- [ ] **Step 2: Implement the PTY spawn helper in `src-tauri/src/pty.rs`**

```rust
use std::io::{Read, Write};
use std::path::Path;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::{AppHandle, Emitter};

pub struct PtyHandle {
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
}

/// Spawn `claude` in `cwd`, streaming output to the frontend as `pty://data`
/// events tagged with `id`. Returns a handle for writing input and resizing.
pub fn spawn_claude(app: AppHandle, id: String, cwd: &Path) -> Result<PtyHandle, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new("claude");
    cmd.cwd(cwd);

    let _child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let emit_id = id.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app.emit("pty://data", serde_json::json!({
                        "id": emit_id, "chunk": chunk,
                    }));
                }
            }
        }
    });

    Ok(PtyHandle { writer, master: pair.master })
}
```

- [ ] **Step 3: Wire shared state, commands, and `mod pty;` in `src-tauri/src/main.rs`**

```rust
mod core;
mod pty;

use std::collections::HashMap;
use std::sync::Mutex;
use std::path::PathBuf;
use core::session::{SessionManager, SessionState};
use pty::{spawn_claude, PtyHandle};
use portable_pty::PtySize;
use std::io::Write;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct AppState {
    manager: Mutex<SessionManager>,
    ptys: Mutex<HashMap<String, PtyHandle>>,
}

fn emit_state(app: &AppHandle, id: &str, state: SessionState) {
    let _ = app.emit("session://state", serde_json::json!({ "id": id, "state": state }));
}

#[tauri::command]
fn start_session(app: AppHandle, state: State<AppState>, cwd: String) -> Result<String, String> {
    let path = PathBuf::from(&cwd);
    let id = { state.manager.lock().unwrap().create(path.clone()) };
    let handle = spawn_claude(app.clone(), id.clone(), &path)?;
    state.ptys.lock().unwrap().insert(id.clone(), handle);
    {
        let mut m = state.manager.lock().unwrap();
        m.set_state(&id, SessionState::Running);
    }
    emit_state(&app, &id, SessionState::Running);
    Ok(id)
}

#[tauri::command]
fn write_to_pty(state: State<AppState>, id: String, data: String) -> Result<(), String> {
    let mut ptys = state.ptys.lock().unwrap();
    let handle = ptys.get_mut(&id).ok_or("unknown session")?;
    handle.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    handle.writer.flush().map_err(|e| e.to_string())
}

#[tauri::command]
fn resize_pty(state: State<AppState>, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let ptys = state.ptys.lock().unwrap();
    let handle = ptys.get(&id).ok_or("unknown session")?;
    handle.master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![start_session, write_to_pty, resize_pty])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 4: Add `serde_json` to `src-tauri/Cargo.toml`**

```toml
serde_json = "1"
```

- [ ] **Step 5: Wire xterm ⇄ IPC in `src/main.ts`**

Replace the placeholder body with input/output wiring (keep the terminal setup from Task 1):
```typescript
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ...existing term + fit setup from Task 1...

let sessionId: string | null = null;

listen<{ id: string; chunk: string }>("pty://data", (e) => {
  if (e.payload.id === sessionId) term.write(e.payload.chunk);
});

term.onData((data) => {
  if (sessionId) invoke("write_to_pty", { id: sessionId, data });
});

function syncSize() {
  fit.fit();
  if (sessionId) invoke("resize_pty", { id: sessionId, cols: term.cols, rows: term.rows });
}
window.addEventListener("resize", syncSize);

// Temporary: start one session in the home directory on load.
(async () => {
  sessionId = await invoke<string>("start_session", { cwd: (await import("@tauri-apps/api/path")).homeDir ? await (await import("@tauri-apps/api/path")).homeDir() : "." });
  syncSize();
})();
```

- [ ] **Step 6: Verify `claude` is on PATH**

Run: `which claude`
Expected: a path prints. If not, the session will error — install/login to Claude Code first (`claude` interactive once) before testing.

- [ ] **Step 7: Run the app and verify a live session**

Run: `pnpm tauri dev`
Expected: the window shows the real `claude` TUI starting up; typing a prompt and pressing enter gets a real response streamed into the pane; native scroll and text selection work.

- [ ] **Step 8: Spike-validate CLI facts for Plan 2 (record findings, do not implement)**

Run and note results in `docs/superpowers/notes-cli-facts.md`:
```bash
claude --help | grep -E "session-id|resume|continue|settings"
```
Record: does `--session-id <uuid>` exist? `--resume <id>`? `--continue`? `--settings <json|file>`? These determine Plan 2 (hooks + reaping). Also run one session, trigger a permission prompt, and note whether Claude Code emits hook events (for Plan 2's state bridge).

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: spawn real claude in a PTY and stream to xterm"
```

---

### Task 4: Multi-session sidebar with folder picker and session switching

Generalizes from one hard-coded session to many, adds the sidebar UI, the "＋ new session" folder picker, and switching (each session keeps its own xterm buffer).

**Files:**
- Modify: `index.html` (sidebar + terminal layout)
- Modify: `src/styles.css` (sidebar layout, state glyphs)
- Create: `src/sessions.ts` (per-session terminal registry + rendering)
- Modify: `src/main.ts` (bootstrap sidebar, remove temporary auto-start)
- Modify: `src-tauri/Cargo.toml` (add `tauri-plugin-dialog`)
- Modify: `src-tauri/src/main.rs` (register dialog plugin; add `list_sessions` command)
- Modify: `src-tauri/tauri.conf.json` / capabilities (allow dialog)

**Interfaces:**
- Consumes: `start_session`, `write_to_pty`, `resize_pty`, `pty://data`, `session://state` (Task 3); `SessionManager::list` (Task 2).
- Produces:
  - Tauri command `list_sessions() -> Vec<Session>`.
  - Frontend module `sessions.ts` exporting:
    - `openSession(cwd: string): Promise<string>` (starts backend session + creates its xterm buffer)
    - `focusSession(id: string): void` (shows that session's terminal, hides others)
    - `setSidebarState(id: string, state: string): void`
    - `renderSidebar(sessions: {id,label,state}[]): void`

- [ ] **Step 1: Add the dialog plugin (Rust)**

Run:
```bash
cd src-tauri && cargo add tauri-plugin-dialog && cd ..
pnpm add @tauri-apps/plugin-dialog
```

- [ ] **Step 2: Register the plugin and add `list_sessions` in `src-tauri/src/main.rs`**

In `main()` builder chain, before `.run(...)`:
```rust
.plugin(tauri_plugin_dialog::init())
```
Add command:
```rust
#[tauri::command]
fn list_sessions(state: State<AppState>) -> Vec<core::session::Session> {
    state.manager.lock().unwrap().list()
}
```
And add `list_sessions` to `generate_handler![...]`.

- [ ] **Step 3: Two-pane layout in `index.html`**

```html
<body>
  <div id="app">
    <aside id="sidebar">
      <button id="new-session">＋ new session</button>
      <ul id="session-list"></ul>
    </aside>
    <main id="terminals"></main>
  </div>
  <script type="module" src="/src/main.ts"></script>
</body>
```

- [ ] **Step 4: Sidebar + terminal styling in `src/styles.css`**

```css
html, body, #app { height: 100%; margin: 0; }
#app { display: flex; background: #0b0b0e; color: #d6d6dd; font-family: Menlo, monospace; }
#sidebar { width: 220px; border-right: 1px solid #1c1c22; padding: 8px; box-sizing: border-box; }
#new-session { width: 100%; background: #16161c; color: #d6d6dd; border: 1px solid #2a2a33; border-radius: 6px; padding: 6px; cursor: pointer; }
#session-list { list-style: none; margin: 8px 0 0; padding: 0; }
.session-row { display: flex; gap: 6px; align-items: center; padding: 6px; border-radius: 6px; cursor: pointer; font-size: 12px; }
.session-row.active { background: #16161c; }
.glyph { width: 1em; }
#terminals { flex: 1; position: relative; }
.term-pane { position: absolute; inset: 0; padding: 6px; box-sizing: border-box; }
.term-pane.hidden { display: none; }
```

- [ ] **Step 5: Per-session terminal registry in `src/sessions.ts`**

```typescript
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import "@xterm/xterm/css/xterm.css";

interface Pane { term: Terminal; fit: FitAddon; el: HTMLElement; }
const panes = new Map<string, Pane>();
const glyphs: Record<string, string> = {
  starting: "○", running: "⏳", waitingOnYou: "◍", idle: "✓", parked: "◌", error: "✗",
};
let activeId: string | null = null;

export async function openSession(cwd: string): Promise<string> {
  const id = await invoke<string>("start_session", { cwd });
  const el = document.createElement("div");
  el.className = "term-pane hidden";
  document.getElementById("terminals")!.appendChild(el);

  const term = new Terminal({ fontFamily: "Menlo, monospace", fontSize: 13, cursorBlink: true, theme: { background: "#0b0b0e" } });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(el);
  term.onData((data) => invoke("write_to_pty", { id, data }));
  panes.set(id, { term, fit, el });
  focusSession(id);
  return id;
}

export function writeToSession(id: string, chunk: string) {
  panes.get(id)?.term.write(chunk);
}

export function focusSession(id: string) {
  activeId = id;
  for (const [pid, p] of panes) p.el.classList.toggle("hidden", pid !== id);
  const p = panes.get(id);
  if (p) {
    p.fit.fit();
    invoke("resize_pty", { id, cols: p.term.cols, rows: p.term.rows });
    p.term.focus();
  }
  for (const row of document.querySelectorAll(".session-row"))
    row.classList.toggle("active", (row as HTMLElement).dataset.id === id);
}

export function renderSidebar(sessions: { id: string; label: string; state: string }[]) {
  const list = document.getElementById("session-list")!;
  list.innerHTML = "";
  for (const s of sessions) {
    const li = document.createElement("li");
    li.className = "session-row" + (s.id === activeId ? " active" : "");
    li.dataset.id = s.id;
    li.innerHTML = `<span class="glyph">${glyphs[s.state] ?? "○"}</span><span>${s.label}</span>`;
    li.addEventListener("click", () => focusSession(s.id));
    list.appendChild(li);
  }
}

export function setSidebarState(id: string, state: string) {
  const row = document.querySelector<HTMLElement>(`.session-row[data-id="${id}"] .glyph`);
  if (row) row.textContent = glyphs[state] ?? "○";
}

export function activeSession() { return activeId; }
```

- [ ] **Step 6: Bootstrap in `src/main.ts` (replace prior contents)**

```typescript
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./styles.css";
import { openSession, writeToSession, setSidebarState, renderSidebar, activeSession } from "./sessions";

async function refreshSidebar() {
  const sessions = await invoke<{ id: string; label: string; state: string }[]>("list_sessions");
  renderSidebar(sessions);
}

listen<{ id: string; chunk: string }>("pty://data", (e) => writeToSession(e.payload.id, e.payload.chunk));
listen<{ id: string; state: string }>("session://state", (e) => setSidebarState(e.payload.id, e.payload.state));

document.getElementById("new-session")!.addEventListener("click", async () => {
  const dir = await open({ directory: true, multiple: false });
  if (typeof dir === "string") {
    await openSession(dir);
    await refreshSidebar();
  }
});
```

- [ ] **Step 7: Allow the dialog capability**

In `src-tauri/capabilities/default.json`, ensure the permissions array includes:
```json
"dialog:allow-open"
```

- [ ] **Step 8: Run and verify multi-session behavior**

Run: `pnpm tauri dev`
Expected:
- Click "＋ new session" → OS folder picker → pick a repo → a `claude` session starts in it and appears in the sidebar with a state glyph.
- Add a second session in a different folder; both appear in the sidebar.
- Clicking a sidebar row switches the terminal to that session; each session keeps its own scrollback.
- Typing goes to the focused session only.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: multi-session sidebar with folder picker and switching"
```

---

## Self-Review

**Spec coverage (this plan's scope = Spec milestones 1–2 + foundation):**
- Run real `claude` in real PTY panes (Spec §2, §4.1) → Task 3. ✓
- One UI runtime for all sessions (Spec §6 lever 1) → Task 4 (single window, per-session buffers). ✓
- Sidebar with state glyphs, folder picker / "select my own path", switching (Spec §4.3) → Task 4. ✓
- State enum as shared source of truth (Spec §5) → Task 2. ✓ (heuristic drive now; hook-driven precision deferred to Plan 2.)
- Terminal-native aesthetic, no web chrome (Spec §2, §7) → Tasks 1 & 4 styling. ✓
- Clean auth (never touch tokens/API) (Spec §2, §3) → Global Constraints; Task 3 spawns real `claude`. ✓
- **Deferred to Plan 2 (correctly out of scope here):** hook bridge + precise state (Spec §4.2, §5), reaping/`--resume` (Spec §6 lever 2), error-state polish (Spec §9). Task 3 Step 8 gathers the CLI facts Plan 2 needs.

**Placeholder scan:** No TBD/TODO. The only intentionally temporary code (Task 3 auto-start-one-session) is explicitly replaced in Task 4 Step 6. ✓

**Type consistency:** `SessionState` variants (`Starting/Running/WaitingOnYou/Idle/Parked/Error`) serialize camelCase and match the frontend `glyphs` keys (`starting/running/waitingOnYou/idle/parked/error`). Command names (`start_session`, `write_to_pty`, `resize_pty`, `list_sessions`) and event names (`pty://data`, `session://state`) are identical across Rust and TS. ✓

**Note on `--session-id`:** This plan deliberately does *not* pass `--session-id` to `claude` (Task 3 uses a plain spawn) because that flag is unverified; Task 3 Step 8 confirms it, and Plan 2 adopts it for resume/correlation once confirmed.
