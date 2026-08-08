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
