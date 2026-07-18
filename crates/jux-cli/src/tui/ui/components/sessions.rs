use super::super::RenderState;
use super::super::theme::palette;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Widget};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TIME_COLUMN_WIDTH: usize = 9;
const RUN_COLUMN_WIDTH: usize = 10;
const COLUMN_GAP: &str = "  ";

pub(in crate::tui::ui) fn render(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    area: Rect,
) -> Position {
    let theme_palette = palette(state.theme());
    let background = theme_palette.conversation;
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

    let width = usize::from(regions[1].width);
    let sessions = state.filtered_sessions();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let mut lines = Vec::new();
    let mut accordion_index = 0;
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
        for (index, session) in group {
            let title = session.name.as_deref().unwrap_or("(unnamed)");
            let age = relative_time(now.saturating_sub(session.updated_at));
            let run_count = state
                .session_history(&session.id)
                .map_or(0, |history| history.runs.len());
            let background = if index == state.selected_session() {
                theme_palette.session_row_selected
            } else if accordion_index % 2 == 0 {
                theme_palette.session_row
            } else {
                theme_palette.session_row_alternate
            };
            lines.push(session_line(
                title,
                &age,
                run_count,
                width,
                Style::default().bg(background),
            ));
            accordion_index += 1;
        }
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

fn session_line(
    title: &str,
    age: &str,
    run_count: usize,
    width: usize,
    style: Style,
) -> Line<'static> {
    let age = fit_text_right(age, TIME_COLUMN_WIDTH);
    let run_count = fit_text_right(&format_run_count(run_count), RUN_COLUMN_WIDTH);
    let metadata = format!(" {age}{COLUMN_GAP}{run_count}{COLUMN_GAP}");
    let metadata_width = UnicodeWidthStr::width(metadata.as_str()).min(width);
    let title_width = width.saturating_sub(metadata_width);
    let title = fit_text_left(title, title_width);
    Line::styled(format!("{metadata}{title}"), style)
}

fn format_run_count(run_count: usize) -> String {
    match run_count {
        0..=9_999 => format!("{run_count} runs"),
        10_000..=999_999 => format!("{}k runs", run_count / 1_000),
        _ => format!("{}m+ runs", (run_count / 1_000_000).min(999)),
    }
}

fn fit_text_right(text: &str, width: usize) -> String {
    let (fitted, fitted_width) = fit_text(text, width);
    format!("{}{fitted}", " ".repeat(width.saturating_sub(fitted_width)))
}

fn fit_text_left(text: &str, width: usize) -> String {
    let (fitted, fitted_width) = fit_text(text, width);
    format!("{fitted}{}", " ".repeat(width.saturating_sub(fitted_width)))
}

fn fit_text(text: &str, width: usize) -> (String, usize) {
    let mut fitted = String::new();
    let mut fitted_width: usize = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or_default();
        if fitted_width.saturating_add(character_width) > width {
            break;
        }
        fitted.push(character);
        fitted_width = fitted_width.saturating_add(character_width);
    }
    (fitted, fitted_width)
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
