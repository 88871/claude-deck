use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use tui_term::widget::PseudoTerminal;
use crate::app::{App, Focus, Prompt, resume_matches, SETTINGS};
use crate::home;
use crate::icons;
use crate::mem;
use crate::mouse::SIDEBAR_WIDTH;
use crate::resume;
use crate::session::SessionState;

/// Truncate `s` to at most `max_chars` Unicode scalar values. If the string
/// is longer, the last two characters of the output are replaced with "…"
/// (a single Unicode ellipsis, U+2026) so the total visible length is
/// `max_chars`.
pub fn truncate_msg(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let take = max_chars.saturating_sub(1);
        let mut out: String = chars[..take].iter().collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_msg;

    #[test]
    fn no_truncation_when_short_enough() {
        assert_eq!(truncate_msg("hello", 10), "hello");
        assert_eq!(truncate_msg("hello", 5), "hello");
    }

    #[test]
    fn truncates_and_appends_ellipsis() {
        // "hello world" is 11 chars; max 8 → first 7 + ellipsis
        assert_eq!(truncate_msg("hello world", 8), "hello w…");
    }

    #[test]
    fn truncation_respects_unicode_scalar_values() {
        // Each emoji is 1 scalar value for our purposes
        let s = "αβγδεζηθ"; // 8 Greek letters
        assert_eq!(truncate_msg(s, 6), "αβγδε…");
    }

    #[test]
    fn max_chars_one_returns_ellipsis_or_char() {
        assert_eq!(truncate_msg("ab", 1), "…");
        assert_eq!(truncate_msg("a", 1), "a");
    }

    #[test]
    fn empty_string_unchanged() {
        assert_eq!(truncate_msg("", 5), "");
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    // ── Outer horizontal split: sidebar | main ──────────────────────────────
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)])
        .split(f.area());

    // ── Sidebar ─────────────────────────────────────────────────────────────
    draw_sidebar(f, app, chunks[0]);

    // ── Main pane: split vertically for optional input line ──────────────────
    // Choose the main block title: show a scrollback indicator when the focused
    // session is scrolled back from the live view.
    let scrollback_active = if let Focus::Session(i) = app.focus {
        app.sessions.get(i)
            .and_then(|(_, pty_opt)| pty_opt.as_ref())
            .map(|pty| pty.scroll > 0)
            .unwrap_or(false)
    } else {
        false
    };
    let main_title = if scrollback_active { "claude-deck  -- SCROLLBACK --" } else { "claude-deck" };
    let main_block = Block::default().borders(Borders::ALL).title(main_title);

    if let Some(Prompt::ConfirmRestart(ref id)) = app.prompt {
        // Confirmation prompt: render the current session behind it, with a
        // single-line confirmation message at the bottom.
        let main_area = chunks[1];
        let inner = main_block.inner(main_area);
        f.render_widget(main_block, main_area);

        // Find a human-readable label for the session being restarted.
        let label = app.manager.get(id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| id[..8.min(id.len())].to_string());

        let confirm_msg = format!(
            "restart {}? in-flight work will be lost — press y to confirm, any other key cancels",
            label
        );

        if inner.height > 1 {
            let vsplit = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(inner);

            // Show the live session behind the prompt.
            if let Focus::Session(i) = app.focus {
                if let Some((_, Some(pty))) = app.sessions.get(i) {
                    let parser = pty.parser.lock().unwrap();
                    f.render_widget(PseudoTerminal::new(parser.screen()), vsplit[0]);
                }
            }

            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(confirm_msg, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                ])),
                vsplit[1],
            );
        } else {
            f.render_widget(Paragraph::new(confirm_msg), inner);
        }
    } else if matches!(&app.prompt, Some(Prompt::EditSetting { .. })) {
        // EditSetting: render the settings view with an inline edit line at the bottom.
        // We draw inside the main block directly so the settings pane is behind the prompt.
        let main_area = chunks[1];
        f.render_widget(&main_block, main_area);
        let inner = main_block.inner(main_area);
        draw_settings_view_with_edit(f, inner, app);
    } else if app.prompt.is_some() {
        let main_area = chunks[1];
        let inner = main_block.inner(main_area);
        f.render_widget(main_block, main_area);

        // Determine label prefix and buffer content from the active prompt variant.
        let (prefix, buf) = match &app.prompt {
            Some(Prompt::NewSession(s)) => ("new session path: ", s.as_str()),
            Some(Prompt::Rename(s))     => ("rename session: ",   s.as_str()),
            Some(Prompt::ConfirmRestart(_)) | Some(Prompt::EditSetting { .. }) | None => ("", ""),
        };
        let is_new_session = matches!(&app.prompt, Some(Prompt::NewSession(_)));

        if inner.height > 1 {
            let vsplit = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(inner);

            // Show current pane (Home or session) behind the input prompt.
            match app.focus {
                Focus::Home => {
                    home::render(f, vsplit[0], app.sessions.len(), app.icons);
                }
                Focus::Settings => {
                    draw_settings_view(f, vsplit[0], app);
                }
                Focus::ResumePicker => {
                    draw_resume_picker(f, vsplit[0], app);
                }
                Focus::Session(i) => {
                    match app.sessions.get(i) {
                        Some((_, Some(pty))) => {
                            let parser = pty.parser.lock().unwrap();
                            f.render_widget(PseudoTerminal::new(parser.screen()), vsplit[0]);
                        }
                        _ => {
                            f.render_widget(
                                Paragraph::new("parked to save memory — resuming…")
                                    .alignment(Alignment::Center),
                                vsplit[0],
                            );
                        }
                    }
                }
            }

            // Input line.
            let mut spans = vec![
                Span::raw(prefix),
                Span::raw(buf),
                Span::raw("_"),
            ];
            if is_new_session {
                spans.push(Span::styled(
                    "  Enter start · Tab complete · Ctrl-r resume here",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            let prompt_text = Text::from(Line::from(spans));
            f.render_widget(Paragraph::new(prompt_text), vsplit[1]);
        } else {
            // Terminal too small; just show the input line.
            f.render_widget(
                Paragraph::new(format!("{}{}_", prefix, buf)),
                inner,
            );
        }
    } else {
        // Normal: render Home, Settings, Resume Picker, or the focused session.
        match app.focus {
            Focus::Home => {
                home::render(f, chunks[1], app.sessions.len(), app.icons);
            }
            Focus::Settings => {
                draw_settings_view(f, chunks[1], app);
            }
            Focus::ResumePicker => {
                draw_resume_picker(f, chunks[1], app);
            }
            Focus::Session(i) => {
                match app.sessions.get(i) {
                    Some((_, Some(pty))) => {
                        let parser = pty.parser.lock().unwrap();
                        f.render_widget(
                            PseudoTerminal::new(parser.screen()).block(main_block),
                            chunks[1],
                        );
                    }
                    Some((_, None)) => {
                        // Parked session: show a transient placeholder while resuming.
                        f.render_widget(
                            Paragraph::new("parked to save memory — resuming…")
                                .alignment(Alignment::Center)
                                .block(main_block),
                            chunks[1],
                        );
                    }
                    None => {
                        f.render_widget(
                            Paragraph::new("(no session — C-a n to start one)").block(main_block),
                            chunks[1],
                        );
                    }
                }
            }
        }
    }
}

// ── Sidebar renderer ──────────────────────────────────────────────────────────

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let hint = if app.leader { "SESSIONS  [C-a]" } else { "SESSIONS" };
    let sidebar_block = Block::default().borders(Borders::ALL).title(hint);
    let inner = sidebar_block.inner(area);

    // Render the outer block (borders + title) first.
    f.render_widget(sidebar_block, area);

    // The Settings row is pinned at the last interior row.
    // Split inner into [session list area | settings row].
    let settings_row_h = 1u16;
    let list_h = inner.height.saturating_sub(settings_row_h);

    let vsplit = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(list_h), Constraint::Length(settings_row_h)])
        .split(inner);

    let list_area = vsplit[0];
    let settings_row_area = vsplit[1];

    // ── Session list (Home + sessions) ────────────────────────────────────────
    let mut items: Vec<ListItem> = Vec::new();

    // Home row (shown when home_visible).
    if app.home_visible {
        let row_style = if app.focus == Focus::Home {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let label = format!("{} Home", icons::home(app.icons));
        items.push(ListItem::new(Line::from(vec![Span::styled(label, row_style)])));
    }

    // Session rows.
    for (i, (id, pty_opt)) in app.sessions.iter().enumerate() {
        let session_info = app.manager.get(id);
        let (label, state, pinned) = session_info
            .map(|s| (s.label.clone(), s.state, s.pinned))
            .unwrap_or_else(|| (id[..8.min(id.len())].to_string(), SessionState::Error, false));

        let g = icons::state(state, app.icons);
        let glyph_color: Color = icons::state_color(state);

        let focused = app.focus == Focus::Session(i);
        let glyph_style = if focused {
            Style::default().fg(glyph_color).add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(glyph_color)
        };
        let label_style = if focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };

        // Prepend a pin marker for pinned sessions.
        let pin_prefix = if pinned { "* " } else { "" };

        // Build spans for this row.
        let mut spans = vec![
            Span::styled(g, glyph_style),
            Span::styled(format!(" {}{}", pin_prefix, label), label_style),
        ];

        // For live sessions: show RSS + optional warning marker.
        if pty_opt.is_some() {
            if let Some(&rss_kb) = app.rss.get(id) {
                let mem_str = mem::fmt_kb(rss_kb);
                let over_warn = app.mem_warn_kb.map(|warn| rss_kb >= warn).unwrap_or(false);
                let mem_style = if over_warn {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let warn_marker = if over_warn { "! " } else { "" };
                spans.push(Span::styled(
                    format!(" {}{}", warn_marker, mem_str),
                    mem_style,
                ));
            }
        }

        // Feature B: for WaitingOnYou sessions with a pending message, show a
        // dim sub-line so the user can see what the session wants without switching.
        if state == SessionState::WaitingOnYou {
            if let Some(msg) = app.pending_msg.get(id) {
                // Reserve 2 chars for the indent ("  "), leaving 22 chars for content.
                let truncated = truncate_msg(msg, 22);
                let msg_line = Line::from(vec![
                    Span::styled(
                        format!("  {}", truncated),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);
                let item = ListItem::new(Text::from(vec![Line::from(spans), msg_line]));
                items.push(item);
                continue;
            }
        }

        items.push(ListItem::new(Line::from(spans)));
    }

    if items.is_empty() {
        f.render_widget(
            Paragraph::new("no sessions"),
            list_area,
        );
    } else {
        f.render_widget(List::new(items), list_area);
    }

    // ── Settings row pinned at the bottom ────────────────────────────────────
    let settings_style = if app.focus == Focus::Settings {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    let gear = icons::settings(app.icons);
    // Show a subtle "mouse: off" indicator when mouse capture is disabled.
    let mouse_indicator = if !app.mouse_on { "  mouse: off" } else { "" };
    let settings_label = format!("{} Settings{}", gear, mouse_indicator);
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(settings_label, settings_style)])),
        settings_row_area,
    );
}

// ── Settings view renderer ────────────────────────────────────────────────────

/// The ordered list of setting rows: (label, value).
/// Labels come from `SETTINGS` (the single source of truth).
/// Order must match `App::toggle_settings_bool_at_cursor`.
fn settings_rows(app: &App) -> Vec<(&'static str, String)> {
    let cfg = &app.config;
    // Values in the same order as SETTINGS:
    // 0 dnd, 1 bell, 2 desktop_notify, 3 ntfy_topic, 4 reap_idle,
    // 5 reap_timeout_secs, 6 mem_warn_mb, 7 nerd_icons, 8 mouse
    let values: Vec<String> = vec![
        bool_val(cfg.dnd),
        bool_val(cfg.bell),
        bool_val(cfg.desktop_notify),
        cfg.ntfy_topic.clone().unwrap_or_else(|| "(none)".to_string()),
        bool_val(cfg.reap_idle),
        format!("{} s", cfg.reap_timeout_secs),
        if cfg.mem_warn_mb == 0 { "off".to_string() } else { format!("{} MB", cfg.mem_warn_mb) },
        bool_val(cfg.nerd_icons),
        bool_val(cfg.mouse),
    ];
    SETTINGS.iter().zip(values.into_iter()).map(|(meta, val)| (meta.label, val)).collect()
}

fn bool_val(b: bool) -> String {
    if b { "on".to_string() } else { "off".to_string() }
}

fn draw_settings_view(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Settings");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    // Header line.
    let header = Line::from(vec![Span::styled(
        "↑/↓ move · Enter toggle/edit · Space toggle · Esc back  (see hint below)",
        Style::default().fg(Color::DarkGray),
    )]);

    let rows = settings_rows(app);

    // Help line: description of the currently highlighted setting.
    let help_text = SETTINGS
        .get(app.settings_cursor)
        .map(|m| m.description)
        .unwrap_or("");
    let help_line = Line::from(vec![Span::styled(
        help_text,
        Style::default().fg(Color::Cyan),
    )]);

    // Layout: header (1) | list (remaining) | help line (1).
    // Need at least 3 rows for header + list + help.
    if inner.height < 2 {
        f.render_widget(Paragraph::new(header), inner);
        return;
    }

    let (list_constraint, show_help) = if inner.height >= 3 {
        (Constraint::Min(0), true)
    } else {
        // height == 2: header + list only, no help line
        (Constraint::Min(0), false)
    };

    if show_help {
        let vsplit = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                list_constraint,       // list
                Constraint::Length(1), // help line
            ])
            .split(inner);
        f.render_widget(Paragraph::new(header), vsplit[0]);
        render_settings_list(f, vsplit[1], app, &rows);
        f.render_widget(Paragraph::new(help_line), vsplit[2]);
    } else {
        let vsplit = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), list_constraint])
            .split(inner);
        f.render_widget(Paragraph::new(header), vsplit[0]);
        render_settings_list(f, vsplit[1], app, &rows);
    }
}

/// Render the settings view with an active EditSetting prompt at the bottom.
/// Called when `app.prompt` is `Some(Prompt::EditSetting { .. })`.
fn draw_settings_view_with_edit(f: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }

    let (setting_key, buf) = match &app.prompt {
        Some(Prompt::EditSetting { key, buf }) => (*key, buf.as_str()),
        _ => return,
    };

    // Header line.
    let header = Line::from(vec![Span::styled(
        "↑/↓ move · Enter toggle/edit · Space toggle · Esc back  (see hint below)",
        Style::default().fg(Color::DarkGray),
    )]);

    let rows = settings_rows(app);

    // Help line: description of the currently highlighted setting.
    let help_text = SETTINGS
        .get(app.settings_cursor)
        .map(|m| m.description)
        .unwrap_or("");

    // Layout: header | list | help line | edit line
    // Need at least 3 rows: we always show at least header + edit line.
    if area.height < 3 {
        // Too small: just show the edit line.
        let edit_text = format!("edit {}: {}_", setting_key.label(), buf);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(edit_text, Style::default().fg(Color::Yellow)),
            ])),
            area,
        );
        return;
    }

    // With 4+ rows: header | list | help | edit.
    // With exactly 3 rows: header | list | edit (skip help).
    let (constraints, has_help) = if area.height >= 4 {
        (
            vec![
                Constraint::Length(1), // header
                Constraint::Min(1),    // list
                Constraint::Length(1), // help line
                Constraint::Length(1), // edit line
            ],
            true,
        )
    } else {
        (
            vec![
                Constraint::Length(1), // header
                Constraint::Min(1),    // list
                Constraint::Length(1), // edit line
            ],
            false,
        )
    };

    let vsplit = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    f.render_widget(Paragraph::new(header), vsplit[0]);
    render_settings_list(f, vsplit[1], app, &rows);

    if has_help {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(help_text, Style::default().fg(Color::Cyan)),
            ])),
            vsplit[2],
        );
        // Edit line at index 3.
        let edit_text = format!("edit {}: {}_", setting_key.label(), buf);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(edit_text, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ])),
            vsplit[3],
        );
    } else {
        // Edit line at index 2.
        let edit_text = format!("edit {}: {}_", setting_key.label(), buf);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(edit_text, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ])),
            vsplit[2],
        );
    }
}

// ── Resume picker renderer ────────────────────────────────────────────────────

/// Render the resume-picker view into `area`.
pub fn draw_resume_picker(f: &mut Frame, area: Rect, app: &App) {
    // Title changes when scoped to a directory.
    let title = match &app.resume_scope {
        Some(path) => format!("Resume in {}", path.display()),
        None => "Resume a past conversation".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.as_str());
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let header_text = "type to filter  ·  ↑/↓  ·  Enter open  ·  Esc cancel";
    let filter_text = format!("filter: {}_", app.resume_filter);

    let filter = app.resume_filter.to_lowercase();
    let filtered: Vec<&resume::Past> = app
        .resume_items
        .iter()
        .filter(|p| resume_matches(p, &filter))
        .collect();

    let now = std::time::SystemTime::now();

    // Layout: header (1) | filter line (1) | list (remaining).
    if inner.height < 3 {
        f.render_widget(Paragraph::new(filter_text), inner);
        return;
    }

    let vsplit = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header hint
            Constraint::Length(1), // filter line
            Constraint::Min(0),    // list
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            header_text,
            Style::default().fg(Color::DarkGray),
        )])),
        vsplit[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            filter_text,
            Style::default().fg(Color::Yellow),
        )])),
        vsplit[1],
    );

    // List area.
    let list_area = vsplit[2];

    if filtered.is_empty() {
        // Show a context-aware empty message.
        let empty_msg = match &app.resume_scope {
            Some(path) if app.resume_items.is_empty() => {
                format!("no past conversations in {}", path.display())
            }
            _ => "(no matches)".to_string(),
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                empty_msg,
                Style::default().fg(Color::DarkGray),
            )])),
            list_area,
        );
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, past)| {
            let is_cursor = i == app.resume_cursor;

            // Main label: title.
            let cwd_comp = past
                .cwd
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            let time_str = resume::rel_time(past.mtime, now);
            let sub = format!("  {} · {}", cwd_comp, time_str);

            let title_style = if is_cursor {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let sub_style = if is_cursor {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            ListItem::new(Text::from(vec![
                Line::from(vec![Span::styled(past.title.clone(), title_style)]),
                Line::from(vec![Span::styled(sub, sub_style)]),
            ]))
        })
        .collect();

    f.render_widget(List::new(items), list_area);
}

/// Shared helper: render the settings list rows into `area`.
fn render_settings_list(f: &mut Frame, area: Rect, app: &App, rows: &[(&'static str, String)]) {
    let label_col_width = area.width.saturating_sub(12).max(8) as usize;
    let mut items: Vec<ListItem> = Vec::new();
    for (i, (label, value)) in rows.iter().enumerate() {
        let is_cursor = i == app.settings_cursor;
        // Pad label to label_col_width, then value right-aligned.
        let dots = ".".repeat(
            label_col_width
                .saturating_sub(label.len())
                .saturating_sub(value.len())
                .saturating_sub(2)
                .max(1)
        );
        let row_text = format!("{} {}{} {}", label, dots, "", value);
        let style = if is_cursor {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        items.push(ListItem::new(Line::from(vec![Span::styled(row_text, style)])));
    }
    f.render_widget(List::new(items), area);
}

