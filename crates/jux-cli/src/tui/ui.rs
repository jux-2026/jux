use super::{AppState, MessageRole, TimelineStatus, TuiCodeChangeResult, TuiRunStatus};
use jux_core::{HumanInputKind, StepKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

const MAX_TIMELINE_DETAIL_CHARS: usize = 80;

pub fn render_app(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    let footer_height = if area.height >= 12 { 5 } else { 1 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(footer_height)])
        .split(area);

    render_workspace(frame, state, chunks[0]);
    frame.render_widget(status_bar(state), chunks[1]);
}

fn render_workspace(frame: &mut Frame<'_>, state: &AppState, area: ratatui::layout::Rect) {
    if area.width < 60 {
        frame.render_widget(prompt_panel(state), area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    frame.render_widget(prompt_panel(state), chunks[0]);
    if state.help_visible() {
        frame.render_widget(help_panel(), chunks[1]);
    } else if state.log_panel_visible() {
        frame.render_widget(log_panel(state), chunks[1]);
    } else if state.skill_panel_visible() {
        frame.render_widget(skill_panel(state), chunks[1]);
    } else if state.session_panel_visible() {
        frame.render_widget(session_panel(state), chunks[1]);
    } else if state.audit_panel_visible() {
        frame.render_widget(audit_panel(state), chunks[1]);
    } else {
        frame.render_widget(run_panel(state), chunks[1]);
    }
}

fn log_panel(state: &AppState) -> Paragraph<'_> {
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
    Paragraph::new(lines)
        .block(shell_block("logs"))
        .wrap(Wrap { trim: true })
}

fn skill_panel(state: &AppState) -> Paragraph<'_> {
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
    Paragraph::new(lines)
        .block(shell_block("skills"))
        .wrap(Wrap { trim: true })
}

fn audit_panel(state: &AppState) -> Paragraph<'_> {
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
    Paragraph::new(lines)
        .block(shell_block("audit"))
        .wrap(Wrap { trim: true })
}

fn session_panel(state: &AppState) -> Paragraph<'_> {
    let mut lines = vec![Line::from("Sessions"), Line::from("")];
    for session in state.sessions() {
        let marker = if state.session_id() == Some(session.id.as_str()) {
            "*"
        } else {
            " "
        };
        let name = session.name.as_deref().unwrap_or("(unnamed)");
        lines.push(Line::from(format!("{marker} {name}")));
        lines.push(Line::from(format!("  {}", session.id)));
        if let Some(history) = state.session_history(&session.id) {
            for run in &history.runs {
                lines.push(Line::from(format!("  [{:?}] {}", run.status, run.request)));
            }
        }
    }
    Paragraph::new(lines)
        .block(shell_block("sessions"))
        .wrap(Wrap { trim: true })
}

fn prompt_panel(state: &AppState) -> Paragraph<'_> {
    let mut lines = vec![Line::from("What should Jux work on?"), Line::from("")];
    for message in state.messages() {
        let (label, color) = match message.role {
            MessageRole::User => ("You", Color::Cyan),
            MessageRole::Assistant => ("Jux", Color::Cyan),
            MessageRole::Error => ("Error", Color::Red),
        };
        lines.push(Line::styled(label, Style::default().fg(color)));
        lines.extend(message.content.lines().map(Line::from));
        lines.push(Line::from(""));
    }
    for item in state.timeline() {
        let status = match item.status {
            TimelineStatus::Running => "Running",
            TimelineStatus::Output => "Output",
            TimelineStatus::Completed => "Completed",
            TimelineStatus::Failed => "Failed",
        };
        lines.push(Line::from(format!("{}  {status}", item.label)));
        if let Some(detail) = &item.detail {
            lines.push(Line::from(detail.as_str()));
        }
        if item.expanded {
            if let Some(arguments) = &item.arguments {
                lines.push(Line::from("Arguments:"));
                let arguments = truncate_timeline_detail(arguments);
                lines.extend(arguments.lines().map(|line| Line::from(line.to_owned())));
            }
            if let Some(output) = &item.output {
                lines.push(Line::from("Output:"));
                let output = truncate_timeline_detail(output);
                lines.extend(output.lines().map(|line| Line::from(line.to_owned())));
            }
        } else if let Some(output) = &item.output {
            let summary = output.split_whitespace().collect::<Vec<_>>().join(" ");
            lines.push(Line::from(truncate_timeline_detail(&summary)));
        }
    }
    if !state.timeline().is_empty() {
        lines.push(Line::from(""));
    }
    for step in state.steps() {
        let label = match step.kind {
            StepKind::UserMessage => "User message",
            StepKind::AssistantResponse => "Assistant response",
            StepKind::ToolResult => "Tool result",
            StepKind::SkillExecution => "Skill execution",
            StepKind::Error => "Error",
        };
        lines.push(Line::from(format!("Step  {label}")));
    }
    if !state.steps().is_empty() {
        lines.push(Line::from(""));
    }
    if let Some(request) = state.pending_human_input() {
        let title = match request.kind {
            HumanInputKind::Clarification => "Input required",
            HumanInputKind::Confirmation => "Confirmation required",
        };
        lines.push(Line::styled(title, Style::default().fg(Color::Yellow)));
        lines.push(Line::from(request.prompt.as_str()));
        if let Some(reason) = &request.reason {
            lines.push(Line::from(reason.as_str()));
        }
        for (index, option) in request.options.iter().enumerate() {
            let marker = if index == state.selected_human_option() {
                ">"
            } else {
                " "
            };
            lines.push(Line::from(format!(
                "{marker} {}  {}",
                option.id, option.label
            )));
        }
        if let Some(error) = state.human_input_error() {
            lines.push(Line::styled(error, Style::default().fg(Color::Red)));
        }
        lines.push(Line::from(""));
    }
    if let Some(review) = state.code_change_review() {
        lines.push(Line::styled(
            format!("Plan: {}", review.proposal.plan.summary),
            Style::default().fg(Color::Yellow),
        ));
        for item in &review.proposal.plan.items {
            lines.push(Line::from(format!("- {item}")));
        }
        lines.push(Line::from(format!("Policy: {:?}", review.proposal.policy)));
        lines.push(Line::from(format!("Review: {:?}", review.status)));
        if let Some(result) = state.code_change_result() {
            let message = match result {
                TuiCodeChangeResult::Applied { file_count } => {
                    format!("Applied {file_count} file(s)")
                }
                TuiCodeChangeResult::Rejected => "Rejected".to_owned(),
                TuiCodeChangeResult::ChangesRequested => "Changes requested".to_owned(),
                TuiCodeChangeResult::Conflict { paths } => {
                    format!("Conflict: {}", paths.join(", "))
                }
                TuiCodeChangeResult::Denied => "Denied by policy".to_owned(),
            };
            lines.push(Line::from(message));
        }
        for warning in &review.proposal.warnings {
            lines.push(Line::styled(
                format!(
                    "Risk [{:?}] {}: {}",
                    warning.level,
                    warning.path.as_str(),
                    warning.reason
                ),
                Style::default().fg(Color::Red),
            ));
        }
        for (index, file) in review.proposal.files.iter().enumerate() {
            let marker = if index == state.selected_changed_file() {
                ">"
            } else {
                " "
            };
            lines.push(Line::from(format!("{marker} {}", file.path.as_str())));
        }
        if let Some(file) = review.proposal.files.get(state.selected_changed_file()) {
            lines.extend(file.diff.lines().map(Line::from));
        }
        lines.push(Line::from(""));
    }
    if state.input_text().is_empty() {
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "Start typing in the next step",
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    } else {
        lines.extend(state.input_text().lines().map(|line| {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::Cyan)),
                Span::raw(line),
            ])
        }));
    }
    lines.extend([
        Line::from(""),
        Line::from("No active run."),
        Line::from("Use the CLI subcommands for run, skills, and session inspection."),
    ]);
    Paragraph::new(lines)
        .block(shell_block("conversation"))
        .scroll((state.message_scroll(), 0))
        .wrap(Wrap { trim: true })
}

fn truncate_timeline_detail(content: &str) -> String {
    if content.chars().count() <= MAX_TIMELINE_DETAIL_CHARS {
        return content.to_owned();
    }
    let mut truncated = content
        .chars()
        .take(MAX_TIMELINE_DETAIL_CHARS)
        .collect::<String>();
    truncated.push_str("… [truncated]");
    truncated
}

fn run_panel(state: &AppState) -> Paragraph<'_> {
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
        Line::from(format!("Status: {status}")),
        Line::from(format!("Session: {}", state.session_id().unwrap_or("-"))),
        Line::from(format!("Run: {}", state.run_id().unwrap_or("-"))),
        Line::from(match state.run_elapsed_millis() {
            Some(millis) => format!("Elapsed: {millis} ms"),
            None => "Elapsed: -".to_owned(),
        }),
        Line::from(""),
        Line::from(format!("Workspace: {}", state.workspace_root.display())),
        Line::from(format!(
            "Workspace ID: {}",
            state.runtime_info().workspace_id.as_deref().unwrap_or("-")
        )),
        Line::from(format!(
            "Model: {}/{}",
            state.runtime_info().model_provider,
            state.runtime_info().model_name
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
    Paragraph::new(lines)
        .block(shell_block("status"))
        .wrap(Wrap { trim: true })
}

fn help_panel() -> Paragraph<'static> {
    let lines = vec![
        Line::from("Commands"),
        Line::from("/help  Show help"),
        Line::from("/clear Clear messages"),
        Line::from("/quit  Quit Jux"),
        Line::from("/skills Browse and select skills"),
        Line::from("/logs   Show runtime logs"),
        Line::from(""),
        Line::from("Shortcuts"),
        Line::from("Shift+Enter Newline"),
        Line::from("PageUp/PageDown Scroll"),
        Line::from("Ctrl+C Quit"),
    ];
    Paragraph::new(lines)
        .block(shell_block("help"))
        .wrap(Wrap { trim: true })
}

fn display_names(names: &[String]) -> String {
    if names.is_empty() {
        "-".to_owned()
    } else {
        names.join(", ")
    }
}

fn status_bar(state: &AppState) -> Paragraph<'_> {
    let status = match state.run_status() {
        TuiRunStatus::Idle => "Idle",
        TuiRunStatus::Running => "Running",
        TuiRunStatus::WaitingForHumanInput => "Waiting",
        TuiRunStatus::Completed => "Completed",
        TuiRunStatus::Failed => "Failed",
        TuiRunStatus::Canceled => "Canceled",
    };
    Paragraph::new(format!(
        "Session {} | Run {} | {}/{} | {status} | Ctrl+C quit",
        state.session_id().unwrap_or("-"),
        state.run_id().unwrap_or("-"),
        state.runtime_info().model_provider,
        state.runtime_info().model_name
    ))
    .style(Style::default().fg(Color::DarkGray))
    .block(shell_block("keys"))
    .alignment(Alignment::Center)
}

fn shell_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" {title}"),
            Style::default().fg(Color::Gray),
        ))
        .padding(ratatui::widgets::Padding::uniform(1))
}
