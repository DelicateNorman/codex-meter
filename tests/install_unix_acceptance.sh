#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: install_unix_acceptance.sh ASSET_DIR ASSET_NAME OLD_BINARY" >&2
    exit 2
fi

asset_dir="$(cd "$1" && pwd)"
asset_name="$2"
old_binary="$(cd "$(dirname "$3")" && pwd)/$(basename "$3")"
repository_root="$(cd "$(dirname "$0")/.." && pwd)"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

bin_dir="$test_root/bin"
history_home="$test_root/history"
sessions="$test_root/sessions"
os_home="$test_root/os-home"
mkdir -p "$bin_dir" "$os_home"
cp "$old_binary" "$bin_dir/codex-meter"
chmod 0755 "$bin_dir/codex-meter"

python3 "$repository_root/tests/release_history_guard.py" seed \
    --binary "$bin_dir/codex-meter" --home "$history_home" --sessions "$sessions"
python3 "$repository_root/tests/release_history_guard.py" manifest \
    --home "$history_home" --output "$test_root/history-before.json"

old_hash="$(shasum -a 256 "$bin_dir/codex-meter" | awk '{print $1}')"
new_hash="$(shasum -a 256 "$asset_dir/$asset_name" | awk '{print $1}')"

HOME="$os_home" CODEX_METER_HOME="$history_home" \
CODEX_METER_BASE_URL="$asset_dir" CODEX_METER_BIN_DIR="$bin_dir" \
    sh -s -- < "$repository_root/install.sh"

test "$(shasum -a 256 "$bin_dir/codex-meter" | awk '{print $1}')" = "$new_hash"
test "$(shasum -a 256 "$bin_dir/codex-meter.previous" | awk '{print $1}')" = "$old_hash"
python3 "$repository_root/tests/release_history_guard.py" manifest \
    --home "$history_home" --expect "$test_root/history-before.json"

mkdir "$test_root/tampered"
cp "$asset_dir/$asset_name" "$test_root/tampered/$asset_name"
cp "$asset_dir/SHA256SUMS" "$test_root/tampered/SHA256SUMS"
printf '\000' >> "$test_root/tampered/$asset_name"
if HOME="$os_home" CODEX_METER_HOME="$history_home" \
   CODEX_METER_BASE_URL="$test_root/tampered" CODEX_METER_BIN_DIR="$bin_dir" \
       sh -s -- < "$repository_root/install.sh" >/dev/null 2>&1; then
    echo "tampered release unexpectedly installed" >&2
    exit 1
fi
test "$(shasum -a 256 "$bin_dir/codex-meter" | awk '{print $1}')" = "$new_hash"
python3 "$repository_root/tests/release_history_guard.py" manifest \
    --home "$history_home" --expect "$test_root/history-before.json"

HOME="$os_home" CODEX_METER_HOME="$history_home" CODEX_METER_BIN_DIR="$bin_dir" \
    sh -s -- --rollback < "$repository_root/install.sh"
test "$(shasum -a 256 "$bin_dir/codex-meter" | awk '{print $1}')" = "$old_hash"
test "$(shasum -a 256 "$bin_dir/codex-meter.previous" | awk '{print $1}')" = "$new_hash"
python3 "$repository_root/tests/release_history_guard.py" manifest \
    --home "$history_home" --expect "$test_root/history-before.json"

HOME="$os_home" CODEX_METER_HOME="$history_home" \
CODEX_METER_BASE_URL="$asset_dir" CODEX_METER_BIN_DIR="$bin_dir" \
    sh -s -- < "$repository_root/install.sh" >/dev/null
python3 "$repository_root/tests/release_history_guard.py" database \
    --home "$history_home" --output "$test_root/database-before.json"
"$bin_dir/codex-meter" --home "$history_home" --no-color summary --period all >/dev/null
"$bin_dir/codex-meter" --home "$history_home" --no-color history --group month >/dev/null
"$bin_dir/codex-meter" --home "$history_home" export --format json >/dev/null
"$bin_dir/codex-meter" --home "$history_home" doctor >/dev/null
python3 "$repository_root/tests/release_history_guard.py" database \
    --home "$history_home" --expect "$test_root/database-before.json"

echo "Unix installer upgrade/checksum/rollback/history acceptance passed for $asset_name"
