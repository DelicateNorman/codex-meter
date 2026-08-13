#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "Usage: desktop_first_launch.sh /path/to/codex-meter-desktop" >&2
    exit 2
fi

binary=$1
test -x "$binary"
test_root=$(mktemp -d "${TMPDIR:-/tmp}/codex-meter-desktop-first-launch.XXXXXX")
state_dir="$test_root/meter-home"
codex_dir="$test_root/codex-home"
log_file="$test_root/desktop.log"
mkdir -p "$codex_dir/sessions"

cleanup() {
    if [ "${desktop_pid:-}" ]; then
        kill "$desktop_pid" 2>/dev/null || true
        wait "$desktop_pid" 2>/dev/null || true
    fi
    rm -rf "$test_root"
}
trap cleanup EXIT HUP INT TERM

CODEX_METER_HOME="$state_dir" CODEX_HOME="$codex_dir" "$binary" >"$log_file" 2>&1 &
desktop_pid=$!

attempt=0
while [ "$attempt" -lt 15 ]; do
    if [ -f "$state_dir/config.toml" ] && [ -f "$state_dir/meter.db" ]; then
        break
    fi
    if ! kill -0 "$desktop_pid" 2>/dev/null; then
        cat "$log_file" >&2
        echo "Desktop app exited before first-launch state was initialized." >&2
        exit 1
    fi
    attempt=$((attempt + 1))
    sleep 1
done

test -f "$state_dir/config.toml"
test -f "$state_dir/meter.db"
grep -F 'store_prompt = false' "$state_dir/config.toml"
sqlite3 "$state_dir/meter.db" 'PRAGMA integrity_check;' | grep -Fx ok
