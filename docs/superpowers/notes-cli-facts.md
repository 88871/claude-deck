# Claude Code CLI facts (for Plan 2)

Recorded during Task 3. Machine: macOS. `claude` resolves via login shell to
`/Users/m/.local/bin/claude`. Version: **2.1.214 (Claude Code)**.

Source: `claude --help` output on 2026-07-18.

## Flags relevant to Plan 2 (hooks + reaping / session identity)

| Flag | Exists? | Help line (verbatim) |
| --- | --- | --- |
| `--session-id <uuid>` | **Yes** | `--session-id <uuid>  Use a specific session ID for the conversation (must be a valid UUID)` |
| `--resume [value]` (`-r`) | **Yes** | `-r, --resume [value]  Resume a conversation by session ID, or open interactive picker with optional search term` |
| `--continue` (`-c`) | **Yes** | `-c, --continue  Continue the most recent conversation in the current directory` |
| `--settings <file-or-json>` | **Yes** | `--settings <file-or-json>  Path to a settings JSON file or a JSON string to load additional settings from` |

### Notes / nuances

- **`--session-id`** takes a caller-supplied UUID and requires it to be a valid
  UUID. This is exactly the identity `SessionManager::create` already mints
  (`uuid::Uuid::new_v4()`), so Plan 2 can pass our own session id through to
  `claude` and correlate PTY sessions with claude conversation ids 1:1.
- **`--resume`** accepts an optional value: with a session id it resumes that
  conversation directly; with no value it opens an interactive picker (optional
  search term). For headless/programmatic reattach, pass the id explicitly.
- **`--continue`** resumes the *most recent* conversation in the current
  directory — directory-scoped, no id needed. Useful as a fallback but less
  precise than `--resume <id>`.
- **`--settings`** accepts **either** a path to a JSON file **or** a raw JSON
  string. Plan 2's hooks/state-bridge config can therefore be injected inline
  (no temp file required) — e.g. a settings JSON string wiring hook commands.

### Adjacent flags observed (context for Plan 2)

- `--setting-sources <sources>` — comma-separated list of setting sources.
- `--bare` — minimal mode that **skips hooks** (and LSP, plugin sync, auto-memory,
  etc.), sets `CLAUDE_CODE_SIMPLE=1`. Relevant because Plan 2's hook-based state
  bridge is incompatible with `--bare`; do NOT use `--bare` if hooks are needed.
- `--dangerously-skip-permissions` / `--allow-dangerously-skip-permissions` —
  bypass permission prompts (sandbox use only).

## Hook events / permission-prompt behavior (Plan 2 state bridge)

Not exercised end-to-end in this task: triggering a live permission prompt and
observing hook-event emission requires a human at the GUI (typing a prompt and
letting claude request a tool). The flags above confirm the mechanism is
available — hooks are configurable via `--settings` (inline JSON or file) and
are active unless `--bare` is passed. This should be validated interactively at
the start of Plan 2.
