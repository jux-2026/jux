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

## Install

Prebuilt binaries are published through GitHub Releases for Apple Silicon
macOS, x86_64 Linux, and x86_64 Windows.

Install on macOS or Linux with the shell installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/jux-2026/jux/releases/latest/download/jux-installer.sh | sh
```

Install on Windows with PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/jux-2026/jux/releases/latest/download/jux-installer.ps1 | iex"
```

Release archives and SHA-256 checksums can also be downloaded directly from
the [GitHub Releases](https://github.com/jux-2026/jux/releases) page.

Verify the installation with:

```sh
jux --version
```

Check for a newer version and display the upgrade method for the embedded
distribution channel:

```sh
jux update --check
```

The TUI performs the same check in the background at most once every 24 hours.
An available version is shown on the next startup and at the bottom of the
right sidebar. Jux recommends a fixed package-manager or installer command; it
does not execute that command automatically.

## Release Process

Releases use cargo-dist inside GitHub Actions. Each platform is compiled once,
then a release step injects a fixed 1 KiB distribution metadata slot before
the final archive, signature, and checksum are produced. A tag whose version
matches the Cargo workspace version publishes the platform archives,
checksums, and both installers:

```sh
git tag v0.1.0
git push origin v0.1.0
```

Before pushing a release tag, run the full quality gate and inspect the release
plan:

```sh
make check
make release-plan
```

## Commands

```sh
make build
make test
make fmt
make lint
make quick-check
make check
make release-plan
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
cargo run -p jux -- run "Explain this project" --workspace /path/to/workspace
```

Structured output is available from the top-level `--output` option:

```sh
cargo run -p jux -- --output json run "Explain this project"
cargo run -p jux -- --output yaml run "Explain this project"
```

The command initializes `.jux/state.db`, creates the active Workspace and
Session when needed, creates a Run, records the current Step timeline, calls the
configured LLM provider, and stores the final status.
