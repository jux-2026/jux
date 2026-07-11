use super::super::text::{apply_text_selection, display_names};
use super::super::theme::{palette, panel_block};
use crate::tui::{AppState, SelectionPanel, TuiRunStatus};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use std::time::{SystemTime, UNIX_EPOCH};

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
    let mut lines = vec![
        Line::from("Audit"),
        Line::from(format!("Filter: {:?} (F to change)", state.audit_filter())),
        Line::from(""),
    ];
    for (index, item) in state.filtered_audit_items().iter().enumerate() {
        let marker = if index == state.selected_audit_item() {
            ">"
        } else {
            " "
        };
        lines.push(Line::from(format!("{marker} {}", item.title)));
        if let Some(detail) = &item.detail {
            lines.push(Line::styled(
                detail.as_str(),
                Style::default().fg(Color::DarkGray),
            ));
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
        Line::from(format!("Session: {}", state.session_name().unwrap_or("-"))),
        Line::from(format!("Session ID: {}", state.session_id().unwrap_or("-"))),
        Line::from(format!("Run: {}", state.run_id().unwrap_or("-"))),
        Line::from(format!(
            "Model: {}/{}",
            state.runtime_info().model_provider,
            state.runtime_info().model_name
        )),
        Line::from("Focus: Left/Right"),
        Line::from(format!("Quit: {}", state.quit_shortcut_label())),
        Line::from(""),
        Line::from(format!("Status: {status}")),
        Line::from(format!("Activity: {}", activity_indicator(state))),
        Line::from(format!("Progress: {}", run_progress(state))),
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

fn activity_indicator(state: &AppState) -> &'static str {
    if state.run_status() != TuiRunStatus::Running {
        return "-";
    }
    const FRAMES: [&str; 4] = ["◐", "◓", "◑", "◒"];
    let frame = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() / 100);
    FRAMES[(frame as usize) % FRAMES.len()]
}

fn run_progress(state: &AppState) -> &'static str {
    if state.run_status() == TuiRunStatus::WaitingForHumanInput {
        return "Waiting for input";
    }
    if state.run_status() != TuiRunStatus::Running {
        return "-";
    }
    state.timeline().last().map_or("Thinking", |item| {
        if item.label.starts_with("LLM") {
            "Generating response"
        } else if item.label.starts_with("Tool") {
            "Calling tool"
        } else {
            "Thinking"
        }
    })
}

pub(in crate::tui::ui) fn help_panel(state: &AppState) -> Paragraph<'static> {
    let contextual = if state.skill_panel_visible() {
        "Up/Down select | Space toggle | Esc close"
    } else if state.session_panel_visible() {
        "Ctrl+N new | Enter switch | Ctrl+L favorite"
    } else if state.run_status() == TuiRunStatus::Running {
        "Esc twice Interrupt run"
    } else {
        "Home/End Jump | PageUp/PageDown Scroll"
    };
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
            Line::from(contextual),
            Line::from("Shift+Enter Newline"),
            Line::from("PageUp/PageDown Scroll"),
            Line::from(format!("{} Quit", state.quit_shortcut_label())),
            Line::from(format!("{} Copy message", state.copy_shortcut_label())),
        ],
    )
}

fn sidebar_paragraph<'a>(state: &AppState, lines: Vec<Line<'a>>) -> Paragraph<'a> {
    let lines = apply_text_selection(state, SelectionPanel::Sidebar, 0, lines);
    let background = palette(state.theme()).sidebar;
    Paragraph::new(lines)
        .block(panel_block(background, 2))
        .style(Style::default().bg(background))
        .wrap(Wrap { trim: true })
}
