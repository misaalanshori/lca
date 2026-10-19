#!/usr/bin/env python3
"""A stdio MCP server that reports the environment it was spawned with
and the directory it landed in (gh #418 Phase C#C1).

One tool, `probe`, answers with the facts a spawn change is judged on:

- `cwd`: the absolute working directory the child actually got
- `env`: the value of each name given in `LCA_PROBE_ENV` (comma
  separated), plus whether `PATH` is present at all - the point of the
  overlay rule is that a `mcp.json` `env` block must not replace the
  inherited environment.

`LCA_PROBE_FAIL=1` makes the server write a diagnostic to stderr and
exit non-zero before answering `initialize`, which is how the stderr
tail is exercised: the host must be able to say *why* the server died,
not just that it did.

A line `exit` on stdin (or `LCA_PROBE_CRASH_AFTER_INIT=1`) exits 3
after writing to stderr, so the disconnect path can be judged too.
"""
import json
import os
import sys


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def probe_result():
    names = [name for name in os.environ.get("LCA_PROBE_ENV", "").split(",") if name]
    return {
        "cwd": os.getcwd(),
        "env": {name: os.environ.get(name, "<unset>") for name in names},
        "path_present": "PATH" in os.environ,
        "probe_env_present": "LCA_PROBE_ENV" in os.environ,
    }


def main():
    if os.environ.get("LCA_PROBE_FAIL") == "1":
        sys.stderr.write("probe: refusing to start, the config is wrong\n")
        sys.stderr.write("probe: hint: check LCA_PROBE_ENV\n")
        sys.stderr.flush()
        sys.exit(3)

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        if line == "exit":
            sys.stderr.write("probe: exiting on request\n")
            sys.stderr.flush()
            sys.exit(3)
        method = request.get("method")
        request_id = request.get("id")
        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "probe", "version": "0.1.0"},
                },
            })
            if os.environ.get("LCA_PROBE_CRASH_AFTER_INIT") == "1":
                sys.stderr.write("probe: crashing after initialize\n")
                sys.stderr.flush()
                sys.exit(3)
        elif method == "tools/list":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "tools": [
                        {
                            "name": "probe",
                            "description": "Reports the spawn environment and cwd.",
                            "inputSchema": {"type": "object", "properties": {}},
                        }
                    ]
                },
            })
        elif method == "tools/call":
            send({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "content": [
                        {"type": "text", "text": json.dumps(probe_result(), sort_keys=True)}
                    ]
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
