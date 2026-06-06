# AGENTS.md

This file defines the local collaboration rules for `jux`.

`jux` is an open-source Rust workspace for the agent-side runtime and clients. Public-facing repository content must be written in English, including code comments, README files, API docs, error messages, examples, and developer documentation.

## Current Scope

The current runtime foundation includes:

- Rust workspace root.
- `jux-core` library crate.
- `jux-cli` binary crate.
- Basic build, test, format, lint, and Git hook quality commands.
- Minimal Workspace, Session, Run, and Step concepts.
- SQLite-backed local state under the workspace `.jux` directory.
- Minimal `jux run` command backed by Rig and the DeepSeek provider.

Do not add policy, patch review, tool execution, approvals, resume, MCP, Skill, TUI, or Tauri behavior until the roadmap and runtime design are updated first.

## Directory Structure

```text
jux
├── crates
│   ├── jux-core
│   └── jux-cli
├── AGENTS.md
├── Cargo.toml
├── Makefile
└── README.md
```

- `crates/jux-core`: Core domain library. It must not depend on CLI, TUI, or Tauri presentation layers.
- `crates/jux-cli`: CLI adapter. It may depend on `jux-core`, but core business logic should remain in `jux-core`.

Future client crates should keep the same boundary:

- `crates/jux-client-tui`
- `crates/jux-client-tauri`
- `crates/jux-client-tauri/jux-client-webui`

## Commands

Run commands from the `jux` directory.

```sh
make build
make test
make fmt
make lint
make quick-check
make check
```

Equivalent direct commands:

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`make quick-check` is the default pre-commit quality gate. It runs format checks and lint checks.

`make check` is the full quality gate for pre-push and merge readiness. It runs format checks, lint checks, and tests.

The repository stores Git hooks in `.githooks`. Enable them in a checkout with:

```sh
git config core.hooksPath .githooks
```

## Engineering Rules

- Keep core types strongly typed and explicit.
- Do not pass business data through unstructured JSON, dynamic maps, or string protocols.
- Keep the CLI crate as an adapter over core behavior.
- Prefer small, verifiable changes with tests.
- Keep unit tests in a `tests.rs` file in the same directory as the code under test.
- Use `tracing` for diagnostics and structured logs. Initialize `tracing-subscriber` in application crates, keep logs on stderr, and keep user-facing command output on stdout.
- Keep `make quick-check` passing before committing.
- Keep `make check` passing before pushing or merging.
- Keep public documentation and messages in English.
