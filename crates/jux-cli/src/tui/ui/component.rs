use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::event::UiEvent;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventResult<Msg> {
    pub message: Option<Msg>,
    pub consumed: bool,
}

impl<Msg> EventResult<Msg> {
    pub fn ignored() -> Self {
        Self {
            message: None,
            consumed: false,
        }
    }

    pub fn consumed(message: Option<Msg>) -> Self {
        Self {
            message,
            consumed: true,
        }
    }
}

pub trait Component<Msg> {
    type Model;

    fn render(&mut self, model: &Self::Model, area: Rect, buffer: &mut Buffer);

    fn handle_event(&mut self, _model: &Self::Model, _event: &UiEvent) -> EventResult<Msg> {
        EventResult::ignored()
    }

    fn focusable(&self) -> bool {
        false
    }

    fn set_focused(&mut self, _focused: bool) {}
}
