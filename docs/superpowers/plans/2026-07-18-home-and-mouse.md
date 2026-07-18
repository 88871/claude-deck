# Claude Deck — Home Screen + Mouse — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Replace the auto-started session with a warm Home/welcome landing screen, and add mouse support to both the sidebar and the session pane.

**Architecture:** Extend the existing ratatui TUI. Introduce a `Focus { Home, Session(usize) }` target (replacing `App.focused: usize`) plus `home_visible: bool`. A new `src/home.rs` renders the welcome view. A new `src/mouse.rs` maps clicks to sidebar indices and encodes main-pane mouse events as SGR sequences forwarded to `claude`. PTY/session core is untouched.

**Tech Stack:** Rust, `ratatui` 0.29, `crossterm` 0.28 (mouse events + capture), existing `portable-pty`/`vt100`/`tui-term`.

## Global Constraints

- **Terminal application only** — no window/GUI. (Foundation spec.)
- **Never call the Anthropic API / handle tokens** — sessions are the real `claude`. (Foundation spec.)
- **Terminal restored on EVERY exit path** — now ALSO `DisableMouseCapture` in teardown + panic hook, paired with the existing raw-mode/alt-screen restore. A left-on mouse-capture corrupts the shell.
- **No auto-started session** — the app launches on Home; sessions start only on explicit `Ctrl-a n`.
- **Pane-engine isolation preserved** — `vt100`/`tui-term` stay confined to `pty.rs`/`ui.rs`; mouse encoding stays in `mouse.rs`.

---

### Task 1: Home welcome screen (+ remove auto-start, `Focus` model)

**Files:**
- Create: `src/home.rs` (welcome-view render fn)
- Modify: `src/app.rs` (`Focus` enum, `home_visible`, remove auto-start, `Ctrl-a h` recall, `Ctrl-a x` hides Home when focused, route keys/resize by focus)
- Modify: `src/ui.rs` (sidebar shows `⌂ Home` at top; render Home view or the focused session's pane)

**Interfaces:**
- Consumes: `SessionManager`, `PtySession`, `pane_dims` (foundation).
- Produces:
  - `enum Focus { Home, Session(usize) }` in `app.rs`.
  - `App.focus: Focus` (default `Focus::Home`) and `App.home_visible: bool` (default `true`), replacing `App.focused: usize`.
  - `home::render(f: &mut Frame, area: Rect, session_count: usize)` — draws the welcome view (title, greeting, CTA, keybind line, status line).
  - Sidebar rows = `⌂ Home` (when `home_visible`) followed by each session with its glyph; the focused row is highlighted.

- [ ] **Step 1: Write the `Focus` enum + unit tests for focus/hide transitions**

Add to `app.rs`:
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus { Home, Session(usize) }
```
Tests (in `app.rs` `#[cfg(test)]`, testing pure helpers — extract the transition logic into free functions or methods that don't need a live PTY):
- `cycle_focus` forward/back over `[Home, S0, S1]` wraps correctly and respects `home_visible=false` (Home skipped).
- hiding Home when sessions exist moves focus to `Session(0)`; hiding Home with no sessions leaves focus somewhere safe (define `Focus::Session(0)` guarded by an is-empty check, or a `None`-safe render).
Write these first, run `cargo test`, watch them fail.

- [ ] **Step 2: Implement `home.rs`**

```rust
use ratatui::{
    layout::{Alignment, Rect},
    style::{Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render(f: &mut Frame, area: Rect, session_count: usize) {
    let status = if session_count == 0 {
        "no sessions yet — start one above".to_string()
    } else {
        format!("{} active session{}", session_count, if session_count == 1 { "" } else { "s" })
    };
    let lines = vec![
        Line::from(""),
        Line::from("claude-deck").bold().centered(),
        Line::from(""),
        Line::from("Welcome back 👋").centered(),
        Line::from("What do you want to work on today?").centered(),
        Line::from(""),
        Line::from("▸  Ctrl-a  n     new session (pick a folder)").centered(),
        Line::from("▸  Ctrl-a  h     back to this Home screen").centered(),
        Line::from(""),
        Line::from("switch  Ctrl-a 1-9 / [ / ]    kill  Ctrl-a x    quit  Ctrl-a q")
            .style(Style::new().dim()).centered(),
        Line::from(""),
        Line::from(status).style(Style::new().dim()).centered(),
    ];
    let block = Block::default().borders(Borders::ALL).title("claude-deck");
    f.render_widget(Paragraph::new(Text::from(lines)).alignment(Alignment::Center).block(block), area);
}
```
(Adapt method names to the installed ratatui 0.29 API — e.g. `.centered()`, `.bold()`, `.dim()` exist on `Line`/`Style` in 0.29; if a helper differs, use `Style`/`Alignment` directly.)

- [ ] **Step 3: Remove auto-start + wire `Focus` in `app.rs`**

- Delete the startup `self.start_session(current_dir, ...)` call — launch with `focus = Focus::Home`, `home_visible = true`, empty `sessions`.
- `start_session` still creates a session but now also sets `self.focus = Focus::Session(new_index)`.
- Key routing in `on_key`: forward keystrokes to the PTY only when `focus == Focus::Session(i)`; when `focus == Focus::Home`, non-leader keys do nothing (Home isn't a terminal).
- Leader map additions/changes: `h` → `if !home_visible { home_visible = true } self.focus = Focus::Home`; `x` → if focused on Home, hide Home (`home_visible=false`, move focus to `Session(0)` if any else leave Home hidden with focus on a session-or-empty state); if focused on a session, kill it (existing behavior). `1..=9` → `Focus::Session(i)` if it exists. `[`/`]` → cycle over the visible entries (Home if visible, then sessions), wrapping.
- `on_resize`/`sync_focus_size`: only a `Focus::Session(i)` needs PTY+parser resize; Home needs none.

- [ ] **Step 4: Render by focus in `ui.rs`**

Sidebar: build rows = optional `⌂ Home` (if `home_visible`) then each session `"{glyph} {label}"`; highlight the row matching `focus`. Main pane: if `focus == Focus::Home` call `home::render(...)`; else render the focused session's `parser.screen()` via `PseudoTerminal` (existing). If focus is a session but none exists (edge), show a hint paragraph.

- [ ] **Step 5: Run tests + build**

Run: `cargo test` (Focus transition tests + the 4 session tests pass) and `cargo build` (clean).

- [ ] **Step 6: Smoke-test the launch behavior**

Extend `/tmp/cdeck_smoke.py` OR run a focused check: launch the binary in the PTY harness and assert the boot output contains `Welcome back` / `claude-deck` AND does NOT immediately spawn a claude child (no `claude`-specific output before any `Ctrl-a n`). Then `Ctrl-a q` exits clean. Report the result. (Headless: a subagent can run the python harness; if it can't, verify by inspection that startup no longer calls `start_session`.)

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(tui): home welcome screen; land on Home instead of auto-starting a session"
```

---

### Task 2: Mouse support (both panes)

**Files:**
- Create: `src/mouse.rs` (SGR encoding + sidebar-hit mapping)
- Modify: `src/main.rs` (EnableMouseCapture on setup, DisableMouseCapture in restore + panic hook)
- Modify: `src/app.rs` (handle `Event::Mouse`: sidebar click→focus, pane mouse→forward; scroll)
- Modify: `Cargo.toml` only if a crossterm feature is needed (mouse is default in crossterm 0.28)

**Interfaces:**
- Consumes: `Focus`, sessions, `pane_dims`, sidebar layout constants (width 26) from Tasks 1/foundation.
- Produces:
  - `mouse::encode_sgr(ev: &crossterm::event::MouseEvent, col: u16, row: u16) -> Option<Vec<u8>>` — SGR mouse byte sequence for pane-relative `(col,row)` (1-based), for press/release/scroll/drag; `None` for events we don't forward.
  - `mouse::sidebar_hit(row: u16, home_visible: bool, session_count: usize) -> Option<Focus>` — maps a click Y (terminal row) inside the sidebar to a `Focus`, accounting for the top border (row 0) and Home-at-top; `None` if out of range.
  - App handling: mouse in sidebar column range (`x < 26`) → `sidebar_hit` → set focus; mouse in pane and `focus==Session` → `encode_sgr` with pane-relative coords → write to that session's PTY; scroll over sidebar → cycle focus.

- [ ] **Step 1: Enable/disable mouse capture in `main.rs`**

In `init_terminal`, add `EnableMouseCapture` to the `execute!` after `EnterAlternateScreen`; in `restore_terminal`, add `DisableMouseCapture` (before/with `LeaveAlternateScreen`). Import from `crossterm::event`. The panic hook already calls `restore_terminal`, so capture is disabled on panic too. Verify by inspection that BOTH enter and leave include the mouse toggle.

- [ ] **Step 2: Implement `mouse.rs` with unit tests (TDD)**

Write tests first for:
- `sidebar_hit`: with `home_visible=true`, click row 1 → `Focus::Home`, row 2 → `Focus::Session(0)`, row (2+n) → `Focus::Session(n-1)`; row 0 (border) → `None`; row beyond last entry → `None`. With `home_visible=false`, row 1 → `Focus::Session(0)`.
- `encode_sgr`: left-button press at pane-relative (col=1,row=1) → `b"\x1b[<0;1;1M"`; release → `...m`; scroll-up → button 64; scroll-down → 65. (Confirm SGR button codes: left=0, middle=1, right=2, wheel-up=64, wheel-down=65; press uses `M`, release uses `m`.)
Then implement to pass. Run `cargo test mouse`.

- [ ] **Step 3: Handle `Event::Mouse` in `app.rs`**

In the event loop, match `AppEvent::Input(Event::Mouse(m))`:
- Compute region from `m.column`: if `m.column < 26` → sidebar. On a left `MouseEventKind::Down`, `mouse::sidebar_hit(m.row, home_visible, sessions.len())` → set `focus` (+ `sync_focus_size` if a session). On `ScrollUp`/`ScrollDown` over the sidebar → cycle focus prev/next.
- Else (pane) and `focus == Focus::Session(i)`: translate to pane-relative coords `col = m.column - 26 - 1 + 1` (pane interior starts after the 26-wide sidebar and its left border — compute against the actual pane rect origin; keep 1-based for SGR), `row = m.row - 1`; `mouse::encode_sgr(&m, col, row)` → write bytes to that session's PTY writer. Ignore pane mouse when `focus == Home`.
- Keep it robust: out-of-range → ignore, never panic.

- [ ] **Step 4: Build + tests + smoke**

`cargo test` (mouse + focus + session tests pass), `cargo build` clean. Smoke: launch in the PTY harness, send an SGR mouse click on a sidebar row (write e.g. `\x1b[<0;3;2M\x1b[<0;3;2m` to the master), and confirm no crash + clean `Ctrl-a q` exit. Report result (or verify by inspection if the harness can't drive it).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(tui): mouse support — click sidebar to focus, forward pane mouse to claude"
```

---

## Self-Review

**Spec coverage:**
- Land on Home, no auto-session (Spec F1) → Task 1 Steps 3/6. ✓
- Home welcome content + recall/hide (Spec F1) → Task 1 Steps 2/3/4. ✓
- `Focus { Home, Session }` model (Spec F1) → Task 1 Step 1/3. ✓
- Mouse both panes: sidebar-click focus + pane forwarding + scroll (Spec F2) → Task 2 Steps 2/3. ✓
- Mouse capture cleaned up on all exit paths (Spec F2) → Task 2 Step 1. ✓
- Isolation (mouse in `mouse.rs`, engine untouched) → Task 2 file structure. ✓

**Placeholder scan:** none. Coordinate math in Task 2 Step 3 is described against the actual pane rect origin (compute from layout, don't hardcode) — the `26`+border offsets are the known sidebar width from `ui.rs`.

**Type consistency:** `Focus` introduced in Task 1 and consumed by `mouse::sidebar_hit` in Task 2 (same enum). `App.focused: usize` fully replaced by `App.focus: Focus` — every prior reference (on_key routing, on_resize, ui::draw, kill) updated in Task 1. SGR button codes fixed in Task 2 Step 2 tests.

**Known risks:**
- **SGR coordinate origin:** the pane-relative translation must use the real pane `Rect` (from the ratatui `Layout` split), not a hardcoded 26 — if the sidebar width ever changes, derive from layout. Called out in Task 2 Step 3.
- **Mouse + claude expectations:** claude must be in a mode that reads SGR mouse; since we set `TERM=xterm-256color` and forward standard SGR, this is the widely-supported path. If claude doesn't respond to forwarded mouse, that's a claude-side mode issue, not a teardown/correctness bug — note, don't block.
