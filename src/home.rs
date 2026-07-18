use ratatui::{
    layout::{Alignment, Rect},
    style::{Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render(f: &mut Frame, area: Rect, session_count: usize) {
    let status = if session_count == 0 {
        "no sessions yet — start one above".to_string()
    } else {
        format!("{} active session{}", session_count, if session_count == 1 { "" } else { "s" })
    };
    let lines = vec![
        Line::from(""),
        Line::from("claude-deck").bold().centered(),
        Line::from(""),
        Line::from("Welcome back 👋").centered(),
        Line::from("What do you want to work on today?").centered(),
        Line::from(""),
        Line::from("▸  Ctrl-a  n     new session (pick a folder)").centered(),
        Line::from("▸  Ctrl-a  h     back to this Home screen").centered(),
        Line::from(""),
        Line::from("switch  Ctrl-a 1-9 / [ / ]    kill  Ctrl-a x    quit  Ctrl-a q")
            .style(Style::new().dim())
            .centered(),
        Line::from(""),
        Line::from(status).style(Style::new().dim()).centered(),
    ];
    let block = Block::default().borders(Borders::ALL).title("claude-deck");
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .block(block),
        area,
    );
}
