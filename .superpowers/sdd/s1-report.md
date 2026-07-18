# Task S1 Report — Workspace Save/Restore

## Status
COMPLETE — all checks pass.

## What was built

### `Cargo.toml`
- Added `dirs = "5"` (resolved to `5.0.1` by Cargo).

### `src/workspace.rs` (new)
- `Entry { id, label, cwd, pinned }` — `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]`
- `path() -> Option<PathBuf>` — `dirs::config_dir()?.join("claude-deck").join("workspace.json")`
- `save_to(path, entries)` / `load_from(path)` — testable helpers that target an arbitrary path (do NOT touch the real config dir in tests)
- `save(entries)` / `load()` — delegates to `path()` for production use
- 5 unit tests covering: round-trip, nonexistent path, garbage JSON, empty slice, order preservation

### `src/lib.rs`
- Added `pub mod workspace;`

### `src/app.rs`
- Import: `use crate::{..., workspace, ...}`
- `snapshot_entries(&self) -> Vec<workspace::Entry>` — maps `manager.list()` → Entry for all sessions
- `save_workspace(&self)` — calls `workspace::save(&self.snapshot_entries())`
- Restore in `App::new` (after struct construction, unless `--no-restore` in args):
  - For each `workspace::load()` entry: `create_with_id`, `rename`, optionally `set_pinned`, `set_state(Parked)`, `sessions.push((id, None))`
  - Focus stays at `Focus::Home`; no PTY spawned (RAM-safe)
- `save_workspace()` called after: `start_session`, `kill_focused`, `park_session`, rename confirm, `toggle_pin`

## Test summary
`cargo test`: **109 passed, 0 failed** (5 new `workspace::` tests + 104 existing).

## Build
`cargo build --bin claude-deck --bin cdeck`: clean, no warnings.

## `dirs` version
`dirs = "5"` (Cargo resolved `5.0.1`).

## Safety / RAM confirmation (by inspection)
- Restore pushes `(id, None)` — never calls `pty::spawn`; PTY spawning happens only in `revive_session` on focus.
- `--no-restore` guard: `if !args.contains(&"--no-restore".to_string())` wraps the restore loop.
- `save_workspace` fires on: `start_session`, `kill_focused`, `park_session`, rename confirm, `toggle_pin`.
- `workspace::path()` uses `dirs::config_dir()` — cross-platform, no `std::os::unix`-only code.
- Missing/corrupt workspace file → `load_from` returns empty Vec → clean start (tested).

## Concerns
None. `SessionManager` already had all needed methods (`create_with_id`, `rename`, `set_pinned`, `set_state`); no new methods were required.
