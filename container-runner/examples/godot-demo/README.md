# Godot MCP zone

A Rivet actor running headless Godot with two surfaces in one process. An agent
drives the live SceneTree over MCP through Rivet's gateway, while game clients
connect over WebSocket.

| Surface   | Port       | Protocol | Reached by       |
| --------- | ---------- | -------- | ---------------- |
| MCP       | `PORT`     | HTTP     | Rivet's tunnel   |
| Game zone | `PORT + 1` | WebSocket | Direct, for now |

MCP takes the proxied port on purpose. `container-runner` forwards one port, so
putting MCP there puts Rivet's routing and auth in front of the agent surface.

## What makes this work

`vsekai-godot-mcp` is an in-editor addon, and it also has a runtime path.
`addons/vsekai_godot_mcp/mcp_runtime.gd` describes itself as the same MCP server
as the editor plugin, running inside the deployed game over its live SceneTree.
That is the bridge `rfd/0054` calls for.

Editor-only commands return an error. Scene, node, eval, screenshot, and
performance commands work against the running game.

`mcp_runtime.gd` hardcodes `HTTP_PORT := 8788`, so `project/zone_main.gd` drives
`mcp_http_server.gd` directly and passes the injected `PORT` instead. Its
`start(port, host)` already takes both, so the addon needs no patch.

The addon is fetched at a pinned commit,
`580bb5fedc7c1bb56eb38b8377f918d9c5ffc998`, because the repository publishes no
tags. Override with `--build-arg MCP_COMMIT=...`.

## Engine

The image builds on `Godot_v4.7.1-stable_linux.x86_64` from the upstream Godot
release. Godot 4 ships one Linux binary that runs headless under `--headless`,
so there is no separate server download. Override with
`--build-arg GODOT_VERSION=...`.

## Ready line

The zone prints one JSON line on stdout, so a supervisor can parse it:

```json
{"event": "ready", "port": 7770, "mcp_path": "/mcp", "ws_port": 7771}
```

Readiness needs no beacon, because the MCP listener is TCP on `PORT` and
`container-runner` probes with `TcpStream::connect`.

## Build

Build from the repository root, because the runner binary comes from this
workspace:

```sh
docker build -f container-runner/examples/godot-demo/Dockerfile -t godot-mcp-zone .
```

## Reaching MCP

Raw HTTP arrives at the child under a `/request/*` prefix, which the runner
strips. So the actor's MCP endpoint is the gateway URL plus `/request/mcp`.

## Known gaps

**The game port is not proxied.** Rivet forwards one port, and MCP holds it. A
client reaches `PORT + 1` directly until Guard terminates WebTransport and
routes to the child.

**Editor-only MCP commands fail.** That is by design in `mcp_runtime.gd`, and it
means the demo shows scene inspection rather than scene authoring.
