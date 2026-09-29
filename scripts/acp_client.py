"""A minimal ACP editor for driving `vktr acp` from scripts, the way Zed or a JetBrains IDE would.

It speaks JSON-RPC over the agent's stdio and serves the editor half of the protocol for real:
`fs/read_text_file` and `fs/write_text_file` against a workspace directory, `terminal/*` with
real subprocesses, and `session/request_permission` from a policy the caller sets. Every message
is kept in `self.log` so a run can be saved as evidence.

Used by scripts/smoke-acp.py (mock Viktor) and scripts/live-acp.py (the real Viktor API).
"""
import json
import os
import subprocess
import threading


class Terminal:
    """One `terminal/create`: a real process whose combined output is captured."""

    def __init__(self, command, args, cwd, env, limit):
        self.limit = limit or 1 << 20
        self.output = b""
        self.truncated = False
        self.proc = subprocess.Popen(
            [command, *args],
            cwd=cwd or None,
            env=dict(os.environ, **{e["name"]: e["value"] for e in env or []}),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
        )
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        for chunk in iter(lambda: self.proc.stdout.read1(4096), b""):
            self.output += chunk
            if len(self.output) > self.limit:
                # ACP keeps the end of the output when it truncates.
                self.output = self.output[-self.limit:]
                self.truncated = True

    def exit_status(self):
        code = self.proc.poll()
        if code is None:
            return None
        return {"exitCode": code, "signal": None} if code >= 0 else {"exitCode": None, "signal": str(-code)}

    def wait(self):
        self.proc.wait()
        self.reader.join(timeout=5)
        return self.exit_status()


class AcpEditor:
    def __init__(self, binary, env, workspace, secret=None):
        self.workspace = workspace
        self.secret = secret
        self.secret_seen = False  # set if the secret ever crosses the wire, before any scrubbing
        self.log = []
        self.permission_answers = []  # option ids (or "cancelled") for the next permission requests
        self.permission_requests = []
        self.terminals = {}
        self.on_update = None  # called with each session/update; may send notifications
        self.on_permission = None  # called with each permission request; returns the option id
        self.next_id = 0
        self.agent = subprocess.Popen(
            [binary, "acp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            text=True,
            bufsize=1,
        )

    # -- wire -------------------------------------------------------------------------------

    def _scrub(self, text):
        if self.secret and self.secret in text:
            self.secret_seen = True
            return text.replace(self.secret, "<VIKTOR_API_KEY>")
        return text

    def _send(self, message):
        line = json.dumps(message)
        self.log.append(">> " + self._scrub(line))
        self.agent.stdin.write(line + "\n")
        self.agent.stdin.flush()

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method, params):
        """Send one request and serve the agent until it answers. Returns (message, updates)."""
        self.next_id += 1
        rid = self.next_id
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        updates = []
        while True:
            line = self.agent.stdout.readline()
            if not line:
                raise RuntimeError(f"vktr acp closed the connection during {method}")
            self.log.append("<< " + self._scrub(line.rstrip("\n")))
            message = json.loads(line)
            if message.get("method") == "session/update":
                updates.append(message["params"]["update"])
                if self.on_update:
                    self.on_update(message["params"]["update"])
            elif "method" in message and "id" in message:
                self._serve(message)
            elif message.get("id") == rid:
                return message, updates

    def close(self):
        """Stop the agent and return its stderr (raw: callers check it for the key before saving
        anything). Safe to call twice; the second call returns the same text."""
        if getattr(self, "_stderr", None) is not None:
            return self._stderr
        if self.agent.stdin and not self.agent.stdin.closed:
            self.agent.stdin.close()
        try:
            self.agent.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.agent.kill()
            self.agent.wait()
        for terminal in self.terminals.values():
            if terminal.proc.poll() is None:
                terminal.proc.kill()
        self._stderr = self.agent.stderr.read()
        return self._stderr

    # -- the editor half of the protocol -----------------------------------------------------

    def _serve(self, message):
        method, params, rid = message["method"], message.get("params") or {}, message["id"]
        try:
            result = self._handle(method, params)
            self._send({"jsonrpc": "2.0", "id": rid, "result": result})
        except Exception as error:  # an editor reports failures as JSON-RPC errors
            self._send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32603, "message": str(error)}})

    def _handle(self, method, params):
        if method == "fs/read_text_file":
            with open(params["path"], encoding="utf-8") as f:
                lines = f.read().splitlines(keepends=True)
            start = max((params.get("line") or 1) - 1, 0)
            limit = params.get("limit")
            return {"content": "".join(lines[start:start + limit] if limit else lines[start:])}
        if method == "fs/write_text_file":
            os.makedirs(os.path.dirname(params["path"]) or ".", exist_ok=True)
            with open(params["path"], "w", encoding="utf-8") as f:
                f.write(params["content"])
            return {}
        if method == "session/request_permission":
            self.permission_requests.append(params)
            if self.on_permission:
                answer = self.on_permission(params)
            else:
                answer = self.permission_answers.pop(0) if self.permission_answers else "reject_once"
            if answer == "cancelled":
                return {"outcome": {"outcome": "cancelled"}}
            return {"outcome": {"outcome": "selected", "optionId": answer}}
        if method == "terminal/create":
            terminal_id = f"term-{len(self.terminals) + 1}"
            self.terminals[terminal_id] = Terminal(
                params["command"], params.get("args", []), params.get("cwd"), params.get("env"),
                params.get("outputByteLimit"),
            )
            return {"terminalId": terminal_id}
        terminal = self.terminals[params["terminalId"]]
        if method == "terminal/wait_for_exit":
            return terminal.wait()
        if method == "terminal/output":
            status = terminal.exit_status()
            result = {"output": terminal.output.decode("utf-8", "replace"), "truncated": terminal.truncated}
            if status is not None:
                result["exitStatus"] = status
            return result
        if method == "terminal/kill":
            terminal.proc.kill()
            return {}
        if method == "terminal/release":
            if terminal.proc.poll() is None:
                terminal.proc.kill()
            return {}
        raise ValueError(f"method not supported by this editor: {method}")


def text_of(updates, kind="agent_message_chunk"):
    return "".join(
        u["content"].get("text", "") for u in updates if u.get("sessionUpdate") == kind
    )


def tool_updates(updates):
    return [u for u in updates if u.get("sessionUpdate") in ("tool_call", "tool_call_update")]


EDITOR_CAPABILITIES = {"fs": {"readTextFile": True, "writeTextFile": True}, "terminal": True}
