#!/bin/sh
# The scripted run of the extension bridge (macOS).
#
# usage: ci/side-panel/run.sh [spike|pump]
#   spike  two launches on one throwaway profile: install the test
#          extension through the bridge's rewrite, then restart and
#          check the bridge, a click on a stopped worker, tabs.query
#          and the latency of extension calls with the window unfocused.
#   pump   one launch: only the latency run, started with the worker.
#
# Each launch is judged by the browser, not only by the driver. A launch
# fails when the browser exits with a non-zero status (a signal included:
# 128 plus its number), when it is still up 120 s after its --exit-after
# (the script then kills it, and the launch fails as "did not exit"), or
# when its log names a crash or a leak: a Rust panic, a fatal runtime error,
# a RefCell borrowed twice, or browsers still open at shutdown. The browser
# is expected to end by itself at --exit-after; the script never kills a
# browser that is in time. A failed launch ends the driver and skips the
# launches after it.
#
# Prints the driver's PASS and FAIL lines, then a RESULT line that names the
# driver's and the launches' verdicts. Exits 0 only when the driver passed
# and every launch passed; non-zero otherwise, including when the driver
# died or the run was interrupted (INT, TERM, HUP).
#
# Cleanup runs on every exit, including the failing and interrupted ones:
# any browser or watchdog this script started is killed if it is still up,
# and the throwaway profile, the control directory and the work directory are
# removed. Set SIDE_PANEL_LOGS=<directory> to keep the logs first: the work
# directory is copied there before it is removed (the driver's log, each
# launch's log as run-<n>.log, and the control directories).
set -eu
mode=${1:-spike}
root=$(cd "$(dirname "$0")/../.." && pwd)
app="$root/target/release/Nautilus.app/Contents/MacOS/Nautilus"
[ -x "$app" ] || { echo "no bundle: run cargo xtask bundle-mac first" >&2; exit 2; }
case "$mode" in
  spike|pump) ;;
  *) echo "usage: $0 [spike|pump]" >&2; exit 2 ;;
esac

# Set before the trap so that cleanup can read them whatever happened.
work=""
profile=side-panel-test
profile_dir="$HOME/Library/Application Support/Nautilus/profiles/$profile"
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
  rm -rf "$profile_dir"
  exit "$code"
}
# A signal exits through the EXIT trap, so cleanup runs for it too.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

work=$(mktemp -d "${TMPDIR:-/tmp}/nautilus-side-panel.XXXXXX")
rm -rf "$profile_dir"
# Control directories are private: the engine refuses one that another user
# owns or that group or others can write (NAUTILUS_TEST_CONTROL).
mkdir -p -m 700 "$work/ctl-1" "$work/ctl-2"

python3 "$root/ci/side-panel/driver.py" "$mode" "$work" "$root/ci/side-panel/ext" > "$work/driver.log" 2>&1 &
driver=$!

# launch <n> <seconds>: one run of the browser on two tabs, with Finder
# brought to the front six seconds in so that its window is not focused.
launch() {
  n=$1
  grace=$(( $2 + 120 ))
  touch "$work/run-$n.started"
  ( sleep 6; open -a Finder ) &
  finder=$!
  RUST_LOG="info,nautilus_engine=debug" NAUTILUS_PUMP_LOG=1 NAUTILUS_TEST_CONTROL="$work/ctl-$n" \
    "$app" --profile "$profile" --exit-after "$2" https://example.com/ https://example.org/ \
    > "$work/run-$n.log" 2>&1 &
  app_pid=$!
  # The watchdog: only a browser still up at $grace seconds is killed. Its
  # marker file says so, which is how a kill by this script differs from the
  # browser's own exit. TERM (from cleanup or from the wait below) stops it,
  # and its sleep with it.
  (
    sleep "$grace" & s=$!
    trap 'kill "$s" 2>/dev/null; exit 0' TERM
    wait "$s" || exit 0
    touch "$work/run-$n.hung"
    kill -KILL "$app_pid" 2>/dev/null || true
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
    echo "FAIL launch $n: the browser was still up $grace s after its --exit-after $2; the script killed it"
    failed=1
  elif [ "$status" -ne 0 ]; then
    echo "FAIL launch $n: the browser exited with status $status (128 plus a signal number for a signal)"
    failed=1
  fi
  for marker in "panicked at" "fatal runtime error" "already borrowed" "browser(s) still open at shutdown"; do
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
