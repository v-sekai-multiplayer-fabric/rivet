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
use hyper_tungstenite::tungstenite::Message;
use hyper_tungstenite::tungstenite::protocol::Role;
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;
use std::sync::Arc;
use futures_util::StreamExt;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;

use crate::datagram_transport::{SendOutcome, UnbufferedSink, resolve_outcome};
use crate::roq::{self, RtpHeader, SequenceTracker};
use crate::websocket_handle::{BoxedWsStream, WebSocketHandle, WebSocketSender};

/// ALPN identifier for HTTP/3. A QUIC client must offer this to reach the
/// listener.
pub const ALPN_H3: &[u8] = b"h3";

/// How long a stream may take to name its target before it is dropped.
const STREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on the routing path a stream may name.
const MAX_STREAM_PATH_LEN: u16 = 2048;

/// What a channel promises about delivery.
///
/// A QUIC bidirectional stream already gives reliable and sequenced, so only
/// the unreliable pair is expressed here. Reliable and unsequenced is
/// deliberately absent: nothing in this system wants it, and a stream provides
/// no way to offer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
	/// Every datagram is delivered as it arrives, in whatever order it arrives.
	Unsequenced,
	/// A datagram older than the newest already delivered is dropped.
	///
	/// This is what a pose stream wants. A late pose has been superseded, and
	/// applying it moves the avatar backwards.
	Sequenced,
}

/// Dynamic RTP payload type for a channel carrying opaque application bytes.
///
/// 96 is the first value RFC 3551 leaves to dynamic assignment.
const RTP_PAYLOAD_TYPE: u8 = 96;

/// How many datagrams may queue for one channel before the router drops them.
///
/// Small on purpose. A queue that grows holds stale poses, and a stale pose is
/// what this transport exists to discard.
const DATAGRAM_CHANNEL_DEPTH: usize = 32;

/// A stream whose target carries this query flag is served over QUIC datagrams
/// rather than over the stream itself.
///
/// Unreliability is chosen per connection rather than per message, so the
/// actor receives ordinary frames and never learns which transport carried
/// them.
const UNRELIABLE_PREFIX: &str = "rivet_unreliable=";

/// Ask for a sequenced unreliable channel: deliver a datagram only if it is
/// newer than the last one delivered on that channel.
const SEQUENCED_FLAG: &str = "rivet_sequenced=1";

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

			let session = Arc::new(
					WebTransportSession::accept(req, stream, h3_conn)
						.await
						.context("failed to accept the WebTransport session")?,
				);

			tracing::debug!(%path, "WebTransport session established");

			serve_session(session, wt_req, on_websocket).await
		}
		Result::Ok(None) => Ok(()),
		Err(err) => Err(err).context("h3 accept failed"),
	}
}

/// Accept bidirectional streams from a session and wrap each as a WebSocket.
async fn serve_session<F>(
	session: Arc<WebTransportSession<h3_quinn::Connection, Bytes>>,
	req: WebTransportRequest,
	on_websocket: WebSocketSink<F>,
) -> Result<()>
where
	F: Future<Output = ()> + Send + 'static,
{
	// One router per session, owning the single datagram reader.
	let router = Arc::new(DatagramRouter::start(session.clone()));

	loop {
		match session.accept_bi().await {
			Result::Ok(Some(AcceptedBi::BidiStream(_session_id, mut stream))) => {
				let req = req.clone();
				let on_websocket = on_websocket.clone();
				let session = session.clone();
				let router = router.clone();

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

					let channel = unreliable_channel(&path);
					let delivery = if path.contains(SEQUENCED_FLAG) {
						Delivery::Sequenced
					} else {
						Delivery::Unsequenced
					};

					let mut stream_req = req;
					stream_req.path = strip_unreliable_flag(&path);

					let handle = if let Some(channel) = channel {
						// The stream carries only the routing header. Its
						// payload then travels on the session's datagrams,
						// tagged with this channel id.
						router.handle(&session, channel, delivery).await
					} else {
						// `BidiStream` implements tokio's AsyncRead and AsyncWrite,
						// so tungstenite drives it directly. The handshake already
						// happened at the CONNECT layer, so the stream starts in
						// the established state.
						let ws_stream =
							WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
						WebSocketHandle::from_stream(ws_stream)
					};

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

/// Read the unreliable channel id a stream names, if it names one.
///
/// `rivet_unreliable=7` asks for datagrams on channel 7. Absent, the stream is
/// served reliably over itself.
pub(crate) fn unreliable_channel(path: &str) -> Option<u16> {
	let (_, query) = path.split_once('?')?;

	query
		.split('&')
		.find_map(|part| part.strip_prefix(UNRELIABLE_PREFIX))
		.and_then(|v| v.parse::<u16>().ok())
}

/// Remove the unreliable flag so routing sees an ordinary actor path.
pub(crate) fn strip_unreliable_flag(path: &str) -> String {
	let Some((base, query)) = path.split_once('?') else {
		return path.to_string();
	};

	let rest: Vec<&str> = query
		.split('&')
		.filter(|part| !part.starts_with(UNRELIABLE_PREFIX) && *part != SEQUENCED_FLAG)
		.collect();

	if rest.is_empty() {
		base.to_string()
	} else {
		format!("{base}?{}", rest.join("&"))
	}
}

/// Routes session datagrams to the connections that asked for them.
///
/// QUIC datagrams carry no stream identity, so a session has exactly one
/// datagram channel at the protocol level. That is one fewer than needed: an
/// avatar wants an unreliable pose channel *and* reliable channels beside it,
/// all on one connection so they share a congestion controller.
///
/// So identity is added in the payload. Every datagram is a `u16` channel id
/// followed by the message. Two bytes against a ~1200 byte datagram, and a
/// ShaderMotion frame is ~260 bytes, so a frame still fits in one datagram and
/// a loss still costs exactly one pose.
///
/// Reliable channels need none of this. Each one is its own bidirectional
/// stream, already independent, already free of head-of-line blocking against
/// the others.
pub struct DatagramRouter {
	channels: Arc<scc::HashMap<u16, ChannelSink>>,
}

struct ChannelSink {
	tx: tokio::sync::mpsc::Sender<Bytes>,
	delivery: Delivery,
	/// Rollover-aware view of the RTP sequence, for a sequenced channel.
	sequence: Arc<tokio::sync::Mutex<SequenceTracker>>,
}

impl DatagramRouter {
	/// Take ownership of the session's datagram reader and start demultiplexing.
	///
	/// One reader per session, not one per connection. Several readers on the
	/// same session compete for the same datagrams, and each would see an
	/// arbitrary subset.
	pub fn start(session: Arc<WebTransportSession<h3_quinn::Connection, Bytes>>) -> Self {
		let channels: Arc<scc::HashMap<u16, ChannelSink>> = Arc::new(scc::HashMap::new());
		let reader_channels = channels.clone();

		tokio::spawn(async move {
			let mut reader = session.datagram_reader();

			loop {
				match reader.read_datagram().await {
					Result::Ok(datagram) => {
						let payload = datagram.into_payload();

						// draft-ietf-avtcore-rtp-over-quic: a flow identifier,
						// then a complete RTP packet.
						let (flow_id, rtp) = match roq::parse_datagram(&payload) {
							Result::Ok(parts) => parts,
							Err(err) => {
								tracing::debug!(?err, "dropping an unparseable RoQ datagram");
								continue;
							}
						};

						let header = match roq::parse_rtp(rtp) {
							Result::Ok(h) => h,
							Err(err) => {
								tracing::debug!(?err, flow_id, "dropping a bad RTP packet");
								continue;
							}
						};

						let Result::Ok(channel) = u16::try_from(flow_id) else {
							tracing::debug!(flow_id, "flow identifier outside the channel range");
							continue;
						};

						let body = Bytes::copy_from_slice(&rtp[header.payload_offset..]);

						let found = reader_channels
							.read_async(&channel, |_, v| {
								(v.tx.clone(), v.delivery, v.sequence.clone())
							})
							.await;

						match found {
							Some((tx, delivery, sequence)) => {
								if delivery == Delivery::Sequenced
									&& sequence.lock().await.accept(header.sequence).is_none()
								{
									tracing::trace!(
										channel,
										seq = header.sequence,
										"dropped a superseded datagram"
									);
									continue;
								}

								// Drop rather than await. Waiting here would
								// delay every other channel behind this one,
								// which is the blocking this design exists to
								// avoid.
								if tx.try_send(body).is_err() {
									tracing::trace!(
										channel,
										"dropped a datagram, receiver full or gone"
									);
								}
							}
							None => {
								tracing::trace!(channel, "datagram for an unknown channel");
							}
						}
					}
					Err(err) => {
						tracing::debug!(?err, "datagram reader closed");
						break;
					}
				}
			}
		});

		Self { channels }
	}

	/// Build a handle carrying one unreliable channel.
	pub async fn handle(
		&self,
		session: &Arc<WebTransportSession<h3_quinn::Connection, Bytes>>,
		channel: u16,
		delivery: Delivery,
	) -> WebSocketHandle {
		let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(DATAGRAM_CHANNEL_DEPTH);
		let _ = self
			.channels
			.insert_async(
				channel,
				ChannelSink {
					tx,
					delivery,
					sequence: Arc::new(tokio::sync::Mutex::new(SequenceTracker::new())),
				},
			)
			.await;

		let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx)
			.map(|b| Result::Ok(Message::Binary(b)));

		let mut sender = session.datagram_sender();
		let out_seq = Arc::new(std::sync::atomic::AtomicU64::new(1));
		// One synchronisation source per channel, as RTP expects.
		let ssrc = u32::from(channel) | 0x5256_0000;
		let sink = UnbufferedSink::new(move |msg: Message| {
			let body = match msg {
				Message::Binary(data) => data,
				Message::Text(text) => Bytes::from(text.as_bytes().to_vec()),
				// Ping, Pong and Close are stream-level concepts. A datagram
				// has no ordering to close in order with.
				_ => return Result::Ok(()),
			};

			let seq = out_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u16;

			let rtp = roq::build_rtp(
				&RtpHeader {
					sequence: seq,
					// Sender wall clock, so a receiver can measure one-way
					// delay without a separate timing channel.
					timestamp: rivet_util::timestamp::now() as u32,
					ssrc,
					payload_type: RTP_PAYLOAD_TYPE,
					marker: false,
					payload_offset: roq::RTP_MIN_HEADER,
				},
				&body,
			);

			let mut framed = Vec::with_capacity(8 + rtp.len());
			if let Err(err) = roq::encode_varint(u64::from(channel), &mut framed) {
				tracing::debug!(?err, "could not encode the flow identifier");
				return Result::Ok(());
			}
			framed.extend_from_slice(&rtp);

			let outcome = match sender.send_datagram(Bytes::from(framed)) {
				Result::Ok(()) => SendOutcome::Sent,
				Err(err) => classify_send_error(&err),
			};

			resolve_outcome(outcome)
		});

		WebSocketHandle::from_parts(
			Box::new(sink) as WebSocketSender,
			Box::new(rx_stream.boxed()) as BoxedWsStream,
		)
	}
}

/// Decide whether a failed datagram send ends the connection.
fn classify_send_error(err: &impl std::fmt::Debug) -> SendOutcome {
	let text = format!("{err:?}");

	// Too large and not available are ordinary for an unreliable transport: the
	// frame is dropped and the next one is already on its way. Anything else is
	// a connection-level failure.
	if text.contains("TooLarge") {
		SendOutcome::Dropped("too large for one datagram")
	} else if text.contains("NotAvailable") {
		SendOutcome::Dropped("send window full")
	} else {
		SendOutcome::Fatal
	}
}
