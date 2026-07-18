use super::super::theme::palette;
use jux_core::TuiTheme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

pub(in crate::tui::ui) fn render(
    buffer: &mut Buffer,
    area: Rect,
    sidebar_visible: bool,
    theme: TuiTheme,
) {
    let arrow_row = area.height / 2;
    let lines = (0..area.height)
        .map(|row| {
            if row == arrow_row {
                let arrow = if sidebar_visible { "▶" } else { "◀" };
                Line::styled(arrow, Style::default().fg(Color::Cyan))
            } else {
                Line::styled("│", Style::default().fg(Color::DarkGray))
            }
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines)
        .style(Style::default().bg(palette(theme).divider))
        .render(area, buffer);
}
