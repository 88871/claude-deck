use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Persisted user configuration for claude-deck.
///
/// Loaded at startup and applied before CLI flags (flags override for the
/// current run only; toggling a setting from the UI updates and saves this
/// struct immediately).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    /// When true, ALL alert types (bell, desktop, ntfy) are suppressed.
    pub dnd: bool,
    /// Ring the terminal bell when a session needs attention.
    pub bell: bool,
    /// Fire a desktop notification when a session needs attention.
    pub desktop_notify: bool,
    /// ntfy.sh topic for phone push notifications. `None` = disabled.
    pub ntfy_topic: Option<String>,
    /// Automatically park sessions that have been Idle for `reap_timeout_secs`.
    pub reap_idle: bool,
    /// Seconds a session must be Idle before it is auto-parked (when `reap_idle`).
    pub reap_timeout_secs: u64,
    /// RSS warning threshold in megabytes. `0` disables the warning.
    pub mem_warn_mb: u64,
    /// Use Nerd Font glyphs. When false, unicode/ASCII fallbacks are used.
    pub nerd_icons: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dnd: false,
            bell: true,
            desktop_notify: true,
            ntfy_topic: None,
            reap_idle: false,
            reap_timeout_secs: 600,
            mem_warn_mb: 4096,
            nerd_icons: false,
        }
    }
}

/// Cross-platform path to the config file.
/// Returns `None` only if `dirs::config_dir()` returns `None`.
pub fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("claude-deck").join("config.json"))
}

/// Write `config` as pretty JSON to `path`, creating parent directories as needed.
/// All errors are silently ignored — a failed save is non-fatal.
pub fn save_to(path: &Path, config: &Config) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(config) {
        let _ = std::fs::write(path, json);
    }
}

/// Read and parse the config JSON at `path`.
/// Returns `Config::default()` on any error (missing file, parse failure, etc.).
pub fn load_from(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist `config` to the canonical config-dir path.
/// No-op (silently) when `path()` returns `None`.
pub fn save(config: &Config) {
    if let Some(p) = path() {
        save_to(&p, config);
    }
}

/// Load config from the canonical config-dir path.
/// Returns `Config::default()` when `path()` is `None` or on any I/O / parse error.
pub fn load() -> Config {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn nondefault_config() -> Config {
        Config {
            dnd: true,
            bell: false,
            desktop_notify: false,
            ntfy_topic: Some("my-topic".to_string()),
            reap_idle: true,
            reap_timeout_secs: 300,
            mem_warn_mb: 2048,
            nerd_icons: true,
        }
    }

    /// Round-trip: save_to → load_from should reproduce the original config.
    #[test]
    fn round_trip_save_load() {
        let p = std::env::temp_dir().join("cdeck-config-test-roundtrip.json");
        let cfg = nondefault_config();
        save_to(&p, &cfg);
        let loaded = load_from(&p);
        assert_eq!(loaded, cfg);
        let _ = std::fs::remove_file(&p);
    }

    /// Round-trip with Default values.
    #[test]
    fn round_trip_default() {
        let p = std::env::temp_dir().join("cdeck-config-test-default.json");
        let cfg = Config::default();
        save_to(&p, &cfg);
        let loaded = load_from(&p);
        assert_eq!(loaded, cfg);
        let _ = std::fs::remove_file(&p);
    }

    /// load_from a nonexistent path returns Config::default() (no panic).
    #[test]
    fn load_from_nonexistent_returns_default() {
        let p = std::env::temp_dir().join("cdeck-config-test-nonexistent-xyz.json");
        let _ = std::fs::remove_file(&p);
        let loaded = load_from(&p);
        assert_eq!(loaded, Config::default());
    }

    /// load_from a file containing garbage JSON returns Config::default().
    #[test]
    fn load_from_garbage_returns_default() {
        let p = std::env::temp_dir().join("cdeck-config-test-garbage.json");
        std::fs::write(&p, b"not valid json {{{{ ").unwrap();
        let loaded = load_from(&p);
        assert_eq!(loaded, Config::default());
        let _ = std::fs::remove_file(&p);
    }

    /// Check that default values match the spec.
    #[test]
    fn default_values_match_spec() {
        let cfg = Config::default();
        assert!(!cfg.dnd);
        assert!(cfg.bell);
        assert!(cfg.desktop_notify);
        assert!(cfg.ntfy_topic.is_none());
        assert!(!cfg.reap_idle);
        assert_eq!(cfg.reap_timeout_secs, 600);
        assert_eq!(cfg.mem_warn_mb, 4096);
        assert!(!cfg.nerd_icons);
    }
}
