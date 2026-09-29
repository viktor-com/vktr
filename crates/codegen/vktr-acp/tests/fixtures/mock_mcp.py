#!/usr/bin/env python3
"""A minimal MCP server on stdio (newline-delimited JSON-RPC) for vktr acp's tests and smoke.

One tool, `lookup`, answers `MCP_LOOKUP:<code>`; `fail` returns an MCP-level error result.
"""
import json
import sys

TOOLS = [
    {
        "name": "lookup",
        "description": "Look up an internal code in the team database.",
        "inputSchema": {"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]},
    },
    {
        "name": "fail",
        "description": "Always fails.",
        "inputSchema": {"type": "object", "properties": {}},
    },
]

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if "id" not in msg:
        continue  # notifications
    method, rid = msg.get("method"), msg["id"]
    if method == "initialize":
        result = {
            "protocolVersion": msg["params"].get("protocolVersion", "2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "mock-mcp", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        name = msg["params"]["name"]
        args = msg["params"].get("arguments") or {}
        if name == "lookup":
            result = {"content": [{"type": "text", "text": "MCP_LOOKUP:" + str(args.get("code"))}], "isError": False}
        else:
            result = {"content": [{"type": "text", "text": "it broke"}], "isError": True}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": method}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}), flush=True)
