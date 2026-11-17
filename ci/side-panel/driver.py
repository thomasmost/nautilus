#!/usr/bin/env python3
"""The driver of ci/side-panel: the HTTP server the test extension reports
to, and the writer of the browser's test-control commands.

usage: driver.py spike|pump <work directory> <test extension directory>

run.sh starts it, then launches the browser once (pump) or twice (spike)
with NAUTILUS_TEST_CONTROL=<work>/ctl-<n>, touching <work>/run-<n>.started
before each launch. Prints one PASS or FAIL line per check; exits 0 when
every check passed.
"""
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = 47613
URLS = {'https://example.com/', 'https://example.org/'}
records = []   # what the extension posted, in arrival order
cfg = {}       # what the extension reads from /cfg
lock = threading.Lock()
failures = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _ok(self, body=b'ok'):
        self.send_response(200)
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Headers', '*')
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self._ok()

    def do_GET(self):
        with lock:
            body = json.dumps(cfg).encode()
        self._ok(body if self.path.startswith('/cfg') else b'ok')

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        try:
            record = json.loads(body)
        except ValueError:
            record = {'kind': 'bad', 'data': body.decode('utf-8', 'replace')}
        with lock:
            records.append(record)
            with open(os.path.join(WORK, 'results.jsonl'), 'a') as out:
                out.write(json.dumps(record) + '\n')
        self._ok()


def count():
    with lock:
        return len(records)


def since(mark, kind, test=lambda d: True):
    with lock:
        return [r for r in records[mark:] if r.get('kind') == kind and test(r.get('data') or {})]


def wait_for(mark, kind, secs, test=lambda d: True):
    end = time.time() + secs
    while time.time() < end:
        found = since(mark, kind, test)
        if found:
            return found[0]['data']
        time.sleep(0.2)
    return None


def wait_file(path, secs):
    end = time.time() + secs
    while time.time() < end and not os.path.exists(path):
        time.sleep(0.2)
    return os.path.exists(path)


def check(name, ok, detail=''):
    print(('PASS ' if ok else 'FAIL ') + name + (': ' + str(detail) if detail != '' else ''), flush=True)
    if not ok:
        failures.append(name)
    return ok


class Control:
    """Writes cmd-N.json into a run's control directory."""

    def __init__(self, directory):
        self.dir = directory
        self.n = 0

    def send(self, command):
        self.n += 1
        tmp = os.path.join(self.dir, 'cmd-%d.json.tmp' % self.n)
        with open(tmp, 'w') as f:
            json.dump(command, f)
        os.rename(tmp, os.path.join(self.dir, 'cmd-%d.json' % self.n))
        return self.n

    def devtools(self, method, params=None, secs=10):
        """Send a DevTools message through the management browser; its answer."""
        n = self.send({'devtools': {'method': method, 'params': params or {}}})
        end = time.time() + secs
        while time.time() < end:
            try:
                with open(os.path.join(self.dir, 'devtools.log')) as f:
                    for line in f:
                        try:
                            message = json.loads(line)
                        except ValueError:
                            continue
                        if message.get('id') == 9000 + n:
                            return message
            except FileNotFoundError:
                pass
            time.sleep(0.3)
        return None


def bridged(start):
    bridge = (start or {}).get('bridge') or {}
    return bridge.get('protocol') == 1


def install(ext):
    """Run 1: load the extension through the rewrite; the worker starts bridged."""
    check('run 1 started', wait_file(os.path.join(WORK, 'run-1.started'), 60))
    control = Control(os.path.join(WORK, 'ctl-1'))
    control.send({'load': ext})
    start = wait_for(0, 'sw-start', 40)
    check('install: the worker started', start is not None)
    check('install: the bridge is in the worker', bridged(start), (start or {}).get('bridge'))
    check('install: the worker script is the loader beside the entry',
          (start or {}).get('script') == '/background/nautilus-loader.js', (start or {}).get('script'))
    check('install: the bridge hides its port (U3)', ((start or {}).get('bridge') or {}).get('portHidden') is True)
    return (start or {}).get('id')


def tabs_work(label, mark):
    """(c): tabs.query answered by the engine, its ids accepted by Chromium."""
    done = wait_for(mark, 'checks-done', 60)
    if not check(label + ' (c): tabs.query answered', done is not None and not done.get('error'), done and done.get('error')):
        return
    tabs = done['tabs']
    check(label + ' (c): the tabs are the two Nautilus tabs and nothing else',
          {t['url'] for t in tabs} == URLS and len(tabs) == 2, [t['url'] for t in tabs])
    check(label + ' (c): exactly one tab is active', sum(1 for t in tabs if t['active']) == 1)
    for tab in tabs:
        calls = {c['name']: c for c in (r['data'] for r in since(mark, 'call', lambda d: d.get('tabId') == tab['id']))}
        script = calls.get('scripting.executeScript', {})
        check('%s (c): scripting.executeScript accepts id %s' % (label, tab['id']),
              script.get('ok') and script.get('result') == tab['url'], script.get('error') or script.get('result'))
        attach = calls.get('debugger.attach', {})
        check('%s (c): debugger.attach accepts id %s' % (label, tab['id']), attach.get('ok'), attach.get('error', ''))
        evaluate = calls.get('debugger.Runtime.evaluate', {})
        check('%s (c): the debugger evaluates in that tab' % label,
              evaluate.get('ok') and evaluate.get('result') == tab['url'], evaluate.get('error') or evaluate.get('result'))


def worker_running(control, ext_id):
    answer = control.devtools('Target.getTargets')
    if answer is None:
        return None
    infos = (answer.get('result') or {}).get('targetInfos') or []
    return any(t.get('type') == 'service_worker' and ext_id in t.get('url', '') for t in infos)


def stop_worker(control, ext_id):
    """Stop the worker: by DevTools if the management page's session allows
    it, otherwise by waiting out Chromium's 30 s idle stop."""
    control.devtools('ServiceWorker.enable', secs=5)
    control.devtools('ServiceWorker.stopAllWorkers', secs=5)
    started = time.time()
    while time.time() - started < 90:
        running = worker_running(control, ext_id)
        if running is False:
            took = time.time() - started
            print('INFO the worker stopped after %.0f s (%s)' % (took, 'DevTools' if took < 8 else 'idle timeout'), flush=True)
            return True
        time.sleep(3)
    return False


def latency_result(label, mark, secs):
    done = wait_for(mark, 'latency-done', secs)
    if not check(label + ' (d): the latency run finished', done is not None and not done.get('error'), done and done.get('error')):
        return
    print('INFO %s: %d calls, max %d ms, p95 %d ms, %d timeout(s)' % (
        label, done['count'], done['max'], done['p95'], done['timeouts']), flush=True)
    check(label + ' (d): no API call took a second or more', done['over1s'] == 0 and done['timeouts'] == 0,
          '%d over 1 s, max %d ms' % (done['over1s'], done['max']))
    slow = sorted((r['data'] for r in since(mark, 'call', lambda d: d.get('latency'))), key=lambda d: -d['ms'])[:5]
    for call in slow:
        print('INFO   slowest: %5d ms %s%s' % (call['ms'], call['name'], '' if call['ok'] else ' ' + call['error']), flush=True)


def spike(ext):
    ext_id = install(ext)
    if not ext_id:
        return
    check('run 2 started', wait_file(os.path.join(WORK, 'run-2.started'), 120))
    control = Control(os.path.join(WORK, 'ctl-2'))
    mark = count()
    time.sleep(12)  # the management page, the helper and the relay come up

    # (a) after a restart, and a first click.
    control.send({'click': ext_id})
    click = wait_for(mark, 'event', 20, lambda d: d.get('name') == 'action.onClicked')
    start = wait_for(mark, 'sw-start', 5)
    check('(a) after a restart: the worker runs with the bridge', bridged(start), (start or {}).get('bridge'))
    check('restart: the click reached action.onClicked', click is not None)
    check('restart: the click carries one of the tabs', click is not None and (click.get('tab') or {}).get('url') in URLS,
          click and click.get('tab'))
    tabs_work('restart', mark)

    # (a) after the worker was stopped, (b) one click, delivered once.
    check('the worker stopped', stop_worker(control, ext_id))
    mark = count()
    control.send({'click': ext_id})
    start = wait_for(mark, 'sw-start', 20)
    check('(a) after a stop: the click started the worker, with the bridge', bridged(start), (start or {}).get('bridge'))
    time.sleep(8)
    clicks = since(mark, 'event', lambda d: d.get('name') == 'action.onClicked')
    check('(b) the click was delivered exactly once', len(clicks) == 1, '%d time(s)' % len(clicks))
    check('(b) it was delivered to the worker it started',
          len(clicks) == 1 and start is not None and clicks[0]['data'].get('run') == start.get('run'))
    tabs_work('after a stop', mark)
    check('the extension\'s own onConnect listener never saw the bridge (U3)', not since(0, 'spy'),
          [r['data'] for r in since(0, 'spy')])

    # (d) the calls that stalled, with the window unfocused.
    with lock:
        cfg['latency'] = True
    mark = count()
    control.send({'click': ext_id})
    latency_result('unfocused', mark, 150)
    control.send({'table': True})


def pump(ext):
    with lock:
        cfg['latencyAtStart'] = True
    check('run 1 started', wait_file(os.path.join(WORK, 'run-1.started'), 60))
    control = Control(os.path.join(WORK, 'ctl-1'))
    time.sleep(8)  # the two tabs load, so that there is a page target
    control.send({'load': ext})
    latency_result('pump', 0, 140)


if __name__ == '__main__':
    mode, WORK, extension = sys.argv[1], sys.argv[2], sys.argv[3]
    server = ThreadingHTTPServer(('127.0.0.1', PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    {'spike': spike, 'pump': pump}[mode](extension)
    print('RESULT ' + ('all checks passed' if not failures else '%d check(s) failed: %s' % (len(failures), '; '.join(failures))), flush=True)
    sys.exit(1 if failures else 0)
