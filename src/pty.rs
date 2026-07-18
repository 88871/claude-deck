use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use crate::app::AppEvent;

pub struct PtySession {
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// OS process ID of the child `claude` process; `None` if the platform
    /// doesn't expose it.  Captured **before** the child moves to the waiter
    /// thread so the waiter's `move` closure doesn't take it first.
    pub pid: Option<u32>,
    /// How many rows the user has scrolled back from the live view.
    /// 0 = live (bottom of output); positive = scrolled back that many rows.
    pub scroll: usize,
}

/// Resolve the path to the `claude` binary.
///
/// Strategy:
/// 1. Try `which::which("claude")` — works on all platforms, respects the
///    current PATH (fast, no subprocess).
/// 2. On Unix only, fall back to a login-shell probe so that Finder/packaged
///    launches (which inherit a minimal PATH) can still find claude if it lives
///    in a shell-profile-managed dir like ~/.local/bin.
pub fn resolve_claude_path() -> Option<String> {
    if let Ok(p) = which::which("claude") {
        return Some(p.to_string_lossy().into_owned());
    }
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        if let Ok(o) = Command::new(&shell).args(["-lc", "command -v claude"]).output() {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

pub fn spawn(
    claude_path: &str,
    cwd: &Path,
    rows: u16,
    cols: u16,
    id: String,
    settings_path: &str,
    tx: Sender<AppEvent>,
    resume: bool,
) -> std::io::Result<PtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(to_io)?;

    let mut cmd = CommandBuilder::new(claude_path);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("LANG", "en_US.UTF-8");
    if resume {
        cmd.args(["--resume", &id, "--settings", settings_path]);
    } else {
        cmd.args(["--session-id", &id, "--settings", settings_path]);
    }

    let child = pair.slave.spawn_command(cmd).map_err(to_io)?;
    drop(pair.slave);
    let killer = child.clone_killer();
    // Capture the pid BEFORE the child moves into the waiter thread.
    let pid: Option<u32> = child.process_id();

    let mut reader = pair.master.try_clone_reader().map_err(to_io)?;
    let writer = pair.master.take_writer().map_err(to_io)?;
    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 5000)));

    // Reader thread: feed the vt100 parser and wake the UI on each chunk.
    let read_parser = parser.clone();
    let read_tx = tx.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    read_parser.lock().unwrap().process(&buf[..n]);
                    if read_tx.send(AppEvent::Output).is_err() { break; }
                }
            }
        }
    });

    // Waiter thread: report clean vs. abnormal exit.
    let mut child = child;
    std::thread::spawn(move || {
        let clean = child.wait().map(|s| s.success()).unwrap_or(false);
        let _ = tx.send(AppEvent::Exited { id, clean });
    });

    Ok(PtySession { writer, master: pair.master, killer, parser, pid, scroll: 0 })
}

fn to_io<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}
