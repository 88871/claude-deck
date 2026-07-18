use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

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
