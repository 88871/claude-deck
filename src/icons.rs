use crate::session::SessionState;
use ratatui::style::Color;

// ── Icon mode ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconMode {
    Nerd,
    Ascii,
}

// ── Glyph table ───────────────────────────────────────────────────────────────

const HOME_NERD:  &str = "\u{f015}"; //
const HOME_ASCII: &str = "⌂";

const FOLDER_NERD:  &str = "\u{f07b}"; //
const FOLDER_ASCII: &str = "▸";

// State glyphs: (nerd, ascii)
const STARTING_NERD:       &str = "\u{f252}"; //
const STARTING_ASCII:      &str = "○";
const RUNNING_NERD:        &str = "\u{f04b}"; //
const RUNNING_ASCII:       &str = "●";
const WAITING_NERD:        &str = "\u{f0f3}"; //
const WAITING_ASCII:       &str = "◍";
const IDLE_NERD:           &str = "\u{f00c}"; //
const IDLE_ASCII:          &str = "✓";
const PARKED_NERD:         &str = "\u{f04c}"; //
const PARKED_ASCII:        &str = "◌";
const CLOSED_NERD:         &str = "\u{f04d}"; //
const CLOSED_ASCII:        &str = "⏹";
const ERROR_NERD:          &str = "\u{f00d}"; //
const ERROR_ASCII:         &str = "✗";

// ── Public accessors ──────────────────────────────────────────────────────────

pub fn home(mode: IconMode) -> &'static str {
    match mode {
        IconMode::Nerd  => HOME_NERD,
        IconMode::Ascii => HOME_ASCII,
    }
}

pub fn folder(mode: IconMode) -> &'static str {
    match mode {
        IconMode::Nerd  => FOLDER_NERD,
        IconMode::Ascii => FOLDER_ASCII,
    }
}

pub fn state(s: SessionState, mode: IconMode) -> &'static str {
    use SessionState::*;
    match (s, mode) {
        (Starting,     IconMode::Nerd)  => STARTING_NERD,
        (Starting,     IconMode::Ascii) => STARTING_ASCII,
        (Running,      IconMode::Nerd)  => RUNNING_NERD,
        (Running,      IconMode::Ascii) => RUNNING_ASCII,
        (WaitingOnYou, IconMode::Nerd)  => WAITING_NERD,
        (WaitingOnYou, IconMode::Ascii) => WAITING_ASCII,
        (Idle,         IconMode::Nerd)  => IDLE_NERD,
        (Idle,         IconMode::Ascii) => IDLE_ASCII,
        (Parked,       IconMode::Nerd)  => PARKED_NERD,
        (Parked,       IconMode::Ascii) => PARKED_ASCII,
        (Closed,       IconMode::Nerd)  => CLOSED_NERD,
        (Closed,       IconMode::Ascii) => CLOSED_ASCII,
        (Error,        IconMode::Nerd)  => ERROR_NERD,
        (Error,        IconMode::Ascii) => ERROR_ASCII,
    }
}

// ── State color ───────────────────────────────────────────────────────────────

pub fn state_color(s: SessionState) -> Color {
    use SessionState::*;
    match s {
        Running      => Color::Green,
        WaitingOnYou => Color::Yellow,
        Idle         => Color::Green,
        Starting     => Color::DarkGray,
        Parked       => Color::DarkGray,
        Closed       => Color::DarkGray,
        Error        => Color::Red,
    }
}

// ── Mode detection ────────────────────────────────────────────────────────────

/// Pure function: given CLI args and whether the nerd env var is set,
/// return the appropriate IconMode. Testable without touching the real env.
///
/// Default is `IconMode::Ascii` (universal Unicode symbols, works in any
/// terminal). `IconMode::Nerd` is returned only when the user explicitly
/// opts in via `--nerd` in args or `nerd_env == true`.
///
/// `--ascii` / `CLAUDE_DECK_ICONS=ascii` are kept as explicit no-ops for
/// backward-compat (they select Ascii, which is already the default).
pub fn mode_from(args: &[String], nerd_env: bool) -> IconMode {
    if nerd_env || args.iter().any(|a| a == "--nerd") {
        IconMode::Nerd
    } else {
        IconMode::Ascii
    }
}

/// Detect the icon mode from the real process environment and CLI args.
pub fn detect_mode() -> IconMode {
    let args: Vec<String> = std::env::args().collect();
    let nerd_env = std::env::var("CLAUDE_DECK_ICONS")
        .map(|v| v.eq_ignore_ascii_case("nerd"))
        .unwrap_or(false);
    mode_from(&args, nerd_env)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::*;

    // ── home / folder differ by mode ─────────────────────────────────────────

    #[test]
    fn home_glyphs_differ_by_mode() {
        assert_ne!(home(IconMode::Nerd), home(IconMode::Ascii));
        assert_eq!(home(IconMode::Nerd),  HOME_NERD);
        assert_eq!(home(IconMode::Ascii), HOME_ASCII);
    }

    #[test]
    fn folder_glyphs_differ_by_mode() {
        assert_ne!(folder(IconMode::Nerd), folder(IconMode::Ascii));
        assert_eq!(folder(IconMode::Nerd),  FOLDER_NERD);
        assert_eq!(folder(IconMode::Ascii), FOLDER_ASCII);
    }

    // ── each SessionState maps to expected glyph in both modes ───────────────

    #[test]
    fn state_starting_nerd() {
        assert_eq!(state(Starting, IconMode::Nerd), STARTING_NERD);
    }

    #[test]
    fn state_starting_ascii() {
        assert_eq!(state(Starting, IconMode::Ascii), STARTING_ASCII);
    }

    #[test]
    fn state_running_nerd() {
        assert_eq!(state(Running, IconMode::Nerd), RUNNING_NERD);
    }

    #[test]
    fn state_running_ascii() {
        assert_eq!(state(Running, IconMode::Ascii), RUNNING_ASCII);
    }

    #[test]
    fn state_waiting_nerd() {
        assert_eq!(state(WaitingOnYou, IconMode::Nerd), WAITING_NERD);
    }

    #[test]
    fn state_waiting_ascii() {
        assert_eq!(state(WaitingOnYou, IconMode::Ascii), WAITING_ASCII);
    }

    #[test]
    fn state_idle_nerd() {
        assert_eq!(state(Idle, IconMode::Nerd), IDLE_NERD);
    }

    #[test]
    fn state_idle_ascii() {
        assert_eq!(state(Idle, IconMode::Ascii), IDLE_ASCII);
    }

    #[test]
    fn state_parked_nerd() {
        assert_eq!(state(Parked, IconMode::Nerd), PARKED_NERD);
    }

    #[test]
    fn state_parked_ascii() {
        assert_eq!(state(Parked, IconMode::Ascii), PARKED_ASCII);
    }

    #[test]
    fn state_closed_nerd() {
        assert_eq!(state(Closed, IconMode::Nerd), CLOSED_NERD);
    }

    #[test]
    fn state_closed_ascii() {
        assert_eq!(state(Closed, IconMode::Ascii), CLOSED_ASCII);
    }

    #[test]
    fn state_error_nerd() {
        assert_eq!(state(Error, IconMode::Nerd), ERROR_NERD);
    }

    #[test]
    fn state_error_ascii() {
        assert_eq!(state(Error, IconMode::Ascii), ERROR_ASCII);
    }

    // ── mode_from: pure detection logic ──────────────────────────────────────

    #[test]
    fn mode_from_default_is_ascii() {
        // No args, nerd_env=false → default is Ascii (works in any terminal)
        assert_eq!(mode_from(&[], false), IconMode::Ascii);
    }

    #[test]
    fn mode_from_nerd_flag_enables_nerd() {
        let args = vec!["claude-deck".to_string(), "--nerd".to_string()];
        assert_eq!(mode_from(&args, false), IconMode::Nerd);
    }

    #[test]
    fn mode_from_nerd_env_enables_nerd() {
        // nerd_env=true → Nerd (caller set CLAUDE_DECK_ICONS=nerd)
        assert_eq!(mode_from(&[], true), IconMode::Nerd);
    }

    #[test]
    fn mode_from_nerd_flag_and_env_both_enable_nerd() {
        let args = vec!["--nerd".to_string()];
        assert_eq!(mode_from(&args, true), IconMode::Nerd);
    }

    #[test]
    fn mode_from_ascii_flag_is_noop_stays_ascii() {
        // --ascii is a backward-compat no-op; default is already Ascii
        let args = vec!["claude-deck".to_string(), "--ascii".to_string()];
        assert_eq!(mode_from(&args, false), IconMode::Ascii);
    }

    #[test]
    fn mode_from_other_flags_stay_ascii() {
        let args = vec!["--verbose".to_string(), "--debug".to_string()];
        assert_eq!(mode_from(&args, false), IconMode::Ascii);
    }

    // ── all states produce distinct nerd/ascii glyphs ─────────────────────────

    #[test]
    fn nerd_and_ascii_never_equal_for_any_state() {
        let states = [Starting, Running, WaitingOnYou, Idle, Parked, Closed, Error];
        for s in states {
            assert_ne!(
                state(s, IconMode::Nerd),
                state(s, IconMode::Ascii),
                "nerd and ascii should differ for {s:?}"
            );
        }
    }
}
