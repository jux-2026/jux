# jux

`jux` is the open-source agent-side monorepo for Jux.

The current Phase 1.2 scope is intentionally small: establish a Rust workspace, a core crate, a CLI crate, and the basic engineering commands needed for later feature work.

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
