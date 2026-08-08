# ShaderMotion frame decoding.
#
# Ported from MotionDecoder.js in V-Sekai/shader-motion-navy-lead-ostrich,
# which is MIT licensed and copyright lox9973, itself a port of ShaderImpl.cs
# and MotionLayout.cs from gitlab.com/lox9973/ShaderMotion.
#
# The wire payload is the slot array, not the video. A performer samples the
# ShaderMotion texture and sends slots 0 through 129; this turns those into
# bone poses. Splitting it there is what keeps a frame inside one datagram,
# because 130 slots is around 260 bytes at 16 bits while a video frame is not.
#
# Slot values are reals in [-1, +1]. Most bones carry swing-twist angles in
# XYZ scaled from [-180, +180], which is the representation godot-humanoid's
# bone_swing_twists already uses. The Hips are the exception and carry a
# position, an orthogonalized rotation, and a scale.
extends RefCounted
class_name ShaderMotion

# Slots 0 through 129 inclusive. Derived from the layout table below rather
# than asserted: max(base[i] + len(channels[i])) - 1 is 129.
const SLOT_COUNT := 130

const BONE_COUNT := 55

# Index of each bone's first slot. Bone order is Unity HumanBodyBones, which
# godot-humanoid mirrors in human_trait.gd.
const BASE_INDICES: Array[int] = [0, 27, 30, 33, 36, 39, 42, 12, 15, 21, 24, 45, 48, 51, 54, 57, 60, 63, 66, 69, 70, 71, 73, 75, 90, 92, 93, 94, 96, 97, 98, 100, 101, 102, 104, 105, 106, 108, 109, 110, 112, 113, 114, 116, 117, 118, 120, 121, 122, 124, 125, 126, 128, 129, 18]

const POSITION_SCALE := 2.0
const ROTATION_TOLERANCE := 0.1

# Which of the 15 working channels each bone fills. A row starting below 3
# means the bone stores swing-twist degrees directly.
const CHANNELS: Array = [
	[3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[0, 1, 2],
	[2],
	[2],
	[1, 2],
	[1, 2],
	[1, 2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[1, 2],
	[2],
	[2],
	[0, 1, 2],
]


# Recover a higher-precision real from a hi and lo slot pair.
#
# The encoding is a base-3 Gray curve, so hi and lo are not independent digits
# and cannot be combined by simple scaling.
static func decode_video_float(hi: float, lo: float, pow_: float) -> float:
	hi = hi * ((pow_ - 1.0) / 2.0) + (pow_ - 1.0) / 2.0
	lo = lo * ((pow_ - 1.0) / 2.0) + (pow_ - 1.0) / 2.0

	var x: float = round(lo)
	var y: float = minf(lo - x, 0.0)
	var z: float = maxf(lo - x, 0.0)
	var r: float = round(hi)

	if int(r) & 1 != 0:
		var nx: float = pow_ - 1.0 - x
		var ny: float = -z
		var nz: float = -y
		x = nx
		y = ny
		z = nz

	if is_equal_approx(x, 0.0):
		y += minf(0.0, hi - r)
	if is_equal_approx(x, pow_ - 1.0):
		z += maxf(0.0, hi - r)

	x += r * pow_
	x -= (pow_ * pow_ - 1.0) / 2.0
	y += 0.5
	z -= 0.5

	var denom := maxf(absf(y), absf(z))
	if denom == 0.0:
		return 0.0

	return (((y + z) / denom) * 0.5 + x) / ((pow_ - 1.0) / 2.0)


# Build a rotation from swing-twist degrees, matching HumanAxes.cs.
static func swing_twist(x: float, y: float, z: float) -> Quaternion:
	var degree_yz: Vector3 = Vector3(0.0, y, z)
	var length: float = degree_yz.length()
	if length == 0.0:
		return Quaternion(Vector3(1.0, 0.0, 0.0), deg_to_rad(x))

	var axis: Vector3 = degree_yz / length
	var q: Quaternion = Quaternion(axis, deg_to_rad(length))
	return q * Quaternion(Vector3(1.0, 0.0, 0.0), deg_to_rad(x))


# Make two near-orthogonal vectors exactly orthogonal without favouring
# either, so a rotation built from them is not biased toward one axis.
static func orthogonalize(u: Vector3, v: Vector3) -> Array:
	var b: float = u.dot(v) * -2.0
	var a: float = u.dot(u) + v.dot(v)
	a += sqrt(maxf(0.0, a * a - b * b))

	var big_u: Vector3 = u * a + v * b
	var uu: float = big_u.dot(big_u)
	if uu != 0.0:
		big_u = big_u * (u.dot(big_u) / uu)
	else:
		big_u = Vector3.ZERO

	var big_v: Vector3 = v * a + u * b
	var vv: float = big_v.dot(big_v)
	if vv != 0.0:
		big_v = big_v * (v.dot(big_v) / vv)
	else:
		big_v = Vector3.ZERO

	return [big_u, big_v]


# Decode one frame.
#
# Returns BONE_COUNT dictionaries. Every bone carries `swing_twist` in degrees,
# which is what a humanoid rig consumes. The Hips additionally carry
# `position`, `rotation`, and `scale`, and set `has_transform`.
static func decode(slots: PackedFloat32Array) -> Array:
	assert(slots.size() >= SLOT_COUNT, "a frame is %d slots, got %d" % [SLOT_COUNT, slots.size()])

	# tileRadix 3, tileLen 2, so 3 ** 6.
	var tile_pow: float = 729.0
	var out: Array = []

	for i in range(BONE_COUNT):
		var vec: Array = [Vector3.ZERO, Vector3.ZERO, Vector3.ZERO, Vector3.ZERO, Vector3.ZERO]
		var idx: int = BASE_INDICES[i]
		var channels: Array = CHANNELS[i]

		for j in channels:
			var v: Vector3 = vec[j / 3]
			v[j % 3] = slots[idx]
			vec[j / 3] = v
			idx += 1

		var bone: Dictionary = {
			"swing_twist": Vector3.ZERO,
			"has_transform": false,
		}

		if channels[0] < 3:
			# The common path: the slots already are swing-twist, scaled.
			bone["swing_twist"] = vec[0] * 180.0
		else:
			var v2: Vector3 = vec[2]
			for j in range(3):
				v2[j] = decode_video_float(vec[1][j], vec[2][j], tile_pow)

			var ortho: Array = orthogonalize(vec[3], vec[4])
			var rot_y: Vector3 = ortho[0]
			var rot_z: Vector3 = ortho[1]

			var len_y: float = rot_y.length()
			var len_z: float = rot_z.length()
			var err: float = (
				vec[3].distance_squared_to(rot_y)
				+ vec[4].distance_squared_to(rot_z)
				+ pow(maxf(0.1 - maxf(len_y, len_z), 0.0), 2.0)
			)

			# A degenerate frame gives zero-length axes, and looking_at treats
			# that as an error rather than returning identity. Reject it here
			# so a dropped or blank frame does not spam the log.
			if len_y == 0.0 or len_z == 0.0:
				bone["valid"] = false
				out.append(bone)
				continue

			if err > ROTATION_TOLERANCE * ROTATION_TOLERANCE:
				# The frame did not carry a usable rotation. Reporting it is
				# better than emitting a plausible wrong pose.
				bone["valid"] = false
				out.append(bone)
				continue

			bone["position"] = v2 * POSITION_SCALE
			if len_z != 0.0:
				bone["scale"] = len_y / len_z
			else:
				bone["scale"] = 1.0
			bone["rotation"] = Basis.looking_at(rot_z, rot_y).get_rotation_quaternion()
			bone["has_transform"] = true

		bone["valid"] = true
		out.append(bone)

	return out
