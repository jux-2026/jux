use crossterm::event::{Event, KeyEvent, MouseEvent};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Paste(String),
    Scroll(i32),
    Tick,
}

impl UiEvent {
    pub fn from_crossterm(event: Event) -> Option<Self> {
        match event {
            Event::Key(event) => Some(Self::Key(event)),
            Event::Mouse(event) => Some(Self::Mouse(event)),
            Event::Resize(width, height) => Some(Self::Resize(width, height)),
            Event::Paste(content) => Some(Self::Paste(content)),
            Event::FocusGained | Event::FocusLost => None,
        }
    }
}
