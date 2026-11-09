#!/usr/bin/env bash
# End to end: `jot restart --all` (the palette's Restart jot) closes every
# window and opens it again, as the same window: its workspaces, its split,
# its terminals (the same shells), on a display of its own (Xvfb).
#
#   tests/e2e/reopen.sh           (after cargo build --release)
#   JOT=path/to/jot DISP=:78 tests/e2e/reopen.sh
#
# Needs Xvfb and xdotool. Touches nothing outside its own folder.

set -u
here=$(cd "$(dirname "$0")/../.." && pwd)
JOT=${JOT:-$here/target/release/jot}
DISP=${DISP:-:78}
T=$(mktemp -d /tmp/jot-e2e-XXXX)
R=$T/r
mkdir -p -m700 "$R" "$T/s/jot" "$T/cfg"

pids=()
fail=0
cleanup() {
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done
    sleep 0.5
    XDG_RUNTIME_DIR=$R XDG_STATE_HOME=$T/s "$JOT" kill >/dev/null 2>&1
    [ $fail = 0 ] && [ -z "${KEEP:-}" ] && rm -rf "$T"
}
trap cleanup EXIT

export DISPLAY=$DISP SHELL=/bin/bash XDG_CONFIG_HOME=$T/cfg XDG_RUNTIME_DIR=$R XDG_STATE_HOME=$T/s
j() { "$JOT" "$@"; }
check() {
    if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}
until_() {
    for _ in $(seq 100); do eval "$1" && return 0; sleep 0.1; done
    return 1
}
key() { xdotool windowfocus --sync "$1" key "$2"; sleep 0.8; }
# Each window as jot status -v says (its name, what it shows), and each
# terminal (its number and shell): not the connections, which change.
win_part() { j status -v | sed -n '/^-- windows/,/^-- terminals/p' | grep -v '^--'; }
windows() { win_part | grep '  name ' | sed -E 's/window [0-9]+ +//; s/seen \[.*\]//' | sort; }
layouts() { win_part | grep -v '  name ' | sort; }
terms() { j status -v | sed -n '/^-- terminals/,/^-- w/p' | grep pid | sed -E 's/shown by \[[^]]*\]//' | sort; }

Xvfb "$DISP" -screen 0 1300x950x24 >/dev/null 2>&1 & pids+=($!)
sleep 1
j debug on >/dev/null
"$JOT" >/dev/null 2>&1 & pids+=($!); apid=$!
until_ 'xdotool search --onlyvisible --pid $apid >/dev/null 2>&1'
sleep 1.5
A=$(xdotool search --onlyvisible --pid $apid | tail -1)
xdotool windowsize "$A" 900 450 windowmove "$A" 0 0
key "$A" alt+v
key "$A" ctrl+shift+n
until_ '[ "$(windows | grep -c .)" -ge 2 ]'
sleep 1
echo "before:"; { windows; layouts; terms; } | sed "s/^/  /"
before=$(windows; layouts; terms)
geo=$(xdotool getwindowgeometry "$A" | grep -v Window)
# The palette's Restart jot (it runs jot restart --all).
key "$A" ctrl+p
xdotool type --delay 30 "restart jot"; sleep 0.5
key "$A" Return
sleep 5
echo "after:"; { windows; layouts; terms; } | sed "s/^/  /"
after=$(windows; layouts; terms)
check "the same windows, with the same terminals" '[ "$before" = "$after" ]'
check "each window reopened (in the debug log)" '[ "$(grep -o -- "--again [0-9a-f]*" "$T/s/jot/debug.log" | sort -u | wc -l)" = 2 ]'
check "window A's process is the same one, started again (exec)" 'kill -0 $apid && grep -q "\[$apid\] debug on (.*--again" "$T/s/jot/debug.log"'
A2=$(xdotool search --onlyvisible --pid $apid | tail -1)
check "window A where it was, its size kept" '[ "$(xdotool getwindowgeometry "$A2" | grep -v Window)" = "$geo" ]'
[ -n "${SHOT:-}" ] && import -window root "$SHOT"
[ -n "${VERBOSE:-}" ] && j status -v
exit $fail
