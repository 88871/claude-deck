use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use crate::app::Focus;

/// Shared sidebar width constant — must match the `Constraint::Length` in `ui.rs`.
pub const SIDEBAR_WIDTH: u16 = 26;

/// Map a click's terminal Y-coordinate inside the sidebar to a `Focus`.
///
/// The sidebar has a top border at row 0, so list entries start at row 1:
///   - With `home_visible=true`:  row 1 → `Focus::Home`,
///                                 row 2 → `Focus::Session(0)`,
///                                 row 2+n → `Focus::Session(n)`
///   - With `home_visible=false`: row 1 → `Focus::Session(0)`.
///
/// Row 0 (border) or any row past the last entry → `None`.
pub fn sidebar_hit(row: u16, home_visible: bool, session_count: usize) -> Option<Focus> {
    if row == 0 {
        return None;
    }
    // Convert 1-based row to 0-based list index.
    let idx = (row - 1) as usize;
    if home_visible {
        if idx == 0 {
            return Some(Focus::Home);
        }
        let session_idx = idx - 1;
        if session_idx < session_count {
            Some(Focus::Session(session_idx))
        } else {
            None
        }
    } else {
        if idx < session_count {
            Some(Focus::Session(idx))
        } else {
            None
        }
    }
}

/// Encode a `MouseEvent` as an SGR mouse sequence for the pane-relative
/// 1-based coordinates `(col, row)`.
///
/// SGR button codes:
///   left=0, middle=1, right=2, wheel-up=64, wheel-down=65
/// Press/drag/scroll → final byte `M`; release → final byte `m`.
///
/// Returns `None` for event kinds we don't forward (e.g. Moved, ScrollLeft,
/// ScrollRight).
pub fn encode_sgr(ev: &MouseEvent, col: u16, row: u16) -> Option<Vec<u8>> {
    let (button_code, final_byte) = match ev.kind {
        MouseEventKind::Down(MouseButton::Left)   => (0u16, b'M'),
        MouseEventKind::Down(MouseButton::Middle) => (1,    b'M'),
        MouseEventKind::Down(MouseButton::Right)  => (2,    b'M'),
        MouseEventKind::Up(MouseButton::Left)     => (0,    b'm'),
        MouseEventKind::Up(MouseButton::Middle)   => (1,    b'm'),
        MouseEventKind::Up(MouseButton::Right)    => (2,    b'm'),
        MouseEventKind::Drag(MouseButton::Left)   => (0,    b'M'),
        MouseEventKind::Drag(MouseButton::Middle) => (1,    b'M'),
        MouseEventKind::Drag(MouseButton::Right)  => (2,    b'M'),
        MouseEventKind::ScrollUp                  => (64,   b'M'),
        MouseEventKind::ScrollDown                => (65,   b'M'),
        // Moved, ScrollLeft, ScrollRight → not forwarded.
        _ => return None,
    };
    let seq = format!("\x1b[<{};{};{}{}", button_code, col, row, final_byte as char);
    Some(seq.into_bytes())
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn mouse_ev(kind: MouseEventKind) -> MouseEvent {
        MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE }
    }

    // ── sidebar_hit ───────────────────────────────────────────────────────────

    #[test]
    fn sidebar_hit_row0_is_border_returns_none() {
        assert_eq!(sidebar_hit(0, true, 3), None);
        assert_eq!(sidebar_hit(0, false, 3), None);
    }

    #[test]
    fn sidebar_hit_home_visible_row1_is_home() {
        assert_eq!(sidebar_hit(1, true, 2), Some(Focus::Home));
    }

    #[test]
    fn sidebar_hit_home_visible_row2_is_session0() {
        assert_eq!(sidebar_hit(2, true, 2), Some(Focus::Session(0)));
    }

    #[test]
    fn sidebar_hit_home_visible_row3_is_session1() {
        assert_eq!(sidebar_hit(3, true, 2), Some(Focus::Session(1)));
    }

    #[test]
    fn sidebar_hit_home_visible_past_last_returns_none() {
        // 2 sessions with home_visible: valid rows are 1 (Home), 2 (S0), 3 (S1).
        // Row 4 is past the end.
        assert_eq!(sidebar_hit(4, true, 2), None);
    }

    #[test]
    fn sidebar_hit_home_hidden_row1_is_session0() {
        assert_eq!(sidebar_hit(1, false, 2), Some(Focus::Session(0)));
    }

    #[test]
    fn sidebar_hit_home_hidden_row2_is_session1() {
        assert_eq!(sidebar_hit(2, false, 2), Some(Focus::Session(1)));
    }

    #[test]
    fn sidebar_hit_home_hidden_past_last_returns_none() {
        assert_eq!(sidebar_hit(3, false, 2), None);
    }

    #[test]
    fn sidebar_hit_home_visible_no_sessions_row1_is_home() {
        assert_eq!(sidebar_hit(1, true, 0), Some(Focus::Home));
    }

    #[test]
    fn sidebar_hit_home_visible_no_sessions_row2_returns_none() {
        assert_eq!(sidebar_hit(2, true, 0), None);
    }

    #[test]
    fn sidebar_hit_home_hidden_no_sessions_returns_none() {
        assert_eq!(sidebar_hit(1, false, 0), None);
    }

    // ── encode_sgr ────────────────────────────────────────────────────────────

    #[test]
    fn encode_sgr_left_press_at_1_1() {
        let ev = mouse_ev(MouseEventKind::Down(MouseButton::Left));
        let result = encode_sgr(&ev, 1, 1);
        assert_eq!(result, Some(b"\x1b[<0;1;1M".to_vec()));
    }

    #[test]
    fn encode_sgr_left_release_at_1_1() {
        let ev = mouse_ev(MouseEventKind::Up(MouseButton::Left));
        let result = encode_sgr(&ev, 1, 1);
        assert_eq!(result, Some(b"\x1b[<0;1;1m".to_vec()));
    }

    #[test]
    fn encode_sgr_scroll_up_at_3_4() {
        let ev = mouse_ev(MouseEventKind::ScrollUp);
        let result = encode_sgr(&ev, 3, 4);
        assert_eq!(result, Some(b"\x1b[<64;3;4M".to_vec()));
    }

    #[test]
    fn encode_sgr_scroll_down_at_3_4() {
        let ev = mouse_ev(MouseEventKind::ScrollDown);
        let result = encode_sgr(&ev, 3, 4);
        assert_eq!(result, Some(b"\x1b[<65;3;4M".to_vec()));
    }

    #[test]
    fn encode_sgr_middle_press() {
        let ev = mouse_ev(MouseEventKind::Down(MouseButton::Middle));
        let result = encode_sgr(&ev, 5, 10);
        assert_eq!(result, Some(b"\x1b[<1;5;10M".to_vec()));
    }

    #[test]
    fn encode_sgr_right_press() {
        let ev = mouse_ev(MouseEventKind::Down(MouseButton::Right));
        let result = encode_sgr(&ev, 2, 3);
        assert_eq!(result, Some(b"\x1b[<2;2;3M".to_vec()));
    }

    #[test]
    fn encode_sgr_moved_returns_none() {
        let ev = mouse_ev(MouseEventKind::Moved);
        assert_eq!(encode_sgr(&ev, 1, 1), None);
    }

    #[test]
    fn encode_sgr_scroll_left_returns_none() {
        let ev = mouse_ev(MouseEventKind::ScrollLeft);
        assert_eq!(encode_sgr(&ev, 1, 1), None);
    }

    #[test]
    fn encode_sgr_drag_left() {
        let ev = mouse_ev(MouseEventKind::Drag(MouseButton::Left));
        let result = encode_sgr(&ev, 4, 2);
        assert_eq!(result, Some(b"\x1b[<0;4;2M".to_vec()));
    }
}
