extends "res://addons/vsekai_godot_mcp/mcp_protocol.gd"
## Adds the asset tools to `tools/list` so a client discovers them the normal
## way. The addon's own registry note applies unchanged: tool name equals the
## MCPCommands dispatch command, and arguments pass straight through.

func _tool_defs() -> Array:
	return super._tool_defs() + [
		["asset_begin",
			"Start a glb upload (glb only, not gltf). Returns an id and chunk size.",
			{ "name": "string" }],
		["asset_chunk",
			"Append one base64 chunk. Pass offset to make the append checked.",
			{ "id": "string", "data": "string", "offset": "integer" }],
		["asset_status",
			"Bytes received so far, for resuming an interrupted upload.",
			{ "id": "string" }],
		["asset_convert",
			"Convert the uploaded glb to a compressed Godot scene (RSCC).",
			{ "id": "string" }],
		["asset_fetch",
			"Fetch one base64 chunk of the converted scene.",
			{ "id": "string", "seq": "integer" }],
	]
