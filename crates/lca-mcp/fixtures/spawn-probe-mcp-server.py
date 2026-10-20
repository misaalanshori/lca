#!/usr/bin/env python3
"""An MCP server that records its own spawn (gh #418 Phase E, gh #266).

Every launch appends one line to `$LCA_SPAWN_MARKER` before doing anything
else, so a test can prove *zero spawns* by the marker's absence. The point of
the row is that a disabled or `--no-mcp` run must never start this process at
all: a marker file is a direct observation of "no child process existed",
where checking the tool list only proves the tools were filtered afterwards.

`LCA_PROBE_DELAY` sleeps that many seconds before answering `initialize`,
which is how the non-blocking-startup rows get a slow server.

One tool, `probe`, so the server also serves as a normal reachable server once
it lands.
"""
import json
import os
import sys
import time


def marker(kind):
    path = os.environ.get("LCA_SPAWN_MARKER")
    if not path:
        return
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(f"{kind} pid={os.getpid()} cwd={os.getcwd()}\n")


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main():
    marker("spawn")
    delay = float(os.environ.get("LCA_PROBE_DELAY", "0") or 0)
    if delay > 0:
        time.sleep(delay)
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
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "spawnprobe", "version": "0.1.0"},
                },
            })
        elif method == "tools/list":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "tools": [
                        {
                            "name": "probe",
                            "description": "Answers 'pong'.",
                            "inputSchema": {"type": "object", "properties": {}},
                        }
                    ]
                },
            })
        elif method == "tools/call":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {"content": [{"type": "text", "text": "pong"}]},
            })
        elif request_id is not None:
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": f"unknown method: {method}"},
            })


if __name__ == "__main__":
    main()
