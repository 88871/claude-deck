use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use serde::Deserialize;

/// A deserialized Claude Code hook payload. Extra JSON fields are ignored.
#[derive(Deserialize, Debug, Clone)]
pub struct HookEvent {
    pub session_id: String,
    #[serde(rename = "hook_event_name")]
    pub event: String,
    #[serde(default)]
    pub notification_type: Option<String>,
}

/// Bind a `UnixListener` on `socket_path`, remove any stale socket first,
/// and spawn a background thread that calls `on_event` for each incoming
/// well-formed `HookEvent` payload. Bad/unparseable payloads are dropped
/// silently. Returns immediately after spawning the thread.
pub fn listen<F: Fn(HookEvent) + Send + 'static>(
    socket_path: PathBuf,
    on_event: F,
) -> std::io::Result<()> {
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let mut buf = Vec::new();
            if std::io::Read::read_to_end(&mut stream, &mut buf).is_ok() {
                if let Ok(ev) = serde_json::from_slice::<HookEvent>(&buf) {
                    on_event(ev);
                }
            }
        }
    });
    Ok(())
}

/// Returns `(socket_path, settings_path)` — both under `temp_dir()`, named
/// after this process's PID so multiple instances don't collide.
pub fn paths() -> (PathBuf, PathBuf) {
    let pid = std::process::id();
    let dir = std::env::temp_dir();
    (
        dir.join(format!("claude-deck-{pid}.sock")),
        dir.join(format!("claude-deck-{pid}-settings.json")),
    )
}

/// Build the shared settings JSON. Each hook invokes our own binary as a
/// forwarder using an ABSOLUTE path (hooks don't source shell profiles).
pub fn settings_json(binary: &str, socket: &str) -> String {
    let cmd = format!("{binary} __hook {socket}");
    let entry = serde_json::json!([{
        "matcher": "",
        "hooks": [{ "type": "command", "command": cmd }]
    }]);
    serde_json::json!({
        "hooks": {
            "SessionStart":       entry,
            "UserPromptSubmit":   entry,
            "Notification":       entry,
            "Stop":               entry,
        }
    })
    .to_string()
}

/// Write the shared hooks settings file to `settings_path`.
/// The binary path is resolved via `std::env::current_exe()`.
pub fn write_settings_file(
    settings_path: &std::path::Path,
    socket: &str,
) -> std::io::Result<()> {
    let binary = std::env::current_exe()?.to_string_lossy().into_owned();
    std::fs::write(settings_path, settings_json(&binary, socket))
}

/// `claude-deck __hook <socket>`: read the hook payload from stdin and forward
/// it to the app's socket. Never fails loudly — a broken forward must not break
/// the claude session.
pub fn forward(socket_path: &str) -> std::io::Result<()> {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    if let Ok(mut stream) = UnixStream::connect(socket_path) {
        let _ = stream.write_all(&input);
        let _ = stream.flush();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_hook_payloads() {
        let ups = r#"{"session_id":"abc","cwd":"/x","hook_event_name":"UserPromptSubmit","prompt":"hi"}"#;
        let e: HookEvent = serde_json::from_str(ups).unwrap();
        assert_eq!(e.session_id, "abc");
        assert_eq!(e.event, "UserPromptSubmit");
        let notif = r#"{"session_id":"abc","hook_event_name":"Notification","notification_type":"permission_prompt"}"#;
        let n: HookEvent = serde_json::from_str(notif).unwrap();
        assert_eq!(n.notification_type.as_deref(), Some("permission_prompt"));
    }

    #[test]
    fn settings_json_registers_hooks_pointing_at_our_binary() {
        let json = settings_json("/abs/claude-deck", "/tmp/x.sock");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        for ev in ["SessionStart", "UserPromptSubmit", "Notification", "Stop"] {
            let cmd = v["hooks"][ev][0]["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.contains("/abs/claude-deck"), "{ev} cmd = {cmd}");
            assert!(cmd.contains("__hook"), "{ev}");
            assert!(cmd.contains("/tmp/x.sock"), "{ev}");
            assert_eq!(v["hooks"][ev][0]["hooks"][0]["type"], "command");
        }
    }
}
