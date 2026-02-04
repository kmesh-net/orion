# Security Policy

## Supported Versions

The Orion Proxy team fully supports the most recent minor release train and will consider patches to
prior versions depending on prevalence of the affected version(s) and severity of the reported
issue. For example, if the most recent release is 0.1.5, we would provide a fix in a 0.1.x
release, but we might not backport the fix to earlier major versions.

Orion Proxy is a high-performance L7 proxy implemented in Rust, designed to support Envoy-compatible
configurations as well as Cloud and AI native networking scenarios.

## Reporting a Vulnerability

Please do not report vulnerabilities via public issue reports or pull requests. Instead, report
vulnerabilities via GitHub's private [report a vulnerability](https://github.com/kmesh-net/orion/security/advisories/new)
link. The Orion Proxy team will make every effort to respond to vulnerability disclosures within 5
working days. After initial triage, we will work with the reporting researcher on a disclosure
time-frame and mutually agreeable embargo date, taking into account the work needed to:

  * Identify affected versions
  * Prepare a fix and regression test
  * Coordinate response with the Envoy community (if the issue affects Envoy as well)
  * Coordinate response with downstream projects in the Kmesh ecosystem

After testing a fix and upon the end of the embargo date we will:

* Submit an advisory to [rustsec/advisory-db](https://github.com/RustSec/advisory-db)
* Publish fixed releases on crates.io and deprecate prior releases as appropriate
* Publish release notes and security advisories on the [GitHub Releases](https://github.com/kmesh-net/orion/releases) page
* Notify the Kmesh community via official communication channels
