# Claude Deck — Home Screen + Mouse Support — Design Spec

**Date:** 2026-07-18
**Status:** Approved (via dialogue)
**Builds on:** the completed TUI foundation (`2026-07-18-claude-deck-tui-foundation.md`).

## Motivation

Today the app **auto-starts a `claude` session** in the launch directory — the user is dropped into a session they didn't pick ("I just get launched into a claude section which I haven't selected"). Instead, the app should open on a **warm landing screen** that greets the user and lets them choose what to do. Plus **mouse support** so the sidebar and pane are clickable, not keyboard-only.

## Feature 1 — Home screen

A pinned, non-session **Home** entry at the TOP of the sidebar (`⌂ Home`), above the numbered `claude` sessions. Focusing it renders a welcome/launcher view in the main pane instead of a terminal.

**Behavior:**
- **On launch, land on Home** — do NOT auto-start a `claude` session. The current cwd-auto-start is removed. Sessions are created only on explicit `Ctrl-a n`.
- **Content (warm launcher, not a dry help table):**
  - centered `claude-deck` title
  - a greeting: "Welcome back" + "What do you want to work on today?"
  - a primary call-to-action: `Ctrl-a n  — new session (pick a folder)`
  - `Ctrl-a h — back to Home`
  - a compact keybind line: switch `Ctrl-a 1-9 / [ / ]`, kill `Ctrl-a x`, quit `Ctrl-a q`
  - a status line: "no sessions yet — start one above" when empty; a short active-session summary once sessions exist.
  - Greeting copy tone: friendly/light (one emoji ok). Easy to restyle — lives in one render fn.
- **Recall / hide:** `Ctrl-a h` focuses Home from anywhere. `Ctrl-a x` while Home is focused HIDES it (removes the sidebar entry); `Ctrl-a h` re-adds+focuses it. Home is never permanently lost. Hiding Home while sessions exist focuses the first session; hiding Home with no sessions leaves an empty main pane with a hint ("Ctrl-a h for Home, Ctrl-a n for a session").
- **Not a numbered session:** `Ctrl-a 1-9` map to real sessions only. `Ctrl-a [` / `]` cycle through all sidebar entries INCLUDING Home (so you can wheel back to it).

**Model change:** replace `App.focused: usize` with a focus target that can be Home or a session:
```rust
enum Focus { Home, Session(usize) }
```
`App` gains `home_visible: bool` (default true) and `focus: Focus` (default `Home`). The sessions `Vec` is unchanged. Rendering, key routing, resize, and kill all branch on `focus`.

## Feature 2 — Mouse support (both panes)

Enable crossterm mouse capture (`EnableMouseCapture` on setup, `DisableMouseCapture` on teardown — add to the same enter/leave sequence as raw mode / alternate screen so it's always cleaned up).

- **Click in the sidebar** → focus the clicked entry (Home or a session), mapping the click row to the sidebar list index (accounting for the border + Home-at-top offset). Out-of-range clicks are ignored.
- **Click / scroll / drag in the main pane, when a session is focused** → forwarded to that session's `claude` as an SGR mouse event (encode `MouseEvent` → `ESC[<b;x;yM/m`), translating coordinates to be pane-relative (subtract the sidebar width + borders). When Home is focused, main-pane mouse is ignored (Home isn't interactive).
- **Scroll over the sidebar** → cycle focus prev/next (same as `[` / `]`).

**Isolation:** mouse→bytes encoding lives in a small `src/mouse.rs`; coordinate→sidebar-index mapping is a helper the event loop calls. The PTY/session core is untouched.

## Non-goals
- No mouse text-selection/copy of our own (claude handles selection inside its pane via forwarded mouse; terminal-level selection is the emulator's job).
- No recent-folders persistence yet (the Home "active summary" is live sessions only; persisted recents are a later nicety).

## Build order
1. **Home screen** (`src/home.rs` + `Focus` enum + remove auto-start + `Ctrl-a h`/hide). Independently testable and usable.
2. **Mouse** (`src/mouse.rs` + capture enable/disable + click-to-focus + main-pane forwarding).

## Testing
- Unit: `Focus` transitions; sidebar row→index mapping (click Y → entry, including Home offset and out-of-range); SGR mouse encoding for a few representative events (left-click, scroll up/down, with pane-relative coords).
- Smoke (PTY harness, extend `/tmp/cdeck_smoke.py`): launch → assert Home markers ("Welcome back" / "claude-deck") render and NO `claude` child is auto-spawned; then `Ctrl-a n <path> Enter` starts a session; `Ctrl-a q` exits clean.
- Terminal restore must still hold on all exit paths WITH mouse capture enabled (DisableMouseCapture in teardown + panic hook).
