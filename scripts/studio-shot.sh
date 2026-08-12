#!/usr/bin/env bash
#
# Launch the studio on a virtual display and photograph it.
#
# This exists because "it compiles" and "it works" are different claims, and only one of
# them can be checked by a compiler. Every interface change should be verified against a
# picture of the running application, at the window size the change is about.
#
#   scripts/studio-shot.sh out.png [width] [height] [project-dir]
#
# Sizes worth using, matching the three shells the app switches between:
#   1440x900   desktop   — rails on both sides
#   1024x768   tablet    — collapsed rails, bigger targets
#   390x844    phone     — canvas-first, bottom sheet
#
set -euo pipefail

OUT="${1:-/tmp/studio.png}"
WIDTH="${2:-1440}"
HEIGHT="${3:-900}"
PROJECT="${4:-}"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO/apps/studio/src-tauri/target/debug/md-studio"

if [[ ! -x "$BIN" ]]; then
  echo "no binary at $BIN — run: (cd apps/studio/src-tauri && cargo build)" >&2
  exit 1
fi

# A display number derived from the PID, so two runs cannot collide on :99.
DISPLAY_NUM=$(( (RANDOM % 400) + 100 ))
export DISPLAY=":${DISPLAY_NUM}"

# WebKit inside a container has no GPU and no sandbox namespace to enter. Without these
# the process dies on startup with a bus error that looks nothing like the real cause.
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export LIBGL_ALWAYS_SOFTWARE=1
export GDK_BACKEND=x11

cleanup() {
  [[ -n "${APP_PID:-}" ]] && kill "$APP_PID" 2>/dev/null || true
  [[ -n "${WEB_PID:-}" ]] && kill "$WEB_PID" 2>/dev/null || true
  [[ -n "${XVFB_PID:-}" ]] && kill "$XVFB_PID" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT

# A debug build loads `devUrl` rather than the bundled frontend, so something has to be
# serving it. Serving the *built* dist rather than running the dev server means what gets
# photographed is the bundle that ships, not a hot-reloading approximation of it.
if [[ ! -f "$REPO/apps/studio/dist/index.html" ]]; then
  echo "building the frontend…" >&2
  (cd "$REPO/apps/studio" && npx vite build >/dev/null 2>&1)
fi

(cd "$REPO/apps/studio" && npx vite preview --port 5173 --strictPort >/dev/null 2>&1) &
WEB_PID=$!

for _ in $(seq 1 100); do
  curl -s -o /dev/null "http://localhost:5173" && break
  sleep 0.1
done

Xvfb "$DISPLAY" -screen 0 "${WIDTH}x${HEIGHT}x24" -nolisten tcp >/dev/null 2>&1 &
XVFB_PID=$!

# Wait for the display rather than sleeping a fixed amount: Xvfb start time varies with
# load, and a fixed sleep is either flaky or slow.
for _ in $(seq 1 50); do
  xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 && break
  sleep 0.1
done

LOG=$(mktemp)
if [[ -n "$PROJECT" ]]; then
  "$BIN" --project "$PROJECT" >"$LOG" 2>&1 &
else
  "$BIN" >"$LOG" 2>&1 &
fi
APP_PID=$!

# Wait for a mapped window, then give the webview a moment to paint. Screenshotting the
# instant the window appears reliably captures an empty grey rectangle.
MAPPED=0
for _ in $(seq 1 150); do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "the app exited before mapping a window:" >&2
    cat "$LOG" >&2
    exit 1
  fi
  if xdotool search --onlyvisible --name "Master Design" >/dev/null 2>&1; then
    MAPPED=1
    break
  fi
  sleep 0.2
done

if [[ "$MAPPED" != "1" ]]; then
  echo "no window appeared within 30s:" >&2
  cat "$LOG" >&2
  exit 1
fi

sleep 2.5
import -display "$DISPLAY" -window root "$OUT"

echo "wrote $OUT (${WIDTH}x${HEIGHT})"
if [[ -s "$LOG" ]]; then
  echo "--- app output ---" >&2
  head -40 "$LOG" >&2
fi
