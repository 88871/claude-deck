use ratatui::{
    layout::{Alignment, Rect},
    style::{Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use crate::icons::{self, IconMode};

pub fn render(f: &mut Frame, area: Rect, session_count: usize, icon_mode: IconMode) {
    let status = if session_count == 0 {
        "no sessions yet — start one above".to_string()
    } else {
        format!("{} active session{}", session_count, if session_count == 1 { "" } else { "s" })
    };
    let folder = icons::folder(icon_mode);
    let dim = Style::new().dim();
    let lines = vec![
        Line::from(""),
        Line::from("claude-deck").bold().centered(),
        Line::from(""),
        Line::from("Welcome back 👋").centered(),
        Line::from("What do you want to work on today?").centered(),
        Line::from(""),
        Line::from(format!("{}  Ctrl-a  n   new session      Ctrl-a  o   resume a past one", folder))
            .centered(),
        Line::from(""),
        Line::from("─────────  keys  (leader: Ctrl-a, then…)  ─────────").style(dim).centered(),
        Line::from("n  new session      o  resume past      s  Settings      h  Home").style(dim).centered(),
        Line::from("1-9 / [ ] / ↑ ↓  switch sessions        !  jump to one waiting on you").style(dim).centered(),
        Line::from("r  rename      R  restart      p  pin      x  kill      q  quit").style(dim).centered(),
        Line::from("m  toggle mouse (turn off for native copy/paste)").style(dim).centered(),
        Line::from("in new-session:  Tab  complete path   ·   Ctrl-r  resume here").style(dim).centered(),
        Line::from(""),
        Line::from(status).style(dim).centered(),
    ];
    let block = Block::default().borders(Borders::ALL).title("claude-deck");
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .block(block),
        area,
    );
}
