# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/).

## [Unreleased]

### Added

- `tmpl applied-files [--null]` subcommand: prints the file paths
  recorded in `.template/state.toml`. Used by `init.yml` to drive
  selective `git rm`, and available to downstream tooling that needs
  the rendered-files whitelist without parsing TOML in shell.
- Initial scaffold of the layer-DAG `tmpl` engine and Phase A layer set.

### Changed

- The `dependabot-actions` layer is replaced by `renovate`, which provides `deps-bot` and ships a `renovate.json` extending the shared `github>P4suta/renovate-config` preset.
  `rust-workspace` and `typescript-package` no longer ship `.github/dependabot.yml`, since Renovate finds cargo, npm and Dockerfile dependencies on its own.
  The `conventional-commits` layer's commitlint workflow now skips PRs from every bot account, not just Dependabot.
- `init.yml` now graduates the templated repository in a single run:
  after `tmpl apply`, every tracked file not rendered by a layer is removed and the entire `.template/` tree is deleted.
  The resulting repo contains only the layered output — engine, layer sources, this workflow, and `tmpl-verify.yml` are all gone after the initial commit.
  The gate now checks for the engine's presence rather than `state.toml` existence.

### Fixed

- The core layer's `.gitignore` excludes `.template/tmpl/target/` as
  defence-in-depth against engine build artefacts ever being staged.
  The new prune step in `init.yml` removes `.template/` entirely
  before the initial commit, so templated repositories no longer
  inherit ~1100 cargo build artefacts (regression observed in
  slot-booking-system on 2026-05-05).
