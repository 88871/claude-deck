use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use base64::Engine;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tauri::{AppHandle, Emitter};

pub struct PtyHandle {
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
    // Kept for Plan 2 (reaping/kill). Unused in the foundation.
    #[allow(dead_code)]
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}

/// Probe the user's login shell for the `claude` binary. A packaged macOS app
/// launched from Finder gets a minimal PATH (npm-global / `~/.local/bin`
/// absent), so `-lc` sources the user's profile. Returns `None` if not found —
/// the caller surfaces the §9 onboarding error. Run ONCE at startup and cache:
/// `-lc` sources `.zprofile`/`.profile` and can take a few hundred ms, so this
/// must not run per-spawn (Review nuance #3).
pub fn resolve_claude_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let out = Command::new(&shell).args(["-lc", "command -v claude"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() { None } else { Some(p) }
}

/// Spawn `claude` (at the pre-resolved `claude_path`) in `cwd`. Streams raw PTY
/// bytes to the frontend as base64 in `pty://data` events tagged with `id`
/// (frontend decodes; xterm handles UTF-8 across chunk boundaries — Review fix
/// #1). A waiter thread reports child exit via `session://state`: `"closed"` for
/// a clean exit (`/exit`, Ctrl-D, status 0) and `"error"` for a nonzero/abnormal
/// exit, so an intentional close is not shown as a failure (Review nuance #2).
/// `clone_killer()` is stored in `PtyHandle` for Plan 2 reaping — the child is
/// waited on, never left a zombie (Review fix #2).
pub fn spawn_claude(
    app: AppHandle,
    id: String,
    cwd: &Path,
    claude_path: &str,
) -> Result<PtyHandle, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(claude_path);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("LANG", "en_US.UTF-8");

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let killer = child.clone_killer();

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    // Reader thread: forward raw bytes as base64 (no lossy per-read decode).
    let read_id = id.clone();
    let read_app = app.clone();
    std::thread::spawn(move || {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let b64 = engine.encode(&buf[..n]);
                    let _ = read_app.emit(
                        "pty://data",
                        serde_json::json!({ "id": read_id, "b64": b64 }),
                    );
                }
            }
        }
    });

    // Waiter thread: block on child exit → clean vs. abnormal (Review nuance #2).
    let exit_id = id.clone();
    let exit_app = app.clone();
    std::thread::spawn(move || {
        let mut child = child;
        let clean = child.wait().map(|s| s.success()).unwrap_or(false);
        let state = if clean { "closed" } else { "error" };
        let _ = exit_app.emit(
            "session://state",
            serde_json::json!({ "id": exit_id, "state": state }),
        );
    });

    Ok(PtyHandle { writer, master: pair.master, killer })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration-ish: on a machine with Claude Code installed and on PATH via
    // the login shell, the probe must resolve a path. Ignored by default so CI
    // without `claude` installed doesn't fail; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn resolve_claude_path_finds_binary_when_installed() {
        assert!(
            resolve_claude_path().is_some(),
            "expected `claude` on PATH via login shell"
        );
    }
}
