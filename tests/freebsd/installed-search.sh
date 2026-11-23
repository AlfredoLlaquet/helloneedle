#!/bin/sh
# CG4FBS BEGIN ADDITION — Exercise the installed application on an isolated desktop.
set -eu

work=$(mktemp -d)
server=
cleanup() {
    if [ -n "$server" ]; then
        kill "$server" 2>/dev/null || :
        wait "$server" 2>/dev/null || :
    fi
    rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

export XDG_CONFIG_HOME="$work/config" XDG_DATA_HOME="$work/data"
export XDG_CACHE_HOME="$work/cache" XDG_STATE_HOME="$work/state"
export XDG_RUNTIME_DIR="$work/runtime" GSETTINGS_BACKEND=memory
language=${1:-en}
case "$language" in
    en) export LC_ALL=en_US.UTF-8 ;;
    es) export LC_ALL=es_ES.UTF-8 ;;
    *) echo "Usage: $0 [en|es]" >&2; exit 2 ;;
esac
export LANGUAGE="$language" GDK_BACKEND=x11 GSK_RENDERER=cairo GTK_A11Y=atspi
mkdir -m 700 "$XDG_RUNTIME_DIR"
mkdir "$work/documents"

Xvfb -screen 0 1680x1050x24 -nolisten tcp -displayfd 1 > "$work/display" &
server=$!
tries=0
while [ ! -s "$work/display" ]; do
    kill -0 "$server"
    tries=$((tries + 1))
    [ "$tries" -lt 100 ] || { echo 'Xvfb did not become ready' >&2; exit 1; }
    sleep 0.1
done
export DISPLAY=":$(cat "$work/display")"
dbus-run-session -- python3 "$(dirname "$0")/installed-search.py" "$work/documents" "$language"
# CG4FBS END
