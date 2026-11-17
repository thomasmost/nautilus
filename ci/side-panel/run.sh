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
# Needs the bundle (cargo xtask bundle-mac). Prints the driver's PASS and
# FAIL lines; exits with the driver's status: 0 only when every check
# passed, non-zero when one failed, the driver died, or the run was
# interrupted (INT, TERM, HUP).
#
# Cleanup runs on every exit, including the failing and interrupted ones:
# the browser this script started is killed if it is still up, and the
# throwaway profile, the control directory and the work directory are
# removed. Set SIDE_PANEL_LOGS=<directory> to keep the logs first: the work
# directory is copied there before it is removed.
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

cleanup() {
  code=$?
  trap - EXIT
  # Only processes this script started, by pid.
  for pid in "$finder" "$app_pid" "$driver"; do
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
mkdir -p "$work/ctl-1" "$work/ctl-2"

python3 "$root/ci/side-panel/driver.py" "$mode" "$work" "$root/ci/side-panel/ext" > "$work/driver.log" 2>&1 &
driver=$!

# launch <n> <seconds>: one run of the browser on two tabs, with Finder
# brought to the front six seconds in so that its window is not focused.
launch() {
  touch "$work/run-$1.started"
  ( sleep 6; open -a Finder ) &
  finder=$!
  status=0
  RUST_LOG="info,nautilus_engine=debug" NAUTILUS_PUMP_LOG=1 NAUTILUS_TEST_CONTROL="$work/ctl-$1" \
    "$app" --profile "$profile" --exit-after "$2" https://example.com/ https://example.org/ \
    > "$work/run-$1.log" 2>&1 &
  app_pid=$!
  wait "$app_pid" || status=$?
  app_pid=""
  kill "$finder" 2>/dev/null || true
  finder=""
  [ "$status" -eq 0 ] || echo "run $1 exited with status $status"
}

case "$mode" in
  spike) launch 1 45; launch 2 "${SPIKE_SECONDS:-300}" ;;
  pump)  launch 1 "${PUMP_SECONDS:-160}" ;;
esac

status=0
wait "$driver" || status=$?
driver=""
cat "$work/driver.log"
exit "$status"
