#!/bin/sh
# Side panel checks in the real window (macOS, from the release bundle built
# with the test hooks: cargo xtask bundle-mac --features test-hooks).
# usage: ci/side-panel/panel.sh panel
#        ci/side-panel/panel.sh behavior
#        ci/side-panel/panel.sh groups
#        ci/side-panel/panel.sh sample-global <cookbook dir>
#        ci/side-panel/panel.sh sample-site <cookbook dir>
# panel_checks.py starts Nautilus through run.sh serve (a scratch data
# directory, no remote debugging port, no DevTools), reads what the test
# extension reports to it, and exits with the checks' status: judge the run
# by that, never through a pipe. HOLD (default 180) is how long the browser
# stays open after the checks' time, for the steps by hand; BUDGET (default
# 150) is the panel suite's time. Output: ci/side-panel/out/.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$here/out"
exec python3 -I "$here/panel_checks.py" "$@"
