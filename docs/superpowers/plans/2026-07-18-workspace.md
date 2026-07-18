# Claude Deck — Workspace Save/Restore Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Checkbox steps.

**Goal:** Persist the set of open sessions (id, label, cwd, pinned) and restore them on next launch — as **parked** entries (no process spawned until focused), so the workspace reappears without a RAM spike. Focusing a restored session resumes it via `claude --resume` (verified: restores the conversation, same session_id).

**Architecture:** A `workspace.rs` module saves/loads a small JSON to the cross-platform config dir (`dirs::config_dir()/claude-deck/workspace.json`). The app writes it after any session mutation (create/rename/pin/kill/park). On launch (unless `--no-restore`), it loads the file and repopulates `App.sessions` as `(id, None)` parked entries with the saved label/cwd/pinned + state `Parked`; the existing revive-on-focus (`--resume`) brings each back when focused.

## Global Constraints
- **Cross-platform** — config path via `dirs` (no hardcoded `~/.config`); no `std::os::unix`-only code.
- **Restore does NOT auto-spawn sessions** — restored sessions are parked (RAM-safe); they spawn only on focus.
- **Never call the Anthropic API / handle tokens.** Revival is `claude --resume`.
- Graceful: a missing/corrupt workspace file → start clean (empty). A restored session whose `claude` conversation no longer exists → its revive fails and it shows `Error` (handled by the existing Exited path); do not crash.

---

### Task S1: Persist + restore the workspace

**Files:** `Cargo.toml` (add `dirs`), `src/workspace.rs` (new), `src/app.rs`

**Interfaces:**
- Add dep `dirs = "5"` (or current).
- `src/workspace.rs` (`mod workspace;`):
  - `#[derive(Serialize, Deserialize, Clone)] struct Entry { id: String, label: String, cwd: PathBuf, pinned: bool }`
  - `fn path() -> Option<PathBuf>` → `dirs::config_dir()?/ "claude-deck" / "workspace.json"`.
  - `fn save(entries: &[Entry])` → create the parent dir, write pretty JSON; ignore errors.
  - `fn load() -> Vec<Entry>` → read + parse; on any error return empty Vec.
  - Unit-test round-trip: `save`→`load` yields the same entries (use a temp path via a `save_to(path, entries)`/`load_from(path)` pair so the test doesn't touch the real config dir; `save`/`load` delegate to them with `path()`).
- `src/app.rs`:
  - `App::snapshot_entries() -> Vec<workspace::Entry>` — build from `manager.list()` (id, label, cwd, pinned) for ALL sessions (live or parked).
  - `App::save_workspace()` — `workspace::save(&self.snapshot_entries())`. Call it after: `start_session`, rename confirm, `toggle_pin`, `kill_focused`, and `park_session`.
  - In `App::new` (or startup, unless `std::env::args()` contains `--no-restore`): `for e in workspace::load()` → `manager.create_with_id(e.id.clone(), e.cwd.clone())`, set its label (`manager` needs a way to set label — reuse `rename`) and pinned (`set_pinned`), set state `Parked`, and `self.sessions.push((e.id, None))`. Leave `focus = Home`. Restored sessions are parked; the existing revive-on-focus handles resuming them.

- [ ] Step 1: Add `dirs`; write the `save_to`/`load_from` round-trip test in `workspace.rs`. Run → fail.
- [ ] Step 2: Implement `workspace.rs`; wire `save_workspace()` on mutations + restore-on-launch + `--no-restore`. Ensure `SessionManager` can set a restored session's label + pinned.
- [ ] Step 3: `cargo test` all pass; `cargo build` clean; both binaries build. Confirm by inspection: restore creates PARKED (`None`) sessions (no spawn); save fires on mutations. Commit: `feat(workspace): save open sessions and restore them (parked) on launch`.

---

### Task S2: Integration verify (real `claude`)

- [ ] Step 1: Harness: (a) create a real `claude` session (`--session-id UUID --settings <gen>`), establish memory ("banana42"), kill it; (b) hand-write a `workspace.json` (in a temp `XDG_CONFIG_HOME`/`HOME`) with that entry; (c) confirm `workspace::load` reads it (build a tiny test bin OR just re-run the app's `load_from` on the file in a unit-style check); (d) confirm `claude --resume UUID` still restores "banana42" (proves a restored+revived session resumes correctly). Record results. (Most of this reuses earlier verified spikes; the new bit is the save/load round-trip + that a restored id resumes.)
- [ ] Step 2: Report. Verification-only.

---

## Self-Review
**Coverage:** persist on mutations (S1), restore as parked/RAM-safe (S1), cross-platform path via `dirs` (S1), `--no-restore` (S1), graceful missing/corrupt/gone-session (S1 + existing Error path), real-claude resume-after-restore (S2). ✓
**Risks:** restored session whose conversation was deleted → revive fails → Error (acceptable). Config-dir write perms → save ignored silently (acceptable). `dirs` is cross-platform (config_dir differs per OS — that's desired).
