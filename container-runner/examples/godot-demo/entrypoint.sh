#!/bin/sh
# Glue between container-runner's contract and the engine's WebTransport demo.
#
# container-runner injects PORT and then probes it with a TCP connect before it
# reports the actor ready. The demo listens on two sockets: WebTransport over
# UDP on ZONE_PORT, and a plain HTTP page over TCP on ZONE_PORT + 1.
#
# So ZONE_PORT is set one below PORT. The HTTP page then lands exactly on PORT,
# readiness passes, and the tunnel proxies the page. WebTransport stays on
# PORT - 1 over UDP, reached directly rather than through the tunnel, because
# container-runner proxies TCP only.
set -eu

: "${PORT:?container-runner did not inject PORT}"

ZONE_PORT=$((PORT - 1))
export ZONE_PORT

echo "godot-demo: PORT=${PORT} (http, tcp) ZONE_PORT=${ZONE_PORT} (webtransport, udp)" >&2

exec "${GODOT_BIN:-godot}" \
	--headless \
	--path "${GODOT_PROJECT:-/opt/zone}" \
	--script "${DEMO_SCRIPT:-modules/http3/demo/wt_server_demo.gd}"
