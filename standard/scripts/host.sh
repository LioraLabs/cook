#!/usr/bin/env bash
#
# Host the Cook Standard docs on the local tailnet.
#
# Builds the site if dist/ is missing, serves it on 127.0.0.1, and
# exposes it as HTTPS via `tailscale serve`. Runs in the foreground;
# Ctrl-C cleans up the tailscale serve config and the local server.
#
# Usage (from the standard/ directory):  pnpm host

set -euo pipefail

PORT=4321
STANDARD_DIR="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"
DIST_DIR="$STANDARD_DIR/dist"

if [ ! -d "$DIST_DIR" ] || [ -z "$(ls -A "$DIST_DIR" 2>/dev/null)" ]; then
  echo "dist/ missing or empty — running pnpm build..." >&2
  (cd "$STANDARD_DIR" && pnpm build)
fi

if ! command -v tailscale >/dev/null; then
  echo "error: tailscale CLI not found in PATH" >&2
  exit 1
fi

if ! command -v python3 >/dev/null; then
  echo "error: python3 not found in PATH" >&2
  exit 1
fi

FQDN="$(tailscale status --self --json 2>/dev/null \
          | grep -Po '"DNSName"\s*:\s*"\K[^"]+' \
          | head -1 \
          | sed 's/\.$//' || true)"

cleanup() {
  echo
  echo "Stopping tailnet exposure..."
  tailscale serve reset 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Serve the built static files on loopback only.
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$DIST_DIR" \
  > /dev/null 2>&1 &
LOCAL_PID=$!

# Give the server a moment to bind before asking tailscale to proxy it.
sleep 1
if ! kill -0 "$LOCAL_PID" 2>/dev/null; then
  echo "error: local static server failed to start on port $PORT" >&2
  exit 1
fi

# Expose on the tailnet over HTTPS. Tailscale provisions a cert from its
# internal CA. The site is reachable only by devices on the same tailnet;
# it is NOT exposed to the public internet (use `tailscale funnel` for that).
tailscale serve --bg --https=443 "http://localhost:$PORT"

echo
if [ -n "$FQDN" ]; then
  echo "Cook Standard live at: https://$FQDN/"
else
  echo "Cook Standard live on the tailnet — run 'tailscale status --self' for the FQDN."
fi
echo "Press Ctrl-C to stop."

wait "$LOCAL_PID"
