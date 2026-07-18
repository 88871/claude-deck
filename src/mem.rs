use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use crate::session::SessionState;
use std::time::Duration;

/// Wraps a `sysinfo::System` for per-process RSS queries.
pub struct Mem {
    sys: System,
}

impl Mem {
    pub fn new() -> Self {
        Self { sys: System::new() }
    }

    /// Refresh the given pid and return its RSS in **kilobytes**, or `None`
    /// if the process is not found.
    ///
    /// sysinfo 0.33 API used:
    ///   `System::refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::nothing().with_memory())`
    ///   then `System::process(pid).map(|p| p.memory() / 1024)`
    pub fn rss_kb(&mut self, pid: u32) -> Option<u64> {
        let sys_pid = Pid::from(pid as usize);
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            false,
            ProcessRefreshKind::nothing().with_memory(),
        );
        self.sys.process(sys_pid).map(|p| p.memory() / 1024)
    }
}

impl Default for Mem {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a kilobyte count as a human-readable string: "1.2G", "834M", "512K".
pub fn fmt_kb(kb: u64) -> String {
    if kb >= 1_048_576 {
        // >= 1 GiB
        let gb = kb as f64 / 1_048_576.0;
        format!("{:.1}G", gb)
    } else if kb >= 1_024 {
        // >= 1 MiB
        let mb = kb as f64 / 1_024.0;
        format!("{:.0}M", mb)
    } else {
        format!("{}K", kb)
    }
}

/// Pure predicate: should we park this session automatically?
///
/// `reap_idle` — the `--reap-idle` flag (default: false).
/// Memory is NOT a park/kill trigger — it is only a warning.
pub fn should_park(
    reap_idle: bool,
    state: SessionState,
    focused: bool,
    pinned: bool,
    idle_for: Duration,
    timeout: Duration,
) -> bool {
    reap_idle && state == SessionState::Idle && !focused && !pinned && idle_for >= timeout
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use std::time::Duration;

    // ── fmt_kb ────────────────────────────────────────────────────────────────

    #[test]
    fn fmt_kb_under_1024_shows_k() {
        assert_eq!(fmt_kb(0), "0K");
        assert_eq!(fmt_kb(1), "1K");
        assert_eq!(fmt_kb(512), "512K");
        assert_eq!(fmt_kb(1023), "1023K");
    }

    #[test]
    fn fmt_kb_exactly_1024_shows_m() {
        assert_eq!(fmt_kb(1024), "1M");
    }

    #[test]
    fn fmt_kb_megabytes_rounded() {
        // 854 * 1024 = 874496 KB → 834 MB? Let's check: 854 MiB = 854 * 1024 KB = 874496 KB
        // 874496 / 1024 = 854.0 → "854M"
        assert_eq!(fmt_kb(854 * 1024), "854M");
        // 512 MiB = 524288 KB → "512M"
        assert_eq!(fmt_kb(512 * 1024), "512M");
    }

    #[test]
    fn fmt_kb_exactly_1_gib() {
        // 1 GiB = 1048576 KB → "1.0G"
        assert_eq!(fmt_kb(1_048_576), "1.0G");
    }

    #[test]
    fn fmt_kb_1_2_gib() {
        // 1.2 GiB = 1258291 KB (roughly)
        let kb = (1.2f64 * 1_048_576.0) as u64;
        assert_eq!(fmt_kb(kb), "1.2G");
    }

    #[test]
    fn fmt_kb_large_value() {
        // 16 GiB
        assert_eq!(fmt_kb(16 * 1_048_576), "16.0G");
    }

    // ── should_park ───────────────────────────────────────────────────────────

    const TIMEOUT: Duration = Duration::from_secs(600);
    const LONG: Duration = Duration::from_secs(700);
    const SHORT: Duration = Duration::from_secs(100);

    /// The "happy path" — all conditions met.
    #[test]
    fn should_park_all_conditions_met_returns_true() {
        assert!(should_park(true, SessionState::Idle, false, false, LONG, TIMEOUT));
    }

    /// Default off — reap_idle=false means never park.
    #[test]
    fn should_park_false_when_reap_idle_off() {
        assert!(!should_park(false, SessionState::Idle, false, false, LONG, TIMEOUT));
    }

    /// Non-Idle states are never parked.
    #[test]
    fn should_park_false_when_running() {
        assert!(!should_park(true, SessionState::Running, false, false, LONG, TIMEOUT));
    }

    #[test]
    fn should_park_false_when_waiting_on_you() {
        assert!(!should_park(true, SessionState::WaitingOnYou, false, false, LONG, TIMEOUT));
    }

    #[test]
    fn should_park_false_when_starting() {
        assert!(!should_park(true, SessionState::Starting, false, false, LONG, TIMEOUT));
    }

    #[test]
    fn should_park_false_when_parked() {
        assert!(!should_park(true, SessionState::Parked, false, false, LONG, TIMEOUT));
    }

    #[test]
    fn should_park_false_when_closed() {
        assert!(!should_park(true, SessionState::Closed, false, false, LONG, TIMEOUT));
    }

    #[test]
    fn should_park_false_when_error() {
        assert!(!should_park(true, SessionState::Error, false, false, LONG, TIMEOUT));
    }

    /// Focused session must not be parked even if idle and timed-out.
    #[test]
    fn should_park_false_when_focused() {
        assert!(!should_park(true, SessionState::Idle, true, false, LONG, TIMEOUT));
    }

    /// Pinned session must not be parked.
    #[test]
    fn should_park_false_when_pinned() {
        assert!(!should_park(true, SessionState::Idle, false, true, LONG, TIMEOUT));
    }

    /// Not yet timed-out.
    #[test]
    fn should_park_false_when_not_timed_out() {
        assert!(!should_park(true, SessionState::Idle, false, false, SHORT, TIMEOUT));
    }

    /// Exactly at the timeout boundary (idle_for == timeout) — must park.
    #[test]
    fn should_park_true_at_exact_timeout_boundary() {
        assert!(should_park(true, SessionState::Idle, false, false, TIMEOUT, TIMEOUT));
    }

    /// Memory is never a park trigger — there is no memory parameter.
    /// (Verified structurally: `should_park` has no memory param.)
    #[test]
    fn should_park_signature_has_no_memory_param() {
        // If this compiles with these exact 6 args, the signature is correct.
        let _ = should_park(true, SessionState::Idle, false, false, LONG, TIMEOUT);
    }
}
