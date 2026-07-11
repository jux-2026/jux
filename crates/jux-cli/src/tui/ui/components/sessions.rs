use super::super::theme::CONVERSATION_BACKGROUND;
use crate::tui::AppState;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

const SELECTED_BACKGROUND: Color = Color::Rgb(40, 52, 64);

pub(in crate::tui::ui) fn render(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let (label, value) = state
        .session_rename()
        .map_or((" Search: ", state.session_search()), |rename| {
            (" Rename: ", rename)
        });
    let search = Paragraph::new(Line::from(vec![
        Span::styled(label, Style::default().fg(Color::DarkGray)),
        Span::raw(value),
    ]))
    .style(Style::default().bg(CONVERSATION_BACKGROUND))
    .block(Block::default().padding(Padding::vertical(1)));
    frame.render_widget(search, regions[0]);

    let width = regions[1].width.saturating_sub(1) as usize;
    let lines = state
        .filtered_sessions()
        .into_iter()
        .enumerate()
        .map(|(index, session)| {
            let title = session.name.as_deref().unwrap_or("(unnamed)");
            let title = if session.liked {
                format!("♥ {title}")
            } else {
                format!("  {title}")
            };
            let style = if index == state.selected_session() {
                Style::default().bg(SELECTED_BACKGROUND)
            } else {
                Style::default()
            };
            Line::styled(format!(" {title:<width$}"), style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(CONVERSATION_BACKGROUND)),
        regions[1],
    );
    frame.render_widget(
        Paragraph::new(" ↑/↓ select  Enter switch  Ctrl+L like  Ctrl+R rename  Esc close").style(
            Style::default()
                .fg(Color::DarkGray)
                .bg(CONVERSATION_BACKGROUND),
        ),
        regions[2],
    );

    let query_width = value.chars().count() as u16;
    let cursor_x = regions[0]
        .x
        .saturating_add(9)
        .saturating_add(query_width)
        .min(regions[0].right().saturating_sub(1));
    frame.set_cursor_position(Position::new(cursor_x, regions[0].y.saturating_add(1)));
}
