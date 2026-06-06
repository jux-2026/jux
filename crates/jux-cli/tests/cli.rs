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
        .stdout(predicate::str::contains("session"));
}

#[test]
fn run_command_executes_mocked_llm_and_persists_state() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock(r#"{"type":"final_answer","answer":"Mocked Jux answer"}"#);

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
        .stdout(predicate::str::contains("UserRequest").not())
        .stdout(predicate::str::contains("LlmCall").not());

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
fn run_command_can_output_json() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock(r#"{"type":"final_answer","answer":"JSON answer"}"#);

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
    assert_eq!(output["steps"][0]["kind"], "UserRequest");
    assert_eq!(
        output["steps"][0]["payload"]["UserRequest"]["content"],
        "Return JSON output"
    );
    assert!(output["steps"][0]["created_at"].as_u64().is_some());
    assert!(output["steps"][0]["updated_at"].as_u64().is_some());
    assert!(output["steps"][0].get("status").is_none());

    let requests = mock.join();
    assert_eq!(requests.len(), 1);
}

#[test]
fn run_command_can_output_yaml() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock(r#"{"type":"final_answer","answer":"YAML answer"}"#);

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
        .stdout(predicate::str::contains("kind: UserRequest"))
        .stdout(predicate::str::contains("payload:"));

    let requests = mock.join();
    assert_eq!(requests.len(), 1);
}

#[test]
fn run_command_executes_mocked_tool_call_loop() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock_sequence([
        r#"{"type":"tool_call","tool_name":"echo","input":"cli tool result"}"#,
        r#"{"type":"final_answer","answer":"Final answer after tool"}"#,
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
        .stdout(predicate::str::contains("AssistantToolCall").not())
        .stdout(predicate::str::contains("ToolResult").not());

    let requests = mock.join();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("Use a tool"));
    assert!(requests[1].contains("Tool echo: cli tool result"));
}

#[test]
fn session_show_outputs_active_session_state() {
    let workspace = TempDir::new().expect("temp workspace exists");
    let mock = start_deepseek_mock(r#"{"type":"final_answer","answer":"Session answer"}"#);

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
    assert_eq!(output["runs"][0]["request"], "Create session state");
    assert_eq!(output["runs"][0]["status"], "Completed");
    assert!(output.get("steps").is_none());
    assert_eq!(output["runs"][0]["steps"][0]["kind"], "UserRequest");
    assert_eq!(output["runs"][0]["steps"][1]["kind"], "LlmCall");
    assert_eq!(output["runs"][0]["steps"][2]["kind"], "AssistantMessage");
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

fn start_deepseek_mock(content: &str) -> MockDeepseek {
    start_deepseek_mock_sequence([content])
}

fn start_deepseek_mock_sequence<'a>(contents: impl IntoIterator<Item = &'a str>) -> MockDeepseek {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock server binds");
    let address = listener.local_addr().expect("mock server has address");
    let contents = contents
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for content in contents {
            let (mut stream, _) = listener.accept().expect("mock server accepts request");
            requests.push(read_http_request(&mut stream));
            let body = serde_json::json!({
                "choices": [{
                    "finish_reason": "stop",
                    "index": 0,
                    "logprobs": null,
                    "message": {
                        "role": "assistant",
                        "content": content
                    }
                }],
                "usage": {
                    "completion_tokens": 0,
                    "prompt_tokens": 0,
                    "prompt_cache_hit_tokens": 0,
                    "prompt_cache_miss_tokens": 0,
                    "total_tokens": 0
                }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
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
