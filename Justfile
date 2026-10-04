set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set dotenv-load := false

ENGINE_MANIFEST := ".template/tmpl/Cargo.toml"

default:
    @just --list --unsorted

bootstrap:
    mise install

hooks:
    mise x -- lefthook install

check suite="local":
    ci-policy project-check --suite {{quote(suite)}}

check-staged:
    ci-policy project-check --index --phase commit

fmt:
    mise x rust@1.99.0 -- cargo fmt --manifest-path .ci/ci-policy/Cargo.toml
    mise x -- cargo fmt --manifest-path {{ENGINE_MANIFEST}} --all

test:
    mise x -- cargo nextest run --locked --manifest-path {{ENGINE_MANIFEST}}

build:
    mise x -- cargo build --locked --manifest-path {{ENGINE_MANIFEST}} --release

coverage:
    mise x -- cargo llvm-cov --locked --manifest-path {{ENGINE_MANIFEST}} --ignore-filename-regex 'src/main\.rs' --fail-under-regions 94 --summary-only

prove:
    ci-policy prove --manifest-path .ci/ci-policy/Cargo.toml

verify-template:
    mise x -- cargo run --locked --manifest-path {{ENGINE_MANIFEST}} --release -- verify

ci:
    just check

watch job="check":
    mise x -- bacon --manifest-path {{ENGINE_MANIFEST}} {{quote(job)}}
