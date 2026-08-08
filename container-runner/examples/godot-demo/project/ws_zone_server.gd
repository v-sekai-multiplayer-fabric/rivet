extends SceneTree
# A WebSocket echo zone, run as the child of a Rivet actor.
#
# The client does not speak WebSocket. Guard terminates WebTransport from the
# client and carries WebSocket frames over Rivet's tunnel, so the child only
# needs a plain WebSocket server on its injected PORT.
#
# container-runner probes readiness with a TCP connect, which a WebSocket
# listener satisfies without a beacon.
#
# Usage:
#   godot --headless --path <project> --script ws_zone_server.gd

var port: int = int(OS.get_environment("PORT")) if OS.get_environment("PORT") != "" else 7770

var tcp_server: TCPServer
var peers: Array[WebSocketPeer] = []
var pending: Array[StreamPeerTCP] = []

func _init() -> void:
	tcp_server = TCPServer.new()
	var err := tcp_server.listen(port)
	if err != OK:
		printerr("listen on %d failed: %d" % [port, err])
		quit(1)
		return
	# Rivet's own demo prints a JSON ready line, so keep the same shape here.
	print(JSON.stringify({"event": "ready", "port": port, "transport": "websocket"}))

func _process(_delta: float) -> bool:
	_accept_new()
	_pump_handshakes()
	_pump_peers()
	return false

func _accept_new() -> void:
	while tcp_server.is_connection_available():
		var conn := tcp_server.take_connection()
		if conn:
			pending.append(conn)

func _pump_handshakes() -> void:
	# A TCP connection becomes a WebSocket peer once accept_stream succeeds.
	var still_pending: Array[StreamPeerTCP] = []
	for conn in pending:
		var peer := WebSocketPeer.new()
		if peer.accept_stream(conn) == OK:
			peers.append(peer)
		else:
			still_pending.append(conn)
	pending = still_pending

func _pump_peers() -> void:
	var live: Array[WebSocketPeer] = []
	for peer in peers:
		peer.poll()
		match peer.get_ready_state():
			WebSocketPeer.STATE_OPEN:
				while peer.get_available_packet_count() > 0:
					var pkt := peer.get_packet()
					# Match the echo shape the e2e client asserts on.
					var text := pkt.get_string_from_utf8()
					peer.send_text("echo: %s" % text)
				live.append(peer)
			WebSocketPeer.STATE_CONNECTING:
				live.append(peer)
			_:
				pass
	peers = live
