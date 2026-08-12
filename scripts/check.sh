#!/usr/bin/env bash
#
# Everything CI checks, in one command.
#
# This exists because the repository holds **two** cargo workspaces. The studio's Rust
# shell needs a system webview, so it is not a member of the root workspace — which means
# `cargo test --workspace` silently does not build it. That gap has already cost one red
# CI run: a struct gained a field, every crate in the root workspace was updated, the
# studio was not, and nothing local said so.
#
#   scripts/check.sh            everything
#   scripts/check.sh --fast     skip the parts that need a browser or a display
#
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

FAST=0
[[ "${1:-}" == "--fast" ]] && FAST=1

step() { printf '\n\033[1m── %s\033[0m\n' "$1"; }
ok() { printf '   \033[32mok\033[0m  %s\n' "$1"; }

# ---------------------------------------------------------------------------
# Core crates
# ---------------------------------------------------------------------------

step "Core crates"
cargo fmt --all --check
ok "formatted"
cargo clippy --workspace --all-targets -- -D warnings
ok "lint"
cargo test --workspace --quiet
ok "tests"
cargo check -q -p md-geom --target wasm32-unknown-unknown 2>/dev/null && ok "wasm" || \
  echo "   --  wasm target not installed, skipped"

# ---------------------------------------------------------------------------
# The studio's backend — the workspace the root one does not reach
# ---------------------------------------------------------------------------

step "Studio backend"
if [[ ! -f apps/studio/dist/index.html ]]; then
  # `generate_context!` embeds the frontend, so it has to exist before the Rust side
  # will compile at all.
  echo "   building the frontend first…"
  (cd apps/studio && npx vite build >/dev/null)
fi
(
  cd apps/studio/src-tauri
  cargo fmt --all --check
  ok "formatted"
  cargo clippy --all-targets -- -D warnings
  ok "lint"
  cargo test --quiet
  ok "tests"
)

# ---------------------------------------------------------------------------
# Frontend
# ---------------------------------------------------------------------------

step "Studio frontend"
pnpm --filter @master-design/studio typecheck
ok "typecheck"
pnpm --filter @master-design/studio build >/dev/null
ok "build"

if [[ "$FAST" == "1" ]]; then
  printf '\n\033[1mAll fast checks passed.\033[0m Run without --fast for the browser and display checks.\n'
  exit 0
fi

# ---------------------------------------------------------------------------
# The things only a real browser and a real window can answer
# ---------------------------------------------------------------------------

step "The two renderers agree"
cargo build -q --release -p md-cli
MD=./target/release/md
$MD export fixtures/parity --out /tmp/check-parity-dist >/dev/null
$MD snapshot fixtures/parity --out /tmp/check-parity.png --width 900 >/dev/null
if node .github/scripts/renderer-parity.mjs \
  /tmp/check-parity-dist/index.html /tmp/check-parity.png 900 700; then
  ok "parity"
else
  echo "   run it yourself to see where: node .github/scripts/renderer-parity.mjs …" >&2
  exit 1
fi

step "The application actually starts"
(cd apps/studio/src-tauri && cargo build -q)
./scripts/studio-shot.sh /tmp/check-studio.png 1280 800 >/dev/null
test -s /tmp/check-studio.png
ok "launched and photographed → /tmp/check-studio.png"

printf '\n\033[1mAll checks passed.\033[0m\n'
