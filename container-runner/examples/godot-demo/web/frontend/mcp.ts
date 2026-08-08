/// MCP JSON-RPC over the actor's raw fetch surface.
///
/// `container-runner` delivers `/request/*` to the child with the prefix
/// stripped, and the RivetKit client handles actor addressing, so a call here
/// is `actor.fetch("/mcp", …)` rather than a hand-built URL with
/// `x-rivet-actor` headers.

export interface ActorFetch {
	fetch(path: string, init?: RequestInit): Promise<Response>;
}

let nextId = 1;

/// Call one MCP tool. Throws on both JSON-RPC errors and the zone's own
/// `{ok: false}` tool errors, so a caller never has to check two shapes.
export async function callTool(
	actor: ActorFetch,
	name: string,
	args: Record<string, unknown>,
): Promise<any> {
	const res = await actor.fetch("/mcp", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({
			jsonrpc: "2.0",
			id: nextId++,
			method: "tools/call",
			params: { name, arguments: args },
		}),
	});
	if (!res.ok) throw new Error(`${name}: HTTP ${res.status}`);

	const rpc = await res.json();
	if (rpc.error) throw new Error(`${name}: ${rpc.error.message ?? "rpc error"}`);

	const text = rpc.result?.content?.[0]?.text;
	if (typeof text !== "string") throw new Error(`${name}: no content returned`);

	const out = JSON.parse(text);
	if (out && out.ok === false) throw new Error(`${name}: ${out.error ?? "tool error"}`);
	return out;
}

/// Base64 without blowing the stack. `btoa(String.fromCharCode(...bytes))`
/// spreads every byte as an argument and dies well before 4 MiB.
export function toBase64(bytes: Uint8Array): string {
	let out = "";
	const step = 0x8000;
	for (let i = 0; i < bytes.length; i += step) {
		out += String.fromCharCode(...bytes.subarray(i, i + step));
	}
	return btoa(out);
}

export function fromBase64(b64: string): Uint8Array {
	const bin = atob(b64);
	const out = new Uint8Array(bin.length);
	for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
	return out;
}
