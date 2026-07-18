use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_term::widget::PseudoTerminal;
use crate::app::App;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(0)])
        .split(f.area());

    let hint = if app.leader { "SESSIONS  [C-a]" } else { "SESSIONS" };
    let sidebar = Block::default().borders(Borders::ALL).title(hint);
    let label = app.session.as_ref()
        .and_then(|(id, _)| app.manager.get(id))
        .map(|s| format!("▸ {}  [{:?}]", s.label, s.state))
        .unwrap_or_else(|| "no sessions".to_string());
    f.render_widget(Paragraph::new(label).block(sidebar), chunks[0]);

    let main = Block::default().borders(Borders::ALL).title("claude-deck");
    if let Some((_, pty)) = &app.session {
        let parser = pty.parser.lock().unwrap();
        f.render_widget(PseudoTerminal::new(parser.screen()).block(main), chunks[1]);
    } else {
        f.render_widget(Paragraph::new("(no session — C-a q to quit)").block(main), chunks[1]);
    }
}
