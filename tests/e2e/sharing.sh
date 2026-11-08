#!/usr/bin/env bash
# End to end: real jot windows sharing workspaces, on a display of their own
# (Xvfb), with a second machine ("fakehost": a session of its own, reached
# through a stand-in ssh that runs `jot bridge` for it). Keys are typed with
# xdotool; a click on a terminal's row in the sidebar is `jot click`, which
# does what the click does. What the windows and sessions hold is read with
# `jot list -v` and `jot status -v`.
#
#   tests/e2e/sharing.sh           (after cargo build --release)
#   JOT=path/to/jot DISP=:79 tests/e2e/sharing.sh
#
# Needs Xvfb and xdotool. Touches nothing outside its own folder: its
# sessions are its own (XDG_RUNTIME_DIR, XDG_STATE_HOME), and it ends only
# the processes it started.

set -u
here=$(cd "$(dirname "$0")/../.." && pwd)
JOT=${JOT:-$here/target/release/jot}
DISP=${DISP:-:79}
T=$(mktemp -d /tmp/jot-e2e-XXXX)
R1=$T/r1 R2=$T/r2
mkdir -p -m700 "$R1" "$R2" "$T/s1/jot" "$T/s2" "$T/cfg" "$T/bin"
echo fakehost >"$T/s1/jot/machines"
cat >"$T/bin/ssh" <<EOF
#!/bin/sh
exec env -u DISPLAY SHELL=/bin/bash XDG_RUNTIME_DIR=$R2 XDG_STATE_HOME=$T/s2 XDG_CONFIG_HOME=$T/cfg $JOT bridge
EOF
chmod +x "$T/bin/ssh"

pids=()
fail=0
# Its windows, display and sessions ended; its folder gone, unless a check
# failed (its logs are there).
cleanup() {
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done
    sleep 0.5
    XDG_RUNTIME_DIR=$R1 "$JOT" kill >/dev/null 2>&1
    XDG_RUNTIME_DIR=$R2 "$JOT" kill >/dev/null 2>&1
    [ $fail = 0 ] && rm -rf "$T"
}
trap cleanup EXIT

export DISPLAY=$DISP SHELL=/bin/bash XDG_CONFIG_HOME=$T/cfg PATH=$T/bin:$PATH
here1() { env XDG_RUNTIME_DIR="$R1" XDG_STATE_HOME="$T/s1" "$JOT" "$@"; }
there() { env XDG_RUNTIME_DIR="$R2" XDG_STATE_HOME="$T/s2" "$JOT" "$@"; }

check() {
    if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}
# Until a condition holds, five seconds at most.
until_() {
    for _ in $(seq 50); do eval "$1" && return 0; sleep 0.1; done
    return 1
}
# The first workspace's panes, as `jot list -v` gives them: "M:T" each.
panes1() { here1 list -v | awk '/^-- workspaces/{w=1;next} w&&/^2 |^-- /{exit} w&&/terminal/{for(i=1;i<=NF;i++) if($i=="terminal") print $(i+1)}'; }
# The session's shared workspaces, as jot status -v gives them.
shared() { here1 status -v | sed -n "/^-- shared workspaces/,/^-- windows/p"; }
free_on() { here1 list -v | awk -v m="$1:" '/^-- terminals in no pane here/{f=1;next} /^-- /{f=0} f&&index($1,m)==1{print $1}' | head -1; }
key() { xdotool windowfocus --sync "$1" key "$2"; sleep 0.8; }

Xvfb "$DISP" -screen 0 1300x950x24 >/dev/null 2>&1 & pids+=($!)
sleep 1
there debug on >/dev/null; here1 debug on >/dev/null
env XDG_RUNTIME_DIR="$R2" XDG_STATE_HOME="$T/s2" "$JOT" >/dev/null 2>&1 & pids+=($!); cpid=$!
sleep 3
env XDG_RUNTIME_DIR="$R1" XDG_STATE_HOME="$T/s1" "$JOT" >/dev/null 2>&1 & pids+=($!); apid=$!
until_ 'here1 list -v 2>/dev/null | grep -q "fakehost  reached, shares"'
A=$(xdotool search --pid $apid | tail -1)
C=$(xdotool search --pid $cpid | tail -1)
xdotool windowsize "$A" 1300 450 windowmove "$A" 0 0
xdotool windowsize "$C" 1300 420 windowmove "$C" 0 500

check "the other machine is reached, and its session shares" 'here1 list -v | grep -q "fakehost  reached, shares"'

key "$A" alt+v
check "a split here: two panes, both this machine's" '[ "$(panes1 | grep -c "^0:")" = 2 ]'

# A terminal of fakehost's in no pane: Alt N in its own window pushes one out.
key "$C" alt+n
until_ '[ -n "$(free_on 1)" ]'
remote=$(free_on 1)
check "a terminal of the other machine's, in no pane: $remote" '[ -n "$remote" ]'

# The bug: clicked into the second pane, the workspace was dropped.
xdotool windowfocus --sync "$A"
here1 click "$remote"
until_ '[ "$(panes1 | tail -1)" = "$remote" ]'
check "clicked into the split's second pane, the split stays: $(panes1 | tr '\n' ' ')" '[ "$(panes1 | wc -l)" = 2 ] && [ "$(panes1 | tail -1)" = "$remote" ]'
check "the workspace is shared, the remote terminal in it" 'shared | grep -q "pfakehost:${remote#1:}"'

key "$A" ctrl+shift+n
until_ '[ "$(here1 status -v | grep -c "pfakehost:${remote#1:}")" -ge 3 ]'
check "a new window has it as well, the remote terminal in it" '[ "$(here1 status -v | grep -c "(shown)  a(p[0-9]*,pfakehost:${remote#1:})")" = 2 ]'

# Alt N on the remote pane of a workspace here: a terminal here.
key "$A" alt+n
until_ '[ "$(panes1 | tail -1 | cut -d: -f1)" = 0 ]'
check "Alt N on the remote pane gives a terminal of this machine's" '[ "$(panes1 | tail -1 | cut -d: -f1)" = 0 ]'
until_ '[ "$(here1 status -v | grep -c "(shown)  a(p[0-9]*,p[0-9]*)")" = 2 ]'
check "and both windows show it" '[ "$(here1 status -v | grep -c "(shown)  a(p[0-9]*,p[0-9]*)")" = 2 ]'

# Alt . steps through this machine's terminals only.
only_here=1
for _ in 1 2 3 4; do
    key "$A" alt+period
    [ "$(panes1 | tail -1 | cut -d: -f1)" = 0 ] || only_here=0
done
check "Alt . steps through this machine's terminals only" '[ $only_here = 1 ]'

# Mixed, then both sessions restarted: as it was.
remote=$(free_on 1)
here1 click "$remote"
until_ '[ "$(panes1 | tail -1)" = "$remote" ]'
here1 restart >/dev/null
there restart >/dev/null
sleep 5
check "after jot restart of both sessions, the split with the remote terminal is there" '[ "$(panes1 | wc -l)" = 2 ] && [ "$(panes1 | tail -1)" = "$remote" ]'
check "and still shared" 'shared | grep -q "pfakehost:${remote#1:}"'

if [ $fail = 0 ]; then echo "all passed"; else echo "some FAILED (logs kept: $T/s1/jot/debug.log, $T/s2/jot/debug.log)"; fi
exit $fail
