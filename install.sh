#!/bin/sh
set -eu

repository="DelicateNorman/codex-meter"
version="${CODEX_METER_VERSION:-v0.15.0}"
bin_dir="${CODEX_METER_BIN_DIR:-$HOME/.local/bin}"
rollback=0

case "${1:-}" in
    "") ;;
    --rollback) rollback=1 ;;
    -h|--help)
        echo "Usage: install.sh [--rollback]"
        echo "  --rollback  swap codex-meter with the previous installed version"
        exit 0
        ;;
    *)
        echo "Unknown option: $1" >&2
        echo "Usage: install.sh [--rollback]" >&2
        exit 2
        ;;
esac

destination="$bin_dir/codex-meter"
previous="$destination.previous"

if [ "$rollback" -eq 1 ]; then
    if [ ! -f "$previous" ]; then
        echo "No previous codex-meter installation is available to restore." >&2
        exit 1
    fi
    mkdir -p "$bin_dir"
    swap="$bin_dir/.codex-meter.rollback.$$"
    if [ -f "$destination" ]; then
        mv "$destination" "$swap"
    fi
    if ! mv "$previous" "$destination"; then
        if [ -f "$swap" ]; then mv "$swap" "$destination"; fi
        echo "Could not restore the previous codex-meter installation." >&2
        exit 1
    fi
    chmod 0755 "$destination"
    if ! "$destination" --version; then
        mv "$destination" "$previous"
        if [ -f "$swap" ]; then mv "$swap" "$destination"; fi
        echo "The previous codex-meter installation failed its self-check; rollback was cancelled." >&2
        exit 1
    fi
    if [ -f "$swap" ]; then
        mv "$swap" "$previous"
    fi
    echo "Restored the previous installation at $destination"
    echo "Existing usage data under ~/.codex-meter was not changed."
    exit 0
fi

case "$(uname -s)" in
    Linux) platform="linux" ;;
    Darwin) platform="macos" ;;
    *)
        echo "This installer supports Linux and macOS. On Windows, use install.ps1." >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64|amd64) architecture="x86_64" ;;
    arm64|aarch64) architecture="arm64" ;;
    *)
        echo "Unsupported CPU architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

if [ "$platform" = "linux" ] && [ "$architecture" != "x86_64" ]; then
    echo "The standalone Linux release currently supports x86_64; use the source-build guide on this machine." >&2
    exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
    echo "curl is required." >&2
    exit 1
fi

asset="codex-meter-$platform-$architecture"
base_url="${CODEX_METER_BASE_URL:-https://github.com/$repository/releases/download/$version}"
temporary_dir="$(mktemp -d)"
trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

echo "Downloading codex-meter $version for $platform/$architecture..."
if [ -d "$base_url" ]; then
    cp "$base_url/$asset" "$temporary_dir/$asset"
    cp "$base_url/SHA256SUMS" "$temporary_dir/SHA256SUMS"
else
    curl --fail --location --silent --show-error "$base_url/$asset" --output "$temporary_dir/$asset"
    curl --fail --location --silent --show-error "$base_url/SHA256SUMS" --output "$temporary_dir/SHA256SUMS"
fi

expected="$(awk -v name="$asset" '$2 == name { print $1 }' "$temporary_dir/SHA256SUMS")"
if [ -z "$expected" ]; then
    echo "Release checksum does not contain $asset." >&2
    exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$temporary_dir/$asset" | awk '{print $1}')"
else
    actual="$(shasum -a 256 "$temporary_dir/$asset" | awk '{print $1}')"
fi
if [ "$actual" != "$expected" ]; then
    echo "Checksum verification failed for $asset." >&2
    exit 1
fi

mkdir -p "$bin_dir"
chmod 0755 "$temporary_dir/$asset"
if ! "$temporary_dir/$asset" --version >/dev/null; then
    echo "The downloaded codex-meter binary failed its self-check." >&2
    exit 1
fi

staged="$(mktemp "$bin_dir/.codex-meter.new.XXXXXX")"
cp "$temporary_dir/$asset" "$staged"
chmod 0755 "$staged"
had_previous=0
if [ -f "$destination" ]; then
    mv "$destination" "$previous"
    had_previous=1
fi
if ! mv "$staged" "$destination"; then
    if [ "$had_previous" -eq 1 ] && [ -f "$previous" ]; then
        mv "$previous" "$destination"
    fi
    echo "Could not install codex-meter." >&2
    exit 1
fi

if ! "$destination" --version; then
    if [ "$had_previous" -eq 1 ] && [ -f "$previous" ]; then
        mv "$previous" "$destination"
    else
        rm -f "$destination"
    fi
    echo "The new codex-meter failed its self-check; the previous installation was restored." >&2
    exit 1
fi

echo "Installed to $destination"
if [ "$had_previous" -eq 1 ]; then
    echo "Previous version saved at $previous"
    echo "Rollback command: sh install.sh --rollback"
fi
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to PATH, then run: codex-meter" ;;
esac
echo "Existing usage data under ~/.codex-meter was not changed."
