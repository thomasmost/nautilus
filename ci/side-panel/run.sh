#!/bin/sh
# The scripted run of the extension bridge (macOS).
#
# usage: ci/side-panel/run.sh [spike|pump|reload|zombie|broken|stop|idle]
#        CONTROL=<dir> ci/side-panel/run.sh serve
#
# Needs a bundle with the test hooks, which no packaged build has:
#   cargo xtask bundle-mac --features test-hooks
#
#   spike  two launches on one throwaway profile: install the test
#          extension through the bridge's rewrite, then restart and
#          check the bridge, a click on a stopped worker, tabs.query
#          and the latency of extension calls with the window unfocused.
#   pump   one launch: only the latency run, started with the worker.
#   reload one launch: the worker debugs a background tab, reloads the
#          extension; the tab must end hidden only if no session survives.
#   zombie as reload, with the extension list held during the reload:
#          the worker-target detector must be the path that sees it.
#   broken one launch: the relay page is damaged; the give-up keeps a
#          debugged tab shown.
#   stop   one launch: the worker is stopped over DevTools with a debugger held.
#   idle   one launch: the worker debugs a background tab and idles; the engine
#          must not start it again for 90 s after Chromium stops it.
#   serve  one launch with no driver and no DevTools port, for a checker that
#          has its own server and starts this script itself (the side panel's
#          checks). The browser's output goes to this script's stdout (its
#          stderr with it) and the script ends when the browser does, so the
#          caller waits for it. It reads:
#            CONTROL  the command directory the browser reads
#                     (NAUTILUS_TEST_CONTROL). Required. The caller makes it,
#                     empty and mode 0700 (the engine refuses any other), and
#                     writes cmd-2.json onward, each through a .tmp name and a
#                     rename. This script writes cmd-1.json, {"load": EXT_DIR},
#                     the same way; it makes the directory if it is missing.
#            EXT_DIR  the unpacked extension to load, bridged (copied into the
#                     profile and rewritten). Default: ci/side-panel/ext.
#            PROFILE  the throwaway profile's name. Default: side-panel-serve.
#                     It lives in a scratch data directory (--data-dir) that
#                     this script makes for the launch and removes after it.
#            SECS     --exit-after seconds. Default: 600.
#            URLS     the tabs to open, space separated. Default:
#                     https://example.com/ https://example.org/
#          The launch is judged as every launch is (next paragraph): exit 0
#          only when the browser exited 0 by itself, in time, with no crash
#          or leak in its log; otherwise the browser's own status, or 1,
#          after a "FAIL serve:" line on stderr. Exit 2: the mode could not
#          start. The scratch data directory is removed on every exit; the
#          control directory is the caller's and is left.
#
# Each launch is judged by the browser, not only by the driver. A launch
# fails when the browser exits with a non-zero status (a signal included:
# 128 plus its number), when it is still up 120 s after its --exit-after
# (the script then kills it, and the launch fails as "still up"), or
# when its log names a crash or a leak: a Rust panic, a fatal runtime error,
# a RefCell borrowed twice ("already borrowed" or "already mutably borrowed"),
# or browsers still open at shutdown. The browser is expected to end by itself
# at --exit-after; the script never kills a browser that is in time. A failed
# launch ends the driver and skips the launches after it.
#
# Every mode but serve prints the driver's PASS and FAIL lines, then a RESULT
# line that names the driver's and the launches' verdicts. Exits 0 only when
# the driver passed and every launch passed; non-zero otherwise, including
# when the driver died or the run was interrupted (INT, TERM, HUP).
#
# Cleanup runs on every exit, including the failing and interrupted ones:
# any browser or watchdog this script started is killed if it is still up,
# and the throwaway profile, the control directory and the work directory are
# removed. Set SIDE_PANEL_LOGS=<directory> to keep the logs first: the work
# directory is copied there before it is removed (the driver's log, each
# launch's log as run-<n>.log, and the control directories).
set -eu

# launched_still_up <pid> <path>: true when <pid> is up, is not a zombie, and
# its command line starts with <path>, the browser this script launched. A pid
# that has ended, or was reused by another process, is not ours to kill.
launched_still_up() {
  state=$(ps -o stat= -p "$1" 2>/dev/null) || return 1
  case "$state" in
    ''|Z*) return 1 ;;
  esac
  cmdline=$(ps -o command= -p "$1" 2>/dev/null) || return 1
  case "$cmdline" in
    "$2"*) return 0 ;;
    *) return 1 ;;
  esac
}

if [ "${1:-}" = serve ]; then
  root=$(cd "$(dirname "$0")/../.." && pwd)
  app="$root/target/release/Nautilus.app/Contents/MacOS/Nautilus"
  [ -x "$app" ] || { echo "no bundle: run cargo xtask bundle-mac --features test-hooks first" >&2; exit 2; }
  [ -n "${CONTROL:-}" ] || { echo "serve: CONTROL names the command directory of the scripted run" >&2; exit 2; }
  ext=${EXT_DIR:-$root/ci/side-panel/ext}
  profile=${PROFILE:-side-panel-serve}
  secs=${SECS:-600}
  urls=${URLS:-https://example.com/ https://example.org/}
  case "$secs" in
    ''|*[!0-9]*) echo "serve: SECS is not a number of seconds: $secs" >&2; exit 2 ;;
  esac
  [ -f "$ext/manifest.json" ] || { echo "serve: no extension at $ext (EXT_DIR)" >&2; exit 2; }
  ext=$(cd "$ext" && pwd)
  # The engine refuses a control directory that another user owns or that
  # group or others can write, with a warning in its log and the hook off:
  # the extension would never load. Said here instead.
  [ -e "$CONTROL" ] || mkdir -p -m 700 "$CONTROL"
  [ -d "$CONTROL" ] && [ -O "$CONTROL" ] && [ "$(stat -f %Lp "$CONTROL")" = 700 ] \
    || { echo "serve: $CONTROL must be a directory of this user's with mode 0700" >&2; exit 2; }
  [ ! -e "$CONTROL/cmd-1.json" ] \
    || { echo "serve: $CONTROL already holds cmd-1.json; the caller starts with it empty" >&2; exit 2; }

  # Set before the trap so that cleanup can read them whatever happened.
  work=""
  app_pid=""
  watchdog=""
  logger=""
  serve_cleanup() {
    code=$?
    trap - EXIT
    # Only processes this script started, by pid; the browser only while the
    # pid is still the browser it launched.
    if [ -n "$app_pid" ] && launched_still_up "$app_pid" "$app"; then
      kill "$app_pid" 2>/dev/null || true
    fi
    for pid in "$watchdog" "$logger"; do
      [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
    done
    [ -z "$work" ] || rm -rf "$work"
    exit "$code"
  }
  trap serve_cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP

  work=$(mktemp -d "${TMPDIR:-/tmp}/nautilus-side-panel-serve.XXXXXX")
  # A data directory of its own (--data-dir), as the other modes have and for
  # the same reason: without it the launch is handed to the Nautilus the user
  # has open, or starts their default profile's Chromium under the test.
  data="$work/data"

  # The first command, written whole: the engine never reads half of one.
  python3 -I -c 'import json, sys; sys.stdout.write(json.dumps({"load": sys.argv[1]}))' "$ext" \
    > "$CONTROL/cmd-1.json.tmp"
  mv "$CONTROL/cmd-1.json.tmp" "$CONTROL/cmd-1.json"

  # The browser's output goes to the caller and to a copy this script reads
  # afterwards, through a named pipe: a pipeline would hide the browser's exit
  # status.
  mkfifo "$work/out"
  tee "$work/serve.log" < "$work/out" &
  logger=$!
  allow=120   # how long the launch may stay up after its --exit-after
  # URLS is a space-separated list of addresses: no quoting on purpose.
  # shellcheck disable=SC2086
  RUST_LOG="info,nautilus_engine=debug" NAUTILUS_TEST_CONTROL="$CONTROL" \
    "$app" --data-dir "$data" --profile "$profile" --exit-after "$secs" $urls \
    > "$work/out" 2>&1 &
  app_pid=$!
  # The watchdog, as launch's below: only a browser still up at the end of
  # the grace is killed, only while the pid is still that browser, and the
  # marker is written only when the kill itself succeeds.
  (
    sleep $(( secs + allow )) & s=$!
    trap 'kill "$s" 2>/dev/null; exit 0' TERM
    wait "$s" || exit 0
    if launched_still_up "$app_pid" "$app" && kill -KILL "$app_pid" 2>/dev/null; then
      touch "$work/serve.hung"
    fi
  ) &
  watchdog=$!
  status=0
  wait "$app_pid" || status=$?
  app_pid=""
  kill "$watchdog" 2>/dev/null || true
  wait "$watchdog" 2>/dev/null || true
  watchdog=""
  # The copy ends when the pipe's last writer does. A helper process that
  # outlived the browser may still hold it: wait ten seconds, then go on.
  n=0
  while kill -0 "$logger" 2>/dev/null && [ "$n" -lt 50 ]; do
    sleep 0.2
    n=$(( n + 1 ))
  done
  kill "$logger" 2>/dev/null || true
  wait "$logger" 2>/dev/null || true
  logger=""

  failed=""
  if [ -e "$work/serve.hung" ]; then
    echo "FAIL serve: the browser was still up $allow s after its --exit-after $secs; the script killed it" >&2
    failed=1
  elif [ "$status" -ne 0 ]; then
    echo "FAIL serve: the browser exited with status $status (128 plus a signal number for a signal)" >&2
    failed=1
  fi
  for marker in "panicked at" "fatal runtime error" "already borrowed" "already mutably borrowed" "browser(s) still open at shutdown"; do
    if grep -qF "$marker" "$work/serve.log"; then
      echo "FAIL serve: the browser's log has '$marker'" >&2
      failed=1
    fi
  done
  if [ -n "$failed" ] && [ "$status" -eq 0 ]; then
    status=1
  fi
  exit "$status"
fi

mode=${1:-spike}
root=$(cd "$(dirname "$0")/../.." && pwd)
app="$root/target/release/Nautilus.app/Contents/MacOS/Nautilus"
[ -x "$app" ] || { echo "no bundle: run cargo xtask bundle-mac --features test-hooks first" >&2; exit 2; }
case "$mode" in
  spike|pump|reload|zombie|broken|stop|idle) ;;
  *) echo "usage: $0 [spike|pump|reload|zombie|broken|stop|idle], or CONTROL=<dir> $0 serve" >&2; exit 2 ;;
esac

# Set before the trap so that cleanup can read them whatever happened.
work=""
profile=side-panel-test
driver=""
finder=""
app_pid=""
watchdog=""
failed=""     # set to 1 by any failed launch

cleanup() {
  code=$?
  trap - EXIT
  # Only processes this script started, by pid.
  for pid in "$finder" "$app_pid" "$watchdog" "$driver"; do
    [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
  done
  if [ -n "$work" ]; then
    if [ -n "${SIDE_PANEL_LOGS:-}" ]; then
      mkdir -p "$SIDE_PANEL_LOGS" && cp -R "$work/." "$SIDE_PANEL_LOGS/" || true
      echo "logs: $SIDE_PANEL_LOGS"
    fi
    rm -rf "$work"
  fi
  exit "$code"
}
# A signal exits through the EXIT trap, so cleanup runs for it too.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

work=$(mktemp -d "${TMPDIR:-/tmp}/nautilus-side-panel.XXXXXX")
# A data directory of its own (--data-dir): one browser process runs every
# profile of a data directory, so a launch on the user's own would be handed
# to the browser they have open, or would start their default profile's
# Chromium under the test.
data="$work/data"
profile_dir="$data/Nautilus/profiles/$profile"
# Control directories are private: the engine refuses one that another user
# owns or that group or others can write (NAUTILUS_TEST_CONTROL).
mkdir -p -m 700 "$work/ctl-1" "$work/ctl-2"

python3 "$root/ci/side-panel/driver.py" "$mode" "$work" "$root/ci/side-panel/ext" "$profile_dir" > "$work/driver.log" 2>&1 &
driver=$!

# launch <n> <seconds>: one run of the browser on two tabs, with Finder
# brought to the front six seconds in so that its window is not focused.
launch() {
  n=$1
  allow=120   # how long a launch may stay up after its --exit-after
  grace=$(( $2 + allow ))
  touch "$work/run-$n.started"
  ( sleep 6; open -a Finder ) &
  finder=$!
  RUST_LOG="info,nautilus_engine=debug" NAUTILUS_PUMP_LOG=1 NAUTILUS_TEST_CONTROL="$work/ctl-$n" \
    "$app" --data-dir "$data" --profile "$profile" --exit-after "$2" https://example.com/ https://example.org/ \
    > "$work/run-$n.log" 2>&1 &
  app_pid=$!
  # The watchdog: only a browser still up at $grace seconds is killed. The
  # browser may end by itself as the sleep does, so before the kill it must
  # still be this launch (launched_still_up), and the marker file is written
  # only when the kill itself succeeds: the parent reads "hung" from that
  # marker alone, never from the watchdog's intent. TERM (from cleanup or from
  # the wait below) stops it, and its sleep with it.
  (
    sleep "$grace" & s=$!
    trap 'kill "$s" 2>/dev/null; exit 0' TERM
    wait "$s" || exit 0
    if launched_still_up "$app_pid" "$app" && kill -KILL "$app_pid" 2>/dev/null; then
      touch "$work/run-$n.hung"
    fi
  ) &
  watchdog=$!
  status=0
  wait "$app_pid" || status=$?
  app_pid=""
  kill "$watchdog" 2>/dev/null || true
  wait "$watchdog" 2>/dev/null || true
  watchdog=""
  kill "$finder" 2>/dev/null || true
  finder=""

  if [ -e "$work/run-$n.hung" ]; then
    echo "FAIL launch $n: the browser was still up $allow s after its --exit-after $2; the script killed it"
    failed=1
  elif [ "$status" -ne 0 ]; then
    echo "FAIL launch $n: the browser exited with status $status (128 plus a signal number for a signal)"
    failed=1
  fi
  # Rust's RefCell says "already borrowed" or "already mutably borrowed",
  # depending on which borrow was taken; both are the same defect.
  for marker in "panicked at" "fatal runtime error" "already borrowed" "already mutably borrowed" "browser(s) still open at shutdown"; do
    if grep -qF "$marker" "$work/run-$n.log"; then
      echo "FAIL launch $n: its log has '$marker' (run-$n.log)"
      failed=1
    fi
  done
  if [ -n "$failed" ]; then
    # Nothing later can be judged: end the driver rather than wait out its
    # timeouts.
    [ -z "$driver" ] || kill "$driver" 2>/dev/null || true
  fi
  return 0
}

case "$mode" in
  spike)
    launch 1 45
    if [ -z "$failed" ]; then launch 2 "${SPIKE_SECONDS:-300}"; fi
    ;;
  pump)
    launch 1 "${PUMP_SECONDS:-160}"
    ;;
  reload)
    launch 1 "${RELOAD_SECONDS:-75}"
    ;;
  zombie)
    launch 1 "${ZOMBIE_SECONDS:-100}"
    ;;
  broken)
    launch 1 "${BROKEN_SECONDS:-110}"
    ;;
  stop)
    launch 1 "${STOP_SECONDS:-100}"
    ;;
  idle)
    launch 1 "${IDLE_SECONDS:-240}"
    ;;
esac

status=0
wait "$driver" || status=$?
driver=""
cat "$work/driver.log"
if [ -n "$failed" ]; then
  echo "RESULT run failed: a browser launch failed (the FAIL lines above); driver status $status"
  exit 1
fi
if [ "$status" -eq 0 ]; then
  echo "RESULT run passed: the driver passed and every launch exited by itself with no crash in its log"
else
  echo "RESULT run failed: the driver exited with status $status"
fi
exit "$status"
