extends SceneTree
## Convert a glTF binary to a compressed packed Godot scene.
##
##   godot --headless --path /opt/zone --script convert.gd -- <in.glb> <out.scn>
##
## Run as a one-shot process per conversion rather than as an endpoint on the
## zone's HTTP port. A 100 MB import holds the whole buffer plus the generated
## scene in memory, and a separate process returns that memory to the OS when it
## exits instead of leaving the long-lived zone actor holding the high-water
## mark.

func _init() -> void:
	var args := OS.get_cmdline_user_args()
	if args.size() < 2:
		printerr("usage: convert.gd -- <in.glb> <out.scn.zst>")
		quit(2)
		return

	var in_path: String = args[0]
	var out_path: String = args[1]

	var glb := FileAccess.get_file_as_bytes(in_path)
	if glb.is_empty():
		printerr("convert: cannot read %s (err %d)" % [in_path, FileAccess.get_open_error()])
		quit(3)
		return

	var doc := GLTFDocument.new()
	var state := GLTFState.new()
	# base_path is where external references resolve from. A .glb is
	# self-contained, so the input's directory is the correct and only answer.
	var err := doc.append_from_buffer(glb, in_path.get_base_dir(), state)
	if err != OK:
		printerr("convert: append_from_buffer failed (err %d)" % err)
		quit(4)
		return

	var root := doc.generate_scene(state)
	if root == null:
		printerr("convert: generate_scene returned null")
		quit(5)
		return

	# PackedScene only stores a node's children when their `owner` points at the
	# scene root. generate_scene does not set that, so pack() would otherwise
	# write a single empty root.
	_claim(root, root)

	var packed := PackedScene.new()
	err = packed.pack(root)
	if err != OK:
		printerr("convert: pack failed (err %d)" % err)
		quit(6)
		return

	# FLAG_COMPRESS writes Godot's own compressed resource container, magic
	# "RSCC". It is self-describing: ResourceLoader.load reads it directly with
	# no size passed alongside and no separate decompress step.
	#
	# Compressing the packed bytes by hand instead gives a roughly 3x better
	# ratio, because RSCC compresses in blocks to stay seekable while a one-shot
	# pass over the whole buffer packs tighter. It is still the wrong trade
	# here: the client would have to carry the uncompressed size out of band,
	# decompress before loading, and hold both copies at once. For a 100 MB
	# asset that peak matters more than the transfer does.
	err = ResourceSaver.save(packed, out_path, ResourceSaver.FLAG_COMPRESS)
	if err != OK:
		printerr("convert: ResourceSaver.save failed (err %d)" % err)
		quit(7)
		return

	var written := FileAccess.get_file_as_bytes(out_path)
	if written.is_empty():
		printerr("convert: wrote an empty scene")
		quit(8)
		return

	# One JSON line on stdout, so the caller parses rather than scrapes.
	print(JSON.stringify({
		"event": "converted",
		"glb_bytes": glb.size(),
		"scene_bytes": written.size(),
		"format": written.slice(0, 4).get_string_from_ascii(),
	}))
	quit(0)

## Give every descendant the scene root as owner, so PackedScene keeps it.
func _claim(node: Node, owner_node: Node) -> void:
	for child in node.get_children():
		child.owner = owner_node
		_claim(child, owner_node)
