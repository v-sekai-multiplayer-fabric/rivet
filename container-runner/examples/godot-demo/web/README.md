# glb → Godot scene, from a web form

A page that posts a `.glb` to a Godot zone actor and downloads a compressed
Godot scene back.

**glb only.** `.gltf` references external `.bin` and texture files that a
single-file upload cannot carry, so it is excluded rather than merely
unsupported. vrm is deferred.

Follows `examples/raw-fetch-handler`: point a client at the engine, address the
actor by name and key, and use `actor.fetch(path)` for raw HTTP. The client
handles actor addressing, so this page never constructs a URL or sets
`x-rivet-actor` itself.

```sh
npm install
VITE_RIVET_ENDPOINT=http://localhost:6420 npm run dev
```

| Variable | Default | Meaning |
|---|---|---|
| `VITE_RIVET_ENDPOINT` | `http://localhost:6420` | engine the client connects to |
| `VITE_ACTOR_NAME` | `game` | matches `container-runner --actor-name` |

## What it does

Four MCP tools, in order. See
[RFD 0022](https://github.com/v-sekai-multiplayer-fabric/fabric-quickstart/blob/main/rfd/0022-glb-to-godot-scene.md).

1. `asset_begin` — the **zone** returns the chunk size, so this page does not
   hardcode a limit the server may change.
2. `asset_chunk` — base64 chunks at checked offsets. A duplicate or reordered
   chunk is refused rather than silently corrupting the upload.
3. `asset_convert` — one-shot child process in the zone.
4. `asset_fetch` — chunks back, reassembled into a download.

The result is Godot's `RSCC` container, which `ResourceLoader.load` reads
directly with no size passed alongside and no decompress step.

## Notes

**The client is untyped.** `examples/raw-fetch-handler` does
`createClient<typeof registry>` against a TypeScript registry. The zone is a
Rust actor hosted by `container-runner`, so there is no registry to type
against and the client is cast. Calls are checked at runtime by the MCP layer
instead.

**Base64 is chunked when encoding.** `btoa(String.fromCharCode(...bytes))`
spreads every byte as a function argument and overflows the stack well below
4 MiB, so `toBase64` walks in 32 KiB blocks.

**Untested against a live cluster.** The page builds and the MCP flow it drives
is verified against the zone container directly (43 MB round-trip, 17s). It has
not been run through Guard and the gateway, because the engine image has not
been built.
