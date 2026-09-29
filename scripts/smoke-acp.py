#!/usr/bin/env python3
"""ACP smoke test: drive the real `vktr acp` binary over stdio, as an editor does.

    scripts/smoke-acp.py [path/to/vktr]

The in-process tests in crates/codegen/vktr-acp cover the protocol case by case. This one covers
the other half: that the shipped binary wires the subcommand up correctly, keeps stdout clean for
the protocol, maps one editor session onto one Viktor thread on the wire, runs Viktor's editor
tool calls through the editor (fs and terminal, with a permission prompt for commands), and
brings a session back on the same thread after the agent restarts. The server is
scripts/mock_viktor.py, so no API key and no network are needed.
"""
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from acp_client import EDITOR_CAPABILITIES, AcpEditor, text_of, tool_updates  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "target/release/vktr")
KEY = "zt_test_sk_mock"

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
port = sock.getsockname()[1]
sock.close()

mock = subprocess.Popen(
    [sys.executable, os.path.join(ROOT, "scripts/mock_viktor.py"), str(port)],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.PIPE,
)
# Wait for the mock to listen, and stop if it never does: a slow runner otherwise fails the first
# prompts with "Could not reach Viktor" and hides the cause (macOS CI, 2026-10-01).
deadline = time.monotonic() + float(os.environ.get("VKTR_MOCK_WAIT", "60"))
while True:
    try:
        socket.create_connection(("127.0.0.1", port), 0.2).close()
        break
    except OSError:
        if mock.poll() is not None or time.monotonic() > deadline:
            mock.kill()
            sys.exit(f"mock Viktor on 127.0.0.1:{port} did not start: {mock.stderr.read().decode()[-2000:]}")
        time.sleep(0.1)

home = tempfile.mkdtemp(prefix="vktr-acp-smoke-home-")
workspace = tempfile.mkdtemp(prefix="vktr-acp-smoke-ws-")
with open(os.path.join(workspace, "hello.txt"), "w") as f:
    f.write("HELLO_FROM_THE_WORKSPACE\n")

env = dict(
    os.environ,
    VIKTOR_API_KEY=KEY,
    VIKTOR_BASE_URL=f"http://127.0.0.1:{port}/v1",
    VKTR_HOME=home,
)

FAILS = []


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f"  {detail}"))
    if not ok:
        FAILS.append(name)


def prompt(editor, session, text):
    answer, updates = editor.request("session/prompt", {"sessionId": session, "prompt": [{"type": "text", "text": text}]})
    if "result" not in answer:
        # Say why before a check trips over the missing result. The key is scrubbed here and
        # checked for separately (key-never-printed).
        print(f"prompt {text!r} failed: {editor._scrub(repr(answer.get('error', answer)))}")
    return answer, updates


def stop_reason(answer):
    return (answer.get("result") or {}).get("stopReason")


diagnostics = ""
try:
    # A chat-only client: the TypeScript agent's wire, no tools.
    chat = AcpEditor(BIN, env, workspace, KEY)
    init, _ = chat.request("initialize", {"protocolVersion": 1, "clientCapabilities": {}})
    result = init["result"]
    check("initialize-v1", result["protocolVersion"] == 1, result)
    check("auth-method-advertised", result["authMethods"][0]["id"] == "viktor-api-key")
    check("image-capability", result["agentCapabilities"]["promptCapabilities"]["image"] is True)
    check("load-session-advertised", result["agentCapabilities"]["loadSession"] is True, result)
    release = subprocess.run([BIN, "--version"], capture_output=True, text=True).stdout.split()[1]
    check("agent-info-reports-the-vktr-release", result["agentInfo"]["version"] == release, (result["agentInfo"], release))

    first = chat.request("session/new", {"cwd": workspace, "mcpServers": []})[0]["result"]["sessionId"]
    check("session-new", bool(first))
    answer, updates = prompt(chat, first, "hi")
    check("streamed-reply", "MOCK_OK" in text_of(updates), repr(text_of(updates)))
    check("end-turn", stop_reason(answer) == "end_turn", answer)
    prompt(chat, first, "again")
    second = chat.request("session/new", {"cwd": workspace, "mcpServers": []})[0]["result"]["sessionId"]
    prompt(chat, second, "fresh")
    diagnostics += chat.close()

    # An editor with fs and terminal: Viktor's tool calls run through it.
    editor = AcpEditor(BIN, env, workspace, KEY)
    editor.request("initialize", {"protocolVersion": 1, "clientCapabilities": EDITOR_CAPABILITIES})
    session = editor.request("session/new", {"cwd": workspace, "mcpServers": []})[0]["result"]["sessionId"]

    answer, updates = prompt(editor, session, "[mock:acp_read] read hello.txt")
    check("read-round-trip", "HELLO_FROM_THE_WORKSPACE" in text_of(updates), repr(text_of(updates)))
    check("read-reported-as-tool-call", any(u.get("kind") == "read" for u in tool_updates(updates)), tool_updates(updates))
    check("read-asks-no-permission", not editor.permission_requests, editor.permission_requests)

    editor.permission_answers = ["allow_once"]
    answer, updates = prompt(editor, session, "[mock:acp_run] run the marker command")
    check("command-asks-permission", len(editor.permission_requests) == 1, editor.permission_requests)
    check("command-round-trip", "ACP_TOOL_RAN" in text_of(updates), repr(text_of(updates)))

    # An MCP server the editor lists: its tool is offered to Viktor and runs here.
    mcp_script = os.path.join(ROOT, "crates/codegen/vktr-acp/tests/fixtures/mock_mcp.py")
    mcp_session = editor.request("session/new", {"cwd": workspace, "mcpServers": [
        {"name": "mock", "command": sys.executable, "args": [mcp_script], "env": []}]})[0]["result"]["sessionId"]
    editor.permission_answers = ["allow_once"]
    answer, updates = prompt(editor, mcp_session, "[mock:acp_mcp] look up Z9")
    check("mcp-tool-round-trip", "MCP_LOOKUP:Z9" in text_of(updates), repr(text_of(updates)))

    editor.permission_answers = ["reject_once"]
    answer, updates = prompt(editor, session, "[mock:acp_write] write a file")
    check("rejected-write-not-written", not os.path.exists(os.path.join(workspace, "made-by-viktor.txt")))
    check("rejection-reaches-viktor", "rejected" in text_of(updates), repr(text_of(updates)))
    check("turn-ends-after-rejection", stop_reason(answer) == "end_turn", answer)
    diagnostics += editor.close()

    # A restarted agent: the session comes back on the same thread.
    restarted = AcpEditor(BIN, env, workspace, KEY)
    restarted.request("initialize", {"protocolVersion": 1, "clientCapabilities": EDITOR_CAPABILITIES})
    listed = restarted.request("session/list", {"cwd": workspace})[0]["result"]["sessions"]
    check("session-list", session in [s["sessionId"] for s in listed], listed)
    loaded, replay = restarted.request("session/load", {"sessionId": session, "cwd": workspace, "mcpServers": []})
    check("session-load", "result" in loaded, loaded)
    check("load-replays-transcript", "read hello.txt" in text_of(replay, "user_message_chunk"), replay[:3])
    answer, updates = prompt(restarted, session, "[mock:ok] still there?")
    check("loaded-session-answers", "MOCK_OK" in text_of(updates), repr(text_of(updates)))
    diagnostics += restarted.close()
finally:
    mock.terminate()
    served = mock.stderr.read().decode()
    shutil.rmtree(home, ignore_errors=True)
    shutil.rmtree(workspace, ignore_errors=True)

threads = [line for line in served.splitlines() if "responses thread=" in line]
check("second-prompt-continues-the-thread", len(threads) > 1 and "continued=True" in threads[1], threads)
check("second-session-starts-a-new-thread", len(threads) > 2 and "continued=False" in threads[2], threads)
tool_thread = threads[3].split("thread=")[1].split()[0] if len(threads) > 3 else None
check(
    "restarted-agent-continues-the-same-thread",
    bool(tool_thread) and f"thread={tool_thread} continued=True" in threads[-1],
    threads[-2:],
)
check("key-never-printed", KEY not in diagnostics, "the API key reached stderr")

if FAILS:
    # What the mock served and what the agent logged, so a failure on a CI runner explains itself.
    print("--- mock Viktor log (last 30 lines)")
    print("\n".join(served.splitlines()[-30:]))
    print("--- vktr acp stderr (last 30 lines)")
    print("\n".join(diagnostics.replace(KEY, "<VIKTOR_API_KEY>").splitlines()[-30:]))

print("---")
print(f"{len(threads)} Viktor requests, {len(FAILS)} failed")
sys.exit(1 if FAILS else 0)
