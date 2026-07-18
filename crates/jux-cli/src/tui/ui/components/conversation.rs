use super::super::layout::ConversationLayout;
use super::super::text::{
    apply_text_selection, full_width_line, padded_full_width_lines, truncate_timeline_detail,
};
use super::super::theme::{CONVERSATION_PADDING, palette, panel_block};
use super::super::{RenderState, VirtualListState, VisibleItem};
use super::command_output;
use super::markdown::MarkdownRenderer;
use crate::tui::app::MessageRenderKey;
use crate::tui::ui::state::ConversationUiState;
use crate::tui::{
    Message, MessageRole, SelectionPanel, TimelineStatus, TuiCodeChangeResult, TuiRunStatus,
    TuiViewport,
};
use jux_core::{AssistantResponseItem, HumanInputKind, LlmUsage, StepPayload, TuiTheme};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Widget, Wrap,
};
use std::collections::HashSet;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAX_FILE_REFERENCE_ROWS: usize = 8;

fn input_line_count(state: &RenderState<'_>) -> u16 {
    u16::try_from(state.input_text().split('\n').count().max(1)).unwrap_or(u16::MAX)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConversationScroll {
    offset: u16,
    total_rows: u16,
    visible_rows: u16,
    command_toggle_rows: Vec<(u16, usize)>,
}

struct PreparedConversation<'a> {
    sections: &'a [Option<CachedConversationSection>],
    footer: ConversationContent,
    logical_offsets: &'a [usize],
    scroll: ConversationScroll,
    visible_sections: Vec<VisibleItem>,
}

#[derive(Default)]
pub(crate) struct ConversationPanel {
    pub(crate) ui_state: ConversationUiState,
    section_cache: ConversationSectionCache,
    virtual_list: VirtualListState,
    logical_offsets: Vec<usize>,
    scroll_from_bottom: u16,
    maximum_scroll: u16,
    scroll_initialized: bool,
    expanded_timeline_items: HashSet<String>,
    expansion_revision: u64,
}

#[derive(Default)]
struct ConversationSectionCache {
    entries: Vec<Option<CachedConversationSection>>,
}

struct CachedConversationSection {
    message_id: u64,
    revision: u64,
    selected: bool,
    content_width: u16,
    theme: TuiTheme,
    timeline: Vec<crate::tui::TimelineItem>,
    expansion_revision: u64,
    content: ConversationContent,
    command_toggle_rows: Vec<(u16, usize)>,
}

#[derive(Clone)]
struct CachedRenderedLine {
    line: Line<'static>,
    height: usize,
}

struct ConversationContent {
    lines: Vec<CachedRenderedLine>,
    logical_lines: usize,
    last_line_has_content: Option<bool>,
}

impl ConversationContent {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            logical_lines: 0,
            last_line_has_content: None,
        }
    }

    fn append_dynamic_line(
        &mut self,
        _state: &RenderState<'_>,
        line: Line<'_>,
        content_width: u16,
        _theme: TuiTheme,
    ) {
        let has_content = line_has_content(&line);
        let rendered = prepare_rendered_line(line, content_width);
        self.lines.push(rendered);
        self.logical_lines = self.logical_lines.saturating_add(1);
        self.last_line_has_content = Some(has_content);
    }

    fn append_dynamic_lines<'line>(
        &mut self,
        state: &RenderState<'_>,
        lines: impl IntoIterator<Item = Line<'line>>,
        content_width: u16,
        theme: TuiTheme,
    ) {
        for line in lines {
            self.append_dynamic_line(state, line, content_width, theme);
        }
    }

    fn append_spacing(&mut self, state: &RenderState<'_>, content_width: u16, theme: TuiTheme) {
        if self
            .last_line_has_content
            .is_none_or(|has_content| has_content)
        {
            self.append_dynamic_line(state, Line::default(), content_width, theme);
        }
    }
}

impl ConversationContent {
    fn total_rows(&self) -> usize {
        self.lines.iter().map(|line| line.height).sum()
    }
}

fn conversation_line_is_selected(state: &RenderState<'_>, line: usize) -> bool {
    let Some(selection) = state
        .text_selection()
        .filter(|selection| selection.panel == SelectionPanel::Conversation)
    else {
        return false;
    };
    let (start, end) = if (selection.anchor.line, selection.anchor.column)
        <= (selection.focus.line, selection.focus.column)
    {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    };
    line >= start.line && line <= end.line
}

impl ConversationSectionCache {
    #[allow(clippy::too_many_arguments)]
    fn entry_needs_rebuild(
        &mut self,
        index: usize,
        key: MessageRenderKey,
        selected: bool,
        content_width: u16,
        theme: TuiTheme,
        timeline: &[crate::tui::TimelineItem],
        expansion_revision: u64,
    ) -> bool {
        if self.entries.len() <= index {
            self.entries.resize_with(index.saturating_add(1), || None);
        }
        self.entries[index].as_ref().is_none_or(|cached| {
            cached.message_id != key.id
                || cached.revision != key.revision
                || cached.selected != selected
                || cached.content_width != content_width
                || cached.theme != theme
                || cached.timeline != timeline
                || cached.expansion_revision != expansion_revision
        })
    }

    fn truncate(&mut self, message_count: usize) {
        self.entries.truncate(message_count);
    }
}

fn prepare_rendered_line(line: Line<'_>, content_width: u16) -> CachedRenderedLine {
    let style = line.style;
    let line = Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect::<Vec<_>>(),
    )
    .style(style);
    let height = Paragraph::new(line.clone())
        .wrap(Wrap { trim: false })
        .line_count(content_width)
        .max(1);
    CachedRenderedLine { line, height }
}

fn render_message_lines(
    message: &Message,
    selected: bool,
    content_width: u16,
    theme: TuiTheme,
) -> Vec<Line<'static>> {
    match message.role {
        MessageRole::User => {
            let background = Style::default().bg(palette(theme).input);
            let mut lines = vec![full_width_line("", content_width, background)];
            lines.extend(message.content.split('\n').flat_map(|line| {
                let marker = if selected { "▶" } else { ">" };
                padded_full_width_lines(&format!("{marker} {line}"), content_width, background)
            }));
            lines.push(full_width_line("", content_width, background));
            lines
        }
        MessageRole::Assistant | MessageRole::Error => {
            let markdown_width = content_width.saturating_sub(6);
            MarkdownRenderer::new(markdown_width)
                .render(&message.content)
                .into_iter()
                .map(pad_assistant_line)
                .collect()
        }
    }
}

impl ConversationPanel {
    pub(crate) fn toggle_timeline_item(&mut self, id: &str) {
        if !self.expanded_timeline_items.remove(id) {
            self.expanded_timeline_items.insert(id.to_owned());
        }
        self.expansion_revision = self.expansion_revision.wrapping_add(1);
    }

    pub(crate) fn scroll_by(&mut self, delta: i32) {
        if delta >= 0 {
            self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(delta as u16);
        } else {
            self.scroll_from_bottom = self
                .scroll_from_bottom
                .saturating_sub(delta.unsigned_abs() as u16);
        }
        self.scroll_from_bottom = self.scroll_from_bottom.min(self.maximum_scroll);
        self.scroll_initialized = true;
    }

    pub(crate) fn scroll_to_top(&mut self) {
        self.scroll_from_bottom = self.maximum_scroll;
        self.scroll_initialized = true;
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom = 0;
        self.scroll_initialized = true;
    }

    pub(crate) fn scroll_from_bottom(&self) -> u16 {
        self.scroll_from_bottom
    }

    pub(crate) fn scroll_position_label(&self) -> &'static str {
        if self.scroll_from_bottom == 0 {
            "Bottom"
        } else if self.scroll_from_bottom >= self.maximum_scroll {
            "Top"
        } else {
            "History"
        }
    }

    pub(crate) fn command_toggle_at(
        &mut self,
        state: &RenderState<'_>,
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
        let layout = ConversationLayout::calculate(input_line_count(state), area);
        if column <= layout.history.x
            || column >= layout.history.right().saturating_sub(1)
            || row <= layout.history.y
            || row >= layout.history.bottom().saturating_sub(1)
        {
            return None;
        }
        let prepared = prompt_panel(
            self,
            state,
            layout.history.width.saturating_sub(2),
            layout.history.height.saturating_sub(2),
        );
        let visible_row = row.saturating_sub(layout.history.y.saturating_add(1));
        let content_row = prepared.scroll.offset.saturating_add(visible_row);
        prepared
            .scroll
            .command_toggle_rows
            .iter()
            .find_map(|(toggle_row, index)| (*toggle_row == content_row).then_some(*index))
    }
}

pub(in crate::tui::ui) fn render_conversation_history(
    panel: &mut ConversationPanel,
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    area: Rect,
) {
    let layout = ConversationLayout::calculate(input_line_count(state), area);
    let prepared = prompt_panel(
        panel,
        state,
        layout.history.width.saturating_sub(2),
        layout.history.height.saturating_sub(2),
    );
    let render_started = Instant::now();
    render_prepared_conversation(buffer, state, layout.history, &prepared);
    tracing::debug!(
        target: "jux::selection_perf",
        elapsed_us = %render_started.elapsed().as_micros(),
        "[DEBUG-selection-perf] conversation paragraph rendered"
    );
    render_conversation_scrollbar(buffer, state, layout.scrollbar, prepared.scroll);
}

pub(in crate::tui::ui) fn render_prompt_input(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    area: Rect,
    focused: bool,
) -> Option<Position> {
    render_input_area(
        buffer,
        state,
        ConversationLayout::calculate(input_line_count(state), area),
        focused,
    )
}

pub(in crate::tui::ui) fn render_conversation_status(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    area: Rect,
    scroll_position: &str,
) {
    let layout = ConversationLayout::calculate(input_line_count(state), area);
    render_status_bar(buffer, state, layout.status, scroll_position);
}

fn prompt_panel<'a>(
    panel: &'a mut ConversationPanel,
    state: &RenderState<'_>,
    content_width: u16,
    visible_rows: u16,
) -> PreparedConversation<'a> {
    let prompt_started = Instant::now();
    let mut footer = ConversationContent::new();
    let (timeline_start, timeline) = timeline_items_at(state, 0);
    let prelude_key = MessageRenderKey { id: 0, revision: 0 };
    if panel.section_cache.entry_needs_rebuild(
        0,
        prelude_key,
        false,
        content_width,
        state.theme(),
        timeline,
        panel.expansion_revision,
    ) {
        panel.section_cache.entries[0] = Some(build_timeline_section(
            state,
            prelude_key,
            false,
            content_width,
            timeline_start,
            timeline,
            &panel.expanded_timeline_items,
            panel.expansion_revision,
        ));
    }
    for (message_index, message) in state.messages().iter().enumerate() {
        let cache_index = message_index.saturating_add(1);
        let key = state.message_render_key(message_index);
        let selected =
            message.role == MessageRole::User && state.selected_message() == Some(message_index);
        let (timeline_start, timeline) = timeline_items_at(state, message_index.saturating_add(1));
        if panel.section_cache.entry_needs_rebuild(
            cache_index,
            key,
            selected,
            content_width,
            state.theme(),
            timeline,
            panel.expansion_revision,
        ) {
            panel.section_cache.entries[cache_index] = Some(build_message_section(
                state,
                message,
                key,
                selected,
                content_width,
                timeline_start,
                timeline,
                &panel.expanded_timeline_items,
                panel.expansion_revision,
            ));
        }
    }
    panel
        .section_cache
        .truncate(state.messages().len().saturating_add(1));
    if state.run_status() == TuiRunStatus::Running {
        footer.append_spacing(state, content_width, state.theme());
        let (indicator, color) = generating_frame();
        let (input_tokens, output_tokens) = state.estimated_token_usage();
        footer.append_dynamic_line(
            state,
            Line::styled(
                format!(
                    "   Generating {indicator} · ~{} in / ~{} out",
                    format_compact_number(input_tokens),
                    format_compact_number(output_tokens)
                ),
                Style::default().fg(color),
            ),
            content_width,
            state.theme(),
        );
    }
    if let Some(request) = state.pending_human_input() {
        let title = match request.kind {
            HumanInputKind::Clarification => "Input required",
            HumanInputKind::Confirmation => "Confirmation required",
        };
        footer.append_dynamic_line(
            state,
            Line::styled(title, Style::default().fg(Color::Yellow)),
            content_width,
            state.theme(),
        );
        footer.append_dynamic_line(
            state,
            Line::from(request.prompt.as_str()),
            content_width,
            state.theme(),
        );
        if let Some(reason) = &request.reason {
            footer.append_dynamic_line(
                state,
                Line::from(reason.as_str()),
                content_width,
                state.theme(),
            );
        }
        for (index, option) in request.options.iter().enumerate() {
            let marker = if index == state.selected_human_option() {
                ">"
            } else {
                " "
            };
            footer.append_dynamic_line(
                state,
                Line::from(format!("{marker} {}  {}", option.id, option.label)),
                content_width,
                state.theme(),
            );
        }
        if let Some(error) = state.human_input_error() {
            footer.append_dynamic_line(
                state,
                Line::styled(error, Style::default().fg(Color::Red)),
                content_width,
                state.theme(),
            );
        }
        footer.append_dynamic_line(state, Line::default(), content_width, state.theme());
    }
    if let Some(review) = state.code_change_review() {
        footer.append_dynamic_line(
            state,
            Line::styled(
                format!("Plan: {}", review.proposal.plan.summary),
                Style::default().fg(Color::Yellow),
            ),
            content_width,
            state.theme(),
        );
        for item in &review.proposal.plan.items {
            footer.append_dynamic_line(
                state,
                Line::from(format!("- {item}")),
                content_width,
                state.theme(),
            );
        }
        footer.append_dynamic_line(
            state,
            Line::from(format!("Policy: {:?}", review.proposal.policy)),
            content_width,
            state.theme(),
        );
        footer.append_dynamic_line(
            state,
            Line::from(format!("Review: {:?}", review.status)),
            content_width,
            state.theme(),
        );
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
            footer.append_dynamic_line(state, Line::from(message), content_width, state.theme());
        }
        for warning in &review.proposal.warnings {
            footer.append_dynamic_line(
                state,
                Line::styled(
                    format!(
                        "Risk [{:?}] {}: {}",
                        warning.level,
                        warning.path.as_str(),
                        warning.reason
                    ),
                    Style::default().fg(Color::Red),
                ),
                content_width,
                state.theme(),
            );
        }
        for (index, file) in review.proposal.files.iter().enumerate() {
            let marker = if index == state.selected_changed_file() {
                ">"
            } else {
                " "
            };
            footer.append_dynamic_line(
                state,
                Line::from(format!("{marker} {}", file.path.as_str())),
                content_width,
                state.theme(),
            );
        }
        if let Some(file) = review.proposal.files.get(state.selected_changed_file()) {
            footer.append_dynamic_lines(
                state,
                file.diff.lines().map(Line::from),
                content_width,
                state.theme(),
            );
        }
        footer.append_dynamic_line(state, Line::default(), content_width, state.theme());
    }
    let assembled_elapsed = prompt_started.elapsed();
    panel.logical_offsets.clear();
    let mut logical_offset = 0usize;
    let mut content_row = 0usize;
    let mut command_toggle_rows = Vec::new();
    let mut section_heights = Vec::with_capacity(panel.section_cache.entries.len() + 1);
    for section in panel.section_cache.entries.iter().flatten() {
        panel.logical_offsets.push(logical_offset);
        logical_offset = logical_offset.saturating_add(section.content.logical_lines);
        section_heights.push(section.content.total_rows());
        command_toggle_rows.extend(section.command_toggle_rows.iter().map(|(row, index)| {
            (
                u16::try_from(content_row.saturating_add(usize::from(*row))).unwrap_or(u16::MAX),
                *index,
            )
        }));
        content_row = content_row.saturating_add(section.content.total_rows());
    }
    panel.logical_offsets.push(logical_offset);
    section_heights.push(footer.total_rows().max(1));
    let total_rows = section_heights
        .iter()
        .sum::<usize>()
        .saturating_add(usize::from(CONVERSATION_PADDING.saturating_mul(2)));
    let maximum = total_rows.saturating_sub(usize::from(visible_rows));
    if !panel.scroll_initialized {
        panel.scroll_from_bottom = state.conversation_scroll_from_bottom();
        panel.scroll_initialized = true;
    }
    panel.maximum_scroll = u16::try_from(maximum).unwrap_or(u16::MAX);
    panel.scroll_from_bottom = panel.scroll_from_bottom.min(panel.maximum_scroll);
    let offset = maximum.saturating_sub(usize::from(panel.scroll_from_bottom).min(maximum));
    let scroll = ConversationScroll {
        offset: u16::try_from(offset).unwrap_or(u16::MAX),
        total_rows: u16::try_from(total_rows).unwrap_or(u16::MAX),
        visible_rows,
        command_toggle_rows,
    };
    panel.virtual_list.use_cached_measurements(
        content_width,
        visible_rows,
        section_heights,
        offset,
        panel.scroll_from_bottom == 0,
    );
    let visible_sections = panel
        .virtual_list
        .visible_items(Rect::new(0, 0, content_width, visible_rows), 0);
    tracing::debug!(
        target: "jux::selection_perf",
        assembled_us = %assembled_elapsed.as_micros(),
        line_count_us = 0,
        total_us = %prompt_started.elapsed().as_micros(),
        total_rows,
        expanded_timeline_items = panel.expanded_timeline_items.len(),
        "[DEBUG-selection-perf] conversation paragraph prepared"
    );
    PreparedConversation {
        sections: &panel.section_cache.entries,
        footer,
        logical_offsets: &panel.logical_offsets,
        scroll,
        visible_sections,
    }
}

fn timeline_items_at<'a>(
    state: &'a RenderState<'a>,
    message_count: usize,
) -> (usize, &'a [crate::tui::TimelineItem]) {
    let timeline = state.timeline();
    let start = timeline.partition_point(|item| item.message_count < message_count);
    let end = timeline.partition_point(|item| item.message_count <= message_count);
    (start, &timeline[start..end])
}

#[allow(clippy::too_many_arguments)]
fn build_timeline_section(
    state: &RenderState<'_>,
    key: MessageRenderKey,
    selected: bool,
    content_width: u16,
    timeline_start: usize,
    timeline: &[crate::tui::TimelineItem],
    expanded_timeline_items: &HashSet<String>,
    expansion_revision: u64,
) -> CachedConversationSection {
    let mut content = ConversationContent::new();
    let mut command_toggle_rows = Vec::new();
    append_timeline_items(
        state,
        content_width,
        state.theme(),
        timeline_start,
        timeline,
        expanded_timeline_items,
        &mut content,
        &mut command_toggle_rows,
    );
    CachedConversationSection {
        message_id: key.id,
        revision: key.revision,
        selected,
        content_width,
        theme: state.theme(),
        timeline: timeline.to_vec(),
        expansion_revision,
        content,
        command_toggle_rows,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_message_section(
    state: &RenderState<'_>,
    message: &Message,
    key: MessageRenderKey,
    selected: bool,
    content_width: u16,
    timeline_start: usize,
    timeline: &[crate::tui::TimelineItem],
    expanded_timeline_items: &HashSet<String>,
    expansion_revision: u64,
) -> CachedConversationSection {
    let started = Instant::now();
    let mut section = build_timeline_section(
        state,
        key,
        selected,
        content_width,
        timeline_start,
        &[],
        expanded_timeline_items,
        expansion_revision,
    );
    match message.role {
        MessageRole::User => section.content.append_dynamic_lines(
            state,
            render_message_lines(message, selected, content_width, state.theme()),
            content_width,
            state.theme(),
        ),
        MessageRole::Assistant | MessageRole::Error => {
            section
                .content
                .append_spacing(state, content_width, state.theme());
            if message.role == MessageRole::Error {
                section.content.append_dynamic_line(
                    state,
                    Line::styled("Error", Style::default().fg(Color::Red)),
                    content_width,
                    state.theme(),
                );
            }
            section.content.append_dynamic_lines(
                state,
                render_message_lines(message, selected, content_width, state.theme()),
                content_width,
                state.theme(),
            );
            section
                .content
                .append_spacing(state, content_width, state.theme());
        }
    }
    append_timeline_items(
        state,
        content_width,
        state.theme(),
        timeline_start,
        timeline,
        expanded_timeline_items,
        &mut section.content,
        &mut section.command_toggle_rows,
    );
    section.timeline = timeline.to_vec();
    if let Some(metadata) = response_metadata_for_message(state, message) {
        section
            .content
            .append_spacing(state, content_width, state.theme());
        section.content.append_dynamic_line(
            state,
            Line::styled(
                format!("\u{00a0}\u{00a0}\u{00a0}{metadata}"),
                Style::default().fg(Color::DarkGray),
            ),
            content_width,
            state.theme(),
        );
        section
            .content
            .append_spacing(state, content_width, state.theme());
    }
    tracing::debug!(
        target: "jux::selection_perf",
        elapsed_us = %started.elapsed().as_micros(),
        "[DEBUG-selection-perf] conversation section cache rebuilt"
    );
    section
}

fn line_has_content(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .any(|span| !span.content.as_ref().is_empty())
}

#[allow(clippy::too_many_arguments)]
fn append_timeline_items(
    state: &RenderState<'_>,
    content_width: u16,
    theme: TuiTheme,
    timeline_start: usize,
    timeline: &[crate::tui::TimelineItem],
    expanded_timeline_items: &HashSet<String>,
    content: &mut ConversationContent,
    command_toggle_rows: &mut Vec<(u16, usize)>,
) {
    if timeline.iter().any(|item| item.command.is_some()) {
        content.append_spacing(state, content_width, theme);
    }
    for (local_index, item) in timeline.iter().enumerate() {
        let expanded = expanded_timeline_items.contains(&item.id);
        let timeline_index = timeline_start.saturating_add(local_index);
        if let Some(command) = &item.command {
            command_toggle_rows.push((
                u16::try_from(content.total_rows()).unwrap_or(u16::MAX),
                timeline_index,
            ));
            content.append_dynamic_lines(
                state,
                command_output::render(
                    command,
                    item.status,
                    item.detail.as_deref(),
                    expanded,
                    content_width,
                ),
                content_width,
                theme,
            );
            let next_is_command = timeline
                .get(local_index.saturating_add(1))
                .is_some_and(|next| next.command.is_some());
            if !next_is_command {
                content.append_dynamic_line(state, Line::default(), content_width, theme);
            }
            continue;
        }
        let status = match item.status {
            TimelineStatus::Running => "Running",
            TimelineStatus::Output => "Output",
            TimelineStatus::Completed => "Completed",
            TimelineStatus::Failed => "Failed",
        };
        content.append_dynamic_line(
            state,
            Line::from(format!("{}  {status}", item.label)),
            content_width,
            theme,
        );
        if let Some(detail) = &item.detail {
            content.append_dynamic_line(state, Line::from(detail.as_str()), content_width, theme);
        }
        if expanded {
            if let Some(arguments) = &item.arguments {
                content.append_dynamic_line(state, Line::from("Arguments:"), content_width, theme);
                let arguments = truncate_timeline_detail(arguments);
                content.append_dynamic_lines(
                    state,
                    arguments.lines().map(|line| Line::from(line.to_owned())),
                    content_width,
                    theme,
                );
            }
            if let Some(output) = &item.output {
                content.append_dynamic_line(state, Line::from("Output:"), content_width, theme);
                let output = truncate_timeline_detail(output);
                content.append_dynamic_lines(
                    state,
                    output.lines().map(|line| Line::from(line.to_owned())),
                    content_width,
                    theme,
                );
            }
        } else if let Some(output) = &item.output {
            let summary = output.split_whitespace().collect::<Vec<_>>().join(" ");
            content.append_dynamic_line(
                state,
                Line::from(truncate_timeline_detail(&summary)),
                content_width,
                theme,
            );
        }
    }
}

fn pad_assistant_line(mut line: Line<'_>) -> Line<'_> {
    line.spans.insert(0, Span::raw("\u{00a0}\u{00a0}\u{00a0}"));
    line.spans.push(Span::raw("\u{00a0}\u{00a0}\u{00a0}"));
    line
}

fn response_metadata_for_message(
    state: &RenderState<'_>,
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
        .map(|run| run.updated_at.saturating_sub(run.created_at))
        .or_else(|| {
            (state.run_id() == Some(step.id.run_id().to_string().as_str()))
                .then(|| state.run_elapsed_millis())
                .flatten()
        });
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

fn render_prepared_conversation(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    area: Rect,
    prepared: &PreparedConversation<'_>,
) {
    let background = palette(state.theme()).conversation;
    panel_block(background, CONVERSATION_PADDING).render(area, buffer);
    let inner = Rect::new(
        area.x.saturating_add(CONVERSATION_PADDING),
        area.y.saturating_add(CONVERSATION_PADDING),
        area.width
            .saturating_sub(CONVERSATION_PADDING.saturating_mul(2)),
        area.height
            .saturating_sub(CONVERSATION_PADDING.saturating_mul(2)),
    );
    for visible in &prepared.visible_sections {
        let content = prepared
            .sections
            .get(visible.index)
            .and_then(Option::as_ref)
            .map(|section| &section.content)
            .unwrap_or(&prepared.footer);
        render_visible_section(
            buffer,
            state,
            inner,
            background,
            visible,
            content,
            prepared.logical_offsets[visible.index],
        );
    }
}

fn render_visible_section(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    viewport: Rect,
    background: Color,
    visible: &VisibleItem,
    content: &ConversationContent,
    logical_offset: usize,
) {
    let mut line_top = 0usize;
    let visible_bottom =
        usize::from(visible.area.y).saturating_add(usize::from(visible.area.height));
    for (line_index, rendered) in content.lines.iter().enumerate() {
        let line_bottom = line_top.saturating_add(rendered.height);
        if line_bottom <= visible.skip_top {
            line_top = line_bottom;
            continue;
        }
        let viewport_y =
            usize::from(visible.area.y).saturating_add(line_top.saturating_sub(visible.skip_top));
        if viewport_y >= visible_bottom {
            break;
        }
        let logical_line = logical_offset.saturating_add(line_index);
        let line = if conversation_line_is_selected(state, logical_line) {
            apply_text_selection(
                state,
                SelectionPanel::Conversation,
                logical_line,
                vec![rendered.line.clone()],
            )
            .into_iter()
            .next()
            .unwrap_or_default()
        } else {
            rendered.line.clone()
        };
        let skip_top = visible.skip_top.saturating_sub(line_top);
        let height = rendered
            .height
            .saturating_sub(skip_top)
            .min(visible_bottom.saturating_sub(viewport_y));
        let item_area = Rect::new(
            viewport.x,
            viewport
                .y
                .saturating_add(u16::try_from(viewport_y).unwrap_or(u16::MAX)),
            viewport.width,
            u16::try_from(height).unwrap_or(u16::MAX),
        );
        Paragraph::new(line)
            .style(Style::default().bg(background))
            .wrap(Wrap { trim: false })
            .scroll((u16::try_from(skip_top).unwrap_or(u16::MAX), 0))
            .render(item_area, buffer);
        line_top = line_bottom;
    }
}

fn render_conversation_scrollbar(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
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
    StatefulWidget::render(scrollbar, area, buffer, &mut scrollbar_state);
}

fn render_input_area(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    layout: ConversationLayout,
    active: bool,
) -> Option<Position> {
    let area = layout.input;
    if area.is_empty() {
        return None;
    }
    let height = area.height;
    let popup_bounds = Rect::new(
        area.x,
        layout.history.y.saturating_add(1),
        area.width,
        area.y.saturating_sub(layout.history.y.saturating_add(1)),
    );
    render_slash_command_popup(buffer, state, popup_bounds, area.y);
    render_inline_skill_popup(buffer, state, popup_bounds, area.y);
    render_file_reference_popup(buffer, state, popup_bounds, area.y);
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
    Paragraph::new(lines)
        .style(style)
        .scroll((vertical_scroll, 0))
        .wrap(Wrap { trim: false })
        .render(area, buffer);
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
        return Some(Position::new(cursor_x, cursor_y));
    }
    None
}

fn render_inline_skill_popup(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
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
    Paragraph::new(lines)
        .block(
            Block::default()
                .style(Style::default().bg(palette(state.theme()).popup))
                .padding(Padding::uniform(1)),
        )
        .style(Style::default().bg(palette(state.theme()).popup))
        .render(area, buffer);
}

fn render_file_reference_popup(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    input_bounds: Rect,
    input_top: u16,
) {
    let suggestion_count = state.file_reference_suggestion_count();
    if suggestion_count == 0 {
        return;
    }
    let available_height = input_top.saturating_sub(input_bounds.y);
    let visible_count = suggestion_count
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
        .min(suggestion_count.saturating_sub(1));
    let window_start = selected
        .saturating_add(1)
        .saturating_sub(visible_count)
        .min(suggestion_count.saturating_sub(visible_count));
    let row_width = usize::from(input_bounds.width.saturating_sub(2));
    let lines = (window_start..window_start + visible_count)
        .filter_map(|index| {
            let path = state.file_reference_suggestion(index)?;
            let style = if index == selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };
            Some(Line::styled(
                format!("@{path}")
                    .chars()
                    .take(row_width)
                    .collect::<String>(),
                style,
            ))
        })
        .collect::<Vec<_>>();
    let area = Rect::new(
        input_bounds.x,
        input_top.saturating_sub(height),
        input_bounds.width,
        height,
    );
    let background = palette(state.theme()).popup;
    Clear.render(area, buffer);
    Paragraph::new(lines)
        .block(
            Block::default()
                .style(Style::default().bg(background))
                .padding(Padding::uniform(1)),
        )
        .style(Style::default().bg(background))
        .render(area, buffer);
}

fn render_status_bar(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
    area: Rect,
    scroll_position: &str,
) {
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
    let text = format!("{text} | {scroll_position}");
    let style = Style::default()
        .fg(Color::Gray)
        .bg(palette(state.theme()).status);
    let aligned_text = format!("   {text}");
    Paragraph::new(full_width_line(&aligned_text, area.width, style))
        .style(style)
        .render(area, buffer);
}

fn render_slash_command_popup(
    buffer: &mut Buffer,
    state: &RenderState<'_>,
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
    Paragraph::new(lines)
        .block(
            Block::default()
                .style(Style::default().bg(palette(state.theme()).popup))
                .padding(Padding::uniform(1)),
        )
        .style(Style::default().bg(palette(state.theme()).popup))
        .render(area, buffer);
}

fn input_lines<'a>(state: &'a RenderState<'a>) -> Vec<Line<'a>> {
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
    state: &RenderState<'_>,
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

#[cfg(test)]
mod tests {
    use super::{ConversationSectionCache, RenderState, build_message_section};
    use crate::tui::app::MessageRenderKey;
    use crate::tui::{AppModel, Message, MessageRole};
    use jux_core::TuiTheme;
    use std::collections::HashSet;

    #[test]
    fn conversation_section_cache_reuses_unchanged_message_lines() {
        let model = AppModel::new(".");
        let state = RenderState::for_test(&model);
        let mut cache = ConversationSectionCache::default();
        let message = Message {
            role: MessageRole::Assistant,
            content: "```rust\nlet value = 1;\n```".to_owned(),
        };
        let key = MessageRenderKey { id: 1, revision: 0 };
        cache.entries.push(Some(build_message_section(
            &state,
            &message,
            key,
            false,
            80,
            0,
            &[],
            &HashSet::new(),
            0,
        )));
        let first = cache.entries[0]
            .as_ref()
            .expect("cached section")
            .content
            .lines
            .as_ptr();

        assert!(!cache.entry_needs_rebuild(0, key, false, 80, TuiTheme::Dark, &[], 0));
        assert_eq!(
            first,
            cache.entries[0]
                .as_ref()
                .expect("cached section")
                .content
                .lines
                .as_ptr()
        );
    }

    #[test]
    fn conversation_section_cache_invalidates_message_revision() {
        let mut cache = ConversationSectionCache::default();
        let key = MessageRenderKey { id: 1, revision: 0 };
        let model = crate::tui::AppModel::new(".");
        let state = RenderState::for_test(&model);

        assert!(cache.entry_needs_rebuild(0, key, false, 80, TuiTheme::Dark, &[], 0));
        cache.entries[0] = Some(build_message_section(
            &state,
            &Message {
                role: MessageRole::Assistant,
                content: "first".to_owned(),
            },
            key,
            false,
            80,
            0,
            &[],
            &HashSet::new(),
            0,
        ));
        assert!(cache.entry_needs_rebuild(
            0,
            MessageRenderKey { id: 1, revision: 1 },
            false,
            80,
            TuiTheme::Dark,
            &[],
            0,
        ));
    }

    #[test]
    fn conversation_section_cache_invalidates_width_and_theme() {
        let model = AppModel::new(".");
        let state = RenderState::for_test(&model);
        let mut cache = ConversationSectionCache::default();
        let message = Message {
            role: MessageRole::User,
            content: "request".to_owned(),
        };
        let key = MessageRenderKey { id: 1, revision: 0 };
        cache.entries.push(Some(build_message_section(
            &state,
            &message,
            key,
            false,
            80,
            0,
            &[],
            &HashSet::new(),
            0,
        )));

        assert!(cache.entry_needs_rebuild(0, key, false, 40, TuiTheme::Dark, &[], 0));
        assert!(cache.entry_needs_rebuild(0, key, false, 80, TuiTheme::HighContrast, &[], 0,));
    }
}
