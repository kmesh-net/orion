lint:
	cargo clippy --all -- -D warnings
build:
	cargo build --all --release

.PHONY: fmt fmt-check lint build test ci

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

build:
	cargo build --workspace --release --locked

test:
	cargo test --workspace --release --locked

ci: fmt-check lint build test
