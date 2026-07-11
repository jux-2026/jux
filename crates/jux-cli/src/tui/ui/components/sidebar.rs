use super::super::text::{apply_text_selection, display_names};
use super::super::theme::{SIDEBAR_BACKGROUND, panel_block};
use crate::tui::{AppState, SelectionPanel, TuiRunStatus};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

pub(in crate::tui::ui) fn log_panel(state: &AppState) -> Paragraph<'_> {
    let mut lines = vec![Line::from("Runtime logs"), Line::from("")];
    if let Some(error) = &state.runtime_info().config_error {
        lines.push(Line::styled(
            "Configuration error",
            Style::default().fg(Color::Red),
        ));
        lines.push(Line::from(error.as_str()));
        lines.push(Line::from(""));
    }
    for item in state.runtime_logs() {
        lines.push(Line::from(item.title.as_str()));
        if let Some(detail) = &item.detail {
            lines.push(Line::styled(
                detail.as_str(),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    sidebar_paragraph(state, lines)
}

pub(in crate::tui::ui) fn skill_panel(state: &AppState) -> Paragraph<'_> {
    let mut lines = vec![Line::from("Skills"), Line::from("")];
    for (index, skill) in state.skills().iter().enumerate() {
        let cursor = if index == state.selected_skill() {
            ">"
        } else {
            " "
        };
        let selected = if state.selected_skill_names().contains(&skill.name) {
            "[x]"
        } else {
            "[ ]"
        };
        let override_label = state
            .skill_overrides()
            .iter()
            .find(|item| item.name == skill.name)
            .map(|item| format!("; overrides {:?}", item.overridden.scope))
            .unwrap_or_default();
        lines.push(Line::from(format!(
            "{cursor} {selected} {} [{:?}{override_label}]",
            skill.name, skill.scope
        )));
    }
    if let Some(skill) = state.skills().get(state.selected_skill()) {
        lines.extend([
            Line::from(""),
            Line::from(skill.description.as_str()),
            Line::from(skill.path.display().to_string()),
        ]);
    }
    lines.extend([
        Line::from(""),
        Line::styled(
            "Up/Down select | Space toggle | Esc close",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    sidebar_paragraph(state, lines)
}

pub(in crate::tui::ui) fn audit_panel(state: &AppState) -> Paragraph<'_> {
    let mut lines = vec![Line::from("Audit"), Line::from("")];
    for item in state.audit_items() {
        lines.push(Line::from(item.title.as_str()));
        if let Some(detail) = &item.detail {
            lines.push(Line::styled(
                detail.as_str(),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    sidebar_paragraph(state, lines)
}

pub(in crate::tui::ui) fn session_panel(state: &AppState) -> Paragraph<'_> {
    let mut lines = vec![Line::from("Sessions"), Line::from("")];
    for (index, session) in state.sessions().iter().enumerate() {
        let selected = index == state.selected_session();
        let marker = match (selected, state.session_id() == Some(session.id.as_str())) {
            (true, true) => ">*",
            (true, false) => "> ",
            (false, true) => " *",
            (false, false) => "  ",
        };
        let name = session.name.as_deref().unwrap_or("(unnamed)");
        let style = if selected {
            Style::default().bg(Color::Rgb(40, 52, 64))
        } else {
            Style::default()
        };
        lines.push(Line::styled(format!("{marker} {name}"), style));
        lines.push(Line::from(format!("  {}", session.id)));
        if let Some(history) = state.session_history(&session.id) {
            for run in &history.runs {
                lines.push(Line::from(format!("  [{:?}] {}", run.status, run.request)));
            }
        }
    }
    sidebar_paragraph(state, lines)
}

pub(in crate::tui::ui) fn run_panel(state: &AppState) -> Paragraph<'_> {
    let status = match state.run_status() {
        TuiRunStatus::Idle => "Idle",
        TuiRunStatus::Running => "Running",
        TuiRunStatus::WaitingForHumanInput => "Waiting",
        TuiRunStatus::Completed => "Completed",
        TuiRunStatus::Failed => "Failed",
        TuiRunStatus::Canceled => "Canceled",
    };
    let lines = vec![
        Line::from("Jux"),
        Line::from(""),
        Line::from(format!("Session: {}", state.session_id().unwrap_or("-"))),
        Line::from(format!("Run: {}", state.run_id().unwrap_or("-"))),
        Line::from(format!(
            "Model: {}/{}",
            state.runtime_info().model_provider,
            state.runtime_info().model_name
        )),
        Line::from("Focus: Left/Right"),
        Line::from("Quit: Ctrl+C"),
        Line::from(""),
        Line::from(format!("Status: {status}")),
        Line::from(state.run_elapsed_millis().map_or_else(
            || "Elapsed: -".to_owned(),
            |millis| format!("Elapsed: {millis} ms"),
        )),
        Line::from(""),
        Line::from(format!("Workspace: {}", state.workspace_root.display())),
        Line::from(format!(
            "Workspace ID: {}",
            state.runtime_info().workspace_id.as_deref().unwrap_or("-")
        )),
        Line::from(format!(
            "Filesystem: {}",
            state.runtime_info().sandbox.filesystem
        )),
        Line::from(format!("Network: {}", state.runtime_info().sandbox.network)),
        Line::from(format!(
            "Native commands: {}",
            state.runtime_info().sandbox.native_commands
        )),
        Line::from("Mode: TUI shell"),
        Line::from("Tools: exec, lua, human_input"),
        Line::from("Skills: call_skill"),
        Line::from(format!(
            "Selected skills: {}",
            display_names(state.selected_skill_names())
        )),
        Line::from(format!(
            "Active skills: {}",
            display_names(state.active_skill_names())
        )),
    ];
    sidebar_paragraph(state, lines)
}

pub(in crate::tui::ui) fn help_panel(state: &AppState) -> Paragraph<'static> {
    sidebar_paragraph(
        state,
        vec![
            Line::from("Commands"),
            Line::from("/help  Show help"),
            Line::from("/clear Clear messages"),
            Line::from("/quit  Quit Jux"),
            Line::from("/new   Start a new session"),
            Line::from("/version Show the Jux version"),
            Line::from("/skills Browse and select skills"),
            Line::from("/logs   Show runtime logs"),
            Line::from(""),
            Line::from("Shortcuts"),
            Line::from("Shift+Enter Newline"),
            Line::from("PageUp/PageDown Scroll"),
            Line::from("Ctrl+C Quit"),
        ],
    )
}

fn sidebar_paragraph<'a>(state: &AppState, lines: Vec<Line<'a>>) -> Paragraph<'a> {
    let lines = apply_text_selection(state, SelectionPanel::Sidebar, 0, lines);
    Paragraph::new(lines)
        .block(panel_block(SIDEBAR_BACKGROUND, 2))
        .style(Style::default().bg(SIDEBAR_BACKGROUND))
        .wrap(Wrap { trim: true })
}
