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
use hyper::header::HeaderMap;
use hyper_tungstenite::tungstenite::protocol::Role;
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;

use crate::websocket_handle::WebSocketHandle;

/// ALPN identifier for HTTP/3. A QUIC client must offer this to reach the
/// listener.
pub const ALPN_H3: &[u8] = b"h3";

/// How long a stream may take to name its target before it is dropped.
const STREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on the routing path a stream may name.
const MAX_STREAM_PATH_LEN: u16 = 2048;

/// The parts of the CONNECT request that opened a WebTransport session.
///
/// Routing needs the same inputs the TCP path uses, so the authority and
/// headers travel alongside the path rather than the path alone.
#[derive(Clone)]
pub struct WebTransportRequest {
	/// Includes path and query.
	pub path: String,
	/// Authority of the CONNECT request, used as the `Host` for routing.
	pub authority: String,
	pub headers: HeaderMap,
	pub remote_addr: SocketAddr,
}

/// Called once per accepted WebSocket-over-WebTransport stream.
pub type WebSocketSink<F> = Arc<dyn Fn(WebSocketHandle, WebTransportRequest) -> F + Send + Sync>;

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
					if let Err(err) = serve_connection(conn, remote_addr, on_websocket).await {
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
async fn serve_connection<F>(
	conn: quinn::Connection,
	remote_addr: SocketAddr,
	on_websocket: WebSocketSink<F>,
) -> Result<()>
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

			let path = req
				.uri()
				.path_and_query()
				.map(|pq| pq.as_str().to_string())
				.unwrap_or_else(|| req.uri().path().to_string());
				let authority = req
					.uri()
					.authority()
					.map(|a| a.as_str().to_string())
					.unwrap_or_default();
				let wt_req = WebTransportRequest {
					path: path.clone(),
					authority,
					headers: req.headers().clone(),
					remote_addr,
				};

			let session = WebTransportSession::accept(req, stream, h3_conn)
				.await
				.context("failed to accept the WebTransport session")?;

			tracing::debug!(%path, "WebTransport session established");

			serve_session(session, wt_req, on_websocket).await
		}
		Result::Ok(None) => Ok(()),
		Err(err) => Err(err).context("h3 accept failed"),
	}
}

/// Accept bidirectional streams from a session and wrap each as a WebSocket.
async fn serve_session<F>(
	session: WebTransportSession<h3_quinn::Connection, Bytes>,
	req: WebTransportRequest,
	on_websocket: WebSocketSink<F>,
) -> Result<()>
where
	F: Future<Output = ()> + Send + 'static,
{
	loop {
		match session.accept_bi().await {
			Result::Ok(Some(AcceptedBi::BidiStream(_session_id, mut stream))) => {
				let req = req.clone();
				let on_websocket = on_websocket.clone();

				tokio::spawn(async move {
					// A session CONNECTs to one path, and streams inside it carry
					// none of their own, so one session would otherwise reach one
					// actor. Reading a target per stream is what lets a single
					// QUIC connection serve two zones, which is the point: two
					// sessions would mean two congestion controllers.
					let path = match read_stream_path(&mut stream).await {
						Result::Ok(Some(path)) => path,
						Result::Ok(None) => req.path.clone(),
						Err(err) => {
							tracing::debug!(?err, "dropping stream that never named a target");
							return;
						}
					};

					let mut stream_req = req;
					stream_req.path = path;

					// `BidiStream` implements tokio's AsyncRead and AsyncWrite, so
					// tungstenite drives it directly. The handshake already
					// happened at the CONNECT layer, so the stream starts in the
					// established state.
					let ws_stream =
						WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
					let handle = WebSocketHandle::from_stream(ws_stream);

					on_websocket(handle, stream_req).await;
				});
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

/// Read the routing target a stream names before its WebSocket framing starts.
///
/// The header is a big-endian `u16` length followed by that many UTF-8 bytes. A
/// length of zero means the stream inherits the session's CONNECT path, so a
/// client that only talks to one actor sends two zero bytes and nothing else.
///
/// This is read before tungstenite sees the stream, because the frames after it
/// are ordinary WebSocket frames and must arrive unmodified.
async fn read_stream_path<S>(stream: &mut S) -> Result<Option<String>>
where
	S: tokio::io::AsyncRead + Unpin,
{
	let read = async {
		let mut len_buf = [0u8; 2];
		stream.read_exact(&mut len_buf).await?;
		let len = u16::from_be_bytes(len_buf);

		if len == 0 {
			return Result::Ok(None);
		}

		ensure!(
			len <= MAX_STREAM_PATH_LEN,
			"stream named a target of {len} bytes, over the {MAX_STREAM_PATH_LEN} limit"
		);

		let mut path = vec![0u8; len as usize];
		stream.read_exact(&mut path).await?;

		let path = String::from_utf8(path).context("stream target is not valid UTF-8")?;
		ensure!(
			path.starts_with('/'),
			"stream target must be an absolute path"
		);

		Result::Ok(Some(path))
	};

	tokio::time::timeout(STREAM_HEADER_TIMEOUT, read)
		.await
		.context("stream did not name a target in time")?
}

#[cfg(test)]
#[path = "h3_server/tests.rs"]
mod tests;
