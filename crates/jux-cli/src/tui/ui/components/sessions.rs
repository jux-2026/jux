use super::super::RenderState;
use super::super::theme::palette;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Widget};
use std::time::{SystemTime, UNIX_EPOCH};

const SELECTED_BACKGROUND: Color = Color::Rgb(40, 52, 64);

pub(in crate::tui::ui) fn render(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    area: Rect,
) -> Position {
    let background = palette(state.theme()).conversation;
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
    .style(Style::default().bg(background))
    .block(Block::default().padding(Padding::vertical(1)));
    search.render(regions[0], buffer);

    let width = regions[1].width.saturating_sub(1) as usize;
    let sessions = state.filtered_sessions();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let mut lines = Vec::new();
    const DAY_MILLIS: u128 = 24 * 60 * 60 * 1_000;
    for group_index in 0..3 {
        let group = sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| match group_index {
                0 => session.liked,
                1 => !session.liked && now.saturating_sub(session.updated_at) < DAY_MILLIS,
                _ => !session.liked && now.saturating_sub(session.updated_at) >= DAY_MILLIS,
            })
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        lines.push(Line::styled(
            match group_index {
                0 => " Pinned",
                1 => " Today",
                _ => " Earlier",
            },
            Style::default().fg(Color::Cyan),
        ));
        lines.extend(group.into_iter().map(|(index, session)| {
            let title = session.name.as_deref().unwrap_or("(unnamed)");
            let age = relative_time(now.saturating_sub(session.updated_at));
            let run_count = state
                .session_history(&session.id)
                .map_or(0, |history| history.runs.len());
            let title = format!("{}  {run_count} runs · {age}", title);
            let style = if index == state.selected_session() {
                Style::default().bg(SELECTED_BACKGROUND)
            } else {
                Style::default()
            };
            Line::styled(format!(" {title:<width$}"), style)
        }));
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            " No sessions match your search.",
            Style::default().fg(Color::DarkGray),
        ));
    }
    Paragraph::new(lines)
        .style(Style::default().bg(background))
        .render(regions[1], buffer);
    Paragraph::new(
        " Ctrl+N new  Ctrl+D delete  Ctrl+A archive  ↑/↓ select  Enter switch  Ctrl+L pin",
    )
    .style(Style::default().fg(Color::DarkGray).bg(background))
    .render(regions[2], buffer);

    let query_width = value.chars().count() as u16;
    let cursor_x = regions[0]
        .x
        .saturating_add(9)
        .saturating_add(query_width)
        .min(regions[0].right().saturating_sub(1));
    Position::new(cursor_x, regions[0].y.saturating_add(1))
}

fn relative_time(age_millis: u128) -> String {
    let seconds = age_millis / 1_000;
    match seconds {
        0..=9 => "just now".to_owned(),
        10..=59 => format!("{seconds}s ago"),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        86_400..=604_799 => format!("{}d ago", seconds / 86_400),
        _ => format!("{}w ago", seconds / 604_800),
    }
}
