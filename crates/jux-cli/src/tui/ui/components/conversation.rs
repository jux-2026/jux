use super::super::layout::ConversationLayout;
use super::super::text::{
    apply_text_selection, full_width_line, padded_full_width_lines, truncate_timeline_detail,
};
use super::super::theme::{
    COMMAND_POPUP_BACKGROUND, CONVERSATION_BACKGROUND, CONVERSATION_PADDING, STATUS_BAR_BACKGROUND,
    USER_MESSAGE_BACKGROUND, input_inactive_style, input_line_style, panel_block,
};
use crate::tui::{
    AppState, MessageRole, SelectionPanel, TimelineStatus, TuiCodeChangeResult, TuiRunStatus,
};
use jux_core::HumanInputKind;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConversationScroll {
    offset: u16,
    total_rows: u16,
    visible_rows: u16,
}

pub(crate) fn conversation_max_scroll(state: &AppState, area: Rect) -> u16 {
    let layout = ConversationLayout::calculate(state, area);
    let (_, scroll) = prompt_panel(
        state,
        layout.history.width.saturating_sub(2),
        layout.history.height.saturating_sub(2),
    );
    scroll.total_rows.saturating_sub(scroll.visible_rows)
}

pub(in crate::tui::ui) fn render_conversation_panel(
    frame: &mut Frame<'_>,
    state: &AppState,
    area: Rect,
    active: bool,
) {
    let layout = ConversationLayout::calculate(state, area);
    let (paragraph, scroll) = prompt_panel(
        state,
        layout.history.width.saturating_sub(2),
        layout.history.height.saturating_sub(2),
    );
    frame.render_widget(paragraph, layout.history);
    render_input_area(frame, state, layout, active);
    render_status_bar(frame, state, layout.status);
    render_conversation_scrollbar(frame, layout.scrollbar, scroll);
}

fn prompt_panel(
    state: &AppState,
    content_width: u16,
    visible_rows: u16,
) -> (Paragraph<'_>, ConversationScroll) {
    let mut lines = Vec::new();
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
    let lines = apply_text_selection(state, SelectionPanel::Conversation, 0, lines);
    let paragraph = Paragraph::new(lines)
        .block(panel_block(CONVERSATION_BACKGROUND, CONVERSATION_PADDING))
        .style(Style::default().bg(CONVERSATION_BACKGROUND))
        .wrap(Wrap { trim: false });
    // `line_count` receives the full Paragraph area width and accounts for the
    // block's inner padding itself.
    let total_rows = paragraph.line_count(content_width.saturating_add(2));
    let maximum = total_rows.saturating_sub(usize::from(visible_rows));
    let offset =
        maximum.saturating_sub(usize::from(state.conversation_scroll_from_bottom()).min(maximum));
    let scroll = ConversationScroll {
        offset: u16::try_from(offset).unwrap_or(u16::MAX),
        total_rows: u16::try_from(total_rows).unwrap_or(u16::MAX),
        visible_rows,
    };
    (
        paragraph.scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
        scroll,
    )
}

fn render_conversation_scrollbar(frame: &mut Frame<'_>, area: Rect, scroll: ConversationScroll) {
    if area.is_empty() {
        return;
    }
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(
            Style::default()
                .fg(Color::DarkGray)
                .bg(CONVERSATION_BACKGROUND),
        )
        .thumb_symbol("█")
        .thumb_style(Style::default().fg(Color::Gray).bg(CONVERSATION_BACKGROUND));
    // Ratatui 0.29 models `content_length` as the range of possible positions,
    // then adds `viewport_content_length` when calculating the thumb. Our
    // position is a viewport offset, so its range is `0..=maximum`, not the
    // number of rendered rows.
    let maximum = scroll.total_rows.saturating_sub(scroll.visible_rows);
    let mut scrollbar_state = ScrollbarState::new(usize::from(maximum.saturating_add(1)))
        .position(usize::from(scroll.offset))
        .viewport_content_length(usize::from(scroll.visible_rows));
    frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
}

fn render_input_area(
    frame: &mut Frame<'_>,
    state: &AppState,
    layout: ConversationLayout,
    active: bool,
) {
    let area = layout.input;
    if area.is_empty() {
        return;
    }
    let height = area.height;
    let popup_bounds = Rect::new(
        area.x,
        layout.history.y.saturating_add(1),
        area.width,
        area.y.saturating_sub(layout.history.y.saturating_add(1)),
    );
    render_slash_command_popup(frame, state, popup_bounds, area.y);
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
    let style = if active {
        input_line_style()
    } else {
        input_inactive_style()
    };
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

fn render_status_bar(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    if area.is_empty() {
        return;
    }
    let text = state.escape_confirmation_hint().unwrap_or_else(|| {
        if state.run_status() == TuiRunStatus::Running {
            "Shift+Enter newline | Esc twice to interrupt | Ctrl+C quit"
        } else {
            "Shift+Enter newline | Esc twice to clear | Ctrl+C quit"
        }
    });
    let style = Style::default().fg(Color::Gray).bg(STATUS_BAR_BACKGROUND);
    let aligned_text = format!("   {text}");
    frame.render_widget(
        Paragraph::new(full_width_line(&aligned_text, area.width, style)).style(style),
        area,
    );
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
