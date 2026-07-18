# Claude Deck — Awareness Layer (Phase 1) — Design Spec

**Date:** 2026-07-18
**Status:** Approved (pending user sign-off)
**Builds on:** the shipped TUI (Home + multi-session + mouse + icons + rename).

## Goal

Make the sidebar reflect each session's **real** state — running / waiting-on-you / idle — driven by Claude Code hooks, and actively surface sessions that need attention (bell + desktop notification + jump-to-attention). Optionally, approve/deny a session's permission prompt from the sidebar.

This is the app's core promise: run many sessions, and *the tool tells you which one needs you* instead of you polling tabs.

## Confirmed mechanics (empirically verified against real `claude`)

- Hooks fire in an interactive PTY session when passed `claude --settings <file>` (the file **merges** with the user's global config — non-destructive, per-invocation).
- `claude --session-id <uuid>` forces the id, and it appears as `session_id` in every hook payload → clean correlation to our session.
- Verified event cycle: `SessionStart` → `UserPromptSubmit` (payload includes `prompt`) → `Stop` (payload includes `stop_hook_active`). Every payload includes `session_id`, `cwd`, `hook_event_name`, `transcript_path`.
- Per docs: `Notification` fires with `notification_type` = `permission_prompt` (needs approval) or `idle_prompt` (done, awaiting prompt).
- Hook commands run in the session cwd and do **not** source shell profiles → the hook command must use the **absolute path** to our binary.
- `--bare` skips all hooks — must NOT be used.
- Approve/deny: a `PreToolUse` hook can emit `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"|"deny"}}` (exit 0) to decide a tool call without Claude's in-pane prompt.

## Architecture

### Hook forwarder — reuse our own binary
Add a hidden subcommand: **`claude-deck __hook <socket_path>`**. It reads the hook JSON from stdin, connects to the Unix socket at `<socket_path>`, writes the JSON, and exits 0. (For sidebar-approval it also reads one line back — the decision — and prints the `permissionDecision` JSON before exiting; see Phase 1b.) No separate helper binary, no `nc` dependency. The hook command embeds the **absolute** path (`std::env::current_exe()`), resolved when the settings file is written.

### IPC — Unix domain socket
On startup claude-deck binds a socket at `std::env::temp_dir()/claude-deck-<pid>.sock`. A listener thread `accept()`s connections (one per hook invocation), reads the JSON, and sends `AppEvent::Hook(HookEvent)` into the existing mpsc event loop. Socket + settings file are removed on exit (and best-effort on panic).

### Per-session hook injection — one shared settings file
At startup, write ONE settings JSON to `temp_dir()/claude-deck-<pid>-settings.json` containing the hooks (below), each pointing to `<abs-binary> __hook <socket>`. Every session is spawned with `--settings <that file> --session-id <our-uuid>`. Shared file is fine because the forwarder reads `session_id` from each payload. The session's id in `SessionManager` **is** the `--session-id` uuid (so create() must use a supplied uuid, not generate its own).

Hooks configured (Phase 1a): `SessionStart`, `UserPromptSubmit`, `Notification`, `Stop`. (Phase 1b adds `PreToolUse`.)

### State machine (driven by `AppEvent::Hook`, matched by `session_id`)
| Hook event | New state |
|---|---|
| `SessionStart` | Starting |
| `UserPromptSubmit` | Running |
| `Notification` (`notification_type` = `permission_prompt`) | WaitingOnYou |
| `Notification` (`notification_type` = `idle_prompt`) | Idle |
| `Notification` (other/unknown) | WaitingOnYou (safe default — any notification means it wants you) |
| `Stop` | Idle |
| process exit | Closed / Error (existing) |

Hook events for an unknown/already-removed `session_id` are ignored (no panic).

### Attention signals
On a transition **into `WaitingOnYou`** for a session that is **not currently focused**:
- **Bell:** write `\x07` to the app's stdout (terminal bell).
- **Desktop notification (macOS):** spawn a detached `osascript -e 'display notification "<label> needs you" with title "claude-deck"'`.
- Throttle: fire only on the *transition* into WaitingOnYou, not on repeats. Focused session → no notification (you're already looking at it).
- Both are on by default; a `--no-notify` flag (and/or config) disables the desktop notification, `--no-bell` the bell. (Config file is a later phase; flags for now.)

### Jump-to-attention
**`Ctrl-a !`** focuses the next session in `WaitingOnYou` (cycling from the current focus). If none are waiting, it's a no-op (optionally a brief status blip).

### Sidebar
Glyphs already exist and are colored by state; now they're driven by real events. `WaitingOnYou` (yellow bell/◍) becomes meaningful. No new rendering needed beyond what the state changes drive.

## Phase 1b — Approve/deny from the sidebar (opt-in)

**Opt-in via `--sidebar-approvals` (default OFF).** When off, no `PreToolUse` hook is injected: Claude shows its own rich permission prompt in the pane, and `Notification(permission_prompt)` still drives awareness + jump-to-attention (you jump in and approve in Claude's UI). When on:

- A `PreToolUse` hook is added. On fire, the forwarder sends the payload (`session_id`, `tool_name`, `tool_input`) and **blocks** reading the decision.
- claude-deck marks the session `WaitingOnYou` with a **pending approval** (stores the tool name + the open socket connection to answer). The sidebar/pane shows: `<label> wants to run <tool_name>` with `[a]pprove / [d]eny`.
- The user presses **`Ctrl-a a`** (approve) / **`Ctrl-a d`** (deny) on that session — or clicks an approve/deny affordance — and claude-deck writes the decision back; the forwarder prints `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"|"deny"}}` and exits 0.

**Tradeoff (stated):** when sidebar-approvals is on, Claude's own (richer) permission UI is bypassed in favor of our simpler tool-name prompt. That's why it's opt-in and defaults off. Enriching the approval view (showing the command/diff from `tool_input`) is a later refinement.

## Non-goals (this phase)
- No config file yet (flags only; config is a later phase).
- No persistence of states across restarts (that's the Persistence phase).
- No approval of MCP/`PermissionRequest`-only tools (PreToolUse covers the common case).
- Broadcast, split view, scrollback — separate phases.

## Build order
1. **Phase 1a — Awareness:** forwarder subcommand + socket listener + shared settings injection + `--session-id` correlation + hook→state machine + bell + macOS notification + `Ctrl-a !` jump-to-attention.
2. **Phase 1b — Sidebar approvals (opt-in):** `PreToolUse` blocking hook + pending-approval model + `Ctrl-a a`/`Ctrl-a d` + approval affordance.

## Testing
- **Unit:** hook-event → state-transition mapping (incl. unknown notification_type → WaitingOnYou, unknown session_id ignored); jump-to-attention target selection; settings-JSON generation (correct hook structure + absolute binary path); the throttle (notify only on transition-into-waiting for unfocused sessions).
- **Integration (real `claude`, PTY harness):** reuse the verified spike — spawn a session with our generated settings + `--session-id`, submit a prompt, assert the app receives `UserPromptSubmit`→Running then `Stop`→Idle over the socket. Assert no notification fires for the focused session.
- **Manual:** two sessions; make one hit a permission prompt → sidebar turns yellow + bell + desktop notification; `Ctrl-a !` jumps to it. With `--sidebar-approvals`, approve/deny from the sidebar drives the tool.
- **Cleanup:** socket + settings file removed on quit; verify no stale files.
