.PHONY: build test fmt lint quick-check check

build:
	cargo build --workspace

test:
	cargo test --workspace

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings

quick-check: fmt lint

check: quick-check test
