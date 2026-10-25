# Contributing

The repository contains the template engine, reusable layers, and shared verification policy.
Keep changes focused and explain the resulting behavior in the pull request.

## Development

Install the pinned tools with `just bootstrap` and register the Git hooks with `just hooks`.
The maintained commands use mise and native tools.
Just recipes require Bash.

Run `mise x -- just check` for ordinary verification.
The gate checks formatting, spelling, Clippy, tests, dependencies, and template generation.
Coverage runs on Linux through `just coverage` and hosted CI.
The commit hook runs `just check-staged`, and the push hook runs the ordinary gate.

Use `just fmt` to format Rust, `just test` for engine tests, and `just verify-template` to validate generation.
The commands and their pinned tools are defined in `Justfile`, `mise.toml`, and `.ci/verification.json`.

## Changes

Engine code lives under `.template/tmpl/`.
Engine behavior changes need snapshot tests and property tests for affected DAG, manifest, render, state, or merge behavior.

Layers live under `.template/layers/<name>/`.
A layer change needs a rendered snapshot and updated layer documentation.
Run `just verify-template` after changing a layer or its capability relationships.

The shared policy lives under `tools/ci-policy/`.
Run the ordinary gate after changing its installation, command validation, or workflow contracts.

## Candidate verification

Production proofs run through the manual Candidate verification workflow from the default branch.
Supply a complete commit ID containing the current default branch.
Qualification requires successful ordinary and proof results for that same commit.
An ordinary gate result doesn't establish a proof result.
The workflow doesn't publish artifacts.

## Pull requests

Use [Conventional Commits](https://www.conventionalcommits.org/) for commit subjects and pull request titles.
The pull request title becomes the squash commit subject.
Describe the change, relevant tests, and remaining verification limits.
Required checks and supported review findings must be resolved before merge.

## License

Contributions are dual-licensed under Apache-2.0 OR MIT.
