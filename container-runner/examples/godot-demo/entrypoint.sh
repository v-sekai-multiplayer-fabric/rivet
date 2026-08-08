#!/bin/sh
# Glue between container-runner's contract and the engine's WebTransport demo.
#
# WebTransport is QUIC, so the child listens on UDP. container-runner's default
# readiness probe is a TCP connect, which a UDP-only child can never satisfy.
# So this demo runs the runner with --readiness-beacon, and the runner waits for
# the demo's own stdout line instead.
#
# wt_server_demo.gd prints that line as JSON:
#   {"event": "ready", "port": ..., "cert_hash": ...}
#
# ZONE_PORT therefore equals PORT, and WebTransport owns it. Nothing serves
# HTTP/1 on the actor's port.
set -eu

: "${PORT:?container-runner did not inject PORT}"

ZONE_PORT="${PORT}"
export ZONE_PORT

echo "godot-demo: ZONE_PORT=${ZONE_PORT} (webtransport, udp)" >&2

exec "${GODOT_BIN:-godot}" \
	--headless \
	--path "${GODOT_PROJECT:-/opt/zone}" \
	--script "${DEMO_SCRIPT:-modules/http3/demo/wt_server_demo.gd}"
