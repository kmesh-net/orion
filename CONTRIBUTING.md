# Contributing to Orion Proxy

Before contributing, please review the terms of our license ([Apache License 2.0](./LICENSE)). We chose this license for two main reasons:

- To align with the broader Rust ecosystem and encourage open collaboration
- To enable liberal use with minimal restrictions for Cloud and AI native networking scenarios

After ensuring the license is compatible with your contribution goals, please submit your PR for review. We will review it as soon as possible. My only ask is that you don't do this for free unless it's a passion or learning project for you. Please find a way to be paid for your work. You are worth it.

## Understanding the Design

Orion Proxy is a high-performance L7 proxy implemented in Rust, designed to be compatible with Envoy's xDS configuration while providing 2-4x better performance. Before contributing, please familiarize yourself with:

- **Architecture**: The overall proxy architecture and component design
- **Performance Goals**: Our commitment to maintaining high performance and memory safety
- **Envoy Compatibility**: How we maintain compatibility with Envoy's xDS protobuf definitions
- **Kmesh Integration**: How Orion fits into the broader Kmesh ecosystem

For technical details, refer to the [Performance Documentation](docs/performance/performance.md) and examples in the `examples/` directory.

## Submitting PRs

Before submitting a PR, it's highly recommended to discuss the proposed change in an issue to avoid duplicated or wasted work. Feel free to reach out via GitHub Discussions or issues. Please keep PRs focused on one issue at a time whenever possible.

While issues are not strictly required for a PR to be accepted, they are strongly encouraged, especially for:
- Behavioral changes
- API modifications
- Medium to large changes
- Performance-impacting changes

### Best Practices for PRs

- **Separate commits**: Keep refactoring commits separate from functional change commits
- **Logical progression**: Structure commits so reviewers can follow the evolution of changes
- **Descriptive messages**: Write clear commit messages explaining the "why" behind changes
- **Performance considerations**: If your change affects performance, include benchmark results
- **Breaking changes**: Clearly document any breaking changes in the PR description

These practices aren't mandatory but will help your PRs get reviewed and merged more quickly.

## Test Policy

All PRs **must** pass all tests before merging. We aim for high code coverage:

- **Target coverage**: Ideally 85%+ for new code
- **Required tests**: Unit tests for new functionality
- **Integration tests**: For changes affecting proxy behavior
- **Performance tests**: For performance-sensitive changes (see `docs/performance/`)

When tests fail, especially on older branches, this may be due to:
- Expired test certificates (for TLS tests)
- Environment-specific issues
- Version-specific dependencies

See [Testing Guidelines](#testing-guidelines) below for more details.

## Releases

Orion Proxy follows [semantic versioning](https://semver.org/) principles:

- **Major versions** (`1.0.0`, `2.0.0`): Breaking API changes
- **Minor versions** (`0.x.0`): New features, backwards-compatible
- **Patch versions** (`0.0.x`): Bug fixes and minor improvements

**Important**: Until we reach `1.0.0`, all `0.x.0` minor releases may include breaking changes.

Releases are performed on an as-needed basis when significant features or fixes are ready.

### Maintaining Release Branches

*For Maintainers*: If changes are needed for previous releases, create a `release/x.x` branch from the corresponding tag:

```bash
git fetch origin
git checkout v0.1.5
git branch release/0.1
git push --set-upstream origin release/0.1
```

## Performing a Release (Maintainers Only)

1. Create a new branch: `git checkout -b prepare-0.x.x`
2. Update all `Cargo.toml` files to the new version: `version = "0.x.x"`
3. Update dependencies: `cargo update`
4. Update inter-dependent crates to use the new version
5. Run all tests: `cargo test --all-features`
6. Run performance benchmarks to ensure no regressions
7. Push to GitHub, create a PR, and merge into `main` or the target release branch
8. Create a new release on [GitHub Releases](https://github.com/kmesh-net/orion/releases)
   - Tag version: `vX.x.x` (e.g., `v0.1.6`)
   - Target branch: `main` or appropriate release branch
   - Generate release notes
   - Highlight breaking changes and new features
9. Publish the release

After publication, verify the release works correctly:
- Test Docker images
- Verify crates.io publication (if applicable)
- Update documentation if needed

## Testing Guidelines

### Running Tests

```bash
# Run all tests
cargo test --all-features

# Run specific test suite
cargo test --package orion-core

# Run with logging
RUST_LOG=debug cargo test

# Run integration tests
cargo test --test integration_tests
```

### Performance Testing

Before submitting performance-sensitive changes, run benchmarks:

```bash
# See docs/performance/performance.md for detailed instructions
cd benchmarks
./run_benchmarks.sh
```

Include benchmark results in your PR description if your changes affect performance.

### TLS/Security Testing

If TLS tests are failing due to expired certificates:

```bash
# Regenerate test certificates (instructions TBD)
# See examples/tlv-filter-demo for TLV-specific testing
cd examples/tlv-filter-demo
./test_tlv_config.sh
```

## Development Environment Setup

### Prerequisites

- Rust stable (latest)
- Git with submodule support
- Docker (for integration tests)
- `cargo-fmt`, `cargo-clippy` (recommended)

### Initial Setup

```bash
git clone https://github.com/kmesh-net/orion
cd orion
git submodule init
git submodule update --force
cargo build
```

### Code Quality Tools

Before submitting PRs, run:

```bash
# Format code
cargo fmt --all

# Run clippy lints
cargo clippy --all-targets --all-features -- -D warnings

# Check for common issues
cargo check --all-features
```

### Configuring rust-analyzer

For optimal IDE support, configure rust-analyzer for the workspace. If using VS Code, create `.vscode/settings.json`:

```jsonc
{
    "rust-analyzer.cargo.features": "all",
    "rust-analyzer.checkOnSave.command": "clippy",
    "rust-analyzer.rustfmt.extraArgs": ["+nightly"]
}
```

## Docker Development

Build and test with Docker:

```bash
# Build image
docker build -t orion-proxy -f docker/Dockerfile .

# Run with test backends
docker run -d -p 4001:80 --name backend1 nginx:alpine
docker run -d -p 4002:80 --name backend2 nginx:alpine
docker run -d --network host --name orion-proxy orion-proxy

# Test
curl http://localhost:8000/

# Cleanup
docker rm -f backend1 backend2 orion-proxy
```

See [docker/README.md](docker/README.md) for more details.

## Code Style and Guidelines

- **Follow Rust conventions**: Use `cargo fmt` and `cargo clippy`
- **Document public APIs**: All public functions and types should have doc comments
- **Error handling**: Use `Result` types and avoid panics in production code
- **Performance**: Be mindful of allocations and use zero-copy where possible
- **Safety**: Minimize `unsafe` code; when needed, document safety invariants
- **Testing**: Write tests for new features and bug fixes

## FAQ

### Why focus on Envoy compatibility?

Envoy is the de facto standard for cloud-native proxies. By maintaining compatibility, we enable users to migrate to Orion with minimal friction while gaining 2-4x performance improvements.

### Why Rust?

Rust provides memory safety without garbage collection, enabling both high performance and security—critical for a proxy handling production traffic.

### Will new maintainers be considered?

Yes! We welcome anyone who has been actively contributing to the project. We look for contributors who:
- Understand proxy/networking fundamentals
- Are committed to open-source principles
- Can dedicate time to code review and maintenance
- Share our vision for high-performance, secure networking

Please reach out via GitHub Discussions if you're interested in becoming a maintainer.

### How can I get help?

- **Issues**: For bugs and feature requests
- **Discussions**: For questions and design discussions
- **Examples**: Check `examples/` directory for working demos

### What's the relationship with Kmesh?

Orion Proxy is part of the [Kmesh](https://github.com/kmesh-net) ecosystem, which provides eBPF-based service mesh solutions. Orion serves as a high-performance L7 proxy component within this ecosystem.

## Community Guidelines

We are committed to fostering an open and welcoming community. Please:

- Be respectful and inclusive in all interactions
- Provide constructive feedback
- Focus on what's best for the project and users
- Help newcomers get started
- Report any unacceptable behavior to project maintainers

## Thank You!

Thank you for contributing to Orion Proxy! Whether you're fixing bugs, adding features, improving documentation, or helping with reviews, your contributions make this project better. We're building something meaningful together—a high-performance, memory-safe proxy that pushes the boundaries of what's possible in cloud and AI native networking.

Every contribution matters. Welcome to the team! 🚀
