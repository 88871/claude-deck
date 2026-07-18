# Claude Deck — Awareness Phase 1a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Drive the sidebar from real Claude Code hook events (running / waiting-on-you / idle), and surface sessions needing attention via bell + macOS notification + `Ctrl-a !` jump-to-attention.

**Architecture:** Each `claude` session is spawned with `--session-id <uuid> --settings <shared file>`; the settings file registers hooks that invoke our own binary (`claude-deck __hook <socket>`) as a forwarder. The forwarder pipes each hook's JSON to a Unix socket claude-deck listens on; a listener thread turns them into `AppEvent::Hook`, matched to a session by `session_id`, driving the state machine. (Verified end-to-end against real `claude`; see the spec.)

**Tech Stack:** Rust, existing `ratatui`/`crossterm`/`portable-pty`/`vt100`; add `serde` + `serde_json` for hook-payload parsing and settings generation; `uuid` (already present).

## Global Constraints

- **Never modify the user's global `~/.claude/settings.json`.** Inject hooks only via a temp `--settings` file that Claude merges non-destructively.
- **Never call the Anthropic API / handle tokens.** Sessions are the real `claude`.
- **Hook command must use the ABSOLUTE path** to our binary (`std::env::current_exe()`), because hooks don't source shell profiles.
- **Never use `--bare`** (it skips hooks).
- **Terminal restored on all exit paths** (unchanged); also remove the socket + settings temp files on exit (best-effort on panic).
- **The `__hook` subcommand must never start the TUI** — it reads stdin, forwards to the socket, exits.
- State enum is unchanged: `Starting/Running/WaitingOnYou/Idle/Parked/Closed/Error`.

---

### Task A1: `__hook` forwarder subcommand + settings/socket generation

**Files:**
- Create: `src/hooks.rs`
- Modify: `src/lib.rs` (dispatch `__hook` before the TUI; `mod hooks;`)

**Interfaces:**
- Produces:
  - `hooks::paths() -> (socket_path: PathBuf, settings_path: PathBuf)` — both under `std::env::temp_dir()`, named `claude-deck-<pid>.sock` / `claude-deck-<pid>-settings.json`.
  - `hooks::write_settings_file(settings_path, socket_path) -> io::Result<()>` — writes the shared hooks JSON; each hook command is `"<abs-binary> __hook <socket_path>"` using `std::env::current_exe()`. Registers `SessionStart`, `UserPromptSubmit`, `Notification`, `Stop`, each with `matcher: ""`.
  - `hooks::forward(socket_path: &str) -> io::Result<()>` — read ALL of stdin, connect to the unix socket, write the bytes, flush, close. (Errors are swallowed to exit 0 — a failed forward must never break the claude session.)
  - In `lib.rs::run()`: at the very top, if `std::env::args().nth(1).as_deref() == Some("__hook")`, call `hooks::forward(&args[2])` and return `Ok(())` WITHOUT initializing the terminal.

- [ ] **Step 1: Add deps** — in `Cargo.toml` ensure `serde = { version = "1", features = ["derive"] }` and `serde_json = "1"`.

- [ ] **Step 2: Write the failing test for settings generation** in `src/hooks.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_json_registers_hooks_pointing_at_our_binary() {
        let json = settings_json("/abs/claude-deck", "/tmp/x.sock");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        for ev in ["SessionStart", "UserPromptSubmit", "Notification", "Stop"] {
            let cmd = v["hooks"][ev][0]["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.contains("/abs/claude-deck"), "{ev} cmd = {cmd}");
            assert!(cmd.contains("__hook"), "{ev}");
            assert!(cmd.contains("/tmp/x.sock"), "{ev}");
            assert_eq!(v["hooks"][ev][0]["hooks"][0]["type"], "command");
        }
    }
}
```

- [ ] **Step 3: Run it — expect fail** (`cargo test hooks::` → `settings_json` not found).

- [ ] **Step 4: Implement `src/hooks.rs`:**

```rust
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub fn paths() -> (PathBuf, PathBuf) {
    let pid = std::process::id();
    let dir = std::env::temp_dir();
    (dir.join(format!("claude-deck-{pid}.sock")),
     dir.join(format!("claude-deck-{pid}-settings.json")))
}

/// Build the shared settings JSON. Each hook invokes our own binary as a
/// forwarder using an ABSOLUTE path (hooks don't source shell profiles).
pub fn settings_json(binary: &str, socket: &str) -> String {
    let cmd = format!("{binary} __hook {socket}");
    let entry = serde_json::json!([{ "matcher": "", "hooks": [{ "type": "command", "command": cmd }] }]);
    serde_json::json!({
        "hooks": {
            "SessionStart": entry, "UserPromptSubmit": entry,
            "Notification": entry, "Stop": entry,
        }
    }).to_string()
}

pub fn write_settings_file(settings_path: &std::path::Path, socket: &str) -> std::io::Result<()> {
    let binary = std::env::current_exe()?.to_string_lossy().into_owned();
    std::fs::write(settings_path, settings_json(&binary, socket))
}

/// `claude-deck __hook <socket>`: read the hook payload from stdin and forward
/// it to the app's socket. Never fails loudly — a broken forward must not break
/// the claude session.
pub fn forward(socket_path: &str) -> std::io::Result<()> {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    if let Ok(mut stream) = UnixStream::connect(socket_path) {
        let _ = stream.write_all(&input);
        let _ = stream.flush();
    }
    Ok(())
}
```

- [ ] **Step 5: Dispatch `__hook` in `src/lib.rs::run()`** — at the very top, before terminal init:

```rust
pub mod hooks; // add alongside other pub mod lines

pub fn run() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("__hook") {
        let sock = args.get(2).cloned().unwrap_or_default();
        return hooks::forward(&sock);
    }
    // ...existing terminal setup + App::run...
}
```

- [ ] **Step 6: Run tests + build** — `cargo test hooks::` passes; `cargo build` clean; both binaries still build.

- [ ] **Step 7: Commit** — `feat(awareness): __hook forwarder subcommand + settings/socket generation`.

---

### Task A2: Socket listener → `AppEvent::Hook`

**Files:**
- Modify: `src/hooks.rs` (add `HookEvent` + `listen`)
- Modify: `src/app.rs` (`AppEvent::Hook`, start the listener, cleanup)

**Interfaces:**
- Consumes: `AppEvent` sender (Task A2 wires it), `hooks::paths` (A1).
- Produces:
  - `hooks::HookEvent { session_id: String, event: String, notification_type: Option<String> }` (deserialized from the payload; extra fields ignored via serde).
  - `hooks::listen(socket_path: PathBuf, tx: Sender<AppEvent>)` — binds a `UnixListener`, spawns a thread; per accepted connection, read all bytes, parse JSON → `HookEvent`, `tx.send(AppEvent::Hook(ev))`. Bad/unparseable payloads are dropped silently.
  - `App` gains `AppEvent::Hook(hooks::HookEvent)`.

- [ ] **Step 1: Failing test — payload parses to HookEvent** in `hooks.rs`:

```rust
#[test]
fn parses_real_hook_payloads() {
    let ups = r#"{"session_id":"abc","cwd":"/x","hook_event_name":"UserPromptSubmit","prompt":"hi"}"#;
    let e: HookEvent = serde_json::from_str(ups).unwrap();
    assert_eq!(e.session_id, "abc");
    assert_eq!(e.event, "UserPromptSubmit");
    let notif = r#"{"session_id":"abc","hook_event_name":"Notification","notification_type":"permission_prompt"}"#;
    let n: HookEvent = serde_json::from_str(notif).unwrap();
    assert_eq!(n.notification_type.as_deref(), Some("permission_prompt"));
}
```

- [ ] **Step 2: Run — expect fail.**

- [ ] **Step 3: Implement `HookEvent` + `listen`:**

```rust
use serde::Deserialize;
use std::os::unix::net::UnixListener;
use std::sync::mpsc::Sender;

#[derive(Deserialize, Debug, Clone)]
pub struct HookEvent {
    pub session_id: String,
    #[serde(rename = "hook_event_name")]
    pub event: String,
    #[serde(default)]
    pub notification_type: Option<String>,
}

pub fn listen<F: Fn(HookEvent) + Send + 'static>(socket_path: PathBuf, on_event: F) -> std::io::Result<()> {
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let mut buf = Vec::new();
            if std::io::Read::read_to_end(&mut stream, &mut buf).is_ok() {
                if let Ok(ev) = serde_json::from_slice::<HookEvent>(&buf) {
                    on_event(ev);
                }
            }
        }
    });
    Ok(())
}
```
(The `on_event` closure captures a cloned `Sender<AppEvent>` and does `let _ = tx.send(AppEvent::Hook(ev));` — keeps `hooks.rs` free of the `AppEvent` type.)

- [ ] **Step 4: Wire in `app.rs`** — add `Hook(hooks::HookEvent)` to `AppEvent`; in `App::run` (or `new`), call `hooks::listen(socket_path, move |ev| { let _ = tx.send(AppEvent::Hook(ev)); })`. Handle `AppEvent::Hook` in the loop (Task A3 fills the body — for now, match it and do nothing / redraw). On quit, `std::fs::remove_file` the socket + settings paths.

- [ ] **Step 5: Tests + build pass. Commit** — `feat(awareness): unix socket listener → AppEvent::Hook`.

---

### Task A3: Spawn with `--session-id`/`--settings` + hook→state machine

**Files:**
- Modify: `src/session.rs` (`create` accepts a provided id)
- Modify: `src/pty.rs` (`spawn` adds `--session-id` + `--settings` args)
- Modify: `src/app.rs` (generate uuid, pass settings/socket paths, write settings file at startup, handle `AppEvent::Hook`)

**Interfaces:**
- Consumes: `HookEvent` (A2), `SessionState`.
- Produces:
  - `SessionManager::create_with_id(id: String, cwd: PathBuf)` (or change `create` to take an id) — session id equals the claude `--session-id`.
  - `pty::spawn(claude_path, cwd, rows, cols, id, settings_path: &str, tx)` — appends `cmd.args(["--session-id", id, "--settings", settings_path])`.
  - `app::state_for_hook(ev: &HookEvent) -> Option<SessionState>` (pure, testable):
    - `SessionStart` → `Starting`; `UserPromptSubmit` → `Running`; `Stop` → `Idle`;
    - `Notification` → `WaitingOnYou` if `notification_type` != `Some("idle_prompt")`, else `Idle`;
    - anything else → `None`.

- [ ] **Step 1: Failing test for `state_for_hook`** (all rows above, incl. `Notification` with `permission_prompt`→WaitingOnYou, `idle_prompt`→Idle, unknown type→WaitingOnYou, unknown event→None).

- [ ] **Step 2: Run — expect fail. Implement `state_for_hook`.**

- [ ] **Step 3: Thread the uuid + settings** — in `App::new`/startup: `hooks::paths()`, `hooks::write_settings_file(...)`, start the listener. In `start_session`: `let id = uuid::Uuid::new_v4().to_string();` pass to `create_with_id(id.clone(), cwd)` and `pty::spawn(..., &id, &settings_path, tx)`.

- [ ] **Step 4: Handle `AppEvent::Hook`** — `if let Some(state) = state_for_hook(&ev) { self.manager.set_state(&ev.session_id, state); }` (set_state already no-ops unknown ids). Redraw.

- [ ] **Step 5: Tests + build pass. Commit** — `feat(awareness): correlate sessions via --session-id and drive states from hooks`.

---

### Task A4: Attention — bell, macOS notification, throttle, jump-to-attention

**Files:**
- Modify: `src/app.rs` (transition detection + bell + notify + `Ctrl-a !`)
- Create: `src/notify.rs` (macOS notification + bell helpers + flags)

**Interfaces:**
- Consumes: `state_for_hook`, `Focus`, sessions (A3).
- Produces:
  - `notify::bell()` — writes `\x07` to stdout and flushes.
  - `notify::desktop(label: &str)` — spawns a detached `osascript -e 'display notification "<label> needs you" with title "claude-deck"'` (ignore errors; escape quotes in label).
  - `App` fields `bell_on: bool`, `notify_on: bool` (default true; `--no-bell` / `--no-notify` args flip them — parse in `App::new`).
  - `App::jump_to_attention()` — sets focus to the next `WaitingOnYou` session after the current focus (wrapping); no-op if none.
  - Leader `Ctrl-a !` → `jump_to_attention()`.

- [ ] **Step 1: Failing test — jump target selection** (given sessions with states, focus advances to the next WaitingOnYou, wraps, and is a no-op when none). Factor the target-picking into a pure `fn next_attention(sessions_states: &[SessionState], from: usize) -> Option<usize>` and test it.

- [ ] **Step 2: Run — expect fail. Implement `next_attention` + `jump_to_attention` + the leader key.**

- [ ] **Step 3: Transition-triggered attention** — in the `AppEvent::Hook` handler, capture the session's OLD state before `set_state`; if it changed TO `WaitingOnYou` AND that session is NOT the focused one, call `notify::bell()` (if `bell_on`) and `notify::desktop(label)` (if `notify_on`). Only on the transition edge (old != WaitingOnYou), never repeated.

- [ ] **Step 4: Implement `src/notify.rs`** (bell + `osascript` spawn with label escaping). Add `mod notify;`.

- [ ] **Step 5: Tests + build pass. Commit** — `feat(awareness): bell + macOS notification on attention, Ctrl-a! jump-to-attention`.

---

### Task A5: Integration check against real `claude` (verify, don't just unit-test)

- [ ] **Step 1:** Extend the PTY harness (or a one-off script) to: start the app's socket listener path logic in isolation OR run the built binary and, in a trusted dir, spawn a `claude --session-id <uuid> --settings <generated file>` and assert the socket receives `SessionStart`→`UserPromptSubmit`→`Stop` for that `session_id` when a prompt is submitted. (This mirrors the verified spike; confirm the *generated* settings file works, not a hand-written one.) Record the result in the task report. If a full run isn't feasible headlessly, assert the generated settings file matches the schema the spike proved, and note it.

- [ ] **Step 2: Commit** any harness script under `docs/` or note the manual result. (No code change if it's verification-only.)

---

## Self-Review

**Spec coverage (Phase 1a):**
- Hook forwarder via our binary (Spec: Architecture) → A1. ✓
- Socket + settings injection, non-destructive `--settings`, absolute binary path (Spec: constraints) → A1/A2. ✓
- `--session-id` correlation (Spec) → A3. ✓
- Hook→state machine incl. `permission_prompt`/`idle_prompt`/unknown (Spec: state table) → A3. ✓
- Bell + macOS notification, throttled to unfocused transition (Spec: Attention) → A4. ✓
- `Ctrl-a !` jump-to-attention (Spec) → A4. ✓
- Cleanup of socket/settings on exit (Spec) → A2 Step 4. ✓
- **Deferred to 1b:** `PreToolUse` blocking hook + sidebar approve/deny (`--sidebar-approvals`).

**Placeholder scan:** none — A2 Step 4's "Task A3 fills the body" is an explicit staged handoff, not a placeholder.

**Type consistency:** `HookEvent { session_id, event (renamed from hook_event_name), notification_type }` used identically across A2/A3/A4. `state_for_hook`/`next_attention` are pure and unit-tested. `create_with_id` id == `--session-id` == `HookEvent.session_id` (the correlation contract).

**Risks:** `osascript` notifications require macOS (fine — target platform); on non-macOS `notify::desktop` should no-op. The listener thread lifetime ends at process exit (detached) — acceptable, same pattern as the PTY reader threads.
