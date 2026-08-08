# Checks the ShaderMotion port against values produced by the JavaScript
# reference in V-Sekai/shader-motion-navy-lead-ostrich, so a translation slip
# in the base-3 Gray decoding fails here rather than as a subtly wrong pose.
extends SceneTree

const ShaderMotionLib = preload("res://shader_motion.gd")

const REFERENCE := [
	[-1, -1, -730],
	[-1, -0.5, -729.5],
	[-1, 0, -729],
	[-1, 0.5, -728.5],
	[-1, 1, -728],
	[-0.5, -1, -365.5],
	[-0.5, -0.5, -365],
	[-0.5, 0, -364.5],
	[-0.5, 0.5, -364],
	[-0.5, 1, -363.5],
	[-0.25, -1, -181.25],
	[-0.25, -0.5, -181.75],
	[-0.25, 0, -182.25],
	[-0.25, 0.5, -182.75],
	[-0.25, 1, -183.25],
	[0, -1, -1],
	[0, -0.5, -0.5],
	[0, 0, 0],
	[0, 0.5, 0.5],
	[0, 1, 1],
	[0.25, -1, 183.25],
	[0.25, -0.5, 182.75],
	[0.25, 0, 182.25],
	[0.25, 0.5, 181.75],
	[0.25, 1, 181.25],
	[0.5, -1, 363.5],
	[0.5, -0.5, 364],
	[0.5, 0, 364.5],
	[0.5, 0.5, 365],
	[0.5, 1, 365.5],
	[0.75, -1, 547.75],
	[0.75, -0.5, 547.25],
	[0.75, 0, 546.75],
	[0.75, 0.5, 546.25],
	[0.75, 1, 545.75],
	[1, -1, 728],
	[1, -0.5, 728.5],
	[1, 0, 729],
	[1, 0.5, 729.5],
	[1, 1, 730],
]


func _init() -> void:
	var failures := 0

	for case in REFERENCE:
		var got: float = ShaderMotionLib.decode_video_float(case[0], case[1], 729.0)
		var want: float = case[2]
		if absf(got - want) > 1e-6:
			print("MISMATCH hi=%f lo=%f want=%.9f got=%.9f" % [case[0], case[1], want, got])
			failures += 1

	print("decode_video_float: %d cases, %d failures" % [REFERENCE.size(), failures])

	# A frame of zeroes must still decode to the right shape.
	var slots := PackedFloat32Array()
	slots.resize(ShaderMotionLib.SLOT_COUNT)
	var bones := ShaderMotionLib.decode(slots)
	print("bones decoded: %d" % bones.size())
	if bones.size() != ShaderMotionLib.BONE_COUNT:
		failures += 1

	# Every bone but the Hips is on the swing-twist path.
	var with_transform := 0
	for b in bones:
		if b["has_transform"]:
			with_transform += 1
	print("bones carrying a transform: %d" % with_transform)

	# Slot count must match what the layout actually touches.
	var max_slot := 0
	for i in range(ShaderMotionLib.BONE_COUNT):
		var last: int = ShaderMotionLib.BASE_INDICES[i] + ShaderMotionLib.CHANNELS[i].size() - 1
		max_slot = maxi(max_slot, last)
	print("max slot touched: %d, SLOT_COUNT: %d" % [max_slot, ShaderMotionLib.SLOT_COUNT])
	if max_slot + 1 != ShaderMotionLib.SLOT_COUNT:
		failures += 1

	print("RESULT: %s" % ("PASS" if failures == 0 else "FAIL"))
	quit(1 if failures > 0 else 0)
