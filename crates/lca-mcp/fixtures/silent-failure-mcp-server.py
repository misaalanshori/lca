#!/usr/bin/env python3
"""An MCP server that fails the handshake and then stays alive, silently (gh #459).

The defect this fixture exists for: `native.rs` wraps every handshake failure with
`stderr_tail(...)` so a chatty crash explains itself, and the tail read *blocked* on
a live child whose stderr pipe was empty - no EOF, no data. So the path that exists
to surface an error hung before it could surface one, and the row stayed
`connecting` until the startup budget expired and then still said `connecting`.

This server answers `initialize` normally and then answers `tools/list` with a
JSON-RPC error, and **does not exit** and **writes nothing to stderr**. That is the
exact shape: a failure with no explanation on the pipe, from a process that is
still running.

Contrast with `chatty-crash-mcp-server.py`, which explains itself and dies.

`LCA_SILENT_LINGER` (seconds, default 300) is how long it stays alive after the
error, so a test cannot leak a process if something goes wrong.
"""
import json
import os
import sys
import time


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main():
    linger = float(os.environ.get("LCA_SILENT_LINGER", "300"))
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
        if request_id is None:
            continue

        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "silent-failure", "version": "0.1.0"},
                },
            })
        else:
            # Fail everything past the handshake, explaining nothing on stderr,
            # and stay alive. This is the defect's shape, not a mistake in the
            # fixture.
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "code": -32700,
                    "message": "the token in your config is not valid",
                },
            })
            # Keep the process alive with an empty stderr pipe. A blocking tail
            # read waits here; a non-blocking one returns immediately.
            time.sleep(linger)
            return


if __name__ == "__main__":
    main()
