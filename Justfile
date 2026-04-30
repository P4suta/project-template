set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set dotenv-load := false

# Cargo manifests
ENGINE_MANIFEST := ".template/tmpl/Cargo.toml"

# Default: list available recipes.
default:
    @just --list --unsorted

# ---------------------------------------------------------------------------
# Bootstrap
# ---------------------------------------------------------------------------

# Install pinned tools via mise + register lefthook hooks.
bootstrap:
    mise install
    just hooks

hooks:
    lefthook install

hooks-uninstall:
    lefthook uninstall

# ---------------------------------------------------------------------------
# Engine (the Rust crate at .template/tmpl/)
# ---------------------------------------------------------------------------

engine-build:
    cargo build --manifest-path {{ENGINE_MANIFEST}} --release

engine-check:
    cargo check --manifest-path {{ENGINE_MANIFEST}} --all-targets

# Watch loop for fast feedback while editing the engine.
watch JOB="check":
    bacon --manifest-path {{ENGINE_MANIFEST}} {{JOB}}

# ---------------------------------------------------------------------------
# Lint, format, and the strict-code grep gate
# ---------------------------------------------------------------------------

fmt:
    cargo fmt --manifest-path {{ENGINE_MANIFEST}} --all

fmt-check:
    cargo fmt --manifest-path {{ENGINE_MANIFEST}} --all -- --check

clippy:
    cargo clippy --manifest-path {{ENGINE_MANIFEST}} --all-targets -- -D warnings

typos:
    typos

actionlint:
    actionlint

yamllint:
    yamllint .

markdownlint:
    markdownlint-cli2 "**/*.md" "#target" "#.template/tmpl/target"

# Reject patterns that mask real bugs even when the type / lint gates pass.
# Sources of truth: docs/adr/0002-strict-code-grep.md.
strict-code:
    @echo "::group::strict-code"
    # No bare TODO/FIXME without an issue reference.
    ! grep -rEn '\b(TODO|FIXME)\b(?!\(#[0-9]+\))' \
        --include='*.rs' --include='*.toml' --include='*.yml' --include='*.yaml' \
        --exclude-dir=target --exclude-dir=.template/tmpl/target \
        . || (echo "bare TODO/FIXME — add (#NN) issue link" && exit 1)
    # No #[allow(...)] without `reason = "..."`.
    ! grep -rEn '#\[allow\([a-z_:]+\)\]' \
        --include='*.rs' --exclude-dir=target . \
        || (echo "#[allow(...)] missing reason = \"...\"" && exit 1)
    # No `unsafe` blocks (engine is `#![forbid(unsafe_code)]`; this is belt-and-braces).
    ! grep -rEn '\bunsafe[[:space:]]*\{' \
        --include='*.rs' --exclude-dir=target . \
        || (echo "unsafe block detected" && exit 1)
    # No nightly toolchain markers slipping in.
    ! grep -rEn '#!\[feature\(' \
        --include='*.rs' --exclude-dir=target . \
        || (echo "#![feature(...)] requires nightly — not allowed" && exit 1)
    @echo "::endgroup::"

lint: fmt-check clippy typos actionlint yamllint markdownlint strict-code

# ---------------------------------------------------------------------------
# Test, coverage, audit
# ---------------------------------------------------------------------------

test:
    cargo nextest run --manifest-path {{ENGINE_MANIFEST}}

test-property:
    cargo nextest run --manifest-path {{ENGINE_MANIFEST}} \
        --filter-expr 'test(/property_/)' \
        --no-capture

# Region-coverage gate. Day-1 floor is 92 %; the long-term target
# matches afm at 96 %. Ratchet up by raising COVERAGE_FLOOR as test
# surface grows (boon validation paths, filesystem error injection,
# 3-way merge fault paths). Never lower.
COVERAGE_FLOOR := "92"

coverage:
    cargo llvm-cov --manifest-path {{ENGINE_MANIFEST}} \
        --ignore-filename-regex 'src/main\.rs' \
        --fail-under-regions {{COVERAGE_FLOOR}} \
        --summary-only

audit:
    cargo deny --manifest-path {{ENGINE_MANIFEST}} check

# Engine-side template-self-CI: manifest + DAG soundness over every layer.
verify-template: engine-build
    .template/tmpl/target/release/tmpl verify

# ---------------------------------------------------------------------------
# Aggregate gates
# ---------------------------------------------------------------------------

# What the CI workflow runs end-to-end.
ci: lint test coverage verify-template audit

# Mirror of the pre-commit hook chain so contributors can preview locally.
pre-commit: fmt-check clippy typos strict-code
