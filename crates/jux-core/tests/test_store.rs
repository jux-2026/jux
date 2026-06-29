use jux_core::{
    AssistantResponseItem, LlmUsage, SessionContextKind, SessionContextPayload,
    SqliteWorkspaceStore, StepKind, StepPayload,
};

#[test]
fn sqlite_store_creates_and_lists_named_sessions() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let workspace = store.init_workspace().expect("workspace initializes");

    let session = store
        .create_session(Some("feature-a".to_owned()))
        .expect("session is created");
    let sessions = store.load_sessions().expect("sessions load");

    assert_eq!(session.id.to_string(), format!("{}-0002", workspace.id));
    assert_eq!(session.name.as_deref(), Some("feature-a"));
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].name.as_deref(), Some("default"));
    assert_eq!(sessions[1], session);
}

#[test]
fn sqlite_store_renames_a_session() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let session = store
        .create_session(Some("old-name".to_owned()))
        .expect("session is created");

    let renamed = store
        .rename_session(&session.id, Some("new-name".to_owned()))
        .expect("session is renamed");

    assert_eq!(renamed.id, session.id);
    assert_eq!(renamed.name.as_deref(), Some("new-name"));
    assert_eq!(
        store
            .load_session(&session.id)
            .expect("session reloads")
            .name
            .as_deref(),
        Some("new-name")
    );
}

#[test]
fn sqlite_store_switches_the_active_session() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let session = store
        .create_session(Some("feature-a".to_owned()))
        .expect("session is created");

    let workspace = store
        .set_active_session(&session.id)
        .expect("active session is switched");

    assert_eq!(workspace.active_session_id, session.id);
    assert_eq!(
        store
            .load_active_session()
            .expect("active session reloads")
            .id,
        session.id
    );
}

#[test]
fn sqlite_store_persists_workspace_session_run_and_ordered_steps() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());

    let workspace = store.init_workspace().expect("workspace initializes");
    let session = store
        .load_active_session()
        .expect("active session can be loaded");
    let run = store
        .create_run("Explain this project".to_owned())
        .expect("run is created");
    let second_run = store
        .create_run("Explain the second run".to_owned())
        .expect("second run is created");

    let first_step = store
        .append_step(
            &run.id,
            StepKind::UserMessage,
            StepPayload::UserMessage {
                content: "Explain this project".to_owned(),
            },
        )
        .expect("first step is saved");
    let second_step = store
        .append_step(
            &run.id,
            StepKind::AssistantResponse,
            StepPayload::AssistantResponse {
                message_id: None,
                usage: LlmUsage::default(),
                items: vec![AssistantResponseItem::Text {
                    content: "Done".to_owned(),
                }],
            },
        )
        .expect("second step is saved");
    let steps = store.load_run_steps(&run.id).expect("steps load");

    assert_eq!(workspace.active_session_id, session.id);
    assert_eq!(run.id.session_id(), session.id);
    assert_eq!(run.id.to_string(), format!("{}-000001", session.id));
    assert_eq!(second_run.id.to_string(), format!("{}-000002", session.id));
    assert_eq!(first_step.id.to_string(), format!("{}-000001", run.id));
    assert_eq!(second_step.id.to_string(), format!("{}-000002", run.id));
    assert_eq!(steps, vec![first_step, second_step]);

    let connection = rusqlite::Connection::open(store.database_path()).expect("database opens");
    let sequence_table_count: u64 = connection
        .query_row(
            "select count(*) from sqlite_master where type = 'table' and name = 'sequences'",
            [],
            |row| row.get(0),
        )
        .expect("schema table count can be queried");
    assert_eq!(sequence_table_count, 0);
}

#[test]
fn sqlite_store_persists_ordered_session_context_items_once() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let workspace = store.init_workspace().expect("workspace initializes");
    let session_id = workspace.active_session_id;

    let first = store
        .ensure_session_context_items(
            &session_id,
            vec![
                (
                    SessionContextKind::SystemPrompt,
                    SessionContextPayload::SystemPrompt {
                        content: "system".to_owned(),
                    },
                ),
                (
                    SessionContextKind::ToolDefinition,
                    SessionContextPayload::ToolDefinition {
                        name: "tool".to_owned(),
                        description: "tool description".to_owned(),
                        parameters: serde_json::json!({ "type": "object" }),
                    },
                ),
            ],
        )
        .expect("session context initializes");
    let second = store
        .ensure_session_context_items(
            &session_id,
            vec![(
                SessionContextKind::SystemPrompt,
                SessionContextPayload::SystemPrompt {
                    content: "ignored".to_owned(),
                },
            )],
        )
        .expect("existing session context loads");

    assert_eq!(first, second);
    assert_eq!(first[0].sequence, 1);
    assert_eq!(first[0].kind, SessionContextKind::SystemPrompt);
    assert_eq!(first[1].sequence, 2);
    assert_eq!(first[1].kind, SessionContextKind::ToolDefinition);
}

#[test]
fn sqlite_store_appends_missing_default_session_context_items() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let workspace = store.init_workspace().expect("workspace initializes");
    let session_id = workspace.active_session_id;

    store
        .ensure_session_context_items(
            &session_id,
            vec![
                (
                    SessionContextKind::SystemPrompt,
                    SessionContextPayload::SystemPrompt {
                        content: "system".to_owned(),
                    },
                ),
                (
                    SessionContextKind::ToolDefinition,
                    SessionContextPayload::ToolDefinition {
                        name: "exec".to_owned(),
                        description: "exec".to_owned(),
                        parameters: serde_json::json!({ "type": "object" }),
                    },
                ),
            ],
        )
        .expect("initial session context saves");
    let updated = store
        .ensure_session_context_items(
            &session_id,
            vec![
                (
                    SessionContextKind::SystemPrompt,
                    SessionContextPayload::SystemPrompt {
                        content: "new system ignored".to_owned(),
                    },
                ),
                (
                    SessionContextKind::ToolDefinition,
                    SessionContextPayload::ToolDefinition {
                        name: "exec".to_owned(),
                        description: "new exec ignored".to_owned(),
                        parameters: serde_json::json!({ "type": "object" }),
                    },
                ),
                (
                    SessionContextKind::ToolDefinition,
                    SessionContextPayload::ToolDefinition {
                        name: "lua".to_owned(),
                        description: "lua".to_owned(),
                        parameters: serde_json::json!({ "type": "object" }),
                    },
                ),
            ],
        )
        .expect("missing session context appends");

    assert_eq!(updated.len(), 3);
    assert_eq!(updated[0].sequence, 1);
    assert_eq!(updated[0].payload.to_system_prompt(), Some("system"));
    assert_eq!(updated[1].sequence, 2);
    assert_eq!(updated[1].payload.to_tool_name(), Some("exec"));
    assert_eq!(updated[2].sequence, 3);
    assert_eq!(updated[2].payload.to_tool_name(), Some("lua"));
}

trait SessionContextPayloadTestExt {
    fn to_system_prompt(&self) -> Option<&str>;
    fn to_tool_name(&self) -> Option<&str>;
}

impl SessionContextPayloadTestExt for SessionContextPayload {
    fn to_system_prompt(&self) -> Option<&str> {
        match self {
            SessionContextPayload::SystemPrompt { content } => Some(content),
            SessionContextPayload::ToolDefinition { .. } => None,
        }
    }

    fn to_tool_name(&self) -> Option<&str> {
        match self {
            SessionContextPayload::ToolDefinition { name, .. } => Some(name),
            SessionContextPayload::SystemPrompt { .. } => None,
        }
    }
}

fn temp_workspace_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("jux-store-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace root is created");
    root
}
