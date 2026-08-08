# QUIC-based WebSockets in Guard

Fork goal: carry Guard's client-facing WebSocket traffic over QUIC instead
of TCP, so that head-of-line blocking is per stream rather than per
connection.

Upstream has no work to rebase onto. As of 2026-08-08 the upstream
repository has no open PR or issue for WebTransport, QUIC, or HTTP/3, no
branch matching those names across 1042 branches, and no `quinn` or `h3`
entry in any workspace `Cargo.toml`. Every `webtransport` path in the tree
belongs to Unity's vendored `SimpleWebTransport`, which is a WebSocket
library. A branch dated 2025-05-30 reads
`chore(cluster): remove tcp & udp ports on gg`.

## Why this is smaller than a datagram tunnel

The tunnel between Guard and an actor is WebSocket-shaped in both
directions: `pegboard-gateway/src/tunnel_to_ws_task.rs` and
`ws_to_tunnel_task.rs`. A reliable ordered stream over QUIC keeps that
shape, so the tunnel needs no new frame type and `container-runner` needs
no change.

Unreliable datagrams are a later increment. They require a datagram frame
in the tunnel, and that is a separate project.

## Target: WebTransport, not raw QUIC streams

`zone-client-godot` is a web and WASM build. A browser cannot open a raw
QUIC stream, and it can open a WebTransport session. So the client-facing
surface is WebTransport over HTTP/3.

The engine already carries picoquic in its `http3` module, at tag
`v2026.06.27.1907-multiplayer-fabric`, so the native client has a QUIC
stack on the other end.

## The coupling point

`engine/packages/guard-core/src/websocket_handle.rs` hardcodes the
transport in two type aliases:

- `WebSocketReceiver = Peekable<SplitStream<WebSocketStream<TokioIo<Upgraded>>>>`
- `WebSocketSender = SplitSink<WebSocketStream<TokioIo<Upgraded>>, Message>`

`WebSocketHandle` appears 66 times across `guard-core`,
`pegboard-runner`, `pegboard-gateway2`, and the Rust SDKs. So the handle
must not become generic, because that change would reach all 66 sites.

Box the split halves instead. The public API stays identical, and only
`websocket_handle.rs` changes. The cost is one dynamic dispatch per
message, which is noise against a network hop.

## Steps

1. `websocket_handle.rs`: change the two aliases to boxed trait objects.
   Add a `from_stream` constructor beside `new`, so any
   `WebSocketStream<S>` can back a handle. No call-site changes.
2. Workspace and `guard-core/Cargo.toml`: add `quinn`, `h3`, `h3-quinn`,
   and `h3-webtransport`. Match the workspace `rustls` version, because
   `quinn` and `tokio-rustls` must agree.
3. `server.rs`: add an HTTP/3 listener beside the existing HTTP and HTTPS
   `TcpListener` binds in `run_server`. Bind UDP, build the rustls config
   from the existing `create_tls_config(resolver_fn)`, and set ALPN to
   `h3`.
4. Accept a WebTransport session, take its bidirectional streams, and
   wrap each in an adapter that implements `AsyncRead` and `AsyncWrite`.
   Feed that to `WebSocketStream::from_raw_socket` with `Role::Server`,
   then `WebSocketHandle::from_stream`.
5. Route into the existing `ProxyService` path, so routing, caching, and
   metrics stay unchanged.
6. Measure with `container-runner/examples/e2e-test/load-test.mjs`, which
   reports `p50`, `p95`, `p99`, and `max`. Compare `p95` against the
   15.6 ms tick.

## What stays untouched

`container-runner` needs no change. It proxies to the child over loopback
TCP, where loss is nil, so TCP there costs nothing.

The tunnel needs no change, because a reliable ordered QUIC stream
carries the same WebSocket frames.

## Notes

Guard already exposes `config.guard().tcp_nodelay()`. The runner does not
set `TCP_NODELAY`, and that is a separate fix worth making on the
loopback hop.

`pegboard-gateway2` exists beside `pegboard-gateway`. Check which one a
given deployment uses before touching either.
