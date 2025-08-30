.PHONY: fmt fmt-check lint build test ci ci-parallel init docker-build

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets --all-features

build:
	cargo build --workspace --release --locked

test:
	cargo test --workspace --release --locked

# Sequential CI (keep for local/dev reproducibility)
ci: init
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features
	cargo build --workspace --release --locked
	cargo test --workspace --release --locked

# Parallel CI: run fmt-check, lint and build concurrently; if build succeeds, run tests.
ci-parallel: init
	@echo "Starting parallel CI: fmt-check, lint, build"
	@bash -ec '\
	set -o pipefail; \
	cargo fmt --all -- --check & pid_fmt=$$!; \
	cargo clippy --all-targets --all-features & pid_clippy=$$!; \
	cargo build --workspace --release --locked & pid_build=$$!; \
	wait $$pid_fmt; rc_fmt=$$?; \
	wait $$pid_clippy; rc_clippy=$$?; \
	wait $$pid_build; rc_build=$$?; \
	if [ $$rc_fmt -ne 0 ]; then echo ">>> fmt-check failed (exit $$rc_fmt)"; fi; \
	if [ $$rc_clippy -ne 0 ]; then echo ">>> clippy failed (exit $$rc_clippy)"; fi; \
	if [ $$rc_build -ne 0 ]; then echo ">>> build failed (exit $$rc_build)"; fi; \
	# Fail fast if any of the three failed (so PR check fails with non-zero code) \
	if [ $$rc_fmt -ne 0 ] || [ $$rc_clippy -ne 0 ] || [ $$rc_build -ne 0 ]; then exit 1; fi; \
	# If we reach here, build succeeded, so run tests \
	cargo test --workspace --release --locked; \
	'

init:
	@echo "Initializing git submodules..."
	@git submodule init
	@git submodule update --recursive

docker-build: init
	@echo "Building Docker image: orion-proxy"
	docker build -t orion-proxy -f docker/Dockerfile .
