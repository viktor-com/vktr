#!/usr/bin/env python3
"""Startup and idle cost of the welcome screen.

Usage: scripts/welcome-bench/stub_viktor.py &  then  scripts/welcome-bench/measure.py <vktr binary> <label>
Spawns the binary in a 130x36 pty against the local stub, times ten starts to the first welcome frame,
then reports CPU, terminal bytes and writes per second while the wordmark animates (1-11 s) and once idle (15-45 s).
"""
import os, pty, sys, time, fcntl, termios, struct, threading, signal, statistics, json
BIN, LABEL = sys.argv[1], sys.argv[2]
COLS, ROWS = 130, 36
ENV = dict(os.environ, VKTR_HOME=os.environ.get('BENCH_HOME', '/tmp/vktr-bench-home'), VIKTOR_API_KEY='vk_demo', VIKTOR_BASE_URL='http://127.0.0.1:8765/v1',
           COLORTERM='truecolor', TERM='xterm-256color')
os.makedirs(ENV['VKTR_HOME'], exist_ok=True)
open(os.path.join(ENV['VKTR_HOME'], 'config.toml'), 'w').write('theme = "vktr-night"\n')
TICK = os.sysconf('SC_CLK_TCK')
def spawn():
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(os.environ.get('BENCH_CWD', os.getcwd())); os.execve(BIN, [BIN], ENV)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', ROWS, COLS, 0, 0))
    os.kill(pid, signal.SIGWINCH)
    st = {'bytes': 0, 'chunks': 0, 'buf': b'', 'done': False}
    def rd():
        while True:
            try: d = os.read(fd, 65536)
            except OSError: break
            if not d: break
            st['bytes'] += len(d); st['chunks'] += 1
            if not st['done']:
                st['buf'] = (st['buf'] + d)[-200000:]
    threading.Thread(target=rd, daemon=True).start()
    return pid, fd, st
def cpu(pid):
    f = open(f'/proc/{pid}/stat').read().rsplit(')', 1)[1].split()
    return (int(f[11]) + int(f[12])) / TICK
def kill(pid):
    os.kill(pid, signal.SIGTERM)
    for _ in range(50):
        if os.waitpid(pid, os.WNOHANG)[0]: return
        time.sleep(0.05)
    os.kill(pid, signal.SIGKILL); os.waitpid(pid, 0)
res = {'label': LABEL}
starts = []
for _ in range(10):
    t0 = time.perf_counter(); pid, fd, st = spawn()
    while b'Quit' not in st['buf']:
        if time.perf_counter() - t0 > 20: raise SystemExit('no welcome')
        time.sleep(0.002)
    starts.append((time.perf_counter() - t0) * 1000); st['done'] = True
    kill(pid); os.close(fd)
res['startup_ms_median'] = round(statistics.median(starts), 1)
res['startup_ms_all'] = [round(s) for s in starts]
pid, fd, st = spawn(); t0 = time.perf_counter()
while b'Quit' not in st['buf']: time.sleep(0.002)
st['done'] = True
def window(a, b):
    while time.perf_counter() - t0 < a: time.sleep(0.01)
    c0, b0, k0, w0 = cpu(pid), st['bytes'], st['chunks'], time.perf_counter()
    while time.perf_counter() - t0 < b: time.sleep(0.01)
    dt = time.perf_counter() - w0
    return {'cpu_pct': round(100 * (cpu(pid) - c0) / dt, 2), 'bytes_per_s': round((st['bytes'] - b0) / dt), 'writes_per_s': round((st['chunks'] - k0) / dt, 1)}
res['animating_1_to_11s'] = window(1, 11)
res['idle_15_to_45s'] = window(15, 45)
kill(pid)
print(json.dumps(res))
