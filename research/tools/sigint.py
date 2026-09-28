#!/usr/bin/env python3
"""Does Ctrl-C end a long `vagcan` command at once?

FOR: a command that spends minutes on the main task can lose a SIGINT: `main.rs` races the command
against `ctrl_c()` in a `select!`, and a branch that blocks never gives it back. Found on
2026-09-28 that `dev dash build` and `setup` ignored Ctrl-C in about half the runs while they read
a VCDS registry (fixed by running the read off the main task). Written by review subagents; kept
per the cleanup skill's 2026-09-22 and 2026-09-27 rules.

IN: the binary, a HOME to run it in (a scratch copy — the command may write there), the command's
arguments after `--`, and when to send the signal: `--after SECONDS`, or `--marker TEXT` (a line
the command prints) plus `--delay SECONDS` after it. `--prepare CMD` runs a shell command before
each trial to make the command long again — e.g. remove a key from the HOME's rod-keys.json so a
key search runs. `--trials N`, `--timeout SECONDS`, `--logs DIR`.
    python3 research/tools/sigint.py --binary target/debug/vagcan --home /tmp/h --after 5 \
        --prepare 'cp /tmp/keys-minus-one.json /tmp/h/.vagcan/data/P/rod-keys.json' \
        -- dev dash build <VIN>
OUT: one line per trial: how long after the SIGINT the process ended and its return code — 130
is the handler's exit, -2 the default disposition's kill, and "still running" after --timeout
seconds is a lost signal. Each trial's output is kept in --logs DIR.
"""
import argparse
import os
import signal
import subprocess
import time

ap = argparse.ArgumentParser()
ap.add_argument("--binary", required=True)
ap.add_argument("--home", required=True)
ap.add_argument("--after", type=float)
ap.add_argument("--marker")
ap.add_argument("--delay", type=float, default=0.0)
ap.add_argument("--prepare")
ap.add_argument("--trials", type=int, default=4)
ap.add_argument("--timeout", type=float, default=4.0)
ap.add_argument("--logs", default=".")
ap.add_argument("args", nargs=argparse.REMAINDER)
a = ap.parse_args()
cmd = [a.binary] + [x for x in a.args if x != "--"]
env = dict(os.environ, HOME=a.home)
os.makedirs(a.logs, exist_ok=True)


def default_sigint():
    # The child gets the default disposition, as a terminal's Ctrl-C would find it.
    signal.signal(signal.SIGINT, signal.SIG_DFL)


for trial in range(1, a.trials + 1):
    if a.prepare:
        subprocess.run(a.prepare, shell=True, check=True)
    out = os.path.join(a.logs, f"sigint-{trial}.log")
    p = subprocess.Popen(cmd, env=env, stdin=subprocess.DEVNULL, stdout=open(out, "w"), stderr=subprocess.STDOUT, preexec_fn=default_sigint)
    if a.marker:
        t0 = time.time()
        while time.time() - t0 < 600 and a.marker not in open(out).read() and p.poll() is None:
            time.sleep(0.1)
        time.sleep(a.delay)
    else:
        time.sleep(a.after or 5.0)
    if p.poll() is not None:
        print(f"trial {trial}: finished before the signal, returncode={p.returncode}")
        continue
    p.send_signal(signal.SIGINT)
    t = time.time()
    try:
        rc = p.wait(timeout=a.timeout)
        print(f"trial {trial}: ended {time.time() - t:.2f} s after SIGINT, returncode={rc}")
    except subprocess.TimeoutExpired:
        print(f"trial {trial}: still running {a.timeout:.0f} s after SIGINT")
        p.terminate()
        p.wait()
