#!/usr/bin/env python3
"""A one-tool MCP server over newline-delimited stdio JSON-RPC (gh #53).

Phase-1 fixture only: `initialize`, `tools/list` (one `echo` tool), and
`tools/call`. Unknown methods answer a JSON-RPC error; anything
unparseable is ignored, the way a tolerant server treats line noise.
"""
import json
import sys


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


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
        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}, "resources": {}},
                    "serverInfo": {"name": "echo", "version": "0.1.0"},
                },
            })
        elif method == "tools/list":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Echoes its text argument back.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "text": {"type": "string"}
                                },
                            },
                            "annotations": {"readOnlyHint": True},
                        }
                    ]
                },
            })
        elif method == "resources/list":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "resources": [
                        {
                            "uri": "echo://greeting",
                            "name": "greeting",
                            "description": "A hello.",
                            "mimeType": "text/plain",
                        },
                        {
                            "uri": "echo://bytes",
                            "name": "bytes",
                            "mimeType": "application/octet-stream",
                        },
                    ]
                },
            })
        elif method == "resources/templates/list":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "resourceTemplates": [
                        {"uriTemplate": "echo://{name}", "name": "named"}
                    ]
                },
            })
        elif method == "resources/read":
            uri = request.get("params", {}).get("uri", "")
            if uri == "echo://greeting":
                content = [{"uri": uri, "mimeType": "text/plain", "text": "hello, resource"}]
            elif uri == "echo://bytes":
                content = [{"uri": uri, "mimeType": "application/octet-stream",
                             "blob": "YmluYXJ5LWJ5dGVz"}]
            elif uri.startswith("echo://"):
                content = [{"uri": uri, "mimeType": "text/plain",
                             "text": "hello, {0}".format(uri[len("echo://"):])}]
            else:
                send({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32002, "message": "unknown resource"},
                })
                continue
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {"contents": content},
            })
        elif method == "tools/call":
            params = request.get("params", {})
            arguments = params.get("arguments", {})
            text = arguments.get("text", "")
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "content": [{"type": "text", "text": "echo: {0}".format(text)}]
                },
            })
        elif request_id is not None:
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "unknown method: {0}".format(method)},
            })


if __name__ == "__main__":
    main()
