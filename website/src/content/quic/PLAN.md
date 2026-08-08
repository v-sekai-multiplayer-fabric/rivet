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

Status as of 2026-08-08. Each done step compiles under
`cargo check -p rivet-guard-core`.

1. **Done.** `websocket_handle.rs` erases the transport. The two aliases
   are boxed trait objects, and `from_stream` accepts any
   `WebSocketStream<S>`. The public API is unchanged, and
   `rivet-guard`, `pegboard-runner`, `pegboard-gateway`,
   `pegboard-gateway2`, and `rivet-envoy-client` all still check.
2. **Done.** Workspace carries `quinn` 0.11.11 with `runtime-tokio` and
   `rustls-ring`, `h3` 0.0.8, `h3-quinn` 0.0.10 with its `datagram`
   feature, and `h3-webtransport` 0.1.2. The `datagram` feature is
   required, because `WebTransportSession::accept` needs
   `DatagramConnectionExt`.
3. **Done.** `h3_server.rs` binds a QUIC endpoint, sets ALPN to `h3`, and
   builds the QUIC config from any rustls `ServerConfig`, so Guard's
   certificate resolver carries over unchanged.
4. **Done.** A WebTransport session's `BidiStream` already implements
   tokio's `AsyncRead` and `AsyncWrite`, so no adapter is needed.
   `WebSocketStream::from_raw_socket` with `Role::Server` feeds
   `WebSocketHandle::from_stream` directly.
5. **Not done.** Route into the existing `ProxyService` path. The TCP
   path reaches a handler at `proxy_service.rs:1684` through
   `handler.handle_websocket(req_ctx, ws_handle, after_hibernation)`,
   inside `handle_websocket_upgrade` at line 1029. That function also
   owns routing, retries, header rewriting, and hibernation.

   The clean move is to lift the post-upgrade half of
   `handle_websocket_upgrade` into a function taking a `WebSocketHandle`
   and a routing target, then call it from both the TCP and the HTTP/3
   path. Until that lands, `run_h3_listener` accepts sessions and hands
   each WebSocket to a caller-supplied closure, and no traffic routes.
6. **Not done.** Bind the listener inside `run_server`. Use the existing
   `https.port` over UDP, which is the conventional HTTP/3 pairing, so
   the config schema needs no new field.
7. **Not done.** A Godot demo against the pinned engine at
   `v2026.06.27.1907-multiplayer-fabric`, whose `http3` module already
   carries `HTTP3Client`, `QUICClient`, `QUICServer`, and
   `WebTransportPeer`.
8. **Not done.** Measure with
   `container-runner/examples/e2e-test/load-test.mjs`, which reports
   `p50`, `p95`, `p99`, and `max`. Compare `p95` against the 15.6 ms
   tick.

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
