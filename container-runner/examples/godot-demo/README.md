# Godot WebTransport demo

A Rivet actor that runs the engine's own WebTransport echo server. It needs no
new client code, because the demo serves its own browser test page.

## What runs

The child is the pinned engine build,
`v2026.06.27.1907-multiplayer-fabric`, running
`modules/http3/demo/wt_server_demo.gd`. That script already does the work:

- Builds a fresh P-256 ECDSA certificate with 13-day validity, using
  `Crypto.generate_ecdsa` and `generate_self_signed_certificate_san`.
- Starts `WebTransportPeer` as a server on `ZONE_PORT`, at path `/wt`.
- Prints a ready beacon on stdout as JSON:
  `{"event": "ready", "port": ..., "cert_hash": ...}`.
- Serves a browser test page over plain HTTP on `ZONE_PORT + 1`.
- Echoes every incoming datagram and stream back to the sender.

The 13-day validity is deliberate. A browser accepts
`serverCertificateHashes` only for a short-lived certificate, so this avoids
any certificate authority.

## Why two ports

`container-runner` injects `PORT` and then probes it with a TCP connect before
it reports the actor ready. Its proxy is TCP only: `src/proxy.rs` forwards to
`http://127.0.0.1:{child_port}` and the `ws://` form, and `src/child.rs` checks
readiness with `TcpStream::connect`.

WebTransport is QUIC, which is UDP. So the two do not overlap.

`entrypoint.sh` sets `ZONE_PORT` to `PORT - 1`. The demo's HTTP page then lands
exactly on `PORT`, readiness passes, and the tunnel proxies the page.
WebTransport stays on `PORT - 1` over UDP, reached directly rather than through
the tunnel.

| Socket       | Port       | Protocol | Reached by            |
| ------------ | ---------- | -------- | --------------------- |
| Test page    | `PORT`     | HTTP/TCP | Rivet's tunnel        |
| WebTransport | `PORT - 1` | QUIC/UDP | The client, directly  |

## Build

Build from the repository root, because the runner binary comes from this
workspace:

```sh
docker build -f container-runner/examples/godot-demo/Dockerfile -t godot-wt-demo .
```

Override the engine image or tag with `--build-arg ENGINE_IMAGE=...` and
`--build-arg ENGINE_TAG=...`.

## Known gaps

**The engine tag ships no Linux binaries.** The release for
`v2026.06.27.1907-multiplayer-fabric` carries `windows-editor.zip`,
`windows-template-debug.zip`, and `windows-template-release.zip`. Its notes
name two Linux images instead:
`ghcr.io/v-sekai-multiplayer-fabric/godot-editor-double:latest` and
`ghcr.io/v-sekai-multiplayer-fabric/zone-godot-runtime:latest`.

Both are named at `latest`, which drifts. `ENGINE_TAG` defaults to the engine
tag so the build is reproducible when a matching tag exists. Verify that the
registry carries it, because anonymous access to that registry fails, so this
is unverified here.

**The test page hardcodes `127.0.0.1`.** `wt_server_demo.gd` builds the page
with `https://127.0.0.1:<port>/wt`, so it self-tests only from inside the
container's network namespace. Reaching it from elsewhere needs the host
address instead, which is a change in the engine repository rather than here.

**The UDP port is not routed by Rivet.** A client needs the host address and
`PORT - 1`. Something must publish that pair, which is a zone directory. This
demo does not supply one.

## What this does not demonstrate

Guard does not yet terminate WebTransport. The fork's
`engine/packages/guard-core/src/h3_server.rs` binds a QUIC endpoint and accepts
WebTransport sessions, and it is not yet bound in `run_server` and not yet
routed into `ProxyService`. See `engine/packages/guard-core/WEBTRANSPORT.md`
for the remaining steps.

So this demo proves the engine side and the actor lifecycle. It does not prove
WebTransport through Rivet's gateway.
