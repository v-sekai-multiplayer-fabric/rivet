//! HTTP/3 listener carrying WebTransport sessions.
//!
//! Guard's TCP listeners serve HTTP/1.1 and HTTP/2, and a WebSocket arrives as
//! a hyper upgrade. This module adds a QUIC listener beside them so a client
//! can open a WebTransport session instead. Each bidirectional stream inside
//! that session carries WebSocket frames, so the tunnel and every
//! `WebSocketHandle` consumer stay unchanged.
//!
//! The reason to prefer this over a TCP WebSocket is head-of-line blocking.
//! On TCP one lost segment stalls every channel sharing the connection. On
//! QUIC each stream recovers independently, so a reliable control channel and
//! a high-rate state channel no longer block each other.
//!
//! Datagrams are deliberately out of scope here. `WebTransportSession` exposes
//! `datagram_reader` and `datagram_sender`, and carrying those to an actor
//! needs a datagram frame in the tunnel, which is a separate change.

use anyhow::*;
use bytes::Bytes;
use h3::server::Connection as H3Connection;
use h3_webtransport::server::{AcceptedBi, WebTransportSession};
use hyper_tungstenite::tungstenite::protocol::Role;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_tungstenite::WebSocketStream;

use crate::websocket_handle::WebSocketHandle;

/// ALPN identifier for HTTP/3. A QUIC client must offer this to reach the
/// listener.
pub const ALPN_H3: &[u8] = b"h3";

/// Called once per accepted WebSocket-over-WebTransport stream.
///
/// The `path` is the `:path` of the CONNECT request that opened the session,
/// so routing can reuse whatever the TCP path already does with it.
pub type WebSocketSink<F> = Arc<dyn Fn(WebSocketHandle, String) -> F + Send + Sync>;

/// Turn a rustls config into one QUIC accepts.
///
/// QUIC requires TLS 1.3 and an ALPN of `h3`. The caller passes the same
/// config the TCP listener uses, built from Guard's certificate resolver, so
/// both listeners present the same certificates.
pub fn quic_server_config(mut tls: rustls::ServerConfig) -> Result<quinn::ServerConfig> {
	tls.alpn_protocols = vec![ALPN_H3.to_vec()];

	let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
		.context("rustls config is not usable for QUIC, which requires TLS 1.3")?;

	Ok(quinn::ServerConfig::with_crypto(Arc::new(crypto)))
}

/// Bind a QUIC endpoint and serve WebTransport sessions until cancelled.
pub async fn run_h3_listener<F>(
	addr: SocketAddr,
	tls: rustls::ServerConfig,
	on_websocket: WebSocketSink<F>,
) -> Result<()>
where
	F: Future<Output = ()> + Send + 'static,
{
	let server_config = quic_server_config(tls)?;
	let endpoint = quinn::Endpoint::server(server_config, addr)
		.with_context(|| format!("failed to bind QUIC endpoint on {addr}"))?;

	tracing::info!(?addr, "HTTP/3 server listening");

	while let Some(incoming) = endpoint.accept().await {
		let on_websocket = on_websocket.clone();

		tokio::spawn(async move {
			let remote_addr = incoming.remote_address();
			match incoming.await {
				Result::Ok(conn) => {
					if let Err(err) = serve_connection(conn, on_websocket).await {
						tracing::warn!(?err, ?remote_addr, "h3 connection ended with an error");
					}
				}
				Err(err) => {
					tracing::debug!(?err, ?remote_addr, "QUIC handshake failed");
				}
			}
		});
	}

	Ok(())
}

/// Drive one QUIC connection: accept CONNECT requests, promote each to a
/// WebTransport session, then hand every bidirectional stream to the caller.
async fn serve_connection<F>(conn: quinn::Connection, on_websocket: WebSocketSink<F>) -> Result<()>
where
	F: Future<Output = ()> + Send + 'static,
{
	let mut h3_conn: H3Connection<h3_quinn::Connection, Bytes> = h3::server::builder()
		.enable_webtransport(true)
		.enable_extended_connect(true)
		.enable_datagram(true)
		.max_webtransport_sessions(1)
		.send_grease(true)
		.build(h3_quinn::Connection::new(conn))
		.await
		.context("failed to establish the h3 connection")?;

	// A WebTransport session takes ownership of the connection, so this loop
	// serves one session per QUIC connection.
	match h3_conn.accept().await {
		Result::Ok(Some(resolver)) => {
			let (req, stream) = resolver
				.resolve_request()
				.await
				.context("failed to resolve the h3 request")?;

			let path = req.uri().path().to_string();

			let session = WebTransportSession::accept(req, stream, h3_conn)
				.await
				.context("failed to accept the WebTransport session")?;

			tracing::debug!(%path, "WebTransport session established");

			serve_session(session, path, on_websocket).await
		}
		Result::Ok(None) => Ok(()),
		Err(err) => Err(err).context("h3 accept failed"),
	}
}

/// Accept bidirectional streams from a session and wrap each as a WebSocket.
async fn serve_session<F>(
	session: WebTransportSession<h3_quinn::Connection, Bytes>,
	path: String,
	on_websocket: WebSocketSink<F>,
) -> Result<()>
where
	F: Future<Output = ()> + Send + 'static,
{
	loop {
		match session.accept_bi().await {
			Result::Ok(Some(AcceptedBi::BidiStream(_session_id, stream))) => {
				// `BidiStream` implements tokio's AsyncRead and AsyncWrite, so
				// tungstenite drives it directly. The handshake already
				// happened at the CONNECT layer, so the stream starts in the
				// established state.
				let ws_stream = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
				let handle = WebSocketHandle::from_stream(ws_stream);

				let fut = on_websocket(handle, path.clone());
				tokio::spawn(fut);
			}
			Result::Ok(Some(AcceptedBi::Request(..))) => {
				// A nested HTTP/3 request inside the session. Guard has no use
				// for one yet, so it is dropped rather than answered.
				tracing::debug!("ignoring a nested h3 request inside a WebTransport session");
			}
			Result::Ok(None) => return Ok(()),
			Err(err) => {
				tracing::debug!(?err, "WebTransport session closed");
				return Ok(());
			}
		}
	}
}
