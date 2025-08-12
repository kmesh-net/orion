# Orion Makefile
# Simple development tasks for the Orion project

.PHONY: all build build-release check ci clean dev-setup docker docs format format-check help info install lint test test-verbose update

# Default target
all: check test build

# Display available targets
help:
	@echo "Available targets:"
	@echo "  all            - Run check, test, and build (default)"
	@echo "  build          - Build the project in debug mode"
	@echo "  build-release  - Build the project in release mode"
	@echo "  check          - Run cargo check"
	@echo "  ci             - Run all CI checks (format-check, lint, test, build, build-release)"
	@echo "  clean          - Clean build artifacts"
	@echo "  dev-setup      - Install development dependencies"
	@echo "  docker         - Build Docker image"
	@echo "  docs           - Generate and open documentation"
	@echo "  format         - Format code with rustfmt"
	@echo "  format-check   - Check if code is formatted correctly"
	@echo "  info           - Show project and build information"
	@echo "  install        - Install the binaries"
	@echo "  lint           - Run clippy linter"
	@echo "  test           - Run all tests"
	@echo "  test-verbose   - Run all tests with verbose output"
	@echo "  update         - Update cargo dependencies"

# Build targets
build:
	cargo build

build-release:
	cargo build --release

# Test targets
test:
	cargo test

test-verbose:
	cargo test --verbose

# Quality targets
check:
	cargo check --all-targets

lint:
	cargo clippy --all-targets

format:
	cargo fmt --all

format-check:
	cargo fmt --all -- --check

# Maintenance targets
clean:
	cargo clean

install:
	cargo install --path orion-proxy

# Documentation
docs:
	cargo doc --open

# Docker
docker:
	docker build -f docker/Dockerfile -t orion:latest .

# CI simulation - run all checks that would run in CI
ci: format-check lint test build build-release

# Development helpers
dev-setup:
	@echo "Installing development dependencies..."
	@rustup component add rustfmt clippy
	@echo "Development environment setup complete!"

# Update dependencies
update:
	cargo update

# Show project information
info:
	@echo "Project: Orion - Next Generation Cloud Native Proxy"
	@echo "Build information:"
	@rustc --version
	@cargo --version
