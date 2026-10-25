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
`Justfile`, `mise.toml`, and `.ci/verification.json` define the commands and pinned tools.

## Changes

Engine code lives under `.template/tmpl/`.
Engine behavior changes need snapshot tests and property tests for affected dependency graph, manifest, render, state, or merge behavior.

Layers live under `.template/layers/<name>/`.
A layer change needs a rendered snapshot and updated layer documentation.
Run `just verify-template` after changing a layer or its capability relationships.

The shared policy lives under `tools/ci-policy/`.
Run the ordinary gate after changing its installation, command validation, or workflow contracts.

## Candidate verification

Production proofs run through the manual Candidate verification workflow from the default branch.
Supply a complete commit ID whose history includes the current default branch tip.
Qualification requires successful ordinary and proof results for that same commit.
An ordinary gate result doesn't establish a proof result.
The workflow doesn't publish artifacts.

## Pull requests

Use [Conventional Commits](https://www.conventionalcommits.org/) for commit subjects and pull request titles.
The pull request title becomes the squash commit subject.
Describe the change, relevant tests, and remaining verification limits.
Resolve supported review findings and pass required checks before merge.

## License

Contributions use the license expression `Apache-2.0 OR MIT`.
