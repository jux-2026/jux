use super::super::layout::ConversationLayout;
use super::super::text::{
    apply_text_selection, full_width_line, padded_full_width_lines, truncate_timeline_detail,
};
use super::super::theme::{CONVERSATION_PADDING, palette, panel_block};
use super::command_output;
use super::markdown::MarkdownRenderer;
use crate::tui::{
    AppState, MessageRole, SelectionPanel, TimelineStatus, TuiCodeChangeResult, TuiRunStatus,
    TuiViewport,
};
use jux_core::{AssistantResponseItem, HumanInputKind, LlmUsage, StepPayload};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE_REFERENCE_ROWS: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConversationScroll {
    offset: u16,
    total_rows: u16,
    visible_rows: u16,
    command_toggle_rows: Vec<(u16, usize)>,
}

pub(crate) fn command_toggle_at(
    state: &AppState,
    viewport: TuiViewport,
    column: u16,
    row: u16,
) -> Option<usize> {
    let area = Rect::new(
        0,
        0,
        state.conversation_panel_width(viewport.width),
        viewport.height,
    );
    let layout = ConversationLayout::calculate(state, area);
    if column <= layout.history.x
        || column >= layout.history.right().saturating_sub(1)
        || row <= layout.history.y
        || row >= layout.history.bottom().saturating_sub(1)
    {
        return None;
    }
    let (_, scroll) = prompt_panel(
        state,
        layout.history.width.saturating_sub(2),
        layout.history.height.saturating_sub(2),
    );
    let visible_row = row.saturating_sub(layout.history.y.saturating_add(1));
    let content_row = scroll.offset.saturating_add(visible_row);
    scroll
        .command_toggle_rows
        .iter()
        .find_map(|(toggle_row, index)| (*toggle_row == content_row).then_some(*index))
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
    render_conversation_scrollbar(frame, state, layout.scrollbar, scroll);
}

fn prompt_panel(
    state: &AppState,
    content_width: u16,
    visible_rows: u16,
) -> (Paragraph<'_>, ConversationScroll) {
    let mut lines = Vec::new();
    let mut command_toggle_rows = Vec::new();
    let mut rendered_rows = RenderedRowCounter::default();
    let colors = palette(state.theme());
    append_timeline_items(
        state,
        0,
        content_width,
        &mut lines,
        &mut command_toggle_rows,
        &mut rendered_rows,
    );
    for (message_index, message) in state.messages().iter().enumerate() {
        let selected = state.selected_message() == Some(message_index);
        match message.role {
            MessageRole::User => {
                let background = Style::default().bg(colors.input);
                lines.push(full_width_line("", content_width, background));
                lines.extend(message.content.split('\n').flat_map(|line| {
                    let marker = if selected { "▶" } else { ">" };
                    padded_full_width_lines(&format!("{marker} {line}"), content_width, background)
                }));
                lines.push(full_width_line("", content_width, background));
            }
            MessageRole::Assistant | MessageRole::Error => {
                append_spacing(&mut lines);
                let markdown_width = content_width.saturating_sub(6);
                lines.extend(
                    MarkdownRenderer::new(markdown_width)
                        .render(&message.content)
                        .into_iter()
                        .map(pad_assistant_line),
                );
                if let Some(metadata) = response_metadata_for_message(state, message) {
                    append_spacing(&mut lines);
                    lines.push(Line::styled(
                        format!("\u{00a0}\u{00a0}\u{00a0}{metadata}"),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                append_spacing(&mut lines);
            }
        }
        append_timeline_items(
            state,
            message_index.saturating_add(1),
            content_width,
            &mut lines,
            &mut command_toggle_rows,
            &mut rendered_rows,
        );
    }
    if state.run_status() == TuiRunStatus::Running {
        append_spacing(&mut lines);
        let (indicator, color) = generating_frame();
        let (input_tokens, output_tokens) = state.estimated_token_usage();
        lines.push(Line::styled(
            format!(
                "   Generating {indicator} · ~{} in / ~{} out",
                format_compact_number(input_tokens),
                format_compact_number(output_tokens)
            ),
            Style::default().fg(color),
        ));
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
        .block(panel_block(colors.conversation, CONVERSATION_PADDING))
        .style(Style::default().bg(colors.conversation))
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
        command_toggle_rows,
    };
    (
        paragraph.scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
        scroll,
    )
}

fn append_spacing(lines: &mut Vec<Line<'_>>) {
    if lines.last().is_none_or(line_has_content) {
        lines.push(Line::from(""));
    }
}

fn line_has_content(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .any(|span| !span.content.as_ref().is_empty())
}

fn append_timeline_items<'a>(
    state: &'a AppState,
    message_count: usize,
    content_width: u16,
    lines: &mut Vec<Line<'a>>,
    command_toggle_rows: &mut Vec<(u16, usize)>,
    rendered_rows: &mut RenderedRowCounter,
) {
    if state
        .timeline()
        .iter()
        .any(|item| item.message_count == message_count && item.command.is_some())
    {
        append_spacing(lines);
    }
    for (timeline_index, item) in state.timeline().iter().enumerate() {
        if item.message_count != message_count {
            continue;
        }
        if let Some(command) = &item.command {
            command_toggle_rows.push((rendered_rows.count(lines, content_width), timeline_index));
            lines.extend(command_output::render(
                command,
                item.status,
                item.detail.as_deref(),
                item.expanded,
                content_width,
            ));
            let next_is_command = state
                .timeline()
                .get(timeline_index.saturating_add(1))
                .is_some_and(|next| next.message_count == message_count && next.command.is_some());
            if !next_is_command {
                lines.push(Line::from(""));
            }
            continue;
        }
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
}

fn pad_assistant_line(mut line: Line<'_>) -> Line<'_> {
    line.spans.insert(0, Span::raw("\u{00a0}\u{00a0}\u{00a0}"));
    line.spans.push(Span::raw("\u{00a0}\u{00a0}\u{00a0}"));
    line
}

fn response_metadata_for_message(
    state: &AppState,
    message: &crate::tui::Message,
) -> Option<String> {
    let step = state
        .steps()
        .iter()
        .find(|step| match (&message.role, &step.payload) {
            (MessageRole::Assistant, StepPayload::AssistantResponse { items, .. }) => {
                assistant_text(items) == message.content
            }
            (MessageRole::Error, StepPayload::Error { message: error }) => {
                error == &message.content
            }
            _ => false,
        })?;
    let elapsed = state
        .runs()
        .iter()
        .find(|run| run.id == step.id.run_id())
        .map(|run| run.updated_at.saturating_sub(run.created_at));
    let usage = match &step.payload {
        StepPayload::AssistantResponse { usage, .. } => Some(usage),
        _ => None,
    };
    let metadata = format_response_metadata(usage, elapsed)?;
    (message.role == MessageRole::Error)
        .then(|| format!("Failed · {metadata}"))
        .or(Some(metadata))
}

fn generating_frame() -> (&'static str, Color) {
    const FRAMES: [(&str, Color); 6] = [
        ("·", Color::Rgb(70, 130, 180)),
        ("∙", Color::Rgb(64, 170, 190)),
        ("●", Color::Rgb(80, 200, 170)),
        ("∙", Color::Rgb(190, 210, 90)),
        ("·", Color::Rgb(230, 180, 70)),
        (" ", Color::Rgb(170, 110, 190)),
    ];
    let frame = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() / 120);
    FRAMES[(frame as usize) % FRAMES.len()]
}

fn assistant_text(items: &[AssistantResponseItem]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            AssistantResponseItem::Text { content } => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

fn format_response_metadata(
    usage: Option<&LlmUsage>,
    elapsed_millis: Option<u128>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(usage) = usage {
        parts.push(format!(
            "{} tokens ({} in / {} out)",
            format_compact_number(usage.total_tokens),
            format_compact_number(usage.input_tokens),
            format_compact_number(usage.output_tokens)
        ));
    }
    if let Some(elapsed_millis) = elapsed_millis {
        parts.push(format_duration(elapsed_millis));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn format_compact_number(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => format_compact(value, 1_000, "k"),
        _ => format_compact(value, 1_000_000, "M"),
    }
}

fn format_compact(value: u64, divisor: u64, suffix: &str) -> String {
    let scaled = value as f64 / divisor as f64;
    if scaled >= 100.0 || (scaled.fract() == 0.0) {
        format!("{scaled:.0}{suffix}")
    } else {
        format!("{scaled:.1}{suffix}")
    }
}

fn format_duration(elapsed_millis: u128) -> String {
    if elapsed_millis < 1_000 {
        format!("{elapsed_millis} ms")
    } else {
        format!("{:.1} s", elapsed_millis as f64 / 1_000.0)
    }
}

#[derive(Default)]
struct RenderedRowCounter {
    counted_lines: usize,
    rows: u16,
}

impl RenderedRowCounter {
    fn count(&mut self, lines: &[Line<'_>], content_width: u16) -> u16 {
        // Command hit-testing needs the wrapped row before each command. Do
        // not call `line_count` for the complete prefix here: sessions with
        // many commands then repeatedly process the same earlier lines and
        // turn conversation rendering into O(commands * history). Counting
        // only newly appended lines keeps the whole layout pass linear.
        let new_lines = &lines[self.counted_lines..];
        if !new_lines.is_empty() {
            let paragraph = Paragraph::new(new_lines.to_vec()).wrap(Wrap { trim: false });
            let rows = u16::try_from(paragraph.line_count(content_width)).unwrap_or(u16::MAX);
            self.rows = self.rows.saturating_add(rows);
            self.counted_lines = lines.len();
        }
        self.rows
    }
}

fn render_conversation_scrollbar(
    frame: &mut Frame<'_>,
    state: &AppState,
    area: Rect,
    scroll: ConversationScroll,
) {
    if area.is_empty() {
        return;
    }
    let background = palette(state.theme()).conversation;
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(Style::default().fg(Color::DarkGray).bg(background))
        .thumb_symbol("█")
        .thumb_style(Style::default().fg(Color::Gray).bg(background));
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
    render_inline_skill_popup(frame, state, popup_bounds, area.y);
    render_file_reference_popup(frame, state, popup_bounds, area.y);
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
    let colors = palette(state.theme());
    let style = if active {
        Style::default().bg(colors.input)
    } else {
        Style::default().bg(colors.input_inactive)
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

fn render_inline_skill_popup(
    frame: &mut Frame<'_>,
    state: &AppState,
    input_bounds: Rect,
    input_top: u16,
) {
    let suggestions = state.inline_skill_suggestions();
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
        .map(|(index, skill)| {
            let style = if index == state.selected_inline_skill() {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };
            let content = format!("${:<20} {}", skill.name, skill.description);
            Line::styled(content.chars().take(row_width).collect::<String>(), style)
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
                    .style(Style::default().bg(palette(state.theme()).popup))
                    .padding(Padding::uniform(1)),
            )
            .style(Style::default().bg(palette(state.theme()).popup)),
        area,
    );
}

fn render_file_reference_popup(
    frame: &mut Frame<'_>,
    state: &AppState,
    input_bounds: Rect,
    input_top: u16,
) {
    let suggestions = state.file_reference_suggestions();
    if suggestions.is_empty() {
        return;
    }
    let available_height = input_top.saturating_sub(input_bounds.y);
    let visible_count = suggestions
        .len()
        .min(MAX_FILE_REFERENCE_ROWS)
        .min(usize::from(available_height.saturating_sub(2)));
    let height = u16::try_from(visible_count)
        .unwrap_or_default()
        .saturating_add(2);
    if height < 3 {
        return;
    }
    let selected = state
        .selected_file_reference()
        .min(suggestions.len().saturating_sub(1));
    let window_start = selected
        .saturating_add(1)
        .saturating_sub(visible_count)
        .min(suggestions.len().saturating_sub(visible_count));
    let row_width = usize::from(input_bounds.width.saturating_sub(2));
    let lines = suggestions
        .iter()
        .enumerate()
        .skip(window_start)
        .take(visible_count)
        .map(|(index, path)| {
            let style = if index == selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::styled(
                format!("@{path}")
                    .chars()
                    .take(row_width)
                    .collect::<String>(),
                style,
            )
        })
        .collect::<Vec<_>>();
    let area = Rect::new(
        input_bounds.x,
        input_top.saturating_sub(height),
        input_bounds.width,
        height,
    );
    let background = palette(state.theme()).popup;
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .style(Style::default().bg(background))
                    .padding(Padding::uniform(1)),
            )
            .style(Style::default().bg(background)),
        area,
    );
}

fn render_status_bar(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    if area.is_empty() {
        return;
    }
    let text = state
        .notification()
        .or_else(|| state.escape_confirmation_hint())
        .or(state.conversation_search())
        .unwrap_or_else(|| {
            if state.run_status() == TuiRunStatus::Running {
                "Shift+Enter newline | Esc twice to interrupt | Ctrl+C quit"
            } else {
                "Shift+Enter newline | Esc twice to clear | Ctrl+C quit"
            }
        });
    let text = text.replace("Ctrl+C", state.quit_shortcut_label());
    let text = format!("{text} | {}", state.scroll_position_label());
    let style = Style::default()
        .fg(Color::Gray)
        .bg(palette(state.theme()).status);
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
            let arguments = if definition.usage != definition.name {
                format!(" ({})", definition.usage)
            } else {
                String::new()
            };
            let content = format!(
                "{:<10} {}{arguments}",
                definition.name, definition.description
            );
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
                    .style(Style::default().bg(palette(state.theme()).popup))
                    .padding(Padding::uniform(1)),
            )
            .style(Style::default().bg(palette(state.theme()).popup)),
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
    let mut line_start = 0;
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
            let mut spans = vec![prefix];
            spans.extend(highlighted_file_references(state, line, line_start));
            line_start += line.len() + 1;
            Line::from(spans)
        })
        .collect()
}

fn highlighted_file_references<'a>(
    state: &AppState,
    line: &'a str,
    line_start: usize,
) -> Vec<Span<'a>> {
    let line_end = line_start + line.len();
    let mut spans = Vec::new();
    let mut cursor = 0;
    for (start, end) in state.completed_file_reference_ranges() {
        if start < line_start || end > line_end {
            continue;
        }
        let local_start = start - line_start;
        let local_end = end - line_start;
        if cursor < local_start {
            spans.push(Span::raw(&line[cursor..local_start]));
        }
        spans.push(Span::styled(
            &line[local_start..local_end],
            Style::default().fg(Color::Cyan),
        ));
        cursor = local_end;
    }
    if cursor < line.len() {
        spans.push(Span::raw(&line[cursor..]));
    }
    spans
}
