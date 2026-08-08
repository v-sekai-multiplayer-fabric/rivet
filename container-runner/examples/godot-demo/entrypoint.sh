#!/bin/sh
# Glue between container-runner's contract and the Godot WebSocket zone.
#
# The child speaks WebSocket, not WebTransport. Guard terminates WebTransport
# from the client and carries WebSocket frames over Rivet's tunnel, so the
# child only needs a plain WebSocket listener on the injected PORT.
#
# That also means container-runner's default readiness probe works, because a
# WebSocket listener is TCP.
set -eu

: "${PORT:?container-runner did not inject PORT}"

echo "godot-demo: PORT=${PORT} (websocket, tcp)" >&2

exec "${GODOT_BIN:-godot}" \
	--headless \
	--path "${GODOT_PROJECT:-/opt/zone}" \
	--script "${ZONE_SCRIPT:-ws_zone_server.gd}"
