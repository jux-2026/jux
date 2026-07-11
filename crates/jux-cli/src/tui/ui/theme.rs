use jux_core::TuiTheme;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Padding};

pub(super) const CONVERSATION_BACKGROUND: Color = Color::Rgb(12, 18, 24);
pub(super) const SIDEBAR_BACKGROUND: Color = Color::Rgb(18, 24, 32);
pub(super) const DIVIDER_BACKGROUND: Color = Color::Rgb(24, 32, 42);
pub(super) const COMMAND_POPUP_BACKGROUND: Color = Color::Rgb(28, 36, 46);
pub(super) const USER_MESSAGE_BACKGROUND: Color = Color::Rgb(24, 34, 44);
pub(super) const STATUS_BAR_BACKGROUND: Color = Color::Rgb(18, 28, 36);
pub(super) const CONVERSATION_PADDING: u16 = 1;

#[derive(Clone, Copy)]
pub(super) struct ThemePalette {
    pub conversation: Color,
    pub sidebar: Color,
    pub divider: Color,
    pub popup: Color,
    pub user_message: Color,
    pub status: Color,
    pub input: Color,
    pub input_inactive: Color,
}

pub(super) fn palette(theme: TuiTheme) -> ThemePalette {
    match theme {
        TuiTheme::Dark => ThemePalette {
            conversation: CONVERSATION_BACKGROUND,
            sidebar: SIDEBAR_BACKGROUND,
            divider: DIVIDER_BACKGROUND,
            popup: COMMAND_POPUP_BACKGROUND,
            user_message: USER_MESSAGE_BACKGROUND,
            status: STATUS_BAR_BACKGROUND,
            input: Color::Rgb(20, 38, 48),
            input_inactive: Color::Rgb(16, 28, 36),
        },
        TuiTheme::HighContrast => ThemePalette {
            conversation: Color::Black,
            sidebar: Color::Rgb(8, 8, 8),
            divider: Color::White,
            popup: Color::Rgb(32, 32, 32),
            user_message: Color::Rgb(48, 48, 48),
            status: Color::Rgb(24, 24, 24),
            input: Color::Rgb(0, 48, 64),
            input_inactive: Color::Rgb(16, 16, 16),
        },
    }
}

pub(super) fn panel_block(background: Color, padding: u16) -> Block<'static> {
    Block::default()
        .style(Style::default().bg(background))
        .padding(Padding::uniform(padding))
}

pub(super) fn selection_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Yellow)
}
