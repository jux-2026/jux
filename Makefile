.PHONY: build test fmt lint quick-check check release-plan

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

release-plan:
	dist plan
	@grep -q "brand-dist-archive.sh" .github/workflows/v-release.yml
	@grep -q "brand-dist-archive.ps1" .github/workflows/v-release.yml
