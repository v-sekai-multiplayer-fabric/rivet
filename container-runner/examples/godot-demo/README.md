# Godot WebSocket zone, reached over WebTransport

A Rivet actor whose child is a headless Godot WebSocket echo server. The client
speaks WebTransport. Guard terminates it and carries WebSocket frames over
Rivet's tunnel, so the child never speaks QUIC.

## The split

| Leg                | Transport   | Why                                                        |
| ------------------ | ----------- | ---------------------------------------------------------- |
| Client to Guard    | WebTransport | The lossy leg. Datagrams and per-stream recovery matter     |
| Guard to the child | WebSocket    | Inside the datacenter, over the pub/sub tunnel              |

Loss lives on the client's last mile, not inside the datacenter. So QUIC earns
its cost on the first leg and nothing on the second.

The internal path crosses `universalpubsub`, publishing to
`RunnerReceiverSubject` and subscribing on `GatewayReceiverSubject`. A round
trip crosses the broker four times, which is an estimated 1 to 3 ms against a
15.6 ms tick.

## The child

`project/ws_zone_server.gd` listens on the injected `PORT` with `TCPServer`,
promotes each connection with `WebSocketPeer.accept_stream`, and replies
`echo: <message>`. That matches what
`container-runner/examples/e2e-test/ws-test-client.mjs` asserts.

It prints a JSON ready line for parity with the engine's own demo:

```json
{"event": "ready", "port": 7770, "transport": "websocket"}
```

Readiness needs no beacon, because a WebSocket listener is TCP and
`container-runner` probes with `TcpStream::connect`.

## Build

Build from the repository root, because the runner binary comes from this
workspace:

```sh
docker build -f container-runner/examples/godot-demo/Dockerfile -t godot-ws-zone .
```

Override the engine image or tag with `--build-arg ENGINE_IMAGE=...` and
`--build-arg ENGINE_TAG=...`.

## Known gaps

**The engine tag ships no Linux binaries.** The release for
`v2026.06.27.1907-multiplayer-fabric` carries `windows-editor.zip`,
`windows-template-debug.zip`, and `windows-template-release.zip`. Its notes
name two Linux images instead:
`ghcr.io/v-sekai-multiplayer-fabric/zone-godot-runtime:latest` and
`ghcr.io/v-sekai-multiplayer-fabric/godot-editor-double:latest`.

Both are named at `latest`, which drifts. `ENGINE_TAG` defaults to the engine
tag so the build is reproducible when a matching tag exists. Anonymous access to
that registry fails, so whether it carries that tag is unverified here.

**Guard does not terminate WebTransport yet.** `h3_server.rs` binds a QUIC
endpoint and accepts WebTransport sessions, and it is not yet bound in
`run_server` and not yet routed into `ProxyService`. Until that lands, a client
reaches this actor over WebSocket through the gateway. See
`engine/packages/guard-core/WEBTRANSPORT.md`.

**The GDScript is unverified.** No engine binary ran here, so
`ws_zone_server.gd` compiles against the documented Godot 4 API and has not
been executed. `WebSocketPeer.accept_stream` in a polling loop is the part to
check first.
