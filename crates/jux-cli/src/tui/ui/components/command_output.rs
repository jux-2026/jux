use super::super::text::truncate_timeline_detail;
use crate::tui::{TimelineStatus, TuiCommandExecution};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const COMMAND_BACKGROUND: Color = Color::Rgb(20, 28, 36);
const EXPANDED_COMMAND_BACKGROUND: Color = Color::Rgb(24, 38, 48);
const OUTPUT_BACKGROUND: Color = Color::Rgb(16, 22, 28);

pub(super) fn render(
    command: &TuiCommandExecution,
    status: TimelineStatus,
    detail: Option<&str>,
    expanded: bool,
    maximum_width: u16,
) -> Vec<Line<'static>> {
    let width = usize::from(maximum_width.max(1));
    let mut lines = vec![command_line(command, status, expanded, width)];
    if command_failed(command, status) {
        let error = detail
            .or_else(|| command.stderr.lines().find(|line| !line.trim().is_empty()))
            .unwrap_or("Command failed");
        lines.push(background_line(
            vec![Span::styled(
                format!("  {}", truncate_timeline_detail(error)),
                Style::default().fg(Color::Red),
            )],
            width,
            OUTPUT_BACKGROUND,
        ));
    }
    if expanded {
        lines.push(section_title("stdout", width));
        lines.extend(output_lines(
            &command.stdout,
            Color::Gray,
            "No standard output",
            width,
        ));
        lines.push(section_title("stderr", width));
        lines.extend(output_lines(
            &command.stderr,
            Color::LightRed,
            "No standard error",
            width,
        ));
    }
    lines
}

fn command_line(
    command: &TuiCommandExecution,
    status: TimelineStatus,
    expanded: bool,
    width: usize,
) -> Line<'static> {
    let icon = if expanded { "▼" } else { "▶" };
    let icon_color = if command_failed(command, status) {
        Color::Red
    } else if command_succeeded(command, status) {
        Color::Green
    } else {
        running_color()
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{icon} "),
            Style::default().fg(icon_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("$ ", Style::default().fg(Color::Cyan)),
        Span::styled(
            command.program.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    for argument in &command.args {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            quote_argument(argument),
            Style::default().fg(Color::LightBlue),
        ));
    }
    let background = if expanded {
        EXPANDED_COMMAND_BACKGROUND
    } else {
        COMMAND_BACKGROUND
    };
    background_line(spans, width, background)
}

fn command_failed(command: &TuiCommandExecution, status: TimelineStatus) -> bool {
    command.success == Some(false) || status == TimelineStatus::Failed
}

fn command_succeeded(command: &TuiCommandExecution, status: TimelineStatus) -> bool {
    command.success == Some(true)
        || (command.success.is_none() && status == TimelineStatus::Completed)
}

fn running_color() -> Color {
    const FRAMES: [Color; 6] = [
        Color::Rgb(70, 130, 180),
        Color::Rgb(64, 170, 190),
        Color::Rgb(80, 200, 170),
        Color::Rgb(190, 210, 90),
        Color::Rgb(230, 180, 70),
        Color::Rgb(170, 110, 190),
    ];
    let frame = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() / 120);
    FRAMES[(frame as usize) % FRAMES.len()]
}

fn section_title(title: &str, width: usize) -> Line<'static> {
    background_line(
        vec![Span::styled(
            format!("  {title}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )],
        width,
        OUTPUT_BACKGROUND,
    )
}

fn output_lines(output: &str, color: Color, empty_label: &str, width: usize) -> Vec<Line<'static>> {
    if output.is_empty() {
        return vec![background_line(
            vec![Span::styled(
                format!("    {empty_label}"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )],
            width,
            OUTPUT_BACKGROUND,
        )];
    }
    let content_width = width.saturating_sub(4).max(1);
    output
        .split('\n')
        .flat_map(|line| wrap_output_line(line, content_width))
        .map(|line| {
            background_line(
                vec![Span::styled(
                    format!("    {line}"),
                    Style::default().fg(color),
                )],
                width,
                OUTPUT_BACKGROUND,
            )
        })
        .collect()
}

fn wrap_output_line(line: &str, maximum_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for character in line.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or_default();
        if !current.is_empty() && current_width + character_width > maximum_width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(character);
        current_width += character_width;
    }
    lines.push(current);
    lines
}

fn background_line(spans: Vec<Span<'static>>, width: usize, background: Color) -> Line<'static> {
    let spans = spans
        .into_iter()
        .map(|span| {
            Span::styled(
                span.content,
                span.style.patch(Style::default().bg(background)),
            )
        })
        .collect();
    let mut spans = truncate_spans(spans, width);
    let rendered_width = spans_width(&spans);
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(rendered_width)),
        Style::default().bg(background),
    ));
    Line::from(spans).style(Style::default().bg(background))
}

fn quote_argument(argument: &str) -> String {
    if argument
        .chars()
        .all(|character| character.is_alphanumeric() || "-_=./:".contains(character))
    {
        argument.to_owned()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn truncate_spans(spans: Vec<Span<'static>>, maximum_width: usize) -> Vec<Span<'static>> {
    let mut remaining = maximum_width;
    let mut rendered = Vec::new();
    for span in spans {
        let mut content = String::new();
        for character in span.content.chars() {
            let width = UnicodeWidthChar::width(character).unwrap_or_default();
            if width > remaining {
                break;
            }
            content.push(character);
            remaining -= width;
        }
        if !content.is_empty() {
            rendered.push(Span::styled(content, span.style));
        }
        if remaining == 0 {
            break;
        }
    }
    rendered
}
