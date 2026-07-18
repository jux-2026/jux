#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FocusId {
    Conversation,
    PromptInput,
    Sidebar,
    Overlay,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusManager {
    order: Vec<FocusId>,
    current: Option<FocusId>,
    modal: Option<FocusId>,
    restore_after_modal: Option<FocusId>,
}

impl FocusManager {
    pub fn new(order: impl IntoIterator<Item = FocusId>) -> Self {
        let order = order.into_iter().collect::<Vec<_>>();
        let current = order.first().copied();
        Self {
            order,
            current,
            modal: None,
            restore_after_modal: None,
        }
    }

    pub fn current(&self) -> Option<FocusId> {
        self.modal.or(self.current)
    }

    pub fn set_order(&mut self, order: impl IntoIterator<Item = FocusId>) {
        self.order = order.into_iter().collect();
        if self.modal.is_none() && self.current.is_none_or(|id| !self.order.contains(&id)) {
            self.current = self.order.first().copied();
        }
    }

    pub fn focus(&mut self, id: FocusId) -> bool {
        if self.modal.is_some() || !self.order.contains(&id) {
            return false;
        }
        self.current = Some(id);
        true
    }

    pub fn focus_next(&mut self, reverse: bool) -> Option<FocusId> {
        if let Some(modal) = self.modal {
            return Some(modal);
        }
        if self.order.is_empty() {
            self.current = None;
            return None;
        }
        let current = self
            .current
            .and_then(|current| self.order.iter().position(|id| *id == current))
            .unwrap_or(0);
        let next = if reverse {
            current.checked_sub(1).unwrap_or(self.order.len() - 1)
        } else {
            (current + 1) % self.order.len()
        };
        self.current = Some(self.order[next]);
        self.current
    }

    pub fn open_modal(&mut self, id: FocusId) {
        if self.modal == Some(id) {
            return;
        }
        self.restore_after_modal = self.current;
        self.modal = Some(id);
    }

    pub fn close_modal(&mut self) {
        self.modal = None;
        self.current = self
            .restore_after_modal
            .take()
            .filter(|id| self.order.contains(id))
            .or_else(|| self.order.first().copied());
    }

    pub fn modal_active(&self) -> bool {
        self.modal.is_some()
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new([FocusId::PromptInput, FocusId::Sidebar])
    }
}
