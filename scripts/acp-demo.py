#!/usr/bin/env python3
"""Show a `vktr acp` session the way an editor experiences it, in the terminal.

    VIKTOR_API_KEY=... scripts/acp-demo.py [path/to/vktr]

Launches `vktr acp` exactly as Zed would (stdio, fs and terminal capabilities), in a copy of a
small Python project with a failing test, and narrates the protocol: Viktor's streamed reply, the
editor tools it calls, the permission prompts (answered "allow once", with the diff shown), and
finally a restarted agent that reloads the session and still knows what it did. Used for
docs/recordings/live-acp-launch.cast; every prompt is a billed Viktor run.
"""
import difflib
import os
import shutil
import sys
import tempfile
import threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from acp_client import EDITOR_CAPABILITIES, AcpEditor  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "target/release/vktr")
KEY = os.environ.get("VIKTOR_API_KEY", "")

DIM, BOLD, CYAN, GREEN, YELLOW, RED, RESET = (
    "\033[2m", "\033[1m", "\033[36m", "\033[32m", "\033[33m", "\033[31m", "\033[0m",
)

CALC = '''def average(values):
    """Return the arithmetic mean of a non-empty list of numbers."""
    return sum(values) / len(values) + 1
'''
TEST = '''import unittest

from calc import average


class CalcTest(unittest.TestCase):
    def test_average(self):
        self.assertEqual(average([2, 4, 6]), 4)


if __name__ == "__main__":
    unittest.main()
'''


def say(text=""):
    print(text, flush=True)


def narrate(update):
    kind = update.get("sessionUpdate")
    if kind == "agent_message_chunk":
        print(update["content"].get("text", ""), end="", flush=True)
    elif kind == "agent_thought_chunk":
        say(f"\n{DIM}… {update['content'].get('text', '')}{RESET}")
    elif kind == "tool_call":
        say(f"\n{YELLOW}⚙ tool_call{RESET} {update.get('title')} {DIM}[{update.get('kind')}, {update.get('status') or 'pending'}]{RESET}")
    elif kind == "tool_call_update" and update.get("status") in ("completed", "failed"):
        colour = GREEN if update["status"] == "completed" else RED
        say(f"{colour}  ↳ {update['status']}{RESET}")


def permission(params):
    call = params["toolCall"]
    say(f"\n{BOLD}? editor asks the user:{RESET} {call.get('title')}")
    for content in call.get("content") or []:
        if content.get("type") == "diff":
            old = (content.get("oldText") or "").splitlines()
            new = content.get("newText", "").splitlines()
            for line in difflib.unified_diff(old, new, lineterm="", n=0):
                if line.startswith(("---", "+++", "@@")):
                    continue
                colour = GREEN if line.startswith("+") else RED
                say(f"    {colour}{line}{RESET}")
    options = ", ".join(o["name"] for o in params["options"])
    say(f"  {DIM}options: {options}{RESET}")
    say(f"  {GREEN}→ Allow{RESET}")
    return "allow_once"


def start_agent(workspace, env):
    say(f"{DIM}$ vktr acp        # started by the editor, JSON-RPC over stdio{RESET}")
    editor = AcpEditor(BIN, env, workspace, KEY)
    editor.on_update = narrate
    editor.on_permission = permission
    # The agent's two start-up lines go to stderr; show them as an editor's log panel would.
    lines = []

    def tee():
        for line in editor.agent.stderr:
            lines.append(line)
            if len(lines) <= 2:
                say(f"{DIM}  {line.rstrip()}{RESET}")

    threading.Thread(target=tee, daemon=True).start()
    init, _ = editor.request("initialize", {"protocolVersion": 1, "clientCapabilities": EDITOR_CAPABILITIES,
                                            "clientInfo": {"name": "acp-demo", "version": "1"}})
    info = init["result"]["agentInfo"]
    caps = init["result"]["agentCapabilities"]
    say(f"{CYAN}initialize{RESET} → {info['title']} ({info['name']} {info['version']}), "
        f"loadSession={caps['loadSession']}, images={caps['promptCapabilities']['image']}")
    return editor


def prompt(editor, session, text):
    say(f"\n{BOLD}{CYAN}you ›{RESET} {text}")
    print(f"{BOLD}viktor ›{RESET} ", end="", flush=True)
    answer, _ = editor.request("session/prompt", {"sessionId": session, "prompt": [{"type": "text", "text": text}]})
    stop = (answer.get("result") or {}).get("stopReason") or answer.get("error", {}).get("message")
    say(f"\n{DIM}[{stop}]{RESET}")


def main():
    if not KEY:
        sys.exit("VIKTOR_API_KEY is not set")
    home = tempfile.mkdtemp(prefix="vktr-acp-demo-home-")
    workspace = tempfile.mkdtemp(prefix="calc-")
    with open(os.path.join(workspace, "calc.py"), "w") as f:
        f.write(CALC)
    with open(os.path.join(workspace, "test_calc.py"), "w") as f:
        f.write(TEST)
    env = dict(os.environ, VKTR_HOME=home)
    try:
        editor = start_agent(workspace, env)
        session = editor.request("session/new", {"cwd": workspace, "mcpServers": []})[0]["result"]["sessionId"]
        say(f"{CYAN}session/new{RESET} → {session} {DIM}(workspace {workspace}){RESET}")
        prompt(editor, session, "Read calc.py and test_calc.py in my workspace and tell me in two sentences "
                                "why the test fails. Don't change anything yet.")
        prompt(editor, session, "Fix the bug in calc.py, then run python3 -m unittest -q in my workspace to confirm.")
        editor.close()

        say(f"\n{BOLD}── the editor restarts the agent ──{RESET}")
        editor = start_agent(workspace, env)
        editor.on_update = None
        _, replay = editor.request("session/load", {"sessionId": session, "cwd": workspace, "mcpServers": []})
        say(f"{CYAN}session/load{RESET} → replayed {len(replay)} updates of the earlier conversation")
        editor.on_update = narrate
        prompt(editor, session, "In one sentence: what did you change, and did the tests pass?")
        editor.close()
        say(f"\n{DIM}calc.py in the workspace now reads:{RESET}")
        with open(os.path.join(workspace, "calc.py")) as f:
            say(f.read().rstrip())
    finally:
        shutil.rmtree(home, ignore_errors=True)
        shutil.rmtree(workspace, ignore_errors=True)


if __name__ == "__main__":
    main()
