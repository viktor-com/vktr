#!/usr/bin/env python3
"""Live ACP check: drive `vktr acp` against the real Viktor compat API, as an editor would.

    VIKTOR_API_KEY=... scripts/live-acp.py [path/to/vktr] [--out DIR]

Each case needs real Viktor behaviour to pass: a random codeword that only exists in a local file,
arithmetic Viktor has to do in its own sandbox, recall of that codeword after the agent process
restarts. The key comes only from the environment and is scrubbed from everything written; the
run fails if the key shows up in the agent's stderr or in the saved transcript.

Every prompt is a billed Viktor run (about a dozen per invocation).
"""
import base64
import hashlib
import json
import os
import secrets
import shutil
import struct
import sys
import tempfile
import threading
import time
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from acp_client import EDITOR_CAPABILITIES, AcpEditor, text_of, tool_updates  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
args = [a for a in sys.argv[1:] if not a.startswith("--out")]
BIN = args[0] if args else os.path.join(ROOT, "target/release/vktr")
OUT = next((a.split("=", 1)[1] for a in sys.argv[1:] if a.startswith("--out=")), None)
KEY = os.environ.get("VIKTOR_API_KEY", "")
if not KEY:
    sys.exit("VIKTOR_API_KEY is not set")

home = tempfile.mkdtemp(prefix="vktr-live-acp-home-")
workspace = tempfile.mkdtemp(prefix="vktr-live-acp-ws-")
codeword = "pelican-" + secrets.token_hex(3)
os.makedirs(os.path.join(workspace, "notes"))
with open(os.path.join(workspace, "notes", "codeword.txt"), "w") as f:
    f.write(f"The codeword is {codeword}.\n")

env = dict(os.environ, VKTR_HOME=home)
env.setdefault("VIKTOR_BASE_URL", "https://api.viktor.com/api/compat/v1")

RESULTS = []
LOGS = []


def check(name, ok, detail="", seconds=None):
    took = f" ({seconds:.0f}s)" if seconds is not None else ""
    print(("PASS " if ok else "FAIL ") + name + took + ("" if ok else f"  {str(detail)[:400]}"), flush=True)
    RESULTS.append({"case": name, "ok": bool(ok), "seconds": seconds})


def prompt(editor, session, blocks):
    if isinstance(blocks, str):
        blocks = [{"type": "text", "text": blocks}]
    start = time.time()
    answer, updates = editor.request("session/prompt", {"sessionId": session, "prompt": blocks})
    return answer, updates, time.time() - start


def stop_reason(answer):
    return (answer.get("result") or {}).get("stopReason")


def thread_of(session):
    path = os.path.join(home, "acp", "sessions", f"{session}.json")
    with open(path) as f:
        return json.load(f).get("thread_id")


def red_png(size=64):
    raw = b"".join(b"\x00" + b"\xff\x00\x00" * size for _ in range(size))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    return base64.b64encode(png).decode()


stderr = ""
editors = []


def open_editor():
    editor = AcpEditor(BIN, env, workspace, KEY)
    editors.append(editor)
    return editor


try:
    editor = open_editor()
    init, _ = editor.request("initialize", {"protocolVersion": 1, "clientCapabilities": EDITOR_CAPABILITIES})
    check("initialize", init.get("result", {}).get("protocolVersion") == 1, init)
    start = time.time()
    auth, _ = editor.request("authenticate", {"methodId": "viktor-api-key"})
    check("authenticate-against-live-models-endpoint", "result" in auth, auth, time.time() - start)
    session = editor.request("session/new", {"cwd": workspace, "mcpServers": []})[0]["result"]["sessionId"]

    # 1. Read round trip: the codeword exists only in the local workspace.
    answer, updates, took = prompt(
        editor, session,
        "Use your editor_read_file tool to read notes/codeword.txt from my local workspace, "
        "then reply with only the codeword it contains.",
    )
    reads = [u for u in tool_updates(updates) if u.get("kind") == "read"]
    check("read-tool-round-trip", codeword in text_of(updates) and bool(reads), text_of(updates), took)
    check("read-shown-as-completed-tool-call",
          any(u.get("status") == "completed" for u in tool_updates(updates)), tool_updates(updates))
    first_thread = thread_of(session)

    # 2. Continuation: the same thread remembers without a tool.
    answer, updates, took = prompt(editor, session, "What was the codeword? Reply with only the codeword; do not use any tool.")
    check("continuation-recalls-context", codeword in text_of(updates), text_of(updates), took)
    check("continuation-keeps-one-viktor-thread", thread_of(session) == first_thread, (first_thread, thread_of(session)))

    # 3. Write with approval, showing the diff.
    editor.permission_answers = ["allow_once"]
    answer, updates, took = prompt(
        editor, session,
        "Use editor_write_file to create greeting.txt in my workspace with exactly this content and "
        "nothing else: hello from viktor",
    )
    path = os.path.join(workspace, "greeting.txt")
    written = open(path).read().strip() if os.path.exists(path) else None
    check("write-with-approval", written == "hello from viktor", written, took)
    asked = editor.permission_requests[-1] if editor.permission_requests else {}
    check("write-approval-shows-diff",
          any(c.get("type") == "diff" for c in asked.get("toolCall", {}).get("content") or []), asked)

    # 4. Command with approval, in an editor terminal.
    editor.permission_answers = ["allow_once"]
    answer, updates, took = prompt(
        editor, session,
        "Use editor_run_command to run: python3 -c 'print(6*7)'  and tell me the number it printed.",
    )
    check("command-with-approval", "42" in text_of(updates) and bool(editor.terminals), text_of(updates), took)

    # 5. Rejected command: nothing runs, the turn still ends normally.
    before = len(editor.terminals)
    editor.permission_answers = ["reject_once"]
    answer, updates, took = prompt(
        editor, session,
        "Use editor_run_command to run: rm greeting.txt  (if I reject it, just say you did not delete it).",
    )
    check("rejected-command-does-not-run",
          os.path.exists(path) and len(editor.terminals) == before and stop_reason(answer) == "end_turn",
          (stop_reason(answer), text_of(updates)), took)

    # 5b. The editor's MCP server: Viktor calls a tool that only exists on this machine.
    mcp_script = os.path.join(ROOT, "crates/codegen/vktr-acp/tests/fixtures/mock_mcp.py")
    mcp_session = editor.request("session/new", {"cwd": workspace, "mcpServers": [
        {"name": "mock", "command": sys.executable, "args": [mcp_script], "env": []}]})[0]["result"]["sessionId"]
    code = "K" + secrets.token_hex(2)
    editor.permission_answers = ["allow_once"]
    answer, updates, took = prompt(
        editor, mcp_session,
        f"Use the mcp__mock__lookup tool to look up the code {code}, then reply with exactly the text it returned.",
    )
    check("mcp-tool-round-trip", f"MCP_LOOKUP:{code}" in text_of(updates), text_of(updates), took)

    # 6. Viktor's own sandbox: a remote tool, no editor involvement.
    expected = hashlib.sha256(b"vktr").hexdigest()
    answer, updates, took = prompt(
        editor, session,
        "In your own sandbox (not my editor), compute the SHA-256 hex digest of the ASCII string vktr "
        "with Python, and reply with only the 64-character hex digest.",
    )
    check("remote-sandbox-tool", expected in text_of(updates), text_of(updates), took)
    check("remote-tool-is-not-an-editor-call", not tool_updates(updates), tool_updates(updates))

    # 7. Image input.
    answer, updates, took = prompt(editor, session, [
        {"type": "text", "text": "What single colour fills this image? Answer with one word."},
        {"type": "image", "mimeType": "image/png", "data": red_png()},
    ])
    check("image-input", "red" in text_of(updates).lower(), text_of(updates), took)

    # 8. Cancel mid-run, then keep going on the same thread.
    cancelled = threading.Event()

    def cancel_soon(_update):
        if not cancelled.is_set():
            cancelled.set()
            editor.notify("session/cancel", {"sessionId": session})

    timer = threading.Timer(4.0, lambda: cancel_soon(None))
    timer.start()
    editor.on_update = cancel_soon
    answer, updates, took = prompt(editor, session, "Write a 600-word story about a lighthouse keeper.")
    editor.on_update = None
    timer.cancel()
    check("cancel-stops-the-turn", stop_reason(answer) == "cancelled", answer, took)
    answer, updates, took = prompt(editor, session, "Reply with only the word: ready")
    check("session-usable-after-cancel", "ready" in text_of(updates).lower() and stop_reason(answer) == "end_turn",
          (stop_reason(answer), text_of(updates)), took)

    # 9. Restart: a new agent process loads the session and Viktor still knows the codeword.
    editor.close()
    restarted = open_editor()
    restarted.request("initialize", {"protocolVersion": 1, "clientCapabilities": EDITOR_CAPABILITIES})
    loaded, replay = restarted.request("session/load", {"sessionId": session, "cwd": workspace, "mcpServers": []})
    check("session-load-after-restart", "result" in loaded and codeword in text_of(replay), loaded)
    answer, updates, took = prompt(restarted, session, "One more time: what was the codeword? Reply with only it and use no tools.")
    check("restarted-agent-continues-the-thread", codeword in text_of(updates) and thread_of(session) == first_thread,
          (text_of(updates), first_thread, thread_of(session)), took)
finally:
    # Close every editor here, so a failed case still yields its stderr for the key check.
    for opened in editors:
        stderr += opened.close()
        LOGS.extend(opened.log)
    check("key-never-in-stderr", bool(editors) and KEY not in stderr)
    check("key-never-on-the-acp-wire", bool(editors) and not any(e.secret_seen for e in editors))
    if OUT:
        os.makedirs(OUT, exist_ok=True)
        stamp = time.strftime("%Y-%m-%d")
        with open(os.path.join(OUT, f"live-acp-{stamp}.jsonl"), "w") as f:
            for line in LOGS:
                f.write(line.replace(KEY, "<VIKTOR_API_KEY>").replace(workspace, "<workspace>").replace(home, "<vktr_home>") + "\n")
        with open(os.path.join(OUT, f"live-acp-{stamp}.txt"), "w") as f:
            f.write(f"vktr acp live check against {env['VIKTOR_BASE_URL']} on {time.strftime('%Y-%m-%d %H:%M %Z')}\n\n")
            for r in RESULTS:
                took = f" ({r['seconds']:.0f}s)" if r["seconds"] is not None else ""
                f.write(("PASS " if r["ok"] else "FAIL ") + r["case"] + took + "\n")
    shutil.rmtree(home, ignore_errors=True)
    shutil.rmtree(workspace, ignore_errors=True)

failed = [r["case"] for r in RESULTS if not r["ok"]]
print("---")
print(f"{len(RESULTS)} checks, {len(failed)} failed")
sys.exit(1 if failed else 0)
