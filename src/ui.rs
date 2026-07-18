use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use crate::app::App;

pub fn draw(f: &mut Frame, _app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(0)])
        .split(f.area());

    let sidebar = Block::default().borders(Borders::ALL).title("SESSIONS");
    f.render_widget(Paragraph::new("no sessions\n\nq: quit").block(sidebar), chunks[0]);

    let main = Block::default().borders(Borders::ALL).title("claude-deck");
    f.render_widget(Paragraph::new("(no session selected)").block(main), chunks[1]);
}
