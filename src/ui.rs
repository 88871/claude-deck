use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use tui_term::widget::PseudoTerminal;
use crate::app::{App, Focus, Prompt};
use crate::home;
use crate::icons;
use crate::mem;
use crate::mouse::SIDEBAR_WIDTH;
use crate::session::SessionState;

pub fn draw(f: &mut Frame, app: &App) {
    // ── Outer horizontal split: sidebar | main ──────────────────────────────
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)])
        .split(f.area());

    // ── Sidebar ─────────────────────────────────────────────────────────────
    let hint = if app.leader { "SESSIONS  [C-a]" } else { "SESSIONS" };
    let sidebar_block = Block::default().borders(Borders::ALL).title(hint);

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

        items.push(ListItem::new(Line::from(spans)));
    }

    if items.is_empty() {
        f.render_widget(
            Paragraph::new("no sessions").block(sidebar_block),
            chunks[0],
        );
    } else {
        f.render_widget(List::new(items).block(sidebar_block), chunks[0]);
    }

    // ── Main pane: split vertically for optional input line ──────────────────
    let main_block = Block::default().borders(Borders::ALL).title("claude-deck");

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
    } else if app.prompt.is_some() {
        let main_area = chunks[1];
        let inner = main_block.inner(main_area);
        f.render_widget(main_block, main_area);

        // Determine label prefix and buffer content from the active prompt variant.
        let (prefix, buf) = match &app.prompt {
            Some(Prompt::NewSession(s)) => ("new session path: ", s.as_str()),
            Some(Prompt::Rename(s))     => ("rename session: ",   s.as_str()),
            Some(Prompt::ConfirmRestart(_)) | None => ("", ""),
        };

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
            let prompt_text = Text::from(Line::from(vec![
                Span::raw(prefix),
                Span::raw(buf),
                Span::raw("_"),
            ]));
            f.render_widget(Paragraph::new(prompt_text), vsplit[1]);
        } else {
            // Terminal too small; just show the input line.
            f.render_widget(
                Paragraph::new(format!("{}{}_", prefix, buf)),
                inner,
            );
        }
    } else {
        // Normal: render Home or the focused session.
        match app.focus {
            Focus::Home => {
                home::render(f, chunks[1], app.sessions.len(), app.icons);
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
