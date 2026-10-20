#!/usr/bin/env python3
"""Exercise real Servo callbacks, with an external watchdog for script-thread hangs.

Build with `cargo build --features servo`, then run this with a display available:
    python3 tools/servo-compat/smoke.py [path/to/browsecraft]
Logs and the latest raw frame are saved in target/servo-smoke.
"""

import http.server
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time


PAGE = b"""<!doctype html><meta charset="utf-8"><title>Servo smoke: waiting</title>
<style>
body { background: white; font: 24px sans-serif; }
.box { width: 120px; height: 50px; background: #bde5c3; }
#wrapper { display: contents; }
#outside { position: absolute; top: 10000px; }
</style>
<h1>Servo rendering smoke test</h1>
<div id="wrapper"><div id="contents" class="box">Contents</div></div>
<div id="host"><div id="slotted" class="box">Slotted</div></div>
<div id="outside" class="box">Offscreen</div>
<div class="ad-slot">Bundled rule: this should be hidden</div>
<div id="generic" class="smoke-ad">Generic: this should be hidden</div>
<div id="exception" class="smoke-except">Visible only on 127.0.0.1</div>
<div id="scoped" class="smoke-host">Visible only on 127.0.0.1</div>
<div id="negated" class="smoke-neg">Visible only on localhost</div>
<p id="result">Waiting for observers and a timer...</p>
<script>
host.attachShadow({mode: 'open'}).innerHTML = '<slot></slot>';
const expected = new Map([['contents', true], ['slotted', true], ['outside', false]]);
const seen = new Set();
let timerRan = false;
let failed = false;
let cosmeticsReady = false;
let frameReady = window !== parent;
const dynamic = document.createElement('div');
dynamic.className = 'smoke-ad';
dynamic.textContent = 'Dynamic ad: this should be hidden';
document.body.appendChild(dynamic);
if (window === parent) {
  const frame = document.createElement('iframe');
  frame.width = '700';
  frame.height = '350';
  const origin = 'http://' + (location.hostname === 'localhost' ? '127.0.0.1' : 'localhost') + ':' + location.port;
  addEventListener('message', event => {
    if (event.source === frame.contentWindow && event.origin === origin && event.data === 'cosmetics-pass') {
      frameReady = true;
      finish();
    }
  });
  frame.src = origin + '/frame';
  frame.title = 'Cross-origin cosmetic filtering control';
  document.body.appendChild(frame);
}
function fail(message) {
  failed = true;
  result.textContent = 'FAIL: ' + message;
  console.warn('SERVO_SMOKE FAIL ' + message);
}
function finish() {
  if (failed || (window === parent && seen.size !== expected.size) || !timerRan || !cosmeticsReady || !frameReady) return;
  if (getComputedStyle(document.documentElement).color !== 'rgb(0, 0, 0)') {
    fail('Default text must remain readable on a white page');
    return;
  }
  observer.disconnect();
  if (window !== parent) {
    parent.postMessage('cosmetics-pass', '*');
    return;
  }
  if (location.pathname === '/') {
    location.replace('http://localhost:' + location.port + '/next');
    return;
  }
  document.title = result.textContent = 'Servo smoke: PASS';
  console.warn('SERVO_SMOKE PASS: observers, timer, color; cosmetics: static, dynamic, exceptions, hosts, frames, navigation');
}
const observer = new IntersectionObserver(entries => {
  for (const entry of entries) {
    if (entry.isIntersecting !== expected.get(entry.target.id)) {
      fail('Wrong intersection for ' + entry.target.id);
      return;
    }
    seen.add(entry.target.id);
  }
  finish();
});
if (window === parent) {
  for (const id of expected.keys()) observer.observe(document.getElementById(id));
}
setTimeout(() => { timerRan = true; finish(); }, 300);
setTimeout(() => {
  const hidden = element => getComputedStyle(element).display === 'none';
  const local = location.hostname === 'localhost';
  if (!hidden(generic) || !hidden(dynamic) || !hidden(document.querySelector('.ad-slot')) ||
      hidden(exception) !== local || hidden(scoped) !== local || hidden(negated) === local) {
    fail('Cosmetic rules/host exceptions incorrect on ' + location.host);
    return;
  }
  cosmeticsReady = true;
  finish();
}, 1500);
</script>"""


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(PAGE)

    def log_message(self, *args):
        pass


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/browsecraft").resolve()
    output = Path("target/servo-smoke").resolve()
    output.mkdir(parents=True, exist_ok=True)
    frame = output / "frame.png"
    frame.unlink(missing_ok=True)
    log_path = output / "browser.log"
    with tempfile.TemporaryDirectory(prefix="servo-smoke-") as config:
        filters = Path(config) / "filters"
        filters.mkdir()
        (filters / "smoke.txt").write_text(
            "##.smoke-ad\n##.smoke-except\n127.0.0.1#@#.smoke-except\n"
            "localhost##.smoke-host\n~localhost##.smoke-neg\n")
        env = dict(os.environ, XDG_CONFIG_HOME=config, BROWSECRAFT_SERVO_TRACE="1",
                   BROWSECRAFT_SERVO_FRAME=str(frame), RUST_LOG="error")
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            threading.Thread(target=server.serve_forever, daemon=True).start()
            with log_path.open("w") as log:
                process = subprocess.Popen(
                    [str(binary), f"http://127.0.0.1:{server.server_port}/"],
                    cwd=config, env=env, stdout=log, stderr=subprocess.STDOUT)
                passed = False
                try:
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline:
                        text = log_path.read_text(errors="replace")
                        if "SERVO_SMOKE FAIL" in text or "panicked" in text or process.poll() is not None:
                            break
                        if "SERVO_SMOKE PASS" in text:
                            passed = True
                            # Let the asynchronous paint callback save the result too.
                            time.sleep(0.5)
                            break
                        time.sleep(0.1)
                finally:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                    server.shutdown()
    print(log_path.read_text(errors="replace"))
    print(f"Servo smoke: {'PASS' if passed else 'FAIL (error or watchdog expired)'}; {output}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
