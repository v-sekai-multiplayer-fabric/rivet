use futures_util::SinkExt;
use hyper_tungstenite::tungstenite::Message;
use hyper_tungstenite::tungstenite::error::Error as WsError;
use std::sync::{Arc, Mutex};

use super::{SendOutcome, UnbufferedSink, resolve_outcome};

fn recorder() -> (Arc<Mutex<Vec<Message>>>, impl FnMut(Message) -> Result<(), WsError> + Unpin) {
	let seen = Arc::new(Mutex::new(Vec::new()));
	let sink_seen = seen.clone();
	(seen, move |msg| {
		sink_seen.lock().unwrap().push(msg);
		Ok(())
	})
}

#[tokio::test]
async fn messages_reach_the_transport() {
	let (seen, send) = recorder();
	let mut sink = UnbufferedSink::new(send);

	sink.send(Message::Binary(vec![1, 2, 3].into())).await.unwrap();
	sink.send(Message::Binary(vec![4].into())).await.unwrap();

	assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_dropped_datagram_is_not_an_error() {
	// An unreliable transport drops rather than fails. Reporting an error here
	// would tear down a connection that is working as designed.
	assert!(resolve_outcome(SendOutcome::Dropped("send window full")).is_ok());
	assert!(resolve_outcome(SendOutcome::Dropped("too large for one datagram")).is_ok());
}

#[tokio::test]
async fn a_connection_failure_still_surfaces() {
	assert!(resolve_outcome(SendOutcome::Fatal).is_err());
}

#[tokio::test]
async fn a_sent_datagram_is_ok() {
	assert!(resolve_outcome(SendOutcome::Sent).is_ok());
}

#[tokio::test]
async fn the_sink_never_applies_backpressure() {
	// The whole point of the datagram path is that a slow transport drops
	// stale data instead of holding fresher data behind it. If this sink ever
	// became not-ready, a pose would queue and arrive after it mattered.
	let (seen, send) = recorder();
	let mut sink = UnbufferedSink::new(send);

	for i in 0..10_000u32 {
		sink.feed(Message::Binary(i.to_be_bytes().to_vec().into()))
			.await
			.unwrap();
	}
	sink.flush().await.unwrap();

	assert_eq!(seen.lock().unwrap().len(), 10_000);
}

#[tokio::test]
async fn flush_and_close_are_immediate() {
	// Nothing is buffered, so neither can block.
	let (_seen, send) = recorder();
	let mut sink = UnbufferedSink::new(send);

	sink.flush().await.unwrap();
	sink.close().await.unwrap();
}
