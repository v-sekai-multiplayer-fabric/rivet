extends SceneTree
## A Rivet actor zone with two surfaces in one headless Godot process.
##
##   PORT     MCP over HTTP at /mcp, proxied by Rivet's tunnel.
##   PORT + 1 WebSocket echo zone for game traffic.
##
## MCP takes the proxied port on purpose. `container-runner` forwards one port,
## so putting MCP there lets an agent reach the live SceneTree through Rivet's
## gateway, with the gateway's routing and auth in front of it.
##
## The MCP server is the addon's own, unmodified. `mcp_runtime.gd` hardcodes
## HTTP_PORT to 8788, so this script drives `mcp_http_server.gd` directly and
## passes the injected port instead.
##
## Usage:
##   godot --headless --path <project> --script zone_main.gd

const MCPCommandsLib = preload("res://addons/vsekai_godot_mcp/mcp_commands.gd")
const MCPHttpServerLib = preload("res://addons/vsekai_godot_mcp/mcp_http_server.gd")

var mcp_port: int = int(OS.get_environment("PORT")) if OS.get_environment("PORT") != "" else 7770
var ws_port: int = mcp_port + 1

var _cmds = MCPCommandsLib.new()
var _http = MCPHttpServerLib.new()

var _ws_server: TCPServer
var _peers: Array[WebSocketPeer] = []
var _pending: Array[StreamPeerTCP] = []

func _init() -> void:
	_http.protocol.commands = _cmds
	if _http.start(mcp_port, "127.0.0.1") != OK:
		printerr("MCP HTTP listen failed on :%d" % mcp_port)
		quit(1)
		return

	_ws_server = TCPServer.new()
	if _ws_server.listen(ws_port) != OK:
		printerr("WebSocket listen failed on :%d" % ws_port)
		quit(1)
		return

	# One JSON line, so a supervisor can parse it from stdout. The same shape the
	# engine's own WebTransport demo prints.
	print(JSON.stringify({
		"event": "ready",
		"port": mcp_port,
		"mcp_path": "/mcp",
		"ws_port": ws_port,
	}))

func _process(_delta: float) -> bool:
	# The addon resolves commands against a root node. There is no editor here,
	# and no current_scene under `--script`, so the tree root is the subject.
	_cmds.editor = null
	_cmds.root = get_root().get_child(0) if get_root().get_child_count() > 0 else get_root()
	_http.poll()

	_accept_ws()
	_pump_handshakes()
	_pump_peers()
	return false

func _finalize() -> void:
	_http.stop()

func _accept_ws() -> void:
	while _ws_server.is_connection_available():
		var conn := _ws_server.take_connection()
		if conn:
			_pending.append(conn)

func _pump_handshakes() -> void:
	var still: Array[StreamPeerTCP] = []
	for conn in _pending:
		var peer := WebSocketPeer.new()
		if peer.accept_stream(conn) == OK:
			_peers.append(peer)
		else:
			still.append(conn)
	_pending = still

func _pump_peers() -> void:
	var live: Array[WebSocketPeer] = []
	for peer in _peers:
		peer.poll()
		match peer.get_ready_state():
			WebSocketPeer.STATE_OPEN:
				while peer.get_available_packet_count() > 0:
					var text := peer.get_packet().get_string_from_utf8()
					# The shape ws-test-client.mjs asserts on.
					peer.send_text("echo: %s" % text)
				live.append(peer)
			WebSocketPeer.STATE_CONNECTING:
				live.append(peer)
			_:
				pass
	_peers = live
