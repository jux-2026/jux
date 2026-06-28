use super::AppState;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

pub fn render_app(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(5)])
        .split(area);

    render_workspace(frame, state, chunks[0]);
    frame.render_widget(status_bar(), chunks[1]);
}

fn render_workspace(frame: &mut Frame<'_>, state: &AppState, area: ratatui::layout::Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(67), Constraint::Percentage(33)])
        .split(area);

    frame.render_widget(prompt_panel(), chunks[0]);
    frame.render_widget(run_panel(state), chunks[1]);
}

fn prompt_panel() -> Paragraph<'static> {
    let lines = vec![
        Line::from("What should Jux work on?"),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "Start typing in the next step",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(""),
        Line::from("No active run."),
        Line::from("Use the CLI subcommands for run, skills, and session inspection."),
    ];
    Paragraph::new(lines)
        .block(shell_block("conversation"))
        .wrap(Wrap { trim: true })
}

fn run_panel(state: &AppState) -> Paragraph<'_> {
    let lines = vec![
        Line::from("Jux"),
        Line::from(""),
        Line::from("Status: Idle"),
        Line::from(""),
        Line::from(format!("Workspace: {}", state.workspace_root.display())),
        Line::from("Mode: TUI shell"),
        Line::from("Tools: exec, lua, human_input"),
        Line::from("Skills: call_skill"),
    ];
    Paragraph::new(lines)
        .block(shell_block("status"))
        .wrap(Wrap { trim: true })
}

fn status_bar() -> Paragraph<'static> {
    Paragraph::new("Enter submit | /help commands | q quit | Ctrl+C quit")
        .style(Style::default().fg(Color::DarkGray))
        .block(shell_block("keys"))
        .alignment(Alignment::Center)
}

fn shell_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" {title}"),
            Style::default().fg(Color::Gray),
        ))
        .padding(ratatui::widgets::Padding::uniform(1))
}
