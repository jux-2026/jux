use self::components::conversation::render_conversation_panel;
use self::components::divider;
use self::components::sessions;
use self::components::sidebar::{audit_panel, help_panel, log_panel, run_panel, skill_panel};
use self::layout::WorkspaceLayout;
use super::{AppState, FocusedPanel};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

mod components;
mod layout;
mod text;
mod theme;

pub fn render_app(frame: &mut Frame<'_>, state: &AppState) {
    render_workspace(frame, state, frame.area());
}

pub(crate) use self::components::conversation::conversation_max_scroll;

fn render_workspace(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    if area.width < 20 || area.height < 6 {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new("Terminal too small\nResize to at least 40x10")
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().borders(Borders::ALL).title("Jux"))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let layout = WorkspaceLayout::calculate(state, area);
    let conversation_focused = state.focused_panel() == FocusedPanel::Conversation;
    if state.session_panel_visible() {
        sessions::render(frame, state, layout.conversation);
    } else {
        render_conversation_panel(frame, state, layout.conversation, conversation_focused);
    }
    if let Some(divider) = layout.divider {
        divider::render(frame, divider, state.sidebar_visible());
    }
    let Some(sidebar_area) = layout.sidebar else {
        return;
    };
    if state.help_visible() {
        frame.render_widget(help_panel(state), sidebar_area);
    } else if state.log_panel_visible() {
        frame.render_widget(log_panel(state), sidebar_area);
    } else if state.skill_panel_visible() {
        frame.render_widget(skill_panel(state), sidebar_area);
    } else if state.audit_panel_visible() {
        frame.render_widget(audit_panel(state), sidebar_area);
    } else {
        frame.render_widget(run_panel(state), sidebar_area);
    }
}
