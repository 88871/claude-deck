/// Write the terminal bell character to stdout and flush. Errors are silently
/// ignored — a failed bell must never interrupt the app.
pub fn bell() {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

/// Escape a session label for use inside an AppleScript double-quoted string.
/// Backslashes are escaped first, then double-quotes, so neither can break the
/// string literal.
pub fn escape_label(label: &str) -> String {
    label.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Spawn a detached `osascript` process that fires a macOS desktop notification
/// for the given session label. Errors and non-macOS targets are silently
/// ignored.
///
/// `label` is sanitised via `escape_label` so that any `"` or `\` characters
/// cannot break the AppleScript string literal.
pub fn desktop(label: &str) {
    let safe = escape_label(label);
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
    use super::escape_label;

    #[test]
    fn escape_label_escapes_double_quotes() {
        assert_eq!(escape_label(r#"my "project""#), r#"my \"project\""#);
    }

    #[test]
    fn escape_label_escapes_backslashes() {
        assert_eq!(escape_label(r#"C:\Users\proj"#), r#"C:\\Users\\proj"#);
    }

    #[test]
    fn escape_label_escapes_backslash_before_quote() {
        // A label like `foo\"bar` → `foo\\\"bar` (backslash then quote both escaped)
        assert_eq!(escape_label(r#"foo\"bar"#), r#"foo\\\"bar"#);
    }

    #[test]
    fn escape_label_plain_label_unchanged() {
        assert_eq!(escape_label("my-project"), "my-project");
    }
}
