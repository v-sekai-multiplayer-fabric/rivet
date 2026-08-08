use super::{
	RtpHeader, SequenceTracker, build_rtp, decode_varint, encode_varint, parse_datagram, parse_rtp,
};

fn varint(value: u64) -> Vec<u8> {
	let mut out = Vec::new();
	encode_varint(value, &mut out).unwrap();
	out
}

#[test]
fn varints_use_the_lengths_rfc_9000_specifies() {
	// The top two bits carry the length, so a small flow identifier costs one
	// byte. Getting these boundaries wrong misreads every following field.
	assert_eq!(varint(0).len(), 1);
	assert_eq!(varint(63).len(), 1);
	assert_eq!(varint(64).len(), 2);
	assert_eq!(varint(16_383).len(), 2);
	assert_eq!(varint(16_384).len(), 4);
	assert_eq!(varint(1_073_741_823).len(), 4);
	assert_eq!(varint(1_073_741_824).len(), 8);
}

#[test]
fn varints_round_trip() {
	for value in [0u64, 1, 63, 64, 16_383, 16_384, 1_073_741_823, 1_073_741_824, (1 << 62) - 1] {
		let encoded = varint(value);
		let (decoded, used) = decode_varint(&encoded).unwrap();
		assert_eq!(decoded, value);
		assert_eq!(used, encoded.len());
	}
}

#[test]
fn a_varint_over_62_bits_is_rejected() {
	let mut out = Vec::new();
	assert!(encode_varint(1 << 62, &mut out).is_err());
}

#[test]
fn a_truncated_varint_is_rejected() {
	// The length byte promises more than the buffer holds. Reading past it
	// would take bytes from the RTP header.
	assert!(decode_varint(&[0b1000_0000]).is_err());
	assert!(decode_varint(&[]).is_err());
}

#[test]
fn an_rtp_packet_round_trips() {
	let header = RtpHeader {
		sequence: 4321,
		timestamp: 90_000,
		ssrc: 0xDEAD_BEEF,
		payload_type: 96,
		marker: true,
		payload_offset: super::RTP_MIN_HEADER,
	};
	let packet = build_rtp(&header, b"pose");

	let parsed = parse_rtp(&packet).unwrap();
	assert_eq!(parsed.sequence, 4321);
	assert_eq!(parsed.timestamp, 90_000);
	assert_eq!(parsed.ssrc, 0xDEAD_BEEF);
	assert_eq!(parsed.payload_type, 96);
	assert!(parsed.marker);
	assert_eq!(&packet[parsed.payload_offset..], b"pose");
}

#[test]
fn a_short_rtp_packet_is_rejected() {
	assert!(parse_rtp(&[0x80, 96, 0, 1]).is_err());
}

#[test]
fn a_wrong_rtp_version_is_rejected() {
	let mut packet = build_rtp(
		&RtpHeader {
			sequence: 1,
			timestamp: 0,
			ssrc: 1,
			payload_type: 96,
			marker: false,
			payload_offset: super::RTP_MIN_HEADER,
		},
		b"x",
	);
	packet[0] = 0b0100_0000; // version 1
	assert!(parse_rtp(&packet).is_err());
}

#[test]
fn a_csrc_list_moves_the_payload() {
	// A packet with CSRC entries has a longer header. Assuming 12 bytes would
	// read four bytes of header as payload for each entry.
	let mut packet = build_rtp(
		&RtpHeader {
			sequence: 7,
			timestamp: 0,
			ssrc: 1,
			payload_type: 96,
			marker: false,
			payload_offset: super::RTP_MIN_HEADER,
		},
		b"",
	);
	packet[0] |= 2; // two CSRC entries
	packet.extend_from_slice(&[0u8; 8]);
	packet.extend_from_slice(b"pose");

	let parsed = parse_rtp(&packet).unwrap();
	assert_eq!(parsed.payload_offset, super::RTP_MIN_HEADER + 8);
	assert_eq!(&packet[parsed.payload_offset..], b"pose");
}

#[test]
fn a_datagram_splits_into_flow_and_rtp() {
	let mut datagram = varint(9);
	datagram.extend_from_slice(&build_rtp(
		&RtpHeader {
			sequence: 1,
			timestamp: 0,
			ssrc: 5,
			payload_type: 96,
			marker: false,
			payload_offset: super::RTP_MIN_HEADER,
		},
		b"pose",
	));

	let (flow, rtp) = parse_datagram(&datagram).unwrap();
	assert_eq!(flow, 9);
	assert_eq!(parse_rtp(rtp).unwrap().sequence, 1);
}

#[test]
fn a_newer_sequence_is_accepted() {
	let mut t = SequenceTracker::new();
	assert!(t.accept(1).is_some());
	assert!(t.accept(2).is_some());
	assert!(t.accept(100).is_some());
}

#[test]
fn a_superseded_sequence_is_rejected() {
	// A pose that arrives after a newer one has been superseded. Applying it
	// moves the avatar backwards.
	let mut t = SequenceTracker::new();
	t.accept(100).unwrap();
	assert!(t.accept(99).is_none());
	assert!(t.accept(50).is_none());
}

#[test]
fn a_duplicate_is_rejected() {
	let mut t = SequenceTracker::new();
	t.accept(42).unwrap();
	assert!(t.accept(42).is_none());
}

#[test]
fn the_sequence_keeps_rising_across_a_rollover() {
	// RTP's sequence is 16 bits and wraps about every 17 minutes at 64 Hz.
	// The extended value must keep increasing, or the stream freezes for half
	// a cycle after every wrap.
	let mut t = SequenceTracker::new();

	let before = t.accept(65_534).unwrap();
	let at = t.accept(65_535).unwrap();
	let after = t.accept(0).unwrap();
	let later = t.accept(1).unwrap();

	assert!(at > before, "{at} should exceed {before}");
	assert!(after > at, "{after} should exceed {at} across the rollover");
	assert!(later > after, "{later} should exceed {after}");
}

#[test]
fn an_old_sequence_after_a_rollover_is_still_rejected() {
	let mut t = SequenceTracker::new();
	t.accept(65_535).unwrap();
	t.accept(2).unwrap();

	// 65_000 is behind, not a new cycle.
	assert!(t.accept(65_000).is_none());
}
