//! A WebSocket transport backed by QUIC datagrams.
//!
//! [`WebSocketHandle`](crate::WebSocketHandle) is erased over its transport, so
//! anything that is a `Stream` of messages and a `Sink` for messages can drive
//! it. This module supplies that pair over a WebTransport session's datagrams
//! instead of a bidirectional stream, which makes the connection unreliable and
//! unordered without any consumer of the handle knowing.
//!
//! That is the point. A pose that arrives late is worse than one that never
//! arrives, because a newer pose has already superseded it. A reliable
//! transport cannot express "drop this rather than deliver it late", and a
//! datagram is exactly that expression.
//!
//! Unreliability is a property of the connection here, not of individual
//! messages, so nothing downstream needs a per-message flag and the actor stays
//! unaware of which transport carried its frames.

use futures_util::Sink;
use hyper_tungstenite::tungstenite::Message;
use hyper_tungstenite::tungstenite::error::Error as WsError;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A `Sink` that hands each message straight to a closure and never buffers.
///
/// A conventional sink applies backpressure when the transport is busy. That is
/// wrong for datagrams: holding a pose until the send window opens delivers a
/// stale pose, which is the behaviour being avoided. So this is always ready,
/// and the closure drops rather than queues.
pub struct UnbufferedSink<F> {
	send: F,
}

impl<F> UnbufferedSink<F>
where
	F: FnMut(Message) -> Result<(), WsError> + Unpin,
{
	pub fn new(send: F) -> Self {
		Self { send }
	}
}

impl<F> Sink<Message> for UnbufferedSink<F>
where
	F: FnMut(Message) -> Result<(), WsError> + Unpin,
{
	type Error = WsError;

	fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// Always ready. See the note on the struct: waiting here would deliver
		// stale data rather than fresh data.
		Poll::Ready(Ok(()))
	}

	fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
		let this = self.get_mut();
		(this.send)(item)
	}

	fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// Nothing is buffered, so there is nothing to flush.
		Poll::Ready(Ok(()))
	}

	fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		Poll::Ready(Ok(()))
	}
}

/// Whether a datagram send failure should end the connection.
///
/// A datagram that is too large or momentarily unsendable is dropped, because
/// that is what an unreliable transport is for. A connection-level failure is
/// not recoverable and has to surface.
pub enum SendOutcome {
	Sent,
	Dropped(&'static str),
	Fatal,
}

/// Turn a send outcome into what the sink should report.
///
/// Kept separate from the sink so the drop policy is testable without a live
/// QUIC connection.
pub fn resolve_outcome(outcome: SendOutcome) -> Result<(), WsError> {
	match outcome {
		SendOutcome::Sent => Ok(()),
		SendOutcome::Dropped(reason) => {
			tracing::trace!(reason, "dropped a datagram rather than queueing it");
			Ok(())
		}
		SendOutcome::Fatal => Err(WsError::ConnectionClosed),
	}
}

#[cfg(test)]
#[path = "datagram_transport/tests.rs"]
mod tests;
