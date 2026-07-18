use super::RenderState;
use super::theme::CONVERSATION_PADDING;
use ratatui::layout::Rect;

const NARROW_VIEWPORT_WIDTH: u16 = 60;
const DIVIDER_WIDTH: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceLayout {
    pub conversation: Rect,
    pub divider: Option<Rect>,
    pub sidebar: Option<Rect>,
}

impl WorkspaceLayout {
    pub(crate) fn calculate(state: &RenderState<'_>, area: Rect) -> Self {
        if area.width < NARROW_VIEWPORT_WIDTH {
            return Self {
                conversation: area,
                divider: None,
                sidebar: None,
            };
        }
        let conversation_width = state.conversation_panel_width(area.width);
        let conversation = Rect::new(area.x, area.y, conversation_width, area.height);
        let divider = Rect::new(
            area.x.saturating_add(conversation_width),
            area.y,
            DIVIDER_WIDTH,
            area.height,
        );
        let sidebar = state.sidebar_visible().then(|| {
            Rect::new(
                divider.x.saturating_add(DIVIDER_WIDTH),
                area.y,
                area.width
                    .saturating_sub(conversation_width)
                    .saturating_sub(DIVIDER_WIDTH),
                area.height,
            )
        });
        Self {
            conversation,
            divider: Some(divider),
            sidebar,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConversationLayout {
    pub history: Rect,
    pub input: Rect,
    pub status: Rect,
    pub scrollbar: Rect,
}

impl ConversationLayout {
    pub(crate) fn calculate(input_line_count: u16, area: Rect) -> Self {
        let status_height = u16::from(area.height > 0);
        let available_height = area.height.saturating_sub(status_height);
        let input_height = input_line_count
            .max(1)
            .saturating_add(2)
            .min(available_height);
        let status_y = area.y.saturating_add(available_height);
        let input_y = status_y.saturating_sub(input_height);
        let scrollbar_x = area.x.saturating_add(area.width.saturating_sub(1));
        Self {
            history: Rect::new(area.x, area.y, area.width, input_y.saturating_sub(area.y)),
            input: Rect::new(area.x, input_y, area.width, input_height),
            status: Rect::new(area.x, status_y, area.width, status_height),
            scrollbar: Rect::new(
                scrollbar_x,
                area.y.saturating_add(CONVERSATION_PADDING),
                u16::from(area.width > 0),
                input_y
                    .saturating_sub(area.y)
                    .saturating_sub(CONVERSATION_PADDING.saturating_mul(2)),
            ),
        }
    }
}
