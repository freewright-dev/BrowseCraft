#!/usr/bin/env python3
"""Check native Servo focus on X11 with xdotool and a disposable local page.

Run on a dedicated display (for example Xvfb), not a desktop in active use:
    python3 tools/servo-compat/focus_smoke.py [path/to/browsecraft]
"""

import http.server
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
import time


PAGE = b"""<!doctype html><meta charset="utf-8"><title>Servo focus probe</title>
<style>body { font: 24px sans-serif; padding: 20px; } input { font: inherit; }</style>
<h1>Servo page focus</h1>
<p id="state">Waiting</p><input placeholder="Click here to focus the page">
<p>Address bar or another window: false. Page input: true.</p>
<script>
let previous;
setInterval(() => {
  const focused = document.hasFocus();
  if (focused === previous) return;
  previous = focused;
  state.textContent = 'document.hasFocus(): ' + focused;
  console.warn('SERVO_FOCUS ' + focused);
}, 100);
</script>"""


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(PAGE)

    def log_message(self, *args):
        pass


def xdo(*args):
    return subprocess.check_output(["xdotool", *map(str, args)], text=True, timeout=5).strip()


def wait_focus(log, expected):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        states = re.findall(r"SERVO_FOCUS (true|false)", log.read_text(errors="replace"))
        if states and states[-1] == str(expected).lower():
            time.sleep(0.3)
            return
        time.sleep(0.1)
    raise AssertionError(f"document.hasFocus() did not become {expected}: {states}")


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/browsecraft").resolve()
    output = Path("target/servo-focus").resolve()
    output.mkdir(parents=True, exist_ok=True)
    log_path = output / "browser.log"
    with tempfile.TemporaryDirectory(prefix="servo-focus-") as config:
        env = dict(os.environ, XDG_CONFIG_HOME=config, BROWSECRAFT_SERVO_TRACE="1", RUST_LOG="error")
        env.pop("BROWSECRAFT_SERVO_FRAME", None)
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            threading.Thread(target=server.serve_forever, daemon=True).start()
            with log_path.open("w") as log:
                browser = subprocess.Popen([str(binary), f"http://127.0.0.1:{server.server_port}/"],
                                           env=env, stdout=log, stderr=subprocess.STDOUT)
                try:
                    deadline = time.monotonic() + 15
                    window = None
                    while time.monotonic() < deadline:
                        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(browser.pid)],
                                               capture_output=True, text=True)
                        if found.returncode == 0 and "Servo focus probe" in log_path.read_text(errors="replace"):
                            window = found.stdout.splitlines()[0]
                            break
                        time.sleep(0.1)
                    if window is None:
                        raise AssertionError("Browser window/page did not start")
                    xdo("windowfocus", "--sync", window)
                    xdo("mousemove", "--window", window, 380, 295, "click", 1)
                    wait_focus(log_path, True)
                    print("PASS: page click owns focus")
                    xdo("mousemove", "--window", window, 400, 22, "click", 1)
                    wait_focus(log_path, False)
                    print("PASS: address click blurs page")
                    xdo("mousemove", "--window", window, 380, 295, "click", 1)
                    wait_focus(log_path, True)
                    xdo("key", "ctrl+l")
                    wait_focus(log_path, False)
                    print("PASS: address bar blurs page")
                    xdo("mousemove", "--window", window, 380, 295, "click", 1)
                    wait_focus(log_path, True)
                    print("PASS: page click restores focus")
                    xdo("windowfocus", 0)
                    wait_focus(log_path, False)
                    print("PASS: host window focus loss blurs page")
                    xdo("windowfocus", "--sync", window)
                    wait_focus(log_path, True)
                    print("PASS: host window focus return restores page")
                    xdo("mousemove", "--window", window, 1256, 22, "click", 1)
                    wait_focus(log_path, False)
                    print("PASS: browser popup blurs page")
                    xdo("mousemove", "--window", window, 380, 295, "click", 1)
                    time.sleep(0.3)
                    xdo("click", 1)
                    wait_focus(log_path, True)
                    print("PASS: returning from popup restores page focus")
                finally:
                    browser.terminate()
                    try:
                        browser.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        browser.kill()
                        browser.wait()
                    server.shutdown()
    print(f"Servo focus smoke: PASS; {log_path}")


if __name__ == "__main__":
    main()
