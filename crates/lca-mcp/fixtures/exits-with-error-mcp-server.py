#!/usr/bin/env python3
"""A stdio MCP server that exits after answering with an error (gh #459 matrix).

The `exits` half of the issue's two-mock comparison: answers `initialize`, then
answers `tools/list` with a JSON-RPC error, then dies. This case always worked - the
pipe closes, so the tail read saw EOF - and the row that stayed `connecting` was the
one that *did not* exit.
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
        if request_id is None:
            continue

        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "exits-with-error", "version": "0.1.0"},
                },
            })
        else:
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32700, "message": "Parse error: could not read request id"},
            })
            # Explain on stderr, then exit: the shape that always worked.
            sys.stderr.write("exits-with-error: the request was malformed\n")
            sys.stderr.flush()
            return


if __name__ == "__main__":
    main()
