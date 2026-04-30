#!/usr/bin/env bash
# Path B entry point: build the `tmpl` engine and run an initial apply.
#
# Usage:
#   bash .template/bootstrap.sh                  # apply default selection
#   bash .template/bootstrap.sh --layers a,b,c   # apply explicit selection
#   bash .template/bootstrap.sh --help
#
# Prerequisites:
#   - bash 4+ (POSIX-friendly otherwise)
#   - git (for repo metadata)
#   - mise (https://mise.jdx.dev/) — auto-installed if absent
#
# Side effects:
#   - mise install (toolchain pinned by mise.toml)
#   - cargo build --release -p tmpl
#   - tmpl apply (rendered files written to repo root)
#   - .template/state.toml created

set -eu -o pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
TEMPLATE_ROOT="$REPO_ROOT/.template"
ENGINE_DIR="$TEMPLATE_ROOT/tmpl"

LAYERS=""
PROJECT_NAME=""
PROJECT_OWNER=""
PROJECT_DESCRIPTION=""

usage() {
  cat <<'USAGE'
Usage: bash .template/bootstrap.sh [OPTIONS]

Options:
  --layers a,b,c           Comma-separated layer selection.
                           Defaults to manifest.toml's default_selection.
  --project-name NAME      Repository name (default: directory name).
  --project-owner OWNER    Repository owner / GitHub login.
                           Default: parsed from `git remote get-url origin`,
                           or the local user as a fallback.
  --project-description S  One-line project description (default: empty).
  -h, --help               Show this help.

The script uses `mise` to provision the Rust toolchain that the engine
requires, then builds and runs `tmpl apply`. The rendered files land
in the repository root and `.template/state.toml` records what was
applied.
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --layers)              LAYERS="$2"; shift 2 ;;
    --project-name)        PROJECT_NAME="$2"; shift 2 ;;
    --project-owner)       PROJECT_OWNER="$2"; shift 2 ;;
    --project-description) PROJECT_DESCRIPTION="$2"; shift 2 ;;
    -h|--help)             usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

# ----- derive repo metadata when the user didn't pass --project-* -----------

if [ -z "$PROJECT_NAME" ]; then
  PROJECT_NAME="$(basename "$REPO_ROOT")"
fi

if [ -z "$PROJECT_OWNER" ]; then
  remote_url="$(git -C "$REPO_ROOT" remote get-url origin 2>/dev/null || true)"
  case "$remote_url" in
    git@github.com:*)
      PROJECT_OWNER="${remote_url#git@github.com:}"
      PROJECT_OWNER="${PROJECT_OWNER%%/*}"
      ;;
    https://github.com/*)
      PROJECT_OWNER="${remote_url#https://github.com/}"
      PROJECT_OWNER="${PROJECT_OWNER%%/*}"
      ;;
    *)
      PROJECT_OWNER="$(id -un)"
      ;;
  esac
fi

# ----- ensure mise is on PATH ----------------------------------------------

if ! command -v mise >/dev/null 2>&1; then
  echo "mise not found; installing via the official installer..." >&2
  curl -fsSL https://mise.run | sh
  # The installer drops mise into ~/.local/bin; export so this script
  # can resolve it on the next line without restarting the shell.
  export PATH="$HOME/.local/bin:$PATH"
fi

# Make sure the toolchain pinned in mise.toml is materialised.
mise trust "$REPO_ROOT" >/dev/null 2>&1 || true
mise install --cd "$REPO_ROOT"

# ----- build the engine -----------------------------------------------------

echo "Building the tmpl engine..." >&2
( cd "$ENGINE_DIR" && cargo build --release )

# ----- run apply ------------------------------------------------------------

ENGINE_BIN="$ENGINE_DIR/target/release/tmpl"

apply_args=(
  --template-root "$TEMPLATE_ROOT"
  --dest "$REPO_ROOT"
  apply
  --project-name "$PROJECT_NAME"
  --project-owner "$PROJECT_OWNER"
  --project-description "$PROJECT_DESCRIPTION"
)
if [ -n "$LAYERS" ]; then
  apply_args+=( --layers "$LAYERS" )
fi

"$ENGINE_BIN" "${apply_args[@]}"

echo "" >&2
echo "Apply complete. Inspect $REPO_ROOT/.template/state.toml for the recorded state." >&2
