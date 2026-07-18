use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use serde::Deserialize;

/// A deserialized Claude Code hook payload. Extra JSON fields are ignored.
#[derive(Deserialize, Debug, Clone)]
pub struct HookEvent {
    pub session_id: String,
    #[serde(rename = "hook_event_name")]
    pub event: String,
    #[serde(default)]
    pub notification_type: Option<String>,
    /// The human-readable message from a `Notification` payload, if present.
    #[serde(default)]
    pub message: Option<String>,
}

/// Bind a `TcpListener` on `127.0.0.1:0` (OS-assigned port), spawn a
/// background accept loop that calls `on_event` for each incoming well-formed
/// `HookEvent` payload, and return the assigned port number.
///
/// Bad/unparseable payloads are dropped silently. Each accepted connection is
/// handled in its own short-lived thread so that one slow or hung client cannot
/// stall the accept loop or block other hooks.
pub fn listen<F: Fn(HookEvent) + Send + Sync + 'static>(
    on_event: F,
) -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let on_event = std::sync::Arc::new(on_event);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let handler = std::sync::Arc::clone(&on_event);
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if std::io::Read::read_to_end(&mut stream, &mut buf).is_ok() {
                    if let Ok(ev) = serde_json::from_slice::<HookEvent>(&buf) {
                        handler(ev);
                    }
                }
            });
        }
    });
    Ok(port)
}

/// Returns the settings file path — under `temp_dir()`, named after this
/// process's PID so multiple instances don't collide.
pub fn settings_path() -> std::path::PathBuf {
    let pid = std::process::id();
    std::env::temp_dir().join(format!("claude-deck-{pid}-settings.json"))
}

/// Build the shared settings JSON. Each hook invokes our own binary as a
/// forwarder using an ABSOLUTE path (hooks don't source shell profiles).
pub fn settings_json(binary: &str, port: &str) -> String {
    let cmd = format!("{binary} __hook {port}");
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
    port: &str,
) -> std::io::Result<()> {
    let binary = std::env::current_exe()?.to_string_lossy().into_owned();
    std::fs::write(settings_path, settings_json(&binary, port))
}

/// `claude-deck __hook <port>`: read the hook payload from stdin and forward
/// it to the app's TCP listener. Never fails loudly — a broken forward must
/// not break the claude session.
pub fn forward(port: &str) -> std::io::Result<()> {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let port_num = port.parse::<u16>().unwrap_or(0);
    if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port_num)) {
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
        assert_eq!(e.message, None);
        let notif = r#"{"session_id":"abc","hook_event_name":"Notification","notification_type":"permission_prompt"}"#;
        let n: HookEvent = serde_json::from_str(notif).unwrap();
        assert_eq!(n.notification_type.as_deref(), Some("permission_prompt"));
        assert_eq!(n.message, None);
    }

    #[test]
    fn parses_notification_with_message() {
        let json = r#"{"session_id":"xyz","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Allow bash command?"}"#;
        let ev: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.message.as_deref(), Some("Allow bash command?"));
    }

    #[test]
    fn message_defaults_to_none_when_absent() {
        let json = r#"{"session_id":"xyz","hook_event_name":"Stop"}"#;
        let ev: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.message, None);
    }

    #[test]
    fn settings_json_registers_hooks_pointing_at_our_binary() {
        let json = settings_json("/abs/claude-deck", "54321");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        for ev in ["SessionStart", "UserPromptSubmit", "Notification", "Stop"] {
            let cmd = v["hooks"][ev][0]["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.contains("/abs/claude-deck"), "{ev} cmd = {cmd}");
            assert!(cmd.contains("__hook"), "{ev}");
            assert!(cmd.contains("54321"), "{ev}");
            assert_eq!(v["hooks"][ev][0]["hooks"][0]["type"], "command");
        }
    }
}
