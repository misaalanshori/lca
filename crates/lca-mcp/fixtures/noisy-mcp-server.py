#!/usr/bin/env python3
"""An MCP server that emits JSON-RPC notifications constantly (gh #291).

Same one-tool surface as `echo-mcp-server.py`, but every reply is
preceded by server-to-client notifications of all three kinds MCP
servers actually send (`notifications/message`, `notifications/progress`,
`notifications/tools/list_changed`). It exists to prove the stdio reader
discards notifications instead of mistaking one for a reply - the
defect that used to desync the stream permanently.
"""
import json
import sys

_notification_count = 0


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def notify(kind, params):
    send({"jsonrpc": "2.0", "method": kind, "params": params})


def chatter(request_id):
    """Three notifications of different kinds, before every reply."""
    global _notification_count
    _notification_count += 1
    notify("notifications/message", {
        "level": "info",
        "data": "handling request {0}".format(request_id),
    })
    notify("notifications/progress", {
        "progressToken": "token-{0}".format(_notification_count),
        "progress": _notification_count,
        "total": _notification_count + 1,
    })
    notify("notifications/tools/list_changed", {})


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = request.get("method")
        request_id = request.get("id")

        # The handshake's `notifications/initialized` is client -> server
        # traffic with no id: answer nothing, as the protocol says.
        if request_id is None:
            continue

        chatter(request_id)

        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}, "resources": {}},
                    "serverInfo": {"name": "noisy", "version": "0.1.0"},
                },
            })
        elif method == "tools/list":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "Echoes its text argument back.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"text": {"type": "string"}},
                        },
                        "annotations": {"readOnlyHint": True},
                    }],
                },
            })
        elif method == "resources/list":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {"resources": []},
            })
        elif method == "resources/templates/list":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "resourceTemplates": [
                        {"uriTemplate": "noisy://{name}", "name": "named"}
                    ],
                },
            })
        elif method == "tools/call":
            arguments = request.get("params", {}).get("arguments", {})
            text = arguments.get("text", "")
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "content": [{"type": "text", "text": "noisy echo: {0}".format(text)}],
                },
            })
        else:
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "unknown method: {0}".format(method)},
            })


if __name__ == "__main__":
    main()
