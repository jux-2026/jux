use super::RenderState;
use super::theme::selection_style;
use crate::tui::SelectionPanel;
use crate::tui::TextSelectionPoint;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use std::time::Instant;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_TIMELINE_DETAIL_CHARS: usize = 80;
const TAB_WIDTH: usize = 4;

pub(super) fn expand_tabs(text: &str, initial_column: usize) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut column = initial_column;
    for character in text.chars() {
        if character == '\t' {
            let spaces = TAB_WIDTH.saturating_sub(column % TAB_WIDTH).max(1);
            expanded.push_str(&" ".repeat(spaces));
            column = column.saturating_add(spaces);
        } else {
            expanded.push(character);
            column = column.saturating_add(UnicodeWidthChar::width(character).unwrap_or_default());
        }
    }
    expanded
}

pub(super) fn full_width_line(text: &str, width: u16, style: Style) -> Line<'static> {
    let text = expand_tabs(text, 0);
    let width = usize::from(width);
    let text_width = UnicodeWidthStr::width(text.as_str());
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

pub(super) fn padded_full_width_lines(text: &str, width: u16, style: Style) -> Vec<Line<'static>> {
    let content_width = usize::from(width.saturating_sub(2));
    if content_width == 0 {
        return vec![full_width_line("", width, style)];
    }
    let text = expand_tabs(text, 1);
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

pub(super) fn truncate_timeline_detail(content: &str) -> String {
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

pub(super) fn apply_text_selection<'a>(
    state: &RenderState<'_>,
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
    let started = Instant::now();
    let input_lines = lines.len();
    let (start, end) = ordered_points(selection.anchor, selection.focus);
    let selected_lines = lines
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
        .collect::<Vec<_>>();
    tracing::debug!(
        target: "jux::selection_perf",
        ?panel,
        input_lines,
        selected_start_line = start.line,
        selected_end_line = end.line,
        elapsed_us = %started.elapsed().as_micros(),
        "[DEBUG-selection-perf] selection style applied"
    );
    selected_lines
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
