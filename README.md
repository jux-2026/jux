# Jux

[English](README.md) | [简体中文](docs/zh-CN/README.md)

> [!WARNING]
> Jux is in an early stage of development. Many planned features are incomplete,
> behavior and interfaces may change without notice, and the project has not yet
> received the security hardening required for production use. **Do not use Jux
> in production environments.**

Jux is an open-source, security-oriented programming agent that runs supported
command-line tools inside a WebAssembly sandbox by default. It is being built to
provide auditable, controllable, and extensible AI-assisted software development
through a local runtime and multiple client interfaces.

## Sandboxed by Default

Jux treats agent-generated commands as untrusted input. Supported command-line
operations run inside an embedded WebAssembly sandbox instead of being executed
directly by the host shell.

The default runtime policy provides the following boundaries:

- Native host command execution is disabled.
- The workspace is exposed to sandboxed commands as read-only.
- Host environment variables are not forwarded.
- Network access is disabled unless explicitly allowed by policy.
- Only packaged and approved WASM tools can be executed.
- Shell syntax such as pipes, redirects, command substitution, and chained
  commands is rejected.
- Commands invoked through the restricted Lua runtime use the same WASM-backed
  execution path.

These boundaries reduce the impact of unexpected model-generated commands and
make tool execution easier to control and audit. They do not make running
untrusted code risk-free, and Jux is not yet recommended for production use.

## Project Status

Jux is currently a work in progress rather than a finished product. The existing
implementation is suitable for development, experimentation, and early feedback,
but not for production workloads or sensitive repositories.

The project does not yet have public product documentation. Product concepts,
use cases, and complete user workflows will be documented in a future release.
Until then, this README describes only the capabilities that already exist in the
repository.

## Current Capabilities

- A Rust-based local agent runtime and CLI.
- A terminal user interface built with Ratatui.
- Workspace, session, run, and step lifecycle management.
- SQLite-backed local state stored under the workspace `.jux` directory.
- Multi-iteration LLM execution through the DeepSeek provider.
- WASM-sandboxed command execution with native host commands disabled by default.
- A restricted Lua runtime whose command operations use the same WASM sandbox.
- Skill discovery, explicit or model-selected invocation, isolated transcripts,
  and resume support.
- Persisted human clarification and confirmation flows.
- Session timelines, cancellation, and code-change review in the TUI.
- Parallel task execution, context management, streaming events, and Plan mode
  foundations that are still evolving.

This list is not a stability guarantee. Some capabilities remain partial and may
be redesigned as the runtime matures.

## Documentation

English is the default documentation language. Translations are organized in
language-specific directories under `docs/`:

- [简体中文文档](docs/zh-CN/README.md)

Public product documentation is not available yet and will be added as the
product definition matures.

## Repository Structure

```text
jux
├── crates
│   ├── jux-core
│   └── jux-cli
├── docs
│   └── zh-CN
├── AGENTS.md
├── Cargo.toml
├── Makefile
└── README.md
```

- `crates/jux-core` contains the core domain model and runtime behavior.
- `crates/jux-cli` contains the `jux` binary, CLI adapter, and TUI client.

## Installation

Prebuilt binaries are published through GitHub Releases for Apple Silicon macOS,
x86_64 Linux, and x86_64 Windows. These packages are development previews and
carry the same non-production warning as the source repository.

Install on macOS or Linux with the shell installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/jux-2026/jux/releases/latest/download/jux-installer.sh | sh
```

Install on Windows with PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/jux-2026/jux/releases/latest/download/jux-installer.ps1 | iex"
```

Install globally with npm:

```sh
npm install --global @jux-2026/jux
```

Run a temporary npm installation without adding it globally:

```sh
npx @jux-2026/jux
```

Release archives and SHA-256 checksums can also be downloaded from
[GitHub Releases](https://github.com/jux-2026/jux/releases).

Verify the installation:

```sh
jux --version
```

Check for a newer version and display the upgrade method associated with the
embedded distribution channel:

```sh
jux update --check
```

The TUI checks for updates in the background at most once every 24 hours. Jux
only recommends the appropriate update command; it does not execute that command
automatically.

## Quick Start

Set a DeepSeek API key and run a request in a local workspace:

```sh
export JUX_DEEPSEEK_API_KEY="..."
jux run "Explain this project" --workspace /path/to/workspace
```

Structured command output is available through the top-level `--output` option:

```sh
jux --output json run "Explain this project"
jux --output yaml run "Explain this project"
```

Running Jux initializes `.jux/state.db` in the selected workspace. Review the
permissions and generated changes carefully, especially when experimenting with
tools that can execute commands or modify files.

## Development

The workspace requires Rust 1.91 or later. Run commands from the repository root:

```sh
make build
make test
make fmt
make lint
make quick-check
make check
```

`make quick-check` runs formatting and lint checks. `make check` runs the full
local quality gate: formatting, linting, and tests.

Enable the repository-managed Git hooks with:

```sh
git config core.hooksPath .githooks
```

## License

Jux is available under the [MIT License](LICENSE).
