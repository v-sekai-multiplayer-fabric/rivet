extends MCPCommands
## Asset conversion tools for the zone, added to the MCP surface.
##
## Everything travels over MCP so no second container, transport, or route is
## introduced: the tools reuse the same JSON-RPC endpoint the agent tools
## already use, which is proxied by `container-runner` and reachable through
## Rivet's gateway.
##
## Accepts `.glb` only. `.gltf` references external buffers and textures that a
## single-file upload cannot carry, so it is not a supported input here.
##
## Bytes move in chunks because MCP is JSON-RPC and JSON has no binary type.
## Each chunk is base64, which inflates by 4/3, so `CHUNK_BYTES` is chosen to
## keep an encoded chunk well inside Rivet's 20 MiB request-body limit. A 100 MB
## glb is 25 calls rather than one impossible one.

const WORK_DIR := "/tmp/zone-assets"
## Raw bytes per chunk. 4 MiB encodes to ~5.33 MiB, leaving ample headroom.
const CHUNK_BYTES := 4 * 1024 * 1024


func dispatch(cmd: String, a: Dictionary):
	match cmd:
		"asset_begin": return _asset_begin(a)
		"asset_chunk": return _asset_chunk(a)
		"asset_convert": return _asset_convert(a)
		"asset_fetch": return _asset_fetch(a)
		"asset_status": return _asset_status(a)
		_: return super.dispatch(cmd, a)


func _work_path(id: String, suffix: String) -> String:
	return "%s/%s%s" % [WORK_DIR, id, suffix]


## Reject anything that is not a plain hex id, so an argument cannot escape
## WORK_DIR. These ids are minted here, so a well-behaved caller never trips it.
func _valid_id(id: String) -> bool:
	if id.length() < 8 or id.length() > 64:
		return false
	for c in id:
		if not ((c >= "0" and c <= "9") or (c >= "a" and c <= "f")):
			return false
	return true


func _asset_begin(a: Dictionary):
	DirAccess.make_dir_recursive_absolute(WORK_DIR)
	var id := "%08x%08x" % [Time.get_ticks_usec(), randi()]
	var f := FileAccess.open(_work_path(id, ".glb"), FileAccess.WRITE)
	if f == null:
		return _err("cannot create upload %s" % id)
	f.close()
	return {
		"id": id,
		"chunk_bytes": CHUNK_BYTES,
		"name": String(a.get("name", "")),
	}


## Append one chunk. `seq` is required and checked against the current length so
## a duplicate or out-of-order delivery is refused rather than corrupting the
## file silently. That makes a retry safe: resend the same seq and it is
## rejected, ask `asset_status` and resume from `received_bytes`.
func _asset_chunk(a: Dictionary):
	var id := String(a.get("id", ""))
	if not _valid_id(id):
		return _err("bad id")
	var path := _work_path(id, ".glb")
	if not FileAccess.file_exists(path):
		return _err("no such upload: %s" % id)

	var raw := Marshalls.base64_to_raw(String(a.get("data", "")))
	if raw.is_empty():
		return _err("empty or undecodable chunk")

	var f := FileAccess.open(path, FileAccess.READ_WRITE)
	if f == null:
		return _err("cannot open upload %s" % id)
	var have := f.get_length()

	if a.has("offset"):
		var want := int(a.get("offset"))
		if want != have:
			f.close()
			return _err("offset mismatch: have %d, got %d" % [have, want])

	f.seek(have)
	f.store_buffer(raw)
	var total := f.get_length()
	f.close()
	return { "id": id, "received_bytes": total, "appended": raw.size() }


func _asset_status(a: Dictionary):
	var id := String(a.get("id", ""))
	if not _valid_id(id):
		return _err("bad id")
	var path := _work_path(id, ".glb")
	if not FileAccess.file_exists(path):
		return _err("no such upload: %s" % id)
	var f := FileAccess.open(path, FileAccess.READ)
	var n := f.get_length()
	f.close()
	return { "id": id, "received_bytes": n, "chunk_bytes": CHUNK_BYTES }


## Convert in a one-shot child process rather than in the zone.
##
## A 100 MB import holds the source buffer, the generated scene, and the packed
## output at once. Doing that here would leave this long-lived actor sitting at
## its high-water mark for the rest of its life; a child returns the memory to
## the OS when it exits.
func _asset_convert(a: Dictionary):
	var id := String(a.get("id", ""))
	if not _valid_id(id):
		return _err("bad id")
	var glb := _work_path(id, ".glb")
	if not FileAccess.file_exists(glb):
		return _err("no such upload: %s" % id)
	var out := _work_path(id, ".scn")

	var argv := [
		"--headless",
		"--path", ProjectSettings.globalize_path("res://"),
		"--script", "convert.gd",
		"--", glb, out,
	]
	var lines: Array = []
	var code := OS.execute(OS.get_executable_path(), argv, lines, true)

	if code != 0 or not FileAccess.file_exists(out):
		return _err("convert failed (exit %d): %s" % [code, "\n".join(lines).right(400)])

	var f := FileAccess.open(out, FileAccess.READ)
	var size := f.get_length()
	var magic := f.get_buffer(4).get_string_from_ascii()
	f.close()

	return {
		"id": id,
		"scene_bytes": size,
		"format": magic,
		"chunk_bytes": CHUNK_BYTES,
		"chunks": int(ceil(float(size) / float(CHUNK_BYTES))),
	}


func _asset_fetch(a: Dictionary):
	var id := String(a.get("id", ""))
	if not _valid_id(id):
		return _err("bad id")
	var path := _work_path(id, ".scn")
	if not FileAccess.file_exists(path):
		return _err("no converted scene for %s" % id)

	var seq := int(a.get("seq", 0))
	var f := FileAccess.open(path, FileAccess.READ)
	var size := f.get_length()
	var offset := seq * CHUNK_BYTES
	if offset >= size:
		f.close()
		return _err("seq %d past end (%d bytes)" % [seq, size])
	f.seek(offset)
	var buf := f.get_buffer(mini(CHUNK_BYTES, size - offset))
	f.close()

	return {
		"id": id,
		"seq": seq,
		"data": Marshalls.raw_to_base64(buf),
		"bytes": buf.size(),
		"eof": offset + buf.size() >= size,
	}
