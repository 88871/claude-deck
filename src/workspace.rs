use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// A single entry in the persisted workspace snapshot.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub cwd: PathBuf,
    pub pinned: bool,
}

/// Cross-platform path to the workspace file.
/// Returns `None` only if `dirs::config_dir()` itself returns `None`
/// (i.e. the OS cannot locate a config directory — extremely unusual).
pub fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("claude-deck").join("workspace.json"))
}

/// Write `entries` as pretty JSON to `path`, creating parent directories as
/// needed.  All errors are silently ignored — a failed save is non-fatal.
pub fn save_to(path: &Path, entries: &[Entry]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(entries) {
        let _ = std::fs::write(path, json);
    }
}

/// Read and parse the workspace JSON at `path`.
/// Returns an empty `Vec` on any error (missing file, parse failure, etc.).
pub fn load_from(path: &Path) -> Vec<Entry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist `entries` to the canonical config-dir path.
/// No-op (silently) when `path()` returns `None`.
pub fn save(entries: &[Entry]) {
    if let Some(p) = path() {
        save_to(&p, entries);
    }
}

/// Load entries from the canonical config-dir path.
/// Returns an empty `Vec` when `path()` is `None` or on any I/O / parse error.
pub fn load() -> Vec<Entry> {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_entries() -> Vec<Entry> {
        vec![
            Entry {
                id: "aaaa-1111".to_string(),
                label: "my-project".to_string(),
                cwd: PathBuf::from("/tmp/my-project"),
                pinned: true,
            },
            Entry {
                id: "bbbb-2222".to_string(),
                label: "other".to_string(),
                cwd: PathBuf::from("/tmp/other"),
                pinned: false,
            },
        ]
    }

    /// Round-trip: save_to → load_from should reproduce the original entries.
    #[test]
    fn round_trip_save_load() {
        let p = std::env::temp_dir().join("cdeck-ws-test-roundtrip.json");
        let entries = sample_entries();
        save_to(&p, &entries);
        let loaded = load_from(&p);
        assert_eq!(loaded, entries);
        // Clean up.
        let _ = std::fs::remove_file(&p);
    }

    /// load_from a nonexistent path returns an empty Vec (no panic).
    #[test]
    fn load_from_nonexistent_returns_empty() {
        let p = std::env::temp_dir().join("cdeck-ws-test-nonexistent-xyz.json");
        // Make sure it really doesn't exist.
        let _ = std::fs::remove_file(&p);
        let loaded = load_from(&p);
        assert!(loaded.is_empty());
    }

    /// load_from a file containing garbage JSON returns an empty Vec.
    #[test]
    fn load_from_garbage_returns_empty() {
        let p = std::env::temp_dir().join("cdeck-ws-test-garbage.json");
        std::fs::write(&p, b"not valid json {{{{ ").unwrap();
        let loaded = load_from(&p);
        assert!(loaded.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    /// Round-trip with an empty slice.
    #[test]
    fn round_trip_empty() {
        let p = std::env::temp_dir().join("cdeck-ws-test-empty.json");
        save_to(&p, &[]);
        let loaded = load_from(&p);
        assert!(loaded.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    /// Round-trip preserves order.
    #[test]
    fn round_trip_order_preserved() {
        let p = std::env::temp_dir().join("cdeck-ws-test-order.json");
        let entries = sample_entries();
        save_to(&p, &entries);
        let loaded = load_from(&p);
        assert_eq!(loaded[0].id, "aaaa-1111");
        assert_eq!(loaded[1].id, "bbbb-2222");
        let _ = std::fs::remove_file(&p);
    }
}
