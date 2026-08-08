/// Channels over one WebTransport session.
///
/// The design is deliberately minimal: many reliable channels, one unreliable.
/// A reliable channel is a bidirectional stream, which QUIC already keeps
/// ordered and free of head-of-line blocking against every other stream. The
/// unreliable channel is the session's datagrams, which need no framing at all
/// because there is only one of them.
///
/// Every stream names its target before WebSocket framing begins: a 2-byte
/// big-endian length, then the path. `rivet_unreliable=1` in that path tells
/// Guard to serve the connection over datagrams instead of the stream.

/// Encode the routing header a stream must send first.
export function routingHeader(path: string): Uint8Array {
	const encoded = new TextEncoder().encode(path);
	const out = new Uint8Array(2 + encoded.length);
	new DataView(out.buffer).setUint16(0, encoded.length);
	out.set(encoded, 2);
	return out;
}

export interface ReliableChannel {
	writer: WritableStreamDefaultWriter<Uint8Array>;
	reader: ReadableStreamDefaultReader<Uint8Array>;
	close(): Promise<void>;
}

export class ZoneTransport {
	#wt: WebTransport;

	private constructor(wt: WebTransport) {
		this.#wt = wt;
	}

	/// Open a session. `spkiHash` is only for local development, where the
	/// server runs a self-signed certificate accepted via a Chromium launch
	/// flag; a deployed server presents a real certificate and needs nothing.
	static async connect(url: string): Promise<ZoneTransport> {
		const wt = new WebTransport(url);
		await wt.ready;
		return new ZoneTransport(wt);
	}

	/// Open a reliable channel to an actor path.
	async reliable(path: string): Promise<ReliableChannel> {
		const stream = await this.#wt.createBidirectionalStream();
		const writer = stream.writable.getWriter();
		await writer.write(routingHeader(path));

		return {
			writer,
			reader: stream.readable.getReader(),
			close: async () => {
				await writer.close();
			},
		};
	}

	/// Claim the session's one unreliable channel for an actor path.
	///
	/// The stream carries only the routing header; every payload after that
	/// travels as a datagram. One per session, because QUIC datagrams carry no
	/// stream identity to demultiplex on.
	async unreliable(path: string): Promise<{
		writer: WritableStreamDefaultWriter<Uint8Array>;
		reader: ReadableStreamDefaultReader<Uint8Array>;
	}> {
		const flagged = path.includes("?")
			? `${path}&rivet_unreliable=1`
			: `${path}?rivet_unreliable=1`;

		const stream = await this.#wt.createBidirectionalStream();
		const writer = stream.writable.getWriter();
		await writer.write(routingHeader(flagged));

		return {
			writer: this.#wt.datagrams.writable.getWriter(),
			reader: this.#wt.datagrams.readable.getReader(),
		};
	}

	close(): void {
		this.#wt.close();
	}
}
