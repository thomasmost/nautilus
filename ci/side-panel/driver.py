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
import re
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
    print(('PASS ' if ok else 'FAIL ') + name + (': ' + str(detail) if detail not in (None, '') else ''), flush=True)
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
    probe = wait_for(0, 'probe', 20)
    check('install: onConnect.hasListeners is false before the worker adds a listener (M8)',
          probe is not None and probe.get('before') is False, probe and probe.get('before'))
    with_spy = (probe or {}).get('withSpy') or {}
    check('install: with a spy added, hasListeners and hasListener are true (M8)',
          with_spy.get('hasListeners') is True and with_spy.get('hasListener') is True, with_spy)
    check('install: with the spy removed, hasListeners is false again (M8)',
          probe is not None and probe.get('removed') is False, probe and probe.get('removed'))
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
        if tab is tabs[0]:
            # I8: the attach and the detach that name the tab by its target id.
            by_target = calls.get('debugger.attach({targetId})')
            check('%s (I8): debugger.attach({targetId}) accepts the first tab' % label,
                  bool(by_target and by_target.get('ok')),
                  None if by_target and by_target.get('ok') else (by_target or {}).get('error', 'no call was recorded'))
            detached = calls.get('debugger.detach({targetId})')
            check('%s (I8): debugger.detach({targetId}) accepts the first tab' % label,
                  bool(detached and detached.get('ok')),
                  None if detached and detached.get('ok') else (detached or {}).get('error', 'no call was recorded'))


def worker_running(control, ext_id):
    """True or False from a target list; None when there is no list (no answer,
    or an error), since an empty list would read as a stopped worker."""
    answer = control.devtools('Target.getTargets')
    infos = ((answer or {}).get('result') or {}).get('targetInfos')
    if infos is None:
        return None
    return any(t.get('type') == 'service_worker' and ext_id in t.get('url', '') for t in infos)


def stop_worker(control, ext_id):
    """Stop the worker with ServiceWorker.stopAllWorkers over DevTools, after
    seeing it running, so that 'stopped' means it was running and is gone.
    The clock starts before the stop. Falls back to waiting out the idle stop
    when stopAllWorkers gets no answer."""
    seen = False
    for _ in range(5):
        if worker_running(control, ext_id):
            seen = True
            break
        time.sleep(2)
    if not seen:
        print('INFO the worker was not observed running before the stop', flush=True)
        return False
    started = time.time()
    control.devtools('ServiceWorker.enable', secs=5)
    answered = control.devtools('ServiceWorker.stopAllWorkers', secs=5) is not None
    while time.time() - started < 90:
        running = worker_running(control, ext_id)
        if running is False:
            print('INFO the worker stopped after %.1f s (%s)' % (
                time.time() - started,
                'by DevTools' if answered else 'no answer to stopAllWorkers; idle stop'), flush=True)
            return True
        time.sleep(3)
    return False


def engine_lines(launch):
    """The engine's log of one launch (the browser's stderr and stdout)."""
    try:
        with open(os.path.join(WORK, 'run-%d.log' % launch), errors='replace') as f:
            return f.read().splitlines()
    except OSError:
        return []


def first_index(lines, text, after=-1):
    for i in range(after + 1, len(lines)):
        if text in lines[i]:
            return i
    return -1


def check_debugged_tab(label, launch, chromium):
    """(I7) The tab the latency run debugged was logged hidden before the
    attach and shown at it, and hidden again after the detach. The tab is
    named by Chromium's id, through the table's rows in the same log."""
    lines = engine_lines(launch)
    rows = [m for m in (re.search(r'TabId\((\d+)\) is Chromium tab %s ' % chromium, line) for line in lines) if m]
    if not check(label + ' (I7): the engine log names the debugged tab (Chromium tab %s)' % chromium,
                 bool(rows), None if rows else 'run-%d.log has no table row for it' % launch):
        return
    tab = 'TabId(%s)' % rows[-1].group(1)
    hidden_text = '%s: hidden, painting stopped = true' % tab
    # The latency run's attach is the launch's last one (its checks ran
    # first, and nothing attaches after it), so the last shown line is its.
    shown_text = 'bridge: %s (Chromium tab %s) was hidden; shown for the debugger' % (tab, chromium)
    shown_at = [i for i, line in enumerate(lines) if shown_text in line]
    shown = shown_at[-1] if shown_at else -1
    hidden_before = [i for i in range(max(shown, 0)) if hidden_text in lines[i]]
    check(label + ' (I7): %s was logged hidden before the attach (run-%d.log)' % (tab, launch),
          bool(hidden_before), None if hidden_before else 'no hidden line before line %d' % shown)
    check(label + ' (I7): %s was shown at the attach (run-%d.log)' % (tab, launch), shown >= 0,
          None if shown >= 0 else 'no shown line')
    again = first_index(lines, 'bridge: %s (Chromium tab %s) hidden again' % (tab, chromium), shown)
    check(label + ' (I7): %s is hidden again after the detach (run-%d.log)' % (tab, launch),
          shown >= 0 and again > shown, None if shown >= 0 and again > shown else 'no hidden-again line after line %d' % shown)


def latency_result(label, mark, secs, launch):
    done = wait_for(mark, 'latency-done', secs)
    if not check(label + ' (d): the latency run finished', done is not None and not done.get('error'), done and done.get('error')):
        return
    print('INFO %s: %d calls, max %d ms, p95 %d ms, %d timeout(s), tab %s' % (
        label, done['count'], done['max'], done['p95'], done['timeouts'], done.get('tabId')), flush=True)
    check_debugged_tab(label, launch, done.get('tabId'))
    check(label + ' (d): no API call took a second or more', done['over1s'] == 0 and done['timeouts'] == 0,
          '%d over 1 s, max %d ms' % (done['over1s'], done['max']))
    slow = sorted((r['data'] for r in since(mark, 'call', lambda d: d.get('latency'))), key=lambda d: -d['ms'])[:5]
    for call in slow:
        print('INFO   slowest: %5d ms %s%s' % (call['ms'], call['name'], '' if call['ok'] else ' ' + call['error']), flush=True)


def engine_wait_after(launch, after, text, secs):
    """True once the engine's log has `text` after the first line with `after`."""
    end = time.time() + secs
    while time.time() < end:
        lines = engine_lines(launch)
        at = first_index(lines, after)
        if at >= 0 and first_index(lines, text, at) >= 0:
            return True
        time.sleep(0.5)
    return False


def relay_recovery(control, ext_id, launch):
    """(I9, M10) A relay page closed underneath (Target.closeTarget through the
    test control's DevTools channel) is logged gone, opened again by the
    engine's next turn, and a click after that is delivered to the worker."""
    answer = control.devtools('Target.getTargets')
    infos = ((answer or {}).get('result') or {}).get('targetInfos') or []
    # A relay is an extension's own page in a hidden browser; DevTools lists
    # it as type "other", so the type is not tested, only its URL.
    relays = [t for t in infos if ext_id in t.get('url', '') and 'relay.html' in t.get('url', '')]
    if not check('relay (I9): DevTools lists the relay page', len(relays) == 1, len(relays)):
        return
    closed = control.devtools('Target.closeTarget', {'targetId': relays[0]['targetId']}, secs=10)
    check('relay (I9): Target.closeTarget closed the relay page',
          ((closed or {}).get('result') or {}).get('success') is True, closed)
    check('relay (I9): the engine logged the relay gone',
          wait_text(launch, 'bridge: page %s is gone' % ext_id, 20))
    check('relay (M10): the next turn opened the relay again and it is ready',
          engine_wait_after(launch, 'bridge: page %s is gone' % ext_id, 'bridge: page %s ready' % ext_id, 40))
    mark = count()
    control.send({'click': ext_id})
    click = wait_for(mark, 'event', 20, lambda d: d.get('name') == 'action.onClicked')
    check('relay (M10): a click after the reopening reaches the worker', click is not None)


def wait_text(launch, text, secs):
    end = time.time() + secs
    while time.time() < end:
        if first_index(engine_lines(launch), text) >= 0:
            return True
        time.sleep(0.5)
    return False


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

    relay_recovery(control, ext_id, 2)

    # (d) the calls that stalled, with the window unfocused.
    with lock:
        cfg['latency'] = True
    mark = count()
    control.send({'click': ext_id})
    latency_result('unfocused', mark, 150, 2)
    control.send({'table': True})


def pump(ext):
    with lock:
        cfg['latencyAtStart'] = True
    check('run 1 started', wait_file(os.path.join(WORK, 'run-1.started'), 60))
    control = Control(os.path.join(WORK, 'ctl-1'))
    time.sleep(8)  # the two tabs load, so that there is a page target
    control.send({'load': ext})
    latency_result('pump', 0, 140, 1)


if __name__ == '__main__':
    mode, WORK, extension = sys.argv[1], sys.argv[2], sys.argv[3]
    server = ThreadingHTTPServer(('127.0.0.1', PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    {'spike': spike, 'pump': pump}[mode](extension)
    print('RESULT ' + ('all checks passed' if not failures else '%d check(s) failed: %s' % (len(failures), '; '.join(failures))), flush=True)
    sys.exit(1 if failures else 0)
