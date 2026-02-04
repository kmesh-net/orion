# Contributing to Orion Proxy

Before contributing, please consider the terms of the license (Apache License 2.0). We chose this license for two reasons:

- To be more compatible with the general Rust ecosystem
- So that this software can be liberally used with few restrictions

After ensuring the license options are compatible with the aims of the contribution, then please submit your PR for review and we will review as soon as possible. My only ask is that you do not do this for free, unless it's something that is a passion or learning project for you. Please, find a way to be paid for your work. You are worth it.

## Understanding the design

Please familiarize yourself with the overall architecture and design of Orion Proxy by reviewing the codebase and examples. Orion is designed as a high-performance L7 proxy compatible with Envoy's xDS configuration format while providing superior performance through Rust's memory safety guarantees.

## Submitting PRs

Before submitting a PR it would be good to discuss the change in an issue so as to avoid wasted work, also feel free to reach out via GitHub Discussions. Please, consider keep PRs focused on one issue at a time. While issues are not required for a PR to be accepted they are encouraged, especially for anything that would change behavior, change an API, or be a medium to large change.

When submitting PRs please keep refactoring commits separate from functional change commits. Breaking up the PR into multiple commits such that a reviewer can follow the change improves the review experience. This is not necessary, but can make it easier for a reviewer to follow the changes and will result in PRs getting merged more quickly.

### Test policy

All PRs *must* be passing all tests. Ideally any PR submitted should have more than 85% code coverage, but this is not mandated.

## Performing a Release, for Maintainers

Releases are somewhat automated. The github action, `publish`, watches for any tags on the project. It then attempts to perform a release of all the libraries, this does not always work, for various reasons.

1. Create a new branch like `git checkout -b prepare-0.1.6`
1. Update all Cargo.toml files to the new version, `version = 0.1.6`
1. Update dependencies, `cargo update`
1. Update all inter-dependent crates, i.e. orion-core to use the new versions
1. Push to Github, create a PR and merge in `main` or the target release branch.
1. Go to [Releases](https://github.com/kmesh-net/orion/releases) and `Draft a new release`
1. Give it a `Tag Version` of `vX.x.x`, e.g. `v0.1.6`, *make sure this is tagging the correct branch, e.g. `main` or `release/0.1`*
1. Give it a  `Release Title` of something key to the release
1. Generate release notes
1. `Publish Release`, this will kick off the publish workflow

After approximately 45 minutes it should be published. This may fail.

**TBD**: add instructions to skip already published crates

## FAQ

- Why are there so few maintainers?

There have not been that many people familiar with proxy internals, networking, security, and Rust that the list of maintainers has been relatively small.

- Will new maintainers be considered?

Yes! There is no formal process, and generally it's a goal to open up to anyone who's been committing regularly to the project. We'd ask that you are committed to the goals of an open high-performance proxy implementation that anyone can freely use as they see fit. Please reach out via GitHub Discussions if you'd like to become a maintainer and discuss with us.

## Thank you!

Seriously, thank you for contributing to this project. Orion Proxy would not be where it is today without the support of contributors like you.
