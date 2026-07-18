/// Write the terminal bell character to stdout and flush. Errors are silently
/// ignored — a failed bell must never interrupt the app.
pub fn bell() {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

/// Spawn a detached `osascript` process that fires a macOS desktop notification
/// for the given session label. Errors and non-macOS targets are silently
/// ignored.
///
/// `label` is sanitised so that any `"` or `\` characters cannot break the
/// AppleScript string literal.
pub fn desktop(label: &str) {
    // Escape backslashes first, then double-quotes, to produce a safe
    // AppleScript string literal.
    let safe = label.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{safe} needs you\" with title \"claude-deck\""
    );
    // Spawn detached — we don't wait for the process to finish.
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .spawn();
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_double_quotes_in_label() {
        // A label with quotes must not break the AppleScript string.
        // We can't easily test the actual osascript invocation, but we can
        // verify the escaping logic by reconstructing the script string.
        let label = r#"my "project""#;
        let safe = label.replace('\\', "\\\\").replace('"', "\\\"");
        assert_eq!(safe, r#"my \"project\""#);
        // The script must contain the escaped label.
        let script = format!(
            "display notification \"{safe} needs you\" with title \"claude-deck\""
        );
        assert!(script.contains(r#"my \"project\""#));
    }

    #[test]
    fn escapes_backslashes_in_label() {
        let label = r#"C:\Users\proj"#;
        let safe = label.replace('\\', "\\\\").replace('"', "\\\"");
        assert_eq!(safe, r#"C:\\Users\\proj"#);
    }
}
