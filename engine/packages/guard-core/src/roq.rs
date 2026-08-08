//! RTP over QUIC framing, per `draft-ietf-avtcore-rtp-over-quic`.
//!
//! A QUIC DATAGRAM carries:
//!
//! ```text
//! Payload {
//!   Flow Identifier (i),
//!   RTP Packet (..),
//! }
//! ```
//!
//! The flow identifier multiplexes several flows onto one QUIC connection,
//! which is what lets an unreliable pose channel share a connection, and
//! therefore a congestion controller, with everything else.
//!
//! Following the standard rather than inventing framing buys two things beyond
//! interoperability. RTP already carries the sequence number and timestamp this
//! needs, and RTCP travels the same way, so loss and jitter reporting comes
//! from the protocol instead of from bespoke instrumentation.
//!
//! Reliable channels do not appear here. Each is an ordinary QUIC stream,
//! already reliable and ordered, and RoQ explicitly leaves non-RTP data on the
//! same connection out of scope.

use anyhow::{Result, bail, ensure};

/// Smallest RTP header, with no CSRC entries and no extension.
pub const RTP_MIN_HEADER: usize = 12;

/// RTP version 2, the only version in use.
const RTP_VERSION: u8 = 2;

/// Encode a QUIC variable-length integer, per RFC 9000 section 16.
///
/// The top two bits give the length, so small identifiers cost one byte and
/// large ones stay possible.
pub fn encode_varint(value: u64, out: &mut Vec<u8>) -> Result<()> {
	ensure!(value < (1 << 62), "varint {value} exceeds the 62-bit maximum");

	if value < (1 << 6) {
		out.push(value as u8);
	} else if value < (1 << 14) {
		out.extend_from_slice(&((value as u16) | 0b01 << 14).to_be_bytes());
	} else if value < (1 << 30) {
		out.extend_from_slice(&((value as u32) | 0b10 << 30).to_be_bytes());
	} else {
		out.extend_from_slice(&(value | 0b11 << 62).to_be_bytes());
	}

	Ok(())
}

/// Decode a QUIC variable-length integer. Returns the value and its size.
pub fn decode_varint(input: &[u8]) -> Result<(u64, usize)> {
	let Some(&first) = input.first() else {
		bail!("varint is empty");
	};

	let len = 1usize << (first >> 6);
	ensure!(
		input.len() >= len,
		"varint needs {len} bytes, got {}",
		input.len()
	);

	let mut value = u64::from(first & 0b0011_1111);
	for byte in &input[1..len] {
		value = (value << 8) | u64::from(*byte);
	}

	Ok((value, len))
}

/// The fields of an RTP header this code reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
	pub sequence: u16,
	pub timestamp: u32,
	pub ssrc: u32,
	pub payload_type: u8,
	pub marker: bool,
	/// Where the payload starts, after any CSRC list.
	pub payload_offset: usize,
}

/// Parse an RTP header, per RFC 3550 section 5.1.
pub fn parse_rtp(packet: &[u8]) -> Result<RtpHeader> {
	ensure!(
		packet.len() >= RTP_MIN_HEADER,
		"RTP packet is {} bytes, shorter than the {RTP_MIN_HEADER} byte header",
		packet.len()
	);

	let version = packet[0] >> 6;
	ensure!(version == RTP_VERSION, "RTP version {version} is not 2");

	let csrc_count = usize::from(packet[0] & 0b0000_1111);
	let payload_offset = RTP_MIN_HEADER + csrc_count * 4;
	ensure!(
		packet.len() >= payload_offset,
		"RTP packet declares {csrc_count} CSRC entries but is only {} bytes",
		packet.len()
	);

	Ok(RtpHeader {
		sequence: u16::from_be_bytes([packet[2], packet[3]]),
		timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
		ssrc: u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
		payload_type: packet[1] & 0b0111_1111,
		marker: packet[1] & 0b1000_0000 != 0,
		payload_offset,
	})
}

/// Build a minimal RTP packet: no CSRC list, no extension, no padding.
pub fn build_rtp(header: &RtpHeader, payload: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(RTP_MIN_HEADER + payload.len());

	out.push(RTP_VERSION << 6);
	out.push((u8::from(header.marker) << 7) | (header.payload_type & 0b0111_1111));
	out.extend_from_slice(&header.sequence.to_be_bytes());
	out.extend_from_slice(&header.timestamp.to_be_bytes());
	out.extend_from_slice(&header.ssrc.to_be_bytes());
	out.extend_from_slice(payload);

	out
}

/// Tracks RTP sequence rollover, per RFC 3550 appendix A.1.
///
/// RTP's sequence number is 16 bits, so at 64 Hz it wraps about every 17
/// minutes. Comparing the raw value would discard everything for half a cycle
/// after each wrap. Counting cycles turns it back into a number that only
/// increases.
#[derive(Debug, Default)]
pub struct SequenceTracker {
	cycles: u32,
	last: Option<u16>,
}

impl SequenceTracker {
	pub fn new() -> Self {
		Self::default()
	}

	/// Extend a 16-bit sequence to 48 bits using the cycle count.
	///
	/// Returns `None` when the sequence is not newer than the newest already
	/// seen, which is a superseded or duplicated packet.
	pub fn accept(&mut self, sequence: u16) -> Option<u64> {
		let Some(last) = self.last else {
			self.last = Some(sequence);
			return Some(u64::from(self.cycles) << 16 | u64::from(sequence));
		};

		let delta = sequence.wrapping_sub(last);

		// A small forward step is the ordinary case. Half the space is the
		// boundary between "newer, possibly wrapped" and "older".
		if delta == 0 || delta > u16::MAX / 2 {
			return None;
		}

		if sequence < last {
			self.cycles = self.cycles.wrapping_add(1);
		}
		self.last = Some(sequence);

		Some(u64::from(self.cycles) << 16 | u64::from(sequence))
	}
}

/// Split a RoQ datagram into its flow identifier and RTP packet.
pub fn parse_datagram(payload: &[u8]) -> Result<(u64, &[u8])> {
	let (flow_id, consumed) = decode_varint(payload)?;
	Ok((flow_id, &payload[consumed..]))
}

#[cfg(test)]
#[path = "roq/tests.rs"]
mod tests;
