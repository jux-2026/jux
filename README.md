# jux

`jux` is the open-source agent-side monorepo for Jux.

The current scope provides a small local runtime foundation: a Rust workspace,
a core crate, a CLI crate, SQLite-backed local state, and a minimal LLM-backed
`jux run` command.

## Repository Shape

`jux` is an independent open-source repository. In the private root project, it is consumed as a Git submodule.

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

## Commands

```sh
make build
make test
make fmt
make lint
make quick-check
make check
```

The direct Cargo equivalents are:

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`make quick-check` runs format and lint checks. `make check` runs the full local quality gate: format, lint, and tests.

## Git Hooks

Enable repository-managed Git hooks:

```sh
git config core.hooksPath .githooks
```

After that, every commit runs the fast local gate:

```sh
make quick-check
```

Every push runs the full local gate:

```sh
make check
```

## Minimal Run Command

Run a request through the minimal local agent loop:

```sh
export JUX_DEEPSEEK_API_KEY="..."
cargo run -p jux-cli -- run "Explain this project" --workspace /path/to/workspace
```

Structured output is available from the top-level `--output` option:

```sh
cargo run -p jux-cli -- --output json run "Explain this project"
cargo run -p jux-cli -- --output yaml run "Explain this project"
```

The command initializes `.jux/state.db`, creates the active Workspace and
Session when needed, creates a Run, records the current Step timeline, calls the
configured LLM provider, and stores the final status.
