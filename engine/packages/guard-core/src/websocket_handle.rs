use anyhow::*;
use futures_util::{Sink, SinkExt, Stream, StreamExt, stream::Peekable};
use hyper_tungstenite::HyperWebsocket;
use hyper_tungstenite::tungstenite::Message;
use hyper_tungstenite::tungstenite::error::Error as WsError;
use rivet_perf::{perf_finish, perf_start};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tokio_tungstenite::WebSocketStream;

use crate::metrics;

/// The receive half, erased over the underlying transport.
///
/// A WebSocket runs over a hyper TCP upgrade today and over a WebTransport
/// bidirectional stream once HTTP/3 lands. Both are `AsyncRead + AsyncWrite`,
/// so erasing the transport here keeps every `WebSocketHandle` call site
/// unchanged. The cost is one dynamic dispatch per message, which is noise
/// against a network hop.
pub type WebSocketReceiver = Peekable<BoxedWsStream>;

/// The send half, erased over the underlying transport. See
/// [`WebSocketReceiver`].
pub type WebSocketSender = Box<dyn Sink<Message, Error = WsError> + Send + Unpin>;

pub type BoxedWsStream = Box<dyn Stream<Item = Result<Message, WsError>> + Send + Unpin>;

#[derive(Clone)]
pub struct WebSocketHandle {
	ws_tx: Arc<Mutex<WebSocketSender>>,
	ws_rx: Arc<Mutex<WebSocketReceiver>>,
}

impl WebSocketHandle {
	#[tracing::instrument(skip_all)]
	pub async fn new(websocket: HyperWebsocket) -> Result<Self> {
		let ws_stream = websocket.await?;
		Ok(Self::from_stream(ws_stream))
	}

	/// Build a handle over any already-established WebSocket stream.
	///
	/// This is the entry point for transports other than a hyper upgrade,
	/// such as a WebTransport bidirectional stream carrying WebSocket frames.
	pub fn from_stream<S>(ws_stream: WebSocketStream<S>) -> Self
	where
		S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
	{
		let (ws_tx, ws_rx) = ws_stream.split();

		Self {
			ws_tx: Arc::new(Mutex::new(Box::new(ws_tx) as WebSocketSender)),
			ws_rx: Arc::new(Mutex::new(
				(Box::new(ws_rx) as BoxedWsStream).peekable(),
			)),
		}
	}

	/// Build a handle from an already-split transport.
	///
	/// `from_stream` covers transports that are a single `AsyncRead + AsyncWrite`
	/// object. A datagram transport is not one: its two halves come from
	/// different objects on the QUIC session, so they arrive already split.
	pub fn from_parts(ws_tx: WebSocketSender, ws_rx: BoxedWsStream) -> Self {
		Self {
			ws_tx: Arc::new(Mutex::new(ws_tx)),
			ws_rx: Arc::new(Mutex::new(ws_rx.peekable())),
		}
	}

	#[tracing::instrument(skip_all)]
	pub async fn send(&self, message: Message) -> Result<()> {
		let message_kind = message_kind_label(&message);
		let message_len = message.len();
		let measure = perf_start!(
			&metrics::WEBSOCKET_SEND_DURATION,
			slow_ms = 1000,
			"guard_websocket_send",
			labels: { message_kind = %message_kind },
			fields: { message_len = ?message_len },
		);
		let lock_wait_start = Instant::now();
		let mut guard = self.ws_tx.lock().await;
		let lock_wait_elapsed = lock_wait_start.elapsed();
		metrics::WEBSOCKET_SEND_LOCK_WAIT_DURATION
			.with_label_values(&[message_kind])
			.observe(lock_wait_elapsed.as_secs_f64());

		let write_start = Instant::now();
		let res = guard.send(message).await;
		let write_elapsed = write_start.elapsed();
		metrics::WEBSOCKET_SEND_WRITE_DURATION
			.with_label_values(&[message_kind])
			.observe(write_elapsed.as_secs_f64());
		drop(guard);

		self.record_write_pressure_metrics(message_kind);
		perf_finish!(measure, fields: { message_len = message_len, result = %res.is_ok() });
		res?;
		Ok(())
	}

	#[tracing::instrument(skip_all)]
	pub async fn flush(&self) -> Result<()> {
		let res = self.ws_tx.lock().await.flush().await;
		self.record_write_pressure_metrics("flush");
		res?;
		Ok(())
	}

	pub fn recv(&self) -> Arc<Mutex<WebSocketReceiver>> {
		self.ws_rx.clone()
	}

	fn record_write_pressure_metrics(&self, message_kind: &str) {
		let _ = message_kind;
	}
}

fn message_kind_label(message: &Message) -> &'static str {
	match message {
		Message::Text(_) => "text",
		Message::Binary(_) => "binary",
		Message::Ping(_) => "ping",
		Message::Pong(_) => "pong",
		Message::Close(_) => "close",
		Message::Frame(_) => "frame",
	}
}
