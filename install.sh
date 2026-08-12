#!/bin/sh
set -eu

repository="DelicateNorman/codex-meter"
version="${CODEX_METER_VERSION:-v0.11.2}"
install_root="${CODEX_METER_INSTALL_ROOT:-$HOME/.local/share/codex-meter}"
bin_dir="${CODEX_METER_BIN_DIR:-$HOME/.local/bin}"
python_command="${PYTHON:-python3}"

if [ "$(uname -s)" != "Linux" ]; then
    echo "codex-meter's one-line installer currently supports Linux only." >&2
    echo "macOS and Windows support is planned; see the source-build guide for development use." >&2
    exit 1
fi

if ! command -v "$python_command" >/dev/null 2>&1; then
    echo "Python 3.11 or newer is required." >&2
    exit 1
fi

if ! "$python_command" -c 'import sys; raise SystemExit(sys.version_info < (3, 11))'; then
    echo "Python 3.11 or newer is required." >&2
    exit 1
fi

if ! command -v curl >/dev/null 2>&1 || ! command -v tar >/dev/null 2>&1; then
    echo "curl and tar are required." >&2
    exit 1
fi

temporary_dir="$(mktemp -d)"
trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

archive="$temporary_dir/codex-meter.tar.gz"
source_url="${CODEX_METER_SOURCE_URL:-https://github.com/$repository/archive/refs/tags/$version.tar.gz}"
echo "Downloading codex-meter $version..."
curl --fail --location --silent --show-error "$source_url" --output "$archive"
tar -xzf "$archive" -C "$temporary_dir"

source_dir="$temporary_dir/codex-meter-${version#v}"
if [ ! -d "$source_dir/codex_meter" ]; then
    echo "Downloaded archive does not contain codex_meter." >&2
    exit 1
fi

stage="$temporary_dir/site"
mkdir -p "$stage"
cp -R "$source_dir/codex_meter" "$stage/codex_meter"

mkdir -p "$install_root" "$bin_dir"
rm -rf "$install_root/site.next"
mv "$stage" "$install_root/site.next"
rm -rf "$install_root/site"
mv "$install_root/site.next" "$install_root/site"

python_path="$(command -v "$python_command")"
launcher="$temporary_dir/codex-meter"
cat >"$launcher" <<EOF
#!/bin/sh
CODEX_METER_INSTALL_SITE='$install_root/site' exec '$python_path' -c 'import os, sys; sys.path.insert(0, os.environ["CODEX_METER_INSTALL_SITE"]); from codex_meter.cli import main; raise SystemExit(main())' "\$@"
EOF
chmod 0755 "$launcher"
mv "$launcher" "$bin_dir/codex-meter"

"$bin_dir/codex-meter" --version
echo "Installed to $bin_dir/codex-meter"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to PATH, then run: codex-meter" ;;
esac
echo "Existing usage data under ~/.codex-meter was not changed."
