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
    #[allow(dead_code)] // used by later reaping/kill work
    pub killer: Box<dyn ChildKiller + Send + Sync>,
    pub parser: Arc<Mutex<vt100::Parser>>,
}

/// Login-shell probe for the `claude` binary (Finder/packaged launches get a
/// minimal PATH). Returns None if not found.
pub fn resolve_claude_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let out = Command::new(&shell).args(["-lc", "command -v claude"]).output().ok()?;
    if !out.status.success() { return None; }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() { None } else { Some(p) }
}

pub fn spawn(
    claude_path: &str,
    cwd: &Path,
    rows: u16,
    cols: u16,
    id: String,
    settings_path: &str,
    tx: Sender<AppEvent>,
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
    cmd.args(["--session-id", &id, "--settings", settings_path]);

    let child = pair.slave.spawn_command(cmd).map_err(to_io)?;
    drop(pair.slave);
    let killer = child.clone_killer();

    let mut reader = pair.master.try_clone_reader().map_err(to_io)?;
    let writer = pair.master.take_writer().map_err(to_io)?;
    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));

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

    Ok(PtySession { writer, master: pair.master, killer, parser })
}

fn to_io<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}
