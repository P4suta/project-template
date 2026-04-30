# Security policy

## Reporting a vulnerability

Please report security issues privately via GitHub's
[private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
on this repository.

If GitHub's reporter is unavailable, contact the maintainer listed in
[`.github/CODEOWNERS`](.github/CODEOWNERS).

We aim to acknowledge receipt within 72 hours. Coordinated disclosure
windows are agreed case-by-case based on severity and downstream impact.

## Scope

The `tmpl` engine and the layer assets under `.template/layers/`. Issues
in third-party crates depended on by the engine should be reported
upstream; we will track and update.

## Supply chain

- Dependencies are pinned via `Cargo.lock` (committed) and bumped weekly
  by Dependabot. We do not pre-emptively pin to specific patch versions
  in `Cargo.toml` — `cargo update` is the regular cadence.
- GitHub Actions are pinned to commit SHAs and bumped by Dependabot.
- Releases of the engine are not currently published to crates.io; the
  engine is built in-tree from source whenever a generated repository
  invokes `tmpl apply`.
