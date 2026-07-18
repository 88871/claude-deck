//! Scan `~/.claude/projects/**/*.jsonl` to build a list of past Claude Code
//! conversations that can be reopened inside claude-deck.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A single past conversation found on disk.
#[derive(Debug, Clone)]
pub struct Past {
    /// UUID that matches the `claude --resume <id>` flag.
    pub id: String,
    /// The working directory the session was started in (from the `cwd` field
    /// inside the JSONL file — NOT decoded from the directory name).
    pub cwd: PathBuf,
    /// Human-readable title: first user-message text, or cwd's final component.
    pub title: String,
    /// File modification time (used for recency sorting).
    pub mtime: SystemTime,
}

/// Public entry point: scan `~/.claude/projects` and return up to `limit`
/// past sessions, newest first.  Returns an empty vec on any I/O error.
pub fn scan(limit: usize) -> Vec<Past> {
    let base = match dirs::home_dir() {
        Some(h) => h.join(".claude/projects"),
        None => return Vec::new(),
    };
    scan_in(&base, limit)
}

/// Parse a line of JSONL and return the value of a top-level string field.
/// Returns `None` if the line is not valid JSON or the field is absent / not a string.
fn get_str_field(line: &str, field: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    v.get(field)?.as_str().map(|s| s.to_string())
}

/// Extract the title text from a `user` message line.
///
/// Content can be:
/// - a plain string: `{"type":"user","message":{"content":"hello"}}`
/// - a list of content blocks: `{"type":"user","message":{"content":[{"type":"text","text":"hello"},...]}}`
fn extract_user_title(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("type")?.as_str()? != "user" {
        return None;
    }
    let content = v.get("message")?.get("content")?;
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(blocks) => {
            // Find the first block with type == "text" and return its "text" field.
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        return Some(text.to_string());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Truncate a string to `max` Unicode scalar values, appending `…` if cut.
fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let take = max.saturating_sub(1);
        let mut out: String = chars[..take].iter().collect();
        out.push('…');
        out
    }
}

/// Parse a single `.jsonl` session file at `path` into a `Past`, or `None`
/// if the file cannot be read, has no `cwd`, or the id cannot be determined.
///
/// Extracted so that both `scan_in` and `scan_for_cwd_in` can reuse it.
fn parse_session_file(path: &Path, mtime: SystemTime) -> Option<Past> {
    // id = filename stem (UUID).
    let id = path.file_stem().and_then(|s| s.to_str())?.to_string();

    let content = std::fs::read_to_string(path).ok()?;

    let mut cwd_opt: Option<PathBuf> = None;
    let mut title_opt: Option<String> = None;

    // Read up to ~40 lines.
    for line in content.lines().take(40) {
        if line.trim().is_empty() {
            continue;
        }

        // Try to pick up a cwd field.
        if cwd_opt.is_none() {
            if let Some(c) = get_str_field(line, "cwd") {
                cwd_opt = Some(PathBuf::from(c));
            }
        }

        // Try to pick up the first user message as the title.
        if title_opt.is_none() {
            if let Some(t) = extract_user_title(line) {
                title_opt = Some(t);
            }
        }

        if cwd_opt.is_some() && title_opt.is_some() {
            break;
        }
    }

    // Skip files with no cwd — we cannot resume without knowing where to run.
    let cwd = cwd_opt?;

    // Title fallback: last component of cwd.
    let raw_title = title_opt.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(unknown)")
            .to_string()
    });

    let title = truncate(&raw_title, 60);

    Some(Past { id, cwd, title, mtime })
}

/// Core scanner.  Exposed separately so tests can point at a temp dir.
pub fn scan_in(base: &Path, limit: usize) -> Vec<Past> {
    // Collect all *.jsonl files under base/*/*.jsonl
    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();

    let proj_dir = match std::fs::read_dir(base) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    for proj_entry in proj_dir.flatten() {
        let proj_path = proj_entry.path();
        if !proj_path.is_dir() {
            continue;
        }
        let session_dir = match std::fs::read_dir(&proj_path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for session_entry in session_dir.flatten() {
            let p = session_entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let mtime = p
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((p, mtime));
            }
        }
    }

    // Sort newest-first, then take at most `limit`.
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.truncate(limit);

    let mut results: Vec<Past> = Vec::new();

    for (path, mtime) in files {
        if let Some(past) = parse_session_file(&path, mtime) {
            results.push(past);
        }
    }

    results
}

// ── Directory-scoped resume ───────────────────────────────────────────────────

/// Encode a cwd path to the project directory name that Claude Code uses:
/// replace every `/` and `.` in the path string with `-`.
fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// Normalise a path string for cwd comparison: strip a single trailing `/`
/// so that `/tmp/foo` and `/tmp/foo/` compare equal.
fn norm_cwd(s: &str) -> &str {
    s.trim_end_matches('/')
}

/// Testable core: scan `base/<encoded_cwd>/*.jsonl`, keep only sessions whose
/// internal `cwd` field matches `cwd`, sort by mtime DESC, return up to `limit`.
///
/// `base` is the projects root (normally `~/.claude/projects`).
pub fn scan_for_cwd_in(base: &Path, cwd: &Path, limit: usize) -> Vec<Past> {
    // Normalise: strip trailing separators before encoding so that
    // `/tmp/foo` and `/tmp/foo/` map to the same project directory.
    let cwd_str = cwd.to_string_lossy();
    let cwd_norm = norm_cwd(&cwd_str);
    let encoded = encode_cwd(Path::new(cwd_norm));
    let proj_dir = base.join(&encoded);

    let dir_iter = match std::fs::read_dir(&proj_dir) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let target_norm = cwd_norm;

    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in dir_iter.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let mtime = p
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((p, mtime));
        }
    }

    // Sort newest-first before parsing so we can stop early.
    files.sort_by(|a, b| b.1.cmp(&a.1));

    let mut results: Vec<Past> = Vec::new();
    for (path, mtime) in files {
        if let Some(past) = parse_session_file(&path, mtime) {
            // Guard against encoding collisions: verify the internal cwd matches.
            let internal = past.cwd.to_string_lossy();
            if norm_cwd(&internal) == target_norm {
                results.push(past);
                if results.len() >= limit {
                    break;
                }
            }
        }
    }

    results
}

/// Public entry point: scan `~/.claude/projects/<encoded_cwd>` and return up
/// to `limit` past sessions for that specific directory, newest first.
/// Returns an empty vec if the projects dir is missing or there is no history.
pub fn scan_for_cwd(cwd: &Path, limit: usize) -> Vec<Past> {
    let base = match dirs::home_dir() {
        Some(h) => h.join(".claude/projects"),
        None => return Vec::new(),
    };
    scan_for_cwd_in(&base, cwd, limit)
}

// ── Relative time formatter ───────────────────────────────────────────────────

/// Format a file mtime relative to `now` in a human-friendly way.
///
/// Examples: "just now", "3h ago", "2d ago", "5w ago".
pub fn rel_time(mtime: SystemTime, now: SystemTime) -> String {
    let secs = now
        .duration_since(mtime)
        .unwrap_or_default()
        .as_secs();

    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 7 * 86400 {
        format!("{}d ago", secs / 86400)
    } else {
        format!("{}w ago", secs / (7 * 86400))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    /// Create a temp directory with a given structure and return its path.
    fn make_base() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "cdeck-resume-test-{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn write_jsonl(path: &Path, lines: &[&str]) {
        let content = lines.join("\n") + "\n";
        std::fs::write(path, content.as_bytes()).unwrap();
    }

    // ── scan_in: basic happy path ─────────────────────────────────────────────

    #[test]
    fn scan_in_basic_returns_entry_with_cwd_and_title() {
        let base = make_base();
        let proj = base.join("proj1");
        std::fs::create_dir_all(&proj).unwrap();
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let file = proj.join(format!("{}.jsonl", uuid));
        write_jsonl(&file, &[
            r#"{"cwd":"/tmp/foo","type":"x"}"#,
            r#"{"type":"user","message":{"content":"hello world"}}"#,
        ]);

        let results = scan_in(&base, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, uuid);
        assert_eq!(results[0].cwd, PathBuf::from("/tmp/foo"));
        assert_eq!(results[0].title, "hello world");

        let _ = std::fs::remove_dir_all(&base);
    }

    // ── scan_in: file with no cwd is skipped ─────────────────────────────────

    #[test]
    fn scan_in_skips_file_with_no_cwd() {
        let base = make_base();
        let proj = base.join("proj1");
        std::fs::create_dir_all(&proj).unwrap();
        let uuid = "550e8400-e29b-41d4-a716-446655440001";
        let file = proj.join(format!("{}.jsonl", uuid));
        // No cwd field anywhere.
        write_jsonl(&file, &[
            r#"{"type":"user","message":{"content":"some message"}}"#,
        ]);

        let results = scan_in(&base, 10);
        assert_eq!(results.len(), 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    // ── scan_in: mtime-desc ordering with two files ───────────────────────────

    #[test]
    fn scan_in_returns_newest_first() {
        let base = make_base();

        let proj_a = base.join("proja");
        let proj_b = base.join("projb");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();

        let uuid_a = "aaaaaaaa-0000-0000-0000-000000000001";
        let uuid_b = "bbbbbbbb-0000-0000-0000-000000000002";

        let file_a = proj_a.join(format!("{}.jsonl", uuid_a));
        let file_b = proj_b.join(format!("{}.jsonl", uuid_b));

        write_jsonl(&file_a, &[r#"{"cwd":"/tmp/a","type":"x"}"#]);
        // Sleep a tiny bit to guarantee different mtime.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_jsonl(&file_b, &[r#"{"cwd":"/tmp/b","type":"x"}"#]);

        let results = scan_in(&base, 10);
        assert_eq!(results.len(), 2);
        // Newest (b) must come first.
        assert_eq!(results[0].id, uuid_b);
        assert_eq!(results[1].id, uuid_a);

        let _ = std::fs::remove_dir_all(&base);
    }

    // ── scan_in: list-of-blocks content ──────────────────────────────────────

    #[test]
    fn scan_in_parses_list_of_blocks_content() {
        let base = make_base();
        let proj = base.join("proj1");
        std::fs::create_dir_all(&proj).unwrap();
        let uuid = "550e8400-e29b-41d4-a716-446655440002";
        let file = proj.join(format!("{}.jsonl", uuid));
        write_jsonl(&file, &[
            r#"{"cwd":"/tmp/blocks","type":"x"}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"block title"},{"type":"image","source":{}}]}}"#,
        ]);

        let results = scan_in(&base, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "block title");

        let _ = std::fs::remove_dir_all(&base);
    }

    // ── scan_in: title fallback to cwd component ─────────────────────────────

    #[test]
    fn scan_in_title_fallback_is_cwd_last_component() {
        let base = make_base();
        let proj = base.join("proj1");
        std::fs::create_dir_all(&proj).unwrap();
        let uuid = "550e8400-e29b-41d4-a716-446655440003";
        let file = proj.join(format!("{}.jsonl", uuid));
        // cwd but no user message.
        write_jsonl(&file, &[r#"{"cwd":"/home/user/my-project","type":"x"}"#]);

        let results = scan_in(&base, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "my-project");

        let _ = std::fs::remove_dir_all(&base);
    }

    // ── rel_time ──────────────────────────────────────────────────────────────

    #[test]
    fn rel_time_just_now() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mtime = now - Duration::from_secs(30);
        assert_eq!(rel_time(mtime, now), "just now");
    }

    #[test]
    fn rel_time_minutes() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mtime = now - Duration::from_secs(5 * 60);
        assert_eq!(rel_time(mtime, now), "5m ago");
    }

    #[test]
    fn rel_time_hours() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mtime = now - Duration::from_secs(3 * 3600);
        assert_eq!(rel_time(mtime, now), "3h ago");
    }

    #[test]
    fn rel_time_days() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mtime = now - Duration::from_secs(2 * 86400);
        assert_eq!(rel_time(mtime, now), "2d ago");
    }

    #[test]
    fn rel_time_weeks() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mtime = now - Duration::from_secs(3 * 7 * 86400);
        assert_eq!(rel_time(mtime, now), "3w ago");
    }

    #[test]
    fn rel_time_future_mtime_is_just_now() {
        // If mtime is in the future, duration_since returns an error; should show "just now".
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mtime = now + Duration::from_secs(60);
        assert_eq!(rel_time(mtime, now), "just now");
    }

    // ── scan_for_cwd_in ───────────────────────────────────────────────────────

    #[test]
    fn scan_for_cwd_in_returns_matching_session() {
        let base = make_base();
        // Claude encodes /tmp/foo → -tmp-foo
        let proj = base.join("-tmp-foo");
        std::fs::create_dir_all(&proj).unwrap();
        let uuid = "cccccccc-0000-0000-0000-000000000001";
        let file = proj.join(format!("{}.jsonl", uuid));
        write_jsonl(&file, &[
            r#"{"cwd":"/tmp/foo","type":"x"}"#,
            r#"{"type":"user","message":{"content":"scoped session"}}"#,
        ]);

        let results = scan_for_cwd_in(&base, std::path::Path::new("/tmp/foo"), 50);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, uuid);
        assert_eq!(results[0].cwd, PathBuf::from("/tmp/foo"));
        assert_eq!(results[0].title, "scoped session");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_for_cwd_in_different_cwd_returns_empty() {
        let base = make_base();
        let proj = base.join("-tmp-foo");
        std::fs::create_dir_all(&proj).unwrap();
        let uuid = "cccccccc-0000-0000-0000-000000000002";
        let file = proj.join(format!("{}.jsonl", uuid));
        write_jsonl(&file, &[r#"{"cwd":"/tmp/foo","type":"x"}"#]);

        // Looking for /tmp/bar — different cwd → empty.
        let results = scan_for_cwd_in(&base, std::path::Path::new("/tmp/bar"), 50);
        assert_eq!(results.len(), 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_for_cwd_in_missing_dir_returns_empty() {
        let base = make_base();
        // Don't create any subdirectory — should return empty without panicking.
        let results = scan_for_cwd_in(&base, std::path::Path::new("/nonexistent/path"), 50);
        assert_eq!(results.len(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_for_cwd_in_trailing_slash_is_ignored() {
        let base = make_base();
        let proj = base.join("-tmp-foo");
        std::fs::create_dir_all(&proj).unwrap();
        let uuid = "cccccccc-0000-0000-0000-000000000003";
        let file = proj.join(format!("{}.jsonl", uuid));
        // internal cwd has no trailing slash
        write_jsonl(&file, &[r#"{"cwd":"/tmp/foo","type":"x"}"#]);

        // Query with trailing slash → should still match.
        let results = scan_for_cwd_in(&base, std::path::Path::new("/tmp/foo/"), 50);
        assert_eq!(results.len(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }
}
