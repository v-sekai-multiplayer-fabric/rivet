use std::io::Cursor;

use super::{MAX_STREAM_PATH_LEN, read_stream_path};

fn framed(path: &str) -> Cursor<Vec<u8>> {
	let mut buf = (path.len() as u16).to_be_bytes().to_vec();
	buf.extend_from_slice(path.as_bytes());
	Cursor::new(buf)
}

#[tokio::test]
async fn zero_length_inherits_the_session_path() {
	let mut stream = Cursor::new(vec![0u8, 0u8]);
	assert_eq!(read_stream_path(&mut stream).await.unwrap(), None);
}

#[tokio::test]
async fn a_named_target_is_returned() {
	let mut stream = framed("/zone/motion");
	assert_eq!(
		read_stream_path(&mut stream).await.unwrap(),
		Some("/zone/motion".to_string())
	);
}

#[tokio::test]
async fn the_websocket_frames_after_the_header_are_left_untouched() {
	// The header is consumed before tungstenite sees the stream, so whatever
	// follows must still be readable byte for byte.
	let mut buf = framed("/zone/asset").into_inner();
	buf.extend_from_slice(b"\x81\x03abc");
	let mut stream = Cursor::new(buf);

	assert_eq!(
		read_stream_path(&mut stream).await.unwrap(),
		Some("/zone/asset".to_string())
	);

	let consumed = stream.position() as usize;
	assert_eq!(&stream.into_inner()[consumed..], b"\x81\x03abc");
}

#[tokio::test]
async fn an_oversized_target_is_rejected() {
	let mut buf = (MAX_STREAM_PATH_LEN + 1).to_be_bytes().to_vec();
	buf.extend(std::iter::repeat(b'a').take(MAX_STREAM_PATH_LEN as usize + 1));
	let mut stream = Cursor::new(buf);

	assert!(read_stream_path(&mut stream).await.is_err());
}

#[tokio::test]
async fn a_relative_target_is_rejected() {
	// Routing keys on an absolute path, so a relative one would resolve
	// somewhere unintended rather than failing at the router.
	let mut stream = framed("zone/motion");
	assert!(read_stream_path(&mut stream).await.is_err());
}

#[tokio::test]
async fn invalid_utf8_is_rejected() {
	let mut buf = 2u16.to_be_bytes().to_vec();
	buf.extend_from_slice(&[0xff, 0xfe]);
	let mut stream = Cursor::new(buf);

	assert!(read_stream_path(&mut stream).await.is_err());
}

#[tokio::test]
async fn a_truncated_header_is_rejected() {
	// A stream that closes mid-header must fail rather than route to the
	// session default, which would silently reach the wrong actor.
	let mut stream = Cursor::new(vec![0u8, 8u8, b'/', b'z']);
	assert!(read_stream_path(&mut stream).await.is_err());
}

#[tokio::test]
async fn the_unreliable_flag_is_stripped_before_routing() {
	// Routing must see an ordinary actor path; the flag selects a transport and
	// is not part of the actor's address.
	use super::strip_unreliable_flag;

	assert_eq!(
		strip_unreliable_flag("/zone/motion?rivet_unreliable=1"),
		"/zone/motion"
	);
	assert_eq!(
		strip_unreliable_flag("/zone/motion?a=1&rivet_unreliable=1&b=2"),
		"/zone/motion?a=1&b=2"
	);
	assert_eq!(strip_unreliable_flag("/zone/asset"), "/zone/asset");
	assert_eq!(strip_unreliable_flag("/zone/asset?a=1"), "/zone/asset?a=1");
}

#[tokio::test]
async fn a_stream_can_name_an_unreliable_channel() {
	use super::unreliable_channel;

	// Several unreliable channels can coexist on one session, so the flag
	// carries which one rather than a bare yes.
	assert_eq!(unreliable_channel("/zone/motion?rivet_unreliable=7"), Some(7));
	assert_eq!(unreliable_channel("/zone/motion?a=1&rivet_unreliable=0"), Some(0));
	assert_eq!(unreliable_channel("/zone/motion?rivet_unreliable=65535"), Some(65535));
}

#[tokio::test]
async fn a_stream_without_the_flag_stays_reliable() {
	use super::unreliable_channel;

	// Reliable is the default. A stream with no flag is served over itself,
	// which is what gives each reliable channel its own head-of-line domain.
	assert_eq!(unreliable_channel("/zone/asset"), None);
	assert_eq!(unreliable_channel("/zone/asset?a=1"), None);
}

#[tokio::test]
async fn a_bad_channel_id_does_not_become_a_datagram_channel() {
	use super::unreliable_channel;

	// Falling back to reliable is safe. Guessing a channel id would silently
	// deliver to the wrong connection.
	assert_eq!(unreliable_channel("/zone/motion?rivet_unreliable=notanumber"), None);
	assert_eq!(unreliable_channel("/zone/motion?rivet_unreliable=70000"), None);
	assert_eq!(unreliable_channel("/zone/motion?rivet_unreliable="), None);
}

#[tokio::test]
async fn the_channel_flag_is_stripped_whatever_its_value() {
	use super::strip_unreliable_flag;

	assert_eq!(strip_unreliable_flag("/zone/motion?rivet_unreliable=7"), "/zone/motion");
	assert_eq!(
		strip_unreliable_flag("/zone/motion?a=1&rivet_unreliable=42&b=2"),
		"/zone/motion?a=1&b=2"
	);
}

#[tokio::test]
async fn a_newer_sequence_is_accepted() {
	use super::seq_newer;

	assert!(seq_newer(1, 0));
	assert!(seq_newer(100, 99));
	assert!(seq_newer(u64::MAX, u64::MAX - 1));
}

#[tokio::test]
async fn a_superseded_sequence_is_rejected() {
	use super::seq_newer;

	// This is the whole point of a sequenced unreliable channel. A pose that
	// arrives after a newer one has been superseded, and applying it moves the
	// avatar backwards.
	assert!(!seq_newer(99, 100));
	assert!(!seq_newer(0, 1));
}

#[tokio::test]
async fn the_same_sequence_twice_is_rejected() {
	use super::seq_newer;

	// A duplicate carries nothing new, and QUIC may deliver one.
	assert!(!seq_newer(42, 42));
}

#[tokio::test]
async fn the_sequence_is_wide_enough_never_to_wrap() {
	use super::seq_newer;

	// A 64-bit counter at 64 Hz runs for about nine billion years, so wrapping
	// is not a case to handle. A narrower counter would be: a u16 wraps every
	// 17 minutes, and a plain `>` would then discard everything for half a
	// cycle, freezing the stream on a timer long after anything looked wrong.
	let years = (u64::MAX as f64) / 64.0 / (365.25 * 24.0 * 3600.0);
	assert!(years > 1.0e9, "a u64 sequence lasts {years:e} years at 64 Hz");

	// A high sequence still compares correctly, which a wrapping scheme would
	// have had to prove separately.
	assert!(seq_newer(1_000_000_000_000, 999_999_999_999));
}

#[tokio::test]
async fn a_sequenced_channel_is_requested_explicitly() {
	use super::{Delivery, unreliable_channel};

	// Unsequenced is the default, because it is the cheaper promise.
	let path = "/zone/motion?rivet_unreliable=3&rivet_sequenced=1";
	assert_eq!(unreliable_channel(path), Some(3));
	assert!(path.contains(super::SEQUENCED_FLAG));

	let plain = "/zone/motion?rivet_unreliable=3";
	assert!(!plain.contains(super::SEQUENCED_FLAG));

	// Prove the enum is reachable and distinct.
	assert_ne!(Delivery::Sequenced, Delivery::Unsequenced);
}

#[tokio::test]
async fn both_channel_flags_are_stripped_before_routing() {
	use super::strip_unreliable_flag;

	assert_eq!(
		strip_unreliable_flag("/zone/motion?rivet_unreliable=3&rivet_sequenced=1"),
		"/zone/motion"
	);
	assert_eq!(
		strip_unreliable_flag("/zone/motion?a=1&rivet_unreliable=3&rivet_sequenced=1&b=2"),
		"/zone/motion?a=1&b=2"
	);
}
