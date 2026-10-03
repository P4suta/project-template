# project-template

A GitHub repository template with a Rust layer engine and a shared local and CI verification contract.

Create a repository with **Use this template**.
The initialization workflow produces an `initialized-project` artifact containing `initialized.patch`.
Apply it with `git apply --index initialized.patch`, then submit the generated project through your normal signed commit and pull request workflow.

To render locally:

```console
mise x -- cargo run --locked --release --manifest-path .template/tmpl/Cargo.toml -- --template-root .template --dest ../my-project apply --project-name my-project --project-owner OWNER
```

Select layers with `--layers`; inspect commands with `--help` and available layers in [.template/manifest.toml](.template/manifest.toml).
The engine rejects missing dependencies and conflicting capabilities before writing files, records generated-file hashes, and refuses to overwrite detected drift.

For development, install the pinned tools with `mise install` and `mise install rust@1.99.0`.
With the shared `ci-policy` command installed, run:

```console
mise x -- just check
mise x -- just check-staged
```

To install the policy command from this checkout:

```console
mise x rust@1.99.0 -- cargo install --locked --path .ci/ci-policy
```

[.ci/verification.json](.ci/verification.json) binds local commands to native CI jobs and separates commit checks from development checks.
Successful evidence is reused only for matching inputs, tools, environment, and platform.
See [ADRs](docs/adr/) for architectural decisions.

Licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT).
