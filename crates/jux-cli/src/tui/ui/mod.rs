use self::components::conversation::render_conversation_panel;
use self::components::divider;
use self::components::sessions;
use self::components::sidebar::{audit_panel, help_panel, log_panel, run_panel, skill_panel};
use self::layout::WorkspaceLayout;
use super::{AppState, FocusedPanel};
use jux_core::HumanInputKind;
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

pub(crate) use self::components::conversation::{command_toggle_at, conversation_max_scroll};

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
        divider::render(frame, divider, state.sidebar_visible(), state.theme());
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
    render_confirmation_overlay(frame, state, area);
}

fn render_confirmation_overlay(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    let (title, body) = if let Some(request) = state
        .pending_human_input()
        .filter(|request| request.kind == HumanInputKind::Confirmation)
    {
        (
            "Permission confirmation",
            format!(
                "{}\n\n{}\n\nUse Up/Down and Enter to confirm or reject.",
                request.prompt,
                request.reason.as_deref().unwrap_or("")
            ),
        )
    } else if let Some(review) = state.code_change_review() {
        (
            "File change confirmation",
            format!(
                "{}\n{} file(s), policy {:?}\n\n/review accept | /review reject | /review changes <feedback>",
                review.proposal.plan.summary,
                review.proposal.files.len(),
                review.proposal.policy
            ),
        )
    } else {
        return;
    };
    let width = area.width.saturating_sub(8).min(72);
    let height = area.height.saturating_sub(4).min(11);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(body)
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: true }),
        popup,
    );
}
