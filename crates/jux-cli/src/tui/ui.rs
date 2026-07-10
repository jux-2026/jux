use super::{
    AppState, FocusedPanel, MessageRole, SelectionPanel, TextSelectionPoint, TimelineStatus,
    TuiCodeChangeResult, TuiRunStatus,
};
use jux_core::{HumanInputKind, StepKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_TIMELINE_DETAIL_CHARS: usize = 80;
const CONVERSATION_BACKGROUND: Color = Color::Rgb(12, 18, 24);
const SIDEBAR_BACKGROUND: Color = Color::Rgb(18, 24, 32);
const DIVIDER_BACKGROUND: Color = Color::Rgb(24, 32, 42);
const COMMAND_POPUP_BACKGROUND: Color = Color::Rgb(28, 36, 46);
const USER_MESSAGE_BACKGROUND: Color = Color::Rgb(24, 34, 44);
const CONVERSATION_PADDING: u16 = 1;
const SIDEBAR_PADDING: u16 = 2;

pub fn render_app(frame: &mut Frame<'_>, state: &AppState) {
    render_workspace(frame, state, frame.area());
}

fn render_workspace(frame: &mut Frame<'_>, state: &AppState, area: ratatui::layout::Rect) {
    if area.width < 60 {
        frame.render_widget(prompt_panel(state, area.width.saturating_sub(2)), area);
        render_input_area(frame, state, area, true);
        return;
    }
    let conversation_width = state.conversation_panel_width(area.width);
    let conversation_area = Rect::new(area.x, area.y, conversation_width, area.height);
    let divider_area = Rect::new(
        area.x.saturating_add(conversation_width),
        area.y,
        1,
        area.height,
    );
    let conversation_focused = state.focused_panel() == FocusedPanel::Conversation;
    frame.render_widget(
        prompt_panel(state, conversation_area.width.saturating_sub(2)),
        conversation_area,
    );
    render_input_area(frame, state, conversation_area, conversation_focused);
    render_divider(frame, divider_area, state.sidebar_visible());
    if !state.sidebar_visible() {
        return;
    }
    let sidebar_area = Rect::new(
        divider_area.x.saturating_add(1),
        area.y,
        area.width
            .saturating_sub(conversation_width)
            .saturating_sub(1),
        area.height,
    );
    if state.help_visible() {
        frame.render_widget(help_panel(state), sidebar_area);
    } else if state.log_panel_visible() {
        frame.render_widget(log_panel(state), sidebar_area);
    } else if state.skill_panel_visible() {
        frame.render_widget(skill_panel(state), sidebar_area);
    } else if state.session_panel_visible() {
        frame.render_widget(session_panel(state), sidebar_area);
    } else if state.audit_panel_visible() {
        frame.render_widget(audit_panel(state), sidebar_area);
    } else {
        frame.render_widget(run_panel(state), sidebar_area);
    }
}

fn render_divider(frame: &mut Frame<'_>, area: Rect, sidebar_visible: bool) {
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
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(DIVIDER_BACKGROUND)),
        area,
    );
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
    let lines = apply_text_selection(state, SelectionPanel::Sidebar, 0, lines);
    Paragraph::new(lines)
        .block(panel_block(SIDEBAR_BACKGROUND, SIDEBAR_PADDING))
        .style(Style::default().bg(SIDEBAR_BACKGROUND))
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
    let lines = apply_text_selection(state, SelectionPanel::Sidebar, 0, lines);
    Paragraph::new(lines)
        .block(panel_block(SIDEBAR_BACKGROUND, SIDEBAR_PADDING))
        .style(Style::default().bg(SIDEBAR_BACKGROUND))
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
    let lines = apply_text_selection(state, SelectionPanel::Sidebar, 0, lines);
    Paragraph::new(lines)
        .block(panel_block(SIDEBAR_BACKGROUND, SIDEBAR_PADDING))
        .style(Style::default().bg(SIDEBAR_BACKGROUND))
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
    let lines = apply_text_selection(state, SelectionPanel::Sidebar, 0, lines);
    Paragraph::new(lines)
        .block(panel_block(SIDEBAR_BACKGROUND, SIDEBAR_PADDING))
        .style(Style::default().bg(SIDEBAR_BACKGROUND))
        .wrap(Wrap { trim: true })
}

fn prompt_panel(state: &AppState, content_width: u16) -> Paragraph<'_> {
    let mut lines = vec![Line::from("What should Jux work on?"), Line::from("")];
    for message in state.messages() {
        match message.role {
            MessageRole::User => {
                let background = Style::default().bg(USER_MESSAGE_BACKGROUND);
                lines.push(full_width_line("", content_width, background));
                lines.extend(padded_full_width_lines(
                    "You",
                    content_width,
                    background.fg(Color::Cyan),
                ));
                lines.extend(
                    message
                        .content
                        .split('\n')
                        .flat_map(|line| padded_full_width_lines(line, content_width, background)),
                );
                lines.push(full_width_line("", content_width, background));
                lines.push(Line::from(""));
            }
            MessageRole::Assistant | MessageRole::Error => {
                let (label, color) = match message.role {
                    MessageRole::Assistant => ("Jux", Color::Cyan),
                    MessageRole::Error => ("Error", Color::Red),
                    MessageRole::User => unreachable!(),
                };
                lines.push(Line::styled(label, Style::default().fg(color)));
                lines.extend(message.content.lines().map(Line::from));
                lines.push(Line::from(""));
            }
        }
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
    lines.extend([
        Line::from(""),
        Line::from("No active run."),
        Line::from("Use the CLI subcommands for run, skills, and session inspection."),
    ]);
    let lines = apply_text_selection(state, SelectionPanel::Conversation, 0, lines);
    Paragraph::new(lines)
        .block(panel_block(CONVERSATION_BACKGROUND, CONVERSATION_PADDING))
        .style(Style::default().bg(CONVERSATION_BACKGROUND))
        .scroll((state.message_scroll(), 0))
        .wrap(Wrap { trim: false })
}

fn full_width_line(text: &str, width: u16, style: Style) -> Line<'static> {
    let width = usize::from(width);
    let text_width = UnicodeWidthStr::width(text);
    let padding = if width == 0 {
        0
    } else if text_width == 0 {
        width
    } else if text_width.is_multiple_of(width) {
        0
    } else {
        width - (text_width % width)
    };
    Line::styled(format!("{text}{}", "\u{00a0}".repeat(padding)), style)
}

fn padded_full_width_lines(text: &str, width: u16, style: Style) -> Vec<Line<'static>> {
    let content_width = usize::from(width.saturating_sub(2));
    if content_width == 0 {
        return vec![full_width_line("", width, style)];
    }
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut chunk_width = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or_default();
        if !chunk.is_empty() && chunk_width + character_width > content_width {
            chunks.push(full_width_line(&format!("\u{00a0}{chunk}"), width, style));
            chunk.clear();
            chunk_width = 0;
        }
        chunk.push(character);
        chunk_width += character_width;
    }
    chunks.push(full_width_line(&format!("\u{00a0}{chunk}"), width, style));
    chunks
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

fn input_line_style() -> Style {
    Style::default().bg(Color::Rgb(20, 38, 48))
}

fn render_input_area(frame: &mut Frame<'_>, state: &AppState, panel_area: Rect, active: bool) {
    let inner = Rect {
        x: panel_area.x.saturating_add(1),
        y: panel_area.y.saturating_add(1),
        width: panel_area.width.saturating_sub(2),
        height: panel_area.height.saturating_sub(2),
    };
    if inner.is_empty() {
        return;
    }
    // `str::lines` drops a trailing empty line, but that line is where the cursor sits immediately
    // after Shift+Enter. Count split segments so the bottom padding is reserved before more text is
    // typed on the new line.
    let input_line_count = state.input_text().split('\n').count().max(1) as u16;
    let height = input_line_count.saturating_add(2).min(inner.height);
    let area = Rect {
        x: inner.x,
        y: inner.y + inner.height - height,
        width: inner.width,
        height,
    };
    render_slash_command_popup(frame, state, inner, area.y);
    let mut lines = Vec::new();
    if height >= 3 {
        lines.push(Line::from(""));
    }
    lines.extend(input_lines(state));
    if height >= 3 {
        lines.push(Line::from(""));
    }
    let (cursor_line, cursor_column) = state.input_cursor_line_column();
    let cursor_line = cursor_line.saturating_add(u16::from(height >= 3));
    let vertical_scroll = cursor_line.saturating_sub(height.saturating_sub(1));
    let style = active.then(input_line_style).unwrap_or_default();
    frame.render_widget(
        Paragraph::new(lines)
            .style(style)
            .scroll((vertical_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
    if active {
        let cursor_x = area
            .x
            .saturating_add(3)
            .saturating_add(cursor_column)
            .min(area.x.saturating_add(area.width.saturating_sub(1)));
        let cursor_y = area
            .y
            .saturating_add(cursor_line.saturating_sub(vertical_scroll))
            .min(area.y.saturating_add(area.height.saturating_sub(1)));
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

fn render_slash_command_popup(
    frame: &mut Frame<'_>,
    state: &AppState,
    input_bounds: Rect,
    input_top: u16,
) {
    let suggestions = state.slash_command_suggestions();
    if suggestions.is_empty() {
        return;
    }
    let available_height = input_top.saturating_sub(input_bounds.y);
    let height = u16::try_from(suggestions.len())
        .unwrap_or(available_height)
        .saturating_add(2)
        .min(available_height);
    if height < 3 {
        return;
    }
    let row_width = usize::from(input_bounds.width.saturating_sub(2));
    let lines = suggestions
        .iter()
        .enumerate()
        .map(|(index, definition)| {
            let style = if index == state.selected_slash_command() {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };
            let content = format!("{:<10} {}", definition.name, definition.description);
            let content = content.chars().take(row_width).collect::<String>();
            Line::styled(format!("{content:<row_width$}"), style)
        })
        .collect::<Vec<_>>();
    let area = Rect::new(
        input_bounds.x,
        input_top.saturating_sub(height),
        input_bounds.width,
        height,
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .style(Style::default().bg(COMMAND_POPUP_BACKGROUND))
                    .padding(Padding::uniform(1)),
            )
            .style(Style::default().bg(COMMAND_POPUP_BACKGROUND)),
        area,
    );
}

fn input_lines(state: &AppState) -> Vec<Line<'_>> {
    if state.input_text().is_empty() {
        return vec![Line::from(vec![
            Span::raw(" "),
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "Start typing in the next step",
                Style::default().fg(Color::DarkGray),
            ),
        ])];
    }
    state
        .input_text()
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 {
                Span::styled(" > ", Style::default().fg(Color::Cyan))
            } else {
                Span::raw("   ")
            };
            Line::from(vec![prefix, Span::raw(line)])
        })
        .collect()
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
    let lines = apply_text_selection(state, SelectionPanel::Sidebar, 0, lines);
    Paragraph::new(lines)
        .block(panel_block(SIDEBAR_BACKGROUND, SIDEBAR_PADDING))
        .style(Style::default().bg(SIDEBAR_BACKGROUND))
        .wrap(Wrap { trim: true })
}

fn help_panel(state: &AppState) -> Paragraph<'static> {
    let lines = vec![
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
    ];
    let lines = apply_text_selection(state, SelectionPanel::Sidebar, 0, lines);
    Paragraph::new(lines)
        .block(panel_block(SIDEBAR_BACKGROUND, SIDEBAR_PADDING))
        .style(Style::default().bg(SIDEBAR_BACKGROUND))
        .wrap(Wrap { trim: true })
}

fn display_names(names: &[String]) -> String {
    if names.is_empty() {
        "-".to_owned()
    } else {
        names.join(", ")
    }
}

fn panel_block(background: Color, padding: u16) -> Block<'static> {
    Block::default()
        .style(Style::default().bg(background))
        .padding(Padding::uniform(padding))
}

fn selection_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Yellow)
}

fn apply_text_selection<'a>(
    state: &AppState,
    panel: SelectionPanel,
    line_offset: usize,
    lines: Vec<Line<'a>>,
) -> Vec<Line<'a>> {
    let Some(selection) = state.text_selection() else {
        return lines;
    };
    if selection.panel != panel {
        return lines;
    }
    let (start, end) = ordered_points(selection.anchor, selection.focus);
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let absolute_line = index.saturating_add(line_offset);
            if absolute_line < start.line || absolute_line > end.line {
                return line;
            }
            let text = line_text(&line);
            let start_column = if absolute_line == start.line {
                start.column
            } else {
                0
            };
            let end_column = if absolute_line == end.line {
                end.column
            } else {
                text.chars().count()
            };
            selected_line(&text, start_column, end_column)
        })
        .collect()
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn selected_line(text: &str, start: usize, end: usize) -> Line<'static> {
    let before = take_chars(text, 0, start);
    let selected = take_chars(text, start, end);
    let after = take_chars(text, end, text.chars().count());
    Line::from(vec![
        Span::raw(before),
        Span::styled(selected, selection_style()),
        Span::raw(after),
    ])
}

fn take_chars(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn ordered_points(
    first: TextSelectionPoint,
    second: TextSelectionPoint,
) -> (TextSelectionPoint, TextSelectionPoint) {
    if (first.line, first.column) <= (second.line, second.column) {
        (first, second)
    } else {
        (second, first)
    }
}
