use std::sync::LazyLock;
use std::time::Instant;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use two_face::re_exports::syntect::easy::HighlightLines;
use two_face::re_exports::syntect::highlighting::FontStyle;
use two_face::re_exports::syntect::parsing::{SyntaxReference, SyntaxSet};
use two_face::re_exports::syntect::util::LinesWithEndings;
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CODE_BACKGROUND: Color = Color::Rgb(24, 28, 34);
const INLINE_CODE_BACKGROUND: Color = Color::Rgb(38, 44, 52);
static CODE_SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static CODE_THEMES: LazyLock<EmbeddedLazyThemeSet> = LazyLock::new(two_face::theme::extra);

pub(super) struct MarkdownRenderer {
    maximum_width: usize,
}

impl MarkdownRenderer {
    pub(super) fn new(maximum_width: u16) -> Self {
        Self {
            maximum_width: usize::from(maximum_width.max(1)),
        }
    }

    pub(super) fn render(&self, markdown: &str) -> Vec<Line<'static>> {
        let options =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let mut state = RenderState::new(self.maximum_width);
        for event in Parser::new_ext(markdown, options) {
            state.handle(event);
        }
        state.finish()
    }
}

struct RenderState {
    maximum_width: usize,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    code_block: Option<CodeBlock>,
    table: Option<TableState>,
}

impl RenderState {
    fn new(maximum_width: usize) -> Self {
        Self {
            maximum_width,
            lines: Vec::new(),
            current: Vec::new(),
            styles: vec![Style::default()],
            lists: Vec::new(),
            quote_depth: 0,
            code_block: None,
            table: None,
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(text.as_ref()),
            Event::Code(code) => self.current.push(Span::styled(
                code.into_string(),
                self.style().patch(
                    Style::default()
                        .fg(Color::Yellow)
                        .bg(INLINE_CODE_BACKGROUND),
                ),
            )),
            Event::SoftBreak => self.current.push(Span::raw(" ")),
            Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.flush_line();
                self.lines.push(Line::styled(
                    "─".repeat(self.maximum_width),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Event::TaskListMarker(checked) => {
                self.current
                    .push(Span::raw(if checked { "[x] " } else { "[ ] " }));
            }
            Event::Html(html) | Event::InlineHtml(html) => self.text(html.as_ref()),
            Event::FootnoteReference(reference) => {
                self.current.push(Span::styled(
                    format!("[^{reference}]"),
                    self.style().add_modifier(Modifier::DIM),
                ));
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => self.text(math.as_ref()),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { .. } => self.push_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Tag::BlockQuote(_) => {
                self.quote_depth += 1;
                self.current.push(Span::styled(
                    format!("{} ", "│".repeat(self.quote_depth)),
                    Style::default().fg(Color::DarkGray),
                ));
                self.push_style(
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::ITALIC),
                );
            }
            Tag::CodeBlock(kind) => {
                self.flush_line();
                let language = match kind {
                    CodeBlockKind::Fenced(language) if !language.trim().is_empty() => {
                        language.into_string()
                    }
                    _ => "code".to_owned(),
                };
                self.code_block = Some(CodeBlock {
                    language,
                    source: String::new(),
                });
            }
            Tag::List(start) => self.lists.push(start),
            Tag::Item => {
                let indent = "  ".repeat(self.lists.len().saturating_sub(1));
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "• ".to_owned(),
                };
                self.current.push(Span::raw(format!("{indent}{marker}")));
            }
            Tag::Emphasis => self.push_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { .. } => self.push_style(
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Tag::Table(_) => self.table = Some(TableState::default()),
            Tag::TableHead => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.current_row.clear();
                }
            }
            Tag::TableCell => self.current.clear(),
            Tag::Image { dest_url, .. } => self.current.push(Span::styled(
                format!("[image: {dest_url}]"),
                Style::default().fg(Color::Magenta),
            )),
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList => {}
            Tag::DefinitionListTitle => {
                self.push_style(Style::default().add_modifier(Modifier::BOLD))
            }
            Tag::DefinitionListDefinition => self.current.push(Span::raw("  ")),
            Tag::Superscript | Tag::Subscript => {
                self.push_style(Style::default().add_modifier(Modifier::DIM));
            }
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Item => self.flush_line(),
            TagEnd::Heading(_) => {
                self.pop_style();
                self.flush_line();
            }
            TagEnd::BlockQuote(_) => {
                self.pop_style();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.flush_line();
            }
            TagEnd::CodeBlock => self.finish_code_block(),
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::DefinitionListTitle
            | TagEnd::Superscript
            | TagEnd::Subscript => self.pop_style(),
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    table.current_row.push(std::mem::take(&mut self.current));
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(std::mem::take(&mut table.current_row));
                }
            }
            TagEnd::TableHead => {
                self.pop_style();
                if let Some(table) = &mut self.table
                    && !table.current_row.is_empty()
                {
                    table.rows.push(std::mem::take(&mut table.current_row));
                }
            }
            TagEnd::Table => self.finish_table(),
            TagEnd::Image
            | TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListDefinition => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = &mut self.code_block {
            code.source.push_str(text);
        } else {
            self.current
                .push(Span::styled(text.to_owned(), self.style()));
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, style: Style) {
        self.styles.push(self.style().patch(style));
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn flush_line(&mut self) {
        if !self.current.is_empty() {
            self.lines
                .push(Line::from(std::mem::take(&mut self.current)));
        }
    }

    fn finish_code_block(&mut self) {
        if let Some(code) = self.code_block.take() {
            self.lines.extend(code.render(self.maximum_width));
        }
    }

    fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        self.flush_line();
        self.lines.extend(table.render(self.maximum_width));
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.finish_code_block();
        self.finish_table();
        self.flush_line();
        self.lines
    }
}

struct CodeBlock {
    language: String,
    source: String,
}

impl CodeBlock {
    fn render(&self, maximum_width: usize) -> Vec<Line<'static>> {
        let started = Instant::now();
        let Some(syntax) = syntax_for_language(&self.language) else {
            return self.render_plain_text(maximum_width);
        };
        let theme = CODE_THEMES.get(EmbeddedThemeName::Nord);
        let mut highlighter = HighlightLines::new(syntax, theme);
        let source = self.source.trim_end_matches('\n');
        let mut lines = Vec::new();
        for source_line in LinesWithEndings::from(source) {
            let Ok(regions) = highlighter.highlight_line(source_line, &CODE_SYNTAXES) else {
                return self.render_plain_text(maximum_width);
            };
            let mut spans = vec![Span::styled(
                " ",
                code_style(Color::White, FontStyle::empty()),
            )];
            spans.extend(regions.into_iter().filter_map(|(style, text)| {
                let text = text.trim_end_matches(['\r', '\n']);
                (!text.is_empty()).then(|| {
                    Span::styled(
                        text.to_owned(),
                        code_style(
                            Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b),
                            style.font_style,
                        ),
                    )
                })
            }));
            lines.push(padded_code_line(spans, maximum_width));
        }
        tracing::debug!(
            target: "jux::selection_perf",
            language = %self.language,
            source_bytes = self.source.len(),
            output_lines = lines.len(),
            elapsed_us = %started.elapsed().as_micros(),
            "[DEBUG-selection-perf] code block highlighted"
        );
        lines
    }

    fn render_plain_text(&self, maximum_width: usize) -> Vec<Line<'static>> {
        self.source
            .trim_end_matches('\n')
            .lines()
            .map(|line| {
                padded_code_line(
                    vec![Span::styled(
                        format!(" {line}"),
                        code_style(Color::White, FontStyle::empty()),
                    )],
                    maximum_width,
                )
            })
            .collect()
    }
}

fn syntax_for_language(language: &str) -> Option<&'static SyntaxReference> {
    let language = language.trim().to_ascii_lowercase();
    let token = match language.as_str() {
        "shell" | "sh" | "zsh" => "bash",
        "jsx" => "js",
        "csharp" => "cs",
        other => other,
    };
    CODE_SYNTAXES.find_syntax_by_token(token)
}

fn padded_code_line(mut spans: Vec<Span<'static>>, maximum_width: usize) -> Line<'static> {
    let width = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    spans.push(Span::styled(
        " ".repeat(maximum_width.saturating_sub(width)),
        code_style(Color::White, FontStyle::empty()),
    ));
    Line::from(spans)
}

fn code_style(foreground: Color, font_style: FontStyle) -> Style {
    let mut modifiers = Modifier::empty();
    if font_style.contains(FontStyle::BOLD) {
        modifiers.insert(Modifier::BOLD);
    }
    if font_style.contains(FontStyle::ITALIC) {
        modifiers.insert(Modifier::ITALIC);
    }
    if font_style.contains(FontStyle::UNDERLINE) {
        modifiers.insert(Modifier::UNDERLINED);
    }
    Style::default()
        .fg(foreground)
        .bg(CODE_BACKGROUND)
        .add_modifier(modifiers)
}

#[derive(Default)]
struct TableState {
    rows: Vec<Vec<Vec<Span<'static>>>>,
    current_row: Vec<Vec<Span<'static>>>,
}

impl TableState {
    fn render(self, maximum_width: usize) -> Vec<Line<'static>> {
        let column_count = self.rows.iter().map(Vec::len).max().unwrap_or_default();
        if column_count == 0 {
            return Vec::new();
        }
        let mut widths = (0..column_count)
            .map(|column| {
                self.rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| spans_width(cell))
                    .max()
                    .unwrap_or(1)
                    .max(1)
            })
            .collect::<Vec<_>>();
        shrink_widths(&mut widths, maximum_width);
        let mut lines = vec![table_border(&widths, '┌', '┬', '┐')];
        let row_count = self.rows.len();
        for (index, row) in self.rows.into_iter().enumerate() {
            lines.push(table_row(row, &widths));
            if index + 1 < row_count {
                lines.push(table_border(&widths, '├', '┼', '┤'));
            }
        }
        lines.push(table_border(&widths, '└', '┴', '┘'));
        lines
    }
}

fn shrink_widths(widths: &mut [usize], maximum_width: usize) {
    let border_width = widths.len() + 1 + widths.len() * 2;
    while widths.iter().sum::<usize>() + border_width > maximum_width {
        let Some(width) = widths.iter_mut().max_by_key(|width| **width) else {
            return;
        };
        if *width <= 3 {
            return;
        }
        *width -= 1;
    }
}

fn table_border(widths: &[usize], left: char, middle: char, right: char) -> Line<'static> {
    let mut border = String::new();
    border.push(left);
    for (index, width) in widths.iter().enumerate() {
        border.push_str(&"─".repeat(width + 2));
        border.push(if index + 1 == widths.len() {
            right
        } else {
            middle
        });
    }
    Line::styled(border, Style::default().fg(Color::DarkGray))
}

fn table_row(row: Vec<Vec<Span<'static>>>, widths: &[usize]) -> Line<'static> {
    let mut rendered = vec![Span::styled("│ ", Style::default().fg(Color::DarkGray))];
    for (index, width) in widths.iter().enumerate() {
        let cell = row.get(index).cloned().unwrap_or_default();
        let cell = truncate_spans(cell, *width);
        let cell_width = spans_width(&cell);
        rendered.extend(cell);
        rendered.push(Span::raw(" ".repeat(width.saturating_sub(cell_width))));
        rendered.push(Span::styled(
            if index + 1 == widths.len() {
                " │"
            } else {
                " │ "
            },
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(rendered)
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
        if remaining == 0 {
            break;
        }
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
    }
    rendered
}
