use std::time::Instant;

use super::super::app::{AuditFilter, TextSelection};

const DEFAULT_CONVERSATION_WIDTH_PERCENT: u16 = 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EscapeAction {
    ClearInput,
    InterruptRun,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PendingEscapeAction {
    pub(crate) action: EscapeAction,
    pub(crate) expires_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SlashCommand {
    NewSession,
    Session,
    Version,
    Retry,
    Continue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SlashCommandDefinition {
    pub(crate) command: SlashCommand,
    pub name: &'static str,
    pub description: &'static str,
    pub usage: &'static str,
}

pub(crate) const SLASH_COMMANDS: [SlashCommandDefinition; 5] = [
    SlashCommandDefinition {
        command: SlashCommand::NewSession,
        name: "/new",
        description: "Start a new session",
        usage: "/new",
    },
    SlashCommandDefinition {
        command: SlashCommand::Session,
        name: "/session",
        description: "Switch active session",
        usage: "/session [new|rename|switch]",
    },
    SlashCommandDefinition {
        command: SlashCommand::Version,
        name: "/version",
        description: "Show the Jux version",
        usage: "/version",
    },
    SlashCommandDefinition {
        command: SlashCommand::Retry,
        name: "/retry",
        description: "Retry the failed request",
        usage: "/retry",
    },
    SlashCommandDefinition {
        command: SlashCommand::Continue,
        name: "/continue",
        description: "Continue the canceled request",
        usage: "/continue",
    },
];

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PromptState {
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) input_history: Vec<String>,
    pub(crate) input_history_index: Option<usize>,
    pub(crate) input_history_draft: String,
    pub(crate) undo_input: Option<(String, usize)>,
    pub(crate) selected_slash_command: usize,
    pub(crate) selected_inline_skill: usize,
    pub(crate) file_reference_cache: FileReferenceCache,
    pub(crate) selected_file_reference: usize,
    pub(crate) slash_commands_dismissed: bool,
    pub(crate) pending_escape_action: Option<PendingEscapeAction>,
}

impl PromptState {
    pub(crate) fn input_text(&self) -> &str {
        &self.input
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ConversationUiState {
    pub(crate) selected_message: Option<usize>,
    pub(crate) conversation_search: Option<String>,
    pub(crate) conversation_scroll_from_bottom: u16,
    pub(crate) selected_timeline: Option<usize>,
    pub(crate) text_selection: Option<TextSelection>,
    pub(crate) text_selection_drag: Option<TextSelection>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SidebarUiState {
    pub(crate) help_visible: bool,
    pub(crate) session_panel_visible: bool,
    pub(crate) session_search: String,
    pub(crate) session_rename: Option<String>,
    pub(crate) selected_session: usize,
    pub(crate) audit_panel_visible: bool,
    pub(crate) audit_filter: AuditFilter,
    pub(crate) selected_audit_item: usize,
    pub(crate) selected_skill: usize,
    pub(crate) skill_panel_visible: bool,
    pub(crate) log_panel_visible: bool,
    pub(crate) sidebar_visible: bool,
    pub(crate) divider_dragging: bool,
    pub(crate) conversation_width_percent: u16,
}

impl Default for SidebarUiState {
    fn default() -> Self {
        Self {
            help_visible: false,
            session_panel_visible: false,
            session_search: String::new(),
            session_rename: None,
            selected_session: 0,
            audit_panel_visible: false,
            audit_filter: AuditFilter::All,
            selected_audit_item: 0,
            selected_skill: 0,
            skill_panel_visible: false,
            log_panel_visible: false,
            sidebar_visible: true,
            divider_dragging: false,
            conversation_width_percent: DEFAULT_CONVERSATION_WIDTH_PERCENT,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OverlayUiState {
    pub(crate) selected_human_option: usize,
    pub(crate) human_input_error: Option<String>,
    pub(crate) selected_changed_file: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RootUiState {
    pub(crate) notification: Option<(String, Instant)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FileReferenceCache {
    pub(crate) index_revision: u64,
    pub(crate) query: Option<String>,
    pub(crate) matches: FileReferenceMatches,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum FileReferenceMatches {
    #[default]
    Disabled,
    AllFiles,
    Filtered(Vec<usize>),
}
