use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Padding};

pub(super) const CONVERSATION_BACKGROUND: Color = Color::Rgb(12, 18, 24);
pub(super) const SIDEBAR_BACKGROUND: Color = Color::Rgb(18, 24, 32);
pub(super) const DIVIDER_BACKGROUND: Color = Color::Rgb(24, 32, 42);
pub(super) const COMMAND_POPUP_BACKGROUND: Color = Color::Rgb(28, 36, 46);
pub(super) const USER_MESSAGE_BACKGROUND: Color = Color::Rgb(24, 34, 44);
pub(super) const STATUS_BAR_BACKGROUND: Color = Color::Rgb(18, 28, 36);
pub(super) const CONVERSATION_PADDING: u16 = 1;

pub(super) fn input_line_style() -> Style {
    Style::default().bg(Color::Rgb(20, 38, 48))
}

pub(super) fn input_inactive_style() -> Style {
    Style::default().bg(Color::Rgb(16, 28, 36))
}

pub(super) fn panel_block(background: Color, padding: u16) -> Block<'static> {
    Block::default()
        .style(Style::default().bg(background))
        .padding(Padding::uniform(padding))
}

pub(super) fn selection_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Yellow)
}
