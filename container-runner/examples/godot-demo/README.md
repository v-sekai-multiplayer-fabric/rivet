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

## No HTTP/1 anywhere

WebTransport is QUIC, so the child listens on UDP only. Nothing on this actor
serves HTTP/1.

That breaks `container-runner`'s default readiness probe, which is a TCP
connect in `src/child.rs`. A UDP-only child never satisfies it.

So this branch adds a second readiness mode. `--readiness-beacon <substring>`
makes the runner wait for a stdout line containing that substring, and the
stdout pump matches it while it forwards the line. The demo already prints a
suitable line:

```json
{"event": "ready", "port": 7770, "cert_hash": "..."}
```

`ZONE_PORT` therefore equals `PORT`, and WebTransport owns it.

| Socket       | Port   | Protocol | Readiness signal   |
| ------------ | ------ | -------- | ------------------ |
| WebTransport | `PORT` | QUIC/UDP | The stdout beacon  |

The demo's own HTTP test page still binds `ZONE_PORT + 1`. It is incidental
here, it carries no traffic for the actor, and nothing depends on it.

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
`PORT`. Something must publish that pair together with `cert_hash` from the
beacon, which is a zone directory. This demo does not supply one.

## What this does not demonstrate

Guard does not yet terminate WebTransport. The fork's
`engine/packages/guard-core/src/h3_server.rs` binds a QUIC endpoint and accepts
WebTransport sessions, and it is not yet bound in `run_server` and not yet
routed into `ProxyService`. See `engine/packages/guard-core/WEBTRANSPORT.md`
for the remaining steps.

So this demo proves the engine side and the actor lifecycle. It does not prove
WebTransport through Rivet's gateway.
