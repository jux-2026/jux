mod app;
mod application;
mod events;
mod file_index;
mod runtime;
mod terminal;
mod ui;

pub use self::app::{
    AppCommand, AppModel, AppMsg, AuditItem, FocusedPanel, Message, MessageRole, SelectionPanel,
    SessionHistory, TextSelection, TextSelectionPoint, TimelineItem, TimelineStatus,
    TuiCodeChangeResult, TuiCommandExecution, TuiRunStatus, TuiRuntimeInfo, TuiRuntimeLog,
    TuiSandboxSummary, TuiViewport, assign_default_session_title, execute_code_change_command,
    execute_session_command, load_active_session_history, materialize_pending_new_session, update,
};
pub use self::application::{DispatchResult, TuiApp};
pub use self::events::{EventHandler, TuiEvent};
pub use self::file_index::{FileIndexKind, FileIndexService, FileIndexSnapshot};
pub use self::runtime::{AgentEventSender, BackgroundRun, RunHandler, RunResponse, TuiRunRequest};
pub use self::terminal::{TerminalEventDecoder, run_tui};
pub use self::ui::{
    AppComponent, Component, EventResult, FocusId, FocusManager, UiEvent, VirtualItemRenderer,
    VirtualListState, VisibleItem,
};
