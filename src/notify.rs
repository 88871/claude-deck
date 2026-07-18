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

/// POST a push notification to ntfy.sh/<topic> on a short-lived background
/// thread.  Best-effort: all errors are silently discarded so the UI is never
/// blocked or crashed by a network issue.
///
/// `title` becomes the ntfy `Title` header; `body` is the message text.
pub fn push_ntfy(topic: &str, title: &str, body: &str) {
    let url = format!("https://ntfy.sh/{topic}");
    let title = title.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let _ = ureq::post(&url)
            .set("Title", &title)
            .timeout(std::time::Duration::from_secs(5))
            .send_string(&body);
    });
}

/// Pure helper: derive the ntfy topic from `args` and `env_val`.
/// `--ntfy <topic>` in args wins over the env var; absent = `None`.
///
/// This is a pure function so it can be unit-tested without side effects.
pub fn ntfy_from<'a>(args: &'a [String], env_val: Option<&'a str>) -> Option<String> {
    // arg wins
    if let Some(topic) = args.windows(2).find(|w| w[0] == "--ntfy").map(|w| w[1].as_str()) {
        if !topic.is_empty() {
            return Some(topic.to_string());
        }
    }
    // fall back to env
    env_val.filter(|s| !s.is_empty()).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── ntfy_from ──────────────────────────────────────────────────────────────

    #[test]
    fn ntfy_from_arg_wins_over_env() {
        let args = vec!["claude-deck".to_string(), "--ntfy".to_string(), "arg-topic".to_string()];
        assert_eq!(ntfy_from(&args, Some("env-topic")), Some("arg-topic".to_string()));
    }

    #[test]
    fn ntfy_from_falls_back_to_env() {
        let args = vec!["claude-deck".to_string()];
        assert_eq!(ntfy_from(&args, Some("env-topic")), Some("env-topic".to_string()));
    }

    #[test]
    fn ntfy_from_none_when_both_absent() {
        let args = vec!["claude-deck".to_string()];
        assert_eq!(ntfy_from(&args, None), None);
    }

    #[test]
    fn ntfy_from_none_when_env_empty_and_no_arg() {
        let args = vec!["claude-deck".to_string()];
        assert_eq!(ntfy_from(&args, Some("")), None);
    }

    #[test]
    fn ntfy_from_arg_empty_falls_back_to_env() {
        // --ntfy with an empty string value: treated as absent, env wins
        let args = vec!["claude-deck".to_string(), "--ntfy".to_string(), "".to_string()];
        assert_eq!(ntfy_from(&args, Some("env-topic")), Some("env-topic".to_string()));
    }

    #[test]
    fn ntfy_from_no_env_no_arg_returns_none() {
        let args: Vec<String> = vec![];
        assert_eq!(ntfy_from(&args, None), None);
    }
}
