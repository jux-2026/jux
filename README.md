# jux

`jux` is the open-source agent-side monorepo for Jux.

The current Phase 1.2 scope is intentionally small: establish a Rust workspace, a core crate, a CLI crate, and the basic engineering commands needed for later feature work.

## Repository Shape

`jux` is currently created as a local directory inside the private root repository. It is expected to become an independent open-source repository or submodule once the remote repository is ready.

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
make check
```

The direct Cargo equivalents are:

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Pre-commit Checks

Enable repository-managed Git hooks:

```sh
git config core.hooksPath .githooks
```

After that, every commit runs:

```sh
make check
```
