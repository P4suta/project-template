# syntax=docker/dockerfile:1.7
# Generic project-template development / CI container.
# Every developer and CI job runs inside this image. Host toolchain is
# never invoked.

ARG RUST_VERSION=1.95.0
ARG NODE_VERSION=22

# ---------------------------------------------------------------------------
# Stage: base — system deps + locales + a fast linker (mold).
# ---------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS base

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        libssl-dev \
        clang \
        mold \
        curl \
        git \
        ca-certificates \
        unzip \
        xz-utils \
        locales \
    && sed -i -e 's/# \(en_US.UTF-8 UTF-8\)/\1/' /etc/locale.gen \
    && locale-gen

ENV LANG=en_US.UTF-8 \
    LC_ALL=en_US.UTF-8 \
    RUSTUP_PERMIT_COPY_RENAME=1

# Default linker: mold for fast iterative builds.
RUN mkdir -p /root/.cargo && printf '%s\n' \
    '[target.x86_64-unknown-linux-gnu]' \
    'linker = "clang"' \
    'rustflags = ["-C", "link-arg=-fuse-ld=mold"]' \
    > /root/.cargo/config.toml

# ---------------------------------------------------------------------------
# Stage: tools — common Rust dev binaries (independent of which layers
# are applied; the rust-workspace overlay can install more on top).
# ---------------------------------------------------------------------------
FROM base AS tools

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/tmp/cargo-build \
    CARGO_TARGET_DIR=/tmp/cargo-build \
    cargo install --locked --root /usr/local \
        cargo-nextest \
        cargo-llvm-cov \
        cargo-deny \
        cargo-audit \
        sccache \
        typos-cli

# bacon and just are kept in their own layers so version bumps don't
# invalidate the much larger install above.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/tmp/cargo-build \
    CARGO_TARGET_DIR=/tmp/cargo-build \
    cargo install --locked --root /usr/local bacon

RUN curl -fsSL https://just.systems/install.sh \
    | bash -s -- --to /usr/local/bin --tag 1.36.0

# lefthook (commit-msg + pre-commit + pre-push hook runner).
ARG LEFTHOOK_VERSION=2.1.6
RUN curl -fsSL \
    "https://github.com/evilmartians/lefthook/releases/download/v${LEFTHOOK_VERSION}/lefthook_${LEFTHOOK_VERSION}_Linux_x86_64.gz" \
    | gunzip > /usr/local/bin/lefthook \
    && chmod +x /usr/local/bin/lefthook

# ---------------------------------------------------------------------------
# Stage: node — Node.js for projects that need it (TypeScript, mdbook
# plugins, JS-flavoured tools).
# ---------------------------------------------------------------------------
FROM base AS node-base

ARG NODE_VERSION
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    curl -fsSL https://deb.nodesource.com/setup_${NODE_VERSION}.x | bash - && \
    apt-get install -y --no-install-recommends nodejs && \
    corepack enable

# ---------------------------------------------------------------------------
# Stage: dev — the image developers and CI jobs actually run.
# ---------------------------------------------------------------------------
FROM node-base AS dev

COPY --from=tools /usr/local/cargo/bin/ /usr/local/cargo/bin/
COPY --from=tools /usr/local/bin/ /usr/local/bin/

ENV CARGO_HOME=/workspace/.cargo \
    CARGO_TARGET_DIR=/workspace/target \
    RUSTC_WRAPPER=sccache \
    SCCACHE_DIR=/workspace/.sccache \
    RUST_BACKTRACE=1

# Pre-create the volume mount points so docker can attach the named
# volumes without needing to mkdirat() into the bind-mounted source
# tree.
RUN mkdir -p /workspace/target /workspace/.cargo /workspace/.sccache

WORKDIR /workspace
CMD ["bash"]

# ---------------------------------------------------------------------------
# Stage: ci — currently identical to dev, named separately so workflow
# files can pin the explicit target.
# ---------------------------------------------------------------------------
FROM dev AS ci
