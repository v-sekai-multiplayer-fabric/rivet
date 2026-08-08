import { createClient } from "@rivetkit/react";
import { useCallback, useRef, useState } from "react";
import { callTool, fromBase64, toBase64 } from "./mcp.ts";

// Same shape as examples/raw-fetch-handler: point the client at the engine and
// address the actor by name and key. The zone is a Rust actor hosted by
// container-runner, so there is no TypeScript registry to type against.
const ENGINE = import.meta.env.VITE_RIVET_ENDPOINT ?? "http://localhost:6420";
const ACTOR_NAME = import.meta.env.VITE_ACTOR_NAME ?? "game";

const client = createClient(ENGINE) as any;

type Phase = "idle" | "uploading" | "converting" | "downloading" | "done" | "error";

export default function App() {
	const [zoneKey, setZoneKey] = useState("zone-1");
	const [file, setFile] = useState<File | null>(null);
	const [phase, setPhase] = useState<Phase>("idle");
	const [progress, setProgress] = useState(0);
	const [log, setLog] = useState<string[]>([]);
	const [result, setResult] = useState<{ url: string; name: string; bytes: number } | null>(null);
	const objectUrl = useRef<string | null>(null);

	const say = useCallback((line: string) => setLog((l) => [...l, line]), []);

	const reset = () => {
		if (objectUrl.current) URL.revokeObjectURL(objectUrl.current);
		objectUrl.current = null;
		setResult(null);
		setLog([]);
		setProgress(0);
	};

	const onSubmit = async (e: React.FormEvent) => {
		e.preventDefault();
		if (!file) return;
		reset();

		const actor = client[ACTOR_NAME].getOrCreate([zoneKey]);

		try {
			// 1. Begin. The zone chooses the chunk size, so the page never
			//    hardcodes a limit that the server might change.
			setPhase("uploading");
			const begun = await callTool(actor, "asset_begin", { name: file.name });
			const chunkBytes: number = begun.chunk_bytes;
			say(`upload ${begun.id}, ${chunkBytes / 1048576} MiB chunks`);

			// 2. Upload. Offsets are checked server-side, so a duplicate or
			//    reordered chunk is refused rather than silently corrupting.
			const buf = new Uint8Array(await file.arrayBuffer());
			for (let off = 0; off < buf.length; off += chunkBytes) {
				const piece = buf.subarray(off, Math.min(off + chunkBytes, buf.length));
				await callTool(actor, "asset_chunk", {
					id: begun.id,
					offset: off,
					data: toBase64(piece),
				});
				setProgress(Math.round(((off + piece.length) / buf.length) * 100));
			}
			say(`uploaded ${buf.length.toLocaleString()} bytes`);

			// 3. Convert. Runs as a one-shot child process in the zone.
			setPhase("converting");
			const conv = await callTool(actor, "asset_convert", { id: begun.id });
			say(`converted to ${conv.format}, ${conv.scene_bytes.toLocaleString()} bytes`);

			// 4. Download.
			setPhase("downloading");
			const parts: Uint8Array[] = [];
			for (let seq = 0; seq < conv.chunks; seq++) {
				const got = await callTool(actor, "asset_fetch", { id: begun.id, seq });
				parts.push(fromBase64(got.data));
				setProgress(Math.round(((seq + 1) / conv.chunks) * 100));
				if (got.eof) break;
			}

			const blob = new Blob(parts as BlobPart[], { type: "application/octet-stream" });
			objectUrl.current = URL.createObjectURL(blob);
			setResult({
				url: objectUrl.current,
				name: file.name.replace(/\.(glb|gltf|vrm)$/i, "") + ".scn",
				bytes: blob.size,
			});
			setPhase("done");
		} catch (err) {
			say(String(err));
			setPhase("error");
		}
	};

	const busy = phase === "uploading" || phase === "converting" || phase === "downloading";

	return (
		<main style={{ fontFamily: "system-ui, sans-serif", maxWidth: 640, margin: "3rem auto", padding: "0 1rem" }}>
			<h1>glb → Godot scene</h1>
			<p style={{ color: "#555" }}>
				Posts a glb to a Godot zone actor over MCP and returns a compressed
				Godot scene (<code>RSCC</code>), which <code>ResourceLoader.load</code> reads directly.
			</p>

			<form onSubmit={onSubmit}>
				<label style={{ display: "block", marginBottom: "1rem" }}>
					Zone key
					<input
						value={zoneKey}
						onChange={(e) => setZoneKey(e.target.value)}
						disabled={busy}
						style={{ display: "block", width: "100%", padding: ".5rem", marginTop: ".25rem" }}
					/>
				</label>

				<label style={{ display: "block", marginBottom: "1rem" }}>
					Model
					<input
						type="file"
						accept=".glb,.gltf,.vrm"
						onChange={(e) => setFile(e.target.files?.[0] ?? null)}
						disabled={busy}
						style={{ display: "block", marginTop: ".25rem" }}
					/>
				</label>

				<button type="submit" disabled={!file || busy} style={{ padding: ".6rem 1.2rem" }}>
					{busy ? `${phase}…` : "Convert"}
				</button>
			</form>

			{busy && (
				<progress value={progress} max={100} style={{ width: "100%", marginTop: "1rem" }} />
			)}

			{result && (
				<p style={{ marginTop: "1.5rem" }}>
					<a href={result.url} download={result.name}>
						Download {result.name}
					</a>{" "}
					<span style={{ color: "#555" }}>({result.bytes.toLocaleString()} bytes)</span>
				</p>
			)}

			{log.length > 0 && (
				<pre style={{ background: "#f4f4f4", padding: "1rem", marginTop: "1.5rem", whiteSpace: "pre-wrap" }}>
					{log.join("\n")}
				</pre>
			)}
		</main>
	);
}
