#!/bin/sh
# Glue between container-runner's contract and the Godot MCP zone.
#
# The child serves two surfaces from one process. MCP over HTTP on PORT, which
# Rivet's tunnel proxies, and a WebSocket echo zone on PORT + 1 for game
# traffic.
#
# The default TCP readiness probe works, because the MCP listener is TCP on
# PORT.
set -eu

: "${PORT:?container-runner did not inject PORT}"

echo "godot-demo: PORT=${PORT} (mcp, http) $((PORT + 1)) (websocket)" >&2

exec "${GODOT_BIN:-godot}" \
	--headless \
	--path "${GODOT_PROJECT:-/opt/zone}" \
	--script "${ZONE_SCRIPT:-zone_main.gd}"
