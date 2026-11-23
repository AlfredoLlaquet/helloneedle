#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# A private X server, desktop session bus and window manager for the native XCB tests.
set -eu
runtime=$(mktemp -d)
export XDG_RUNTIME_DIR="$runtime"
Xvfb -screen 0 1920x1500x24 -displayfd 3 -nolisten tcp 3>"$runtime/display" &
server=$!
trap 'kill "$server" 2>/dev/null || :; rm -rf "$runtime"' EXIT
attempts=0
while [ ! -s "$runtime/display" ]; do
    kill -0 "$server"
    attempts=$((attempts + 1))
    [ "$attempts" -le 30 ] || exit 1
    sleep 1
done
export DISPLAY=":$(cat "$runtime/display")" QT_QPA_PLATFORM=xcb
dbus-run-session -- sh -ec '
    openbox >"$XDG_RUNTIME_DIR/window-manager.log" 2>&1 &
    wm=$!
    trap "kill $wm 2>/dev/null || :" EXIT
    "$@"
' sh "$@"
