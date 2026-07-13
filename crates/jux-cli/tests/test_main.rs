use assert_cmd::Command;
use assert_fs::TempDir;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

#[test]
fn cli_exposes_foundation_commands() {
    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("jux 0.1.0"));

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Jux agent command line interface.",
        ))
        .stdout(predicate::str::contains("--output"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("skills"))
        .stdout(predicate::str::contains("session"));

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--stream"))
        .stdout(predicate::str::contains("--skill"))
        .stdout(predicate::str::contains("--no-auto-skills"));
}

#[test]
fn skills_list_outputs_available_skills_with_sources() {
    let home = TempDir::new().expect("temp home exists");
    let workspace = TempDir::new().expect("temp workspace exists");
    write_skill(home.path(), "format", "Format code", "Format body.");
    write_skill(workspace.path(), "review", "Review code", "Review body.");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "skills",
            "list",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
        ])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("format"))
        .stdout(predicate::str::contains("user"))
        .stdout(predicate::str::contains("Format code"))
        .stdout(predicate::str::contains("review"))
        .stdout(predicate::str::contains("project"))
        .stdout(predicate::str::contains("Review code"))
        .stdout(predicate::str::contains(
            "Project skills override user skills",
        ));
}

#[test]
fn skills_list_outputs_override_hints() {
    let home = TempDir::new().expect("temp home exists");
    let workspace = TempDir::new().expect("temp workspace exists");
    write_skill(home.path(), "review", "User review", "User review body.");
    write_skill(
        workspace.path(),
        "review",
        "Project review",
        "Project review body.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "skills",
            "list",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
        ])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("review"))
        .stdout(predicate::str::contains("Project review"))
        .stdout(predicate::str::contains("overrides user skill"));
}

#[test]
fn skills_show_outputs_skill_body_and_source() {
    let workspace = TempDir::new().expect("temp workspace exists");
    write_skill(
        workspace.path(),
        "review",
        "Review code",
        "Full review instructions.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "skills",
            "show",
            "review",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("review"))
        .stdout(predicate::str::contains("Review code"))
        .stdout(predicate::str::contains("project"))
        .stdout(predicate::str::contains("Full review instructions."));
}

#[test]
fn skills_show_reports_missing_skill() {
    let workspace = TempDir::new().expect("temp workspace exists");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "skills",
            "show",
            "missing",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("skill not found: missing"));
}

#[test]
fn run_command_executes_mocked_llm_and_persists_state() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("Mocked Jux answer");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Explain this project",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Mocked Jux answer"))
        .stdout(predicate::str::contains("workspace_id:").not())
        .stdout(predicate::str::contains("session_id:").not())
        .stdout(predicate::str::contains("run_id:").not())
        .stdout(predicate::str::contains("status:").not())
        .stdout(predicate::str::contains("SystemMessage").not());

    let requests = mock.join();
    let request = requests.first().expect("mock receives one request");
    assert!(
        request
            .to_lowercase()
            .contains("authorization: bearer test-api-key")
    );
    assert!(request.contains("Explain this project"));
    assert!(workspace.path().join(".jux/state.db").exists());
}

#[test]
fn run_command_loads_user_and_project_agents_documents() {
    let home = TempDir::new().expect("temp home exists");
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("Instruction answer");
    std::fs::create_dir_all(home.path().join(".jux")).expect("user .jux exists");
    std::fs::create_dir_all(workspace.path().join(".jux")).expect("project .jux exists");
    std::fs::write(home.path().join(".jux/AGENTS.md"), "Use user agent rules.")
        .expect("user agents file is written");
    std::fs::write(
        workspace.path().join(".jux/AGENTS.md"),
        "Use project agent rules.",
    )
    .expect("project agents file is written");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Use instruction documents",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("HOME", home.path())
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Instruction answer"));

    let requests = mock.join();
    let request = requests.first().expect("mock receives one request");
    assert!(request.contains("Project instructions have higher priority than user instructions."));
    assert!(request.contains("Use user agent rules."));
    assert!(request.contains("Use project agent rules."));
    assert!(request.find("Use user agent rules.") < request.find("Use project agent rules."));
}

#[test]
fn run_command_loads_user_agents_documents_from_userprofile() {
    let home = TempDir::new().expect("temp home exists");
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("Userprofile instruction answer");
    std::fs::create_dir_all(home.path().join(".jux")).expect("user .jux exists");
    std::fs::write(
        home.path().join(".jux/AGENTS.md"),
        "Use userprofile agent rules.",
    )
    .expect("user agents file is written");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Use USERPROFILE instruction documents",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env_remove("HOME")
        .env("USERPROFILE", home.path())
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Userprofile instruction answer"));

    let requests = mock.join();
    let request = requests.first().expect("mock receives one request");
    assert!(request.contains("Use userprofile agent rules."));
}

#[test]
fn run_command_loads_available_skill_index() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("Skill index answer");
    write_skill(
        workspace.path(),
        "review",
        "Review code changes",
        "Full review skill body should not be sent in the index.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Use available skills",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Skill index answer"));

    let requests = mock.join();
    let request = requests.first().expect("mock receives one request");
    assert!(request.contains("## Available Skills"));
    assert!(request.contains("- review: Review code changes"));
    assert!(!request.contains("Full review skill body should not be sent in the index."));
}

#[test]
fn run_command_can_activate_explicit_skill() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([
        DeepseekMockResponse::text("Explicit skill result"),
        DeepseekMockResponse::text("Explicit parent answer"),
    ]);
    write_skill(
        workspace.path(),
        "review",
        "Review code changes",
        "Full explicit review instructions.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Use explicit skill",
            "--skill",
            "review",
            "--stream",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "run.skills output skills=[\"review\"]",
        ))
        .stdout(predicate::str::contains("Explicit parent answer"));

    let requests = mock.join();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("Full explicit review instructions."));
    assert!(requests[1].contains("Explicit skill result"));
    assert!(!requests[1].contains("Full explicit review instructions."));
}

#[test]
fn run_command_json_hides_internal_skill_transcript() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([
        DeepseekMockResponse::text("Hidden skill result"),
        DeepseekMockResponse::text("Visible parent answer"),
    ]);
    write_skill(
        workspace.path(),
        "review",
        "Review code changes",
        "Private full skill instructions.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "--output",
            "json",
            "run",
            "Use explicit skill",
            "--skill",
            "review",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Visible parent answer"))
        .stdout(predicate::str::contains("SkillExecution").not())
        .stdout(predicate::str::contains("Private full skill instructions.").not());

    assert_eq!(mock.join().len(), 2);
}

#[test]
fn run_command_reports_missing_explicit_skill() {
    let workspace = TempDir::new().expect("temp workspace exists");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Use missing skill",
            "--skill",
            "missing",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            "http://127.0.0.1:1",
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .failure()
        .stderr(predicate::str::contains("skill not found: missing"));
}

#[test]
fn run_command_does_not_auto_activate_skill_by_request_text() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("Auto skill answer");
    write_skill(
        workspace.path(),
        "review",
        "Review code changes",
        "Full automatic review instructions.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Please review this patch",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Auto skill answer"));

    let requests = mock.join();
    let request = requests.first().expect("mock receives one request");
    assert!(request.contains("## Available Skills"));
    assert!(request.contains("\"name\":\"call_skill\""));
    assert!(!request.contains("## Active Skills"));
    assert!(!request.contains("Full automatic review instructions."));
}

#[test]
fn run_command_can_disable_auto_skill_matching() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("No auto skill answer");
    write_skill(
        workspace.path(),
        "review",
        "Review code changes",
        "Full automatic review instructions.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Please review this patch",
            "--no-auto-skills",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("No auto skill answer"));

    let requests = mock.join();
    let request = requests.first().expect("mock receives one request");
    assert!(request.contains("## Available Skills"));
    assert!(!request.contains("## Active Skills"));
    assert!(!request.contains("Full automatic review instructions."));
}

#[test]
fn run_command_waits_for_human_input_and_resumes_latest_run() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([
        DeepseekMockResponse::tool_call(
            "call_1",
            "human_input",
            serde_json::json!({
                "prompt": "Choose an action",
                "options": [{ "id": "continue", "label": "Continue" }],
                "allow_free_text": false
            }),
        ),
        DeepseekMockResponse::text("Continued after human input"),
    ]);

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Ask a human",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Waiting for human input"))
        .stdout(predicate::str::contains("Choose an action"))
        .stdout(predicate::str::contains("continue"))
        .stdout(predicate::str::contains("Continue"));

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "continue",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Continued after human input"));

    let requests = mock.join();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("continue"));
}

#[test]
fn run_command_resumes_human_input_inside_skill() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([
        DeepseekMockResponse::tool_call(
            "skill_call",
            "call_skill",
            serde_json::json!({
                "name": "review",
                "task": "Review current changes"
            }),
        ),
        DeepseekMockResponse::tool_call(
            "human_call",
            "human_input",
            serde_json::json!({
                "prompt": "Choose review depth",
                "options": [{ "id": "deep", "label": "Deep review" }],
                "allow_free_text": false
            }),
        ),
        DeepseekMockResponse::text("Skill completed a deep review"),
        DeepseekMockResponse::text("Parent received resumed skill result"),
    ]);
    write_skill(
        workspace.path(),
        "review",
        "Review code changes",
        "Ask for review depth before reviewing.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Use review skill",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Waiting for human input"))
        .stdout(predicate::str::contains("Choose review depth"))
        .stdout(predicate::str::contains("deep"))
        .stdout(predicate::str::contains("Deep review"));

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "deep",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Parent received resumed skill result",
        ));

    let requests = mock.join();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].contains("deep"));
    assert!(requests[3].contains("Skill completed a deep review"));
}

#[test]
fn run_command_resumes_human_input_inside_explicit_skill() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([
        DeepseekMockResponse::tool_call(
            "human_call",
            "human_input",
            serde_json::json!({
                "prompt": "Choose review depth",
                "options": [{ "id": "deep", "label": "Deep review" }],
                "allow_free_text": false
            }),
        ),
        DeepseekMockResponse::text("Explicit skill completed"),
        DeepseekMockResponse::text("Parent received explicit skill result"),
    ]);
    write_skill(
        workspace.path(),
        "review",
        "Review code changes",
        "Ask for review depth before reviewing.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Use review skill",
            "--skill",
            "review",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Choose review depth"));

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "deep",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Parent received explicit skill result",
        ));

    let requests = mock.join();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].contains("deep"));
    assert!(requests[2].contains("Explicit skill completed"));
}

#[test]
fn run_command_rejects_invalid_human_input_option() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([DeepseekMockResponse::tool_call(
        "call_1",
        "human_input",
        serde_json::json!({
            "prompt": "Choose an action",
            "options": [{ "id": "continue", "label": "Continue" }],
            "allow_free_text": false
        }),
    )]);

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Ask a human",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success();
    assert_eq!(mock.join().len(), 1);

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "different",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            "http://127.0.0.1:1",
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "human input must match one of the option ids",
        ));
}

#[test]
fn run_command_streams_with_available_skill_tool() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("Streamed skill answer");
    write_skill(
        workspace.path(),
        "review",
        "Review code changes",
        "Full streamed review instructions.",
    );

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Please review this patch",
            "--stream",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Streamed skill answer"));

    let requests = mock.join();
    assert_eq!(requests.len(), 1);
    let request = requests.first().expect("mock receives one request");
    assert!(request.contains("\"name\":\"call_skill\""));
    assert!(!request.contains("Full streamed review instructions."));
}

#[test]
fn run_command_can_output_json() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("JSON answer");

    let assert = Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "--output",
            "json",
            "run",
            "Return JSON output",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout is utf-8");
    let output: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    assert_eq!(output["answer"], "JSON answer");
    assert_eq!(output["status"], "Completed");
    assert_eq!(output["request"], "Return JSON output");
    assert!(
        output["workspace_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(
        output["session_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(output["run_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(output["created_at"].as_u64().is_some());
    assert!(output["updated_at"].as_u64().is_some());
    assert_eq!(output["steps"][0]["kind"], "UserMessage");
    assert!(
        output["steps"][0]["payload"]["UserMessage"]["content"]
            .as_str()
            .is_some_and(|content| content == "Return JSON output")
    );
    assert_eq!(output["steps"][1]["kind"], "AssistantResponse");
    assert!(output["steps"][0]["created_at"].as_u64().is_some());
    assert!(output["steps"][0]["updated_at"].as_u64().is_some());
    assert!(output["steps"][0].get("status").is_none());

    let requests = mock.join();
    assert_eq!(requests.len(), 1);
}

#[test]
fn run_command_can_output_yaml() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("YAML answer");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Return YAML output",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
            "--output",
            "yaml",
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("answer: YAML answer"))
        .stdout(predicate::str::contains("status: Completed"))
        .stdout(predicate::str::contains("workspace_id:"))
        .stdout(predicate::str::contains("session_id:"))
        .stdout(predicate::str::contains("run_id:"))
        .stdout(predicate::str::contains("request: Return YAML output"))
        .stdout(predicate::str::contains("created_at:"))
        .stdout(predicate::str::contains("updated_at:"))
        .stdout(predicate::str::contains("steps:"))
        .stdout(predicate::str::contains("kind: UserMessage"))
        .stdout(predicate::str::contains("payload:"));

    let requests = mock.join();
    assert_eq!(requests.len(), 1);
}

#[test]
fn run_command_executes_mocked_tool_call_loop() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([
        DeepseekMockResponse::tool_call(
            "call_1",
            "exec",
            serde_json::json!({ "program": "printf", "args": ["cli tool result"] }),
        ),
        DeepseekMockResponse::text("Final answer after tool"),
    ]);

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Use a tool",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Final answer after tool"))
        .stdout(predicate::str::contains("status:").not())
        .stdout(predicate::str::contains("SystemMessage").not());

    let requests = mock.join();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("Use a tool"));
    assert!(requests[0].contains("\"tools\""));
    assert!(requests[1].contains("cli tool result"));
    assert!(requests[1].contains("\"role\":\"tool\""));
}

#[test]
fn run_command_can_stream_hierarchical_events() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([
        DeepseekMockResponse::tool_call(
            "call_1",
            "exec",
            serde_json::json!({ "program": "printf", "args": ["streamed result"] }),
        ),
        DeepseekMockResponse::text("Final streamed answer"),
    ]);

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Stream a tool run",
            "--stream",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("run started"))
        .stdout(predicate::str::contains("run.iteration.1 started"))
        .stdout(predicate::str::contains("run.iteration.1.llm.1 started"))
        .stdout(predicate::str::contains(
            "run.iteration.1.tool.exec.1 started",
        ))
        .stdout(predicate::str::contains(
            "run.iteration.1.tool.exec.1 output",
        ))
        .stdout(predicate::str::contains("run completed"))
        .stdout(predicate::str::contains("Final streamed answer"));

    let requests = mock.join();
    assert_eq!(requests.len(), 2);
}

#[test]
fn session_show_outputs_active_session_state() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("Session answer");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Create session state",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success();
    let requests = mock.join();
    assert_eq!(requests.len(), 1);

    let assert = Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "--output",
            "json",
            "session",
            "show",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout is utf-8");
    let output: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    assert_eq!(output["session"]["name"], "default");
    assert_eq!(output["session_context"][0]["sequence"], 1);
    assert_eq!(output["session_context"][0]["kind"], "SystemPrompt");
    assert!(
        output["session_context"][0]["payload"]["SystemPrompt"]["content"]
            .as_str()
            .is_some_and(|content| content.contains("You are Jux"))
    );
    assert_eq!(output["session_context"][1]["sequence"], 2);
    assert_eq!(output["session_context"][1]["kind"], "ToolDefinition");
    assert_eq!(
        output["session_context"][1]["payload"]["ToolDefinition"]["name"],
        "exec"
    );
    assert_eq!(output["session_context"][2]["sequence"], 3);
    assert_eq!(output["session_context"][2]["kind"], "ToolDefinition");
    assert_eq!(
        output["session_context"][2]["payload"]["ToolDefinition"]["name"],
        "lua"
    );
    assert_eq!(output["runs"][0]["request"], "Create session state");
    assert_eq!(output["runs"][0]["status"], "Completed");
    assert!(output.get("steps").is_none());
    assert_eq!(output["runs"][0]["steps"][0]["kind"], "UserMessage");
    assert_eq!(output["runs"][0]["steps"][1]["kind"], "AssistantResponse");
}

#[test]
fn session_show_yaml_uses_block_style_for_multiline_text() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock("First line\nSecond line");

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "run",
            "Create multiline session state",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
            "--deepseek-base-url",
            &mock.base_url,
        ])
        .env("JUX_DEEPSEEK_API_KEY", "test-api-key")
        .assert()
        .success();
    let requests = mock.join();
    assert_eq!(requests.len(), 1);

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .args([
            "--output",
            "yaml",
            "session",
            "show",
            "--workspace",
            workspace.path().to_str().expect("workspace path is utf-8"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("content: |"))
        .stdout(predicate::str::contains("First line"))
        .stdout(predicate::str::contains("Second line"))
        .stdout(predicate::str::contains("First line\\nSecond line").not());
}

struct MockDeepseek {
    base_url: String,
    handle: thread::JoinHandle<Vec<String>>,
}

impl MockDeepseek {
    fn join(self) -> Vec<String> {
        self.handle.join().expect("mock server thread succeeds")
    }
}

#[derive(Clone, Debug)]
enum DeepseekMockResponse {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
}

impl DeepseekMockResponse {
    fn text(content: impl Into<String>) -> Self {
        Self::Text(content.into())
    }

    fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self::ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

fn start_deepseek_mock(content: &str) -> MockDeepseek {
    start_deepseek_mock_sequence([DeepseekMockResponse::text(content)])
}

fn start_deepseek_mock_sequence(
    contents: impl IntoIterator<Item = DeepseekMockResponse>,
) -> MockDeepseek {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock server binds");
    let address = listener.local_addr().expect("mock server has address");
    let contents = contents.into_iter().collect::<Vec<_>>();
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for content in contents {
            let (mut stream, _) = listener.accept().expect("mock server accepts request");
            let request = read_http_request(&mut stream);
            let response = if request.contains("\"stream\":true") {
                deepseek_streaming_response(content)
            } else {
                deepseek_json_response(content)
            };
            requests.push(request);
            stream
                .write_all(response.as_bytes())
                .expect("mock server writes response");
        }
        requests
    });

    MockDeepseek {
        base_url: format!("http://{address}"),
        handle,
    }
}

fn deepseek_json_response(content: DeepseekMockResponse) -> String {
    let choice = deepseek_choice(content);
    let body = serde_json::json!({
        "choices": [choice],
        "usage": deepseek_usage()
    })
    .to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

fn deepseek_streaming_response(content: DeepseekMockResponse) -> String {
    let delta = match content {
        DeepseekMockResponse::Text(content) => serde_json::json!({ "content": content }),
        DeepseekMockResponse::ToolCall {
            id,
            name,
            arguments,
        } => serde_json::json!({
            "tool_calls": [{
                "index": 0,
                "id": id,
                "function": {
                    "name": name,
                    "arguments": arguments.to_string()
                }
            }]
        }),
    };
    let content_event = serde_json::json!({
        "id": "mock-stream",
        "model": "deepseek-chat",
        "choices": [{ "delta": delta }]
    });
    let usage_event = serde_json::json!({
        "id": "mock-stream",
        "model": "deepseek-chat",
        "choices": [],
        "usage": deepseek_usage()
    });
    let body = format!("data: {content_event}\n\ndata: {usage_event}\n\ndata: [DONE]\n\n");
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ncontent-length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

fn deepseek_usage() -> serde_json::Value {
    serde_json::json!({
        "completion_tokens": 0,
        "prompt_tokens": 0,
        "prompt_cache_hit_tokens": 0,
        "prompt_cache_miss_tokens": 0,
        "total_tokens": 0
    })
}

fn deepseek_choice(response: DeepseekMockResponse) -> serde_json::Value {
    match response {
        DeepseekMockResponse::Text(content) => serde_json::json!({
            "finish_reason": "stop",
            "index": 0,
            "logprobs": null,
            "message": {
                "role": "assistant",
                "content": content
            }
        }),
        DeepseekMockResponse::ToolCall {
            id,
            name,
            arguments,
        } => serde_json::json!({
            "finish_reason": "tool_calls",
            "index": 0,
            "logprobs": null,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "arguments": arguments.to_string(),
                        "name": name
                    },
                    "id": id,
                    "index": 0,
                    "type": "function"
                }]
            }
        }),
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let bytes_read = stream.read(&mut buffer).expect("request is readable");
        if bytes_read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..bytes_read]);
        if headers_complete(&request)
            && request_body_len(&request) >= parse_content_length(&request).unwrap_or(0)
        {
            break;
        }
    }
    String::from_utf8(request).expect("request is utf-8")
}

fn headers_complete(request: &[u8]) -> bool {
    request.windows(4).any(|window| window == b"\r\n\r\n")
}

fn parse_content_length(request: &[u8]) -> Option<usize> {
    let request = String::from_utf8_lossy(request);
    request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

fn request_body_len(request: &[u8]) -> usize {
    request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(0, |header_end| request.len().saturating_sub(header_end + 4))
}

fn write_skill(root: &std::path::Path, name: &str, description: &str, content: &str) {
    let directory = root.join(".jux/skills").join(name);
    std::fs::create_dir_all(&directory).expect("skill directory is created");
    let skill = format!("---\nname: {name}\ndescription: {description}\n---\n{content}");
    std::fs::write(directory.join("SKILL.md"), skill).expect("skill file is written");
}
