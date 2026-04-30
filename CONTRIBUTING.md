# Contributing

Thank you for considering a contribution to project-template. The notes
below cover the parts that are easy to get wrong on a first pass.

## Scope of this repository

This repository is the template *and* the engine that generates from it.
Two distinct kinds of change live here:

- **Engine changes** under `.template/tmpl/` — modifying the Rust crate
  that resolves the layer DAG, renders templates, and writes them out.
- **Layer changes** under `.template/layers/<name>/` — adding a layer,
  adjusting an existing layer's templates or metadata, or fixing the
  capability graph.

Engine changes need a snapshot test and (where the change touches the
DAG, manifest, render, state, or merge modules) a property test.
Layer changes need at minimum an `insta` snapshot of the rendered output
and an entry in the layer's own `README.md`.

## Development environment

The Day-1 expectation is Docker-only execution: every cargo / mdbook /
node invocation is wrapped in a `just` task that proxies into the dev
container. Direct host-toolchain calls are intentionally not part of the
documented workflow.

```sh
just bootstrap        # mise install + lefthook install
just lint             # fmt-check + clippy + typos + strict-code grep
just test             # cargo nextest run
just coverage         # cargo llvm-cov, gate at the configured threshold
just verify-template  # tmpl verify (manifest + DAG soundness)
```

`just hooks` installs the lefthook pre-commit / commit-msg / pre-push
hooks. Anything that would fail in CI fails locally first.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/). The
commit-msg hook enforces the type prefix; scope is optional. Bump types:
`feat` / `fix` / `docs` / `style` / `refactor` / `perf` / `test` /
`build` / `ci` / `chore` / `revert`.

## Pull requests

- Branch off `main`, push, open a PR.
- Squash-merge by default. The PR title becomes the squashed commit
  message — write it as a Conventional Commit subject.
- CI must be green; reviewer approval required.

## Adding a layer

1. Create `.template/layers/<name>/layer.toml` declaring `name`,
   `description`, `requires`, `provides`, `conflictsWith`.
2. Drop your tera/jinja templates under `.template/layers/<name>/files/`.
3. Add a snapshot test under `.template/tmpl/tests/golden/<name>.rs`
   (use `insta` and a fixture context).
4. Run `just verify-template` to confirm the layer slots into the DAG
   without conflicts.
5. Document the layer in this repository's `README.md` catalogue table
   and in the layer's own `README.md`.

## License

By contributing you agree that your contribution is dual-licensed under
Apache-2.0 OR MIT, the same terms as the project itself.
