# claude-deck

A lean, terminal-native TUI for running and managing multiple Claude Code sessions at once.

## Run

```bash
cargo run
```

`cargo build --release` produces `target/release/claude-deck`.

Requires the `claude` CLI installed and logged in — it spawns the real `claude` using your subscription.

## Keys

Leader key: `Ctrl-a`, then:

| Key | Action |
|-----|--------|
| `n` | New session (prompts for a path) |
| `1`–`9` | Focus session by number |
| `[` | Previous session |
| `]` | Next session |
| `x` | Kill focused session |
| `q` | Quit |
