# Claude Deck — Design Spec

**Date:** 2026-07-18
**Status:** Approved for planning
**Working name:** `claude-deck` (placeholder — rename freely)

## 1. What we're building

A lightweight, terminal-native desktop app for running and managing **multiple
Claude Code sessions at once**, from a single lean window with a session sidebar.

It is best described in one line: **"t3code, but terminal-native instead of a
web GUI, and Claude-only."** You like what t3code *does* (one place to run coding
agents); you reject *how it feels* (a web UI). This keeps the capability and
replaces the shell with something that looks and behaves like a real terminal.

The core move: **we do not reimplement Claude Code.** Each session is the *real*
`claude` CLI running in a *real* terminal pane. The app is a thin management
shell around real Claude Code processes.

## 2. Why this shape (the decisions already made)

- **Run the real `claude` binary, don't rebuild it.** This guarantees *all*
  Claude functionality (skills, MCP, slash commands, plan mode, subagents,
  hooks, `/config`) with zero re-implementation, stays **update-proof** (new
  Claude Code versions just work), and keeps **your preferences** (`settings.json`,
  `CLAUDE.md`, keybindings, MCP config) intact because it's the real thing
  reading your real config.
- **Subscription / ToS is clean.** The real `claude` CLI handles its own
  subscription auth (same as t3code, which "delegates authentication to each
  provider's own CLI tools rather than managing credentials itself"). We never
  touch tokens, never call the API directly. As of 2026-07, Claude Code /
  subscription usage draws from normal Max limits (the announced separate-credit
  metering was paused 2026-06-15). If that changes later, nothing here breaks —
  only the billing *source* moves.
- **Terminal-native, not a web UI and not a TUI.** A bare-terminal-aesthetic
  window (think Ghostty/Alacritty), not chat bubbles and buttons. The `claude`
  pane is a genuine terminal emulator (native scroll / selection / copy), so it
  never has the "hijacked terminal" feeling of a full-screen TUI.
- **Rust for the performance-critical core**, for a small, snappy, native-feeling
  app.

## 3. Non-goals (YAGNI)

- **No multi-provider support.** Claude only. (Explicitly narrower than t3code.)
- **No credential/auth management.** The `claude` CLI owns auth.
- **No reimplementation of Claude Code UI/features.** We render a terminal, not
  a bespoke chat UI.
- **No cloud/remote/hosting.** Local desktop app, local sessions.
- **No collaboration / sharing / multi-user.** Single user, one machine.

## 4. Architecture

Three components inside one app:

### 4.1 Rust core ("the daemon")
Owns the list of sessions and all RAM-critical logic. Responsibilities:
- Spawn a session: allocate a **PTY** (`portable-pty`) running `claude` in a
  user-chosen working directory, with a generated/known session id.
- Pipe PTY output → UI terminal widget; pipe UI keystrokes → PTY.
- Track each session's **state** (see §5).
- **Reap idle sessions** to reclaim memory (see §6).
- Kill / restart sessions on demand.

### 4.2 Hook bridge
A tiny local IPC endpoint (Unix domain socket) the Rust core listens on. Each
`claude` session is launched configured with Claude Code **hooks** that fire a
small forwarder command; the forwarder reads the hook's JSON payload from stdin
(which includes `session_id` and `cwd`) and POSTs it to the socket. This lets
sessions **self-report state** without screen-scraping the terminal.

Hook → state mapping (MVP):
- `UserPromptSubmit` → **running**
- `Notification` (permission prompt / idle input needed) → **waiting on you**
- `Stop` (Claude finished responding) → **done / idle**

Hooks are injected per-session via a session-scoped settings file (so we don't
mutate the user's global `settings.json`).

### 4.3 UI (thin)
- **Left: session sidebar** — one row per session: state glyph, short label
  (dir name + task hint), and a "＋ new session" affordance that opens a
  **folder picker** ("select my own path"), then spawns `claude` there.
- **Right: focused terminal pane** — the real terminal for the selected session
  (xterm.js), full native terminal behavior.
- **Switch sessions** by click or hotkey (e.g. ⌘1–9 / arrow keys).

State glyphs: `⏳ running · ◍ waiting on you · ✓ done · ○ idle · ✗ error`

## 5. Session state machine

```
              spawn
   (none) ─────────────▶ running ──Stop hook──▶ done/idle
                            ▲  │                    │
             UserPromptSubmit│  │Notification hook   │ user types / attaches
                            │  ▼                    ▼
                          running ◀── waiting-on-you ┘
   any state ──process exits/crashes──▶ error
   idle + timeout ──reap──▶ parked (process killed, resumable)
```

- **running** — actively working (thinking / tool use).
- **waiting on you** — needs a permission decision or input.
- **done / idle** — finished its turn, awaiting next prompt.
- **parked** — reaped to save RAM; resumable (see §6).
- **error** — process exited unexpectedly.

## 6. RAM strategy

The honest constraint: each *actively running* `claude` session is a Node
process — that's inherent and unavoidable, because it's the real agent doing
real work. Our RAM wins come from two levers, both architectural, not from the
shell language:

1. **One UI runtime for all sessions**, instead of N terminal-emulator apps each
   hauling its own runtime + render loop.
2. **Reaping idle/parked sessions.** After a configurable idle timeout, kill the
   session's `claude` process to reclaim its memory, keeping its session id. On
   re-focus, revive with `claude --resume <session-id>` (or `--continue`),
   restoring the conversation. The sidebar shows it as **parked**; revival is
   transparent to the user.

Idle timeout is user-configurable; reaping can be disabled per-session (e.g.
pin a long-running session).

## 7. Tech stack

| Layer | Choice | Why |
| --- | --- | --- |
| Core / daemon | **Rust** | Small, fast, RAM-critical logic (PTY mgmt, reaping, hook socket). |
| PTY | **`portable-pty`** | Cross-platform pseudo-terminal handling. |
| Window shell | **Tauri** (Rust + system WebView) | Light (~50MB vs Electron's 300MB+); Rust-first; WebView is only a *render surface*. |
| Terminal widget | **xterm.js** | The hard part (VT/escape-sequence correctness, resize, colors, mouse) already solved; same widget VS Code uses. Makes the pane feel genuinely native. |
| IPC | **Unix domain socket** | Hook bridge; simple, local, fast. |

**Why not pure-native Rust (iced/egui + a native terminal widget)?** It's the
leanest possible and stays open as a **later upgrade path**, but building a
correct terminal emulator widget is weeks of careful work and the single hardest
part of the project. xterm.js removes that risk entirely. The RAM-critical core
is Rust *either way*; the only difference is whether terminal *rendering* is a
proven JS widget or hand-built native. We start with the proven widget.

**On "isn't a WebView the web UI you rejected?"** No. The WebView renders
xterm.js — a real terminal emulator — not a web page. No chat bubbles, no
buttons, no web chrome. It looks like Ghostty, not like a website.

## 8. Data flow

```
 UI keystrokes ──▶ Rust core ──▶ PTY ──▶ claude CLI
 claude CLI ──▶ PTY ──▶ Rust core ──▶ xterm.js (render)
 claude hooks ──▶ forwarder ──▶ unix socket ──▶ Rust core ──▶ sidebar state
 folder picker ──▶ Rust core: spawn claude in <path>
```

## 9. Error handling

- **`claude` binary missing / not on PATH** → clear onboarding error with the
  install/login hint; app still opens.
- **Session process crashes** → mark **error** in sidebar, keep the pane with
  last output, offer restart (`--resume`).
- **Hook socket delivery fails** → state falls back to a best-effort heuristic
  (e.g. "running" while PTY produces output, "idle" after quiet period); never
  blocks terminal I/O. State accuracy degrades gracefully; usability does not.
- **Resume fails** (session id gone) → offer to start fresh in the same dir.

## 10. Testing strategy

- **Rust core, unit:** state-machine transitions, reap/park/resume logic, hook
  payload parsing, socket message handling.
- **Rust core, integration:** spawn a real (or stub) `claude`, drive a prompt,
  assert PTY round-trip and state transitions from injected hook events.
- **UI, manual + smoke:** sidebar reflects state changes; folder picker spawns in
  the right cwd; session switching preserves scrollback; copy/paste native.
- **RAM validation:** measure idle vs active footprint; confirm reaping actually
  releases memory and resume restores the conversation.

## 11. Risks / open questions

- **Hook coverage:** confirm `UserPromptSubmit` / `Notification` / `Stop` fire in
  all the moments we map to states (esp. permission prompts vs plain idle).
  Fallback heuristic (§9) covers gaps.
- **Per-session hook injection:** confirm the cleanest mechanism to scope hooks
  to one session without touching global config (session-scoped settings file
  vs env vars vs `--settings`).
- **`--resume` fidelity:** verify resumed sessions restore enough context to feel
  seamless; if not, prefer parking only truly-idle sessions.
- **Rename:** pick a real product name before public/shareable state.

## 12. Rough milestones

1. **Spike:** Rust + Tauri + xterm.js window running one real `claude` PTY
   session end-to-end (type, see output, native scroll/copy).
2. **Multi-session + sidebar:** spawn N sessions, folder picker, switch between
   them, per-session scrollback.
3. **State via hooks:** hook bridge socket + per-session hook injection + sidebar
   glyphs.
4. **Reaping/resume:** idle timeout → park → `--resume` revival.
5. **Polish:** hotkeys, error states, config (idle timeout, pinning), theming.
