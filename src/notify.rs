/// Write the terminal bell character to stdout and flush. Errors are silently
/// ignored — a failed bell must never interrupt the app.
pub fn bell() {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

/// Pure helper: build the notification body string for a session label.
/// Used by `desktop` and unit-tested without invoking the OS.
pub fn body_for(label: &str) -> String {
    format!("{label} needs you")
}

/// Fire a desktop notification for the given session label via `notify-rust`.
/// Errors are silently ignored — a failed notification must never crash the app.
pub fn desktop(label: &str) {
    let _ = notify_rust::Notification::new()
        .summary("claude-deck")
        .body(&body_for(label))
        .show();
}

#[cfg(test)]
mod tests {
    use super::body_for;

    #[test]
    fn body_for_includes_label() {
        let body = body_for("my-project");
        assert!(body.contains("my-project"), "body should contain the label");
    }

    #[test]
    fn body_for_includes_label_with_special_chars() {
        let label = r#"my "special" project"#;
        let body = body_for(label);
        assert!(body.contains(label), "body should contain the full label verbatim");
    }

    #[test]
    fn body_for_format() {
        assert_eq!(body_for("work"), "work needs you");
    }
}
