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
# FAIL lines and where the logs are; exits with the driver's status.
set -eu
mode=${1:-spike}
root=$(cd "$(dirname "$0")/../.." && pwd)
app="$root/target/release/Nautilus.app/Contents/MacOS/Nautilus"
[ -x "$app" ] || { echo "no bundle: run cargo xtask bundle-mac first" >&2; exit 2; }
work=$(mktemp -d "${TMPDIR:-/tmp}/nautilus-side-panel.XXXXXX")
profile=side-panel-test
profile_dir="$HOME/Library/Application Support/Nautilus/profiles/$profile"
rm -rf "$profile_dir"
mkdir -p "$work/ctl-1" "$work/ctl-2"

python3 "$root/ci/side-panel/driver.py" "$mode" "$work" "$root/ci/side-panel/ext" > "$work/driver.log" 2>&1 &
driver=$!

# launch <n> <seconds>: one run of the browser on two tabs, with Finder
# brought to the front six seconds in so that its window is not focused.
launch() {
  touch "$work/run-$1.started"
  ( sleep 6; open -a Finder ) &
  RUST_LOG="info,nautilus_engine=debug" NAUTILUS_PUMP_LOG=1 NAUTILUS_TEST_CONTROL="$work/ctl-$1" \
    "$app" --profile "$profile" --exit-after "$2" https://example.com/ https://example.org/ \
    > "$work/run-$1.log" 2>&1 || echo "run $1 exited with status $?"
}

case "$mode" in
  spike) launch 1 45; launch 2 "${SPIKE_SECONDS:-300}" ;;
  pump)  launch 1 "${PUMP_SECONDS:-160}" ;;
  *) echo "usage: $0 [spike|pump]" >&2; exit 2 ;;
esac

status=0
wait "$driver" || status=$?
cat "$work/driver.log"
rm -rf "$profile_dir"
echo "logs: $work"
exit $status
