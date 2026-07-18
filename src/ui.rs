use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use tui_term::widget::PseudoTerminal;
use crate::app::App;
use crate::session::SessionState;

fn glyph(state: SessionState) -> &'static str {
    use SessionState::*;
    match state {
        Starting    => "○",
        Running     => "⏳",
        WaitingOnYou => "◍",
        Idle        => "✓",
        Parked      => "◌",
        Closed      => "⏹",
        Error       => "✗",
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    // ── Outer horizontal split: sidebar | main ──────────────────────────────
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(0)])
        .split(f.area());

    // ── Sidebar ─────────────────────────────────────────────────────────────
    let hint = if app.leader { "SESSIONS  [C-a]" } else { "SESSIONS" };
    let sidebar_block = Block::default().borders(Borders::ALL).title(hint);

    let items: Vec<ListItem> = app.sessions.iter().enumerate().map(|(i, (id, _))| {
        let session_info = app.manager.get(id);
        let (label, state) = session_info
            .map(|s| (s.label.clone(), s.state))
            .unwrap_or_else(|| (id[..8.min(id.len())].to_string(), SessionState::Error));

        let g = glyph(state);
        let text = format!("{} {}", g, label);
        let style = if i == app.focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        ListItem::new(Line::from(vec![Span::styled(text, style)]))
    }).collect();

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

    if app.input.is_some() {
        // Reserve one row at the bottom for the input prompt.
        let main_area = chunks[1];
        // We need to carve the input line from the main area manually.
        // The block uses the full chunks[1]; inside we split rows.
        let inner = main_block.inner(main_area);
        f.render_widget(main_block, main_area);

        if inner.height > 1 {
            let vsplit = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(inner);

            // PTY pane (unfocused / whatever session is active).
            if let Some((_, pty)) = app.sessions.get(app.focused) {
                let parser = pty.parser.lock().unwrap();
                f.render_widget(PseudoTerminal::new(parser.screen()), vsplit[0]);
            } else {
                f.render_widget(
                    Paragraph::new("(no session)"),
                    vsplit[0],
                );
            }

            // Input line.
            let buf = app.input.as_deref().unwrap_or("");
            let prompt_text = Text::from(Line::from(vec![
                Span::raw("new session path: "),
                Span::raw(buf),
                Span::raw("_"),
            ]));
            f.render_widget(Paragraph::new(prompt_text), vsplit[1]);
        } else {
            // Terminal too small; just show the input line.
            let buf = app.input.as_deref().unwrap_or("");
            f.render_widget(
                Paragraph::new(format!("new session path: {}_", buf)),
                inner,
            );
        }
    } else {
        // Normal: render the focused session's screen.
        if let Some((_, pty)) = app.sessions.get(app.focused) {
            let parser = pty.parser.lock().unwrap();
            f.render_widget(PseudoTerminal::new(parser.screen()).block(main_block), chunks[1]);
        } else {
            f.render_widget(
                Paragraph::new("(no session — C-a q to quit)").block(main_block),
                chunks[1],
            );
        }
    }
}
