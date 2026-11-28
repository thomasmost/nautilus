#!/usr/bin/env python3
"""Side panel checks for a Nautilus built from this checkout (macOS).

    export CEF_PATH=~/.local/share/cef-156
    cargo xtask bundle-mac --features test-hooks
    ci/side-panel/panel.sh panel
    ci/side-panel/panel.sh behavior
    ci/side-panel/panel.sh groups
    ci/side-panel/panel.sh sample-global <cookbook dir>
    ci/side-panel/panel.sh sample-site <cookbook dir>

No DevTools and no remote debugging port. The browser is started by
run.sh serve on a scratch data directory; the test extension
(ci/side-panel/panel-ext) is loaded, and the browser is driven, through
NAUTILUS_TEST_CONTROL (cmd-N.json: `load`, `click`). What it did is read
from the reports its pages and worker post to this script's server on
127.0.0.1:47616. The server starts first, because the test extension's
worker asks it for its suite as it starts.

  panel          chrome.sidePanel in the real window: open for a window and
                 for a tab, the rejections, getContexts, the widths, a
                 tab's panel showing only with its tab, setOptions and
                 window.close() closing a panel, the action click through
                 the test control. Window captures in <out>/shots-panel.
  behavior       two launches on one kept data directory: the first sets
                 setPanelBehavior({openPanelOnActionClick: true}) and checks
                 side-panel-behavior.json; the second calls nothing and a
                 click must still open the panel.
  groups         tab groups and a keyboard command over the bridge, with the
                 calls an extension makes when it opens its side panel:
                 tabs.group, tabGroups.update, get, query, tabs.ungroup; a
                 declared command and _execute_action through the test
                 control's extension_command; then a second tab grouped into
                 an existing group, each read made straight after the call
                 before it. Captures in <out>/shots-groups: the strip with
                 the folder, without it again, and with two tabs in one.
  sample-global, sample-site
                 a chrome-extensions-samples side panel cookbook, copied
                 with panel-probe.js added to its page.

Prints PASS or FAIL per check and exits 1 on any FAIL. Judge it by its
exit status, never through a pipe. The browser stays open for HOLD seconds
(default 180) after the time the checks are allowed (BUDGET), for the steps
by hand, and then exits by itself: this script never kills a browser that
is in time (run.sh serve does, 120 s after that). The owner's own Nautilus
may run beside it: the window that is captured is found by the launched
browser's process id.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
APP = REPO / "target/release/Nautilus.app/Contents/MacOS/Nautilus"
EXT = HERE / "panel-ext"
POPUP_EXT = HERE / "popup-ext"
RUN_SH = HERE / "run.sh"
PORT = 47616
PANEL_WIDTH = 360  # osmena-chrome theme::SIDE_PANEL_WIDTH, in points
# A page's reports come every half second, a hidden page's once a second
# (Chromium throttles its timers): a page silent for this long has stopped
# reporting. That alone is true of a hung page too, so a check that says
# "closed" also reads the engine's log for the view's close (`closed`).
FRESH = 3.0
# As run.sh's and phase5.py's: what a launch's log must not hold.
MARKERS = ("panicked at", "fatal runtime error", "already borrowed",
           "already mutably borrowed", "browser(s) still open at shutdown")
CHROMIUM_API_ERROR = "Failed to create API on Chrome object"
BEHAVIOR_FILE = "side-panel-behavior.json"
SAMPLE_NAMES = {"sample-global": "Global side panel", "sample-site": "Site-specific side panel"}
SAMPLE_URLS = {
    "sample-global": "https://example.com/ https://example.org/",
    # The last address is the tab the window shows, which is the tab the
    # control's click is for.
    "sample-site": "https://example.org/ https://www.google.com/",
}
# The site sample's other half: with example.org showing, a click is an
# ordinary action.onClicked and opens nothing.
SAMPLE_SITE_ELSEWHERE = "https://www.google.com/ https://example.org/"

lock = threading.Lock()
records = []  # {"kind", "data", "at"}, in arrival order
released = set()  # checkpoint names the harness has finished with
suite = {"name": None}  # handed out once, to the first worker that asks


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, body=b"ok", content_type="text/plain"):
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.send_header("Access-Control-Allow-Private-Network", "true")
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self._send()

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/suite":
            with lock:
                name, suite["name"] = suite["name"], None
            return self._send(json.dumps({"suite": name}).encode(), "application/json")
        if url.path == "/go":
            name = parse_qs(url.query).get("name", [""])[0]
            with lock:
                go = name in released
            return self._send(b"go" if go else b"wait")
        self._send()

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        try:
            parsed = json.loads(body)
        except ValueError:
            parsed = {"kind": "bad", "data": body.decode("utf-8", "replace")}
        if not isinstance(parsed, dict):
            parsed = {"kind": "bad", "data": parsed}
        with lock:
            records.append({"kind": parsed.get("kind"), "data": parsed.get("data") or {}, "at": time.time()})
        self._send()


def latest(kind, test=lambda data: True):
    with lock:
        for record in reversed(records):
            if record["kind"] == kind and isinstance(record["data"], dict) and test(record["data"]):
                return record
    return None


def fresh(record, within=FRESH):
    return record is not None and time.time() - record["at"] < within


def wait(test, secs=15.0):
    end = time.time() + secs
    while time.time() < end:
        value = test()
        if value:
            return value
        time.sleep(0.2)
    return None


def ps_children(parent):
    """The (pid, command line) of each process whose parent is `parent`."""
    out = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,command="], capture_output=True, text=True).stdout
    found = []
    for line in out.splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3 and parts[0].isdigit() and parts[1].isdigit() and int(parts[1]) == parent:
            found.append((int(parts[0]), parts[2]))
    return found


# The namespaces the bridge replaces on `chrome`, and so on `browser`.
REPLACED = ("tabs", "windows", "tabGroups", "commands", "sidePanel")


class Harness:
    def __init__(self, mode, args):
        self.mode = mode
        self.args = args
        self.out = Path(args.out)
        self.failed = 0
        self.launches = 0
        self.control = None
        self.commands = 0
        self.log_path = None
        self.ext_id = None
        self.active_url = None
        self.other_url = None
        self.active_id = None
        self.full = None
        self.proc = None
        self.logfile = None
        self.started = None
        self.secs = None
        self.pid = None  # the launched browser's, for its window
        self.window = None  # the CGWindowID of the browser's window, for captures
        self.window_tool = None
        self.facts_seen = set()  # the contexts whose `browser` facts were judged

    # --- results ---------------------------------------------------------

    def check(self, name, ok, detail=""):
        ok = bool(ok)
        suffix = "" if ok or detail == "" else "  " + json.dumps(detail, default=str)[:600]
        print(("PASS  " if ok else "FAIL  ") + name + suffix, flush=True)
        if not ok:
            self.failed += 1
        return ok

    def note(self, text):
        print("NOTE  " + text, flush=True)

    # --- the browser -----------------------------------------------------

    def launch(self, ext, urls, budget, suite_name=None, data_dir=None, load=True, hold=None):
        """One launch through run.sh serve. `budget` is the time the checks
        may take; the browser exits by itself `hold` seconds after it."""
        self.launches += 1
        name = self.mode if self.launches == 1 else "%s-%d" % (self.mode, self.launches)
        with lock:
            records.clear()
            released.clear()
            suite["name"] = suite_name
        # Private to this user, as the engine requires, and empty.
        self.control = Path(tempfile.mkdtemp(prefix="nautilus-panel-ctl-"))
        os.chmod(self.control, 0o700)
        self.commands = 1 if load else 0  # run.sh serve wrote cmd-1.json (the load)
        self.log_path = self.out / (name + ".log")
        self.secs = budget + (self.args.hold if hold is None else hold)
        env = dict(
            os.environ,
            CONTROL=str(self.control),
            PROFILE="side-panel-" + self.mode,
            SECS=str(self.secs),
            URLS=urls,
            LOAD="1" if load else "0",
        )
        if load:
            env["EXT_DIR"] = str(ext)
        else:
            env.pop("EXT_DIR", None)
        if data_dir is not None:
            env["DATA_DIR"] = str(data_dir)
        else:
            env.pop("DATA_DIR", None)
        self.pid = None
        self.window = None
        self.logfile = open(self.log_path, "wb")
        self.started = time.time()
        self.proc = subprocess.Popen([str(RUN_SH), "serve"], env=env, stdout=self.logfile, stderr=subprocess.STDOUT)
        self.note(f"launch {self.launches}: its log is {self.log_path}")

    def send(self, command):
        self.commands += 1
        tmp = self.control / ("cmd-%d.json.tmp" % self.commands)
        tmp.write_text(json.dumps(command))
        tmp.rename(self.control / ("cmd-%d.json" % self.commands))

    def log_text(self):
        try:
            return self.log_path.read_text(errors="replace")
        except (FileNotFoundError, TypeError):
            return ""

    def installed_id(self, name):
        found = re.findall(re.escape(name) + r" \(([a-p]{32}), on\)", self.log_text())
        return found[-1] if found else None

    def browser_pid(self):
        """The browser run.sh serve started: its child whose command line is
        the bundle's executable. Never any other Nautilus."""
        if self.pid is None and self.proc is not None:
            for pid, command in ps_children(self.proc.pid):
                if command.startswith(str(APP)):
                    self.pid = pid
        return self.pid

    def window_id(self):
        pid = self.browser_pid()
        if pid is None:
            return None
        if self.window_tool is None:
            tool = self.out / "window-id"
            built = subprocess.run(["swiftc", "-O", str(HERE / "window_id.swift"), "-o", str(tool)],
                                   capture_output=True, text=True)
            if built.returncode != 0:
                self.note("swiftc could not build window_id.swift: no captures")
                self.window_tool = False
            else:
                self.window_tool = tool
        if not self.window_tool:
            return None
        done = subprocess.run([str(self.window_tool), str(pid)], capture_output=True, text=True)
        text = done.stdout.strip()
        return int(text) if text.isdigit() else None

    def find_window(self):
        self.window = wait(self.window_id, 40)
        if self.window is None:
            self.note("the launched browser's window was not found: no captures")

    def shot(self, name, front=False, settle=1.0):
        """Capture the launched browser's first window, or with `front` the
        one of its windows that is in front now, `settle` seconds from now:
        the strip slides to a new layout in about an eighth of a second
        (osmena's STRIP_SLIDE_SECS), and a page repaints within a frame or
        two, so a second is long after both."""
        window = (self.window_id() if front else None) or self.window
        if not window:
            self.note(f"no window id: {name}.png not captured")
            return
        shots = self.out / ("shots-" + self.mode)
        shots.mkdir(parents=True, exist_ok=True)
        time.sleep(settle)  # let the frame with the new layout be painted
        path = shots / (name + ".png")
        done = subprocess.run(["screencapture", "-x", "-o", "-l", str(window), str(path)])
        ok = done.returncode == 0 and path.is_file() and path.stat().st_size > 0
        self.note(f"captured {path}" if ok else f"screencapture failed: {name}")

    # --- what the reports say -------------------------------------------

    def alive(self, page):
        return fresh(latest("panel", lambda d: d.get("page") == page))

    def view_closed(self, page):
        """The engine's log says the browser of the view last opened at
        `page` has closed (`on_before_close`)."""
        log = self.log_text()
        views = re.findall(r"side panel (TabId\(\d+\)): chrome-extension://[a-p]{32}/" + re.escape(page) + r"(?:[?#]\S*)?$",
                           log, re.MULTILINE)
        return bool(views) and f"tab side panel {views[-1]}: browser closed" in log

    def closed(self, page):
        """The page at `page` is closed: it reports no more (true of a hung
        page too) and the engine says its view's browser closed."""
        return not self.alive(page) and self.view_closed(page)

    def shown(self, page):
        record = latest("panel", lambda d: d.get("page") == page)
        return fresh(record) and record["data"].get("visibility") == "visible"

    def hidden(self, page):
        """The page reports, and says it is hidden."""
        record = latest("panel", lambda d: d.get("page") == page)
        return fresh(record) and record["data"].get("visibility") == "hidden"

    def panel(self, page):
        record = latest("panel", lambda d: d.get("page") == page)
        return record["data"] if fresh(record) else None

    def width(self, url, since=0.0):
        record = latest("tab", lambda d: d.get("url", "").startswith(url))
        return record["data"].get("width") if record and record["at"] >= since else None

    def narrower(self):
        width = self.width(self.active_url)
        if width is not None and self.full is not None and width < self.full - 100:
            return width
        return None

    # --- the records, in order -------------------------------------------

    def take(self, index):
        with lock:
            return list(records[index:])

    def pump(self, handlers, secs):
        """Answer the suite's records in the order they arrive, until its
        `done` or `secs` from now."""
        index = 0
        end = time.time() + secs
        while time.time() < end:
            if self.proc.poll() is not None:
                self.check("the suite finished", False, "the browser exited first")
                return False
            batch = self.take(index)
            index += len(batch)
            for record in batch:
                kind, data = record["kind"], record["data"]
                if kind == "tabs":
                    self.ext_id = data.get("id")
                    self.active_id = data.get("active")
                    self.active_url = data.get("activeUrl")
                    self.other_url = data.get("otherUrl")
                    self.note("the extension is " + str(self.ext_id))
                elif kind == "step":
                    self.check(data["name"], data["ok"], data.get("detail"))
                elif kind == "note":
                    self.note(str(data))
                elif kind == "contexts":
                    self.note("getContexts({}) as the worker got it: " + json.dumps(data))
                elif kind == "browser-facts":
                    self.browser_facts(data)
                elif kind == "grant":
                    self.grant(data)
                elif kind == "sidepanel-info":
                    self.note("chrome.sidePanel in %s: %s" % (data.get("context"), json.dumps(data.get("info"))))
                elif kind == "checkpoint":
                    handler = handlers.get(data["name"])
                    if handler is None:
                        self.check("checkpoint " + data["name"] + " is known", False)
                    else:
                        handler()
                    with lock:
                        released.add(data["name"])
                elif kind == "done":
                    return True
            time.sleep(0.2)
        self.check("the suite finished", False, "timed out")
        return False

    def browser_facts(self, data):
        """Judge what a context says of its `browser` global
        (panel-ext/browser-facts.js), once per context name. In a worker and
        in an extension page every namespace the bridge replaces on `chrome`
        must be the same object on `browser`, pinned, and a call through it
        answered by the bridge; a content script has none of them."""
        context = str(data.get("context"))
        if context in self.facts_seen:
            return
        self.facts_seen.add(context)
        facts = data.get("facts") or {}
        query = data.get("query")
        self.note("`browser` in %s: %s; tabs.query through each: %s"
                  % (context, json.dumps(facts, sort_keys=True), json.dumps(query)))
        spaces = facts.get("namespaces") or {}
        if context == "content script":
            has = sorted(name for name in REPLACED
                         if (spaces.get(name) or {}).get("chrome", "undefined") != "undefined"
                         or (spaces.get(name) or {}).get("browser", "undefined") != "undefined")
            self.check("a content script has none of the bridged namespaces, on chrome or on browser", not has, has)
            return
        self.check(f"`browser` exists in {context}", facts.get("browser") == "object", facts.get("browser"))
        there = [name for name in REPLACED if (spaces.get(name) or {}).get("chrome") == "object"]
        apart = [name for name in there if spaces[name].get("same") is not True]
        self.check(f"in {context} browser.<name> is chrome.<name> for every bridged namespace", bool(there) and not apart,
                   {"apart": apart, "present": there})
        # `commands` is replaced in place and left unpinned on both globals
        # (docs/upstream-issues.md, C19); every other one is pinned.
        loose = [name for name in there if name != "commands"
                 and ((spaces[name].get("onBrowser") or {}).get("kind") != "data"
                      or (spaces[name].get("onBrowser") or {}).get("configurable") is not False)]
        self.check(f"in {context} each of them but commands is pinned on browser as on chrome", bool(there) and not loose,
                   {name: spaces[name].get("onBrowser") for name in loose})
        if "commands" in there:
            both = [(spaces["commands"].get(side) or {}).get("configurable") for side in ("onChrome", "onBrowser")]
            self.check(f"in {context} commands is not pinned, on chrome or on browser", both == [True, True],
                       {side: spaces["commands"].get(side) for side in ("onChrome", "onBrowser")})
        runtime = facts.get("runtime") or {}
        members = ("getContexts", "onConnect", "onMessage", "sendMessage", "connect")
        unlike = [m for m in members if (runtime.get(m) or {}).get("same") is not True]
        self.check(f"in {context} browser.runtime's getContexts, onMessage and onConnect are chrome.runtime's",
                   not unlike and runtime.get("onConnectAddListener") is True, {"unlike": unlike, "runtime": runtime})
        ours = isinstance(query, dict) and isinstance(query.get("browser"), list) and len(query["browser"]) >= 2 \
            and query["browser"] == query.get("chrome") \
            and all(isinstance(t[1], int) and t[1] >= 0 and isinstance(t[2], int) and t[2] >= 1 for t in query["browser"])
        self.check(f"in {context} browser.tabs.query is answered by the bridge, with the strip's tabs", ours, query)

    def grant(self, data):
        """What chrome.permissions.request did in the live worker
        (panel-ext, grantCheck): docs/upstream-issues.md, C19."""
        self.note("permissions.request for cookies and storage in the live worker: " + json.dumps(data, sort_keys=True))
        refused = (data.get("fromWorker") or {}).get("error")
        self.check("permissions.request from the worker is refused for want of a user gesture",
                   "user gesture" in str(refused), data.get("fromWorker"))
        answer = (data.get("page") or {}).get("answer")
        answer = answer if isinstance(answer, dict) else {}
        self.check("from a page, in the handler of a click sent through chrome.debugger, it is granted",
                   answer.get("granted") is True and answer.get("trusted") is True
                   and (data.get("contains") or {}).get("value") is True,
                   {"page": data.get("page"), "contains": data.get("contains")})
        # `cookies` sorts after `commands`, which is not pinned, and before
        # `sidePanel`, the first pinned name of this extension.
        self.check("the granted chrome.cookies appears in the live worker and in the live page (commands does not cut the update)",
                   (data.get("before") or {}).get("cookies") == "undefined"
                   and (data.get("after") or {}).get("cookies") == "object"
                   and (answer.get("after") or {}).get("cookies") == "object",
                   {"before": data.get("before"), "after": data.get("after"), "page": answer.get("after")})
        # `storage` sorts after `sidePanel`: C19's cut, as documented.
        self.note("chrome.storage, granted with it and sorting after sidePanel, is %s in the live worker (C19: undefined until the worker starts again)"
                  % (data.get("after") or {}).get("storage"))

    def finish(self):
        """Wait for the browser to end by itself and judge the launch."""
        left = max(0, int(self.started + self.secs - time.time()))
        self.note(f"launch {self.launches}: the browser exits by itself in about {left} s")
        try:
            # run.sh serve kills a browser still up 120 s after its --exit-after.
            status = self.proc.wait(timeout=left + 120 + 60)
        except subprocess.TimeoutExpired:
            self.proc.terminate()  # run.sh's own cleanup ends the browser it started
            status = self.proc.wait()
            self.check(f"launch {self.launches}: run.sh serve ended in time", False)
        self.logfile.close()
        shutil.rmtree(self.control, ignore_errors=True)
        text = self.log_text()
        self.check(f"launch {self.launches}: the browser exited with status 0", status == 0, status)
        for marker in MARKERS:
            self.check(f"launch {self.launches}: its log has no '{marker}'", marker not in text,
                       [line for line in text.splitlines() if marker in line][:2])
        self.check(f"launch {self.launches}: run.sh serve printed no FAIL line", "FAIL serve:" not in text,
                   [line for line in text.splitlines() if "FAIL serve:" in line][:3])
        self.note(f"launch {self.launches}: Chromium's '{CHROMIUM_API_ERROR}' is in the log "
                  f"{text.count(CHROMIUM_API_ERROR)} time(s)")
        self.note(f"launch {self.launches}: 'nautilus-bridge:' error lines: {text.count('nautilus-bridge:')}")


# --- the panel suite's checkpoints ------------------------------------------

def panel_checks(h):
    handlers = {
        "start": lambda: start(h),
        "window-panel": lambda: window_panel(h),
        "tab-panel-hidden": lambda: tab_panel_hidden(h),
        "tab-panel-shown": lambda: tab_panel_shown(h),
        "window-panel-again": lambda: window_panel_again(h),
        "tab-panel-closed": lambda: tab_panel_closed(h),
        "window-closed-by-page": lambda: window_closed_by_page(h),
        "window-panel-reopened": lambda: h.check("the window panel is open again", wait(lambda: h.alive("panel.html"))),
        "click-open": lambda: click_open(h),
        "click-close": lambda: click_close(h),
        "extra-open": lambda: h.check("the window panel is open for the further checks", wait(lambda: h.shown("panel.html"))),
        "bystander": lambda: bystander(h),
        "bystander-page": lambda: bystander_page(h),
        "debugger-held": lambda: debugger_held(h),
        "debugger-done": lambda: debugger_done(h),
        "orphan-closed": lambda: orphan_closed(h),
        "orphan-asked": lambda: orphan_asked(h),
        "orphan-done": lambda: orphan_done(h),
        "path-changed": lambda: path_changed(h),
        "path-restored": lambda: h.check(
            "setOptions({path}) back shows the first page again",
            wait(lambda: h.shown("panel.html") and h.closed("panel-tab.html"), 10),
            [h.panel("panel.html"), h.panel("panel-tab.html"), h.view_closed("panel-tab.html")]),
        "panel-opened-tab": lambda: panel_opened_tab(h),
        "tab-closed-with-panel": lambda: h.check(
            "the closed tab's panel page is gone and the window's is still shown",
            wait(lambda: h.shown("panel.html") and h.closed("panel-tab.html"), 10),
            [h.panel("panel.html"), h.panel("panel-tab.html"), h.view_closed("panel-tab.html")]),
        "second-window": lambda: second_window(h),
        "second-window-closed": lambda: h.check(
            "with the second window closed the first window's panel is still shown",
            wait(lambda: h.shown("panel.html"), 10), h.panel("panel.html")),
        "closer": lambda: closer(h),
        "left-open": lambda: left_open(h),
    }
    h.launch(EXT, "https://example.com/ https://example.org/", h.args.budget, suite_name="panel")
    h.find_window()
    h.pump(handlers, h.args.budget - 10)
    frames_check(h)
    gone = latest("closer-gone")
    if gone is not None:
        asked = latest("closer")
        took = None
        if asked and gone["data"].get("at") and asked["data"].get("closing"):
            took = gone["data"]["at"] - asked["data"]["closing"]
        h.check("window.close() in an extension page that is no panel closes its tab", gone["data"].get("gone"), gone["data"])
        h.note(f"that close took {took} ms from the page's call to tabs.onRemoved in the worker")
    with lock:
        firsts = [r["data"] for r in records if r["kind"] == "sender" and isinstance(r["data"], dict) and not r["data"].get("late")]
    h.note("first messages of the run's panel pages, sent as each loaded: %d, of which with a sender.tab: %d"
           % (len(firsts), sum(1 for d in firsts if d.get("hasTab"))))
    log = h.log_text()
    h.check("the log has a 'side panel TabId(..): chrome-extension://' line for each open",
            len(re.findall(r"side panel TabId\(\d+\): chrome-extension://", log)) == 11,
            re.findall(r"side panel TabId\(\d+\): chrome-extension://\S+", log))
    clicks = re.findall(r"TEST HOOK: command \d+: click on \S+ for .*", log)
    h.check("both clicks of the control opened or closed the side panel",
            len(clicks) == 2 and all("opened or closed its side panel" in line for line in clicks), clicks)
    views = set(re.findall(r"side panel (TabId\(\d+\)): chrome-extension://", log))
    stray = [line for line in log.splitlines() if "no engine tab for" in line and any(view in line for view in views)]
    h.check("no input or command went to a panel's view after it closed", not stray, stray[:3])
    h.finish()


def frames_check(h):
    # C11 and C19: an about:blank or srcdoc frame of an extension page is an
    # extension context to Chromium (getContexts lists it), but it must not
    # hold the bridged namespaces: Chromium's tabs.group in such a frame ends
    # the browser process. The frames are read, never called.
    probe = latest("frames-probe", lambda d: d.get("page") == "panel.html")
    data = probe["data"] if probe else {}
    names = ["tabs", "tabGroups", "windows", "sidePanel", "commands", "debugger"]
    for frame in ("blank", "srcdoc"):
        seen = data.get(frame) or {}
        bare = seen.get("bridge") == "undefined" and all(
            (seen.get("namespaces") or {}).get(name) == "undefined" for name in names)
        h.check(f"an extension page's {frame} frame has no bridged namespace and no bridge",
                bare, seen)
    # Chromium refuses each bad argument shape by a synchronous TypeError; the
    # bridge's held sendMessage copies that (page_report.js, refusedShape).
    shapes = latest("send-shapes", lambda d: d.get("page") == "panel.html")
    answers = (shapes["data"]["shapes"] if shapes else {}) or {}
    h.check("Chromium refuses each bad sendMessage argument shape with a TypeError, at once",
            len(answers) == 7 and all(text.startswith("threw TypeError: Error in invocation of runtime.sendMessage(")
                                      and text.endswith("No matching signature.") for text in answers.values()),
            answers)
    parent = data.get("parent") or {}
    h.check("the extension page itself has the bridge and its bridged namespaces",
            parent.get("bridge") == "object" and parent.get("namespaces", {}).get("tabs") == "object",
            parent)


def start(h):
    since = time.time()
    h.full = wait(lambda: h.width(h.active_url, since), 30) if h.active_url else None
    h.check("the content script reports the tab's width", h.full is not None, h.active_url)
    h.note(f"the tab's page is {h.full} wide with no panel")
    h.shot("1-no-panel")


def window_panel(h):
    panel = wait(lambda: h.panel("panel.html"), 20)
    h.check("AC5.1 the default panel page exists", panel is not None)
    if panel is None:
        return
    shown = wait(lambda: h.shown("panel.html"), 10)
    h.check("the panel's page is visible", shown, h.panel("panel.html"))
    sized = wait(lambda: (h.panel("panel.html") or {}).get("width") == PANEL_WIDTH, 10)
    panel = h.panel("panel.html") or panel
    h.check(f"AC1.1 the panel's page is {PANEL_WIDTH} wide", sized, panel)
    narrow = wait(h.narrower, 10)
    h.check(
        "AC1.1 the tab's page is narrower by the panel",
        narrow is not None and h.full - narrow == PANEL_WIDTH,
        [h.full, narrow if narrow is not None else h.width(h.active_url)],
    )
    h.note(f"widths: the tab's page {h.full} -> {narrow}, the panel's page {panel.get('width')} x {panel.get('height')}")
    query = wait(lambda: latest("panel-query", lambda d: d.get("page") == "panel.html"), 10)
    data = query["data"] if query else {}
    h.check(
        "the panel's active tab of the current window is the window's",
        data.get("currentTabs") == [h.active_id],
        data,
    )
    h.check("tabs.getCurrent() in the panel is undefined", data.get("getCurrent") is True, data)
    h.check("getContexts({windowIds}) from the panel finds itself", data.get("windowContexts") == 1, data)
    # In Chrome a side panel's sender has no tab. Chromium here takes the
    # panel's browser for a tab of its own; the engine tells the worker that
    # tab id is a panel's, and the bridge takes sender.tab off. The page's
    # first message, sent as it loads, is held in the page until the engine
    # says the worker has been told (js/page_report.js), so it has no tab
    # either; so has the one it sends 2.5 s later.
    first = wait(lambda: latest("sender", lambda d: d.get("page") == "panel.html" and not d.get("late")), 10)
    tab = first["data"].get("tab") if first else "no message arrived"
    h.check("the panel page's first message, sent as it loads, reaches the worker with no sender.tab",
            first is not None and first["data"].get("hasTab") is False and first["data"].get("tab") is None,
            "no message arrived" if first is None else (tab if not isinstance(tab, dict) else
                                                        {key: tab.get(key) for key in ("id", "index", "windowId", "url")}))
    late = wait(lambda: latest("sender", lambda d: d.get("page") == "panel.html" and d.get("late")), 10)
    h.check("a later message from the panel's page reaches the worker with no sender.tab",
            late is not None and late["data"].get("hasTab") is False and late["data"].get("tab") is None,
            late["data"] if late else "no later message arrived")
    h.check("and still names the panel's page as its sender",
            late is not None and str(late["data"].get("url", "")).endswith("/panel.html"),
            late["data"] if late else None)
    since = time.time()
    script = wait(lambda: latest("tab", lambda d: d.get("url", "").startswith(h.active_url or "-")
                                 and isinstance(d.get("senderTab"), dict)), 10)
    seen = script["data"].get("senderTab") if script and script["at"] >= since - 10 else None
    h.check("a content script's message still has sender.tab, with the strip's fields",
            isinstance(seen, dict) and seen.get("id") == h.active_id
            and isinstance(seen.get("index"), int) and seen["index"] >= 0
            and isinstance(seen.get("windowId"), int) and seen["windowId"] >= 1,
            script["data"] if script else "no content script report with a sender")
    h.shot("2-window-panel")


def tab_panel_hidden(h):
    tab = wait(lambda: h.panel("panel-tab.html"), 20)
    h.check("AC5.1 the tab's panel page exists", tab is not None)
    if tab is None:
        return
    ok = wait(lambda: h.hidden("panel-tab.html") and h.shown("panel.html"), 10)
    h.check(
        "AC1.4 the other tab's panel is hidden while its tab is not active",
        ok,
        [h.panel("panel-tab.html"), h.panel("panel.html")],
    )


def tab_panel_shown(h):
    ok = wait(lambda: h.shown("panel-tab.html") and h.hidden("panel.html"), 10)
    h.check("AC1.4 with its tab active the tab's panel shows and the window's hides", ok,
            [h.panel("panel-tab.html"), h.panel("panel.html")])
    sized = wait(lambda: (h.panel("panel-tab.html") or {}).get("width") == PANEL_WIDTH, 10)
    h.check(f"AC1.4 the tab's panel is {PANEL_WIDTH} wide when shown", sized, h.panel("panel-tab.html"))
    h.shot("3-tab-panel")


def window_panel_again(h):
    ok = wait(lambda: h.shown("panel.html") and h.hidden("panel-tab.html"), 10)
    h.check("AC1.4 back on the first tab the window's panel shows again", ok,
            [h.panel("panel.html"), h.panel("panel-tab.html")])
    h.shot("4-window-panel-again")


def tab_panel_closed(h):
    h.check("AC5.2 setOptions({enabled: false}) closed the tab panel's page",
            wait(lambda: h.closed("panel-tab.html"), 10),
            [h.panel("panel-tab.html"), h.view_closed("panel-tab.html")])
    h.check("the window's panel is still open", h.alive("panel.html"))


def window_closed_by_page(h):
    h.check("AC5.2 window.close() in the panel closed its page", wait(lambda: h.closed("panel.html"), 10),
            [h.panel("panel.html"), h.view_closed("panel.html")])
    if h.full is None:
        h.check("the page has its full width again", False, "no width was reported before the panel")
    else:
        full = wait(lambda: h.width(h.active_url) == h.full, 10)
        h.check("the page has its full width again", full, [h.full, h.width(h.active_url)])
    h.shot("5-closed")


def click_open(h):
    h.send({"click": h.ext_id})
    h.check("a click on the action opens the window panel", wait(lambda: h.alive("panel.html"), 15))


def click_close(h):
    h.send({"click": h.ext_id})
    h.check("a second click closes it", wait(lambda: h.closed("panel.html"), 15),
            [h.panel("panel.html"), h.view_closed("panel.html")])


def bystander(h):
    """Install a second extension while the test extension's worker and its
    panel's page are alive: Chromium then defines every API on `chrome`
    again in each of them. It is the popup extension, whose popup's close is
    one of the steps by hand."""
    other = POPUP_EXT
    h.send({"load": str(other)})
    listed = wait(lambda: h.installed_id("Nautilus Popup Close Test"), 30)
    h.check("a second extension is installed while the panel is open", listed is not None)
    time.sleep(2)  # the renderers hear of it after the browser does


def bystander_page(h):
    said = wait(lambda: latest("sidepanel-info", lambda d: "after another" in str(d.get("context"))
                               and str(d.get("context")).startswith("panel.html")), 10)
    info = (said["data"].get("info") or {}) if said else {}
    h.check("after another extension is installed chrome.sidePanel is still the engine's in the panel's page",
            info.get("frozen") is True and info.get("property", {}).get("configurable") is False, info or None)


def debugger_lines(h):
    log = h.log_text()
    return (re.findall(r"bridge: TabId\(\d+\) \(Chromium tab \d+\) was hidden; shown for the debugger of " + re.escape(h.ext_id or "-"), log),
            re.findall(r"bridge: TabId\(\d+\) \(Chromium tab \d+\) hidden again: no debugger is left on it", log))


def debugger_held(h):
    """The panel's page holds a chrome.debugger session on the tab behind
    the active one (panel.js, debuggerCheck): the engine has shown that tab
    for it, and every command was answered at once."""
    said = wait(lambda: latest("debugger-check", lambda d: d.get("phase") == "attached"), 5)
    data = said["data"] if said else {}
    h.note("the panel page's debugger on the background tab: " + json.dumps(data))
    h.check("the page's bridge wraps chrome.debugger", "debugger.reports" in (data.get("installed") or []), data.get("installed"))
    commands = data.get("commands") or []
    sent = [c for c in commands if c.get("name") != "attach"]
    h.check("the panel page attached its debugger to the background tab",
            any(c.get("name") == "attach" and c.get("outcome") == "ok" for c in commands), commands)
    h.check("each of its DevTools commands was answered in under a second",
            len(sent) == 6 and all(c.get("outcome") == "ok" and c.get("ms", 9999) < 1000 for c in sent),
            [(c.get("name"), c.get("ms"), c.get("outcome")) for c in sent])
    h.note("command times in ms: " + json.dumps([c.get("ms") for c in sent])
           + "; attach: " + json.dumps([c.get("ms") for c in commands if c.get("name") == "attach"]))
    shown, hidden = debugger_lines(h)
    h.check("the log shows the background tab shown for the page's debugger, and not hidden again while it is held",
            len(shown) == 1 and not hidden, [shown, hidden])
    worker = data.get("worker")
    h.note("the worker on the same tab while the page holds the session: " + json.dumps(worker))
    h.check("the session is the extension's, not the page's: the worker commands the tab through it and cannot attach a second",
            isinstance(worker, dict) and worker.get("command") == "ok"
            and "already attached" in str(worker.get("attach")), worker)


def debugger_done(h):
    said = wait(lambda: latest("debugger-check", lambda d: d.get("phase") == "done"), 5)
    data = said["data"] if said else {}
    detach = [c for c in data.get("commands") or [] if c.get("name") == "detach"]
    h.check("the panel page detached its debugger", len(detach) == 1 and detach[0].get("outcome") == "ok" and "error" not in data,
            data.get("error") or detach)
    hidden = wait(lambda: len(debugger_lines(h)[1]) == 1, 5)
    h.check("after the detach the log shows the tab hidden again", hidden, debugger_lines(h))


def orphan_closed(h):
    """The panel whose page attached a debugger is closed under it: the page
    goes without a detach."""
    h.check("the panel whose page holds a debugger closed", wait(lambda: h.closed("panel.html"), 10),
            [h.panel("panel.html"), h.view_closed("panel.html")])
    time.sleep(1.5)  # the page's process is gone, and whatever follows from it


def orphan_asked(h):
    said = wait(lambda: latest("orphan-debugger"), 5)
    data = said["data"] if said else {}
    shown, hidden = debugger_lines(h)
    h.note("a debugger whose attaching page closed without a detach: " + json.dumps(data, sort_keys=True)
           + "; the log's shown lines: %d, hidden-again lines: %d" % (len(shown), len(hidden)))
    # Chromium's session is the extension's, not the page's: it outlives
    # the page that opened it, so the engine must not forget the debugger
    # (and hide the tab) because that page is gone.
    h.check("the debugger session outlives the page that attached it: the worker still commands the tab through it",
            data.get("attach") == "ok" and data.get("panelClosed") is True and data.get("command") == "ok"
            and data.get("attachedAfter") is True, data)
    h.check("and the engine keeps that tab shown for it",
            len(shown) == 2 and len(hidden) == 1, [shown, hidden])


def orphan_done(h):
    said = wait(lambda: latest("orphan-debugger", lambda d: "detach" in d), 5)
    data = said["data"] if said else {}
    h.note("after it: " + json.dumps(data, sort_keys=True))
    h.check("the worker's detach ends the session the page left, and the tab is hidden again",
            data.get("detach") == "ok" and wait(lambda: len(debugger_lines(h)[1]) == 2, 5), [data, debugger_lines(h)])
    h.check("the window panel is open again after it", wait(lambda: h.alive("panel.html"), 15))


def path_changed(h):
    ok = wait(lambda: h.shown("panel-tab.html") and h.closed("panel.html"), 10)
    h.check("setOptions({path}) on the open window panel shows the new page", ok,
            [h.panel("panel-tab.html"), h.panel("panel.html"), h.view_closed("panel.html")])
    h.shot("7-new-path")


def panel_opened_tab(h):
    said = wait(lambda: latest("panel-opened"), 10)
    h.check("the panel got no handle to what it opened", said is not None and said["data"].get("handle") == "null",
            said["data"] if said else None)
    h.check("the panel stays when it opens a tab", wait(lambda: h.alive("panel.html"), 10))
    h.shot("8-panel-opened-a-tab")


def second_window(h):
    told = latest("second-window")
    tab = told["data"].get("tab") if told else None
    asked = wait(lambda: latest("panel-query", lambda d: d.get("currentTabs") == [tab]), 15)
    h.check("the second window's panel page says its current window is the second window",
            asked is not None and asked["data"].get("windowContexts") == 1,
            asked["data"] if asked else [tab, (latest("panel-query") or {}).get("data")])
    h.shot("9-second-window", front=True)


def closer(h):
    asked = wait(lambda: latest("closer"), 15)
    h.check("the page that is no panel reported before it closed itself", asked is not None)
    if asked:
        h.note("window.close() in a page that is no panel: " + json.dumps(asked["data"]))
        asks = asked["data"].get("asks") or []
        h.check("sidePanel.closeSelf answers false for a page that is no panel",
                bool(asks) and all(a.get("value") is False for a in asks), asks)


def left_open(h):
    h.check("the window panel is left open for the steps by hand", wait(lambda: h.alive("panel.html"), 10))
    h.shot("6-left-open")


# --- the behaviour file, across two launches ----------------------------------

def behavior_checks(h):
    data = Path(tempfile.mkdtemp(prefix="nautilus-panel-data-"))
    try:
        h.launch(EXT, "https://example.com/ https://example.org/", 60, suite_name="behavior", data_dir=data, hold=0)
        h.find_window()

        def behavior_set():
            files = wait(lambda: list(data.rglob(BEHAVIOR_FILE)), 10) or []
            h.check(f"{BEHAVIOR_FILE} is written under the profile's engine directory", len(files) == 1,
                    [str(f) for f in files])
            for file in files:
                text = file.read_text()
                h.note(f"{file.relative_to(data)}: {text.strip()}")
                h.check("it names the extension", bool(h.ext_id) and h.ext_id in text, text)

        h.pump({"behavior-set": behavior_set}, 45)
        first_id = h.ext_id
        h.finish()
        kept = list(data.rglob(BEHAVIOR_FILE))
        h.check("the file is still there after the browser exited", len(kept) == 1, [str(f) for f in kept])

        # The second launch: nothing is loaded and the worker calls nothing.
        h.launch(None, "https://example.com/ https://example.org/", 60, suite_name=None, data_dir=data, load=False, hold=0)
        h.find_window()
        again = wait(lambda: h.installed_id("Nautilus Panel Test"), 40)
        h.check("the second launch lists the same extension, on", again is not None and again == first_id, [first_id, again])
        h.check("its content script reports again", wait(lambda: latest("tab"), 30) is not None)
        if again:
            h.send({"click": again})
            opened = wait(lambda: h.alive("panel.html"), 15)
            h.check("after a restart a click on the action still opens the panel", opened)
            h.shot("behavior-second-launch")
            clicks = re.findall(r"TEST HOOK: command \d+: click on \S+ for .*", h.log_text())
            h.check("the control says the click opened the side panel",
                    len(clicks) == 1 and "opened or closed its side panel" in clicks[0], clicks)
        h.finish()
    finally:
        shutil.rmtree(data, ignore_errors=True)


# --- tab groups and a command --------------------------------------------------

def groups_checks(h):
    def grouped():
        # SHOT_SERIES=1: the same window again and again with no input in
        # between, to tell a slide caught half way from a layout that stays.
        if os.environ.get("SHOT_SERIES"):
            began = time.time()
            for at in (0.2, 1.0, 3.0, 6.0):
                h.shot("1-grouped-at-%.1fs" % at, settle=max(0.0, began + at - time.time()))
        h.shot("1-grouped")
        h.send({"extension_command": {"extension": h.ext_id, "name": "probe-command"}})

    def ungrouped():
        h.shot("2-ungrouped")

    def bystander():
        # A second extension, installed while the worker is alive: Chromium
        # defines every API again in the worker's context.
        h.send({"load": str(POPUP_EXT)})
        listed = wait(lambda: h.installed_id("Nautilus Popup Close Test"), 30)
        h.check("a second extension is installed while the worker is alive", listed is not None)
        time.sleep(2)  # the renderers hear of it after the browser does

    handlers = {
        "grouped": grouped,
        "action-command": lambda: h.send({"extension_command": {"extension": h.ext_id, "name": "_execute_action"}}),
        "ungrouped": ungrouped,
        "joined": lambda: h.shot("3-joined"),
        "bystander": bystander,
        "command-again": lambda: h.send({"extension_command": {"extension": h.ext_id, "name": "probe-command"}}),
    }
    h.launch(EXT, "https://example.com/ https://example.org/", 150, suite_name="groups", hold=0)
    h.find_window()
    if h.pump(handlers, 140):
        h.check("the suite ran to its end", True)
    log = h.log_text()
    sent = re.findall(r"TEST HOOK: command \d+: extension_command .*", log)
    h.check("the control handed the declared command to the bridge, before and after the second extension",
            len(sent) == 3 and all('"probe-command"' in sent[at] and sent[at].endswith("handed to the bridge") for at in (0, 2)), sent)
    h.check("the control's action command was the click, sent to the bridge",
            len(sent) == 3 and '"_execute_action"' in sent[1] and sent[1].endswith("sent to the bridge"), sent)
    delivered = re.findall(r'extension \S+: command "probe-command" for TabId\(\d+\).*', log)
    h.check("the engine sent the command to a context that listens, both times",
            len(delivered) == 2 and not any("no context listens" in line for line in delivered), delivered)
    fallback = [line for line in log.splitlines() if "cannot replace the members of commands" in line]
    h.check("Chromium's commands object let its members be replaced (no fallback to the pin)", not fallback, fallback[:2])
    chords = re.findall(r'extension \S+: command "probe-command" on (\S+)', log)
    h.note("the chord the shell is told for the command: " + json.dumps(sorted(set(chords))))
    h.finish()


# --- the two samples ----------------------------------------------------------

def prepare_sample(source, dest):
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(source, dest, symlinks=False)
    page = dest / "sidepanel.html"
    if not page.is_file():
        raise SystemExit(f"{page} is missing: the sample has no sidepanel.html")
    shutil.copy(EXT / "panel-probe.js", dest / "panel-probe.js")
    page.write_text(page.read_text() + '\n<script src="panel-probe.js"></script>\n')


def sample_checks(h, source):
    dest = h.out / h.mode
    prepare_sample(source, dest)
    h.launch(dest, SAMPLE_URLS[h.mode], 90, hold=h.args.hold)
    h.find_window()
    name = SAMPLE_NAMES[h.mode]
    ext_id = wait(lambda: h.installed_id(name), 60)
    h.check("the sample is installed and bridged", ext_id is not None and "bridged: Yes" in h.log_text(), name)
    if ext_id is None:
        h.finish()
        return
    if h.mode == "sample-site":
        urls = wait(lambda: re.findall(r"https://www\.google\.com/\S*", h.log_text()), 60)
        h.check("a tab on www.google.com loaded", bool(urls))
    time.sleep(8)  # the sample's worker sets its behaviour, and its options after the load
    h.shot("0-before")
    h.send({"click": ext_id})
    opened = wait(lambda: h.alive("sidepanel.html"), 20)
    h.check("a click on the action opens the sample's panel", opened)
    if opened:
        wait(lambda: h.shown("sidepanel.html"), 10)
        h.note("the sample's panel page: " + json.dumps(h.panel("sidepanel.html")))
    h.shot("1-open")
    h.send({"click": ext_id})
    closed = wait(lambda: h.closed("sidepanel.html"), 15)
    h.check("a second click closes the sample's panel", closed,
            [h.panel("sidepanel.html"), h.view_closed("sidepanel.html")])
    h.send({"click": ext_id})
    h.check("a third click opens it again", wait(lambda: h.alive("sidepanel.html"), 20))
    clicks = re.findall(r"TEST HOOK: command \d+: click on \S+ for .*", h.log_text())
    h.note("the control's click lines: " + json.dumps(clicks))
    h.finish()
    if h.mode != "sample-site":
        return
    # Again, with the tab that is not on the site showing.
    h.launch(dest, SAMPLE_SITE_ELSEWHERE, 60, hold=0)
    h.find_window()
    ext_id = wait(lambda: h.installed_id(name), 60)
    h.check("the sample is installed again", ext_id is not None)
    if ext_id is not None:
        time.sleep(12)
        h.send({"click": ext_id})
        line = wait(lambda: re.findall(r"TEST HOOK: command \d+: click on \S+ for .*", h.log_text()), 15) or []
        h.check("on the tab that is not on the site a click is an ordinary action click",
                len(line) == 1 and "sent to the bridge" in line[0], line)
        time.sleep(4)
        made = re.findall(r"side panel TabId\(\d+\): chrome-extension://\S+/sidepanel\.html", h.log_text())
        h.check("and no panel opens", not h.alive("sidepanel.html") and not made, [h.panel("sidepanel.html"), made])
        h.shot("2-elsewhere")
    h.finish()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["panel", "behavior", "groups", "sample-global", "sample-site"])
    parser.add_argument("source", nargs="?", help="the cookbook directory (sample modes)")
    parser.add_argument("--hold", type=int, default=int(os.environ.get("HOLD", "180")),
                        help="seconds the browser stays open after the checks' time, for the steps by hand")
    parser.add_argument("--budget", type=int, default=int(os.environ.get("BUDGET", "170")),
                        help="seconds the panel suite may take")
    parser.add_argument("--out", default=str(HERE / "out"))
    args = parser.parse_args()
    if args.mode.startswith("sample-") and not args.source:
        parser.error("the sample modes need the cookbook directory")
    if not APP.exists():
        sys.exit(f"{APP} is missing: cargo xtask bundle-mac --features test-hooks")
    Path(args.out).mkdir(parents=True, exist_ok=True)

    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    h = Harness(args.mode, args)
    try:
        if args.mode == "panel":
            panel_checks(h)
        elif args.mode == "behavior":
            behavior_checks(h)
        elif args.mode == "groups":
            groups_checks(h)
        else:
            sample_checks(h, Path(args.source))
    finally:
        # A launch still running here means this script is ending early (an
        # error, or an interrupt): run.sh serve's own cleanup ends the
        # browser it started, by pid.
        if h.proc is not None and h.proc.poll() is None:
            h.proc.terminate()
            h.proc.wait()
        if h.control is not None:
            shutil.rmtree(h.control, ignore_errors=True)
        server.shutdown()
    print("FAILED: %d check(s)" % h.failed if h.failed else "ALL PASSED", flush=True)
    sys.exit(1 if h.failed else 0)


if __name__ == "__main__":
    main()
