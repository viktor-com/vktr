#!/usr/bin/env python3
"""Record the key vktr flows against the real Viktor API with asciinema.

    VIKTOR_API_KEY=... scripts/record-live.py <login|coding|approval|acp|all> [path/to/vktr]

Each flow runs in a throwaway VKTR_HOME and project, inside a private tmux server
(`tmux -L vktr-rec -f /dev/null`, so no user tmux config or session restore is involved), recorded
by asciinema to docs/recordings/live-<flow>.cast. The key reaches the recorded shell only through
the environment, never through a command line, and every cast is checked for it afterwards.
Every prompt is a billed Viktor run.
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KEY = os.environ.get("VIKTOR_API_KEY", "")
BIN = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else os.path.join(ROOT, "target/release/vktr")
TMUX = ["tmux", "-L", "vktr-rec", "-f", "/dev/null"]
WIDTH, HEIGHT = 110, 32

CALC = '''def average(values):
    """Return the arithmetic mean of a non-empty list of numbers."""
    return sum(values) / len(values) + 1


def clamp(value, low, high):
    """Limit value to the closed range [low, high]."""
    return max(low, min(value, high))
'''
TEST = '''import unittest

from calc import average, clamp


class CalcTest(unittest.TestCase):
    def test_average(self):
        self.assertEqual(average([2, 4, 6]), 4)

    def test_clamp(self):
        self.assertEqual(clamp(15, 0, 10), 10)
        self.assertEqual(clamp(-3, 0, 10), 0)


if __name__ == "__main__":
    unittest.main()
'''


class Recorder:
    def __init__(self, name, cwd, home, with_key=True):
        self.cast = os.path.join(ROOT, "docs/recordings", f"live-{name}.cast")
        env = dict(
            os.environ,
            VKTR_HOME=home,
            PATH=os.path.dirname(BIN) + os.pathsep + os.environ["PATH"],
            TERM="xterm-256color",
            HISTFILE="/dev/null",
        )
        if not with_key:
            env.pop("VIKTOR_API_KEY", None)
        # Recording from an SSH login would otherwise show the TUI's SSH tip in every take.
        for var in ("SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"):
            env.pop(var, None)
        rcfile = os.path.join(home, "rec.bashrc")
        with open(rcfile, "w") as f:
            f.write("PS1='\\[\\e[1;32m\\]$\\[\\e[0m\\] '\n")
        # The private server inherits this environment, so the recorded shell gets the key
        # without it ever appearing in an argv. extended-keys and allow-passthrough are set before
        # the session exists so the TUI's tmux probe passes and the splash shows no clipboard warning.
        subprocess.run(
            TMUX + ["start-server", ";", "set-option", "-g", "extended-keys", "on",
                    ";", "set-option", "-wg", "allow-passthrough", "on", ";", "new-session", "-d", "-s", "rec", "-x", str(WIDTH), "-y", str(HEIGHT), "-c", cwd,
                    f"asciinema rec -q --overwrite --cols {WIDTH} --rows {HEIGHT} "
                    f"-c 'bash --noprofile --rcfile {rcfile} -i' '{self.cast}'"],
            env=env, check=True,
        )
        self.wait_prompt(20)

    def screen(self):
        return subprocess.run(TMUX + ["capture-pane", "-t", "rec", "-p"], capture_output=True, text=True).stdout

    def wait(self, pattern, timeout=300):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if re.search(pattern, self.screen()):
                return True
            time.sleep(0.5)
        print(f"  timed out waiting for {pattern!r}", flush=True)
        return False

    def wait_prompt(self, timeout=300):
        """Wait for the shell prompt to be the last thing on screen."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            lines = [l for l in self.screen().splitlines() if l.strip()]
            if lines and lines[-1].rstrip().endswith("$"):
                return True
            time.sleep(0.5)
        print("  timed out waiting for the shell prompt", flush=True)
        return False

    def keys(self, *keys):
        subprocess.run(TMUX + ["send-keys", "-t", "rec", *keys], check=True)

    def type(self, text, enter=True, pause=0.8):
        for i in range(0, len(text), 3):
            subprocess.run(TMUX + ["send-keys", "-t", "rec", "-l", "--", text[i:i + 3]], check=True)
            time.sleep(0.04)
        time.sleep(pause)
        if enter:
            self.keys("Enter")

    def paste_secret(self):
        """Paste the key into the pane through a tmux buffer loaded from an owner-only file, so it
        never appears in any process's arguments; the file and the buffer are removed at once."""
        fd, path = tempfile.mkstemp(prefix="vktr-rec-key-")
        try:
            os.write(fd, KEY.encode())
            os.close(fd)
            subprocess.run(TMUX + ["load-buffer", "-b", "k", path], check=True)
        finally:
            os.remove(path)
        subprocess.run(TMUX + ["paste-buffer", "-d", "-b", "k", "-t", "rec"], check=True)

    def quit_tui(self):
        self.keys("C-q")
        time.sleep(0.25)
        self.keys("C-q")
        self.wait_prompt(30)

    def finish(self):
        time.sleep(1.5)
        self.type("exit", pause=0.3)
        for _ in range(40):
            if subprocess.run(TMUX + ["has-session", "-t", "rec"], capture_output=True).returncode:
                break
            time.sleep(0.25)
        stop_recording()
        text = open(self.cast, encoding="utf-8", errors="replace").read()
        leaked = KEY and (KEY in text or KEY[:24] in text)
        print(f"  {os.path.relpath(self.cast, ROOT)}: {len(text)} bytes, key {'LEAKED' if leaked else 'absent'}")
        if leaked:
            os.remove(self.cast)
            sys.exit("the key appeared in the recording; it was deleted")


def project():
    path = tempfile.mkdtemp(prefix="calc-")
    for name, body in (("calc.py", CALC), ("test_calc.py", TEST)):
        with open(os.path.join(path, name), "w") as f:
            f.write(body)
    subprocess.run(["git", "init", "-q", "."], cwd=path, check=True)
    subprocess.run(["git", "add", "-A"], cwd=path, check=True)
    subprocess.run(["git", "-c", "user.name=demo", "-c", "user.email=demo@example.com", "commit", "-qm", "calc"],
                   cwd=path, check=True)
    return path


# Commands say "Yes, proceed" / "Always allow"; edits say "Yes" / "Yes, allow all edits during this session".
PROCEED = r"Yes(?:, proceed)?\s*$"
ALLOW_ALWAYS = r"(?:Always allow|Yes, allow all edits)"
REJECT = r"No, reject"
READY = r"❯"  # the composer, shown once the TUI is signed in and idle


def option_key(screen, label):
    """The number key of the approval option whose text matches `label`; the menu's options and
    their order vary with the call, so options are picked by label, never by position."""
    m = re.search(r"(\d+) \([●○]\) " + label, screen, re.M)
    return m.group(1) if m else None


def run_tui_turn(rec, answer=PROCEED, timeout=420):
    """Answer every approval prompt with the option labelled `answer` until the turn ends."""
    deadline = time.time() + timeout
    # The previous turn's "Worked for" may still be on screen: first see this turn start.
    rec.wait(r"\[stop\]", 60)
    while time.time() < deadline:
        screen = rec.screen()
        if "[stop]" not in screen and not re.search(r"\(●\)|\(○\)", screen):
            return True
        if re.search(r"\(●\)|\(○\)", screen):
            time.sleep(1.5)  # let the viewer read the prompt
            key = option_key(screen, answer)
            if key is None:
                sys.exit(f"no approval option matching {answer!r} on screen:\n{screen}")
            rec.keys(key)
            # Wait for the prompt to close, so a slow redraw cannot take a second key press.
            for _ in range(20):
                time.sleep(0.25)
                if not re.search(r"\(●\)|\(○\)", rec.screen()):
                    break
        time.sleep(0.5)
    print("  turn did not finish in time", flush=True)
    return False


def flow_login(home):
    cwd = tempfile.mkdtemp(prefix="vktr-login-")
    rec = Recorder("login", cwd, home)
    rec.type("vktr --version")
    rec.wait_prompt()
    rec.type('vktr login --api-key "$VIKTOR_API_KEY"')
    rec.wait("Saved to", 60)
    rec.wait_prompt()
    rec.type("unset VIKTOR_API_KEY   # from here on vktr uses the saved key")
    rec.wait_prompt()
    rec.type('vktr -p "In one sentence: what can you help a developer with?"')
    rec.wait_prompt(300)
    rec.finish()
    shutil.rmtree(cwd, ignore_errors=True)


def flow_coding(home):
    cwd = project()
    rec = Recorder("coding", cwd, home)
    rec.type("python3 -m unittest -q 2>&1 | tail -3")
    rec.wait_prompt()
    rec.type("vktr")
    rec.wait(READY, 60)
    time.sleep(1.5)
    rec.type("The unit tests in this project fail. Run python3 -m unittest -q to see why, fix the bug in "
             "calc.py, then run the tests again.")
    run_tui_turn(rec)
    time.sleep(4)
    rec.quit_tui()
    rec.type("git diff && python3 -m unittest -q 2>&1 | tail -3")
    rec.wait_prompt()
    rec.finish()
    shutil.rmtree(cwd, ignore_errors=True)


def flow_approval(home):
    cwd = project()
    rec = Recorder("approval", cwd, home)
    rec.type("vktr")
    rec.wait(READY, 60)
    time.sleep(1.5)
    rec.type("Create README.md describing calc.py in three short bullet points.")
    # Reject the first write: the turn stops and nothing is written.
    run_tui_turn(rec, answer=REJECT)
    time.sleep(2)
    rec.type("Call it CALC.md instead of README.md.")
    # Allow edits for the rest of the session.
    run_tui_turn(rec, answer=ALLOW_ALWAYS)
    time.sleep(4)
    rec.quit_tui()
    rec.type("ls *.md; cat CALC.md")
    rec.wait_prompt()
    rec.finish()
    shutil.rmtree(cwd, ignore_errors=True)


def flow_signin(home):
    cwd = project()
    rec = Recorder("signin", cwd, home, with_key=False)
    rec.type("vktr doctor | tail -8   # no key yet")
    rec.wait_prompt(60)
    rec.type("vktr")
    rec.wait("Paste a Viktor API key", 60)
    time.sleep(2)
    rec.paste_secret()
    time.sleep(1.5)
    rec.keys("Enter")
    rec.wait("Type a message|❯", 60)
    time.sleep(2)
    rec.type("In one short sentence: what does calc.py in this directory do?")
    run_tui_turn(rec)
    time.sleep(3)
    rec.quit_tui()
    rec.type("vktr doctor | tail -8   # the key is saved, owner-only")
    rec.wait_prompt(60)
    rec.finish()
    shutil.rmtree(cwd, ignore_errors=True)


def flow_acp(home):
    rec = Recorder("acp-launch", ROOT, home)
    rec.type("scripts/acp-demo.py")
    rec.wait_prompt(900)
    rec.finish()


def stop_recording():
    """Kill the private tmux server and any asciinema it leaves behind (killing the server alone
    orphans asciinema and its shell)."""
    subprocess.run(TMUX + ["kill-server"], capture_output=True)
    subprocess.run(["pkill", "-9", "-f", "asciinema rec -q --overwrite --cols .* docs/recordings/live-"],
                   capture_output=True)


FLOWS = {"signin": flow_signin, "login": flow_login, "coding": flow_coding, "approval": flow_approval, "acp": flow_acp}


def main():
    if not KEY:
        sys.exit("VIKTOR_API_KEY is not set")
    if subprocess.run(TMUX + ["has-session"], capture_output=True).returncode == 0:
        sys.exit("a vktr-rec tmux server is already running")
    names = list(FLOWS) if sys.argv[1:2] == ["all"] else sys.argv[1:2]
    for name in names:
        home = tempfile.mkdtemp(prefix="vktr-rec-home-")
        print(f"recording {name}", flush=True)
        try:
            FLOWS[name](home)
        finally:
            stop_recording()
            shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
