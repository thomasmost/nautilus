#!/usr/bin/env python3
"""The scripted run for chrome.tabs and chrome.windows over the bridge.

    export CEF_PATH=~/.local/share/cef
    cargo xtask bundle-mac
    python3 ci/side-panel/phase5.py

Launches the bundle twice on a throwaway profile with the test extension
(ci/side-panel/ext): once to run the checks, once with --restore for the
deferred tab. The extension is loaded through the browser's test control
(NAUTILUS_TEST_CONTROL: a directory holding cmd-1.json, {"load": <dir>}),
the same copy-and-rewrite path as "Load unpacked...". The extension posts what
it saw to this script, which judges it. Prints one PASS or FAIL line per check
and exits non-zero on any FAIL.

A launch fails on a non-zero or signal exit, on a launch still up GRACE seconds
after its --exit-after (the script kills it by pid, and its helpers by parent
pid, only while the pid is still the browser it started), and on the log
markers that run.sh checks. Judge the run by its exit status, never through a
pipe. The owner's own Nautilus may run beside it: the window counts are of the
launched process's windows only (windows.swift takes its pid).
"""
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
APP = REPO / "target/release/Nautilus.app/Contents/MacOS/Nautilus"
PROFILE = "side-panel-p5"
PROFILE_DIR = Path.home() / "Library/Application Support/Nautilus/profiles" / PROFILE
PORT = 47615
SYNTHETIC_BASE = 2_000_000_000
GRACE = 120  # as run.sh's allow: a launch still up this long after its --exit-after is killed
MARKERS = ("panicked at", "fatal runtime error", "already borrowed",
           "already mutably borrowed", "browser(s) still open at shutdown")
SERVED = ("tabs.query", "tabs.get", "tabs.create", "tabs.update", "tabs.remove", "tabs.move",
          "tabs.duplicate", "tabs.highlight", "tabs.sendMessage", "windows.getAll", "windows.get",
          "windows.create", "windows.remove", "windows.getCurrent", "windows.getLastFocused",
          "windows.update")

records = []
pending = {"suite": None}
lock = threading.Lock()
CONTROL = {}   # the running launch's control directory, and the next command number
WORK = None
WATCHER = None


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _ok(self, body=b"ok"):
        self.send_response(200)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.send_header("Access-Control-Allow-Private-Network", "true")
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self._ok()

    def do_GET(self):
        # A suite is handed out once: a worker that restarts does not run
        # it again.
        with lock:
            name, pending["suite"] = pending["suite"], None
        self._ok(json.dumps({"suite": name}).encode())

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        record = json.loads(body)
        with lock:
            records.append(record)
            data = record.get("data") or {}
            if record.get("kind") == "step":
                # Where a run stops is the last step reported.
                info(f"step {data.get('step')}: " + ("ok" if data.get("ok") else "failed: " + str(data.get("error"))))
            # The web-page step asks the relay page, once, what it holds: the
            # command is written now and read by the browser on its next turn.
            if (record.get("kind") == "step" and data.get("step") == "web-page" and CONTROL.get("dir")
                    and not CONTROL.get("relay_asked") and (data.get("value") or {}).get("id")):
                CONTROL["relay_asked"] = True
                extension = data["value"]["id"]
                write_command(CONTROL["dir"], CONTROL["next"], {"relay_eval": {
                    "extension": extension,
                    "label": "the relay page",
                    "expression": "JSON.stringify({bridge: typeof self.__nautilusBridge, runtime: typeof chrome.runtime.id})",
                }})
                CONTROL["next"] += 1
        self._ok()


failures = []


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f": {detail}"), flush=True)
    if not ok:
        failures.append(name)


def info(text):
    print("INFO " + text, flush=True)


def of_kind(kind):
    with lock:
        return [r for r in records if r["kind"] == kind]


def done(suite):
    """The suite ran to its end. A `failed: ...` record is the suite throwing, not finishing."""
    return any(r["data"] == suite for r in of_kind("done"))


def write_command(directory, number, command):
    """A command is written whole, by rename, so the engine never reads half of one."""
    tmp = Path(directory) / f"cmd-{number}.json.tmp"
    tmp.write_text(json.dumps(command))
    tmp.rename(Path(directory) / f"cmd-{number}.json")


def ps_table():
    out = subprocess.run(["ps", "-A", "-o", "pid=,ppid="], capture_output=True, text=True).stdout
    table = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0].isdigit() and parts[1].isdigit():
            table.append((int(parts[0]), int(parts[1])))
    return table


def descendants(pid):
    """The processes whose parent chain leads to pid (its helpers), by pid."""
    table = ps_table()
    found, frontier = [], [pid]
    while frontier:
        parent = frontier.pop()
        kids = [child for child, ppid in table if ppid == parent]
        found += kids
        frontier += kids
    return found


def launched_still_up(pid):
    """run.sh's test: the pid is up, is not a zombie, and its command line starts
    with the browser this script launched. A pid that has ended, or was reused,
    is not ours to kill."""
    result = subprocess.run(["ps", "-o", "stat=,command=", "-p", str(pid)], capture_output=True, text=True)
    parts = result.stdout.strip().split(None, 1)
    if result.returncode != 0 or not parts or parts[0].startswith("Z"):
        return False
    return len(parts) == 2 and parts[1].startswith(str(APP))


def kill_launched(pid):
    """Kill the launched browser and the helpers it started, each by pid. The
    helpers are listed while the tree is still whole (they are re-parented when
    the browser dies)."""
    helpers = descendants(pid)
    for target in helpers + [pid]:
        try:
            os.kill(target, signal.SIGKILL)
        except ProcessLookupError:
            pass


def read_samples(path):
    samples = []
    for line in Path(path).read_text().splitlines():
        parts = line.split()
        if len(parts) == 2:
            samples.append((int(parts[0]), int(parts[1])))
    return samples


def launch(suite, args, seconds, log_path, load_extension, watch):
    """One launch. Returns (exit status, the log's text, the window samples).
    Checks: the launch is not still up after GRACE seconds, and its log has none
    of the markers. The exit status is judged by the caller."""
    with lock:
        records.clear()
        pending["suite"] = suite
    env = dict(os.environ, RUST_LOG="info,nautilus_engine=debug", NAUTILUS_PUMP_LOG="1")
    # The browser's one test hook: it carries out cmd-N.json from this
    # directory, in order. A fresh directory per launch (mode 0700 from mkdtemp),
    # since each process starts at cmd-1.json.
    control = Path(tempfile.mkdtemp(prefix="nautilus-p5-ctl-"))
    env["NAUTILUS_TEST_CONTROL"] = str(control)
    if load_extension:
        write_command(control, 1, {"load": str(HERE / "ext")})
    CONTROL.update({"dir": str(control), "next": 2, "relay_asked": False})
    command = [str(APP), "--profile", PROFILE, "--exit-after", str(seconds), *args]
    windows_path = WORK / f"windows-{suite}.txt"
    with open(log_path, "wb") as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
    watcher = None
    if watch:
        with open(windows_path, "w") as out:
            watcher = subprocess.Popen([str(WATCHER), "Nautilus", str(seconds + GRACE), str(process.pid)], stdout=out)
    still_up = False
    try:
        code = process.wait(timeout=seconds + GRACE)
    except subprocess.TimeoutExpired:
        still_up = True
        if launched_still_up(process.pid):
            kill_launched(process.pid)
        code = process.wait()
    finally:
        if watcher is not None:
            watcher.terminate()
            watcher.wait()
        CONTROL.clear()
        shutil.rmtree(control, ignore_errors=True)
    text = Path(log_path).read_text(errors="replace")
    check(f"{suite}: the browser exited by itself, not still up {GRACE} s after its --exit-after", not still_up,
          f"pid {process.pid} was still up and was killed by this script")
    for marker in MARKERS:
        check(f"{suite}: its log has no '{marker}'", marker not in text,
              [line for line in text.splitlines() if marker in line][:2])
    samples = read_samples(windows_path) if watch else []
    return code, text, samples


def steps():
    return {r["data"]["step"]: r["data"] for r in of_kind("step")}


def value(all_steps, name):
    step = all_steps.get(name)
    check(f"{name}: ran", bool(step) and step["ok"], (step or {}).get("error", "the step never reported"))
    return step["value"] if step and step["ok"] else None


def events_named(events, name):
    return [e["args"] for e in events if e["name"] == name]


SENT = re.compile(r"event (\S+) \(delivery (\d+)\) sent$")
TAKEN = re.compile(r"event \S+ \(delivery (\d+)\) taken by the worker")


def judge_deliveries(log):
    """Each event the engine sent reached the worker once: a delivery number is
    sent once and taken once, and the events the worker listens for are counted."""
    sent = [(m.group(1), int(m.group(2))) for m in (SENT.search(line.strip()) for line in log.splitlines()) if m]
    taken = [int(m.group(1)) for m in (TAKEN.search(line) for line in log.splitlines()) if m]
    numbers = [number for _, number in sent]
    counts = {}
    for name, _ in sent:
        counts[name] = counts.get(name, 0) + 1
    info("events the engine sent this launch: " + (", ".join(f"{name} x{n}" for name, n in sorted(counts.items())) or "none"))
    check("every event the engine sent was taken by the worker, each delivery once",
          len(numbers) == len(set(numbers)) and sorted(numbers) == sorted(taken),
          f"sent {sorted(numbers)}, taken {sorted(taken)}")


def judge_main(code, log, samples):
    check("main: the browser exited cleanly", code == 0, f"exit {code}")
    check("main: the suite finished", done("main"), str(of_kind("done")))
    s = steps()

    tabs = value(s, "query-all")
    if tabs:
        ids = [t["id"] for t in tabs]
        window = tabs[0]["windowId"]
        check("AC3.1 every tab is listed in strip order",
              len(tabs) >= 2 and tabs[0]["url"].startswith("https://example.com/")
              and tabs[1]["url"].startswith("https://example.org/")
              and [t["index"] for t in tabs] == list(range(len(tabs))), str(tabs))
        check("AC3.1 active, windowId and groupId match the model",
              sum(t["active"] for t in tabs) == 1 and window >= 1
              and all(t["windowId"] == window and t["groupId"] == -1 for t in tabs), str(tabs))
        check("AC3.4 the ids are Chromium's", all(0 < i < SYNTHETIC_BASE for i in ids) and len(set(ids)) == len(ids), str(ids))
        active = value(s, "query-active-current")
        if active is not None:
            check("AC3.1 {active, currentWindow} is the window's active tab",
                  [t["id"] for t in active] == [t["id"] for t in tabs if t["active"]], str(active))
        windows = value(s, "windows")
        if windows:
            check("AC3.1 windows.getAll lists the window with its tabs",
                  len(windows["all"]) == 1 and windows["all"][0]["id"] == window
                  and [t["id"] for t in windows["all"][0]["tabs"]] == ids
                  and windows["current"] == window and windows["lastFocused"] == window, str(windows))
    web = value(s, "web-page")
    if web:
        check("a web page has no bridge: no __bridgeCall, __nautilusBridgeSend or __nautilusBridge in its main world",
              web["page"] == {"call": "undefined", "send": "undefined", "bridge": "undefined"}, str(web))
    # The engine logs the relay page's answer as a JSON string of a JSON object.
    relay = [line for line in log.splitlines() if "answers the relay page:" in line]
    answers = []
    for line in relay:
        try:
            answers.append(json.loads(json.loads(line.split("answers the relay page: ", 1)[1])))
        except ValueError:
            pass
    check("the relay page is not bridged (no __nautilusBridge in it)",
          bool(answers) and all(answer.get("bridge") == "undefined" for answer in answers),
          relay[:2] or "no answer from the relay page in the log")

    events = value(s, "events") or []
    created = value(s, "create")
    new = created["tab"]["id"] if created else None
    if created and tabs:
        check("AC3.3 create resolves with the tab that exists afterwards",
              created["got"]["id"] == new and created["last"]["id"] == new
              and created["count"] == len(tabs) + 1 and created["tab"]["index"] == len(tabs)
              and created["tab"]["active"] is False and 0 < new < SYNTHETIC_BASE, str(created))
        check("AC3.2 onCreated fired for it",
              any(a[0]["id"] == new for a in events_named(events, "tabs.onCreated")), str(events_named(events, "tabs.onCreated")))
        check("AC3.2 onUpdated reported its load",
              any(a[0] == new and a[1].get("status") == "complete" for a in events_named(events, "tabs.onUpdated")),
              str(events_named(events, "tabs.onUpdated"))[:600])
    activated = value(s, "activate")
    if activated:
        check("AC3.3 update({active: true}) activates the tab",
              activated["tab"]["id"] == new and activated["tab"]["active"] is True
              and [t["id"] for t in activated["active"]] == [new], str(activated))
        check("AC3.2 onActivated fired for it",
              any(a[0]["tabId"] == new for a in events_named(events, "tabs.onActivated")), str(events_named(events, "tabs.onActivated")))
    moved = value(s, "move")
    if moved:
        check("AC3.3 move puts the tab first", moved["tab"]["index"] == 0 and moved["order"][0] == new, str(moved))
        check("AC3.2 onMoved fired for it",
              any(a[0] == new and a[1]["toIndex"] == 0 for a in events_named(events, "tabs.onMoved")), str(events_named(events, "tabs.onMoved")))
    by_id = value(s, "by-id")
    if by_id:
        check("AC3.4 scripting.executeScript by id", by_id["executeScript"] == "Example Domain", str(by_id["executeScript"]))
        check("AC3.4 tabs.sendMessage by id", by_id["sendMessage"] == "pong example.net", str(by_id["sendMessage"]))
        check("AC3.4 tabs.update({url}) and webNavigation by id",
              by_id["update"]["id"] == new and by_id["update"]["windowId"] >= 1
              and "p5-nav" in by_id["navigation"]["url"], str(by_id))
        check("AC3.4 chrome.debugger by id",
              by_id["evaluate"] == 42 and by_id["screenshot"] > 1000 and by_id["input"] == "dispatched", str(by_id))
        check("captureVisibleTab answers a JPEG data URL", by_id["capture"] == "data:image/jpeg;base64,", str(by_id["capture"]))
    closed = value(s, "fail-closed")
    if closed:
        check("AC3.5 tabs.group rejects without reaching Chromium", closed["group"] == "tabs.group is not supported.", closed["group"])
        check("AC3.5 tabs.discard answers without reaching Chromium", closed["discard"] == "undefined", closed["discard"])
        check("an unknown tab rejects in Chromium's words, both styles",
              closed["unknown"] == "No tab with id: 999999." and closed["callback"] == "No tab with id: 999999.", str(closed))
        check("AC3.3 duplicate opens a tab and remove closes it",
              closed["duplicate"]["id"] != new and closed["countAfterDuplicate"] == closed["countAfterRemove"] + 1
              and closed["getRemoved"] == f"No tab with id: {closed['duplicate']['id']}.", str(closed))
    removed = value(s, "remove")
    if removed:
        check("AC3.3 remove closes the tab", removed["get"] == f"No tab with id: {new}.", str(removed))
        check("AC3.2 onRemoved fired for it",
              any(a[0] == new and a[1]["isWindowClosing"] is False for a in events_named(events, "tabs.onRemoved")),
              str(events_named(events, "tabs.onRemoved")))
    page_tab = value(s, "page")
    pages = of_kind("page")
    check("an extension page shown as a tab reported", len(pages) == 1, str(pages))
    if page_tab and pages:
        result, sender = pages[0]["data"]["result"], pages[0]["data"]["senderTab"]
        check("the page is bridged before its scripts run",
              result.get("installed") and "tabs" in result["installed"] and "windows" in result["installed"]
              and result.get("native") is True and "error" not in result, str(result))
        check("AC3.1 a page's current window is the one showing it",
              result.get("current", {}).get("id") == page_tab["id"] and result["current"]["windowId"] == page_tab["windowId"]
              and result["activeCurrent"] == [page_tab["id"]] and result["window"] == page_tab["windowId"], str(result))
        check("AC3.5 a page's tabs.group rejects too", result.get("group") == "tabs.group is not supported.", str(result.get("group")))
        check("sender.tab carries the strip's fields",
              sender and sender["id"] == page_tab["id"] and sender["windowId"] == page_tab["windowId"] and sender["index"] >= 0, str(sender))
    made = value(s, "windows-create")
    if made:
        check("windows.create opens a window and windows.remove closes it",
              made["during"] == made["before"] + 1 and made["after"] == made["before"]
              and len(made["tabs"]) == 1 and made["tabs"][0]["windowId"] == made["id"], str(made))
        check("windows.onCreated and onRemoved fired",
              any(a[0]["id"] == made["id"] for a in events_named(events, "windows.onCreated"))
              and [made["id"]] in events_named(events, "windows.onRemoved"), str([e for e in events if e["name"].startswith("windows.")]))
    info("windows.onFocusChanged events the worker received: %d" % len(events_named(events, "windows.onFocusChanged")))

    judge_deliveries(log)
    served = [line for line in log.splitlines() if "is not supported" in line and any(m in line for m in SERVED)]
    check("no 'not supported' answer for a tabs or windows method the router serves", not served, served[:3])
    check("AC3.5 Chromium opened no browser of its own",
          "on its own; moving it to a tab" not in log, "the log has StrayClient's line")
    check("no bridge error in any context", "nautilus-bridge:" not in log,
          "\n".join(line for line in log.splitlines() if "nautilus-bridge:" in line)[:600])
    info(f"Chromium asked for a default client {log.count('asked for a client for a browser of its own')} time(s)")
    marks = {r["data"]: r["at"] for r in of_kind("mark")}
    opened = marks.get("window-open", float("inf")) - 3000
    shut = marks.get("window-closed", float("-inf"))
    outside = [count for at, count in samples if not opened <= at <= shut]
    check("AC3.5 no Chromium window appeared: never more than the one browser window",
          bool(samples) and max(outside, default=0) <= 1 and max(count for _, count in samples) <= 2,
          f"window counts over time: {samples}")


def judge_restore(code, log):
    check("restore: the browser exited cleanly", code == 0, f"exit {code}")
    check("restore: the suite finished", done("restore"), str(of_kind("done")))
    s = steps()
    listed = value(s, "restore-list")
    if listed:
        deferred = [t for t in listed if t["discarded"]]
        check("AC3.6 a deferred tab is listed with its address, a synthetic id and its place",
              bool(deferred) and all(t["id"] >= SYNTHETIC_BASE and t["url"].startswith("http") and t["status"] == "unloaded"
                                     and isinstance(t.get("title"), str) for t in deferred)
              and [t["index"] for t in listed] == list(range(len(listed))), str(listed))
    shown = value(s, "restore-show")
    if shown:
        check("AC3.6 it gets Chromium's id when first shown, announced by onReplaced",
              shown["replaced"] == [shown["shown"]["id"], shown["was"]["id"]]
              and 0 < shown["shown"]["id"] < SYNTHETIC_BASE and shown["shown"]["active"] is True, str(shown))
        check("AC3.6 the new id works in Chromium and the old one still resolves",
              shown["href"] == shown["was"]["url"] and shown["byOld"]["id"] == shown["shown"]["id"], str(shown))
    check("restore: Chromium opened no browser of its own", "on its own; moving it to a tab" not in log, "StrayClient's line")


def main():
    global WORK, WATCHER
    if not APP.exists():
        sys.exit(f"{APP} is missing: cargo xtask bundle-mac")
    WORK = Path(tempfile.mkdtemp(prefix="nautilus-p5-"))
    shutil.rmtree(PROFILE_DIR, ignore_errors=True)
    server = HTTPServer(("127.0.0.1", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    WATCHER = WORK / "p5-windows"
    subprocess.run(["swiftc", "-O", str(HERE / "windows.swift"), "-o", str(WATCHER)], check=True)
    try:
        code, log, samples = launch("main", ["https://example.com/", "https://example.org/"], 90,
                                    WORK / "main.log", True, watch=True)
        judge_main(code, log, samples)
        code, log, _ = launch("restore", ["--restore"], 45, WORK / "restore.log", False, watch=False)
        judge_restore(code, log)
    finally:
        server.shutdown()
        shutil.rmtree(PROFILE_DIR, ignore_errors=True)
    print(f"logs: {WORK}")
    print("phase 5: " + ("all checks passed" if not failures else f"{len(failures)} check(s) FAILED"), flush=True)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
