# WebTransport in Guard

Fork goal: give Guard a WebTransport surface, so a client reaches an actor
over QUIC instead of TCP. Head-of-line blocking then applies per stream
rather than per connection.

The transport is WebTransport, per `rfd/0023`. This is not WebSocket over
HTTP/3, which RFC 9220 defines and no browser exposes to JavaScript. A
WebTransport bidirectional stream carries WebSocket framing, so Guard's
internals keep their shape, and QUIC sits underneath.

Upstream has no work to rebase onto. As of 2026-08-08 the upstream
repository has no open PR or issue for WebTransport, QUIC, or HTTP/3, no
branch matching those names across 1042 branches, and no `quinn` or `h3`
entry in any workspace `Cargo.toml`. Every `webtransport` path in the tree
belongs to Unity's vendored `SimpleWebTransport`, which is a WebSocket
library. A branch dated 2025-05-30 reads
`chore(cluster): remove tcp & udp ports on gg`.

## Why WebTransport, not WebSocket over HTTP/3

`zone-client-godot` is a web and WASM build. A browser cannot open
WebSocket over HTTP/3 from JavaScript, and it can open a WebTransport
session. RFC 9220 would therefore exclude the web client.

WebTransport also carries datagrams as well as streams, so the pose stream
has a path later without a second transport change. `rfd/0023` chose it
for that reason.

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
5. **Not done.** Route into the existing `ProxyService` path. This is the
   only substantial piece left, and the seam is exact.

   `handle_websocket_upgrade` spans lines 1029 to about 1893 of
   `proxy_service.rs`. It performs the hyper upgrade at the top, which a
   WebTransport stream cannot supply, so the seam has to sit after it.

   The function then branches on `ResolveRouteOutput`. The actor path is the
   `CustomServe(handler)` arm, whose task spawns at line 1671 and reaches
   `handler.handle_websocket(req_ctx, ws_handle, after_hibernation)` at 1684.
   The other arm, spawning at 1083, forwards to an upstream target and is not
   needed for actors.

   Extract the async block spawned at 1671 into a crate-visible function:

   ```rust
   pub(crate) async fn serve_custom_websocket(
       state: Arc<ProxyState>,
       req_ctx: RequestContext,
       handler: Box<dyn CustomServeTrait>,
       ws_handle: WebSocketHandle,
   ) -> Result<()>
   ```

   The block currently builds its own handle at line 1678 with
   `WebSocketHandle::new(client_ws)`. Delete that line and take the handle as
   the parameter above. Both callers then supply one:

   - The TCP path calls `WebSocketHandle::new(client_ws).await?` first, exactly
     as it does today.
   - The HTTP/3 path calls `WebSocketHandle::from_stream(ws_stream)`, which
     step 1 added.

   Watch the captured variables. The block closes over `state` and a cloned
   `req_ctx` from lines 1667 and 1668, and the retry, hibernation, and close
   handling all live inside it. Move the whole block rather than parts of it.

6. **Not done.** Bind the listener inside `run_server`. Add a UDP bind beside
   the two `TcpListener` binds, reusing `https.port` over UDP, which is the
   conventional HTTP/3 pairing, so the config schema needs no new field. The
   `on_websocket` closure then resolves a route and calls
   `serve_custom_websocket` from step 5.

7. **Not done.** A Godot demo. `container-runner/examples/godot-demo` already
   holds the actor and a WebSocket child. It needs a WebTransport client, which
   the pinned engine supplies through `WebTransportPeer`.

8. **Not done.** Measure with
   `container-runner/examples/e2e-test/load-test.mjs`, which reports `p50`,
   `p95`, `p99`, and `max`. Compare `p95` against the 15.6 ms tick, from a real
   client network rather than from inside the datacenter.

## What the internal path costs

Worth knowing before optimising the external leg. The tunnel between Guard and
an actor is not a socket. `pegboard-gateway` carries traffic over
`universalpubsub`, publishing to `RunnerReceiverSubject` and subscribing on
`GatewayReceiverSubject`, and the workspace dependency is `async-nats`.

A round trip crosses the broker four times. Estimated 1 to 3 ms against a
15.6 ms tick, which is 6 to 19 percent of the budget. The 8.9 us `AF_UNIX`
figure from `rfd/0096` is measured; the per-hop broker cost is an estimate.

`shared_state.rs` also logs "gateway subscription unsubscribed, in flight
messages may be lost" on resubscribe, so the internal path is not lossless.

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
