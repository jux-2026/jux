use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jux_cli::tui::{
    AppComponent, AppModel, AppMsg, Component, FocusId, FocusManager, TuiApp, TuiViewport, UiEvent,
    VirtualItemRenderer, VirtualListState, update,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

#[derive(Clone, Copy)]
struct Item {
    height: usize,
    revision: u64,
}

#[derive(Default)]
struct MeasuringRenderer {
    measured: Vec<u64>,
}

impl VirtualItemRenderer<Item> for MeasuringRenderer {
    fn revision(&self, item: &Item) -> u64 {
        item.revision
    }

    fn measure(&mut self, item: &Item, _width: u16) -> usize {
        self.measured.push(item.revision);
        item.height
    }

    fn render(
        &mut self,
        _item: &Item,
        _index: usize,
        _area: Rect,
        _skip_top: usize,
        _buffer: &mut Buffer,
    ) {
    }
}

#[test]
fn virtual_list_calculates_variable_height_visible_items_and_top_clipping() {
    let items = [
        Item {
            height: 2,
            revision: 0,
        },
        Item {
            height: 5,
            revision: 0,
        },
        Item {
            height: 3,
            revision: 0,
        },
    ];
    let mut renderer = MeasuringRenderer::default();
    let mut state = VirtualListState::default();
    state.measure(&items, 40, 4, &mut renderer);
    state.scroll_to_top();
    state.scroll_by(3);

    let visible = state.visible_items(Rect::new(0, 0, 40, 4), 0);

    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].index, 1);
    assert_eq!(visible[0].skip_top, 1);
    assert_eq!(visible[0].area.height, 4);
}

#[test]
fn virtual_list_keeps_bottom_anchor_only_while_stuck_to_bottom() {
    let mut items = vec![
        Item {
            height: 4,
            revision: 0,
        },
        Item {
            height: 4,
            revision: 0,
        },
    ];
    let mut renderer = MeasuringRenderer::default();
    let mut state = VirtualListState::default();
    state.measure(&items, 40, 5, &mut renderer);
    assert_eq!(state.scroll_y, 3);

    items.push(Item {
        height: 2,
        revision: 0,
    });
    state.measure(&items, 40, 5, &mut renderer);
    assert_eq!(state.scroll_y, 5);

    state.scroll_by(-2);
    assert!(!state.stick_to_bottom);
    items.push(Item {
        height: 3,
        revision: 0,
    });
    state.measure(&items, 40, 5, &mut renderer);
    assert_eq!(state.scroll_y, 3);
}

#[test]
fn virtual_list_remeasures_width_changes_and_only_changed_revisions() {
    let mut items = [
        Item {
            height: 2,
            revision: 1,
        },
        Item {
            height: 2,
            revision: 1,
        },
    ];
    let mut renderer = MeasuringRenderer::default();
    let mut state = VirtualListState::default();
    state.measure(&items, 80, 10, &mut renderer);
    assert_eq!(renderer.measured.len(), 2);

    state.measure(&items, 80, 10, &mut renderer);
    assert_eq!(renderer.measured.len(), 2);

    items[1].revision += 1;
    state.measure(&items, 80, 10, &mut renderer);
    assert_eq!(renderer.measured, vec![1, 1, 2]);

    state.measure(&items, 40, 10, &mut renderer);
    assert_eq!(renderer.measured.len(), 5);
}

#[test]
fn focus_manager_cycles_and_restores_focus_after_modal() {
    let mut focus = FocusManager::new([
        FocusId::PromptInput,
        FocusId::Sidebar,
        FocusId::Conversation,
    ]);
    assert_eq!(focus.current(), Some(FocusId::PromptInput));
    assert_eq!(focus.focus_next(false), Some(FocusId::Sidebar));
    assert_eq!(focus.focus_next(true), Some(FocusId::PromptInput));

    focus.open_modal(FocusId::Overlay);
    assert_eq!(focus.current(), Some(FocusId::Overlay));
    assert_eq!(focus.focus_next(false), Some(FocusId::Overlay));
    assert!(!focus.focus(FocusId::Sidebar));

    focus.close_modal();
    assert_eq!(focus.current(), Some(FocusId::PromptInput));
}

#[test]
fn focus_manager_recovers_when_the_focused_target_is_hidden() {
    let mut focus = FocusManager::new([FocusId::PromptInput, FocusId::Sidebar]);
    assert!(focus.focus(FocusId::Sidebar));

    focus.set_order([FocusId::PromptInput]);

    assert_eq!(focus.current(), Some(FocusId::PromptInput));
}

#[test]
fn app_component_consumes_focus_navigation_before_the_model_reducer() {
    let mut component = AppComponent::default();
    let model = AppModel::new("/workspace");

    let result = component.handle_event(
        &model,
        UiEvent::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        TuiViewport {
            width: 100,
            height: 30,
        },
    );

    assert!(result.consumed);
    assert!(result.message.is_none());
}

#[test]
fn app_component_converts_paste_to_one_typed_message() {
    let mut component = AppComponent::default();
    let model = AppModel::new("/workspace");

    let result = component.handle_event(
        &model,
        UiEvent::Paste("first\nsecond".to_owned()),
        TuiViewport {
            width: 80,
            height: 24,
        },
    );

    assert!(result.consumed);
    assert!(matches!(
        result.message,
        Some(UiEvent::Paste(content)) if content == "first\nsecond"
    ));
}

#[test]
fn conversation_component_consumes_scroll_without_mutating_the_model() {
    let mut component = AppComponent::default();
    let mut model = AppModel::new("/workspace");
    for index in 0..20 {
        update(
            &mut model,
            AppMsg::AssistantMessage {
                content: format!("message {index}\nsecond line\nthird line"),
            },
        );
    }
    let area = Rect::new(0, 0, 80, 24);
    let mut buffer = Buffer::empty(area);
    Component::render(&mut component, &model, area, &mut buffer);

    let result = component.handle_event(
        &model,
        UiEvent::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
        TuiViewport {
            width: area.width,
            height: area.height,
        },
    );
    let mut buffer = Buffer::empty(area);
    Component::render(&mut component, &model, area, &mut buffer);

    assert!(result.consumed);
    assert!(result.message.is_none());
    assert_ne!(component.conversation_scroll_position(), "Bottom");
}

#[test]
fn tui_app_routes_coalesced_scroll_directly_to_conversation() {
    let mut app = TuiApp::new(AppModel::new("/workspace"));
    for index in 0..20 {
        app.update(AppMsg::AssistantMessage {
            content: format!("message {index}\nsecond line\nthird line"),
        });
    }
    let area = Rect::new(0, 0, 80, 24);
    let mut buffer = Buffer::empty(area);
    app.render_buffer(area, &mut buffer);

    let result = app.dispatch(
        UiEvent::Scroll(10),
        TuiViewport {
            width: area.width,
            height: area.height,
        },
    );

    assert!(result.consumed);
    assert_ne!(app.conversation_scroll_position(), "Bottom");
}

#[test]
fn tui_app_dispatches_component_messages_through_the_model_update_seam() {
    let mut app = TuiApp::new(AppModel::new("/workspace"));

    let result = app.dispatch(
        UiEvent::Paste("first\nsecond".to_owned()),
        TuiViewport {
            width: 80,
            height: 24,
        },
    );

    assert!(result.consumed);
    assert!(result.command.is_none());
    assert_eq!(app.input_text(), "first\nsecond");
}
